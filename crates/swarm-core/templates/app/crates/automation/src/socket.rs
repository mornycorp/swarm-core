//! The loopback control socket (docs/app-architecture.md › The JSON-lines frame): the frame of
//! [`serve`](crate::serve) over TCP, so an agent or a test drives the running app as the CLI does.
//!
//! - **Loopback only.** It binds `127.0.0.1`, never `0.0.0.0`, and drops a peer that isn't a
//!   loopback address. Port `0` takes a free one ([`Server::addr`] says which).
//! - **`auth` first.** A connection's first message must be `{"id": 0, "method": "auth",
//!   "params": {"token": "…"}}` with the right [`Token`](crate::token::Token) (compared in constant
//!   time), and it must arrive whole within [`AUTH_TIMEOUT`]. Anything else gets `unauthenticated`
//!   and the connection closes (one that sends nothing in time is just closed). Then it speaks the
//!   same frames as `serve` and answers through the same function, so a request means the same on
//!   every transport (`Caller::Socket`).
//! - **The caps** of [`limits`](crate::limits): a request line over [`MAX_REQUEST`] is answered
//!   `too_large` (the rest of the line is discarded and the connection goes on), a reply over
//!   [`MAX_REPLY`] is `too_large` too, at most [`MAX_CONNECTIONS`] authenticated connections are
//!   served at a time (the next is told `unavailable` and closed), and a connection is closed after
//!   [`IDLE_TIMEOUT`] without a request (a request must also arrive whole within it, so a peer
//!   that trickles bytes can't hold a connection).
//! - **No lock-out before `auth`.** A connection that hasn't authenticated yet doesn't take one of
//!   those slots: at most [`MAX_UNAUTHENTICATED`] wait at a time, each for [`AUTH_TIMEOUT`] at most,
//!   and one more closes the oldest of them to make room. Peers without the token can't keep out a
//!   client that has it (a client sends `auth` as soon as it connects).
//! - **Replies survive the close.** Closing a socket with input it hasn't read resets the
//!   connection, and a reset can discard a reply the peer hasn't read yet (Windows does). So a
//!   connection ends by shutting its sending side, then reading and dropping what the peer still
//!   sends, for [`LINGER`] and [`LINGER_BYTES`] at most, before it closes ([`close_gently`]): an
//!   `unavailable` or `unauthenticated` reply reaches a peer that pipelined requests behind `auth`.
//! - **Cancel.** A connection that goes away (closed, reset, or its sending side shut) while a call
//!   runs sets that call's `CancelToken`: a slow command returns `cancelled` (to nobody) and the
//!   connection's thread is free at once, not when the command would have finished. The
//!   details, and what a pipelined request sees, are in [`conn`]'s docs.
//! - **A clean stop.** [`Server::stop`] (and dropping the server) stops accepting, cancels every
//!   connection's calls (a slow command returns `cancelled`, and no request queued behind it runs),
//!   gives the connections [`STOP_GRACE`] to answer, then shuts their sockets down and waits for
//!   their threads, for a bounded time.
//!
//! Two threads serve a connection (one reads, one answers) and calls are synchronous, so a call
//! blocks only its own connection. Nothing here panics on what a peer sends, and a peer's I/O
//! error is its own connection's end, never the server's.

mod conn;

use std::io::{self, Read};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use {{crate_prefix_ident}}engine::{CancelToken, CommandError, Session};

use crate::Options;
use crate::frame::Reply;
use crate::limits::{IDLE_TIMEOUT, MAX_CONNECTIONS, MAX_REPLY, MAX_REQUEST};
use crate::lines::write_line;
use crate::token::Token;

/// The first message (`auth`) must arrive whole within this: a client sends it as soon as it
/// connects, so a connection that hasn't is closed long before the idle timeout.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(3);

/// Connections waiting to authenticate at a time; one more closes the oldest of them.
pub const MAX_UNAUTHENTICATED: usize = 8;

/// How a server is started.
#[derive(Debug, Clone)]
pub struct Config {
    /// The port on `127.0.0.1`; `0` takes a free one.
    pub port: u16,
    /// What a client's `auth` must carry.
    pub token: Token,
    /// What the clients have agreed to (`consent`): one setting for every connection.
    pub options: Options,
    /// A connection closed after this long without a request.
    pub idle: Duration,
    /// The first message (`auth`) must arrive whole within this (or `idle`, when that is shorter).
    pub auth_timeout: Duration,
    /// Authenticated connections served at a time.
    pub max_connections: usize,
    /// Connections waiting to authenticate at a time (at least one): one more closes the oldest.
    pub max_unauthenticated: usize,
}

impl Config {
    /// A server on a free port with the contract's caps ([`IDLE_TIMEOUT`], [`MAX_CONNECTIONS`]) and
    /// the socket's own ([`AUTH_TIMEOUT`], [`MAX_UNAUTHENTICATED`]).
    pub fn new(token: Token, options: Options) -> Config {
        Config {
            port: 0,
            token,
            options,
            idle: IDLE_TIMEOUT,
            auth_timeout: AUTH_TIMEOUT,
            max_connections: MAX_CONNECTIONS,
            max_unauthenticated: MAX_UNAUTHENTICATED,
        }
    }
}

