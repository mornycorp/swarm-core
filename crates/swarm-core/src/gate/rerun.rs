//! `cargo xtask gate --rerun-failed <sub>`: a gate that failed only
//! because tests failed under load gets a verified pass, without a second full gate.
//!
//! - **The decision is [`plan`]'s**, from the failed gate's `.rc` and log and the HEAD now. It
//!   refuses a gate that passed, a rerun (gate again instead), a record with no kind or HEAD, a
//!   HEAD that moved since the gate ran, a build, link, toolchain, Python or unknown failure, a
//!   failure at a step other than the gate's test step (`test`; the smoke's `golden`, a test step
//!   too), and a test step whose failed tests it can't name exactly (more
//!   than [`MAX_TESTS`] included).
//! - **The rerun** is `ci` with the gate's own scope ([`Kind::ci_args`]) and [`Rerun::ci_args`]:
//!   each failed test alone (`--test-alone`: its own nextest run, one thread), then the doctests
//!   for a full gate (the rest of the test step the failure skipped), then only the steps the
//!   failure skipped (`--only`): the `ci` contract (docs/ci-contract.md).
//! - **Its record** ([`outcome`]) is `rc=… kind=… head=… of=<sub> rerun=<tests>`: a pass only when
//!   `ci` passed and its summary names every test it reran alone, so a test that fails again, or
//!   any other step that fails, leaves no pass. `land` accepts the pass like any other and notes
//!   the evidence on each PR ([`super::evidence`]). The periodic full gate reruns its failed tests
//!   the same way before it files a `main-red` issue ([`crate::fullgate`]), and so does a worker's
//!   `ready` before it gives up (mornycorp/swarm-core#14, [`crate::ready::own_gate`]).

use super::{Cause, Kind, ci_summary, classify, failed_step, field, rc_kind, rc_line, tokens};
use crate::stack::short;

/// The most failed tests a rerun takes: more than that is not a load flake to verify one by one.
pub const MAX_TESTS: usize = 12;

/// A test that failed: its nextest binary id (`swarm-engine::panic_hunt`, `xtask::bin/xtask`) and
/// its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Test {
    pub binary: String,
    pub name: String,
}

impl Test {
    /// From `<binary id> <test name>`, nextest's own form and `ci --test-alone`'s. Refused unless
    /// both are plain (the id letters, digits, `_`, `-`, `:` and `/`, not starting with `-`; the
    /// name letters, digits, `_` and `:`), so neither can become an option or a filterset.
    pub fn parse(spec: &str) -> Result<Test, String> {
        let (binary, name) = spec.trim().split_once(' ').ok_or_else(|| format!("`{spec}` isn't `<binary id> <test name>`"))?;
        let plain = |s: &str, extra: &[char]| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':' || extra.contains(&c));
        if !plain(binary, &['-', '/']) || binary.starts_with('-') || !plain(name, &[]) {
            return Err(format!("`{spec}` isn't a test nextest names plainly: gate again instead"));
        }
        Ok(Test { binary: binary.to_owned(), name: name.to_owned() })
    }

    /// `<binary id> <test name>`, as nextest prints it.
    pub fn spec(&self) -> String {
        format!("{} {}", self.binary, self.name)
    }

    /// `<binary id>::<test name>`: one word, for the record.
    pub fn path(&self) -> String {
        format!("{}::{}", self.binary, self.name)
    }

    /// The package that builds it: the binary id up to its first `::`.
    pub fn package(&self) -> &str {
        self.binary.split("::").next().unwrap_or_default()
    }

    /// `cargo`'s arguments that run this test alone: its own nextest run, one thread, the gate's
    /// profile, and a failure if the filter matches no test (a renamed test never passes
    /// vacuously). What a consumer's `ci --test-alone` runs.
    pub fn nextest_args(&self) -> Vec<String> {
        self.nextest_args_in(&["-p".to_owned(), self.package().to_owned()])
    }

    /// [`Self::nextest_args`] built over `scope` instead of its package: a periodic full
    /// gate's rerun builds its test binaries as its run did (`--workspace` and the corpus
    /// features).
    pub fn nextest_args_in(&self, scope: &[String]) -> Vec<String> {
        let filter = format!("binary_id(={}) & test(={})", self.binary, self.name);
        let mut args: Vec<String> = ["nextest", "run", "--profile", "ci"].into_iter().map(str::to_owned).collect();
        args.extend(scope.iter().cloned());
        args.extend(["--locked", "--test-threads", "1", "--no-tests", "fail", "-E", &filter].into_iter().map(str::to_owned));
        args
    }
}

