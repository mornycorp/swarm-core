//! The swarm tooling its consumers' agent swarms share:
//! claims, the tracking timeline, the gates, `stack` and `land`. Each repo's `xtask` pins this crate
//! by git revision, keeps its own commands and hands the swarm's to [`run`]:
//!
//! ```ignore
//! Some(c) if swarm_core::serves(c) => swarm_core::run(&PROJECT, &Ours, &root(), c, &rest),
//! ```
//!
//! [`Project`] holds what differs between the consumers; [`Hooks`] is where behaviour only one of
//! them wants will go, each method with a default that keeps the other's unchanged. The modules
//! are public too: a consumer's own commands call them as a library.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

use std::path::Path;

pub mod assets;
pub mod capabilities;
pub mod capacity;
pub mod changed;
pub mod checkout;
pub mod claim;
pub mod empty;
pub mod event;
pub mod finish;
pub mod fullgate;
pub mod gate;
pub mod gate_times;
pub mod github;
pub mod land;
pub mod layers;
pub mod lock;
pub mod machine_lock;
pub mod needs;
pub mod pin;
pub mod process;
pub mod project;
pub mod prwatch;
pub mod ready;
pub mod stack;
pub mod templates;
pub mod tidy;
pub mod time;

pub use project::Project;

/// This crate's version, from its manifest. The revision a consumer pins is in its `Cargo.lock`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Behaviour that differs between the consumers, beyond [`Project`]'s settings
/// Each method has a default that keeps one
/// consumer's behaviour; the other overrides it. A project with its own change planner overrides
/// `gate_kind`; a project with a GPU gate may override `gate_kind`, `extra_causes`, `around_ci`,
/// `full_gate_capable` and `full_gate_env`. Both describe their workspace to `ci --changed`'s plan (`changed`,
/// `xtask_reads`, `file_class`) and fill in their assets' placeholders (`asset_vars`).
pub trait Hooks {
    /// The gate a change at `root` needs against `origin/main`, and why:
    /// the project's planner decides docs, tooling or full (the coordinator turns full
    /// into the smoke, [`gate::coordinator_kind`], and a smoke of a PR readied with `--no-ci`
    /// into `ci --changed`, [`gate::untested`]). The default: full, unsure, so the smoke.
    fn gate_kind(&self, root: &Path) -> (gate::Kind, String) {
        let _ = root;
        (gate::Kind::Full, "the project plans no gate kind: unsure".to_owned())
    }

    /// The project's own cause of a failed gate, read from its `log` before the machine's
    /// ([`gate::classify`]), such as `gpu` (the GPU or the desktop under load) and `python` (no
    /// Python for its oracle tests). The default: none.
    fn extra_causes(&self, log: &str) -> Option<gate::Cause> {
        let _ = log;
        None
    }

    /// Run the gate's `ci` (`cmd`: this xtask's exe, `ci <args>`, its output to the log, set up),
    /// changing what it must first and cleaning up after; its exit code. A project's may set its
    /// Python and its GPU lease, runs the stack's own xtask when the stack changed it, and ends a
    /// leftover `vkcube`. The default: run it as it is ([`gate::run_status`]).
    fn around_ci(&self, run: &gate::CiRun<'_>, cmd: std::process::Command) -> Result<i32, String> {
        let _ = run;
        gate::run_status(cmd)
    }

    /// Whether this machine runs the periodic full gate when `{prefix}_FULLGATE` doesn't say. The
    /// default: a Windows machine, the owner's desktops (docs/swarm.md › Capabilities,
    /// `full-gate`). A GPU project's might be one with a GPU that doesn't skip its GPU tests.
    fn full_gate_capable(&self) -> bool {
        cfg!(windows)
    }

    /// What the full gate `sub` adds to its run, after `ran` full gates ran before it: a field
    /// for its start note and variables for its `ci` (a test corpus's reference, say). The
    /// default: none.
    fn full_gate_env(&self, sub: &str, ran: usize) -> fullgate::FullGateEnv {
        let _ = (sub, ran);
        fullgate::FullGateEnv::default()
    }

    /// The drift interval (mornycorp/swarm-core#15): a passed full gate older than this many
    /// seconds makes the periodic full gate due again, even of an unchanged `main` (a driver, a
    /// reference build or the machine may have moved). The default: [`fullgate::DRIFT`], a day.
    fn full_gate_drift_secs(&self) -> i64 {
        fullgate::DRIFT
    }

    /// Why the inputs the full gate reads beyond `main` changed since its last pass stopped at
    /// `last_pass` (a Unix time; `None`: none passed), if they did: a driver, a reference build,
    /// pinned tools (mornycorp/swarm-core#15). Some: the gate is due now. The default: none.
    fn full_gate_inputs_changed(&self, root: &Path, last_pass: Option<i64>) -> Option<String> {
        let _ = (root, last_pass);
        None
    }

    /// Write the generated doc `doc` at `root` (`stack`, after a conflict in it). The default:
    /// this xtask's own `<doc.command>`.
    fn regenerate(&self, root: &Path, doc: &project::Generated) -> Result<(), String> {
        stack::regenerate(root, doc)
    }

