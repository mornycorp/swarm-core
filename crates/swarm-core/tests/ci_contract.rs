//! The `ci` contract (docs/ci-contract.md), Swarm Core's side: the gate runs a fake consumer's
//! `ci` (`fake/xtask.rs`), which accepts exactly the contract's command lines, and reads back what
//! it leaves: the exit code, the `CI summary`, nextest's lines and the junit file.

use std::path::{Path, PathBuf};
use std::process::Command;

use swarm_core::Project;
use swarm_core::gate::{self, CiRun, Gate, Kind};
use swarm_core::project::{Branches, GitHub, Labels, Limits, ReadyWatch};
use swarm_core::ready::own_gate::{self, OwnGate};

/// A consumer that is neither swarm: its own names, so nothing here touches theirs.
const FAKE: Project = Project {
    name: "fake-consumer",
    env_prefix: "FAKE_CONSUMER",
    github: GitHub { host: "github.invalid", repo: "fake/consumer" },
    branches: Branches { main: "main", tracking: "tracking", remote: "origin" },
    labels: Labels { in_progress: "status::in-progress", blocked: "status::blocked", main_red: "main-red", bug: "type::bug" },
    limits: Limits { gate_lock_max_age_secs: 3600 },
    checkout_marker: "fake-checkout.txt",
    gate_lock_file: "fake-gate.lock",
    tracking_queue: "fake-tracking-queue.jsonl",
    pr_template: ".github/pull_request_template.md",
    generated: &[],
    stack_state: "fake-stack.json",
    full_gate_worktree: ".claude/worktrees/fullgate",
    xtask_rules: &["xtask"],
    ready_watch: ReadyWatch::Stream,
    gate_tag: "",
    golden_binary: "fake::golden",
    proof: swarm_core::Proof::Ready,
};

const FAKE_XTASK: &str = env!("CARGO_BIN_EXE_fake-xtask");

/// The fake's knobs ([`FakeCi`]'s `around_ci` sets them on the command, as a consumer's sets its own
/// variables), and what it saw of each run.
struct FakeCi {
    record: PathBuf,
    fail: Option<&'static str>,
    seen: std::cell::RefCell<Vec<String>>,
}

impl swarm_core::Hooks for FakeCi {
    fn around_ci(&self, run: &CiRun<'_>, mut cmd: Command) -> Result<i32, String> {
        self.seen.borrow_mut().push(format!("{} {}", run.sub, run.kind.name()));
        cmd.env("FAKE_CI_RECORD", &self.record).env_remove("FAKE_CI_FAIL_ALONE");
        match self.fail {
            Some(f) => cmd.env("FAKE_CI_FAIL", f),
            None => cmd.env_remove("FAKE_CI_FAIL"),
        };
        gate::run_status(cmd)
    }
}

/// A clone with an origin (the gate's events go to its tracking branch), its target dir, and the
/// fake's record file.
struct World {
    root: PathBuf,
    target: PathBuf,
    record: PathBuf,
    head: String,
}

fn world(name: &str) -> World {
    let dir = swarm_core_testkit::temp_dir(name);
    let remote = swarm_core_testkit::git::remote(&dir);
    let head = swarm_core_testkit::git::git(&remote.work, &["rev-parse", "HEAD"]).trim().to_owned();
    World { target: remote.work.join("target"), record: dir.join("record.txt"), root: remote.work, head }
}

fn recorded(w: &World) -> Vec<String> {
    std::fs::read_to_string(&w.record).unwrap_or_default().lines().map(str::to_owned).collect()
}

fn gate_of<'a>(w: &'a World, hooks: &'a FakeCi, kind: Kind, field: &'a str, rerun: Option<&'a gate::rerun::Rerun>) -> Gate<'a> {
    Gate {
        project: &FAKE,
        hooks,
        root: &w.root,
        agent: "fake-1",
        kind,
        kind_field: field,
        env: &[],
        target: &w.target,
        head: &w.head,
        stop_tail: "",
        rerun,
        program: Path::new(FAKE_XTASK),
    }
}

