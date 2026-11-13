//! Swarm Core's own `cargo xtask` (mornycorp/swarm-core#96): the swarm's commands, run on Swarm
//! Core itself, so its workers claim, ready and log with the commands its consumers' workers use,
//! never by hand. Made from `crates/swarm-core/examples/consumer_xtask.rs`
//! (docs/new-swarm.md › The xtask), with three differences:
//!
//! - **Swarm Core by path** (`xtask/Cargo.toml`), never by a git pin: it runs this checkout's
//!   code, the code its workers change.
//! - **What assumes a pin is refused**, saying why ([`NOT_HERE`]): `swarm-core` (`bump`,
//!   `sync-assets`, `init`, `--version`). And there is no `layers`: Swarm Core's crates are a
//!   library, not an app laid out in the contract's layers.
//! - **Its own `ci`** keeps the `ci` contract (docs/ci-contract.md) with this repo's checks
//!   ([`steps`]): fmt, clippy, the workspace's tests under nextest, the doctests, and the app
//!   skeleton's end-to-end tests (`app_skeleton -- --ignored`, which need the crates registry and
//!   `cargo nextest` and fail without them), which are the coordinator's gate (AGENTS.md ›
//!   Landing). It has no smoke of golden tests, no `layers`, no pinned assets' check and no
//!   `conformance` step: those are an app's.
//!
//! Its [`Hooks`] keep every default but two ([`Ours`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use swarm_core::gate::rerun::Test;
use swarm_core::project::{Branches, GitHub, Labels, Limits, ReadyWatch};
use swarm_core::{Hooks, Project, Proof};

/// This repo, as the swarm's commands see it.
pub const PROJECT: Project = Project {
    name: "swarm-core",
    env_prefix: "SWARM_CORE",
    github: GitHub { host: "github.com", repo: "mornycorp/swarm-core" },
    branches: Branches { main: "main", tracking: "tracking", remote: "origin" },
    labels: Labels { in_progress: "status::in-progress", blocked: "status::blocked", main_red: "main-red", bug: "type::bug" },
    limits: Limits { gate_lock_max_age_secs: 2 * 3600 },
    checkout_marker: "swarm-core-checkout.txt",
    gate_lock_file: "swarm-core-gate.lock",
    tracking_queue: "swarm-core-tracking-queue.jsonl",
    // None here yet: a claim's draft PR then says only `Closes #<n>`.
    pr_template: ".github/pull_request_template.md",
    generated: &[],
    stack_state: "swarm-core-stack.json",
    full_gate_worktree: ".claude/worktrees/fullgate",
    // What `ci` is built from. Swarm Core's code isn't: `ci` runs cargo on the tree.
    xtask_rules: &["xtask", "Cargo.toml", "Cargo.lock", ".cargo"],
    ready_watch: ReadyWatch::Stream,
    gate_tag: "",
    // No smoke of golden tests here: the smoke's test step is the skeleton's end to end.
    golden_binary: "swarm-core::app_skeleton",
    // The coordinator gates each batch's merge with `main` on every check (AGENTS.md › Landing):
    // its gate is the full `ci`, and each PR's `ready` runs `ci --changed`, leaving nothing out.
    proof: Proof::Batch { ready_env: &[] },
};

/// Swarm Core's commands that assume it is a pinned dependency, which here it is not: each
/// refused, with why.
const NOT_HERE: &[(&str, &str)] = &[(
    "swarm-core",
    "this repo is Swarm Core: it pins no copy of itself, so there is no pin to bump or show (`bump`, `--version`), no pinned copies to sync \
     (`sync-assets`) and no starting points to write (`init`). A change lands here; each consumer takes it with its own `cargo xtask swarm-core bump`.",
)];

/// Swarm Core's behaviour on itself: every default of [`Hooks`] but two.
///
/// - [`Hooks::claim_holds_first`]: on. Claims here were made by hand, with nothing to settle two
///   made at once, so a claim holds its issue on the tracking branch before it makes anything
///   (`swarm_core::claim::hold`).
/// - [`Hooks::full_gate_capable`]: no machine runs the periodic full gate of `main` by default
///   (`SWARM_CORE_FULLGATE=on` does): the coordinator's gate runs the full `ci` on every batch
///   before it lands ([`PROJECT`]'s batch proof), so each landing proves `main`, and a claim here
///   shouldn't wait on a gate.
struct Ours;

impl Hooks for Ours {
    fn claim_holds_first(&self) -> bool {
        true
    }

