//! One connection of the control socket: `auth`, then the frame's requests until the peer goes, is
//! idle, or the server stops.
//!
//! Two threads serve a connection. The **reader** reads the peer's lines (capped, with the auth,
//! idle and trickle deadlines) and hands them to the **handler**, which answers them in order, one
//! call at a time. Because something is always reading, the peer going away is seen while a call
//! runs: the reader sets the connection's [`CancelToken`] (the one every call of the connection
//! carries), so a slow command returns `cancelled`, and the handler ends the connection. A request
//! the peer sent behind the running one is read, not lost, and is answered in its turn.
//!
//! - **The queue is short.** The reader runs at most [`LOOKAHEAD`] requests ahead of the handler:
//!   it waits for room, so a peer can't make the server hold an unbounded queue. While it waits it
//!   still watches for the stop, and reads on into a buffer of [`AHEAD`] bytes at most, which is
//!   how it sees the peer's end behind the requests it has read.
//! - **Nothing runs once the connection's calls are cancelled.** The handler looks at the token
//!   before each request, so when the server stops, or the peer is seen gone, the requests still
//!   queued are dropped, not run; and the reader hands over no line it has buffered once the server
//!   is stopping.
//! - **What can be seen late.** A peer that goes away with more than that behind the running call
//!   (the queue, the reader's buffer and [`AHEAD`]: some 70 KiB of requests) is seen gone only when
//!   the reader reaches its end, as the handler makes room: the requests before that end are
//!   answered (to nobody) until then.
//! - **A half-close** (the peer shuts its sending side) is a going away: a client that wants its
//!   replies keeps its side open until it has them.

use std::collections::VecDeque;
use std::io::{self, BufReader, Read};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use {{crate_prefix_ident}}engine::{Caller, CancelToken, CommandError};

use super::{Conn, LINGER, Shared, close_gently, full};
use crate::frame::{self, Id, Reply};
use crate::limits::{MAX_REPLY, MAX_REQUEST};
use crate::lines::{Line, read_line, write_line};
use crate::serve::answer;
use crate::token::Token;

/// The requests the reader may hold for the handler, beside the one being answered.
const LOOKAHEAD: usize = 2;

/// The params of `auth`: the token, and nothing else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthParams {
    token: String,
}

/// What the first message of a connection was.
enum Auth {
    /// A good `auth`, with the id to answer.
    Granted(Id),
    /// Anything else; the id of the request, when it was one.
    Refused(Option<Id>),
}

/// The one thing a refused connection is told, whatever was wrong with it: no hint which part.
fn unauthenticated() -> CommandError {
    CommandError::unauthenticated(
        "the connection's first message must be a valid `auth`",
        "send {\"id\": 0, \"method\": \"auth\", \"params\": {\"token\": \"<token>\"}} first; the token is in the file or the environment variable the app was started with",
    )
}

/// What `auth` is judged by: `line` is a request calling `auth` with `token`.
fn judge(token: &Token, line: Line) -> Auth {
    let Line::Text(text) = line else { return Auth::Refused(None) };
    let Ok(request) = frame::parse_request(&text) else { return Auth::Refused(None) };
    let granted = request.method == "auth"
        && request
            .params
            .filter(Value::is_object)
            .and_then(|params| serde_json::from_value::<AuthParams>(params).ok())
            .is_some_and(|params| token.matches(&params.token));
    if granted { Auth::Granted(request.id) } else { Auth::Refused(Some(request.id)) }
}

/// What the connection's two threads share.
#[derive(Debug, Default)]
struct Watch {
    /// Requests the reader has read that the handler hasn't finished answering. While there are
    /// some the peer is waiting for the server, not idle.
    outstanding: AtomicUsize,
    /// The handler is done: the reader stops.
    finished: AtomicBool,
}

impl Watch {
    fn waiting(&self) -> bool {
        self.outstanding.load(Ordering::Acquire) > 0
    }
}