/// Each kind runs `ci` with the contract's arguments and passes; its record names the HEAD it
/// tested, and a run with a test step keeps nextest's junit file.
#[test]
fn each_kind_runs_the_contracts_command_line() {
    let w = world("swarm-core-ci-contract-kinds");
    let hooks = FakeCi { record: w.record.clone(), fail: None, seen: Default::default() };
    for (kind, sub) in [(Kind::Docs, "g-docs"), (Kind::Tooling, "g-tooling"), (Kind::Smoke, "g-smoke"), (Kind::Full, "g-full")] {
        let field = gate::kind_field(kind, "the contract");
        let g = gate_of(&w, &hooks, kind, &field, None);
        assert_eq!(gate::attempt(&g, sub, Some("contract"), (1, None)), Ok((0, None)), "{sub}");
        let rc = std::fs::read_to_string(w.target.join(gate::LOG_DIR).join(format!("{sub}.rc"))).unwrap();
        assert_eq!(gate::passed_head(&rc), Ok(w.head.as_str()), "{rc}");
        assert_eq!(gate::rc_kind(&rc), Some(kind));
        let junit = w.target.join(gate::LOG_DIR).join(format!("{sub}.junit.xml"));
        assert_eq!(junit.exists(), kind != Kind::Smoke, "{sub}: only a run with a test step leaves the junit file");
        let _ = std::fs::remove_file(w.root.join(gate::JUNIT));
    }
    assert_eq!(recorded(&w), ["ci|--changed|--base|origin/main", "ci|--changed|--base|origin/main", "ci|--smoke", "ci"]);
    assert_eq!(hooks.seen.borrow().as_slice(), ["g-docs docs", "g-tooling tooling", "g-smoke smoke", "g-full full"], "every run goes through around_ci");
}

/// A test that fails under load: the gate classifies it, plans its rerun from the records, and the
/// rerun (`--test-alone`, `--doctests`, `--only` the skipped steps) passes, recording what it
/// verified for `land`.
#[test]
fn a_failed_test_is_rerun_alone_through_the_contract() {
    let w = world("swarm-core-ci-contract-rerun");
    let failing = "fake::flaky under_load";
    let hooks = FakeCi { record: w.record.clone(), fail: Some(failing), seen: Default::default() };
    let field = gate::kind_field(Kind::Full, "--full");
    let g = gate_of(&w, &hooks, Kind::Full, &field, None);
    assert_eq!(gate::attempt(&g, "g-9", Some("contract"), (1, None)), Ok((100, Some(gate::Cause::Test))));
    let logs = w.target.join(gate::LOG_DIR);
    let log = std::fs::read_to_string(logs.join("g-9.log")).unwrap();
    assert_eq!(gate::failed_tests(&log), [failing]);
    assert_eq!(gate::rerun::failed_names(&log), [failing]);
    let plan = gate::plan_from_records(&FAKE, &logs, "g-9", "g-9-rerun", &w.head).unwrap();
    assert_eq!((plan.tests.len(), plan.steps.clone()), (1, vec!["layers".to_owned(), "wasm".to_owned(), "conformance".to_owned()]));
    let rerun = gate_of(&w, &hooks, Kind::Full, &field, Some(&plan));
    assert_eq!(gate::attempt(&rerun, "g-9-rerun", Some("rerun of g-9"), (1, None)), Ok((0, None)));
    assert_eq!(recorded(&w).last().map(String::as_str), Some(format!("ci|--test-alone|{failing}|--doctests|--only|layers,wasm,conformance").as_str()));
    let rc = std::fs::read_to_string(logs.join("g-9-rerun.rc")).unwrap();
    assert_eq!(gate::passed_head(&rc), Ok(w.head.as_str()));
    assert!(gate::rerun_evidence(&logs, &rc).unwrap().contains("fake::flaky::under_load"), "{rc}");
}

/// mornycorp/swarm-core#20: a gate of the same tree straight after one failed is its rerun, read
/// from the failed gate's own `.rc`, so its start note says so; after a pass, the next isn't.
#[test]
fn a_gate_of_the_same_tree_after_a_failure_is_its_rerun() {
    let w = world("swarm-core-ci-contract-silent");
    let logs = w.target.join(gate::LOG_DIR);
    let field = gate::kind_field(Kind::Full, "--full");
    let gated = gate::silent::gated(&FAKE, &w.root, &[], &w.head);
    assert_eq!(gated, gate::silent::of_head(&w.head), "no stack record: the HEAD");
    let failing = FakeCi { record: w.record.clone(), fail: Some("fake::flaky under_load"), seen: Default::default() };
    gate::silent::record(&logs, "g-20", &gated);
    assert_eq!(gate::attempt(&gate_of(&w, &failing, Kind::Full, &field, None), "g-20", Some("local gate: #20"), (1, None)), Ok((100, Some(gate::Cause::Test))));
    let of = gate::silent::rerun_in(&logs, &gated, "g-20-2");
    assert_eq!(of.as_deref(), Some("g-20"));
    assert_eq!(gate::silent::note("local gate: #20", of.as_deref()), "rerun of g-20: local gate: #20");

    let passing = FakeCi { record: w.record.clone(), fail: None, seen: Default::default() };
    gate::silent::record(&logs, "g-20-2", &gated);
    assert_eq!(gate::attempt(&gate_of(&w, &passing, Kind::Full, &field, None), "g-20-2", Some("rerun of g-20: local gate: #20"), (1, None)), Ok((0, None)));
    assert_eq!(gate::silent::rerun_in(&logs, &gated, "g-20-3"), None, "the last gate passed");
}

