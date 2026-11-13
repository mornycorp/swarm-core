//! The process module: **the one place {{product}} starts another program**
//! (docs/app-architecture.md › Never hang on another program). `std::process::Command` appears
//! nowhere else in production code (the `process-module` scan holds that); a new reason to start a
//! program is a new function here, not a `Command` elsewhere.
//!
//! [`run`] starts a program and waits for it, and never hangs on it:
//!
//! - **stdin is closed** unless the caller feeds it, so a program that reads it gets end of input
//!   rather than waiting for ever;
//! - **a wall-clock deadline and an idle deadline** ([`Limits`]): the whole run, and the longest
//!   silence on its output. Past either the program is killed;
//! - **the output is capped** ([`Limits::output`] a stream) and drained on threads of its own, so a
//!   chatty program never blocks on a full pipe; over the cap is `too_large`, never a truncated
//!   result;
//! - **its own process group** (Unix), killed whole on a timeout, on a cancel ([`CancelToken`]) and
//!   on every other way out ([`Running`]'s `Drop`), so a grandchild never outlives the call or
//!   holds its pipe. On Windows the tree is ended with `taskkill /T /F`: a grandchild whose parent
//!   has already exited is not found by it, and the call still returns at its deadline;
//! - **the control socket's token** ([`TOKEN_ENV`]) is taken out of the child's environment: a
//!   program the app starts never inherits the secret that lets it drive the app;
//! - **every failure is a [`BackendError`]** with a remediation: the program isn't there
//!   (`not_found`), it printed too much (`too_large`), the caller cancelled (`cancelled`), it ran
//!   past a deadline or couldn't be waited for (`failed`). The engine turns each into a
//!   `CommandError` (`internal` for the last).
//!
//! A program that exits with a failure is not an error here: its code is in [`Ran`], and the
//! command that started it says what that means.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::{BackendError, CancelToken};

/// The environment variable that holds the control socket's token: never a child's.
pub const TOKEN_ENV: &str = "{{env}}_CONTROL_TOKEN";

/// How often a running program is looked at.
const POLL: Duration = Duration::from_millis(5);

/// How long a program may run, how long it may say nothing, and how much it may print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The whole run, output included.
    pub wall: Duration,
    /// The longest silence on stdout and stderr together.
    pub idle: Duration,
    /// The most bytes kept of each stream: more is `too_large`.
    pub output: usize,
}

impl Limits {
    /// A short tool: a minute in all, ten seconds of silence, 1 MiB a stream.
    pub const SHORT: Limits = Limits { wall: Duration::from_secs(60), idle: Duration::from_secs(10), output: 1024 * 1024 };
}

/// What a finished program left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// A started program: killed with what it started when this goes away unreaped, whichever way the
/// call ends.
struct Running {
    child: Child,
    reaped: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.reaped {
            kill_tree(&mut self.child);
        }
    }
}

/// Runs `program` with `args` in `dir` and waits for it within `limits`, feeding it `stdin` when
/// there is some (it is closed either way). `cancel` ends it early. See the module's doc for what
/// each failure is.
pub fn run(program: &Path, args: &[&str], dir: Option<&Path>, stdin: Option<&str>, limits: Limits, cancel: &CancelToken) -> Result<Ran, BackendError> {
    let what = format!("{} {args:?}", program.display());
    let mut command = prepared(program, args, dir, stdin.is_some());
    let started = Instant::now();
    let child = command.spawn().map_err(|e| not_started(program, &e))?;
    let mut running = Running { child, reaped: false };
    feed(&mut running.child, stdin);
    // When either stream last said something, in milliseconds since the start.
    let heard = Arc::new(AtomicU64::new(0));
    let stdout = running.child.stdout.take().map(|pipe| drain(pipe, limits.output, started, Arc::clone(&heard)));
    let stderr = running.child.stderr.take().map(|pipe| drain(pipe, limits.output, started, Arc::clone(&heard)));
    let code = wait(&mut running, &what, started, &heard, limits, cancel)?;
    let until = started + limits.wall;
    let (Some(stdout), Some(stderr)) = (collect(stdout, until), collect(stderr, until)) else {
        kill_tree(&mut running.child);
        return Err(timed_out(&what, "exited, but a program it started still holds its output", limits.wall));
    };
    let text = |(bytes, over): (Vec<u8>, bool), stream: &str| {
        if over {
            Err(BackendError::too_large(
                format!("{what} printed more than {} bytes on {stream}", limits.output),
                "ask it for less (a filter, a limit, a quieter mode), or raise the cap where this program is started",
            ))
        } else {
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
    };
    Ok(Ran { code, stdout: text(stdout, "stdout")?, stderr: text(stderr, "stderr")? })
}

/// `program` as it is started: its pipes, its own process group, and no control token.
fn prepared(program: &Path, args: &[&str], dir: Option<&Path>, fed: bool) -> Command {
    let mut command = Command::new(program);
    command.args(args).env_remove(TOKEN_ENV).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.stdin(if fed { Stdio::piped() } else { Stdio::null() });
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    own_group(&mut command);
    command
}

/// Why `program` didn't start.
fn not_started(program: &Path, e: &std::io::Error) -> BackendError {
    if e.kind() == std::io::ErrorKind::NotFound {
        return BackendError::not_found(format!("the program `{}` isn't on this machine", program.display()), "install it, or put it on the PATH");
    }
    BackendError::failed(format!("couldn't start `{}`: {e}", program.display()))
}

/// The error for a program that ran past a deadline (`how`), after `limit`.
fn timed_out(what: &str, how: &str, limit: Duration) -> BackendError {
    BackendError::failed(format!("{what} {how} after {} s, and was stopped", limit.as_secs_f32()))
}

/// Writes `stdin` to the child and closes it, on a thread that is never joined: a program that
/// never reads would hold it past the deadline. A program that exits without reading breaks the
/// pipe, which is no error.
fn feed(child: &mut Child, stdin: Option<&str>) {
    if let (Some(mut pipe), Some(text)) = (child.stdin.take(), stdin) {
        let text = text.as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&text);
        });
    }
}

