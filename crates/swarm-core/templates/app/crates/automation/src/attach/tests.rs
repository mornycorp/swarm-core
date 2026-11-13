use std::io::{BufRead, Cursor};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};
use serde_json::json;

use {{crate_prefix_ident}}engine::{CallContext, Caller, Command, Effects, Session, Why, commands};
use {{crate_prefix_ident}}testkit::DeadPort;

use super::*;
use crate::frame::{self, Reply as FrameReply};
use crate::socket::{self, Server};
use crate::{commands_json, mcp};

const TOKEN: &str = "test-token-not-real-0123456789";

/// Takes no params.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Done {
    done: bool,
}

/// A command with an effect: it counts its runs, so a test sees whether one happened.
struct Writer(Arc<AtomicUsize>);
impl Command for Writer {
    type Params = NoParams;
    type Output = Done;
    const ID: &'static str = "test.write";
    const TITLE: &'static str = "Write";
    const SUMMARY: &'static str = "Pretends to write a file";
    const EFFECTS: Effects = Effects { writes: true, runs_code: false, network: false, device: false };
    fn run(&self, _: &Session, _: NoParams, _: &CallContext) -> Result<Done, CommandError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Done { done: true })
    }
}

/// A command that is never enabled.
struct Off;
impl Command for Off {
    type Params = NoParams;
    type Output = Done;
    const ID: &'static str = "test.off";
    const TITLE: &'static str = "Off";
    const SUMMARY: &'static str = "Never enabled";
    fn enabled(&self, _: &Session) -> Result<(), Why> {
        Err(Why::new("nothing is ready", "make something ready first"))
    }
    fn run(&self, _: &Session, _: NoParams, _: &CallContext) -> Result<Done, CommandError> {
        Ok(Done { done: true })
    }
}

/// The app's commands, a command with an effect and one that is disabled; the writer's runs.
fn session() -> (Session, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut registry = commands::registry().unwrap();
    registry.register(Writer(Arc::clone(&runs))).unwrap();
    registry.register(Off).unwrap();
    (Session::with_registry(AppInfo { name: "demo".into(), version: "0.3.0".into() }, registry), runs)
}

/// A control socket on a fresh session, its connections' consent `consent` (`--yes`).
fn server_with(consent: bool, change: impl FnOnce(&mut socket::Config)) -> (Server, Arc<AtomicUsize>) {
    let (session, runs) = session();
    let mut config = socket::Config::new(Token::parse(TOKEN).unwrap(), Options { consent });
    change(&mut config);
    (Server::start(Arc::new(session), config).unwrap(), runs)
}

/// A private file holding `token`, as the app writes one.
fn token_file(name: &str, token: &str) -> PathBuf {
    let path = {{crate_prefix_ident}}testkit::temp_dir(name).unwrap().join("control.token");
    std::fs::write(&path, format!("{token}\n")).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    path
}

/// A backend on the app at `port`, its token in `file`, with short deadlines.
fn attached(port: u16, file: &Path) -> Attached {
    let args = AttachArgs { port, token_file: Some(file.display().to_string()) };
    let mut config = Config::new(&args, AppInfo { name: "demo".into(), version: "0.3.0".into() });
    config.connect_timeout = Duration::from_secs(5);
    config.reply_timeout = Duration::from_secs(10);
    Attached::new(config)
}

/// The tests take the token from a file: one set in the environment would win over it.
fn env_free() -> bool {
    !Token::in_env()
}

/// What an MCP server answers to `messages`, one reply a line.
fn replies(backend: &mut dyn Backend, messages: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    mcp::serve_on(backend, Cursor::new(format!("{}\n", messages.join("\n"))), &mut out).unwrap();
    String::from_utf8(out).unwrap().lines().map(str::to_owned).collect()
}

fn call(id: u64, tool: &str, arguments: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": arguments}}).to_string()
}

fn parsed(line: &str) -> Value {
    serde_json::from_str(line).unwrap()
}