/// mornycorp/swarm-core#43: `gate --test-alone` runs only the named tests, each alone, with the
/// tooling scope and no step after them, and its record is a pass only when every one ran: a
/// docs gate of an empty diff would pass vacuously.
#[test]
fn tests_alone_run_only_those_tests_through_the_contract() {
    let w = world("core-ci-contract-alone");
    let hooks = FakeCi { record: w.record.clone(), fail: None, seen: Default::default() };
    let tests = ["fake::replay d3d12_frame".to_owned(), "fake::golden overview".to_owned()];
    let plan = gate::tests_alone("g-12", &tests).unwrap().unwrap();
    assert_eq!(plan.kind, Kind::Tooling, "it builds, so it takes the machine lock");
    let field = gate::kind_field(plan.kind, "2 tests alone");
    let g = gate_of(&w, &hooks, plan.kind, &field, Some(&plan));
    assert_eq!(gate::attempt(&g, "g-12", Some("verify main for g-11"), (1, None)), Ok((0, None)));
    assert_eq!(
        recorded(&w).last().map(String::as_str),
        Some("ci|--changed|--base|origin/main|--test-alone|fake::replay d3d12_frame|--test-alone|fake::golden overview|--only|")
    );
    let rc = std::fs::read_to_string(w.target.join(gate::LOG_DIR).join("g-12.rc")).unwrap();
    assert_eq!(gate::passed_head(&rc), Ok(w.head.as_str()), "{rc}");
    assert!(rc.contains("rerun=fake::replay::d3d12_frame,fake::golden::overview"), "{rc}");
}

/// mornycorp/swarm-core#14: `ready`'s gate keeps its log, `.rc` and junit file as
/// `ready-<head>.*`, and a test that failed under load is rerun once, alone, then the doctests and
/// the steps it skipped, with `ready`'s own scope, as `ready-<head>-rerun`. Once a HEAD; a test
/// that fails again alone, or `--no-rerun`, stops it.
#[test]
fn readys_failed_test_is_rerun_alone_through_the_contract() {
    let w = world("swarm-core-ci-contract-ready");
    let failing = "fake::flaky under_load";
    let hooks = FakeCi { record: w.record.clone(), fail: Some(failing), seen: Default::default() };
    let logs = w.target.join(gate::LOG_DIR);
    let fail_alone = std::cell::Cell::new(false);
    let ci = |args: &[String]| {
        let mut c = Command::new(FAKE_XTASK);
        c.current_dir(&w.root).env("FAKE_CI_RECORD", &w.record).env("FAKE_CI_FAIL", failing).arg("ci").args(args);
        if fail_alone.get() {
            c.env("FAKE_CI_FAIL_ALONE", "1")
        } else {
            c.env_remove("FAKE_CI_FAIL_ALONE")
        };
        Ok(c)
    };
    let gate = |rerun| OwnGate { project: &FAKE, hooks: &hooks, agent: "fake-1", root: &w.root, base: "origin/main", logs: &logs, rerun, ci: &ci };
    let sub = own_gate::sub_of(&w.head);
    let mut events: Vec<String> = Vec::new();
    let reran = gate(true).run(&mut |a| events.push(a.get(..5).unwrap_or_default().join(" "))).unwrap();
    assert_eq!(reran, Some(own_gate::Reran { of: sub.clone(), tests: vec!["fake::flaky::under_load".to_owned()] }));
    assert_eq!(recorded(&w), ["ci|--changed|--base|origin/main", &format!("ci|--changed|--base|origin/main|--test-alone|{failing}|--doctests|--only|layers")]);
    let again = format!("{sub}-rerun");
    let subs = [&sub, &sub, &again, &again].map(|s| s.as_str());
    assert_eq!(
        events,
        ["sub.start", "sub.stop", "sub.start", "sub.stop"].iter().zip(subs).map(|(e, s)| format!("{e} --agent fake-1 --sub {s}")).collect::<Vec<_>>()
    );
    for file in [format!("{sub}.log"), format!("{sub}.junit.xml"), format!("{again}.log")] {
        assert!(logs.join(&file).is_file(), "{file}");
    }
    let first = std::fs::read_to_string(logs.join(format!("{sub}.rc"))).unwrap();
    assert!(first.starts_with("rc=100 ") && first.contains("cause=test kind=tooling (ready)"), "{first}");
    let rc = std::fs::read_to_string(logs.join(format!("{again}.rc"))).unwrap();
    assert_eq!(gate::passed_head(&rc), Ok(w.head.as_str()));

    // One rerun a HEAD: the next failure of the same HEAD stops.
    let e = gate(true).run(&mut |_| {}).unwrap_err();
    assert!(e.contains("cargo xtask ci --changed failed (rc=100") && e.contains("rerun already on this HEAD"), "{e}");
    // A test that fails again alone stops too, and so does `--no-rerun`, at once.
    std::fs::remove_file(logs.join(format!("{again}.rc"))).unwrap();
    fail_alone.set(true);
    assert!(gate(true).run(&mut |_| {}).unwrap_err().contains(&format!("its rerun {again} failed too")));
    let before = recorded(&w).len();
    assert!(gate(false).run(&mut |_| {}).unwrap_err().contains("no rerun of its tests alone: --no-rerun"));
    assert_eq!(recorded(&w).len(), before + 1, "the gate, and no rerun");
}

