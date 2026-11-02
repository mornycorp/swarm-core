//! `ready`'s own gate: the consumer's `ci --changed --base <base>` on the
//! rebased tree, logged as the worker's sub-agent `ready-<head>` (`sub.start --name ready`, then
//! `sub.stop`: `rc=0`, or a failure's `rc=<n>; cause=<cause>; step=<step>`, `stop_note`), apart
//! from the coordinator's `gate-*` gates. Its output goes
//! to the terminal as it prints and to `<target>/gate-logs/ready-<head>.log`, with its `.rc` record
//! and nextest's junit file (`.junit.xml`) beside it, as `gate` keeps them
//! (mornycorp/swarm-core#14).
//!
//! **A test that failed under load** (#14): `ready` reruns it once, as `gate --rerun-failed` does,
//! rather than leaving the worker to run all of `ready` again (on 2026-10-29 one PR ran it five
//! times, another six, each failure a GPU test that passes alone in seconds). When the only failure
//! is the test step's and [`rerun::plan`] can name its failed tests, it reruns each alone, then the
//! doctests and the steps the failure skipped, on the same HEAD, as `ready-<head>-rerun`
//! ([`Rerun::ci_args_in`], `ready`'s own scope). Its record is a pass only when every test passed
//! alone and every step passed ([`gate::run_outcome`]); the tests it reran go in `submit`'s note.
//! Anything else stops `ready` as before: a build, clippy or fmt failure, a failure at another
//! step, failed tests it can't name (more than [`rerun::MAX_TESTS`]), a test that fails again
//! alone, and a HEAD whose rerun already ran (one a HEAD, [`rerun::plan`]'s rule). `--no-rerun`
//! turns it off.

use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::gate::rerun::{self, Rerun};
use crate::gate::{self, Cause, Kind};
use crate::{Hooks, Project};

/// The sub `ready`'s gate is logged as: `ready-<the gated commit, short>`.
pub fn sub_of(head: &str) -> String {
    format!("ready-{}", head.get(..7).unwrap_or(head))
}

/// `ci --changed --base <base>`: what `ready` gates with (the `ci` contract).
pub fn scope(base: &str) -> Vec<String> {
    ["--changed", "--base", base].map(str::to_owned).to_vec()
}

/// The kind its `.rc` records: the tooling gate's, which runs the same `ci --changed`, so
/// [`rerun::plan`] reads its test step as `test`.
fn kind_field() -> String {
    gate::kind_field(Kind::Tooling, "ready")
}

/// One gate of the tree checked out at `root`, at its HEAD, against `base`.
pub struct OwnGate<'a> {
    pub project: &'a Project,
    pub hooks: &'a dyn Hooks,
    pub agent: &'a str,
    pub root: &'a Path,
    pub base: &'a str,
    /// Where its logs and records go: `<target>/gate-logs`.
    pub logs: &'a Path,
    /// Whether a test-only failure is rerun (`--no-rerun` turns it off).
    pub rerun: bool,
    /// The command that runs the consumer's `ci <args>` here (this xtask's own exe, or the
    /// rebased tree's).
    pub ci: &'a dyn Fn(&[String]) -> Result<Command, String>,
}

/// What a gate that passed only by its rerun verified: the failed gate and the tests that passed
/// alone (`<binary id>::<test>`).
#[derive(Debug, PartialEq, Eq)]
pub struct Reran {
    pub of: String,
    pub tests: Vec<String>,
}

impl Reran {
    /// `submit`'s note: which tests passed only when rerun alone.
    pub fn note(&self) -> String {
        format!("{} failed only at its test step; rerun alone and passed: {}", self.of, self.tests.join(", "))
    }
}

/// One run of `ci`: its exit code (the rerun's, from its record), its `.rc` line and its log, and
/// a failure's cause ([`gate::classify`]) and the `ci` step it failed at ([`gate::failed_step`]).
struct Ran {
    rc: i32,
    line: String,
    log: String,
    cause: Option<Cause>,
    step: Option<String>,
}