/// Waits for the program: its exit code, or the error for a cancel, the wall deadline, the idle
/// deadline or a failed wait, each after killing it with what it started.
fn wait(running: &mut Running, what: &str, started: Instant, heard: &AtomicU64, limits: Limits, cancel: &CancelToken) -> Result<Option<i32>, BackendError> {
    loop {
        let stop = match running.child.try_wait() {
            Ok(Some(status)) => {
                running.reaped = true;
                return Ok(status.code());
            }
            Ok(None) => stop_now(what, started, heard, limits, cancel),
            Err(e) => Some(BackendError::failed(format!("couldn't wait for {what}: {e}"))),
        };
        if let Some(error) = stop {
            kill_tree(&mut running.child);
            running.reaped = true;
            return Err(error);
        }
        std::thread::sleep(POLL);
    }
}

/// Why a running program is to be stopped now, if it is.
fn stop_now(what: &str, started: Instant, heard: &AtomicU64, limits: Limits, cancel: &CancelToken) -> Option<BackendError> {
    let ran = started.elapsed();
    let silent = ran.saturating_sub(Duration::from_millis(heard.load(Ordering::Relaxed)));
    if cancel.is_cancelled() {
        Some(BackendError::cancelled())
    } else if ran >= limits.wall {
        Some(timed_out(what, "was still running", limits.wall))
    } else if silent >= limits.idle {
        Some(timed_out(what, "had printed nothing", limits.idle))
    } else {
        None
    }
}

/// A drain's bytes, if it has finished by `until`: `None` when the pipe is still open then. A
/// missing pipe is empty. The thread of one still open is left to end on its own.
fn collect(drain: Option<Receiver<(Vec<u8>, bool)>>, until: Instant) -> Option<(Vec<u8>, bool)> {
    let Some(receiver) = drain else { return Some((Vec::new(), false)) };
    receiver.recv_timeout(until.saturating_duration_since(Instant::now())).ok()
}