/// The failure a tool result carries.
fn tool_failure(reply: &Value) -> Value {
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// Every command, with good params, bad ones and none, each tool that is no command, a request over
/// the cap: `(the messages, which of them call a command whose effects need consent with params it
/// refuses)`.
fn script(session: &Session) -> (Vec<String>, Vec<usize>) {
    let mut messages = vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}).to_string(),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string(),
    ];
    let mut refused_for_consent = Vec::new();
    let cases = [json!({}), Value::Null, json!({"conformance_unknown": 1}), json!(5), json!([1]), json!({"selected": "app.info"}), json!({"show_list": 7})];
    for spec in session.commands() {
        for params in &cases {
            let at = messages.len();
            // Whether the app would refuse it even with consent: asked of a session of its own.
            let refused = || session_of_its_own().call(spec.id, params.clone(), &Options { consent: true }.call_opts(Caller::Test)).is_err();
            if spec.effects.needs_consent() && refused() {
                refused_for_consent.push(at);
            }
            messages.push(call(at as u64, &spec.id.replace('.', "-"), params));
        }
    }
    for tool in ["app-nope", "app.info", "", "-", "a-b-c"] {
        messages.push(call(messages.len() as u64, tool, &json!({})));
    }
    messages.push(json!({"jsonrpc": "2.0", "id": "p", "method": "ping"}).to_string());
    messages.push(call(9_999, "app-info", &json!({"padding": "a".repeat(MAX_REQUEST)})));
    (messages, refused_for_consent)
}

fn session_of_its_own() -> Session {
    session().0
}

#[test]
fn attached_mcp_gives_direct_mcps_answers_even_on_a_socket_started_with_yes() {
    if !env_free() {
        return;
    }
    let (server, runs) = server_with(true, |_| {});
    let file = token_file("attach-same-answers", TOKEN);
    let (direct_session, _) = session();
    let (messages, refused) = script(&direct_session);
    let direct = replies(&mut mcp::Local::new(&direct_session, &Options::default()), &messages);
    let attached = replies(&mut attached(server.addr().port(), &file), &messages);
    assert_eq!(direct.len(), attached.len(), "one reply a request on both");
    for (at, (want, got)) in direct.iter().zip(&attached).enumerate() {
        if refused.contains(&at) {
            // Refused before the app sees it: its params aren't checked first, and it never runs.
            assert_eq!(tool_failure(&parsed(got))["code"], "consent_required", "{}: {got}", messages[at]);
            assert_eq!(parsed(want)["result"]["isError"], true, "{want}");
        } else {
            assert_eq!(got, want, "{}", messages[at].chars().take(200).collect::<String>());
        }
    }
    let codes: Vec<Value> = attached.iter().map(|l| parsed(l)).filter(|r| r["result"]["isError"] == true).map(|r| tool_failure(&r)["code"].clone()).collect();
    for code in ["bad_params", "unavailable", "consent_required"] {
        assert!(codes.contains(&json!(code)), "the script reaches `{code}`: {codes:?}");
    }
    let protocol: Vec<Value> = attached.iter().map(|l| parsed(l)["error"]["data"]["code"].clone()).collect();
    assert!(protocol.contains(&json!("unknown_command")) && protocol.contains(&json!("too_large")), "{protocol:?}");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the command with an effect never ran in the app");
}

#[test]
fn consent_is_decided_from_the_spec_before_the_call_is_sent() {
    if !env_free() {
        return;
    }
    let (server, runs) = server_with(true, |_| {});
    let file = token_file("attach-consent", TOKEN);
    let mut bridge = attached(server.addr().port(), &file);
    let got = replies(&mut bridge, &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string(), call(2, "test-write", &json!({}))]);
    let tools = parsed(&got[0])["result"]["tools"].clone();
    let writer = tools.as_array().unwrap().iter().find(|t| t["name"] == "test-write").unwrap().clone();
    assert_eq!(writer["annotations"]["readOnlyHint"], false, "its effects come through `commands`");
    let failure = tool_failure(&parsed(&got[1]));
    assert_eq!(failure["code"], "consent_required", "{failure}");
    assert!(failure["remediation"].as_str().is_some_and(|r| !r.is_empty()));
    assert_eq!(runs.load(Ordering::SeqCst), 0, "not run");
    // The socket itself consents (`--yes`): a frame client's call runs. MCP over it still doesn't.
    let outcome = Conn::open_to(&bridge.config).unwrap().ask("test.write", None, Duration::from_secs(10), server.addr().port());
    assert!(matches!(outcome, Answer::Replied(Ok(_))), "{outcome:?}");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// The JSON-RPC error a tools/call or tools/list of `bridge` gets, and how long it took.