impl OwnGate<'_> {
    /// Gate the tree's HEAD, rerunning a test-only failure's tests alone once: `None` when it
    /// passed, what the rerun verified when it passed by that, or why it failed.
    pub fn run(&self, log: &mut dyn FnMut(&[&str])) -> Result<Option<Reran>, String> {
        let head = crate::stack::rev(self.root, "HEAD")?;
        let sub = sub_of(&head);
        let first = as_sub(self.agent, &sub, "ci --changed before submitting", log, &mut || self.attempt(&sub, &head, &scope(self.base), None))?;
        if first.rc == 0 {
            return Ok(None);
        }
        let failed = format!("cargo xtask ci --changed failed (rc={}; log {})", first.rc, self.log_of(&sub));
        let previous = std::fs::read_to_string(self.logs.join(format!("{sub}-rerun.rc"))).ok();
        let plan = rerun_plan(self.rerun, self.project.golden_binary, &sub, &first, &head, previous.as_deref())
            .map_err(|why| format!("{failed}\nready: no rerun of its tests alone: {why}"))?;
        let again = format!("{sub}-rerun");
        let tests: Vec<String> = plan.tests.iter().map(rerun::Test::spec).collect();
        println!(
            "ready: {sub} failed only at its test step: rerunning its failed tests alone ({}), then the doctests and the steps it skipped ({}), as {again}",
            tests.join(", "),
            plan.steps.join(", ")
        );
        let note = format!("rerun of {sub}: its failed tests alone ({}), then the steps it skipped", plan.tests.len());
        let args = plan.ci_args_in(&scope(self.base), true);
        let second = as_sub(self.agent, &again, &note, log, &mut || self.attempt(&again, &head, &args, Some(&plan)))?;
        if second.rc != 0 {
            return Err(format!("{failed}\nready: its rerun {again} failed too (rc={}; log {}): not a load flake", second.rc, self.log_of(&again)));
        }
        let reran = Reran { of: sub, tests: plan.tests.iter().map(rerun::Test::path).collect() };
        println!("ready: {again} passed: {}", reran.note());
        Ok(Some(reran))
    }

    fn log_of(&self, sub: &str) -> String {
        self.logs.join(format!("{sub}.log")).display().to_string()
    }

    /// Run `ci <args>` as `sub`: its log, its junit file kept, its `.rc` written, as `gate` writes
    /// them ([`gate::attempt`]). An error only when it couldn't run.
    fn attempt(&self, sub: &str, head: &str, args: &[String], rerun: Option<&Rerun>) -> Result<Ran, String> {
        std::fs::create_dir_all(self.logs).map_err(|e| format!("ready: {}: {e}", self.logs.display()))?;
        let (log, record) = (self.logs.join(format!("{sub}.log")), self.logs.join(format!("{sub}.rc")));
        let _ = std::fs::remove_file(&record);
        let cmd = (self.ci)(args)?;
        println!("ready {sub}: ci {}; log {}", args.join(" "), log.display());
        let (started, since) = (Instant::now(), SystemTime::now());
        let rc = run_logged(cmd, &log)?;
        if let Err(e) = gate::keep_junit(&self.root.join(gate::JUNIT), &self.logs.join(format!("{sub}.junit.xml")), since) {
            eprintln!("warning: ready: the test times: {e}");
        }
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let (rc, cause, line) = gate::run_outcome(self.hooks, rerun, rc, &text, started.elapsed().as_secs(), (&kind_field(), head));
        if let Err(e) = std::fs::write(&record, &line) {
            eprintln!("warning: ready: {}: {e}", record.display());
        }
        let step = if rc == 0 { None } else { gate::failed_step(&text).map(str::to_owned) };
        Ok(Ran { rc, line, log: text, cause, step })
    }
}

/// The rerun of `sub`, which failed as `first` records, on `head`; or why there is none: turned
/// off, or no rerun path ([`rerun::plan`]: a failure that isn't only failed tests, tests it can't
/// name, a rerun of this HEAD already, `previous` being that rerun's record).
fn rerun_plan(enabled: bool, golden: &str, sub: &str, first: &Ran, head: &str, previous: Option<&str>) -> Result<Rerun, String> {
    if !enabled {
        return Err("--no-rerun".to_owned());
    }
    rerun::plan(golden, sub, &first.line, &first.log, head, previous)
}

