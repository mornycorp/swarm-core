use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use {{crate_prefix_ident}}engine::commands::InfoCommand;
use {{crate_prefix_ident}}engine::{AppInfo, CallContext, Command, Registry};

use super::*;
use crate::limits::MAX_REQUEST;

const TOKEN: &str = "test-token-not-real-0123456789";

fn session() -> Arc<Session> {
    Arc::new(Session::new(AppInfo { name: "demo".into(), version: "0.3.0".into() }).unwrap())
}

fn start_with(change: impl FnOnce(&mut Config)) -> Server {
    let mut config = Config::new(Token::parse(TOKEN).unwrap(), Options::default());
    change(&mut config);
    Server::start(session(), config).unwrap()
}

fn start() -> Server {
    start_with(|_| {})
}

/// A client: it never waits for ever, so a server that fails to answer fails the test.
struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    fn connect(server: &Server) -> Client {
        let stream = TcpStream::connect(server.addr()).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Client { writer: stream.try_clone().unwrap(), reader: BufReader::new(stream) }
    }

    fn send(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.write_all(b"\n").unwrap();
    }

    /// The next reply, or `None` when the server closed the connection.
    fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap()),
        }
    }

    fn ask(&mut self, request: &Value) -> Value {
        self.send(&request.to_string());
        self.recv().unwrap()
    }

    fn authed(server: &Server) -> Client {
        let mut client = Client::connect(server);
        let reply = client.ask(&json!({"id": 0, "method": "auth", "params": {"token": TOKEN}}));
        assert_eq!(reply, json!({"id": 0, "ok": true, "result": {"authenticated": true}}));
        client
    }
}

#[test]
fn it_listens_on_loopback_only() {
    let server = start();
    assert!(server.addr().ip().is_loopback() && server.addr().ip().is_ipv4(), "{}", server.addr());
    assert_ne!(server.addr().port(), 0);
}

#[test]
fn a_good_auth_then_the_same_frames_as_serve() {
    let server = start();
    let mut client = Client::authed(&server);
    let info = json!({"name": "demo", "version": "0.3.0", "schema_version": 2});
    assert_eq!(client.ask(&json!({"id": 1, "method": "app.info"})), json!({"id": 1, "ok": true, "result": info}));
    assert_eq!(client.ask(&json!({"id": "b", "method": "commands"}))["result"][0]["id"], "app.info");
    let failed = client.ask(&json!({"id": 2, "method": "app.info", "params": {"x": 1}}));
    assert_eq!((failed["code"].clone(), failed["key"].clone()), (json!("bad_params"), json!("x")));
    let unknown = client.ask(&json!({"id": 3, "method": "auth", "params": {"token": TOKEN}}));
    assert_eq!(unknown["code"], "unknown_command", "`auth` is only the first message: {unknown}");
}

#[test]
fn a_wrong_token_is_unauthenticated_and_the_connection_closes() {
    let server = start();
    let mut client = Client::connect(&server);
    let reply = client.ask(&json!({"id": 5, "method": "auth", "params": {"token": "test-token-not-real-0123456780"}}));
    assert_eq!((reply["ok"].clone(), reply["code"].clone(), reply["id"].clone()), (json!(false), json!("unauthenticated"), json!(5)));
    assert!(!reply["remediation"].as_str().unwrap().is_empty());
    assert!(!reply.to_string().contains("not-real"), "the reply must not echo a token: {reply}");
    assert_eq!(client.recv(), None, "the connection is closed after unauthenticated");
}