fn protocol_error(bridge: &mut Attached, message: &str) -> (Value, Duration) {
    let started = Instant::now();
    let got = replies(bridge, &[message.to_owned()]);
    let reply = parsed(&got[0]);
    assert!(reply.get("result").is_none(), "{reply}");
    (reply["error"].clone(), started.elapsed())
}

#[test]
fn a_wrong_token_is_unauthenticated_with_what_to_check_and_never_shown() {
    if !env_free() {
        return;
    }
    let (server, _) = server_with(false, |_| {});
    let wrong = "test-token-not-real-wrong-one-000";
    let file = token_file("attach-wrong-token", wrong);
    let (error, _) = protocol_error(&mut attached(server.addr().port(), &file), &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string());
    assert_eq!(error["data"]["code"], "unauthenticated", "{error}");
    assert!(error["data"]["remediation"].as_str().is_some_and(|r| r.contains("--token-file") && r.contains(ENV)), "{error}");
    assert!(!error.to_string().contains("not-real"), "the token is never shown: {error}");
}

#[test]
fn a_missing_token_file_says_what_to_pass() {
    if !env_free() {
        return;
    }
    let (server, _) = server_with(false, |_| {});
    let missing = {{crate_prefix_ident}}testkit::temp_dir("attach-no-token").unwrap().join("control.token");
    let (error, _) = protocol_error(&mut attached(server.addr().port(), &missing), &call(1, "app-info", &json!({})));
    assert_eq!(error["data"]["code"], "unavailable", "{error}");
    assert!(!missing.exists(), "a client never makes a token file");
}

/// The answer when the app is gone says how to start it, and that it never got the call.
fn says_to_start_the_app(error: &Value) {
    assert_eq!(error["data"]["code"], "unavailable", "{error}");
    let remediation = error["data"]["remediation"].as_str().unwrap_or_default();
    assert!(remediation.contains("--socket") && !remediation.contains("may have run"), "{error}");
}

/// The port is held for the test ([`DeadPort`]: a port it freed, another process could take before
/// it connects), so the app there is one that is gone: the connection is made, then closed before
/// `auth` is answered, rather than refused. No call has gone out, so the answer is a refused one's.
#[test]
fn a_port_the_app_is_gone_from_is_unavailable_with_how_to_start_the_app() {
    if !env_free() {
        return;
    }
    let dead = DeadPort::new().unwrap();
    let file = token_file("attach-closed", TOKEN);
    let (error, took) = protocol_error(&mut attached(dead.port(), &file), &call(1, "app-info", &json!({})));
    says_to_start_the_app(&error);
    assert!(error["message"].as_str().is_some_and(|m| m.contains("closed before it answered")), "{error}");
    assert_eq!(dead.accepted(), 1, "it connected once");
    assert!(took < Duration::from_secs(8), "{took:?}");
}

/// A refused connection can't be had on a port the test holds (std listens on each port it binds),
/// so its answer is checked here, from the error a refused or timed-out connect gives.
#[test]
fn a_refused_connection_is_unavailable_with_how_to_start_the_app() {
    for kind in [io::ErrorKind::ConnectionRefused, io::ErrorKind::TimedOut] {
        let e = cannot_connect(4000, &io::Error::from(kind));
        assert_eq!(e.code, ErrorCode::Unavailable, "{kind:?}");
        assert!(e.message.starts_with("cannot connect to the app on 127.0.0.1:4000") && e.remediation.contains("--socket"), "{e:?}");
    }
}

/// A server that speaks the frame for the handshake (`auth`, `commands`), then does `then` with the
/// connection: one connection, then it is gone. Its thread gives back its listener, still holding the
/// port: a test that connects again holds it with [`DeadPort::on`], so no other process takes it.
fn fake(then: impl FnOnce(&mut BufReader<TcpStream>, &mut TcpStream) + Send + 'static) -> (u16, JoinHandle<TcpListener>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let specs = commands_json(&session().0).unwrap();
        for answer in [json!({"authenticated": true}), specs] {
            let request = frame::parse_request(&next_line(&mut reader)).unwrap();
            write_line(&mut writer, &FrameReply::success(request.id, answer).encode(usize::MAX)).unwrap();
        }
        then(&mut reader, &mut writer);
        listener
    });
    (port, handle)
}