/// Counts a request as answered when it drops, however the handler left it.
struct Handling<'a>(&'a Watch);

impl Drop for Handling<'_> {
    fn drop(&mut self) {
        self.0.outstanding.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A reader that gives up when the server stops, the handler is done or its deadline passes, and
/// wakes often enough to notice any of them. The deadline is set before each request, so it bounds
/// the whole request (a peer that trickles one byte at a time is cut off) as well as the idle
/// wait; it doesn't run while the peer is waiting for an answer (a long call isn't idleness).
struct Reader {
    stream: TcpStream,
    shared: Arc<Shared>,
    watch: Arc<Watch>,
    deadline: Instant,
    /// How long the next request has: the auth deadline for the first, then the idle timeout.
    limit: Duration,
    /// Bytes of the request being read have arrived.
    begun: bool,
    /// Bytes read on while the queue was full ([`Reader::peer_gone`]), read before the socket's.
    ahead: VecDeque<u8>,
}

/// The longest a read waits before looking at the stop flag and the deadline again.
const SLICE: Duration = Duration::from_millis(100);

/// How often a reader waiting for room in the queue looks at the stop flag and the peer.
const NUDGE: Duration = Duration::from_millis(20);

/// The most a reader waiting for room reads on, looking for the peer's end.
const AHEAD: usize = 64 * 1024;

impl Reader {
    /// Starts the clock of the next request.
    fn expect_request(&mut self) {
        self.begun = false;
        self.deadline = Instant::now() + self.limit;
    }

    /// The server is stopping or the handler is done: read and hand over nothing more.
    fn over(&self) -> bool {
        self.shared.stopping.load(Ordering::Acquire) || self.watch.finished.load(Ordering::Acquire)
    }

    /// Whether the peer has gone (its end, or a reset), looked for by reading on into
    /// [`Reader::ahead`], which keeps what it reads for the lines after. Once [`AHEAD`] bytes wait
    /// there the end can't be seen, and this only waits. It waits [`NUDGE`] at most.
    fn peer_gone(&mut self) -> bool {
        let room = AHEAD.saturating_sub(self.ahead.len());
        if room == 0 {
            thread::sleep(NUDGE);
            return false;
        }
        if self.stream.set_read_timeout(Some(NUDGE)).is_err() {
            return true;
        }
        let mut chunk = [0_u8; 4096];
        let into = chunk.get_mut(..room.min(4096)).unwrap_or_default();
        match self.stream.read(into) {
            Ok(0) => true,
            Ok(n) => {
                self.ahead.extend(into.get(..n).unwrap_or_default());
                false
            }
            Err(e) => !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted),
        }
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.over() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        if !self.ahead.is_empty() {
            self.begun = true;
            return self.ahead.read(buf);
        }
        loop {
            if self.over() {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            if !self.begun && self.watch.waiting() {
                self.deadline = Instant::now() + self.limit;
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            self.stream.set_read_timeout(Some(left.min(SLICE)))?;
            match self.stream.read(buf) {
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Ok(n) if n > 0 => {
                    self.begun = true;
                    return Ok(n);
                }
                other => return other,
            }
        }
    }
}

/// Reads the next line, giving the peer the reader's limit to send it whole. `None`: the connection
/// is over (the peer is gone or idle, or the server stopped: a line already buffered is not handed
/// over then).
fn next_line(input: &mut BufReader<Reader>) -> Option<Line> {
    input.get_mut().expect_request();
    match read_line(input, MAX_REQUEST) {
        Ok(Line::Eof) | Err(_) => None,
        Ok(_) if input.get_ref().over() => None,
        Ok(line) => Some(line),
    }
}

/// Hands `line` to the handler, waiting for room in the queue. False: the connection is over (the
/// handler is done, the server is stopping, or the peer went while the reader waited).
fn deliver(input: &mut BufReader<Reader>, lines: &SyncSender<Line>, mut line: Line) -> bool {
    loop {
        match lines.try_send(line) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(back)) => line = back,
        }
        let reader = input.get_mut();
        if reader.over() || reader.peer_gone() {
            return false;
        }
    }
}