/// `pipe`'s bytes until it closes, on a thread of their own: at most `cap` of them kept, and
/// whether there were more. Each read is noted in `heard`, for the idle deadline.
fn drain(mut pipe: impl Read + Send + 'static, cap: usize, started: Instant, heard: Arc<AtomicU64>) -> Receiver<(Vec<u8>, bool)> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut kept, mut over) = (Vec::new(), false);
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    heard.store(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
                    let room = cap.saturating_sub(kept.len());
                    let read = chunk.get(..n).unwrap_or_default();
                    kept.extend_from_slice(read.get(..room.min(n)).unwrap_or_default());
                    over |= n > room;
                }
            }
        }
        // Nobody waits any more once a deadline passed: not an error here.
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
/// the child itself, in case that failed, and reaps it. Best effort: the call fails anyway.
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
    use crate::Kind;

    const QUICK: Limits = Limits { wall: Duration::from_secs(30), idle: Duration::from_secs(30), output: 1024 * 1024 };

    /// A shell that runs `script`: the same program on every machine the tests run on.
    fn shell(script: &str, stdin: Option<&str>, limits: Limits, cancel: &CancelToken) -> Result<Ran, BackendError> {
        #[cfg(windows)]
        let (program, flag) = ("cmd", "/c");
        #[cfg(not(windows))]
        let (program, flag) = ("sh", "-c");
        run(Path::new(program), &[flag, script], None, stdin, limits, cancel)
    }

    #[test]
    fn a_program_runs_and_its_output_and_code_come_back() {
        let ran = shell("echo out && echo err 1>&2 && exit 3", None, QUICK, &CancelToken::default()).unwrap();
        assert_eq!((ran.code, ran.stdout.trim(), ran.stderr.trim()), (Some(3), "out", "err"));
    }

    #[test]
    fn a_program_that_is_not_there_is_not_found() {
        let e = run(Path::new("no-such-program-anywhere"), &[], None, None, QUICK, &CancelToken::default()).unwrap_err();
        assert_eq!(e.kind, Kind::NotFound);
        assert!(e.message.contains("no-such-program-anywhere") && !e.remediation.is_empty(), "{e}");
    }

    /// Its stdin is closed: a program that reads it gets the end of input at once.
    #[cfg(unix)]
    #[test]
    fn a_program_that_reads_stdin_gets_the_end_of_input_or_what_it_is_fed() {
        let started = Instant::now();
        let ran = shell("cat; echo done", None, QUICK, &CancelToken::default()).unwrap();
        assert_eq!(ran.stdout.trim(), "done");
        assert!(started.elapsed() < Duration::from_secs(10));
        let fed = shell("cat", Some("fed\n"), QUICK, &CancelToken::default()).unwrap();
        assert_eq!(fed.stdout, "fed\n");
    }

    /// A program that never exits is stopped at the wall deadline, with the grandchild it started.
    #[cfg(unix)]
    #[test]
    fn a_program_that_never_exits_is_stopped_at_the_deadline_with_what_it_started() {
        let limits = Limits { wall: Duration::from_millis(400), ..QUICK };
        let started = Instant::now();
        let e = shell("sleep 30 & while true; do echo tick; sleep 0.05; done", None, limits, &CancelToken::default()).unwrap_err();
        assert_eq!(e.kind, Kind::Failed);
        assert!(e.message.contains("was still running") && e.message.contains("stopped"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    }

    /// A program that goes silent is stopped at the idle deadline, well before the wall one.
    #[cfg(unix)]
    #[test]
    fn a_silent_program_is_stopped_at_the_idle_deadline() {
        let limits = Limits { idle: Duration::from_millis(300), ..QUICK };
        let started = Instant::now();
        let e = shell("echo first; sleep 30", None, limits, &CancelToken::default()).unwrap_err();
        assert!(e.message.contains("had printed nothing"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    }

    /// A program that floods its output is never blocked on a full pipe, and is `too_large`.
    #[cfg(unix)]
    #[test]
    fn output_over_the_cap_is_too_large_never_a_truncated_result() {
        let limits = Limits { output: 1000, ..QUICK };
        let e = shell("i=0; while [ $i -lt 2000 ]; do echo 0123456789; i=$((i+1)); done", None, limits, &CancelToken::default()).unwrap_err();
        assert_eq!(e.kind, Kind::TooLarge);
        assert!(e.message.contains("1000 bytes on stdout"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_stops_the_program() {
        let cancel = CancelToken::default();
        cancel.cancel();
        let started = Instant::now();
        let e = shell("sleep 30", None, QUICK, &cancel).unwrap_err();
        assert_eq!(e.kind, Kind::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// A grandchild that outlives the program and holds its pipe: a failure at the deadline, not a
    /// wait for the grandchild.
    #[cfg(unix)]
    #[test]
    fn a_grandchild_holding_the_pipe_is_bounded_by_the_deadline() {
        let limits = Limits { wall: Duration::from_secs(2), ..QUICK };
        let started = Instant::now();
        let e = shell("sleep 20 &", None, limits, &CancelToken::default()).unwrap_err();
        assert!(e.message.contains("still holds its output"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
    }

    /// The control socket's token never reaches a child: the command that is started removes it,
    /// whatever this process's environment holds.
    #[test]
    fn the_control_token_is_taken_out_of_the_childs_environment() {
        let command = prepared(Path::new("x"), &[], None, false);
        assert!(command.get_envs().any(|(name, value)| name == TOKEN_ENV && value.is_none()), "{:?}", command.get_envs().collect::<Vec<_>>());
    }
}