/// What a rerun of gate `of` runs.
#[derive(Debug, PartialEq, Eq)]
pub struct Rerun {
    /// The failed gate.
    pub of: String,
    /// Its kind: the rerun keeps its scope.
    pub kind: Kind,
    /// The tests that failed, each rerun alone.
    pub tests: Vec<Test>,
    /// The steps the failure skipped, in order.
    pub steps: Vec<String>,
}

impl Rerun {
    /// `ci`'s arguments: the gate's scope, each test alone, the doctests for a full gate (nextest
    /// runs none, and the workspace's test step runs them after it), then only the skipped steps.
    pub fn ci_args(&self) -> Vec<String> {
        let scope: Vec<String> = self.kind.ci_args().iter().map(|s| (*s).to_owned()).collect();
        self.ci_args_in(&scope, self.kind == Kind::Full)
    }

    /// [`Self::ci_args`] over `scope` instead of its kind's, with the doctests when `doctests`:
    /// `ready`'s rerun keeps its `--changed --base <base>`, whose test step runs the affected
    /// packages' doctests after nextest (mornycorp/swarm-core#14).
    pub fn ci_args_in(&self, scope: &[String], doctests: bool) -> Vec<String> {
        let mut a = scope.to_vec();
        for t in &self.tests {
            a.extend(["--test-alone".to_owned(), t.spec()]);
        }
        if doctests {
            a.push("--doctests".to_owned());
        }
        a.extend(["--only".to_owned(), self.steps.join(",")]);
        a
    }

    /// The record's `rerun=` field: the tests, comma-separated.
    pub fn tests_field(&self) -> String {
        self.tests.iter().map(Test::path).collect::<Vec<_>>().join(",")
    }
}

/// Gate `of`'s rerun, from its `.rc` line and its log, with HEAD now at `head`; refused (with
/// why) when it has no rerun path. `previous` is `<of>-rerun`'s record, if any: one rerun a HEAD,
/// so a real failure can't be rerun until it happens to pass. `golden` is the smoke's golden
/// tests' binary ([`crate::Project::golden_binary`]).
pub fn plan(golden: &str, of: &str, rc: &str, log: &str, head: &str, previous: Option<&str>) -> Result<Rerun, String> {
    if let Some(p) = previous.filter(|p| field(p, "head") == Some(head)) {
        return Err(format!("gate {of} was rerun already on this HEAD ({}): gate the stack again", p.trim()));
    }
    let kind = check_record(of, rc, head)?;
    check_cause(of, rc, log)?;
    let step = test_step(kind).ok_or_else(|| format!("gate {of} is a {} gate, which runs no tests: gate again", kind.name()))?;
    let steps = skipped_after(of, log, step)?;
    let tests = if kind == Kind::Smoke { golden_failures(golden, of, log)? } else { nextest_failures(of, log)? };
    Ok(Rerun { of: of.to_owned(), kind, tests, steps })
}

/// The step of a `kind` gate whose failed tests a rerun takes: none for a docs gate.
fn test_step(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Docs => None,
        Kind::Smoke => Some("golden"),
        Kind::Tooling | Kind::Full => Some("test"),
    }
}

/// The record says gate `of` failed, isn't a rerun, tested `head` and has a kind.
fn check_record(of: &str, rc: &str, head: &str) -> Result<Kind, String> {
    let tested = field(rc, "head").filter(|h| !h.is_empty()).ok_or_else(|| format!("gate {of}'s record names no HEAD ({}): gate again", rc.trim()))?;
    if tokens(rc).next() == Some("rc=0") {
        return Err(format!("gate {of} passed: nothing to rerun"));
    }
    if field(rc, "rerun").is_some() {
        return Err(format!("gate {of} is a rerun already: gate the stack again"));
    }
    if tested != head {
        return Err(format!("HEAD moved: gate {of} tested {}, HEAD is {}: gate the stack again", short(tested), short(head)));
    }
    rc_kind(rc).ok_or_else(|| format!("gate {of}'s record has no kind ({}): gate again", rc.trim()))
}

