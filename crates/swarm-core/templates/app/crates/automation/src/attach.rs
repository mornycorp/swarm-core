//! `{{cli}} mcp --attach <port>`: MCP whose commands run on an app that is already running (the
//! windowed app with `--socket`, or `{{cli}} serve --socket`), over that app's loopback control
//! socket, rather than on a session of its own. [`Attached`] is the socket side of MCP's
//! [`Backend`]: the MCP code ([`mcp::serve_on`](crate::mcp::serve_on)) is the one a direct server
//! runs, so the tools, the answers and the error mapping are the same. It names no command.
//!
//! ```text
//! --attach <port> [--token-file <path>]
//! ```
//!
//! - **A frame client.** It connects to `127.0.0.1:<port>` (loopback only: there is no host), sends
//!   `auth` with the token, then `commands` once, then one `<id>` frame per call, reading each reply
//!   with the capped line reader ([`lines`](crate::lines)): a reply over [`MAX_REPLY`] is
//!   `too_large`, and a request over [`MAX_REQUEST`] is `too_large` without being sent.
//! - **The token** is the server's: `{{env}}_CONTROL_TOKEN` wins, else the file the app was started
//!   with ([`Token::read_existing`]: only read, never made). It is read again at each connection (on
//!   Windows the app writes a new one at every start), and never shown.
//! - **Consent is not widened.** MCP's client grants nothing, so a command whose effects need consent
//!   answers `consent_required`, as on a direct server, even when the app's socket was started with
//!   `--yes`: the decision is the command's spec's ([`Spec::needs_consent`], from its effects as
//!   `commands` gives them), made here before the call is sent, so the app never sees such a call.
//!   (A direct server checks the params and whether the command is enabled first; here such a call
//!   is refused before either, so its code may be `consent_required` where a direct server's is
//!   `bad_params` or `unavailable`. It never runs on either.)
//! - **Deadlines, no hang.** Connecting and the handshake have [`CONNECT_TIMEOUT`]; each reply has
//!   [`REPLY_TIMEOUT`] (a call that passes it is `cancelled`: the connection is closed, which cancels
//!   the call in the app). Nothing here waits without one.
//! - **Typed failures, no loop.** No app on the port, a wrong token and a connection that drops are
//!   each a [`CommandError`] (`unavailable`, `unauthenticated`) with what to do, which MCP answers as
//!   a JSON-RPC error. A connection the app has closed since the last call (it was idle, or the app
//!   restarted) is seen before the next call is sent and made again, once; a connection that drops
//!   while a call is out is reported, not retried, as the call may have run.

use std::io::{self, BufReader, Read};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use {{crate_prefix_ident}}engine::{AppInfo, CommandError, ErrorCode, clip};

use crate::Options;
use crate::frame::Id;
use crate::limits::{MAX_REPLY, MAX_REQUEST};
use crate::lines::{Line, read_line, write_line};
use crate::mcp::{Backend, Outcome, Spec};
use crate::token::{ENV, Token};

/// Connecting, `auth` and `commands` must be done within this.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A call's reply must arrive whole within this.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// What the flags after `mcp` asked for: `--attach <port> [--token-file <path>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachArgs {
    pub port: u16,
    pub token_file: Option<String>,
}

/// The flags `args` hold, or what is wrong with them: a port from 1 to 65535, and a token, from
/// `{{env}}_CONTROL_TOKEN` or `--token-file`.
pub fn parse(args: &[&str]) -> Result<AttachArgs, String> {
    let (mut port, mut token_file) = (None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--attach" => {
                let given = rest.next().ok_or("`--attach` needs the port of the app's control socket")?;
                let parsed = given.parse::<u16>().ok().filter(|p| *p != 0);
                port = Some(parsed.ok_or_else(|| format!("`--attach` takes a port from 1 to 65535, not `{}`", clip(given)))?);
            }
            "--token-file" => token_file = Some(rest.next().ok_or("`--token-file` needs a path")?.to_string()),
            other => return Err(format!("no option `{}` here (`mcp` takes --attach <port> and --token-file <path>)", clip(other))),
        }
    }
    let port = port.ok_or("`--token-file` goes with `--attach <port>`")?;
    if token_file.is_none() && !Token::in_env() {
        return Err(format!("`mcp --attach` needs the app's token: set {ENV}, or pass `--token-file <path>` naming the file the app was started with"));
    }
    Ok(AttachArgs { port, token_file })
}

/// How an [`Attached`] backend reaches the app.
#[derive(Debug, Clone)]
pub struct Config {
    /// The port of the app's control socket on `127.0.0.1`.
    pub port: u16,
    /// The token file, when `{{env}}_CONTROL_TOKEN` doesn't hold the token.
    pub token_file: Option<PathBuf>,
    /// What `initialize` says the server is: this binary.
    pub app: AppInfo,
    /// What the MCP client has granted: no consent, by default.
    pub options: Options,
    pub connect_timeout: Duration,
    pub reply_timeout: Duration,
}