/// The fake is the contract's executable form: every command line the doc lists is taken, and
/// nothing else.
/// docs/ci-contract.md › The steps (#48): the full run keeps the app's `conformance` step, and the doc
/// and the fake agree on it. `--changed` and `--smoke` need not run it, and `--only` can name it.
#[test]
fn the_full_run_keeps_the_conformance_step() {
    let dir = swarm_core_testkit::temp_dir("core-ci-contract-conformance");
    let summary = |args: &[&str]| {
        let out = Command::new(FAKE_XTASK).current_dir(&dir).args(args).env_remove("FAKE_CI_FAIL").output().unwrap();
        String::from_utf8(out.stdout).unwrap().lines().find(|l| l.starts_with("CI summary")).unwrap_or_default().to_owned()
    };
    assert!(summary(&["ci"]).contains("conformance"), "the full run: {}", summary(&["ci"]));
    for narrower in [&["ci", "--smoke"][..], &["ci", "--changed", "--base", "origin/main"]] {
        assert!(!summary(narrower).contains("conformance"), "{narrower:?}");
    }
    assert!(summary(&["ci", "--test-alone", "a::b c", "--only", "conformance"]).ends_with("(alone a::b c, conformance) in 0.0 min"));
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ci-contract.md")).unwrap().replace("\r\n", "\n");
    let steps = doc.split("## The steps").nth(1).and_then(|s| s.split("\n## ").next()).unwrap_or_default();
    assert!(
        steps.contains("**`conformance`**") && steps.contains("The full run (no arguments) keeps it"),
        "the doc lists the step the full run keeps: {steps}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_fake_consumer_takes_the_contract_and_nothing_else() {
    let dir = swarm_core_testkit::temp_dir("swarm-core-ci-contract-fake");
    let run = |args: &[&str]| Command::new(FAKE_XTASK).current_dir(&dir).args(args).env_remove("FAKE_CI_FAIL").output().unwrap().status.code();
    for ok in [
        &["ci"][..],
        &["ci", "--smoke"],
        &["ci", "--changed", "--base", "origin/main"],
        &["ci", "--changed", "--base", "origin/main", "--plan"],
        &["ci", "--changed", "--base", "origin/main", "--plan", "--head", "origin/7-x"],
        &["ci", "--test-alone", "a::b c", "--doctests", "--only", "layers"],
        &["ci", "--changed", "--base", "origin/main", "--test-alone", "a::b c", "--only", "layers"],
    ] {
        assert_eq!(run(ok), Some(0), "{ok:?}");
    }
    for bad in [
        &["build"][..],
        &["ci", "--changed"],
        &["ci", "--base", "x"],
        &["ci", "--plan"],
        &["ci", "--changed", "--base", "x", "--head", "y"],
        &["ci", "--smoke", "--changed", "--base", "x"],
        &["ci", "--test-alone", "a::b c"],
        &["ci", "--test-alone", "abc", "--only", "x"],
        &["ci", "--fast"],
    ] {
        assert_eq!(run(bad), Some(2), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