/// Only a failure under load has a rerun path: a test's, a git failure in a test's temporary
/// repository, memory or the GPU. Not a build, link or toolchain failure, a missing Python (it
/// fails again) or an unknown one, nor a compile error anywhere in the log. A record that names no
/// cause is read from its log (without a project's extra causes: the record names those).
fn check_cause(of: &str, rc: &str, log: &str) -> Result<(), String> {
    let cause = tokens(rc).find_map(|f| f.strip_prefix("cause=")).and_then(Cause::parse).unwrap_or_else(|| classify(log, None));
    if !matches!(cause, Cause::Test | Cause::GitTemp | Cause::Memory | Cause::Gpu) {
        return Err(format!("gate {of} failed with cause {}: only failed tests are rerun (a build or clippy failure is no flake): gate again", cause.name()));
    }
    if ["error: could not compile", "error[E", "error: aborting due to"].iter().any(|n| log.contains(n)) {
        return Err(format!("gate {of}'s log has a compile error: only failed tests are rerun: gate again"));
    }
    Ok(())
}

/// The steps the failure skipped, from the log's `CI summary`; refused unless it failed at `step`.
fn skipped_after(of: &str, log: &str, step: &str) -> Result<Vec<String>, String> {
    let summary = ci_summary(log).ok_or_else(|| format!("gate {of}'s log has no `CI summary`: did it finish?"))?;
    let word = |l: &str, w: &str| l.trim_start().strip_prefix(w).map(|r| r.trim().to_owned());
    let at = failed_step(log).ok_or_else(|| format!("gate {of}'s `CI summary` names no failed step"))?;
    if at != step {
        return Err(format!("gate {of} failed at `{at}`, not `{step}`: only failed tests are rerun (a build or clippy failure is no flake): gate again"));
    }
    Ok(summary.lines().filter_map(|l| word(l, "skip ")).collect())
}

/// `specs`, each once, if they are exactly as many as `counted`, at least one and at most
/// [`MAX_TESTS`].
fn exactly(of: &str, specs: Vec<String>, counted: usize) -> Result<Vec<Test>, String> {
    let mut tests: Vec<Test> = Vec::new();
    for spec in specs {
        let t = Test::parse(&spec)?;
        if !tests.contains(&t) {
            tests.push(t);
        }
    }
    if tests.len() != counted || tests.is_empty() || tests.len() > MAX_TESTS {
        return Err(format!("gate {of}: {counted} tests failed and {} were named: a rerun takes 1 to {MAX_TESTS} it can name: gate again", tests.len()));
    }
    Ok(tests)
}

/// The failed tests nextest's summary lists, once each, as many as its count says.
fn nextest_failures(of: &str, log: &str) -> Result<Vec<Test>, String> {
    let lines: Vec<&str> = log.lines().collect();
    let at = lines
        .iter()
        .rposition(|l| l.trim_start().starts_with("Summary ["))
        .ok_or_else(|| format!("gate {of}'s log has no nextest summary: a rerun needs nextest's list of failed tests"))?;
    let counted = summary_failures(lines.get(at).copied().unwrap_or_default()).map_err(|e| format!("gate {of}: {e}"))?;
    let specs = lines.iter().skip(at.saturating_add(1)).filter_map(|l| failed_test(l)).map(str::to_owned).collect();
    exactly(of, specs, counted)
}

/// The failures nextest's `Summary [..] N tests run: a passed (b slow, c leaky), d failed, e timed
/// out, f skipped` line counts; refused for a word it doesn't know (tests not run, say).
fn summary_failures(line: &str) -> Result<usize, String> {
    let counts = line.split_once(':').map(|(_, c)| c).ok_or("nextest's summary has no counts")?;
    // A count's details in brackets (`(1 slow, 29 leaky)`, ns gate 134's) hold commas of their own.
    let mut depth = 0usize;
    let counts: String = counts
        .chars()
        .filter(|&c| {
            depth = match c {
                '(' => depth.saturating_add(1),
                ')' => depth.saturating_sub(1),
                _ => depth,
            };
            depth == 0 && c != ')'
        })
        .collect();
    let mut failed = 0usize;
    for item in counts.split(',') {
        let item = item.trim();
        let (n, what) = item.split_once(' ').ok_or_else(|| format!("nextest's summary: `{item}`?"))?;
        let n: usize = n.parse().map_err(|_| format!("nextest's summary: `{item}`?"))?;
        match what.trim() {
            "passed" | "skipped" => {}
            "failed" | "timed out" => failed = failed.saturating_add(n),
            other => return Err(format!("nextest's summary counts `{other}`, which a rerun can't take")),
        }
    }
    Ok(failed)
}