#[test]
fn anything_but_a_good_auth_first_is_unauthenticated_and_closes() {
    let server = start();
    for first in [
        json!({"id": 1, "method": "app.info"}),
        json!({"id": 1, "method": "commands"}),
        json!({"id": 1, "method": "auth"}),
        json!({"id": 1, "method": "auth", "params": {}}),
        json!({"id": 1, "method": "auth", "params": {"token": 5}}),
        json!({"id": 1, "method": "auth", "params": {"token": TOKEN, "extra": 1}}),
        json!({"id": 1, "method": "auth", "params": [TOKEN]}),
        json!({"id": 1, "method": "auth", "params": {"token": ""}}),
    ] {
        let mut client = Client::connect(&server);
        let reply = client.ask(&first);
        assert_eq!(reply["code"], "unauthenticated", "{first}: {reply}");
        assert_eq!(client.recv(), None, "{first}");
    }
    for raw in ["{", "null", "not json", "\u{ff}"] {
        let mut client = Client::connect(&server);
        client.send(raw);
        let reply = client.recv().unwrap();
        assert_eq!((reply["code"].clone(), reply["id"].clone()), (json!("unauthenticated"), Value::Null), "{raw}");
        assert_eq!(client.recv(), None, "{raw}");
    }
    let mut client = Client::connect(&server);
    client.writer.write_all(&vec![b'x'; MAX_REQUEST + 1]).unwrap();
    client.send("");
    assert_eq!(client.recv().unwrap()["code"], "unauthenticated", "a request over the cap, before auth");
    assert_eq!(client.recv(), None);
}

#[test]
fn a_request_over_the_cap_is_too_large_and_the_connection_goes_on() {
    let server = start();
    let mut client = Client::authed(&server);
    client.writer.write_all(&vec![b'x'; MAX_REQUEST + 1]).unwrap();
    client.send("");
    let reply = client.recv().unwrap();
    assert_eq!((reply["ok"].clone(), reply["code"].clone()), (json!(false), json!("too_large")), "{reply}");
    assert!(!reply["remediation"].as_str().unwrap().is_empty());
    assert_eq!(client.ask(&json!({"id": 7, "method": "app.info"}))["id"], 7, "the next request is answered");
    // Exactly the cap is a request (a bad one, answered as such), not too large.
    client.send(&"x".repeat(MAX_REQUEST));
    assert_eq!(client.recv().unwrap()["code"], "bad_params");
}

#[test]
fn hostile_lines_are_answered_and_the_next_request_still_works() {
    let server = start();
    let mut client = Client::authed(&server);
    for line in {{crate_prefix_ident}}testkit::adversarial_lines() {
        client.writer.write_all(&line).unwrap();
        client.send("");
        assert_eq!(client.recv().unwrap()["ok"], false);
        assert_eq!(client.ask(&json!({"id": 9, "method": "app.info"}))["ok"], true);
    }
}