impl Config {
    /// The app on `args`' port, with the default deadlines and no consent.
    pub fn new(args: &AttachArgs, app: AppInfo) -> Config {
        Config {
            port: args.port,
            token_file: args.token_file.as_ref().map(PathBuf::from),
            app,
            options: Options::default(),
            connect_timeout: CONNECT_TIMEOUT,
            reply_timeout: REPLY_TIMEOUT,
        }
    }

    fn addr(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port))
    }
}

/// MCP's backend on a running app: its control socket, connected when first needed.
#[derive(Debug)]
pub struct Attached {
    config: Config,
    conn: Option<Conn>,
}

impl Attached {
    pub fn new(config: Config) -> Attached {
        Attached { config, conn: None }
    }

    /// The connection a call goes out on: the one there is while the app still holds it, else a new
    /// one (made once: a failure is the caller's answer, never a retry).
    fn ready(&mut self) -> Result<&mut Conn, CommandError> {
        if let Some(conn) = self.conn.take().filter(Conn::held) {
            return Ok(self.conn.insert(conn));
        }
        let conn = Conn::open_to(&self.config)?;
        Ok(self.conn.insert(conn))
    }

    /// Closes the connection (the app then cancels a call still running on it); the next call
    /// makes a new one.
    fn drop_conn(&mut self) {
        if let Some(conn) = self.conn.take() {
            let _ = conn.writer.shutdown(Shutdown::Both);
        }
    }
}

impl Backend for Attached {
    fn app(&self) -> &AppInfo {
        &self.config.app
    }

    fn specs(&mut self) -> Result<Vec<Spec>, CommandError> {
        Ok(self.ready()?.specs.clone())
    }

    fn call(&mut self, id: &str, params: Value) -> Result<Outcome, CommandError> {
        let (consent, port, wait) = (self.config.options.consent, self.config.port, self.config.reply_timeout);
        let conn = self.ready()?;
        // The registry is the app's: an id it has no spec for is no command there.
        let Some(spec) = conn.specs.iter().find(|spec| spec.id == id) else { return Ok(Err(CommandError::unknown_command(id))) };
        if spec.needs_consent() && !consent {
            return Ok(Err(CommandError::consent_required(id)));
        }
        let answer = conn.ask(id, Some(&params), wait, port);
        if matches!(answer, Answer::Lost(_)) {
            self.drop_conn();
        }
        answer.into_outcome()
    }
}

/// A reader that gives up at its deadline, however the peer sends: a reply that trickles in can't
/// hold the bridge past it.
#[derive(Debug)]
struct Deadline {
    stream: TcpStream,
    until: Instant,
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf)
    }
}

/// One authenticated connection, and the specs `commands` gave on it.
#[derive(Debug)]
struct Conn {
    reader: BufReader<Deadline>,
    writer: TcpStream,
    specs: Vec<Spec>,
    /// The id of the next request.
    next: u64,
}

/// A request as the frame has it.
#[derive(Debug, Serialize)]
struct Request<'a> {
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a Value>,
}

/// A reply as the frame has it.
#[derive(Debug, Deserialize)]
struct Reply {
    id: Option<Id>,
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    remediation: String,
    #[serde(default)]
    key: Option<String>,
}

/// What came back for a request.
#[derive(Debug)]
enum Answer {
    /// The app answered: its result or its failure.
    Replied(Outcome),
    /// No answer: the connection is gone (dropped, timed out, or speaking something else), and
    /// this is why.
    Lost(CommandError),
}

impl Answer {
    fn into_outcome(self) -> Result<Outcome, CommandError> {
        match self {
            Answer::Replied(outcome) => Ok(outcome),
            Answer::Lost(e) => Err(e),
        }
    }
}

/// What to do when the app isn't there.
fn start_the_app(port: u16) -> String {
    format!(
        "start the app with `--socket` (or `{{cli}} serve --socket`) and pass the port its start-up line names (`\"listening\": \"127.0.0.1:<port>\"`) to `--attach`; nothing answers on {port} now"
    )
}

/// The connection to the app at `port` dropped while a request was out.
fn dropped(port: u16, why: &str) -> CommandError {
    CommandError::unavailable(
        format!("the connection to the app on 127.0.0.1:{port} closed before it answered ({why})"),
        "check that the app is still running with its socket, then call again: the call may have run before the connection closed",
    )
}

/// A failure the frame's reply carries.
fn failure(reply: Reply) -> CommandError {
    let Some(code) = ErrorCode::ALL.into_iter().find(|c| c.as_str() == reply.code) else {
        return CommandError::internal(format!("the app failed with a code this binary doesn't know: `{}`: {}", clip(&reply.code), clip(&reply.error)));
    };
    let e = CommandError::new(code, reply.error, reply.remediation);
    match reply.key {
        Some(key) => e.with_key(key),
        None => e,
    }
}