/// Run `gate` as the worker's own sub-agent `sub`: `sub.start --name ready`
/// with `note` before it and `sub.stop` with its exit after, in `agent`'s file, so the Merge Train
/// sees the ticket's own gate (stage 4). Named apart from `gate`, which the Gates tab counts as the
/// coordinator's and whose failures naming a PR are send-backs. Its stop says why it failed
/// ([`stop_note`]). Its result is the gate's: logging never fails it.
fn as_sub(agent: &str, sub: &str, note: &str, log: &mut dyn FnMut(&[&str]), gate: &mut dyn FnMut() -> Result<Ran, String>) -> Result<Ran, String> {
    log(&["sub.start", "--agent", agent, "--sub", sub, "--name", "ready", "--note", note]);
    let ran = gate();
    log(&["sub.stop", "--agent", agent, "--sub", sub, "--note", &stop_note(&ran)]);
    ran
}

/// `ready`'s `sub.stop` note, in the gate's note grammar (`key=value` fields split at `;`): `rc=0`
/// for a pass; a failure's exit code, then its cause and the `ci` step it failed at, each when
/// known: `rc=<n>; cause=<cause>; step=<step>` (`rc=100; cause=test; step=test`,
/// `rc=101; cause=build; step=clippy`). One whose `ci` couldn't run at all is `rc=1`, as before.
fn stop_note(ran: &Result<Ran, String>) -> String {
    let Ok(ran) = ran else { return "rc=1".to_owned() };
    let mut note = format!("rc={}", ran.rc);
    if ran.rc != 0 {
        if let Some(cause) = ran.cause {
            note.push_str(&format!("; cause={}", cause.name()));
        }
        if let Some(step) = &ran.step {
            note.push_str(&format!("; step={step}"));
        }
    }
    note
}

/// How often [`run_logged`] copies the log to the terminal.
const TAIL_EVERY: Duration = Duration::from_millis(250);

