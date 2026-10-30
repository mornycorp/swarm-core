//! A new consumer's `cargo xtask`, the smallest that runs a swarm on Swarm Core
//! (docs/new-swarm.md): its `Project`, its `Hooks`, the dispatch of the swarm's commands, and a
//! `ci` that keeps the `ci` contract (docs/ci-contract.md). Copy it to `xtask/src/main.rs` in the
//! new repo, then change every value marked `CHANGE`. `cargo test` builds it here, so it keeps up
//! with Swarm Core's API.
//!
//! Its `ci` is a starting point: fmt, clippy, nextest, `layers` and the pinned assets' check, with
//! `--changed` narrowed by Swarm Core's plan. A consumer adds its own steps (a smoke's golden
//! tests and headless run, its generated docs' checks, layers) as it grows.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use swarm_core::changed::{self, Plan};
use swarm_core::gate::rerun::Test;
use swarm_core::project::{Branches, GitHub, Labels, Limits, ReadyWatch};
use swarm_core::{Hooks, Project};

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

/// The values of the pinned assets' own placeholders (`swarm-core sync-assets`) and the templates'
/// (`swarm-core init`). CHANGE: to the new repo's.
const ASSET_VARS: &[(&str, &str)] = &[
    ("product", "My Swarm"),
    ("app", "my-app"),
    ("cli", "my-cli"),
    ("ui_crate", "my-ui"),
    ("task_links", "- Gap: <!-- the gap id it moves (G4), or \"none\" -->"),
    ("task_done", "- [ ] Docs the change touches updated"),
    ("mr_moves", "gap ids (G4), milestone"),
    ("mr_ci", ""),
    ("mr_ci_steps", ""),
    ("mr_docs", "Technical docs current; progress summaries left to the coordinator"),
];

/// The workspace's layers, top first (docs/architecture.md, `cargo xtask layers`). CHANGE: to the
/// new repo's crates; register every new crate at the lowest layer it fits.
const LAYERS: &[swarm_core::layers::Entry] = &[
    swarm_core::layers::Entry { package: "my-app", layer: None, kind: "app", summary: "the desktop app and its control server" },
    swarm_core::layers::Entry { package: "my-cli", layer: None, kind: "app", summary: "the headless CLI: commands, run, serve, mcp" },
    swarm_core::layers::Entry { package: "xtask", layer: None, kind: "tool", summary: "cargo xtask: ci, layers, and the swarm's commands from swarm-core" },
    swarm_core::layers::Entry { package: "my-ui", layer: Some(4), kind: "L4", summary: "the UI shell: panes, menus, UI state as data" },
    swarm_core::layers::Entry {
        package: "my-engine",
        layer: Some(3),
        kind: "L3",
        summary: "the session and the typed command registry: every action is a command",
    },
    swarm_core::layers::Entry { package: "my-model", layer: Some(0), kind: "L0", summary: "the domain's data and pure algorithms" },
];

/// This swarm's behaviour where it differs from Swarm Core's defaults: here, its package prefix,
/// its placeholders and its layers. Add `gate_kind` (a tooling gate), `extra_causes`, `around_ci`,
/// `full_gate_capable`, `full_gate_env` as the repo needs them (`swarm_core::Hooks`).
struct Ours;

impl Hooks for Ours {
    fn changed(&self) -> changed::Rules {
        // CHANGE: the prefix the workspace's package names share.
        changed::Rules { package_prefix: "my-", ..changed::Rules::NONE }
    }

    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        ASSET_VARS
    }

    fn layers(&self) -> Option<swarm_core::layers::Table> {
        Some(swarm_core::layers::Table {
            entries: LAYERS,
            ui_layer: 4,
            ui_crates: &["egui", "eframe", "winit"],
            dev_only: &[],
            reply_readers: &["crates/ui/src/", "apps/"],
            allowed_reads: &[],
        })
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
    let steps = steps(&c, &scope);
    let started = Instant::now();
    let mut lines = Vec::new();
    let mut failed = false;
    for (name, cmd) in &steps {
        if failed {
            lines.push(format!("  skip  {name}"));
            continue;
        }
        println!("=== ci: {name} ===");
        if run(cmd) {
            lines.push(format!("  ok    {name}"));
        } else {
            lines.push(format!("  FAIL  {name}: `cargo {}` failed", cmd.join(" ")));
            failed = true;
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
    let names: Vec<&str> = steps.iter().map(|(n, _)| n.as_str()).collect();
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

/// The steps to run, each a name and cargo's arguments: the tests to run alone first, then the
/// doctests, then the gate's own steps (all of them, or `--only`'s).
fn steps(c: &Ci, scope: &[String]) -> Vec<(String, Vec<String>)> {
    let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let scoped = |a: &[&str], tail: &[&str]| {
        let mut v = args(a);
        v.extend(scope.iter().cloned());
        v.extend(args(tail));
        v
    };
    let mut gate: Vec<(String, Vec<String>)> = vec![
        ("fmt".into(), args(&["fmt", "--all", "--", "--check"])),
        ("clippy".into(), scoped(&["clippy", "--all-targets", "--locked"], &["--", "-D", "warnings"])),
    ];
    if c.smoke {
        gate.push(("build".into(), args(&["build", "--workspace", "--exclude", "xtask"])));
    } else {
        gate.push(("test".into(), scoped(&["nextest", "run", "--profile", "ci"], &[])));
    }
    gate.push(("layers".into(), args(&["xtask", "layers"])));
    gate.push(("assets".into(), args(&["xtask", "swarm-core", "sync-assets", "--check"])));
    let mut out: Vec<(String, Vec<String>)> = c.alone.iter().map(|t| (format!("alone {}", t.spec()), t.nextest_args())).collect();
    if c.doctests {
        out.push(("doctests".into(), args(&["test", "--doc", "--workspace"])));
    }
    match &c.only {
        Some(only) => out.extend(gate.into_iter().filter(|(n, _)| only.contains(n))),
        None => out.extend(gate),
    }
    out
}

/// Run `cargo <args>`: whether it passed.
fn run(args: &[String]) -> bool {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())).args(args).status().is_ok_and(|s| s.success())
}
