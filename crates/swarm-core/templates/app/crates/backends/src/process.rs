//! The process module: **the one place {{product}} starts another program** (docs/app-architecture.md
//! › Never hang on another program). `std::process::Command` is named nowhere else in production
//! code (the `process-module` scan holds that): a new reason to start a program is a [`Program`]
//! built here, or a new function here, never a `Command` elsewhere.
//!
//! [`Program::run`] starts a program, waits for it and never hangs on it:
//!
//! - **its stdin is closed**, or fed what the caller gives and then closed, so a program that reads
//!   it gets the end of its input instead of waiting for ever;
//! - **a wall-clock deadline and an idle deadline** ([`Limits`]): the whole run, and the longest
//!   silence on stdout and stderr together. Past either, the program is stopped;
//! - **its output is capped** ([`Limits::output`] a stream) and drained on threads of its own, so a
//!   chatty program never blocks on a full pipe. Past the cap it is stopped at once and the call is
//!   `too_large`, never a cut-short result;
//! - **what it starts is stopped with it.** On Unix it leads a process group of its own and the
//!   whole group is killed; on Windows its tree is (`taskkill /T /F`; a grandchild whose parent has
//!   already exited isn't found by it, and the call still returns at its deadline). That happens on
//!   a deadline, a cancel ([`CancelToken`]), output over the cap and every other way out, by
//!   `Drop`, so nothing it started outlives the call;
//! - **the control socket's token** ([`TOKEN_ENV`]) is taken out of its environment: a program the
//!   app starts never inherits the secret that lets it drive the app;
//! - **every failure is a [`BackendError`]** with a remediation, of its [`Kind`](crate::Kind): the
//!   program isn't there (`NotFound`), it printed too much (`TooLarge`), the caller cancelled
//!   (`Cancelled`), or it ran past a deadline, couldn't be started or couldn't be waited for
//!   (`Failed`). The engine turns each into a `CommandError`.
//!
//! A program that exits with a failure code is no error here: its code is in [`Ran`], and the
//! command that started it says what that means.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::{BackendError, CancelToken, clip};

/// The environment variable that carries the control socket's token: never a child's.
pub const TOKEN_ENV: &str = "{{env}}_CONTROL_TOKEN";

/// How long the run waits for news (output, an exit) before it looks at its deadlines again.
const POLL: Duration = Duration::from_millis(5);

/// How long the program that stops a tree (`kill`, `taskkill`) may take: it is bounded too.
const STOPPER_DEADLINE: Duration = Duration::from_secs(5);

/// The most a pipe is read at once.
const CHUNK: usize = 64 * 1024;

/// How long a program may run, how long it may say nothing, and how much it may print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The whole run, its output included.
    pub wall: Duration,
    /// The longest silence on stdout and stderr together.
    pub idle: Duration,
    /// The most bytes of each stream kept: more is `too_large`.
    pub output: usize,
}

impl Limits {
    /// A short tool's: a minute in all, ten seconds of silence, 1 MiB a stream.
    pub const SHORT: Limits = Limits { wall: Duration::from_secs(60), idle: Duration::from_secs(10), output: 1024 * 1024 };
}