#[test]
fn connections_over_the_limit_are_told_and_closed_and_a_freed_slot_is_reused() {
    let server = start_with(|c| c.max_connections = 2);
    let (mut a, b) = (Client::authed(&server), Client::authed(&server));
    let mut over = Client::connect(&server);
    let reply = over.recv().unwrap();
    assert_eq!(reply["code"], "unavailable", "{reply}");
    assert_eq!(over.recv(), None);
    assert_eq!(a.ask(&json!({"id": 1, "method": "app.info"}))["ok"], true, "the connections in use are unaffected");
    drop(b);
    // The server notices the close on its next read; a new connection gets in once it has.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut again = Client::connect(&server);
        again.send(&json!({"id": 0, "method": "auth", "params": {"token": TOKEN}}).to_string());
        if again.recv().is_some_and(|r| r["ok"] == true) {
            break;
        }
        assert!(Instant::now() < deadline, "the freed slot was never reused");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_idle_connection_is_closed_and_so_is_one_that_trickles_a_request() {
    let server = start_with(|c| c.idle = Duration::from_millis(300));
    let mut client = Client::authed(&server);
    assert_eq!(client.recv(), None, "closed after the idle timeout");
    let mut slow = Client::authed(&server);
    slow.writer.write_all(b"{\"id\": 1, ").unwrap();
    assert_eq!(slow.recv(), None, "a half request doesn't hold the connection past the timeout");
    let mut silent = Client::connect(&server);
    assert_eq!(silent.recv(), None, "a connection that never sends auth is closed too");
}

#[test]
fn stop_closes_the_connections_and_the_port() {
    let mut server = start();
    let mut client = Client::authed(&server);
    let addr = server.addr();
    assert_eq!(server.connections(), 1);
    server.stop();
    // Nothing listens there: the test can listen there itself, at once. (A connect, refused, would
    // leave the freed port for seconds on Windows, as it tries again, for another process to take
    // and answer: mornycorp/swarm-core#97.)
    let rebound = TcpListener::bind(addr);
    assert!(rebound.is_ok(), "nothing listens after stop: {rebound:?}");
    assert_eq!(server.connections(), 0);
    assert_eq!(client.recv(), None);
    server.stop();
}

// A test-only slow command (the shipped registry has none): it works for `ms` unless its call is
// cancelled, and tells the test what happened to it.

#[derive(Default)]
struct Probe {
    started: AtomicUsize,
    cancelled: AtomicUsize,
    finished: AtomicUsize,
}

impl Probe {
    fn count(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitParams {
    /// How long to work, in milliseconds, unless cancelled.
    ms: u64,
}

#[derive(Serialize, schemars::JsonSchema)]
struct Waited {
    ms: u64,
}

struct Wait(Arc<Probe>);

impl Command for Wait {
    type Params = WaitParams;
    type Output = Waited;
    const ID: &'static str = "test.wait";
    const TITLE: &'static str = "Wait";
    const SUMMARY: &'static str = "Works for a while, polling its cancel token";

    fn run(&self, _: &Session, params: WaitParams, cx: &CallContext) -> Result<Waited, CommandError> {
        self.0.started.fetch_add(1, Ordering::SeqCst);
        let end = Instant::now() + Duration::from_millis(params.ms);
        let result = loop {
            if cx.check_cancelled().is_err() {
                self.0.cancelled.fetch_add(1, Ordering::SeqCst);
                break Err(CommandError::cancelled());
            }
            if Instant::now() >= end {
                break Ok(Waited { ms: params.ms });
            }
            thread::sleep(Duration::from_millis(5));
        };
        self.0.finished.fetch_add(1, Ordering::SeqCst);
        result
    }
}

/// A server whose session has `app.info` and `test.wait`, and what `test.wait` reports.
fn start_waiting(change: impl FnOnce(&mut Config)) -> (Server, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let mut registry = Registry::new();
    registry.register(InfoCommand).unwrap();
    registry.register(Wait(Arc::clone(&probe))).unwrap();
    let session = Arc::new(Session::with_registry(AppInfo { name: "demo".into(), version: "0.3.0".into() }, registry));
    let mut config = Config::new(Token::parse(TOKEN).unwrap(), Options::default());
    change(&mut config);
    (Server::start(session, config).unwrap(), probe)
}

fn wait_request(id: u64, ms: u64) -> String {
    json!({"id": id, "method": "test.wait", "params": {"ms": ms}}).to_string()
}

/// Polls `done` for up to 10 s: a server that never gets there fails the test, not hangs it.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Past this a cancel was not prompt: the calls here would otherwise work for 20 s.
const PROMPT: Duration = Duration::from_secs(5);

#[test]
fn a_call_is_cancelled_when_its_client_disconnects() {
    let (server, probe) = start_waiting(|_| {});
    let mut client = Client::authed(&server);
    client.send(&wait_request(1, 20_000));
    eventually("the call never started", || Probe::count(&probe.started) == 1);
    let dropped = Instant::now();
    drop(client);
    eventually("the call was not cancelled when its client went away", || Probe::count(&probe.finished) == 1);
    assert!(dropped.elapsed() < PROMPT, "{:?}", dropped.elapsed());
    assert_eq!(Probe::count(&probe.cancelled), 1, "the call's token was observed set");
    eventually("the connection's thread was not freed", || server.connections() == 0);
    // The server is healthy: the next client is served.
    let mut next = Client::authed(&server);
    assert_eq!(next.ask(&json!({"id": 2, "method": "app.info"}))["ok"], true);
    assert_eq!(Probe::count(&probe.started), 1, "nothing ran again");
}

#[test]
fn a_disconnect_is_seen_with_a_request_pipelined_behind_the_call() {
    let (server, probe) = start_waiting(|_| {});
    let mut client = Client::authed(&server);
    client.send(&wait_request(1, 20_000));
    client.send(&json!({"id": 2, "method": "app.info"}).to_string());
    eventually("the call never started", || Probe::count(&probe.started) == 1);
    drop(client);
    eventually("the call was not cancelled", || Probe::count(&probe.finished) == 1);
    assert_eq!(Probe::count(&probe.cancelled), 1);
    eventually("the connection's thread was not freed", || server.connections() == 0);
}

#[test]
fn a_request_pipelined_behind_a_call_is_answered_in_order() {
    let (server, probe) = start_waiting(|_| {});
    let mut client = Client::authed(&server);
    client.send(&wait_request(1, 200));
    client.send(&json!({"id": 2, "method": "app.info"}).to_string());
    client.send(&json!({"id": 3, "method": "app.nope"}).to_string());
    let replies: Vec<Value> = (0..3).map(|_| client.recv().unwrap()).collect();
    let ids: Vec<Value> = replies.iter().map(|r| r["id"].clone()).collect();
    assert_eq!(ids, [json!(1), json!(2), json!(3)]);
    assert_eq!(replies[0]["result"], json!({"ms": 200}), "the first call ran to its end");
    assert_eq!((replies[1]["ok"].clone(), replies[2]["code"].clone()), (json!(true), json!("unknown_command")));
    assert_eq!(Probe::count(&probe.cancelled), 0);
}

#[test]
fn one_clients_disconnect_does_not_cancel_anothers_call() {
    let (server, probe) = start_waiting(|_| {});
    let (mut stays, mut goes) = (Client::authed(&server), Client::authed(&server));
    stays.send(&wait_request(1, 600));
    goes.send(&wait_request(2, 20_000));
    eventually("the calls never started", || Probe::count(&probe.started) == 2);
    drop(goes);
    let reply = stays.recv().unwrap();
    assert_eq!(reply["result"], json!({"ms": 600}), "{reply}");
    assert_eq!(Probe::count(&probe.cancelled), 1, "only the call of the connection that went");
}

#[test]
fn a_call_longer_than_the_idle_timeout_is_not_idleness() {
    let (server, probe) = start_waiting(|c| c.idle = Duration::from_millis(300));
    let mut client = Client::authed(&server);
    client.send(&wait_request(1, 900));
    assert_eq!(client.recv().unwrap()["result"], json!({"ms": 900}), "the connection outlived the call");
    assert_eq!(Probe::count(&probe.cancelled), 0);
    assert_eq!(client.recv(), None, "and is closed once it has been idle for the timeout after it");
}

#[test]
fn stop_cancels_a_call_in_flight_and_returns_promptly() {
    let (mut server, probe) = start_waiting(|_| {});
    let mut client = Client::authed(&server);
    client.send(&wait_request(1, 20_000));
    eventually("the call never started", || Probe::count(&probe.started) == 1);
    let stopped = Instant::now();
    server.stop();
    assert!(stopped.elapsed() < PROMPT, "{:?}", stopped.elapsed());
    assert_eq!((Probe::count(&probe.cancelled), server.connections()), (1, 0));
    assert_eq!(client.recv().unwrap()["code"], "cancelled", "the client that is still there is told");
}

// Regressions of, each failing before its fix.

/// A short `test.wait` for each id of `ids`.
fn short_waits(client: &mut Client, ids: std::ops::RangeInclusive<u64>) {
    for id in ids {
        client.send(&wait_request(id, 50));
    }
}

/// Every reply the client gets until the server closes the connection.
fn rest(client: &mut Client) -> Vec<Value> {
    std::iter::from_fn(|| client.recv()).collect()
}

#[test]
fn stop_cancels_a_call_with_requests_queued_past_the_lookahead_and_runs_none_of_them() {
    let (mut server, probe) = start_waiting(|_| {});
    let mut client = Client::authed(&server);
    client.send(&wait_request(1, 20_000));
    short_waits(&mut client, 2..=6);
    eventually("the call never started", || Probe::count(&probe.started) == 1);
    // The reader has read the requests behind the call and waits for room to queue them.
    thread::sleep(Duration::from_millis(300));
    let stopped = Instant::now();
    server.stop();
    assert!(stopped.elapsed() < PROMPT, "{:?}", stopped.elapsed());
    assert_eq!((Probe::count(&probe.cancelled), server.connections()), (1, 0));
    let replies = rest(&mut client);
    assert_eq!(replies.len(), 1, "only the cancelled call is answered: {replies:?}");
    assert_eq!((replies[0]["id"].clone(), replies[0]["code"].clone()), (json!(1), json!("cancelled")));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(Probe::count(&probe.started), 1, "no request queued behind the call ran after the stop");
}

#[test]
fn a_disconnect_with_requests_queued_past_the_lookahead_cancels_the_call_and_runs_none_of_them() {
    let (server, probe) = start_waiting(|_| {});
    let mut client = Client::authed(&server);
    client.send(&wait_request(1, 20_000));
    short_waits(&mut client, 2..=6);
    eventually("the call never started", || Probe::count(&probe.started) == 1);
    thread::sleep(Duration::from_millis(300));
    let dropped = Instant::now();
    drop(client);
    eventually("the call was not cancelled when its client went away", || Probe::count(&probe.finished) == 1);
    assert!(dropped.elapsed() < PROMPT, "{:?}", dropped.elapsed());
    assert_eq!(Probe::count(&probe.cancelled), 1);
    eventually("the connection's thread was not freed", || server.connections() == 0);
    assert_eq!(Probe::count(&probe.started), 1, "no request queued behind the call ran after the disconnect");
}

#[test]
fn the_over_the_limit_reply_reaches_a_client_that_sent_auth_at_once() {
    let server = start_with(|c| c.max_connections = 1);
    let _held = Client::authed(&server);
    for attempt in 0..20 {
        let mut over = Client::connect(&server);
        over.send(&json!({"id": 0, "method": "auth", "params": {"token": TOKEN}}).to_string());
        // The server closes without reading the `auth`: the reply must survive that.
        thread::sleep(Duration::from_millis(50));
        let reply = over.recv();
        assert_eq!(reply.as_ref().map(|r| r["code"].clone()), Some(json!("unavailable")), "attempt {attempt}: {reply:?}");
        assert_eq!(over.recv(), None, "and the connection closes");
    }
}

#[test]
fn the_unauthenticated_reply_reaches_a_client_that_pipelined_requests_behind_a_bad_auth() {
    let server = start();
    for attempt in 0..20 {
        let mut client = Client::connect(&server);
        let mut burst = json!({"id": 5, "method": "auth", "params": {"token": "test-token-not-real-wrong-000"}}).to_string();
        burst.push('\n');
        for id in 0..2000 {
            burst.push_str(&json!({"id": id, "method": "app.info"}).to_string());
            burst.push('\n');
        }
        // The server may close before the burst is all sent: a failed write is that, not the test's failure.
        let _ = client.writer.write_all(burst.as_bytes());
        thread::sleep(Duration::from_millis(100));
        let reply = client.recv();
        let got = reply.as_ref().map(|r| (r["id"].clone(), r["code"].clone()));
        assert_eq!(got, Some((json!(5), json!("unauthenticated"))), "attempt {attempt}: {reply:?}");
    }
}

#[test]
fn silent_connections_do_not_lock_out_a_client_with_the_token() {
    let server = start();
    let silent: Vec<TcpStream> = (0..2 * MAX_CONNECTIONS).map(|_| TcpStream::connect(server.addr()).unwrap()).collect();
    thread::sleep(Duration::from_millis(200));
    let mut client = Client::authed(&server);
    assert_eq!(client.ask(&json!({"id": 1, "method": "app.info"}))["ok"], true);
    eventually("the silent connections were not closed to make room", || server.connections() <= MAX_UNAUTHENTICATED + 1);
    drop(silent);
}

#[test]
fn a_connection_that_does_not_authenticate_in_time_is_closed_long_before_the_idle_timeout() {
    let server = start_with(|c| c.auth_timeout = Duration::from_millis(300));
    let mut silent = Client::connect(&server);
    let waited = Instant::now();
    assert_eq!(silent.recv(), None, "closed at the auth deadline");
    assert!(waited.elapsed() < Duration::from_secs(5), "{:?}", waited.elapsed());
    let mut slow = Client::connect(&server);
    slow.writer.write_all(b"{\"id\": 0, \"method\": \"auth\", ").unwrap();
    assert_eq!(slow.recv(), None, "an `auth` trickled past the deadline is cut off too");
    let mut client = Client::authed(&server);
    thread::sleep(Duration::from_millis(600));
    assert_eq!(client.ask(&json!({"id": 1, "method": "app.info"}))["ok"], true, "after `auth`, the idle timeout applies");
}