/// The reader's thread: lines to the handler until the connection is over, and then `cancel` for
/// the call the handler may be running and the requests still queued (the peer isn't there for
/// their answers; a stopped server's calls end too).
fn read_lines(mut input: BufReader<Reader>, lines: &SyncSender<Line>, watch: &Watch, cancel: &CancelToken) {
    while let Some(line) = next_line(&mut input) {
        // `auth` had the auth deadline; every request after it has the idle timeout.
        let reader = input.get_mut();
        reader.limit = reader.shared.idle;
        watch.outstanding.fetch_add(1, Ordering::AcqRel);
        if !deliver(&mut input, lines, line) {
            break;
        }
    }
    cancel.cancel();
}

/// Serves one connection to its end. An I/O failure ends this connection and nothing else.
pub(super) fn serve(shared: &Arc<Shared>, conn: &Conn, stream: TcpStream) {
    let _ = run(shared, conn, stream);
}

fn run(shared: &Arc<Shared>, conn: &Conn, stream: TcpStream) -> io::Result<()> {
    // A listener that polls may hand out sockets that inherited its non-blocking mode.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(shared.idle))?;
    let mut output = stream.try_clone()?;
    let watch = Arc::new(Watch::default());
    let limit = shared.auth_timeout.min(shared.idle);
    let input = BufReader::new(Reader {
        stream,
        shared: Arc::clone(shared),
        watch: Arc::clone(&watch),
        deadline: Instant::now(),
        limit,
        begun: false,
        ahead: VecDeque::new(),
    });
    let (send, lines) = mpsc::sync_channel(LOOKAHEAD);
    let result = thread::scope(|scope| {
        let (reading, counting) = (conn.cancel.clone(), Arc::clone(&watch));
        thread::Builder::new().name("control-reader".to_owned()).spawn_scoped(scope, move || read_lines(input, &send, &counting, &reading))?;
        let result = converse(shared, conn, &mut output, &lines, &watch);
        conn.answered.store(true, Ordering::Release);
        // Stops the reader: the flag for its next look (its reads and its waits for room wake
        // often), and the dropped receiver for a send.
        watch.finished.store(true, Ordering::Release);
        drop(lines);
        result
    });
    // The reader is done (the scope joined it): what the peer still sends is drained, not reset.
    close_gently(&output, LINGER);
    result
}

/// `auth`, then the requests in order, until the reader has no more, the connection's calls are
/// cancelled or a write fails.
fn converse(shared: &Arc<Shared>, conn: &Conn, output: &mut TcpStream, lines: &Receiver<Line>, watch: &Watch) -> io::Result<()> {
    let Ok(first) = lines.recv() else { return Ok(()) };
    let handling = Handling(watch);
    match judge(&shared.token, first) {
        Auth::Granted(id) if shared.authenticate(conn) => write_line(output, &Reply::success(id, json!({"authenticated": true})).encode(MAX_REPLY))?,
        Auth::Granted(id) => return write_line(output, &Reply::failure(Some(id), &full(shared.max_connections)).encode(MAX_REPLY)),
        Auth::Refused(id) => return write_line(output, &Reply::failure(id, &unauthenticated()).encode(MAX_REPLY)),
    }
    drop(handling);
    let mut opts = shared.options.call_opts(Caller::Socket);
    opts.cancel = conn.cancel.clone();
    while let Ok(line) = lines.recv() {
        let _handling = Handling(watch);
        // Queued behind a call that was cancelled (the peer went, or the server stopped): not run.
        if opts.cancel.is_cancelled() {
            break;
        }
        if let Some(reply) = answer(&shared.session, line, &opts) {
            write_line(output, &reply)?;
        }
    }
    Ok(())
}