    /// What `ci --changed`'s plan reads from the project: its package names'
    /// prefix, the files its packages read from outside their own directory, and its runtime
    /// edges. The default: none of them.
    fn changed(&self) -> changed::Rules {
        changed::Rules::NONE
    }

    /// Whether xtask's tests read `path` (from the root) through the workspace root, which the
    /// sources' scan ([`changed::reads`]) can't see: a change to it tests xtask. The default: none.
    fn xtask_reads(&self, path: &str) -> bool {
        let _ = path;
        false
    }

    /// The project's own class for `path` in `ci --changed`'s plan, before the shared ones
    /// ([`changed::classify`]). The default: none.
    fn file_class(&self, path: &str) -> Option<changed::FileClass> {
        let _ = path;
        None
    }

    /// The values of the project's own placeholders in the assets it pins ([`assets`]): `(name,
    /// value)`, such as `("cli", "swarm-cli")`. The default: none, so an asset that uses one is
    /// refused.
    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// What this machine can run, by the issue label that needs each (swarm-core#13): what
    /// each probe found and how to fix a missing one ([`capabilities`]). A project probes its own
    /// (its tools, its reference data, a GPU). The default: none.
    fn capabilities(&self, root: &Path) -> Vec<capabilities::Capability> {
        let _ = root;
        Vec::new()
    }
    /// The project's architecture, as `cargo xtask layers` checks it ([`layers`]): its packages'
    /// layers, its UI crates, its dev-only crates and where command replies are read. The default:
    /// none, so `layers` checks nothing.
    fn layers(&self) -> Option<layers::Table> {
        None
    }

    /// The library packages allowed `unsafe`, each with why (a GPU or OS binding that can't be
    /// written without it): `cargo xtask layers` holds their roots to `#![deny(unsafe_op_in_unsafe_fn)]`
    /// and `#![warn(clippy::undocumented_unsafe_blocks)]` in place of `#![forbid(unsafe_code)]`
    /// ([`layers::Unsafe`]). The default: none, so every library root forbids `unsafe`.
    fn unsafe_crates(&self) -> &'static [layers::Unsafe] {
        &[]
    }
}

/// Every default of [`Hooks`], for the tests.
#[cfg(test)]
pub(crate) struct Defaults;
#[cfg(test)]
impl Hooks for Defaults {}

/// The `cargo xtask` commands this crate serves, each with its usage line, as a consumer's
/// `cargo xtask help` lists them.
pub const COMMANDS: &[(&str, &str)] = &[
    ("capabilities", capabilities::USAGE),
    ("capacity", "cargo xtask capacity [--helper <name> --note \"<what>\" [--kind build|read] [--worktree] [--role <r>] [--agent <instance>]]"),
    ("claim", claim::USAGE),
    (
        "event",
        "cargo xtask event <event> [--agent <instance>] [--issue <n>] [--mr <n>] [--reason <word>] [--note <text>]: log one event to the swarm's timeline; `event flush` sends queued events",
    ),
    ("finish", finish::USAGE),
    ("swarm-core", pin::USAGE),
    ("fullgate", fullgate::USAGE),
    ("gate", gate::USAGE),
    ("gate-times", "cargo xtask gate-times <gate> [<gate>] [--top <n>] [--dir <gate-logs dir>]"),
    ("install-tools", finish::INSTALL_USAGE),
    ("land", land::USAGE),
    ("lock", lock::USAGE),
    ("prwatch", prwatch::USAGE),
    ("ready", ready::USAGE),
    ("stack", stack::USAGE),
    ("tidy", tidy::USAGE),
];

/// Whether `command` is one of [`COMMANDS`]: a consumer hands it to [`run`].
pub fn serves(command: &str) -> bool {
    COMMANDS.iter().any(|(c, _)| *c == command)
}

/// Run `command` with `args` for `project`, in the consumer's workspace `root`.
pub fn run(project: &Project, hooks: &dyn Hooks, root: &Path, command: &str, args: &[&str]) -> Result<(), String> {
    match command {
        "capacity" => capacity::run(project, root, args, &capacity::EventTimeline(project)),
        "claim" => claim::run(project, hooks, root, args),
        "event" => {
            event::run(project, root, args)?;
            // A session says at its start what this machine can't run (swarm-core#13).
            if args.first() == Some(&"session.start")
                && let Some(s) = capabilities::summary(&hooks.capabilities(root))
            {
                println!("{s}");
            }
            Ok(())
        }
        "capabilities" => capabilities::run(hooks, root, args),
        "finish" => finish::run(project, root, args),
        "swarm-core" => pin::run(project, hooks, root, args),
        "fullgate" => fullgate::run(project, hooks, root, args),
        "gate" => gate::run(project, hooks, root, args),
        "gate-times" => gate_times::run(project, root, args),
        "install-tools" => finish::run_install(root, args),
        "land" => land::run(project, root, args),
        "lock" => lock::run(project, root, args),
        "prwatch" => prwatch::run(project, root, args),
        "ready" => ready::run(project, hooks, root, args),
        "stack" => stack::run(project, hooks, root, args),
        "tidy" => tidy::run(project, root, args),
        _ => Err(format!("swarm-core serves no command `{command}`")),
    }
}
