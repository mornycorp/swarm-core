//! Running git, gh and other tools: every call has a deadline, and a failure says what failed
//! and the tool's own words.

use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// How long one git call may take. Swarm Core's own tests allow more: on a desktop loaded by
/// two swarms' builds, a test's git can take over 20 s, and a test that passes alone then fails; slow there is load, not a fault.
pub const GIT_DEADLINE: Duration = if cfg!(test) { Duration::from_secs(120) } else { Duration::from_secs(20) };

/// Run `cmd` with `stdin`, within `deadline`; its stdout, or why it failed (`what: <stderr>`).
pub fn output(cmd: Command, stdin: Option<&[u8]>, deadline: Duration, what: &str) -> Result<String, String> {
    output_bytes(cmd, stdin, deadline, what).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// [`output`], its stdout as bytes.
pub fn output_bytes(cmd: Command, stdin: Option<&[u8]>, deadline: Duration, what: &str) -> Result<Vec<u8>, String> {
    let (ok, out, err) = captured(cmd, stdin, deadline, what)?;
    if ok { Ok(out) } else { Err(format!("{what}: {}", String::from_utf8_lossy(&err).trim())) }
}

/// Why a call gave no answer: it couldn't start or be waited on, or it ran past its deadline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unanswered {
    /// It couldn't start, take its stdin or be waited on, or a signal ended it: `what: why`.
    Failed(String),
    /// It ran past `after` and was stopped.
    TimedOut { what: String, after: Duration },
}

impl fmt::Display for Unanswered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(why) => f.write_str(why),
            Self::TimedOut { what, after } => write!(f, "{what} took longer than {}s", after.as_secs()),
        }
    }
}

/// Run `cmd` with `stdin`, within `deadline`: whether it succeeded, its stdout and its stderr. A
/// command that couldn't start or ran past its deadline is the error.
pub fn captured(cmd: Command, stdin: Option<&[u8]>, deadline: Duration, what: &str) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
    let (status, out, err) = finished(cmd, stdin, deadline, what).map_err(|e| e.to_string())?;
    Ok((status.success(), out, err))
}

/// Run `cmd` with `stdin`, within `deadline`: how it exited, its stdout and its stderr.
fn finished(mut cmd: Command, stdin: Option<&[u8]>, deadline: Duration, what: &str) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), Unanswered> {
    let failed = |e: std::io::Error| Unanswered::Failed(format!("{what}: {e}"));
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(failed)?;
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut out);
            }
            out
        })
    };
    let out = reader(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = reader(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    // The readers run first: a child that fills its stdout before reading all of stdin can't block.
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes).map_err(failed)?;
    }
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Unanswered::TimedOut { what: what.to_owned(), after: deadline });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(failed(e)),
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    Ok((status, out, err))
}

/// Run git in `dir` within `deadline`, for a command whose exit code is the answer
/// (`merge-base --is-ancestor`, `merge-tree`, `grep`): the code. A signal is [`Unanswered::Failed`].
pub fn git_code(dir: &Path, args: &[&str], deadline: Duration) -> Result<i32, Unanswered> {
    git_status_output(dir, args, deadline).map(|(code, _)| code)
}

/// [`git_code`] and git's stdout: `grep` lists what it found, and exit 1 is none.
pub fn git_status_output(dir: &Path, args: &[&str], deadline: Duration) -> Result<(i32, String), Unanswered> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args).env("GIT_TERMINAL_PROMPT", "0");
    let what = format!("git {}", args.join(" "));
    let (status, out, err) = finished(cmd, None, deadline, &what)?;
    let code = status.code().ok_or_else(|| Unanswered::Failed(format!("{what}: ended by a signal: {}", String::from_utf8_lossy(&err).trim())))?;
    Ok((code, String::from_utf8_lossy(&out).into_owned()))
}

/// Run git in `dir` with `stdin` and `env`, within [`GIT_DEADLINE`]; its stdout, or why it failed.
pub fn git(dir: &Path, args: &[&str], stdin: Option<&[u8]>, env: &[(&str, &str)]) -> Result<String, String> {
    git_bytes(dir, args, stdin, env).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// [`git`], its stdout as bytes. Its readers start before stdin is written ([`output`]), so a
/// large stdin (a `fast-import` stream) can't deadlock against a full stdout.
pub fn git_bytes(dir: &Path, args: &[&str], stdin: Option<&[u8]>, env: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args).env("GIT_TERMINAL_PROMPT", "0");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let short = format!("git {}", args.first().copied().unwrap_or_default());
    output_bytes(cmd, stdin, GIT_DEADLINE, &short).map_err(|e| match e.strip_prefix(&format!("{short}: ")) {
        // A failure names the whole command line, as git's own messages are matched on.
        Some(rest) => format!("{short} {}: {rest}", args.get(1..).unwrap_or_default().join(" ")),
        None => e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_failures_name_the_command_line() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-process-git");
        let err = git(&dir, &["rev-parse", "--verify", "no-such-ref"], None, &[]).unwrap_err();
        assert!(err.starts_with("git rev-parse --verify no-such-ref: "), "{err}");
    }

    #[test]
    fn a_command_past_its_deadline_is_stopped() {
        let mut cmd = Command::new("git");
        cmd.args(["hash-object", "--stdin"]);
        // Stdin is piped but nothing is written and it isn't closed until the writer drops: the
        // zero deadline stops it first or it finishes; either way it returns, never hangs.
        let r = output(cmd, None, Duration::ZERO, "git hash-object");
        assert!(r.is_ok() || r.as_ref().is_err_and(|e| e.contains("took longer")), "{r:?}");
    }

    #[test]
    fn git_code_is_the_answer_and_a_timeout_is_typed() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-process-git-code");
        assert_eq!(git_code(&dir, &["init", "-q"], GIT_DEADLINE), Ok(0));
        // `config --get` of a key that isn't set exits 1: an answer, not a failure.
        assert_eq!(git_code(&dir, &["config", "--get", "swarm.no-such-key"], GIT_DEADLINE), Ok(1));
        let (code, out) = git_status_output(&dir, &["rev-parse", "--is-inside-work-tree"], GIT_DEADLINE).unwrap();
        assert_eq!((code, out.trim()), (0, "true"));
        // An alias that sleeps past the deadline: stopped, and said so in the error's type.
        let nap = ["-c", "alias.nap=!sleep 5", "nap"];
        let late = git_code(&dir, &nap, Duration::from_millis(200));
        assert_eq!(late, Err(Unanswered::TimedOut { what: "git -c alias.nap=!sleep 5 nap".to_owned(), after: Duration::from_millis(200) }));
    }
}