/// What a finished program left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Its exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Ran {
    /// Whether it exited with code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Its stdout as text (a byte that isn't UTF-8 is `�`).
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Its stderr as text (a byte that isn't UTF-8 is `�`).
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// A program to run: what, with which arguments, where, with what on its stdin and within which
/// [`Limits`] ([`Limits::SHORT`] unless told).
#[derive(Debug, Clone)]
pub struct Program {
    program: PathBuf,
    args: Vec<OsString>,
    dir: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    stdin: Option<Vec<u8>>,
    limits: Limits,
}

impl Program {
    /// `program`: a path, or a name looked up on the `PATH`.
    pub fn new(program: impl Into<PathBuf>) -> Program {
        Program { program: program.into(), args: Vec::new(), dir: None, env: Vec::new(), stdin: None, limits: Limits::SHORT }
    }

    /// One more argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Program {
        self.args.push(arg.into());
        self
    }

    /// More arguments.
    #[must_use]
    pub fn args<A: Into<OsString>>(mut self, args: impl IntoIterator<Item = A>) -> Program {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Runs it in `dir` rather than the app's working directory.
    #[must_use]
    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Program {
        self.dir = Some(dir.into());
        self
    }

    /// Sets `name` in its environment (never [`TOKEN_ENV`]: that one is always taken out).
    #[must_use]
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Program {
        self.env.push((name.into(), value.into()));
        self
    }

    /// Feeds it `bytes` on its stdin, then closes it.
    #[must_use]
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Program {
        self.stdin = Some(bytes.into());
        self
    }

    /// Runs it within `limits`.
    #[must_use]
    pub fn limits(mut self, limits: Limits) -> Program {
        self.limits = limits;
        self
    }

    /// Starts it and waits for it within its [`Limits`], stopping it (with what it started) on a
    /// deadline, output over the cap or `cancel`: see the module's doc for each failure.
    pub fn run(&self, cancel: &CancelToken) -> Result<Ran, BackendError> {
        let what = self.describe();
        let started = Instant::now();
        let child = self.command().spawn().map_err(|e| self.not_started(&e))?;
        let mut running = Running { child, exited: None, finished: false };
        feed(running.child.stdin.take(), self.stdin.clone());
        let news = drain(&mut running.child);
        let mut heard = Heard { stdout: Vec::new(), stderr: Vec::new(), open: 2, last: started };
        loop {
            if let Some(stop) = self.stop_now(&what, started, &heard, running.exited.is_some(), cancel) {
                return Err(stop); // `running`'s drop stops it, with what it started
            }
            running.look(&what)?;
            if running.exited.is_some() && heard.open == 0 {
                break;
            }
            match news.recv_timeout(POLL) {
                Ok(news) => heard.take(news, self.limits.output, &what)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => heard.open = 0,
            }
        }
        running.finished = true;
        Ok(Ran { code: running.exited.and_then(|status| status.code()), stdout: heard.stdout, stderr: heard.stderr })
    }

    /// The `Command` it is started with: its pipes, its own process group, and no control token.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).envs(self.env.iter().map(|(k, v)| (k, v))).env_remove(TOKEN_ENV);
        command.stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(dir) = &self.dir {
            command.current_dir(dir);
        }
        own_group(&mut command);
        command
    }

    /// The program and its arguments, as an error names them: clipped, as they may come from input.
    fn describe(&self) -> String {
        let args: Vec<String> = self.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        clip(&format!("`{}` {args:?}", self.program.display()))
    }

    /// Why it didn't start.
    fn not_started(&self, e: &std::io::Error) -> BackendError {
        let program = clip(&self.program.display().to_string());
        if e.kind() == std::io::ErrorKind::NotFound {
            return BackendError::not_found(format!("the program `{program}` isn't on this machine"), "install it, or put it on the PATH");
        }
        BackendError::failed(format!("couldn't start `{program}`: {e}"), "check that it is a program this user can run")
    }

    /// Why the run stops now, if it does: a cancel, the wall deadline, or the idle one.
    fn stop_now(&self, what: &str, started: Instant, heard: &Heard, exited: bool, cancel: &CancelToken) -> Option<BackendError> {
        let held = if exited { "exited, but a program it started still holds its output" } else { "was still running" };
        if cancel.is_cancelled() {
            Some(BackendError::cancelled())
        } else if started.elapsed() >= self.limits.wall {
            Some(past_deadline(what, held, "deadline", self.limits.wall))
        } else if heard.last.elapsed() >= self.limits.idle {
            Some(past_deadline(what, &format!("{held} and printed nothing"), "idle deadline", self.limits.idle))
        } else {
            None
        }
    }
}