    fn full_gate_capable(&self) -> bool {
        false
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let root = root();
    let refused = args.first().and_then(|c| NOT_HERE.iter().find(|(n, _)| n == c));
    let result = match (args.split_first(), refused) {
        (_, Some((command, why))) => Err(format!("`cargo xtask {command}` doesn't apply here: {why}")),
        (Some((&"ci", rest)), None) => ci(&root, rest),
        (Some((command, rest)), None) if swarm_core::serves(command) => swarm_core::run(&PROJECT, &Ours, &root, command, rest),
        (Some((command, _)), None) if !matches!(*command, "help" | "-h" | "--help") => {
            help();
            Err(format!("no command `{command}`"))
        }
        _ => {
            help();
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The commands, each with its usage, and those that don't apply here.
fn help() {
    println!(
        "cargo xtask ci [--changed --base <ref> [--plan [--head <ref>]] | --smoke] [--test-alone \"<binary id> <test>\"]... [--doctests] [--only <step>,...]"
    );
    for (command, usage) in swarm_core::COMMANDS {
        if !NOT_HERE.iter().any(|(n, _)| n == command) {
            println!("{usage}");
        }
    }
    for (command, why) in NOT_HERE {
        println!("\nnot here: cargo xtask {command}: {why}");
    }
}

/// The workspace root, whichever directory `cargo xtask` runs from.
fn root() -> PathBuf {
    let runtime = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    swarm_core::checkout::resolve_root(runtime.as_deref(), cwd.as_deref(), Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// A `ci` command line, as the contract has it.
#[derive(Default)]
struct Ci {
    base: Option<String>,
    plan: bool,
    head: Option<String>,
    smoke: bool,
    alone: Vec<Test>,
    doctests: bool,
    only: Option<Vec<String>>,
}

fn parse(args: &[&str]) -> Result<Ci, String> {
    let mut c = Ci::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().map(|v| (*v).to_owned()).ok_or_else(|| format!("{a} needs a value"));
        match *a {
            "--changed" => c.base = Some(c.base.take().unwrap_or_default()),
            "--base" => c.base = Some(value()?),
            "--plan" => c.plan = true,
            "--head" => c.head = Some(value()?),
            "--smoke" => c.smoke = true,
            "--test-alone" => c.alone.push(Test::parse(&value()?)?),
            "--doctests" => c.doctests = true,
            "--only" => c.only = Some(value()?.split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect()),
            other => return Err(format!("ci: unknown argument `{other}`")),
        }
    }
    if c.base.as_deref() == Some("") {
        return Err("ci: --changed needs --base <ref>".into());
    }
    if c.smoke && c.base.is_some() {
        return Err("ci: --smoke or --changed, not both".into());
    }
    Ok(c)
}

/// `cargo xtask ci`: the gate's steps, then the `CI summary` Swarm Core's gate reads.
fn ci(root: &Path, args: &[&str]) -> Result<(), String> {
    let c = parse(args)?;
    if c.plan {
        // Three packages, each reaching the others: a change is planned as the whole workspace.
        let base = c.base.as_deref().ok_or("ci: --plan goes with --changed --base <ref>")?;
        println!("ci --changed --base {base}: the whole workspace (swarm-core, swarm-core-testkit, xtask)");
        return Ok(());
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    swarm_core::checkout::claim(&PROJECT, &target, root)?;
    let steps = steps(&c);
    let started = Instant::now();
    let mut lines = Vec::new();
    let mut failed = false;
    for step in &steps {
        let name = &step.name;
        if failed {
            lines.push(format!("  skip  {name}"));
            continue;
        }
        println!("=== ci: {name} ===");
        match run(root, step) {
            Ok(()) => lines.push(format!("  ok    {name}")),
            Err(why) => {
                lines.push(format!("  FAIL  {name}: {why}"));
                failed = true;
            }
        }
    }
    println!();
    if failed {
        println!("CI summary:");
        for l in &lines {
            println!("{l}");
        }
        return Err("ci failed".into());
    }
    let names: Vec<&str> = steps.iter().map(|s| s.name.as_str()).collect();
    println!("CI summary: all {} steps passed ({}) in {:.1} min", steps.len(), names.join(", "), started.elapsed().as_secs_f64() / 60.0);
    Ok(())
}

/// One step of `ci`: its name (the `CI summary`'s, `--only`'s) and cargo's arguments.
struct Step {
    name: String,
    args: Vec<String>,
}

impl Step {
    fn new(name: &str, args: &[&str]) -> Self {
        Step { name: name.to_owned(), args: args.iter().map(|s| (*s).to_owned()).collect() }
    }
}

/// The app skeleton's end-to-end tests: they need the crates registry and `cargo nextest`, and
/// fail without them rather than skip.
const SKELETON: &[&str] = &["test", "-p", "swarm-core", "--test", "app_skeleton", "--locked", "--", "--ignored"];

/// The steps to run: the tests to run alone first, then the doctests when asked, then the gate's
/// own (all of them, or `--only`'s). The full run (no arguments): fmt, clippy, test, doctests and
/// skeleton; `--changed` (each PR's `ready`): fmt, clippy and test; `--smoke`: fmt, clippy and
/// skeleton.
fn steps(c: &Ci) -> Vec<Step> {
    let full = c.base.is_none() && !c.smoke;
    let mut gate = vec![
        Step::new("fmt", &["fmt", "--all", "--", "--check"]),
        Step::new("clippy", &["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]),
    ];
    if !c.smoke {
        // `--no-tests=warn`: a scope with nothing to test is no failure.
        gate.push(Step::new("test", &["nextest", "run", "--workspace", "--profile", "ci", "--locked", "--no-tests=warn"]));
    }
    if full {
        gate.push(Step::new("doctests", &["test", "--doc", "--workspace", "--locked"]));
    }
    if c.base.is_none() {
        gate.push(Step::new("skeleton", SKELETON));
    }
    if let Some(only) = &c.only {
        gate.retain(|s| only.contains(&s.name));
    }
    let mut out: Vec<Step> = c.alone.iter().map(|t| Step { name: format!("alone {}", t.spec()), args: t.nextest_args() }).collect();
    if c.doctests && !gate.iter().any(|s| s.name == "doctests") {
        out.push(Step::new("doctests", &["test", "--doc", "--workspace", "--locked"]));
    }
    out.extend(gate);
    out
}

/// Run `cargo <args>` for `step` at `root`: why it failed, if it did.
fn run(root: &Path, step: &Step) -> Result<(), String> {
    let line = format!("cargo {}", step.args.join(" "));
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root).args(&step.args);
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("`{line}` failed ({s})")),
        Err(e) => Err(format!("`{line}`: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(args: &[&str]) -> Vec<String> {
        steps(&parse(args).unwrap()).into_iter().map(|s| s.name).collect()
    }

    /// Every form the `ci` contract calls it with is taken (docs/ci-contract.md › Who calls it),
    /// with this repo's steps; anything else is refused.
    #[test]
    fn ci_takes_every_form_of_the_contract() {
        assert_eq!(names(&[]), ["fmt", "clippy", "test", "doctests", "skeleton"], "the full gate: every check, the skeleton's included");
        assert_eq!(names(&["--changed", "--base", "origin/main"]), ["fmt", "clippy", "test"], "ready, and the docs and tooling gates");
        assert_eq!(names(&["--smoke"]), ["fmt", "clippy", "skeleton"]);
        let rerun = ["--changed", "--base", "origin/main", "--test-alone", "swarm-core claim::tests::x", "--doctests", "--only", "test"];
        assert_eq!(names(&rerun), ["alone swarm-core claim::tests::x", "doctests", "test"]);
        assert_eq!(names(&["--test-alone", "swarm-core a::b", "--doctests", "--only", "doctests,skeleton"]), ["alone swarm-core a::b", "doctests", "skeleton"]);
        assert_eq!(names(&["--changed", "--base", "origin/main", "--test-alone", "swarm-core a::b", "--only", ""]), ["alone swarm-core a::b"]);
        assert!(parse(&["--changed", "--base", "main", "--plan", "--head", "origin/x"]).is_ok_and(|c| c.plan));
        assert!(parse(&["--changed"]).is_err());
        assert!(parse(&["--smoke", "--changed", "--base", "main"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        let skeleton = steps(&parse(&[]).unwrap()).into_iter().find(|s| s.name == "skeleton").unwrap();
        assert_eq!(skeleton.args, SKELETON);
    }

    /// The commands that assume a pin are refused, the rest are Swarm Core's own, and this
    /// repo's settings are its own.
    #[test]
    fn what_doesnt_apply_here_is_refused_and_the_rest_is_served() {
        for (command, why) in NOT_HERE {
            assert!(swarm_core::serves(command) && why.contains("Swarm Core"), "{command}");
        }
        for command in ["claim", "ready", "event", "prwatch", "capacity", "gate", "land", "stack", "finish"] {
            assert!(swarm_core::serves(command) && !NOT_HERE.iter().any(|(n, _)| *n == command), "{command}");
        }
        assert_eq!((PROJECT.github.repo, PROJECT.env_name("AGENT")), ("mornycorp/swarm-core", "SWARM_CORE_AGENT".to_owned()));
        assert_eq!(swarm_core::claim::issue_of_branch("73-own-claim"), Some(73), "branches are <n>-<slug>");
        assert!(Ours.claim_holds_first() && !Ours.full_gate_capable());
    }
}