impl Conn {
    /// Connects to the app, authenticates and reads its specs, all within the connect deadline.
    fn open_to(config: &Config) -> Result<Conn, CommandError> {
        let (port, deadline) = (config.port, config.connect_timeout);
        let token = Token::read_existing(config.token_file.as_deref())
            .map_err(|e| CommandError::new(e.code, e.message, format!("{} (the token of the app on port {port})", e.remediation)))?;
        let stream = TcpStream::connect_timeout(&config.addr(), deadline).map_err(|e| {
            let what = if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) { "timed out".to_owned() } else { e.to_string() };
            CommandError::unavailable(format!("cannot connect to the app on 127.0.0.1:{port}: {what}"), start_the_app(port))
        })?;
        let write_wait = config.reply_timeout.max(deadline);
        let setup = stream.set_nodelay(true).and_then(|()| stream.set_write_timeout(Some(write_wait))).and_then(|()| stream.try_clone());
        let writer = setup.map_err(|e| dropped(port, &e.to_string()))?;
        let mut conn = Conn { reader: BufReader::new(Deadline { stream, until: Instant::now() }), writer, specs: Vec::new(), next: 0 };
        conn.handshake(&token, config)?;
        Ok(conn)
    }

    /// `auth`, then `commands`.
    fn handshake(&mut self, token: &Token, config: &Config) -> Result<(), CommandError> {
        let port = config.port;
        let auth = serde_json::json!({ "token": token.expose() });
        match self.ask("auth", Some(&auth), config.connect_timeout, port) {
            Answer::Replied(Ok(_)) => {}
            Answer::Replied(Err(e)) if e.code == ErrorCode::Unauthenticated => {
                return Err(CommandError::unauthenticated(
                    format!("the app on 127.0.0.1:{port} refused the token"),
                    format!(
                        "check the token: {ENV}, or `--token-file` naming the file the app was started with (on Windows the app writes a new token at every start: attach after it has started)"
                    ),
                ));
            }
            // Another refusal (every connection in use) says what to do itself.
            Answer::Replied(Err(e)) | Answer::Lost(e) => return Err(e),
        }
        let commands = self.ask("commands", None, config.connect_timeout, port).into_outcome()??;
        self.specs = Spec::list(commands)?;
        Ok(())
    }

    /// Whether the app still holds the connection: nothing unread, and no end or reset waiting. A
    /// look that doesn't wait.
    fn held(&self) -> bool {
        if !self.reader.buffer().is_empty() || self.writer.set_nonblocking(true).is_err() {
            return false;
        }
        let looked = self.writer.peek(&mut [0_u8; 1]);
        let back = self.writer.set_nonblocking(false).is_ok();
        back && matches!(looked, Err(e) if e.kind() == io::ErrorKind::WouldBlock)
    }

    /// Sends `method` with `params` and reads its reply, within `wait`.
    fn ask(&mut self, method: &str, params: Option<&Value>, wait: Duration, port: u16) -> Answer {
        let id = self.next;
        self.next += 1;
        let line = match serde_json::to_string(&Request { id, method, params }) {
            Ok(line) if line.len() <= MAX_REQUEST => line,
            Ok(_) => return Answer::Replied(Err(CommandError::too_large(format!("the request is over the {MAX_REQUEST}-byte cap"), "send a smaller request"))),
            Err(e) => return Answer::Replied(Err(CommandError::internal(format!("the request is not JSON: {e}")))),
        };
        if let Err(e) = write_line(&mut self.writer, &line) {
            return Answer::Lost(dropped(port, &e.to_string()));
        }
        self.reader.get_mut().until = Instant::now() + wait;
        let text = match read_line(&mut self.reader, MAX_REPLY) {
            Ok(Line::Text(text)) => text,
            Ok(Line::TooLarge) => {
                let message = format!("the reply is over the {MAX_REPLY}-byte cap");
                return Answer::Replied(Err(CommandError::too_large(message, "narrow what the call asks for, or ask for less at a time")));
            }
            Ok(Line::NotUtf8) => return Answer::Lost(dropped(port, "the reply was not UTF-8")),
            Ok(Line::Eof) => return Answer::Lost(dropped(port, "the app closed it")),
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {
                let message = format!("the app on 127.0.0.1:{port} didn't answer `{}` within {} s", clip(method), wait.as_secs_f32());
                return Answer::Lost(CommandError::new(
                    ErrorCode::Cancelled,
                    message,
                    "call again if the result is still wanted: the connection is closed, which cancels a call still running in the app",
                ));
            }
            Err(e) => return Answer::Lost(dropped(port, &e.to_string())),
        };
        match serde_json::from_str::<Reply>(&text) {
            Ok(reply) if reply.id.is_none() || reply.id == Some(Id::Number(id.into())) => {
                Answer::Replied(if reply.ok { Ok(reply.result) } else { Err(failure(reply)) })
            }
            Ok(_) => Answer::Lost(dropped(port, "the reply was to another request")),
            Err(_) => Answer::Lost(dropped(port, "the reply was not the frame's")),
        }
    }
}

#[cfg(test)]
mod tests;