/// The error of a program stopped at its `deadline` (`which`), and how it was (`how`).
fn past_deadline(what: &str, how: &str, which: &str, deadline: Duration) -> BackendError {
    BackendError::failed(
        format!("{what} {how} at its {which} of {} ms, and was stopped", deadline.as_millis()),
        "the program hung or is slow: run it by hand to see why, or give it longer where it is started",
    )
}

/// A started program, stopped (with what it started) when this goes away unless it finished.
struct Running {
    child: Child,
    /// Its status, once it has exited and been reaped.
    exited: Option<ExitStatus>,
    /// It exited and its pipes closed: nothing is left to stop.
    finished: bool,
}

impl Running {
    /// Notes its exit, if it has exited.
    fn look(&mut self, what: &str) -> Result<(), BackendError> {
        if self.exited.is_none() {
            self.exited = self.child.try_wait().map_err(|e| BackendError::failed(format!("couldn't wait for {what}: {e}"), ""))?;
        }
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.finished {
            stop_tree(&mut self.child);
        }
    }
}

/// Writes `bytes` to the program's stdin and closes it, on a thread that is never joined: a program
/// that never reads would hold it past the deadline. One that exits unread breaks the pipe, which
/// is no error.
fn feed(pipe: Option<ChildStdin>, bytes: Option<Vec<u8>>) {
    if let (Some(mut pipe), Some(bytes)) = (pipe, bytes) {
        std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
    }
}

/// Which of the program's output streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    fn name(self) -> &'static str {
        match self {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        }
    }
}

/// What a drain thread says: bytes a stream gave, or that it closed.
enum News {
    Bytes(Stream, Vec<u8>),
    Closed,
}

/// Reads stdout and stderr on a thread each, until each closes, and tells the one channel.
fn drain(child: &mut Child) -> Receiver<News> {
    let (sender, receiver) = mpsc::channel();
    match child.stdout.take() {
        Some(pipe) => read_on_a_thread(pipe, Stream::Stdout, sender.clone()),
        None => drop(sender.send(News::Closed)),
    }
    match child.stderr.take() {
        Some(pipe) => read_on_a_thread(pipe, Stream::Stderr, sender),
        None => drop(sender.send(News::Closed)),
    }
    receiver
}

/// `pipe`'s bytes as they come, then that it closed. The thread ends when the pipe closes or nobody
/// listens any more; it is never joined (a grandchild may hold the pipe past the call).
fn read_on_a_thread(mut pipe: impl Read + Send + 'static, stream: Stream, sender: Sender<News>) {
    std::thread::spawn(move || {
        let mut chunk = vec![0_u8; CHUNK];
        loop {
            let news = match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => News::Bytes(stream, chunk.get(..n).unwrap_or_default().to_vec()),
            };
            if sender.send(news).is_err() {
                return;
            }
        }
        let _ = sender.send(News::Closed);
    });
}

/// The output so far, how many streams are still open, and when the program last said something.
struct Heard {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    open: usize,
    last: Instant,
}

impl Heard {
    /// Takes in `news`: more than `cap` bytes on a stream is `too_large`.
    fn take(&mut self, news: News, cap: usize, what: &str) -> Result<(), BackendError> {
        match news {
            News::Closed => self.open = self.open.saturating_sub(1),
            News::Bytes(stream, bytes) => {
                self.last = Instant::now();
                let kept = if stream == Stream::Stdout { &mut self.stdout } else { &mut self.stderr };
                if kept.len().saturating_add(bytes.len()) > cap {
                    return Err(BackendError::too_large(
                        format!("{what} printed more than {cap} bytes on {}, and was stopped", stream.name()),
                        "ask it for less (a filter, a limit, a quieter mode), or raise the cap where it is started",
                    ));
                }
                kept.extend_from_slice(&bytes);
            }
        }
        Ok(())
    }
}

