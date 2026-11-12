//! A program run to a deadline: what an app's CLI surface needs in a test (an app's tests start
//! its binary, and a hung binary must fail the test, not stall the run). Standard library only.
//!
//! The deadline bounds the whole call, output included (docs/app-architecture.md › Never hang on
//! another program): past it the program is killed with what it started, and the call returns
//! without waiting for its pipes. A program that exits but leaves a process of its own holding its
//! output is a failure at the deadline too, not a hang.
//!
//! - **Unix:** the program leads its own process group, and a kill is of the group (`kill -KILL
//!   -<pgid>`), so a grandchild dies with it.
//! - **Windows:** a job object needs FFI, which this crate (`forbid(unsafe_code)`) doesn't do. A
//!   kill is `taskkill /T /F`, which ends the program and every process it started that is still
//!   running; a grandchild whose parent has already exited is not found by it and lives on. The
//!   call still returns at the deadline: the threads that drain its pipes are left behind (they end
//!   when the last holder of the pipe exits), never waited for.
//!
//! The output kept is capped ([`OUTPUT_CAP`] a stream): past the cap the pipe is still drained, so
//! the program never blocks, and the call fails.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// The most a stream's output may hold: a program that prints more fails the call.
pub const OUTPUT_CAP: usize = 16 * 1024 * 1024;

/// What a finished program left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `program` with `args`, feeds it `stdin` and closes it, and waits at most `deadline` for it
/// and its output: past it the program is killed (with what it started, see the module's doc) and
/// the call fails. Output is drained on threads, so a chatty program never blocks on a full pipe.
/// An `Err` says what went wrong, never panics.
pub fn run(program: &Path, args: &[&str], stdin: &str, deadline: Duration) -> Result<Ran, String> {
    run_capped(program, args, stdin, deadline, OUTPUT_CAP)
}

/// [`run`], keeping at most `cap` bytes of each stream.
fn run_capped(program: &Path, args: &[&str], stdin: &str, deadline: Duration, cap: usize) -> Result<Ran, String> {
    let until = Instant::now() + deadline;
    let what = format!("{} {args:?}", program.display());
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    own_group(&mut command);
    let mut child = command.spawn().map_err(|e| format!("start {}: {e}", program.display()))?;
    if let Some(mut pipe) = child.stdin.take() {
        let text = stdin.as_bytes().to_vec();
        // A program that exits without reading its stdin breaks the pipe: not an error here. The
        // thread is never joined: a program that never reads would hold it past the deadline.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&text);
        });
    }
    let stdout = child.stdout.take().map(|pipe| drain(pipe, cap));
    let stderr = child.stderr.take().map(|pipe| drain(pipe, cap));
    let status = wait(&mut child, until, &what)?;
    let (Some(stdout), Some(stderr)) = (collect(stdout, until), collect(stderr, until)) else {
        kill_tree(&mut child);
        return Err(format!("{what} exited, but its output stayed open past the {deadline:?} deadline: a process it started holds it, and was killed"));
    };
    let text = |(bytes, over): (Vec<u8>, bool), name: &str| {
        if over { Err(format!("{what} printed more than the {cap}-byte cap on {name}")) } else { Ok(String::from_utf8_lossy(&bytes).into_owned()) }
    };
    Ok(Ran { code: status.code(), stdout: text(stdout, "stdout")?, stderr: text(stderr, "stderr")? })
}

/// Waits for `child` until `until`; past it, kills it with what it started.
fn wait(child: &mut Child, until: Instant, what: &str) -> Result<std::process::ExitStatus, String> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= until => {
                kill_tree(child);
                return Err(format!("{what} did not finish within the deadline, and was killed"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                kill_tree(child);
                return Err(format!("wait for {what}: {e}"));
            }
        }
    }
}

/// A drain's bytes, if it has finished by `until`: `None` when the pipe is still open then. A
/// missing pipe is empty. The thread of one still open is left to end on its own.
fn collect(drain: Option<Receiver<(Vec<u8>, bool)>>, until: Instant) -> Option<(Vec<u8>, bool)> {
    let Some(receiver) = drain else { return Some((Vec::new(), false)) };
    receiver.recv_timeout(until.saturating_duration_since(Instant::now())).ok()
}

/// `pipe`'s bytes until it closes, on a thread of their own: at most `cap` of them kept, and
/// whether there were more.
fn drain(mut pipe: impl Read + Send + 'static, cap: usize) -> Receiver<(Vec<u8>, bool)> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut kept, mut over) = (Vec::new(), false);
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(kept.len());
                    let read = chunk.get(..n).unwrap_or_default();
                    kept.extend_from_slice(read.get(..room.min(n)).unwrap_or_default());
                    over |= n > room;
                }
            }
        }
        // Nobody waits any more when the deadline passed: not an error here.
        let _ = sender.send((kept, over));
    });
    receiver
}

/// Makes the program the leader of a process group of its own, so a kill reaches what it starts.
#[cfg(unix)]
fn own_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// Windows has no process group a kill reaches; [`kill_tree`] walks the tree instead.
#[cfg(not(unix))]
fn own_group(_command: &mut Command) {}

