//! Test helpers for {{product}}: temp dirs, hostile request lines and a port no app answers on. It
//! sits outside the layer stack and is **only a `[dev-dependencies]` entry**, never a dependency of
//! production code (docs/app-architecture.md › Layers). It uses no crate of the workspace, so any
//! layer's tests can use it without a dependency cycle.
//!
//! - [`temp_dir`]: where a test keeps config and state (docs/app-architecture.md › Tests touch
//!   nothing real).
//! - [`adversarial_lines`]: what a hostile caller sends a transport.
//! - [`DeadPort`]: a loopback port where the app is gone, held so that no other process takes it.
//!
//! **The conformance run is not here.** It is swarm-core's: `swarm_core_testkit::conformance`
//! (a `[dev-dependencies]` entry, pinned beside `swarm-core`) drives every command through every
//! surface and compares, and so does `adversarial_params`, the hostile params a command is sent. An
//! app implements `conformance::App` once (`apps/{{cli}}/tests/surfaces.rs`) and gets the run's
//! improvements by bumping the pin, instead of keeping a copy of the logic.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A fresh, empty directory for the test called `name`: cleared first, so what an earlier run left
/// behind (the OS reuses process ids) never meets this one. `name` is unique among the tests of one
/// binary, which run in parallel threads of one process.
pub fn temp_dir(name: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("app-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Raw request lines (no trailing newline) that a transport must answer with an error and survive:
/// not JSON, not a request, not UTF-8, deeply nested.
pub fn adversarial_lines() -> Vec<Vec<u8>> {
    let owned = |s: &str| s.as_bytes().to_vec();
    vec![
        owned("{"),
        owned("null"),
        owned("[]"),
        owned("\"app.info\""),
        owned("{\"id\": 1}"),
        owned("{\"method\": \"app.info\"}"),
        owned("{\"id\": null, \"method\": \"app.info\"}"),
        owned("{\"id\": {}, \"method\": \"app.info\"}"),
        owned("{\"id\": 1, \"method\": 5}"),
        owned("{\"id\": 1, \"method\": \"app.info\", \"params\": 5}"),
        owned("{\"id\": 1, \"method\": \"app.info\", \"params\": [1]}"),
        vec![0xff, 0xfe, 0xfd],
        vec![b'{', 0, b'}'],
        "[".repeat(100_000).into_bytes(),
        ["{\"id\": 1, \"method\": \"", &"a".repeat(100_000), "\"}"].concat().into_bytes(),
    ]
}

/// How often a [`DeadPort`] looks for a connection to close.
const POLL: Duration = Duration::from_millis(5);

/// A port on `127.0.0.1` where the app is gone: each connection is closed as soon as it is made, and
/// no other process can take the port while this lives. A test that wants a port nothing answers on
/// uses it, never a port it bound, read and freed: another process (an app under test in parallel)
/// can take a freed port before the test connects to it (mornycorp/swarm-core#97).
///
/// A client sees its connection made and then closed before anything is answered, as from an app
/// that is going away, rather than refused: std can't hold a port without listening on it.
#[derive(Debug)]
pub struct DeadPort {
    port: u16,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    closer: Option<JoinHandle<()>>,
}

impl DeadPort {
    /// A free port, held from now on.
    pub fn new() -> io::Result<DeadPort> {
        DeadPort::on(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?)
    }

    /// The port `listener` holds, closing each connection from now on: a test's fake app that is
    /// done hands its listener here, so its port is never free.
    pub fn on(listener: TcpListener) -> io::Result<DeadPort> {
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let (accepted, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (count, stopping) = (Arc::clone(&accepted), Arc::clone(&stop));
        let closer = thread::Builder::new().name(format!("dead-port-{port}")).spawn(move || close_each(&listener, &count, &stopping))?;
        Ok(DeadPort { port, accepted, stop, closer: Some(closer) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The connections it has closed: one a client made.
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

impl Drop for DeadPort {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(closer) = self.closer.take() {
            let _ = closer.join();
        }
    }
}

/// Accepts and closes each connection on `listener` until `stop`, counting them. It polls, so a stop
/// never waits for a connection.
fn close_each(listener: &TcpListener, accepted: &AtomicUsize, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                accepted.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
            // Nothing waiting, or a connection that died before it was accepted: look again.
            Err(_) => thread::sleep(POLL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_dir_is_fresh_and_empty() {
        let dir = temp_dir("testkit-fresh").unwrap();
        std::fs::write(dir.join("left-behind"), "x").unwrap();
        let again = temp_dir("testkit-fresh").unwrap();
        assert_eq!(dir, again);
        assert_eq!(std::fs::read_dir(&again).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&again);
    }

    #[test]
    fn a_dead_port_closes_each_connection_and_no_one_else_can_take_it() {
        use std::io::Read;
        let dead = DeadPort::new().unwrap();
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, dead.port())).is_err(), "the port is held");
        for _ in 0..2 {
            let mut stream = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, dead.port())).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let read = stream.read(&mut [0_u8; 1]);
            let waited = |e: &std::io::Error| matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock);
            assert!(matches!(read, Ok(0)) || read.as_ref().is_err_and(|e| !waited(e)), "closed without an answer: {read:?}");
        }
        assert_eq!(dead.accepted(), 2);
    }

    #[test]
    fn the_adversarial_lines_include_one_that_is_not_utf8() {
        assert!(adversarial_lines().iter().any(|l| std::str::from_utf8(l).is_err()));
    }
}