/// Run `cmd` with its whole output in `log` and on ours as it is written: its exit code (-1:
/// killed). The child writes the file itself, as the gate's `ci` does, and this copies what it
/// wrote to stdout until it exits: no pipe that a process it left running could hold open.
fn run_logged(mut cmd: Command, log: &Path) -> Result<i32, String> {
    let file = File::create(log).map_err(|e| format!("ready: {}: {e}", log.display()))?;
    let err = file.try_clone().map_err(|e| format!("ready: {}: {e}", log.display()))?;
    let mut child = cmd.stdin(Stdio::null()).stdout(file).stderr(err).spawn().map_err(|e| format!("ready: ci: {e}"))?;
    let mut reader = File::open(log).map_err(|e| format!("ready: {}: {e}", log.display()))?;
    let mut out = std::io::stdout();
    loop {
        let exited = child.try_wait().map_err(|e| format!("ready: ci: {e}"))?;
        // What it wrote since the last copy: to its end, which the next copy reads on from.
        let _ = std::io::copy(&mut reader, &mut out);
        let _ = std::io::Write::flush(&mut out);
        if let Some(status) = exited {
            return Ok(status.code().unwrap_or(-1));
        }
        std::thread::sleep(TAIL_EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "c89b857c7c9ce601a87162b88d8732b5ebc02506";
    const GOLDEN: &str = "swarm-engine::golden";

    /// `ci --changed`'s log where nextest failed `failed`, and the steps after `test` skipped.
    fn failed_log(failed: &[&str]) -> String {
        let mut log = String::from("=== ci: fmt ===\n=== ci: clippy ===\n=== ci: test ===\n    Starting 120 tests across 30 binaries\n");
        for (i, f) in failed.iter().enumerate() {
            log.push_str(&format!("        FAIL [  49.697s] ({i}/120) {f}\n"));
        }
        log.push_str(&format!(
            "────────────\n     Summary [ 314.665s] 120 tests run: {} passed (2 slow), {} failed, 0 skipped\n",
            120 - failed.len(),
            failed.len()
        ));
        for (i, f) in failed.iter().enumerate() {
            log.push_str(&format!("        FAIL [  49.697s] ({i}/120) {f}\n"));
        }
        log.push_str("error: test run failed\n\nCI summary:\n  ok    fmt\n  ok    clippy\n  FAIL  test: cargo nextest run failed (exit code: 100)\n  skip  layers\n  skip  commands\n  skip  scorecard\nxtask: ci failed at `test`\n");
        log
    }

    /// The first gate's run as [`OwnGate::attempt`] records it.
    fn ran(rc: i32, log: &str) -> Ran {
        let cause = (rc != 0).then(|| gate::classify(log, None));
        let step = gate::failed_step(log).map(str::to_owned);
        Ran { rc, line: gate::rc_line(rc, 900, cause, &kind_field(), HEAD), log: log.to_owned(), cause, step }
    }

    /// #14: a test-only failure is rerun with `ready`'s own scope: each failed test alone, the
    /// doctests (`ci --changed`'s test step runs them after nextest), then the steps it skipped.
    #[test]
    fn a_test_only_failure_is_rerun_alone_with_readys_scope() {
        let first = ran(100, &failed_log(&["gpu-engine::capture_flow vkcube_captures", "xtask::bin/xtask ready::tests::x"]));
        assert!(first.line.contains("cause=test kind=tooling (ready)"), "{}", first.line);
        let plan = rerun_plan(true, GOLDEN, "ready-c89b857", &first, HEAD, None).unwrap();
        assert_eq!(plan.steps, ["layers", "commands", "scorecard"]);
        assert_eq!(
            plan.ci_args_in(&scope("41-base"), true),
            [
                "--changed",
                "--base",
                "41-base",
                "--test-alone",
                "gpu-engine::capture_flow vkcube_captures",
                "--test-alone",
                "xtask::bin/xtask ready::tests::x",
                "--doctests",
                "--only",
                "layers,commands,scorecard"
            ]
        );
        let reran = Reran { of: "ready-c89b857".into(), tests: plan.tests.iter().map(rerun::Test::path).collect() };
        assert_eq!(
            reran.note(),
            "ready-c89b857 failed only at its test step; rerun alone and passed: gpu-engine::capture_flow::vkcube_captures, xtask::bin/xtask::ready::tests::x"
        );
    }

    /// #14: anything but failed tests it can name stops `ready` as before, and so does
    /// `--no-rerun`, and a HEAD whose rerun already ran.
    #[test]
    fn anything_else_has_no_rerun() {
        let flaky = failed_log(&["gpu-engine::capture_flow vkcube_captures"]);
        let clippy = "=== ci: clippy ===\nerror: this `if` has identical blocks\nerror: could not compile `xtask` due to 1 previous error\n\nCI summary:\n  ok    fmt\n  FAIL  clippy: cargo clippy failed\n  skip  test\n  skip  layers\n";
        let fmt = "=== ci: fmt ===\nDiff in xtask/src/main.rs at line 3:\n\nCI summary:\n  FAIL  fmt: cargo fmt --check failed\n  skip  clippy\n  skip  test\n";
        let many: Vec<String> = (0..13).map(|i| format!("p t{i}")).collect();
        let many = failed_log(&many.iter().map(String::as_str).collect::<Vec<_>>());
        let doctest = flaky.replace("119 passed (2 slow), 1 failed", "120 passed (2 slow), 0 failed").replace("FAIL [", "PASS [");
        let earlier = format!("rc=1 30s cause=test kind=tooling (ready) head={HEAD} of=ready-c89b857 rerun=a::b\n");
        for (enabled, first, previous, why) in [
            (false, ran(100, &flaky), None, "--no-rerun"),
            (true, ran(101, clippy), None, "cause build"),
            (true, ran(1, fmt), None, "cause other"),
            (true, ran(100, &many), None, "13 tests failed and 13 were named"),
            (true, ran(100, &doctest), None, "0 tests failed and 0 were named"),
            (true, ran(100, &flaky), Some(earlier.as_str()), "rerun already on this HEAD"),
        ] {
            let e = rerun_plan(enabled, GOLDEN, "ready-c89b857", &first, HEAD, previous).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
        }
        // An earlier HEAD's rerun doesn't count.
        let stale = earlier.replace(HEAD, &"f".repeat(40));
        assert!(rerun_plan(true, GOLDEN, "ready-c89b857", &ran(100, &flaky), HEAD, Some(&stale)).is_ok());
    }

    /// Each gate runs inside `sub.start --name ready` and its `sub.stop`, which says how it
    /// ended; the result is the gate's.
    #[test]
    fn the_gate_is_logged_as_the_workers_own() {
        assert_eq!(sub_of("0c4dba23e5b8b70b"), "ready-0c4dba2");
        assert_eq!(sub_of("abc"), "ready-abc", "a short or odd head is kept whole");
        for (rc, stop) in [(Ok(0), "rc=0"), (Ok(100), "rc=100; cause=test; step=test"), (Err("ci: not found".to_owned()), "rc=1")] {
            let mut logged: Vec<Vec<String>> = Vec::new();
            let got = as_sub("a-1", "ready-0c4dba2", "why", &mut |args| logged.push(args.iter().map(|a| (*a).to_owned()).collect()), &mut || {
                rc.clone().map(|rc| if rc == 0 { ran(0, "CI summary: all 1 steps passed (test) in 1 min\n") } else { ran(rc, &failed_log(&["p t"])) })
            });
            assert_eq!(got.map(|r| r.rc), rc);
            let start = ["sub.start", "--agent", "a-1", "--sub", "ready-0c4dba2", "--name", "ready", "--note", "why"];
            let stop = ["sub.stop", "--agent", "a-1", "--sub", "ready-0c4dba2", "--note", stop];
            assert_eq!(logged, [start.map(str::to_owned).to_vec(), stop.map(str::to_owned).to_vec()]);
        }
    }

    /// #77 (swarmception): a failed `ready` says why, as a failed gate does: its exit code, the
    /// cause read from its log and the `ci` step it failed at; what it can't tell is left out.
    #[test]
    fn a_failure_s_stop_names_its_cause_and_step() {
        let clippy = "=== ci: clippy ===\nerror: could not compile `xtask` due to 1 previous error\n\nCI summary:\n  ok    fmt\n  FAIL  clippy: cargo clippy failed\n  skip  test\n";
        let fmt = "=== ci: fmt ===\nDiff in xtask/src/main.rs at line 3:\n\nCI summary:\n  FAIL  fmt: cargo fmt --check failed\n  skip  clippy\n";
        for (ran, note) in [
            (ran(100, &failed_log(&["p t"])), "rc=100; cause=test; step=test"),
            (ran(101, clippy), "rc=101; cause=build; step=clippy"),
            (ran(1, fmt), "rc=1; cause=other; step=fmt"),
            (ran(-1, "killed before its summary"), "rc=-1; cause=other"),
            (ran(0, "CI summary: all 3 steps passed (fmt, clippy, test) in 2 min\n"), "rc=0"),
        ] {
            assert_eq!(stop_note(&Ok(ran)), note);
        }
        assert_eq!(stop_note(&Err("ready: ci: not found".to_owned())), "rc=1", "ci never ran: no cause to name");
        let unknown = Ran { rc: 3, line: String::new(), log: String::new(), cause: None, step: None };
        assert_eq!(stop_note(&Ok(unknown)), "rc=3");
    }

    /// The child's whole output is in the log, stdout and stderr, and its exit code is returned.
    #[test]
    fn the_output_is_logged_whole() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-ready-logged");
        let log = dir.join("ready-x.log");
        let mut cmd = Command::new("git");
        cmd.current_dir(&dir).args(["--version"]);
        assert_eq!(run_logged(cmd, &log), Ok(0));
        assert!(std::fs::read_to_string(&log).unwrap().starts_with("git version"));
        let mut cmd = Command::new("git");
        cmd.current_dir(&dir).args(["no-such-command"]);
        assert_ne!(run_logged(cmd, &log), Ok(0));
        assert!(std::fs::read_to_string(&log).unwrap().contains("no-such-command"), "stderr too");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