/// Kills `child` and what it started: its process group (Unix), its process tree (Windows). Then
/// the child itself, in case that failed, and reaps it. Best effort: each step's failure is ignored,
/// since the call fails anyway.
fn kill_tree(child: &mut Child) {
    if let Some(mut killer) = tree_killer(child.id()) {
        let _ = killer.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The program that kills process `pid` and what it started.
#[cfg(unix)]
fn tree_killer(pid: u32) -> Option<Command> {
    let mut kill = Command::new("kill");
    kill.args(["-KILL", "--", &format!("-{pid}")]);
    Some(kill)
}

/// The program that kills process `pid` and what it started.
#[cfg(windows)]
fn tree_killer(pid: u32) -> Option<Command> {
    let mut taskkill = Command::new("taskkill");
    taskkill.args(["/T", "/F", "/PID", &pid.to_string()]);
    Some(taskkill)
}

/// Elsewhere only the program itself is killed.
#[cfg(not(any(unix, windows)))]
fn tree_killer(_pid: u32) -> Option<Command> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test binary, started again to run one test of its own: a program that behaves
    /// the same on every platform.
    fn this_binary() -> std::path::PathBuf {
        std::env::current_exe().unwrap()
    }

    #[test]
    fn a_program_runs_and_its_output_comes_back() {
        let ran = run(&this_binary(), &["--list", "--format", "terse"], "ignored stdin", Duration::from_secs(60)).unwrap();
        assert_eq!(ran.code, Some(0));
        assert!(ran.stdout.contains("hang_for_real"), "{}", ran.stdout);
    }

    #[test]
    fn a_program_that_hangs_is_killed_at_the_deadline() {
        let started = Instant::now();
        let args = ["--exact", "process::tests::hang_for_real", "--ignored", "--nocapture"];
        let e = run(&this_binary(), &args, "", Duration::from_millis(500)).unwrap_err();
        assert!(e.contains("deadline") && e.contains("killed"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// Hangs when run. It is `#[ignore]`d: only the test above runs it, as a child.
    #[test]
    #[ignore = "hangs on purpose: a_program_that_hangs_is_killed_at_the_deadline runs it as a child"]
    fn hang_for_real() {
        std::thread::sleep(Duration::from_secs(120));
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error() {
        let e = run(Path::new("swarm-testkit-no-such-program"), &[], "", Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("start swarm-testkit-no-such-program"), "{e}");
    }

    /// Output past the cap fails the call, and the program is never blocked on its pipe.
    #[test]
    fn output_over_the_cap_is_an_error() {
        let ran = run_capped(&this_binary(), &["--list", "--format", "terse"], "", Duration::from_secs(60), 10);
        let e = ran.unwrap_err();
        assert!(e.contains("10-byte cap on stdout"), "{e}");
    }

    /// A shell that runs `script` and the time the call took, with `deadline`.
    fn shell(script: &str, deadline: Duration) -> (Result<Ran, String>, Duration) {
        let started = Instant::now();
        #[cfg(windows)]
        let ran = run(Path::new("cmd"), &["/c", script], "", deadline);
        #[cfg(not(windows))]
        let ran = run(Path::new("sh"), &["-c", script], "", deadline);
        (ran, started.elapsed())
    }

    /// The reviewer's case (#31): the program hangs, and a grandchild it started holds its stdout. The
    /// deadline bounds the call: before, a 500 ms deadline took 29 s.
    #[test]
    fn a_hung_program_with_a_grandchild_holding_its_pipe_is_bounded_by_the_deadline() {
        #[cfg(windows)]
        let script = "start /b ping -n 11 127.0.0.1 & ping -n 30 127.0.0.1";
        #[cfg(not(windows))]
        let script = "sleep 11 & sleep 30";
        let (ran, took) = shell(script, Duration::from_millis(500));
        let e = ran.unwrap_err();
        assert!(e.contains("deadline") && e.contains("killed"), "{e}");
        assert!(took < Duration::from_secs(5), "the 500 ms deadline held for {took:?}");
    }

    /// The program exits at once, but a grandchild keeps its stdout open: a failure at the deadline,
    /// not a wait for the grandchild. The deadline is long enough for the shell itself to exit (a
    /// busy Windows machine can take over 500 ms to start `cmd`, which then fails the wait instead),
    /// and far short of the grandchild's 10 s.
    #[test]
    fn a_grandchild_that_outlives_the_program_and_holds_its_pipe_is_bounded_by_the_deadline() {
        #[cfg(windows)]
        let script = "start /b ping -n 11 127.0.0.1";
        #[cfg(not(windows))]
        let script = "sleep 11 &";
        let (ran, took) = shell(script, Duration::from_secs(3));
        let e = ran.unwrap_err();
        assert!(e.contains("output stayed open"), "{e}");
        assert!(took < Duration::from_secs(7), "the 3 s deadline held for {took:?}");
    }
}