fn next_line(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    line
}

/// Reads until the peer goes (it closes, or the read's timeout passes).
fn until_gone(reader: &mut BufReader<TcpStream>) {
    while !next_line(reader).is_empty() {}
}

#[test]
fn a_connection_that_drops_mid_call_is_a_typed_error_and_the_next_call_connects_once() {
    if !env_free() {
        return;
    }
    let (port, handle) = fake(|reader, _| {
        next_line(reader);
    });
    let file = token_file("attach-dropped", TOKEN);
    let mut bridge = attached(port, &file);
    let (error, took) = protocol_error(&mut bridge, &call(1, "app-info", &json!({})));
    assert_eq!(error["data"]["code"], "unavailable", "{error}");
    assert!(error["message"].as_str().is_some_and(|m| m.contains("closed before it answered")), "{error}");
    assert!(error["data"]["remediation"].as_str().is_some_and(|r| r.contains("may have run")), "{error}");
    assert!(took < Duration::from_secs(8), "{took:?}");
    let dead = DeadPort::on(handle.join().unwrap()).unwrap();
    // The fake is gone: the next call connects once, is closed before `auth` is answered, and says so.
    let (error, took) = protocol_error(&mut bridge, &call(2, "app-info", &json!({})));
    says_to_start_the_app(&error);
    assert_eq!(dead.accepted(), 1, "it connected once");
    assert!(took < Duration::from_secs(8), "{took:?}");
}

#[test]
fn a_reply_that_never_comes_is_cancelled_at_the_deadline() {
    if !env_free() {
        return;
    }
    let (port, handle) = fake(|reader, _| until_gone(reader));
    let file = token_file("attach-silent", TOKEN);
    let mut bridge = attached(port, &file);
    bridge.config.reply_timeout = Duration::from_millis(300);
    let (error, took) = protocol_error(&mut bridge, &call(1, "app-info", &json!({})));
    assert_eq!(error["data"]["code"], "cancelled", "{error}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    drop(bridge);
    handle.join().unwrap();
}

#[test]
fn a_reply_over_the_cap_is_too_large() {
    if !env_free() {
        return;
    }
    let (port, handle) = fake(|reader, writer| {
        next_line(reader);
        let _ = write_line(writer, &"x".repeat(MAX_REPLY + 1));
        until_gone(reader);
    });
    let file = token_file("attach-too-large", TOKEN);
    let mut bridge = attached(port, &file);
    let got = replies(&mut bridge, &[call(1, "app-info", &json!({}))]);
    assert_eq!(tool_failure(&parsed(&got[0]))["code"], "too_large", "{}", got[0].chars().take(300).collect::<String>());
    drop(bridge);
    handle.join().unwrap();
}

#[test]
fn a_connection_the_app_closed_while_idle_is_made_again_before_the_next_call() {
    if !env_free() {
        return;
    }
    let (server, _) = server_with(false, |config| config.idle = Duration::from_millis(200));
    let file = token_file("attach-idle", TOKEN);
    let mut bridge = attached(server.addr().port(), &file);
    let first = replies(&mut bridge, &[call(1, "app-info", &json!({}))]);
    assert_eq!(parsed(&first[0])["result"]["isError"], false, "{}", first[0]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.connections() > 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.connections(), 0, "the app closed the idle connection");
    let again = replies(&mut bridge, &[call(2, "app-info", &json!({}))]);
    assert_eq!(again[0], first[0].replace("\"id\":1", "\"id\":2"), "the same answer, on a new connection");
}

#[test]
fn the_flags_need_a_port_and_a_token() {
    assert_eq!(parse(&["--attach", "4000", "--token-file", "t"]), Ok(AttachArgs { port: 4000, token_file: Some("t".into()) }));
    for bad in [&["--attach"][..], &["--attach", "0"], &["--attach", "x"], &["--attach", "70000"], &["--token-file", "t"], &["--attach", "1", "--nope"]] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
    if env_free() {
        let e = parse(&["--attach", "4000"]).unwrap_err();
        assert!(e.contains(ENV) && e.contains("--token-file"), "{e}");
    }
}
