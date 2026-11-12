//! A new consumer's `cargo xtask`, the smallest that runs a swarm on Swarm Core
//! (docs/new-swarm.md): its `Project`, its `Hooks`, the dispatch of the swarm's commands, and a
//! `ci` that keeps the `ci` contract (docs/ci-contract.md). Copy it to `xtask/src/main.rs` in the
//! new repo, then change every value marked `CHANGE`. `cargo test` builds it here, so it keeps up
//! with Swarm Core's API.
//!
//! Its `ci` is a starting point: fmt, clippy, the architecture's checks (`layers`), nextest and the
//! pinned assets' check, and in an app's full run the `conformance` step, with `--changed` narrowed by
//! Swarm Core's plan. A consumer adds its
//! own steps (a smoke's golden tests and headless run, its generated docs' checks) as it grows.
//!
//! **`cargo xtask layers`** is `swarm_core::layers::run`, which runs `swarm_core::checks`
//! ([`Ours::architecture`] and [`Ours::scans`] configure it): the layers each crate declares in its manifest, the lints and
//! headers of every crate root, and the scans of docs/app-architecture.md, pointed at the files of
//! the app skeleton (`cargo xtask swarm-core init --app`: docs/new-swarm.md › The app's skeleton).
//! This xtask is the `tooling` layer: its own manifest declares it, and its root carries the same
//! headers as every crate's.
//!
//! **An app or not** is read from the workspace, not set here: `swarm_core::skeleton::is_app`
//! (the skeleton's `crates/engine` is there). In a repo with no app (a library, a tooling repo) the
//! skeleton's scans and the `conformance` step are left out, as they name files and a package it
//! doesn't have; once `swarm-core init --app` has written the skeleton they run, and the UI crate's
//! scans with them once `--ui` has. One xtask serves the repo before and after. CHANGE: if the app's
//! crates are laid out other than as the skeleton has them, replace the `skeleton::is_app` and
//! `skeleton::scans_at` calls with the repo's own answer.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use swarm_core::changed::{self, Plan};
use swarm_core::checks::{Layers, Scan};
use swarm_core::gate::rerun::Test;
use swarm_core::project::{Branches, GitHub, Labels, Limits, ReadyWatch};
use swarm_core::{Hooks, Project, skeleton};

/// What this swarm is. CHANGE: every value, to the new repo's.
pub const PROJECT: Project = Project {
    name: "my-swarm",
    env_prefix: "MY_SWARM",
    github: GitHub { host: "github.com", repo: "owner/my-swarm" },
    branches: Branches { main: "main", tracking: "tracking", remote: "origin" },
    labels: Labels { in_progress: "status::in-progress", blocked: "status::blocked", main_red: "main-red", bug: "type::bug" },
    limits: Limits { gate_lock_max_age_secs: 2 * 3600 },
    checkout_marker: "my-swarm-checkout.txt",
    gate_lock_file: "my-swarm-gate.lock",
    tracking_queue: "my-swarm-tracking-queue.jsonl",
    pr_template: ".github/pull_request_template.md",
    generated: &[],
    stack_state: "my-swarm-stack.json",
    full_gate_worktree: ".claude/worktrees/fullgate",
    xtask_rules: &["xtask", "Cargo.toml", "Cargo.lock", ".cargo"],
    ready_watch: ReadyWatch::Stream,
    gate_tag: "",
    golden_binary: "my-engine::golden",
};

/// The app skeleton's CLI binary and package (`apps/<cli>`). CHANGE: to the new repo's (it is `cli` below too).
const CLI: &str = "my-cli";

/// The values of the pinned assets' own placeholders (`swarm-core sync-assets`) and the templates'
/// (`swarm-core init`). CHANGE: to the new repo's.
const ASSET_VARS: &[(&str, &str)] = &[
    ("product", "My Swarm"),
    ("app", "my-app"),
    ("cli", CLI),
    ("ui_crate", "my-ui"),
    ("task_links", "- Gap: <!-- the gap id it moves (G4), or \"none\" -->"),
    ("task_done", "- [ ] Docs the change touches updated"),
    ("mr_moves", "gap ids (G4), milestone"),
    ("mr_ci", ""),
    ("mr_ci_steps", ""),
    ("mr_docs", "Technical docs current; progress summaries left to the coordinator"),
];