/// The failed tests a gate's log names, once each, as `<binary id> <test name>`: those nextest's
/// last summary lists (none without one). For a `main-red` issue, not a rerun: it never refuses.
pub fn failed_names(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().collect();
    let Some(at) = lines.iter().rposition(|l| l.trim_start().starts_with("Summary [")) else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    for name in lines.iter().skip(at.saturating_add(1)).filter_map(|l| failed_test(l)) {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// `<binary id> <test name>` from a nextest status line that is a failure: `FAIL [ 1.2s] (9/921)
/// <binary id> <test name>` (`FAIL + LEAK`, `TIMEOUT`, `ABORT`, `SIG…`, `…-FAIL` too).
fn failed_test(line: &str) -> Option<&str> {
    let (status, rest) = line.trim().split_once('[')?;
    let status = status.split_whitespace().next()?;
    let failure = matches!(status, "FAIL" | "TIMEOUT" | "ABORT") || status.starts_with("SIG") || status.ends_with("-FAIL");
    let (_, rest) = rest.split_once(']')?;
    let rest = rest.trim_start();
    let rest = if rest.starts_with('(') { rest.split_once(')')?.1.trim_start() } else { rest };
    failure.then_some(rest.trim_end()).filter(|r| r.contains(' '))
}

/// The smoke's failed golden tests (libtest's output, `cargo test`): the names its last
/// `failures:` list gives, as many as its `test result: FAILED. a passed; b failed` counts, each
/// as a test of the `golden` binary.
fn golden_failures(golden: &str, of: &str, log: &str) -> Result<Vec<Test>, String> {
    let lines: Vec<&str> = log.lines().collect();
    let result = lines
        .iter()
        .rev()
        .find_map(|l| l.trim().strip_prefix("test result: FAILED."))
        .ok_or_else(|| format!("gate {of}'s log has no `test result: FAILED`: a rerun needs the golden tests' list of failures"))?;
    let counted = result
        .split(';')
        .find_map(|c| c.trim().strip_suffix(" failed"))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .ok_or_else(|| format!("gate {of}: the golden tests' result counts no failures"))?;
    let at = lines.iter().rposition(|l| l.trim() == "failures:").ok_or_else(|| format!("gate {of}'s log lists no failed golden test"))?;
    let specs = lines
        .iter()
        .skip(at.saturating_add(1))
        .map(|l| l.trim())
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty() && !l.starts_with("test result"))
        .map(|name| format!("{golden} {name}"))
        .collect();
    exactly(of, specs, counted)
}

/// The rerun's exit code, cause and `.rc` line (its kind's field and the HEAD it tested): a pass only when `ci`
/// passed (`rc`) and its summary names every test it reran alone; otherwise a failure, never a
/// pass. `extra` is the project's extra causes ([`crate::Hooks::extra_causes`]).
pub fn outcome(r: &Rerun, rc: i32, log: &str, secs: u64, (kind, head): (&str, &str), extra: Option<Cause>) -> (i32, Option<Cause>, String) {
    let (rc, cause) = match rc {
        0 if confirmed(r, log) => (0, None),
        0 => (1, Some(Cause::Other)),
        _ => (rc, Some(classify(log, extra))),
    };
    let line = rc_line(rc, secs, cause, kind, head);
    (rc, cause, format!("{} of={} rerun={}\n", line.trim_end(), r.of, r.tests_field()))
}

/// The rerun's summary says every step passed, and names each test it ran alone.
fn confirmed(r: &Rerun, log: &str) -> bool {
    let first = ci_summary(log).and_then(|s| s.lines().next()).unwrap_or_default();
    first.contains("steps passed") && r.tests.iter().all(|t| first.contains(&format!("alone {}", t.spec())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first project's golden tests' binary.
    const GOLDEN: &str = "swarm-engine::golden";

    const HEAD: &str = "c89b857c7c9ce601a87162b88d8732b5ebc02506";

    /// A full gate's log where nextest failed `failed` (a real gate's).
    fn failed_log(failed: &[&str]) -> String {
        let mut log = String::from("=== ci: fmt ===\n=== ci: clippy ===\n=== ci: test ===\n    Starting 921 tests across 140 binaries\n");
        for (i, f) in failed.iter().enumerate() {
            log.push_str(&format!("        FAIL [  49.697s] ({i}/921) {f}\n  stdout ───\n    test result: FAILED. 0 passed; 1 failed\n"));
        }
        log.push_str(&format!(
            "────────────\n     Summary [ 314.665s] 921 tests run: {} passed (21 slow), {} failed, 0 skipped\n",
            921 - failed.len(),
            failed.len()
        ));
        for (i, f) in failed.iter().enumerate() {
            log.push_str(&format!("        FAIL [  49.697s] ({i}/921) {f}\n"));
        }
        log.push_str("error: test run failed\n\nCI summary:\n  ok    fmt\n  ok    clippy\n  FAIL  test: cargo nextest run --profile ci --workspace --locked failed (exit code: 100)\n  skip  layers\n  skip  wasm\n  skip  scorecard\nxtask: ci failed at `test`\n");
        log
    }

    fn failed_rc(cause: &str) -> String {
        format!("rc=1 763s cause={cause} kind=full head={HEAD}\n")
    }

    /// A `main-red` issue lists the failed tests nextest's summary names, once each.
    #[test]
    fn the_failed_tests_are_named_from_the_summary() {
        let log = failed_log(&["swarm-engine::golden commands_replies_match_their_golden_files", "xtask::bin/xtask gate::tests::x"]);
        assert_eq!(failed_names(&log), ["swarm-engine::golden commands_replies_match_their_golden_files", "xtask::bin/xtask gate::tests::x"]);
        assert!(failed_names("error: could not compile `swarm-engine`").is_empty(), "no summary: none");
        assert!(failed_names(&failed_log(&[])).is_empty());
    }

    #[test]
    fn a_test_that_failed_under_load_is_rerun_alone_with_the_steps_it_skipped() {
        let log = failed_log(&["swarm-engine::panic_hunt hostile_code_health_files_never_panic", "xtask::bin/xtask stack::tests::a_stack_rebases"]);
        let r = plan(GOLDEN, "gate-mr211", &failed_rc("test"), &log, HEAD, None).unwrap();
        assert_eq!(r.kind, Kind::Full);
        assert_eq!(r.steps, ["layers", "wasm", "scorecard"]);
        assert_eq!(r.tests_field(), "swarm-engine::panic_hunt::hostile_code_health_files_never_panic,xtask::bin/xtask::stack::tests::a_stack_rebases");
        assert_eq!(
            r.ci_args(),
            [
                "--test-alone",
                "swarm-engine::panic_hunt hostile_code_health_files_never_panic",
                "--test-alone",
                "xtask::bin/xtask stack::tests::a_stack_rebases",
                "--doctests",
                "--only",
                "layers,wasm,scorecard"
            ]
        );
        assert_eq!(r.tests.get(1).unwrap().package(), "xtask");
        let args = r.tests.first().unwrap().nextest_args();
        assert_eq!(
            args.join(" "),
            "nextest run --profile ci -p swarm-engine --locked --test-threads 1 --no-tests fail -E binary_id(=swarm-engine::panic_hunt) & test(=hostile_code_health_files_never_panic)"
        );
        // A load failure in a test's temporary repository is one too; the tooling gate keeps its scope.
        let r = plan(GOLDEN, "g", &failed_rc("git-temp").replace("kind=full", "kind=tooling"), &log, HEAD, None).unwrap();
        assert_eq!(r.ci_args().get(..3).unwrap(), ["--changed", "--base", "origin/main"]);
        assert!(!r.ci_args().contains(&"--doctests".to_owned()), "the tooling gate runs no doctests");
    }

    #[test]
    fn a_build_or_clippy_failure_has_no_rerun_path() {
        let clippy = "=== ci: clippy ===\nerror: this `if` has identical blocks\nerror: could not compile `xtask` due to 1 previous error\n\nCI summary:\n  ok    fmt\n  FAIL  clippy: cargo clippy failed\n  skip  test\n  skip  layers\n";
        for (rc, log, why) in [
            (failed_rc("build"), clippy.to_owned(), "cause build"),
            // Whatever the record says, a failure at clippy isn't a test's.
            (failed_rc("test"), clippy.replace("error: could not compile `xtask` due to 1 previous error\n", ""), "failed at `clippy`"),
            (failed_rc("link"), failed_log(&["a b"]), "cause link"),
            (failed_rc("toolchain"), failed_log(&["a b"]), "cause toolchain"),
            (failed_rc("other"), failed_log(&["a b"]), "cause other"),
            // A test binary that didn't compile: no rerun either.
            (failed_rc("test"), format!("error[E0308]: mismatched types\n{}", failed_log(&["a b"])), "a compile error"),
        ] {
            let e = plan(GOLDEN, "g", &rc, &log, HEAD, None).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
        }
    }

    /// A smoke gate whose `golden` step failed (libtest, not nextest) under a
    /// git failure in a test's temporary repository is rerun like a `test` failure: the golden
    /// test alone, then the steps it skipped.
    #[test]
    fn a_golden_step_failure_is_rerun_too() {
        let log = "=== ci: golden ===\nrunning 2 tests\ntest every_command_has_a_golden_file ... ok\n\
            thread 'commands_replies_match_their_golden_files' panicked at crates/testkit/src/git.rs:318:5:\n\
            git [\"fetch\"] failed: fatal: error reading 'C:/Users/x/AppData/Local/Temp/swarm-test-36668-golden/origin.git/.git'\n\
            test commands_replies_match_their_golden_files ... FAILED\n\nfailures:\n    commands_replies_match_their_golden_files\n\n\
            test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.2s\n\n\
            CI summary:\n  ok    fmt\n  ok    clippy\n  ok    build\n  ok    layers\n  ok    scorecard\n  FAIL  golden: cargo test -p swarm-engine --test golden --locked failed (exit code: 101)\n  skip  cli\n\
            xtask: ci failed at `golden`\n";
        assert_eq!(classify(log, None), Cause::GitTemp, "a git fatal on a testkit repo is git-temp, retried");
        let rc = format!("rc=1 300s cause=git-temp kind=smoke head={HEAD}\n");
        let r = plan(GOLDEN, "gate-mr314", &rc, log, HEAD, None).unwrap();
        assert_eq!(r.tests_field(), "swarm-engine::golden::commands_replies_match_their_golden_files");
        assert_eq!(r.steps, ["cli"]);
        assert!(r.ci_args().ends_with(&[
            "--test-alone".to_owned(),
            "swarm-engine::golden commands_replies_match_their_golden_files".to_owned(),
            "--only".to_owned(),
            "cli".to_owned()
        ]));
        // A count libtest doesn't back up, or none at all: no rerun.
        let miscounted = log.replace("1 passed; 1 failed", "0 passed; 2 failed");
        assert!(plan(GOLDEN, "g", &rc, &miscounted, HEAD, None).unwrap_err().contains("2 tests failed and 1 were named"));
        let unsummarised = log.replace("test result: FAILED.", "test result:");
        assert!(plan(GOLDEN, "g", &rc, &unsummarised, HEAD, None).unwrap_err().contains("has no `test result: FAILED`"));
    }

    /// A `.rc` line whose kind field is a full gate's note tail, `kind=full;
    /// commit=<sha>`, is read and rerun, as any full gate's.
    #[test]
    fn a_full_gate_of_mains_record_is_read_and_rerun() {
        let rc = format!("rc=1 1283s cause=test kind=full; commit={HEAD} head={HEAD}\n");
        assert_eq!(rc_kind(&rc), Some(Kind::Full));
        assert_eq!((field(&rc, "head"), field(&rc, "commit"), field(&rc, "cause")), (Some(HEAD), Some(HEAD), Some("test")));
        let log = failed_log(&["swarm-engine::panic_hunt every_command"]);
        let r = plan(GOLDEN, "gate-full-a78d993", &rc, &log, HEAD, None).unwrap();
        assert_eq!(r.tests.len(), 1, "{r:?}");
        let passed = format!("rc=0 1283s kind=full; commit={HEAD} head={HEAD}\n");
        assert_eq!(super::super::passed_head(&passed), Ok(HEAD));
    }

    #[test]
    fn a_head_that_moved_or_a_record_that_isnt_a_failed_gate_is_refused() {
        let log = failed_log(&["swarm-engine::panic_hunt every_command"]);
        let e = plan(GOLDEN, "gate-7", &failed_rc("test"), &log, &"e".repeat(40), None).unwrap_err();
        assert_eq!(e, "HEAD moved: gate gate-7 tested c89b857, HEAD is eeeeeee: gate the stack again");
        // One rerun a HEAD: a test that fails again alone isn't rerun until it happens to pass.
        let previous = format!("rc=1 12s cause=test kind=full head={HEAD} of=gate-7 rerun=a::b\n");
        assert!(plan(GOLDEN, "gate-7", &failed_rc("test"), &log, HEAD, Some(&previous)).unwrap_err().contains("rerun already on this HEAD"));
        let stale = previous.replace(HEAD, &"f".repeat(40));
        assert!(plan(GOLDEN, "gate-7", &failed_rc("test"), &log, HEAD, Some(&stale)).is_ok(), "an older gate of the same name");
        for (rc, why) in [
            (format!("rc=0 763s kind=full head={HEAD}"), "passed: nothing to rerun"),
            (format!("rc=1 9s cause=test kind=full head={HEAD} of=gate-6 rerun=a::b"), "a rerun already"),
            (format!("rc=1 9s cause=test head={HEAD}"), "no kind"),
            ("rc=1 9s cause=test kind=full".to_owned(), "names no HEAD"),
            (String::new(), "names no HEAD"),
        ] {
            let e = plan(GOLDEN, "gate-7", &rc, &log, HEAD, None).unwrap_err();
            assert!(e.contains(why), "{rc}: {e}");
        }
    }

    #[test]
    fn the_failed_tests_must_be_exactly_the_ones_nextest_counted() {
        let one = failed_log(&["swarm-engine::panic_hunt every_command"]);
        let rc = failed_rc("test");
        let many: Vec<String> = (0..13).map(|i| format!("p t{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        // A failure it can't name, a count that doesn't match, an unknown word, too many, none.
        for (log, why) in [
            (one.replace("1 failed", "2 failed"), "2 tests failed and 1 were named"),
            (one.replace(", 1 failed", ", 1 failed, 1 timed out"), "2 tests failed"),
            (one.replace("0 skipped", "3 not run"), "`not run`"),
            (one.replace("Summary [", "Sum ["), "no nextest summary"),
            (failed_log(&["a 'b'"]), "isn't a test nextest names plainly"),
            (failed_log(&["--config x"]), "isn't a test nextest names plainly"),
            (failed_log(&["a b"]).replace("1 failed", "0 failed").replace("FAIL [", "PASS ["), "and 0 were named"),
            (failed_log(&many), "and 13 were named"),
            (one.replace("\nCI summary:", "\nno summary"), "no `CI summary`"),
        ] {
            let e = plan(GOLDEN, "g", &rc, &log, HEAD, None).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
        }
        // Timeouts and crashes are failures too, each listed once.
        let log = one.replace("1 failed, 0 skipped", "1 failed, 2 timed out, 0 skipped").replace(
            "error: test run failed",
            "        TIMEOUT [ 600.002s] (2/921) swarm-cli::cli slow_one\n        SIGSEGV [   0.1s] swarm-model::event crashes\n        PASS [   0.1s] x y\nerror: test run failed",
        );
        let names: Vec<String> = plan(GOLDEN, "g", &rc, &log, HEAD, None).unwrap().tests.iter().map(Test::spec).collect();
        assert_eq!(names, ["swarm-engine::panic_hunt every_command", "swarm-cli::cli slow_one", "swarm-model::event crashes"]);
    }

    fn rerun() -> Rerun {
        Rerun {
            of: "gate-mr211".into(),
            kind: Kind::Full,
            tests: vec![Test::parse("swarm-engine::panic_hunt every_command").unwrap()],
            steps: vec!["layers".into()],
        }
    }

    #[test]
    fn only_a_rerun_that_passes_every_test_alone_records_a_pass() {
        let r = rerun();
        let passed = "\nCI summary: all 3 steps passed (alone swarm-engine::panic_hunt every_command, doctests, layers) in 0.4 min\n";
        let (rc, cause, line) = outcome(&r, 0, passed, 25, ("kind=full", HEAD), None);
        assert_eq!((rc, cause), (0, None));
        assert_eq!(line, format!("rc=0 25s kind=full head={HEAD} of=gate-mr211 rerun=swarm-engine::panic_hunt::every_command\n"));
        assert_eq!(super::super::passed_head(&line), Ok(HEAD), "land reads it as a pass of the same HEAD");
        assert_eq!(rc_kind(&line), Some(Kind::Full));
        // The test fails again alone: a failure, never a pass.
        let again = "        FAIL [  12.0s] (1/1) swarm-engine::panic_hunt every_command\nerror: test run failed\n\nCI summary:\n  FAIL  alone swarm-engine::panic_hunt every_command: cargo nextest run failed\n  skip  doctests\n  skip  layers\n";
        let (rc, cause, line) = outcome(&r, 1, again, 12, ("kind=full", HEAD), None);
        assert_eq!((rc, cause), (1, Some(Cause::Test)));
        assert!(super::super::passed_head(&line).unwrap_err().contains("did not pass"));
        // A ci that passed without running the test alone proves nothing.
        let (rc, _, line) = outcome(&r, 0, "\nCI summary: all 1 steps passed (layers) in 0.1 min\n", 3, ("kind=full", HEAD), None);
        assert_eq!(rc, 1);
        assert!(line.starts_with("rc=1 3s cause=other kind=full"));
        // A rerun's record can't itself be rerun.
        assert!(plan(GOLDEN, "gate-mr211-rerun", &line, again, HEAD, None).unwrap_err().contains("a rerun already"));
    }

    #[test]
    fn the_evidence_names_the_gate_its_tests_and_its_summary() {
        let rc = format!("rc=0 25s kind=full head={HEAD} of=gate-mr211 rerun=a::b::t1,c::t2\n");
        let e = super::super::evidence(&rc, Some("x\nCI summary:\n  ok    fmt\n  FAIL  test: nextest failed\n")).unwrap();
        assert_eq!(
            e,
            "Verified by a rerun of gate `gate-mr211`: only these tests failed there, each passed alone, and so did the steps the failure skipped:\n  a::b::t1\n  c::t2\n\ngate-mr211's CI summary:\n  ok    fmt\n  FAIL  test: nextest failed"
        );
        assert!(super::super::evidence(&rc, None).unwrap().ends_with("  c::t2\n"));
        assert_eq!(super::super::evidence(&format!("rc=0 25s kind=full head={HEAD}"), None), None, "a gate that isn't a rerun");
    }

    #[test]
    fn a_test_spec_is_plain_or_refused() {
        assert_eq!(Test::parse("xtask::bin/xtask gate::tests::x").unwrap().path(), "xtask::bin/xtask::gate::tests::x");
        for bad in ["", "one", "a b c", "-p x", "a b)", "a b|c", "a\tb", "a b-c", "a/b c/d"] {
            assert!(Test::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(field("rc=0 1s head=x", "rc"), Some("0"));
        assert_eq!(field("rc=0 1s rcx=2", "rc"), Some("0"));
        assert_eq!(field("rcx=2", "rc"), None);
    }

    /// nextest's other failures: a leak beside it, a timeout, a crash (ns).
    #[test]
    fn every_kind_of_failure_nextest_lists_is_rerun() {
        let one = failed_log(&["swarm-engine::panic_hunt every_command"]);
        let log = one.replace("1 failed, 0 skipped", "1 failed, 3 timed out, 0 skipped").replace(
            "error: test run failed",
            " FAIL + LEAK [  30.461s] (1162/1742) swarm-source::git losing_the_host_ends_its_target\n        TIMEOUT [ 600.002s] (2/1772) swarm-cli::cli slow_one\n        SIGSEGV [   0.1s] swarm-model::event crashes\n        PASS [   0.1s] x y\nerror: test run failed",
        );
        let names: Vec<String> = plan(GOLDEN, "g", &failed_rc("test"), &log, HEAD, None).unwrap().tests.iter().map(Test::spec).collect();
        assert_eq!(
            names,
            [
                "swarm-engine::panic_hunt every_command",
                "swarm-source::git losing_the_host_ends_its_target",
                "swarm-cli::cli slow_one",
                "swarm-model::event crashes"
            ]
        );
        // A count with its details in brackets (`(1 slow, 29 leaky)`).
        let leaky = one.replace("passed (21 slow)", "passed (1 slow, 29 leaky)");
        assert_eq!(plan(GOLDEN, "g", &failed_rc("test"), &leaky, HEAD, None).unwrap().tests.len(), 1);
    }

    /// The smoke's golden tests run through `cargo test`: libtest's list of failures, each named as
    /// a test of the project's golden binary (ns).
    #[test]
    fn a_smokes_failed_golden_test_is_rerun_alone_then_the_cli_run() {
        let log = "\n=== ci: golden ===\nrunning 3 tests\ntest vulkan_overview ... ok\ntest d3d12_overview ... FAILED\n\nfailures:\n\n---- d3d12_overview stdout ----\nthread panicked\n\nfailures:\n    d3d12_overview\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s\n\nerror: test failed, to rerun pass `-p gpu-engine --test golden`\n\nCI summary:\n  ok    fmt\n  ok    clippy\n  ok    build\n  ok    layers\n  FAIL  golden: cargo test -p gpu-engine --test golden: exited with exit code: 101\n  skip  cli\n";
        let rc = format!("rc=1 300s cause=test kind=smoke (reaches gpu-app) head={HEAD}\n");
        let ns = crate::project::fixtures::GPU_SWARM.golden_binary;
        let r = plan(ns, "gate-140", &rc, log, HEAD, None).unwrap();
        assert_eq!((r.kind, r.steps.clone()), (Kind::Smoke, vec!["cli".to_owned()]));
        assert_eq!(r.ci_args(), ["--smoke", "--test-alone", "gpu-engine::golden d3d12_overview", "--only", "cli"]);
        assert!(plan(ns, "g", &rc, &log.replace("1 failed;", "2 failed;"), HEAD, None).unwrap_err().contains("2 tests failed and 1 were named"));
        assert!(plan(ns, "g", &rc, &failed_log(&["a b"]), HEAD, None).unwrap_err().contains("failed at `test`, not `golden`"));
        let docs = format!("rc=1 3s cause=test kind=docs head={HEAD}\n");
        assert!(plan(ns, "g", &docs, log, HEAD, None).unwrap_err().contains("runs no tests"));
    }
}