/// What the accept loop and every connection share.
struct Shared {
    session: Arc<Session>,
    token: Token,
    options: Options,
    idle: Duration,
    auth_timeout: Duration,
    max_connections: usize,
    max_unauthenticated: usize,
    stopping: AtomicBool,
    /// Connections being served, authenticated or not.
    active: AtomicUsize,
    /// Authenticated connections: at most `max_connections`.
    authenticated: AtomicUsize,
    /// Refused connections being closed gently: at most [`MAX_CLOSING`].
    closing: AtomicUsize,
}

impl Shared {
    /// Takes one of the authenticated slots for `conn`: false when they are all in use.
    fn authenticate(&self, conn: &Conn) -> bool {
        let mut taken = self.authenticated.load(Ordering::Acquire);
        while taken < self.max_connections {
            match self.authenticated.compare_exchange_weak(taken, taken + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    conn.authenticated.store(true, Ordering::Release);
                    return true;
                }
                Err(now) => taken = now,
            }
        }
        false
    }
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").field("active", &self.active.load(Ordering::Relaxed)).finish_non_exhaustive()
    }
}

/// One connection, as the server sees it from outside its threads: what a stop, and the room made
/// for the next unauthenticated connection, act on.
#[derive(Debug, Default)]
struct Conn {
    /// Every call of the connection carries it: set by a stop, or by the reader when the peer goes.
    cancel: CancelToken,
    /// It holds one of the authenticated slots.
    authenticated: AtomicBool,
    /// Its handler is done: every reply it will write is written.
    answered: AtomicBool,
    /// Its threads have ended.
    done: AtomicBool,
}

/// A connection the accept loop keeps a handle on, to shut it down.
#[derive(Debug)]
struct Live {
    conn: Arc<Conn>,
    stream: TcpStream,
}

impl Live {
    /// Still waiting to authenticate.
    fn unauthenticated(&self) -> bool {
        !self.conn.authenticated.load(Ordering::Acquire) && !self.conn.done.load(Ordering::Acquire)
    }

    /// Its handler isn't done: it may still write a reply.
    fn answering(&self) -> bool {
        !self.conn.answered.load(Ordering::Acquire) && !self.conn.done.load(Ordering::Acquire)
    }
}

/// A running control socket. Stopped by [`Server::stop`] or by dropping it.
#[derive(Debug)]
pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
    /// The accept loop; it hands back the connections it was serving when it ends.
    accept: Option<JoinHandle<Vec<Live>>>,
}

/// How long the accept loop sleeps between looks at an empty listener, so a stop is seen quickly.
const POLL: Duration = Duration::from_millis(20);

/// How long [`Server::stop`] gives the connections to answer their cancelled calls before it shuts
/// their sockets down.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// The longest [`Server::stop`] then waits for the connections to end.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// How long a closing connection reads what its peer still sends ([`close_gently`]).
const LINGER: Duration = Duration::from_secs(1);

/// The most a closing connection reads and drops.
const LINGER_BYTES: usize = 4 * MAX_REQUEST;

/// Refused connections being closed gently at a time; past it one is told and dropped.
const MAX_CLOSING: usize = 16;