/// Makes the program lead a process group of its own, so a kill reaches what it starts.
#[cfg(unix)]
fn own_group(command: &mut Command) {
    std::os::unix::process::CommandExt::process_group(command, 0);
}

/// Windows has no process group a kill reaches: [`stop_tree`] walks the tree instead.
#[cfg(not(unix))]
fn own_group(_command: &mut Command) {}

/// Stops `child` and what it started: its process group (Unix) or its tree (Windows), then the
/// child itself in case that failed, and reaps it. Best effort: the call has failed already. The
/// child's handle is still held, so its id can't have been given to another process.
fn stop_tree(child: &mut Child) {
    if let Some(mut stopper) = tree_stopper(child.id())
        && let Ok(mut stopper) = stopper.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    {
        let until = Instant::now() + STOPPER_DEADLINE;
        while matches!(stopper.try_wait(), Ok(None)) && Instant::now() < until {
            std::thread::sleep(POLL);
        }
        let _ = stopper.kill();
        let _ = stopper.wait();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The program that stops process `pid`'s group.
#[cfg(unix)]
fn tree_stopper(pid: u32) -> Option<Command> {
    let mut kill = Command::new("kill");
    kill.args(["-KILL", "--", &format!("-{pid}")]);
    Some(kill)
}

/// The program that stops process `pid` and what it started.
#[cfg(windows)]
fn tree_stopper(pid: u32) -> Option<Command> {
    let mut taskkill = Command::new("taskkill");
    taskkill.args(["/T", "/F", "/PID", &pid.to_string()]);
    Some(taskkill)
}

/// Elsewhere only the program itself is stopped.
#[cfg(not(any(unix, windows)))]
fn tree_stopper(_pid: u32) -> Option<Command> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    /// The variable that tells [`child`] what to do when this test binary runs it as a program.
    const ROLE: &str = "PROCESS_TESTS_CHILD_ROLE";

    /// Limits no test reaches unless it means to.
    const ROOMY: Limits = Limits { wall: Duration::from_secs(60), idle: Duration::from_secs(60), output: 1024 * 1024 };

    /// This test binary, run again as [`child`] playing `role`: a program that behaves the same on
    /// every machine, with no shell and no tool the machine may lack.
    fn playing(role: &str) -> Program {
        let this = std::env::current_exe().unwrap();
        Program::new(this).args(["--exact", "process::tests::child", "--nocapture", "--test-threads=1"]).env(ROLE, role).limits(ROOMY)
    }

    /// Not a test of its own: the program the tests run. With no role it does nothing.
    #[test]
    fn child() {
        let Ok(role) = std::env::var(ROLE) else { return };
        let mut out = std::io::stdout();
        match role.as_str() {
            "speak" => {
                write!(out, "<out>").unwrap();
                out.flush().unwrap();
                eprint!("<err>");
                std::process::exit(3);
            }
            "echo" => {
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                write!(out, "<read {} bytes: {}>", input.len(), String::from_utf8_lossy(&input)).unwrap();
                out.flush().unwrap();
            }
            "flood" => loop {
                out.write_all(&[b'x'; CHUNK]).unwrap();
            },
            "grandparent" => {
                // A grandchild that keeps this program's stdout open long after it exits.
                let this = std::env::current_exe().unwrap();
                #[expect(clippy::zombie_processes, reason = "the grandchild outlives this program on purpose: that is the case under test")]
                let _ = Command::new(this).args(["--exact", "process::tests::child"]).env(ROLE, "nap").stdin(Stdio::null()).spawn().unwrap();
            }
            "nap" => std::thread::sleep(Duration::from_secs(20)),
            _ => std::thread::sleep(Duration::from_secs(120)),
        }
    }

    #[test]
    fn a_program_runs_and_its_code_and_output_come_back() {
        let ran = playing("speak").run(&CancelToken::default()).unwrap();
        assert_eq!(ran.code, Some(3), "a failure code is no error: {ran:?}");
        assert!(!ran.success() && ran.stdout_text().contains("<out>") && ran.stderr_text().contains("<err>"), "{ran:?}");
    }

    /// Its stdin is closed when nothing is fed, so a program that reads it ends at once; what is fed
    /// is read, then the end.
    #[test]
    fn stdin_is_closed_or_fed_then_closed() {
        let ran = playing("echo").run(&CancelToken::default()).unwrap();
        assert!(ran.success() && ran.stdout_text().contains("<read 0 bytes: >"), "{ran:?}");
        let fed = playing("echo").stdin("fed\n").run(&CancelToken::default()).unwrap();
        assert!(fed.stdout_text().contains("<read 4 bytes: fed\n>"), "{fed:?}");
    }

    #[test]
    fn a_program_that_is_not_there_is_not_found() {
        let e = Program::new("no-such-program-anywhere-at-all").run(&CancelToken::default()).unwrap_err();
        assert_eq!(e.kind, Kind::NotFound);
        assert!(e.message.contains("no-such-program-anywhere-at-all") && !e.remediation.is_empty(), "{e}");
    }

    /// The wall deadline expires: a program that never exits is stopped there, not waited for.
    #[test]
    fn a_program_that_never_exits_is_stopped_at_the_deadline() {
        let started = Instant::now();
        let e = playing("sleep").limits(Limits { wall: Duration::from_millis(500), ..ROOMY }).run(&CancelToken::default()).unwrap_err();
        assert_eq!(e.kind, Kind::Failed);
        assert!(e.message.contains("was still running at its deadline of 500 ms, and was stopped"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    }

    /// The idle deadline expires: a program that goes silent is stopped long before its wall one.
    #[test]
    fn a_silent_program_is_stopped_at_the_idle_deadline() {
        let started = Instant::now();
        let e = playing("sleep").limits(Limits { idle: Duration::from_millis(500), ..ROOMY }).run(&CancelToken::default()).unwrap_err();
        assert!(e.message.contains("printed nothing at its idle deadline of 500 ms"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    }

    /// The cap: a program that floods its output is stopped at once and is `too_large`, never a
    /// cut-short result, and never blocked on a full pipe.
    #[test]
    fn output_over_the_cap_is_too_large() {
        let started = Instant::now();
        let e = playing("flood").limits(Limits { output: 10_000, ..ROOMY }).run(&CancelToken::default()).unwrap_err();
        assert_eq!(e.kind, Kind::TooLarge);
        assert!(e.message.contains("more than 10000 bytes on stdout"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    }

    #[test]
    fn a_cancel_stops_the_program() {
        let cancel = CancelToken::default();
        let canceller = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                cancel.cancel();
            })
        };
        let started = Instant::now();
        let e = playing("sleep").run(&cancel).unwrap_err();
        canceller.join().unwrap();
        assert_eq!(e.kind, Kind::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    }

    /// A program that exits but leaves a grandchild holding its output: a failure at the deadline,
    /// not a wait for the grandchild.
    #[test]
    fn a_grandchild_holding_the_output_is_bounded_by_the_deadline() {
        let started = Instant::now();
        let e = playing("grandparent").limits(Limits { wall: Duration::from_secs(3), ..ROOMY }).run(&CancelToken::default()).unwrap_err();
        assert!(e.message.contains("exited, but a program it started still holds its output"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
    }

    /// The control socket's token never reaches a program the app starts, even one told to set it.
    #[test]
    fn the_control_token_is_taken_out_of_the_programs_environment() {
        let command = Program::new("x").env(TOKEN_ENV, "test-token-not-real-0123").env("KEPT", "1").command();
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&(std::ffi::OsStr::new(TOKEN_ENV), None)), "{envs:?}");
        assert!(envs.contains(&(std::ffi::OsStr::new("KEPT"), Some(std::ffi::OsStr::new("1")))), "{envs:?}");
    }
}