/// This swarm's behaviour where it differs from Swarm Core's defaults: here, its package
/// prefix, its placeholders and the architecture's scans. Add `gate_kind` (a tooling gate),
/// `extra_causes`, `around_ci`, `full_gate_capable`, `full_gate_env` as the repo needs them
/// (`swarm_core::Hooks`), and change `architecture` for an app whose layers differ from the
/// contract's.
struct Ours;

impl Hooks for Ours {
    fn changed(&self) -> changed::Rules {
        // CHANGE: the prefix the workspace's package names share (the app skeleton's crates take it).
        changed::Rules { package_prefix: "my-", ..changed::Rules::NONE }
    }

    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        ASSET_VARS
    }

    /// The layers each crate's manifest declares (`[package.metadata.swarm] layer = "engine"`),
    /// held to the contract's order (docs/app-architecture.md › Layers). CHANGE: the names, the UI
    /// toolkits or the lints, where the app's differ.
    fn architecture(&self) -> Option<Layers> {
        Some(Layers::STANDARD)
    }

    /// The scans of the app skeleton's files, for the workspace as it is
    /// (`swarm_core::skeleton::scans_at`): none in a repo with no app, the UI crate's too once
    /// `swarm-core init --app --ui` has written it. CHANGE: add the app's own as it grows (the
    /// process module in `process_command`'s `except`, the state module's), each allowance with its
    /// reason and a count.
    fn scans(&self) -> Vec<Scan> {
        skeleton::scans_at(&root())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let root = root();
    let result = match args.split_first() {
        Some((&"ci", rest)) => ci(&root, rest),
        Some((&"layers", rest)) => swarm_core::layers::run(&Ours, &root, rest),
        // The swarm's commands: claim, event, gate, stack, land, prwatch, swarm-core, …
        Some((command, rest)) if swarm_core::serves(command) => swarm_core::run(&PROJECT, &Ours, &root, command, rest),
        _ => {
            println!(
                "cargo xtask ci [--changed --base <ref> [--plan [--head <ref>]] | --smoke] [--test-alone \"<binary id> <test>\"]… [--doctests] [--only <step>,…]"
            );
            println!("{}", swarm_core::layers::USAGE);
            for (_, usage) in swarm_core::COMMANDS {
                println!("{usage}");
            }
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
    Ok(c)
}

/// `cargo xtask ci`: the gate's steps, then the `CI summary` Swarm Core's gate reads.
fn ci(root: &Path, args: &[&str]) -> Result<(), String> {
    let c = parse(args)?;
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    swarm_core::checkout::claim(&PROJECT, &target, root)?;
    let scope = match &c.base {
        Some(base) => changed_scope(root, base, c.head.as_deref(), c.plan)?,
        None => Some(vec!["--workspace".to_owned()]),
    };
    let Some(scope) = scope else { return Ok(()) };
    let steps = steps(&c, &scope, skeleton::is_app(root));
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
        match run(step) {
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

/// `--changed`'s cargo scope from Swarm Core's plan (`-p <package>`…, or the workspace when
/// the change reaches everything); `None` after `--plan` printed it.
fn changed_scope(root: &Path, base: &str, head: Option<&str>, plan_only: bool) -> Result<Option<Vec<String>>, String> {
    let out = Command::new("cargo")
        .current_dir(root)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let (crates, packages) = (changed::crates(&meta), changed::package_dirs(&meta, root));
    let files = changed::changed_files(root, base, head)?;
    let lock = changed::lock_for(root, &files, base, head, &crates);
    let plan = changed::plan(&Ours, &files, &packages, &crates, lock.as_ref());
    print!("{}", changed::describe(&Ours, &plan, base, crates.len()));
    if plan_only {
        return Ok(None);
    }
    Ok(Some(match plan {
        Plan::Everything(_) => vec!["--workspace".to_owned()],
        Plan::Crates { crates, .. } => crates.keys().flat_map(|p| ["-p".to_owned(), p.clone()]).collect(),
    }))
}

/// One step of `ci`: its name (the `CI summary`'s, `--only`'s), cargo's arguments, and a line its
/// output must have besides passing (a test step that must have run its test).
struct Step {
    name: String,
    args: Vec<String>,
    expect: Option<&'static str>,
}

impl Step {
    fn new(name: &str, args: Vec<String>) -> Self {
        Step { name: name.to_owned(), args, expect: None }
    }
}

/// The app's conformance test (`apps/<cli>/tests/surfaces.rs`): the full run's `conformance` step.
const CONFORMANCE: &str = "every_command_gives_the_same_answer_on_every_surface";

/// The steps to run: the tests to run alone first, then the doctests, then the gate's own steps (all
/// of them, or `--only`'s). `app`: the workspace is an app made from the skeleton.
fn steps(c: &Ci, scope: &[String], app: bool) -> Vec<Step> {
    let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let scoped = |a: &[&str], tail: &[&str]| {
        let mut v = args(a);
        v.extend(scope.iter().cloned());
        v.extend(args(tail));
        v
    };
    let mut gate = vec![
        Step::new("fmt", args(&["fmt", "--all", "--", "--check"])),
        Step::new("clippy", scoped(&["clippy", "--all-targets", "--locked"], &["--", "-D", "warnings"])),
        Step::new("layers", args(&["xtask", "layers"])),
    ];
    if c.smoke {
        gate.push(Step::new("build", args(&["build", "--workspace", "--exclude", "xtask"])));
    } else {
        // `--no-tests=warn`: a scope with nothing to test (a repo with no app may have only this
        // xtask, with none) is no failure. That the app's own test ran, the `conformance` step holds.
        gate.push(Step::new("test", scoped(&["nextest", "run", "--profile", "ci", "--no-tests=warn"], &[])));
    }
    gate.push(Step::new("assets", args(&["xtask", "swarm-core", "sync-assets", "--check"])));
    // An app's full run keeps its conformance run as a step of its own (docs/ci-contract.md › The
    // steps): the test step runs it too, but a command that breaks a surface then fails the gate by
    // name. The test is named `--exact`, and the step fails unless that one test ran, so a renamed
    // test fails it rather than matching nothing. A repo with no app has no such step.
    if app && c.base.is_none() && !c.smoke {
        let args = args(&["test", "-p", CLI, "--test", "surfaces", "--", "--exact", CONFORMANCE]);
        gate.push(Step { name: "conformance".to_owned(), args, expect: Some("test result: ok. 1 passed;") });
    }
    let mut out: Vec<Step> = c.alone.iter().map(|t| Step::new(&format!("alone {}", t.spec()), t.nextest_args())).collect();
    if c.doctests {
        out.push(Step::new("doctests", args(&["test", "--doc", "--workspace"])));
    }
    match &c.only {
        Some(only) => out.extend(gate.into_iter().filter(|s| only.contains(&s.name))),
        None => out.extend(gate),
    }
    out
}

/// Run `cargo <args>` for `step`: why it failed, if it did. A step that `expect`s a line passes only
/// when its output has it.
fn run(step: &Step) -> Result<(), String> {
    let line = format!("cargo {}", step.args.join(" "));
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.args(&step.args);
    let Some(expect) = step.expect else {
        return if cmd.status().is_ok_and(|s| s.success()) { Ok(()) } else { Err(format!("`{line}` failed")) };
    };
    let out = cmd.output().map_err(|e| format!("`{line}`: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    print!("{stdout}");
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    match (out.status.success(), stdout.contains(expect)) {
        (false, _) => Err(format!("`{line}` failed")),
        (true, false) => Err(format!("`{line}` passed without `{expect}`: the test it names didn't run")),
        (true, true) => Ok(()),
    }
}