impl Server {
    /// Binds `127.0.0.1:<port>` and serves `session` on it until stopped.
    pub fn start(session: Arc<Session>, config: Config) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let Config { token, options, idle, auth_timeout, max_connections, max_unauthenticated, .. } = config;
        let shared = Arc::new(Shared {
            session,
            token,
            options,
            idle,
            auth_timeout,
            max_connections,
            max_unauthenticated: max_unauthenticated.max(1),
            stopping: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            authenticated: AtomicUsize::new(0),
            closing: AtomicUsize::new(0),
        });
        let accepting = Arc::clone(&shared);
        let accept = thread::Builder::new().name("control-accept".to_owned()).spawn(move || accept_loop(&listener, &accepting))?;
        Ok(Server { addr, shared, accept: Some(accept) })
    }

    /// The address it listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Connections being served now.
    pub fn connections(&self) -> usize {
        self.shared.active.load(Ordering::Acquire)
    }

    /// Stops accepting, cancels every connection's calls, gives the connections [`STOP_GRACE`] to
    /// answer them (`cancelled`), shuts their sockets down and waits (at most [`STOP_WAIT`]) for them
    /// to end. Safe to call again.
    pub fn stop(&mut self) {
        self.shared.stopping.store(true, Ordering::Release);
        let live = self.accept.take().and_then(|accept| accept.join().ok()).unwrap_or_default();
        for each in &live {
            each.conn.cancel.cancel();
        }
        wait_until(STOP_GRACE, || !live.iter().any(Live::answering));
        for each in &live {
            let _ = each.stream.shutdown(Shutdown::Both);
        }
        wait_until(STOP_WAIT, || self.connections() == 0);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Waits until `done`, for `limit` at most.
fn wait_until(limit: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
}

/// Holds a connection's place in the server's counts; it is given back when it drops, however the
/// connection's thread ended.
struct Slot {
    shared: Arc<Shared>,
    conn: Arc<Conn>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.conn.authenticated.load(Ordering::Acquire) {
            self.shared.authenticated.fetch_sub(1, Ordering::AcqRel);
        }
        self.conn.done.store(true, Ordering::Release);
        self.shared.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Accepts connections until the server is stopped, and hands back the ones still being served.
fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) -> Vec<Live> {
    let mut live: Vec<Live> = Vec::new();
    while !shared.stopping.load(Ordering::Acquire) {
        live.retain(|each| !each.conn.done.load(Ordering::Acquire));
        match listener.accept() {
            Ok((stream, peer)) => admit(shared, &mut live, stream, peer),
            // Nothing waiting, or a connection that died before it was accepted: look again.
            Err(_) => thread::sleep(POLL),
        }
    }
    live
}

/// Serves `stream` on a thread of its own, or refuses it: a peer that isn't loopback is dropped,
/// and one that comes while every authenticated slot is in use is told so.
fn admit(shared: &Arc<Shared>, live: &mut Vec<Live>, stream: TcpStream, peer: SocketAddr) {
    if !peer.ip().is_loopback() {
        return;
    }
    if shared.authenticated.load(Ordering::Acquire) >= shared.max_connections {
        refuse(shared, stream);
        return;
    }
    make_room(live, shared.max_unauthenticated);
    let Ok(handle) = stream.try_clone() else { return };
    let conn = Arc::new(Conn::default());
    shared.active.fetch_add(1, Ordering::AcqRel);
    let slot = Slot { shared: Arc::clone(shared), conn: Arc::clone(&conn) };
    live.push(Live { conn: Arc::clone(&conn), stream: handle });
    let shared = Arc::clone(shared);
    // A thread that can't be made gives its place back (the closure, and the slot in it, drop).
    let _ = thread::Builder::new().name("control-connection".to_owned()).spawn(move || {
        let _slot = slot;
        conn::serve(&shared, &conn, stream);
    });
}

/// Closes the oldest connection still waiting to authenticate when `max` of them are, so a new one
/// fits: silent peers can't hold the socket.
fn make_room(live: &mut Vec<Live>, max: usize) {
    if live.iter().filter(|each| each.unauthenticated()).count() < max {
        return;
    }
    if let Some(at) = live.iter().position(Live::unauthenticated) {
        let oldest = live.remove(at);
        let _ = oldest.stream.shutdown(Shutdown::Both);
    }
}

/// What a connection is told when every authenticated slot is in use.
fn full(limit: usize) -> CommandError {
    CommandError::unavailable(
        format!("the control socket serves {limit} connections at a time and they are all in use"),
        "close a connection you no longer need, or wait for one to go idle, then connect again",
    )
}

/// Counts a refused connection as being closed until it drops.
struct Closing(Arc<Shared>);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.closing.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Tells a peer over the limit why and closes, gently, on a thread of its own: a few at a time, and
/// past [`MAX_CLOSING`] one is told and dropped.
fn refuse(shared: &Arc<Shared>, stream: TcpStream) {
    let line = Reply::failure(None, &full(shared.max_connections)).encode(MAX_REPLY);
    let _ = stream.set_nonblocking(false).and_then(|()| stream.set_write_timeout(Some(Duration::from_millis(200))));
    let before = shared.closing.fetch_add(1, Ordering::AcqRel);
    let closing = Closing(Arc::clone(shared));
    if before >= MAX_CLOSING {
        let _ = write_line(&mut &stream, &line);
        return;
    }
    let _ = thread::Builder::new().name("control-refuse".to_owned()).spawn(move || {
        let _closing = closing;
        if write_line(&mut &stream, &line).is_ok() {
            close_gently(&stream, LINGER);
        }
    });
}

/// Ends a connection without losing what was written to it. Closing a socket that has input it
/// hasn't read resets the connection, and the peer may then lose a reply it hasn't read yet; so this
/// shuts the sending side (the peer reads the reply, then the end), then reads and drops what the
/// peer still sends until it closes, `linger` passes or [`LINGER_BYTES`] have come. The caller then
/// drops the stream.
fn close_gently(stream: &TcpStream, linger: Duration) {
    let _ = stream.shutdown(Shutdown::Write);
    let deadline = Instant::now() + linger;
    let mut left = LINGER_BYTES;
    let mut sink = [0_u8; 8192];
    let mut input = stream;
    while left > 0 {
        let wait = deadline.saturating_duration_since(Instant::now());
        if wait.is_zero() || stream.set_read_timeout(Some(wait)).is_err() {
            return;
        }
        match input.read(&mut sink) {
            Ok(0) | Err(_) => return,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

#[cfg(test)]
mod tests;
