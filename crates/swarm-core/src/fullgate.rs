//! `cargo xtask fullgate` (the owner's direction of 2026-10-22 for
//! both swarms): the periodic full gate of `main`. The coordinator merges after a smoke (`ci
//! --smoke`), and the whole `ci` moves here: run by the next agent that can, when it claims a task
//! (`claim` calls [`before_claim`] with the issue held).
//!
//! - **When** ([`due`]): nobody's full gate is open, `main` hasn't already failed one for the
//!   change's cause since its last pass (its `main-red` issue covers it), and either `main` moved
//!   since the last passed full gate, which is more than [`MIN_AGE`] old, or the drift interval
//!   passed since it ([`Drift`], a day by default), or the gate's inputs changed. A landed
//!   `--full` gate that left `main` at its tree is a pass of that commit
//!   (mornycorp/swarm-core#15, [`landed`]), so only merges no full gate verified move `main`
//!   on, and within the drift interval an unchanged `main` is never gated twice.
//! - **One at a time, across agents** ([`race`], then [`lock_ref`]): push the start, read the
//!   tracking branch again, and the earliest open start goes on; the others log a `superseded`
//!   stop and go on to their tasks. Each agent's own clock writes its start, so the earliest is
//!   only a first cut: the gate that goes on must also take the lock ref on the remote by
//!   compare-and-swap, which the server grants to one. A start with
//!   no stop after [`STALE`] is abandoned. The race is run before the shared checkout is touched.
//! - **What:** the whole `ci` on a clean checkout of `origin/main` at the commit it recorded
//!   ([`Project::full_gate_worktree`], beside the primary checkout; `main`'s own `cargo xtask
//!   ci`), holding the machine-wide gate lock so it never runs beside either swarm's gate, and a
//!   test-run slot ([`crate::test_runs`]) so it never tests beside more runs than the machine
//!   takes (a worker's `ready` among them). Holding the machine lock it reads again
//!   ([`recheck`]): a `main` another gate verified while it waited is left to that gate, and a
//!   `main` that moved is gated at its new tip. `claim` starts one only with room for a build
//!   ([`before_claim`]), which there isn't while every slot is taken. A
//!   project's extra inputs ([`Hooks::full_gate_env`]: a test corpus's reference, say) go into its start
//!   note and its `ci`'s environment.
//! - **Whose code**: the agent that won the race only checks the
//!   commit out; it then hands over ([`hand_over`]): `cargo run -p xtask -- fullgate --run <sub>
//!   --commit <sha>` in the checkout, so `main`'s own xtask, built there, runs the gate (the
//!   machine, the attempts, the rerun, the pass logged or `main-red` filed) and ends with an
//!   [`OUTCOME`] line. The caller reads it, and claims a `main-red` issue the gate filed in its own
//!   worktree, so the shared checkout stays detached. When `main`'s xtask leaves no
//!   outcome (it doesn't build, say), the caller runs the gate with its own code, so a broken
//!   `main` is still gated and filed. `--here` (the old hand-over: race and run) and the outcome
//!   file beside the gate's log are kept for callers built before the move.
//! - **The notes** (a convention within tracking format v3, no version bump): a helper sub-agent
//!   named `gate`, `sub` `gate-full-<short sha>`; `sub.start` `…; kind=full; commit=<sha>`;
//!   `sub.stop` `rc=<n>[; cause=<cause>]; kind=full; commit=<sha>`. "The last passed full gate"
//!   is the newest such stop with `rc=0`, across every agent's file ([`parse_gates`]).
//! - **A failure** ([`next`]): tests that failed under load, which nextest names, are rerun once,
//!   each alone, with the steps the failure skipped; else the
//!   machine's causes rerun the whole gate once. The machine's cause again, or the gate machine's
//!   setup, files nothing; a real one opens a `main-red` issue naming the failing commit, the last
//!   green one and the merges between ([`red_issue`]), unless one is open already.
//! - **The gate itself** is still each consumer's (phase 3, moves it
//!   here): one run of it is [`gate::full_attempt`], this crate's gate, kind full.
//!
//! The decisions are pure functions over the parsed tracking files (tested); the git, gh and
//! `ci` calls around them are thin.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::claim::{self, Ctx};
use crate::gate::{self, Cause};
use crate::stack::{self, short};
use crate::{Hooks, Project, capacity, event, lock, machine_lock};

pub mod lock_ref;

/// A passed full gate younger than this leaves `main` alone, however far it moved.
pub const MIN_AGE: i64 = 3600;
/// A passed full gate older than this is due again, whether or not `main` moved: the drift
/// interval ([`Hooks::full_gate_drift_secs`]'s default, mornycorp/swarm-core#15).
pub const DRIFT: i64 = 24 * 3600;
/// A full gate's start with no stop this old was abandoned: anyone may take over.
pub const STALE: i64 = 3 * 3600;
/// The full gates' sub-agent ids begin so (a gate's `sub` begins `gate`, as the inspector reads).
pub const SUB: &str = "gate-full-";
/// What the start note says before the capacity and kind.
pub const NOTE: &str = "full gate of main";
/// How many merges and failing tests a `main-red` issue lists.
const LIST_MAX: usize = 30;
/// The line a handed-over full gate ends with: how it ended ([`outcome_line`]), for the caller
/// ([`parse_outcome`]). A stable format: the two sides may be built from different commits.
pub const OUTCOME: &str = "fullgate-outcome:";

/// How many times a full gate that waited for the machine follows `main` on to a newer tip
/// ([`recheck`]) before it gates the one it has.
const MAX_RETARGETS: u32 = 3;

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask fullgate [--check] [--agent <instance>]";

/// The whole usage, for `project`.
fn usage(project: &Project) -> String {
    format!(
        "usage: {USAGE}
  --check   say whether a full gate of main is due, and why, and run nothing
  --agent   your instance id (else {})
  --run <sub> --commit <sha>
            run a full gate already won (the hand-over): only in the full gate's checkout, at
            <sha>, by main's own xtask
  --here    race for it and run it with this xtask's code (the hand-over of callers built
            before Swarm Core's phase 2)",
        project.env_name("AGENT")
    )
}

/// The variable that says whether this machine runs the full gate (`{prefix}_FULLGATE=on|off`),
/// over [`Hooks::full_gate_capable`].
pub fn env_name(project: &Project) -> String {
    project.env_name("FULLGATE")
}

/// How a full gate ended: its `sub.stop` (`rc` none: superseded or abandoned, it never ran).
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    pub t: i64,
    pub rc: Option<i32>,
    pub cause: Option<Cause>,
}

/// One full gate on the tracking branch.
#[derive(Debug, Clone, PartialEq)]
pub struct FullGate {
    pub instance: String,
    pub sub: String,
    pub commit: String,
    pub started: i64,
    pub end: Option<End>,
}

impl FullGate {
    /// It ran and passed.
    fn passed(&self) -> bool {
        self.end.as_ref().is_some_and(|e| e.rc == Some(0))
    }

    /// It ran, passed or failed (not superseded or abandoned).
    fn ran(&self) -> bool {
        self.end.as_ref().is_some_and(|e| e.rc.is_some())
    }

    /// Begun less than [`STALE`] ago and not stopped.
    fn open_at(&self, now: i64) -> bool {
        self.end.is_none() && now.saturating_sub(self.started) < STALE
    }
}

/// The value of `key=` in a note: its fields split at `;` and whitespace (`kind=full (why)` is
/// `full`).
pub fn field<'a>(note: &'a str, key: &str) -> Option<&'a str> {
    note.split(|c: char| c == ';' || c.is_whitespace()).find_map(|f| f.strip_prefix(key)?.strip_prefix('=')).filter(|v| !v.is_empty())
}

/// A commit id as a note names it: hex, at least 7 characters.
fn is_commit(s: &str) -> bool {
    s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The full gates in the tracking files (instance, `work.jsonl` text): every `sub.start` named
/// `gate` whose note says `kind=full` and `commit=<sha>`, with the stop of its `sub` that follows
/// it in its instance's file. And every landed full gate (mornycorp/swarm-core#15): a
/// `sub.stop` with no such start whose note says `rc=0`, `kind=full` and `main=<sha>`, as
/// `land` closes a `--full` gate that left `main` at the tree it tested ([`landed`]): a pass of
/// that commit.
pub fn parse_gates(files: &[(String, String)]) -> Vec<FullGate> {
    let mut gates = Vec::new();
    for (instance, text) in files {
        let first = gates.len();
        for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            let s = |k: &str| v.get(k).and_then(Value::as_str);
            let (Some(event), Some(sub), Some(t)) = (s("event"), s("sub"), s("t").and_then(claim::parse_time)) else { continue };
            let note = s("note").unwrap_or_default();
            match event {
                "sub.start" if s("name") == Some("gate") && field(note, "kind") == Some("full") => {
                    if let Some(commit) = field(note, "commit").filter(|c| is_commit(c)) {
                        gates.push(FullGate { instance: instance.clone(), sub: sub.to_owned(), commit: commit.to_owned(), started: t, end: None });
                    }
                }
                "sub.stop" => {
                    let rc = field(note, "rc").and_then(|r| r.parse().ok());
                    let open = gates.get_mut(first..).and_then(|g| g.iter_mut().rev().find(|g| g.sub == sub && g.end.is_none()));
                    if let Some(g) = open {
                        g.end = Some(End { t, rc, cause: field(note, "cause").and_then(Cause::parse) });
                    } else if let Some(commit) = landed_commit(note, rc) {
                        let end = Some(End { t, rc, cause: None });
                        gates.push(FullGate { instance: instance.clone(), sub: sub.to_owned(), commit: commit.to_owned(), started: t, end });
                    }
                }
                _ => {}
            }
        }
    }
    gates
}

/// The commit a landed full gate's stop credits: its note says `rc=0`, `kind=full` and
/// `main=<sha>`.
fn landed_commit(note: &str, rc: Option<i32>) -> Option<&str> {
    (rc == Some(0) && field(note, "kind") == Some("full")).then(|| field(note, "main").filter(|c| is_commit(c))).flatten()
}

/// What `land` adds to the stop note of a `--full` gate whose merges left `main` at `main`, a
/// commit of exactly the tree the gate tested (mornycorp/swarm-core#15): `main=<sha>`. The
/// stop then reads `rc=0; kind=full; main=<sha>; merged …`, and [`parse_gates`] counts it as a
/// passed full gate of `main`. A key of its own, not `commit=`: to the tracking format's readers
/// a gate whose notes name a `commit` is the periodic one, which merges nothing.
pub fn landed(main: &str) -> String {
    format!("main={main}")
}

/// The newest passed full gate, by when it stopped.
pub fn last_pass(gates: &[FullGate]) -> Option<&FullGate> {
    gates.iter().filter(|g| g.passed()).max_by_key(|g| g.end.as_ref().map_or(0, |e| e.t))
}

/// The newest passed full gate ([`last_pass`]) and when it stopped.
fn last_pass_at(gates: &[FullGate]) -> Option<(&FullGate, i64)> {
    last_pass(gates).and_then(|p| Some((p, p.end.as_ref()?.t)))
}

/// The open full gates at `now`, the earliest first (by start, then instance: every agent orders
/// them the same).
pub fn open(gates: &[FullGate], now: i64) -> Vec<&FullGate> {
    let mut v: Vec<&FullGate> = gates.iter().filter(|g| g.open_at(now)).collect();
    v.sort_by(|a, b| (a.started, &a.instance, &a.sub).cmp(&(b.started, &b.instance, &b.sub)));
    v
}

/// `n` seconds as minutes or hours.
pub fn age(n: i64) -> String {
    if n < 2 * 3600 { format!("{} min", n.max(0) / 60) } else { format!("{:.1} h", n as f64 / 3600.0) }
}

/// Whether a full gate of `main` is due at `now`, and why.
#[derive(Debug, Clone, PartialEq)]
pub enum Due {
    Yes(String),
    No(String),
}

/// A full gate of `commit` that failed in a way that holds it at `now`: for the change's cause
/// (its main-red issue covers it), or the machine's within the hour (no rerun storm on a loaded
/// machine).
fn failure_at<'a>(gates: &'a [FullGate], commit: &str, now: i64) -> Option<(&'a FullGate, &'a End)> {
    gates.iter().filter(|g| g.commit == commit).find_map(|g| {
        let e = g.end.as_ref()?;
        let recent = now.saturating_sub(e.t) < MIN_AGE;
        (e.rc? != 0 && (recent || !e.cause.is_some_and(Cause::transient))).then_some((g, e))
    })
}

/// What makes a full gate due whatever `main` did (mornycorp/swarm-core#15): a pass older
/// than `every` seconds (the drift interval, [`Hooks::full_gate_drift_secs`]), or inputs the
/// gate reads that changed since the last pass, and why (`changed`: a driver, a reference build,
/// pinned tools; [`Hooks::full_gate_inputs_changed`], or `{prefix}_FULLGATE_DRIFT=<why>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Drift {
    pub every: i64,
    pub changed: Option<String>,
}

impl Default for Drift {
    fn default() -> Self {
        Drift { every: DRIFT, changed: None }
    }
}

/// Whether a full gate of `main` (its commit) is due at `now`, from the full gates so far (the
/// periodic gate's and the landed `--full` gates', [`parse_gates`]) and `drift`:
/// - never while one is open, nor for a `main` that failed one since its last pass
///   (`failure_at`);
/// - when `drift` says the gate's inputs changed;
/// - for a `main` unchanged since its last pass, only once that pass is older than the drift
///   interval;
/// - for a `main` that moved since (merges no landed full gate verified: a smoke's, a docs or
///   tooling gate's, a stack landed in part), once that pass is [`MIN_AGE`] old.
pub fn due(gates: &[FullGate], main: &str, now: i64, drift: &Drift) -> Due {
    if let Some(g) = open(gates, now).first() {
        return Due::No(format!("{} is gating {} ({} {} ago)", g.instance, short(&g.commit), g.sub, age(now - g.started)));
    }
    let pass = last_pass_at(gates);
    let since = pass.map_or(i64::MIN, |(_, t)| t);
    if let Some((g, e)) = failure_at(gates, main, now).filter(|(_, e)| e.t > since) {
        let cause = e.cause.map_or("?", Cause::name);
        return Due::No(format!("main ({}) failed its full gate {} ago ({}, cause {cause}): it waits for main to move", short(main), age(now - e.t), g.sub));
    }
    if let Some(why) = &drift.changed {
        return Due::Yes(format!("the full gate's inputs changed: {why}"));
    }
    match pass {
        Some((p, t)) if p.commit == main && now.saturating_sub(t) < drift.every => Due::No(format!(
            "main ({}) is unchanged since its last passed full gate ({}, {} ago; due again after {})",
            short(main),
            p.sub,
            age(now - t),
            age(drift.every)
        )),
        Some((p, t)) if p.commit == main => {
            Due::Yes(format!("main ({}) is unchanged, but its last full gate passed {} ago (over {}): drift", short(main), age(now - t), age(drift.every)))
        }
        Some((p, t)) if now.saturating_sub(t) < MIN_AGE => {
            Due::No(format!("main moved since {}, but its full gate passed only {} ago (under {})", short(&p.commit), age(now - t), age(MIN_AGE)))
        }
        Some((p, t)) => Due::Yes(format!(
            "main ({}) moved since {} ({}, {} ago) by merges no landed full gate verified",
            short(main),
            short(&p.commit),
            p.sub,
            age(now - t)
        )),
        None => Due::Yes(format!("no full gate has passed yet; main is {}", short(main))),
    }
}

/// The drift a full gate of this checkout reads ([`Drift`]): the project's interval, and why its
/// inputs changed since the last pass (`last_pass`, when it stopped), if they did:
/// `{prefix}_FULLGATE_DRIFT=<why>` first, then [`Hooks::full_gate_inputs_changed`].
pub fn drift(ctx: &Ctx<'_>, last_pass: Option<i64>) -> Drift {
    let forced = ctx.project.env("FULLGATE_DRIFT").map(|why| why.trim().to_owned()).filter(|why| !why.is_empty());
    Drift { every: ctx.hooks.full_gate_drift_secs(), changed: forced.or_else(|| ctx.hooks.full_gate_inputs_changed(ctx.root, last_pass)) }
}

/// [`drift`] for `gates`.
fn drift_of(ctx: &Ctx<'_>, gates: &[FullGate]) -> Drift {
    drift(ctx, last_pass_at(gates).map(|(_, t)| t))
}

/// Whether `me`'s full gate `sub` won the race at `now`: the earliest open start wins. `Err` says
/// who did, or that its start isn't on the tracking branch (not sent: it must not run unseen).
pub fn race(gates: &[FullGate], me: &str, sub: &str, now: i64) -> Result<(), String> {
    let open = open(gates, now);
    if !open.iter().any(|g| g.instance == me && g.sub == sub) {
        return Err("its start isn't on the tracking branch (not sent?)".to_owned());
    }
    match open.first() {
        Some(w) if w.instance == me && w.sub == sub => Ok(()),
        Some(w) => Err(format!("{} began a full gate of {} first ({})", w.instance, short(&w.commit), w.sub)),
        None => Err("no open full gate".to_owned()),
    }
}

/// What a full gate that won the race does once it holds the machine lock ([`recheck`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Recheck {
    /// Nothing changed while it waited: it gates the commit it began for.
    Run,
    /// Another gate verified `main` while it waited: it steps aside (a `superseded:` stop), why.
    Verified(String),
    /// `main` moved on to this commit while it waited: it gates that instead.
    Moved(String),
}

/// Read again under the machine lock (mornycorp/swarm-core#36): a full gate begun at `since` for
/// `gated` that waited for the lock, with `main` now at `main` and `gates` read now. A pass of
/// `main` that stopped since it began (another periodic gate's, or a landed `--full` gate's)
/// verified it: step aside. Else a `main` that moved is gated at its new tip; else the gate runs.
pub fn recheck(gates: &[FullGate], since: i64, gated: &str, main: &str) -> Recheck {
    let verified = gates.iter().filter(|g| g.passed() && g.commit == main).find(|g| g.end.as_ref().is_some_and(|e| e.t >= since));
    match verified {
        Some(g) => Recheck::Verified(format!("{}'s {} verified main ({}) while this gate waited for the machine", g.instance, g.sub, short(main))),
        None if main != gated => Recheck::Moved(main.to_owned()),
        None => Recheck::Run,
    }
}

/// The sub-agent id for `me`'s full gate of `commit`: `gate-full-<short>`, numbered when `me`
/// gated that commit before (ids are unique within a session).
pub fn sub_id(gates: &[FullGate], me: &str, commit: &str) -> String {
    let base = format!("{SUB}{}", short(commit));
    let taken = |s: &str| gates.iter().any(|g| g.instance == me && g.sub == s);
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|s| !taken(s)).unwrap_or(base)
}

/// What a full gate's notes end with: `; kind=full; commit=<sha>`.
pub fn tail(commit: &str) -> String {
    format!("; kind=full; commit={commit}")
}

/// A full gate's `sub.start` note: `<what>; capacity=<n> free=<GB>; kind=full; commit=<sha>`,
/// then `extra` (a project's own field, such as `corpus=on (…)`), within the event's note limit
/// (`what` is cut, never the convention).
pub fn start_note(what: &str, more: u64, free_bytes: Option<u64>, commit: &str, extra: Option<&str>) -> String {
    let free = free_bytes.map_or_else(|| "?".to_owned(), |b| format!("{:.1}", b as f64 / f64::from(1u32 << 30)));
    let extra = extra.map(|x| format!("; {x}")).unwrap_or_default();
    let rest = format!("; capacity={more} free={free}{}{extra}", tail(commit));
    let room = event::MAX_NOTE.saturating_sub(rest.chars().count());
    format!("{}{rest}", gate::one_line(what, room))
}

/// What a full gate does after a run that ended `rc` (with `cause`), `reran` once already (its
/// tests or the whole gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Log the pass.
    Pass,
    /// Tests failed under load: rerun them, each alone, and the steps the failure skipped.
    RerunTests,
    /// The machine's cause: run once more.
    Rerun,
    /// The machine's (its rerun too), or the gate machine's setup: nothing is filed.
    Machine,
    /// The change's: open a `main-red` issue.
    File,
}

/// `tests` says the failed tests can be rerun alone: one rerun at most, of the tests when it can,
/// else of the whole gate for the machine's cause.
pub fn next(rc: i32, cause: Option<Cause>, reran: bool, tests: bool) -> Next {
    match cause {
        _ if rc == 0 => Next::Pass,
        _ if tests && !reran => Next::RerunTests,
        Some(c) if c.transient() && !reran => Next::Rerun,
        Some(c) if c.transient() || c.setup() => Next::Machine,
        _ => Next::File,
    }
}

/// What a real failure files.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure<'a> {
    /// The commit that failed, and the last that passed a full gate.
    pub failing: &'a str,
    pub green: Option<&'a str>,
    /// `git log --first-parent --oneline green..failing` on `main`: the merges between.
    pub merges: &'a [String],
    pub cause: Cause,
    pub tests: &'a [String],
    /// The gate (`sub`), whose (`agent`), and where its log is.
    pub sub: &'a str,
    pub agent: &'a str,
    pub log: &'a str,
}

/// `items` as a Markdown list after a field's name, at most [`LIST_MAX`], or `none`.
fn list(items: &[String], none: &str) -> String {
    if items.is_empty() {
        return format!(" {none}\n");
    }
    let mut s: String = items.iter().take(LIST_MAX).map(|m| format!("\n  - {m}")).collect();
    if items.len() > LIST_MAX {
        s.push_str(&format!("\n  - … and {} more", items.len() - LIST_MAX));
    }
    s + "\n"
}

/// The `main-red` issue a real failure opens: its title and description.
pub fn red_issue(f: &Failure<'_>) -> (String, String) {
    let title = format!("main is red: the full gate of {} failed ({})", short(f.failing), f.cause.name());
    let green = f.green.map_or_else(|| "none recorded (no full gate has passed)".to_owned(), |g| format!("`{g}`"));
    let merges = if f.green.is_some() { list(f.merges, "none") } else { " unknown: no green commit to count from\n".to_owned() };
    let description = format!(
        "The periodic full gate of `main` (`cargo xtask fullgate`) failed for the change's cause.\n\n\
         - **Failing commit:** `{}`\n- **Last green commit:** {green}\n- **Merges between them:**{merges}\
         - **Cause:** `{}`\n- **Failing tests:**{}\
         - **The gate:** `{}` of {}; its log: `{}`\n\n\
         While this issue is open nothing merges and no work starts from `main` (docs/swarm-protocol.md, step 3). \
         The gater claims it: fix forward or revert, through the coordinator's gate; the merge closes it.\n",
        f.failing,
        f.cause.name(),
        list(f.tests, "none listed (see the log)"),
        f.sub,
        f.agent,
        f.log
    );
    (title, description)
}

/// Whether this machine runs the full gate: `{prefix}_FULLGATE` (`env`) `on` or `off` says; else
/// the project's rule (`by_default`, [`Hooks::full_gate_capable`]: by default, a
/// Windows machine; a GPU project's, one with a GPU too), asked only then. Room for its build is
/// [`before_claim`]'s to check.
pub fn capable_by(name: &str, env: Option<&str>, by_default: impl FnOnce() -> bool) -> Result<bool, String> {
    match env.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if ["on", "1", "true"].iter().any(|w| v.eq_ignore_ascii_case(w)) => Ok(true),
        Some(v) if ["off", "0", "false"].iter().any(|w| v.eq_ignore_ascii_case(w)) => Ok(false),
        Some(v) => Err(format!("{name}={v}: expected on or off")),
        None => Ok(by_default()),
    }
}

/// [`capable_by`] for this machine.
pub fn capable(project: &Project, hooks: &dyn Hooks) -> Result<bool, String> {
    let name = env_name(project);
    capable_by(&name, std::env::var(&name).ok().as_deref(), || hooks.full_gate_capable())
}

// ---- What a project's gate does for the full gate (phase 2: the gate is still the consumer's). ----

/// What [`Hooks::full_gate_env`] adds to a full gate: a field for its start note, and variables
/// for its `ci`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FullGateEnv {
    pub field: Option<String>,
    pub env: Vec<(String, String)>,
}

/// One run of the full gate's `ci`, which [`gate::full_attempt`] runs with the
/// gate: in `checkout`, into `target`, as `agent`'s sub-agent `sub`, testing `commit`.
#[derive(Debug, Clone, Copy)]
pub struct Attempt<'a> {
    pub project: &'a Project,
    /// The full gate's checkout, at `commit`.
    pub checkout: &'a Path,
    /// Its target dir (`<checkout>/target`); the gate's logs and records go in `gate-logs` there.
    pub target: &'a Path,
    pub agent: &'a str,
    /// The sub this run is logged as: the won gate's, or `<sub>-rerun`.
    pub sub: &'a str,
    pub commit: &'a str,
    /// A rerun's `sub.start` note, which the gate logs (the won gate's own start is logged
    /// already): what it is, before the capacity it then sees ([`start_note`]'s `what`).
    pub start: Option<&'a str>,
    /// Rerun only the failed tests of this earlier run, each alone, with the steps it skipped.
    pub tests_of: Option<&'a str>,
    /// The project's variables for its `ci` ([`FullGateEnv::env`]).
    pub env: &'a [(String, String)],
}

impl Attempt<'_> {
    /// What a failed run's `sub.stop` note ends with: `; kind=full; commit=<sha>` ([`tail`]).
    pub fn stop_tail(&self) -> String {
        tail(self.commit)
    }
}

/// How a run went, as the consumer's gate saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ran {
    pub rc: i32,
    pub cause: Option<Cause>,
    /// When it failed only at its tests, under load: how many the gate can rerun alone (its
    /// records name them); `None`, it can't.
    pub rerun_tests: Option<usize>,
    /// The failed tests' names, for a `main-red` issue.
    pub failed: Vec<String>,
    /// Its log.
    pub log: PathBuf,
}

// ---- I/O: thin wrappers around git, gh and the gate. ----

/// `origin/main`, fetched now.
fn main_commit(project: &Project, root: &Path) -> Result<String, String> {
    let main = project.branches.main;
    event::fetch(root, "origin", &format!("refs/heads/{main}"), &format!("refs/remotes/origin/{main}"))?;
    stack::rev(root, &format!("origin/{main}"))
}

/// The full gates on the tracking branch, fetched now.
fn read_gates(project: &Project, root: &Path) -> Result<Vec<FullGate>, String> {
    Ok(parse_gates(&claim::tracking_files(project, root)?))
}

/// The full gate's worktree ([`Project::full_gate_worktree`] beside the primary checkout), clean
/// and detached at `commit`, whatever the last gate left there (a branch a claim switched it to, a
/// stray file): made the first time, else reset (its ignored `target/` is kept).
pub fn checkout(project: &Project, root: &Path, commit: &str) -> Result<PathBuf, String> {
    let common = event::common_dir(root);
    let primary = common.parent().ok_or_else(|| format!("fullgate: {} has no parent", common.display()))?;
    let wt = primary.join(project.full_gate_worktree);
    let _ = stack::git_slow(root, &["worktree", "prune"], &[]);
    if wt.join(".git").exists() {
        stack::git_slow(&wt, &["checkout", "-q", "--detach", "--force", commit], &[])?;
        stack::git_slow(&wt, &["clean", "-fdq"], &[])?;
    } else {
        let at = wt.to_str().ok_or_else(|| format!("fullgate: {} is not UTF-8", wt.display()))?;
        stack::git_slow(root, &["worktree", "add", "-q", "--detach", at, commit], &[])?;
    }
    let head = stack::rev(&wt, "HEAD")?;
    if head != commit {
        return Err(format!("fullgate: {} is at {}, not {}", wt.display(), short(&head), short(commit)));
    }
    Ok(wt)
}

/// How a full gate ended, for whoever asked for it.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    NotDue(String),
    /// Another agent's full gate won the race, or this one's start wasn't seen.
    Yielded(String),
    Passed(String),
    /// Failed for the machine's cause, or its setup's: nothing filed.
    Machine(Cause),
    /// Failed for the change's cause: the `main-red` issue opened, if this agent filed it (`None`:
    /// one was open already). The caller claims it in its own worktree ([`hand_over`]).
    Red(Option<u64>),
}

/// [`OUTCOME`]'s line for `o`: `not-due`, `yielded`, `passed <commit>`, `machine <cause>`,
/// `red <issue>` or `red open`.
pub fn outcome_line(o: &Outcome) -> String {
    let what = match o {
        Outcome::NotDue(_) => "not-due".to_owned(),
        Outcome::Yielded(_) => "yielded".to_owned(),
        Outcome::Passed(c) => format!("passed {c}"),
        Outcome::Machine(c) => format!("machine {}", c.name()),
        Outcome::Red(Some(n)) => format!("red {n}"),
        Outcome::Red(None) => "red open".to_owned(),
    };
    format!("{OUTCOME} {what}")
}

/// The outcome a handed-over full gate printed ([`outcome_line`]): its last [`OUTCOME`] line.
/// `None` when it printed none, or one this build doesn't know.
pub fn parse_outcome(output: &str) -> Option<Outcome> {
    let line = output.lines().rev().find_map(|l| l.trim().strip_prefix(OUTCOME))?.trim();
    let (what, rest) = line.split_once(' ').unwrap_or((line, ""));
    let handed = |why: &str| format!("{why} (main's own fullgate)");
    match what {
        "not-due" => Some(Outcome::NotDue(handed("not due"))),
        "yielded" => Some(Outcome::Yielded(handed("another agent's"))),
        "passed" if is_commit(rest) => Some(Outcome::Passed(rest.to_owned())),
        "machine" => Some(Outcome::Machine(Cause::parse(rest).unwrap_or(Cause::Other))),
        "red" if rest == "open" => Some(Outcome::Red(None)),
        "red" => rest.parse().ok().map(|n| Outcome::Red(Some(n))),
        _ => None,
    }
}

/// Where a won gate `sub` also leaves its outcome, beside its log: what a caller built before the
/// move reads.
fn outcome_file(target: &Path, sub: &str) -> PathBuf {
    target.join(gate::LOG_DIR).join(format!("{sub}.outcome"))
}

/// The outcome file's line ([`outcome_file`]): `passed <c>`, `machine <cause>`, `red <n>`, `red`.
pub fn legacy_outcome_line(o: &Outcome) -> String {
    match o {
        Outcome::NotDue(why) => format!("not-due {why}"),
        Outcome::Yielded(why) => format!("yielded {why}"),
        Outcome::Passed(c) => format!("passed {c}"),
        Outcome::Machine(c) => format!("machine {}", c.name()),
        Outcome::Red(Some(n)) => format!("red {n}"),
        Outcome::Red(None) => "red".to_owned(),
    }
}

/// An outcome file's line ([`legacy_outcome_line`]): passed, failed for the machine's cause, or red.
pub fn parse_legacy_outcome(line: &str) -> Option<Outcome> {
    let line = line.trim();
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    match word {
        "passed" if is_commit(rest) => Some(Outcome::Passed(rest.to_owned())),
        "machine" => Cause::parse(rest).map(Outcome::Machine),
        "red" if rest.is_empty() => Some(Outcome::Red(None)),
        "red" => rest.parse().ok().map(|n| Outcome::Red(Some(n))),
        _ => None,
    }
}

/// The program, arguments and environment that run the won gate `sub` of `commit` with the code
/// of the checkout `wt` (`main`'s own xtask, built in its own target dir), as `agent`.
pub fn handover_command(wt: &Path, sub: &str, commit: &str, agent: &str) -> (String, Vec<String>, Vec<(&'static str, PathBuf)>) {
    let args = ["run", "-q", "-p", "xtask", "--", "fullgate", "--run", sub, "--commit", commit, "--agent", agent].map(str::to_owned).to_vec();
    (std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()), args, vec![("CARGO_TARGET_DIR", wt.join("target"))])
}

/// The full gate of `main` as `agent`, when it is due ([`due`]) and this agent wins the race
/// ([`race`]) and the lock ([`lock_ref::take`]): the start logged, the race checked, the lock
/// taken, then, holding the machine lock, read again ([`recheck`]): it steps aside for a `main`
/// another gate verified while it waited, or begins again for a `main` that moved; then it is run
/// by `main`'s own code ([`hand_over`]), or, with `here` (`--here`), by this xtask's.
pub fn take_if_due(ctx: &Ctx<'_>, agent: &str, here: bool) -> Result<Outcome, String> {
    let (project, root) = (ctx.project, ctx.root);
    let mut main = main_commit(project, root)?;
    // Held from the first recheck to the run: a retarget doesn't let another gate in between.
    let mut machine: Option<machine_lock::Guard> = None;
    // A `main` that keeps moving is retargeted a few times, then gated where it is.
    for retargets in 0.. {
        let gates = read_gates(project, root)?;
        if let Due::No(why) = due(&gates, &main, claim::now(), &drift_of(ctx, &gates)) {
            return Ok(Outcome::NotDue(why));
        }
        let sub = sub_id(&gates, agent, &main);
        let tail = tail(&main);
        let extra = ctx.hooks.full_gate_env(&sub, gates.iter().filter(|g| g.ran()).count());
        let (_, more, free) = gate::room(root, &capacity_target(root));
        let note = start_note(NOTE, more, free, &main, extra.field.as_deref());
        let since = claim::now();
        gate::log_event(project, root, &["sub.start", "--agent", agent, "--sub", &sub, "--name", "gate", "--note", &note]);
        let won = read_gates(project, root).and_then(|g| {
            race(&g, agent, &sub, claim::now())?;
            lock_ref::take(root, project.branches.remote, &g, agent, &sub, &main)
        });
        if let Err(why) = won {
            gate::log_event(project, root, &["sub.stop", "--agent", agent, "--sub", &sub, "--note", &format!("superseded: {why}{tail}")]);
            return Ok(Outcome::Yielded(why));
        }
        let rechecked = (|| {
            if machine.is_none() {
                machine = Some(machine_lock::take(project, agent, root, true)?);
            }
            let now_main = main_commit(project, root)?;
            Ok::<_, String>(recheck(&read_gates(project, root)?, since, &main, &now_main))
        })();
        match rechecked {
            Err(e) => {
                let why = gate::one_line(&event::one_line(&e), 120);
                gate::log_event(project, root, &["sub.stop", "--agent", agent, "--sub", &sub, "--note", &format!("abandoned: {why}{tail}")]);
                return Err(e);
            }
            Ok(Recheck::Verified(why)) => {
                gate::log_event(project, root, &["sub.stop", "--agent", agent, "--sub", &sub, "--note", &format!("superseded: {why}{tail}")]);
                return Ok(Outcome::Yielded(why));
            }
            Ok(Recheck::Moved(to)) if retargets < MAX_RETARGETS => {
                let why = format!("main moved to {} while this gate waited for the machine: gating it instead", short(&to));
                gate::log_event(project, root, &["sub.stop", "--agent", agent, "--sub", &sub, "--note", &format!("superseded: {why}{tail}")]);
                println!("fullgate: {why}");
                main = to;
                continue;
            }
            Ok(_) => {}
        }
        println!("fullgate: {sub} won the race for {}: the whole ci", short(&main));
        let runs = |a: &Attempt<'_>| gate::full_attempt(project, ctx.hooks, a);
        let won = Won { ctx: *ctx, agent, sub: &sub, main: &main, gates: &gates, env: &extra.env, runs: &runs };
        let ran = if here { checkout(project, root, &main).and_then(|wt| run_won(&won, &wt, &|a| gate::log_event(project, root, a))) } else { hand_over(&won) };
        return ran.inspect_err(|e| {
            let why = gate::one_line(&event::one_line(e), 120);
            gate::log_event(project, root, &["sub.stop", "--agent", agent, "--sub", &sub, "--note", &format!("abandoned: {why}{tail}")]);
        });
    }
    Err("fullgate: no gate ran".to_owned())
}

/// The target dir capacity is read for before the checkout exists: this checkout's.
fn capacity_target(root: &Path) -> PathBuf {
    capacity::our_target(root).unwrap_or_else(|_| root.join("target"))
}

/// One run of the full gate's `ci`: the gate's ([`gate::full_attempt`]), or a test's fake.
type Runs<'a> = &'a dyn Fn(&Attempt<'_>) -> Result<Ran, String>;

/// A full gate this agent won: whose (`ctx`, its caller's worktree), as which sub-agent, the commit
/// of `main` it tests, the full gates as read before it began, the project's variables, and what
/// runs each attempt.
struct Won<'a> {
    ctx: Ctx<'a>,
    agent: &'a str,
    sub: &'a str,
    main: &'a str,
    gates: &'a [FullGate],
    env: &'a [(String, String)],
    runs: Runs<'a>,
}

/// The won full gate, from its caller: check `main` out, run `main`'s own xtask there to run the
/// gate ([`handover_command`], `--run`), its output passed through, and read its outcome back;
/// claim a `main-red` issue it filed here, in the caller's worktree. When it leaves no outcome
/// (`main`'s xtask doesn't build, say), this xtask runs the gate itself.
fn hand_over(won: &Won<'_>) -> Result<Outcome, String> {
    let (project, root) = (won.ctx.project, won.ctx.root);
    let wt = checkout(project, root, won.main)?;
    let (program, args, env) = handover_command(&wt, won.sub, won.main, won.agent);
    let _ = std::fs::remove_file(outcome_file(&wt.join("target"), won.sub));
    println!("fullgate: running main {}'s own `cargo xtask fullgate` in {}", short(won.main), wt.display());
    // The checkout's own root and target dir, not the caller's (`cargo xtask` set both).
    let mut cmd = Command::new(&program);
    cmd.args(&args).envs(won.env.iter().map(|(k, v)| (k, v))).envs(env).env_remove("CARGO_MANIFEST_DIR").current_dir(&wt).stdout(Stdio::piped());
    let (status, output) = passed_through(cmd).map_err(|e| format!("fullgate: `{program} {}`: {e}", args.join(" ")))?;
    let legacy = || std::fs::read_to_string(outcome_file(&wt.join("target"), won.sub)).ok().and_then(|l| parse_legacy_outcome(&l));
    let outcome = match parse_outcome(&output).or_else(legacy) {
        Some(o) => o,
        None => {
            eprintln!("warning: fullgate: main's own fullgate ended ({status}) without an outcome; running it with this xtask's code");
            run_won(won, &wt, &|a| gate::log_event(project, root, a))?
        }
    };
    if let Outcome::Red(Some(n)) = outcome {
        claim_red(&won.ctx, won.agent, n);
    }
    Ok(outcome)
}

/// Run `cmd`, its stdout piped, each line passed on as it comes: how it ended, and what it said.
fn passed_through(mut cmd: Command) -> Result<(std::process::ExitStatus, String), String> {
    use std::io::BufRead;
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut output = String::new();
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            println!("{line}");
            output.push_str(&line);
            output.push('\n');
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok((status, output))
}

/// Claim the `main-red` issue `n` the full gate filed, in the caller's worktree: the
/// shared checkout stays detached at the commit it gates.
fn claim_red(ctx: &Ctx<'_>, agent: &str, n: u64) {
    if let Err(e) = claim::run(ctx.project, ctx.hooks, ctx.root, &[&n.to_string(), "--agent", agent, "--no-fullgate"]) {
        eprintln!("warning: fullgate: claiming #{n}: {e}");
    }
}

/// `fullgate --run` (the hand-over): the won gate `sub` of `main`, run by `main`'s own xtask in the
/// full gate's checkout (`ctx.root`, which must be at `main`); its outcome printed for the caller
/// ([`OUTCOME`]) and left beside its log for a caller built before the move.
fn run_here(ctx: &Ctx<'_>, agent: &str, sub: &str, main: &str) -> Result<Outcome, String> {
    let (project, root) = (ctx.project, ctx.root);
    let head = stack::rev(root, "HEAD")?;
    if head != main {
        return Err(format!(
            "fullgate --run: {} is at {}, not {}: only the full gate's checkout, at the commit it gates",
            root.display(),
            short(&head),
            short(main)
        ));
    }
    let gates = read_gates(project, root)?;
    let extra = ctx.hooks.full_gate_env(sub, gates.iter().filter(|g| g.ran()).count());
    let runs = |a: &Attempt<'_>| gate::full_attempt(project, ctx.hooks, a);
    let won = Won { ctx: *ctx, agent, sub, main, gates: &gates, env: &extra.env, runs: &runs };
    let outcome = run_won(&won, root, &|a| gate::log_event(project, root, a))?;
    let file = outcome_file(&root.join("target"), sub);
    if let Err(e) = std::fs::write(&file, legacy_outcome_line(&outcome)) {
        eprintln!("warning: fullgate: {}: {e}", file.display());
    }
    Ok(outcome)
}

/// The won full gate, in its checkout `wt`: the run (and a rerun: of the tests that failed under
/// load, else of the whole gate for the machine's cause), by the gate ([`gate::full_attempt`]),
/// then the pass logged (`log`) or the failure filed.
fn run_won(won: &Won<'_>, wt: &Path, log: &dyn Fn(&[&str])) -> Result<Outcome, String> {
    let Won { ctx, agent, sub, main, env, .. } = *won;
    let target = wt.join("target");
    let (mut at, mut reran) = (sub.to_owned(), false);
    let (mut start, mut tests_of): (Option<String>, Option<String>) = (None, None);
    let mut rerun_tests = None;
    loop {
        let attempt = Attempt {
            project: ctx.project,
            checkout: wt,
            target: &target,
            agent,
            sub: &at,
            commit: main,
            start: start.as_deref(),
            tests_of: tests_of.as_deref(),
            env,
        };
        let ran = (won.runs)(&attempt)?;
        match next(ran.rc, ran.cause, reran, !reran && ran.rerun_tests.is_some()) {
            Next::Pass => {
                let how = rerun_tests.map(|n| format!("; reran {n} failed tests alone")).unwrap_or_default();
                log(&["sub.stop", "--agent", agent, "--sub", &at, "--note", &format!("rc=0{how}{}", tail(main))]);
                println!("fullgate: {at} passed: main {} is green", short(main));
                return Ok(Outcome::Passed(main.to_owned()));
            }
            Next::RerunTests => {
                rerun_tests = ran.rerun_tests;
                start = Some(format!("rerun of {at}: its failed tests alone ({}); {NOTE}", ran.rerun_tests.unwrap_or(0)));
                tests_of = Some(at.clone());
            }
            Next::Rerun => start = Some(format!("rerun of {sub}: {NOTE}")),
            Next::Machine => return Ok(Outcome::Machine(ran.cause.unwrap_or(Cause::Other))),
            Next::File => return file_red(won, wt, &ran).map(Outcome::Red),
        }
        at = format!("{sub}-rerun");
        reran = true;
    }
}

/// `git log --first-parent --oneline green..failing` at `root`: the merges between.
fn merges_between(root: &Path, green: &str, failing: &str) -> Vec<String> {
    let range = format!("{green}..{failing}");
    crate::process::git(root, &["log", "--first-parent", "--format=%h %s", &range], None, &[])
        .map(|s| s.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// A real failure of the won full gate, its last run `ran`: a `main-red` issue opened, unless one
/// is open already. The issue this agent filed, which the caller claims ([`hand_over`]).
fn file_red(won: &Won<'_>, wt: &Path, ran: &Ran) -> Result<Option<u64>, String> {
    let Won { ctx, agent, sub, main, gates, .. } = *won;
    let project = ctx.project;
    let open = claim::main_red(project)?;
    if !open.is_empty() {
        let list: Vec<String> = open.iter().map(|(n, t)| format!("#{n} {t}")).collect();
        println!("fullgate: main is already red ({}): not filing another", list.join("; "));
        return Ok(None);
    }
    let green = last_pass(gates).map(|g| g.commit.as_str());
    let merges = green.map(|g| merges_between(wt, g, main)).unwrap_or_default();
    let log_s = ran.log.display().to_string();
    let f = Failure { failing: main, green, merges: &merges, cause: ran.cause.unwrap_or(Cause::Other), tests: &ran.failed, sub, agent, log: &log_s };
    let (title, description) = red_issue(&f);
    let number = crate::github::create_issue(project, &title, &description, &[project.labels.main_red, project.labels.bug])?;
    println!("fullgate: filed #{number}: {title}");
    Ok(Some(number))
}

/// `claim`'s full gate: on a machine that can, with room for a build now, the full gate of `main`
/// runs when it is due, while the claim holds its issue. `Some` is how it ended, for the claim to
/// decide ([`claim::after_gate`]: a `main-red` result releases it); `None`, it didn't run (the
/// machine can't, or `main` is known red). An error running it is a warning, and the claim goes
/// on.
pub fn before_claim(ctx: &Ctx<'_>, agent: &str) -> Option<Outcome> {
    match capable(ctx.project, ctx.hooks) {
        Ok(true) => {}
        Ok(false) => return None,
        Err(e) => {
            eprintln!("warning: fullgate: {e}");
            return None;
        }
    }
    // A red main is known red: its fix's claim, and every other, needs no full gate.
    if claim::main_red(ctx.project).is_ok_and(|red| !red.is_empty()) {
        return None;
    }
    // Only a machine with room for its build starts one; another agent's claim will.
    if !gate::room(ctx.root, &capacity_target(ctx.root)).0 {
        println!("fullgate: no room for a build now: not this claim's to run");
        return None;
    }
    match take_if_due(ctx, agent, false) {
        Ok(o) => {
            println!("fullgate: {}", describe(&o));
            Some(o)
        }
        Err(e) => {
            eprintln!("warning: fullgate: {e}; the claim goes on without it");
            None
        }
    }
}

/// One line for an outcome.
pub fn describe(o: &Outcome) -> String {
    match o {
        Outcome::NotDue(why) => format!("not due: {why}"),
        Outcome::Yielded(why) => format!("not this agent's: {why}"),
        Outcome::Passed(c) => format!("passed: main {} is green", short(c)),
        Outcome::Machine(c) => format!("failed for the machine's cause ({}): nothing filed; it is due again in {}", c.name(), age(MIN_AGE)),
        Outcome::Red(Some(n)) => format!("failed: filed main-red #{n}"),
        Outcome::Red(None) => "failed: main-red is open already".to_owned(),
    }
}

/// `fullgate --check`'s report: main, the last pass, any open start, and whether one is due.
pub fn report(gates: &[FullGate], main: &str, now: i64, drift: &Drift) -> String {
    let mut s = format!("main: {}\n", short(main));
    match last_pass_at(gates) {
        Some((p, t)) => {
            let moved = if p.commit == main { "no" } else { "yes" };
            s += &format!("last passed full gate: {} ({} of {}), {} ago; main moved since: {moved}\n", short(&p.commit), p.sub, p.instance, age(now - t));
        }
        None => s += "last passed full gate: none recorded\n",
    }
    for g in open(gates, now) {
        s += &format!("open: {} of {} gating {}, {} ago\n", g.sub, g.instance, short(&g.commit), age(now - g.started));
    }
    for g in gates.iter().filter(|g| g.end.is_none() && !g.open_at(now)) {
        s += &format!("abandoned: {} of {} ({} ago, over {})\n", g.sub, g.instance, age(now - g.started), age(STALE));
    }
    s += &format!("drift: due again {} after a pass, whatever main did\n", age(drift.every));
    match due(gates, main, now, drift) {
        Due::Yes(why) => s += &format!("due: yes ({why})\n"),
        Due::No(why) => s += &format!("due: no ({why})\n"),
    }
    s
}

/// `fullgate`'s options ([`usage`]).
#[derive(Debug, Default, PartialEq, Eq)]
struct Opts {
    check: bool,
    agent: Option<String>,
    /// `--run <sub>` with `--commit <sha>`: the hand-over.
    run: Option<(String, String)>,
    /// `--here`: race and run with this xtask's code.
    here: bool,
}

fn parse_args(project: &Project, args: &[&str]) -> Result<Opts, String> {
    let (mut o, mut sub, mut commit) = (Opts::default(), None, None);
    let mut it = args.iter();
    let value = |flag: &str, v: Option<&&str>| v.map(|v| (*v).to_owned()).ok_or_else(|| format!("fullgate: {flag} needs a value\n{}", usage(project)));
    while let Some(&a) = it.next() {
        match a {
            "--check" => o.check = true,
            "--here" => o.here = true,
            "--agent" => o.agent = Some(value(a, it.next())?),
            "--run" => sub = Some(value(a, it.next())?),
            "--commit" => commit = Some(value(a, it.next())?),
            other => return Err(format!("fullgate: unknown argument `{other}`\n{}", usage(project))),
        }
    }
    o.run = match (sub, commit) {
        (Some(s), Some(c)) if s.starts_with(SUB) && is_commit(&c) => Some((s, c)),
        (None, None) => None,
        _ => return Err(format!("fullgate: --run takes a full gate's sub ({SUB}…) and --commit its full sha\n{}", usage(project))),
    };
    Ok(o)
}

/// `cargo xtask fullgate …` ([`USAGE`]) for `project`.
pub fn run(project: &Project, hooks: &dyn Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    let Opts { check, agent, run, here } = parse_args(project, args)?;
    let ctx = Ctx { project, hooks, root };
    if let Some((sub, commit)) = run {
        let agent = lock::agent(project, agent, "fullgate")?;
        let outcome = run_here(&ctx, &agent, &sub, &commit)?;
        println!("fullgate: {}", describe(&outcome));
        println!("{}", outcome_line(&outcome));
        return Ok(());
    }
    let can = capable(project, hooks);
    if check {
        let main = main_commit(project, root)?;
        let gates = read_gates(project, root)?;
        print!("{}", report(&gates, &main, claim::now(), &drift_of(&ctx, &gates)));
        match can {
            Ok(c) => println!("this machine: {}", if c { "runs it".to_owned() } else { format!("doesn't run it ({}=on to)", env_name(project)) }),
            Err(e) => println!("this machine: {e}"),
        }
        return Ok(());
    }
    if !can? {
        return Err(format!("fullgate: this machine doesn't run the full gate; {}=on runs it anyway", env_name(project)));
    }
    let agent = lock::agent(project, agent, "fullgate")?;
    let outcome = take_if_due(&ctx, &agent, here)?;
    println!("fullgate: {}", describe(&outcome));
    if here {
        println!("{}", outcome_line(&outcome));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::{APP_SWARM as P, BOTH};
    use std::cell::RefCell;

    const T0: i64 = 1_790_000_000;
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn stopped(instance: &str, sub: &str, commit: &str, started: i64, stopped: i64, rc: Option<i32>) -> FullGate {
        FullGate { instance: instance.into(), sub: sub.into(), commit: commit.into(), started, end: Some(End { t: stopped, rc, cause: None }) }
    }

    /// mornycorp/swarm-core#36: a full gate that waited for the machine lock reads again. Nothing
    /// changed: it runs. Another gate passed `main` meanwhile: it steps aside. `main` moved: it
    /// gates the new tip, unless that was verified meanwhile too.
    #[test]
    fn a_gate_that_waited_for_the_machine_rechecks_main() {
        let since = T0;
        let mine = FullGate { instance: "w-2".into(), sub: format!("{SUB}aaaaaaa"), commit: A.into(), started: since, end: None };
        assert_eq!(recheck(std::slice::from_ref(&mine), since, A, A), Recheck::Run, "nothing changed");
        // A pass of A that stopped before this gate began doesn't count: due() already weighed it.
        let old = stopped("w-1", "gate-full-aaaaaaa", A, since - 7200, since - 3600, Some(0));
        assert_eq!(recheck(&[old, mine.clone()], since, A, A), Recheck::Run);
        // A landed `--full` gate left main at A while this one waited.
        let landed = stopped("coord", "gate-37", A, since + 60, since + 600, Some(0));
        match recheck(&[mine.clone(), landed.clone()], since, A, A) {
            Recheck::Verified(why) => assert!(why.contains("coord's gate-37 verified main (aaaaaaa)"), "{why}"),
            other => panic!("{other:?}"),
        }
        // A failure, or a stop that never ran, verifies nothing.
        for rc in [Some(1), None] {
            let failed = stopped("w-1", "gate-full-aaaaaaa-2", A, since + 60, since + 600, rc);
            assert_eq!(recheck(&[mine.clone(), failed], since, A, A), Recheck::Run, "{rc:?}");
        }
        assert_eq!(recheck(std::slice::from_ref(&mine), since, A, B), Recheck::Moved(B.into()), "main moved on");
        let b_landed = stopped("coord", "gate-38", B, since + 60, since + 600, Some(0));
        assert!(matches!(recheck(&[mine, b_landed], since, A, B), Recheck::Verified(_)), "and the new tip was verified too");
    }

    /// The caller hands the won gate to `main`'s own xtask, built in the checkout, with the sub,
    /// the commit and the agent; that xtask takes them back as `--run`.
    #[test]
    fn the_caller_hands_the_won_gate_to_mains_xtask() {
        let sub = format!("{SUB}aaaaaaa");
        let wt = Path::new("W");
        let (_, args, env) = handover_command(wt, &sub, A, "w-1");
        assert_eq!(args, ["run", "-q", "-p", "xtask", "--", "fullgate", "--run", sub.as_str(), "--commit", A, "--agent", "w-1"]);
        assert_eq!(env, [("CARGO_TARGET_DIR", wt.join("target"))], "the checkout's own target dir");
        let rest: Vec<&str> = args[6..].iter().map(String::as_str).collect();
        let o = parse_args(&P, &rest).unwrap();
        assert_eq!(o, Opts { check: false, agent: Some("w-1".into()), run: Some((sub.clone(), A.into())), here: false });
        assert_eq!(parse_args(&P, &["--check", "--agent", "w-1"]).unwrap(), Opts { check: true, agent: Some("w-1".into()), ..Opts::default() });
        assert!(parse_args(&P, &["--here", "--agent", "w-1"]).unwrap().here, "a caller built before the move hands over so");
        for (bad, why) in [
            (vec!["--run", sub.as_str()], "--commit"),
            (vec!["--commit", A], "--commit"),
            (vec!["--run", "gate-9", "--commit", A], SUB),
            (vec!["--run", sub.as_str(), "--commit", "main"], "sha"),
            (vec!["--run"], "needs a value"),
            (vec!["--fast"], "unknown"),
        ] {
            assert!(parse_args(&P, &bad).unwrap_err().contains(why), "{bad:?}");
        }
    }

    /// `--run` runs only in a checkout at the commit it gates: anywhere else it refuses before it
    /// logs or builds anything.
    #[test]
    fn a_hand_over_runs_only_at_the_commit_it_gates() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-fullgate-run-here");
        swarm_core_testkit::git::git(&dir, &["init", "--quiet"]);
        swarm_core_testkit::git::git(&dir, &["commit", "--quiet", "--allow-empty", "-m", "a"]);
        let ctx = Ctx { project: &P, hooks: &crate::Defaults, root: &dir };
        let e = run_here(&ctx, "w-1", &format!("{SUB}aaaaaaa"), A).unwrap_err();
        assert!(e.contains("only the full gate's checkout"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shared checkout is the full gate's, detached at the commit it gates, whatever the last
    /// gate left there: a branch a claim switched it to, a stray file.
    #[test]
    fn the_full_gates_checkout_stays_detached_at_the_commit_it_gates() {
        use crate::stack::testgit::{self, tgit};
        let dir = swarm_core_testkit::temp_dir("swarm-core-fullgate-checkout");
        let (origin, primary) = (dir.join("origin.git"), dir.join("primary"));
        testgit::bare(&origin);
        let mut h = testgit::History::default();
        let a = h.commit("main", &[], &[("a.txt", "a")], "a");
        h.commit("main", &[&a], &[("a.txt", "b")], "b");
        let ids = h.import(&origin);
        testgit::clone(&origin, &primary);
        let detached_at = |wt: &Path, commit: &str| {
            assert_eq!(stack::rev(wt, "HEAD").unwrap(), commit);
            assert!(crate::process::git(wt, &["symbolic-ref", "-q", "HEAD"], None, &[]).is_err(), "{} is on a branch", wt.display());
        };
        let wt = checkout(&P, &primary, &ids[0]).unwrap();
        assert_eq!(wt, primary.join(P.full_gate_worktree));
        detached_at(&wt, &ids[0]);
        tgit(&wt, &["switch", "-q", "-c", "245-main-red"]);
        std::fs::write(wt.join("stray.txt"), "x").unwrap();
        assert_eq!(checkout(&P, &primary, &ids[1]).unwrap(), wt);
        detached_at(&wt, &ids[1]);
        assert!(!wt.join("stray.txt").exists(), "a stray file is cleaned");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What `main`'s xtask prints for the caller reads back as the outcome it was, from the last
    /// such line among its output; so does the file a caller built before the move reads.
    #[test]
    fn a_won_gates_outcome_reads_back() {
        let all = [Outcome::Passed(A.into()), Outcome::Machine(Cause::Link), Outcome::Machine(Cause::Python), Outcome::Red(Some(229)), Outcome::Red(None)];
        for o in &all {
            assert_eq!(parse_outcome(&outcome_line(o)), Some(o.clone()), "{o:?}");
            assert_eq!(parse_legacy_outcome(&legacy_outcome_line(o)), Some(o.clone()), "{o:?}");
        }
        let output = format!("gate …\nfullgate: passed\n{}\n", outcome_line(&Outcome::Red(Some(7))));
        assert_eq!(parse_outcome(&format!("{}\n{output}", outcome_line(&Outcome::Passed(A.into())))), Some(Outcome::Red(Some(7))), "the last");
        assert!(matches!(parse_outcome(&outcome_line(&Outcome::NotDue("x".into()))), Some(Outcome::NotDue(_))));
        assert!(matches!(parse_outcome(&outcome_line(&Outcome::Yielded("x".into()))), Some(Outcome::Yielded(_))));
        for bad in ["", "passed x", "fullgate-outcome: passed main", "fullgate-outcome: red #1", "fullgate-outcome: lunch"] {
            assert_eq!(parse_outcome(bad), None, "{bad}");
        }
        assert_eq!(parse_legacy_outcome(&format!("passed {A}\n")), Some(Outcome::Passed(A.into())), "a trailing newline");
        for bad in ["", "passed", "passed main", "machine lunch", "red #1", "not-due x", "yielded y"] {
            assert_eq!(parse_legacy_outcome(bad), None, "{bad}");
        }
    }

    /// A tracking line at `T0 + secs`.
    fn line(event: &str, sub: &str, secs: i64, note: &str) -> String {
        let t = crate::time::iso_utc(u64::try_from(T0 + secs).unwrap());
        let name = if event == "sub.start" { r#""name":"gate","# } else { "" };
        format!(r#"{{"event":"{event}",{name}"note":"{note}","sub":"{sub}","t":"{t}"}}"#)
    }

    fn start(sub: &str, secs: i64, commit: &str) -> String {
        line("sub.start", sub, secs, &start_note(NOTE, 1, Some(9 << 30), commit, None))
    }

    fn stop(sub: &str, secs: i64, result: &str, commit: &str) -> String {
        line("sub.stop", sub, secs, &format!("{result}{}", tail(commit)))
    }

    fn files(f: &[(&str, &[String])]) -> Vec<(String, String)> {
        f.iter().map(|(i, lines)| ((*i).to_owned(), lines.join("\n"))).collect()
    }

    #[test]
    fn the_notes_are_read_as_written() {
        assert_eq!(field("rc=0; kind=full; commit=abc1234", "commit"), Some("abc1234"));
        assert_eq!(field("local gate: #1; kind=full; capacity=1 free=2.0", "kind"), Some("full"));
        assert_eq!(field("rc=101; cause=test", "cause"), Some("test"));
        assert_eq!(field("kind=", "kind"), None);
        assert_eq!(field("rc=0", "commit"), None);
        assert_eq!(start_note(NOTE, 1, Some(9 << 30), A, None), format!("full gate of main; capacity=1 free=9.0; kind=full; commit={A}"));
        // A project's corpus field comes last; a long `what` is cut, never the convention.
        let corpus = "corpus=on (the reference tree and its replayer)";
        assert!(start_note(NOTE, 1, None, A, Some(corpus)).ends_with(&format!("free=?; kind=full; commit={A}; {corpus}")));
        let long = start_note(&"x".repeat(500), 1, None, A, Some(corpus));
        assert_eq!(long.chars().count(), event::MAX_NOTE);
        assert_eq!(field(&long, "commit"), Some(A));
        let failed = format!("rc=101; cause=test{}", tail(B));
        let tracked = files(&[(
            "w-1",
            &[
                start("gate-full-aaaaaaa", 0, A),
                line("work", "", 5, "x"),
                stop("gate-full-aaaaaaa", 900, "rc=0", A),
                // The coordinator's own `--full` gate names no commit: not a full gate of main.
                line("sub.start", "gate-120", 1000, "local gate: #5; kind=full; capacity=1 free=9.0"),
                line("sub.stop", "gate-120", 1500, "rc=0; kind=full; merged #5"),
                start("gate-full-bbbbbbb", 2000, B),
                line("sub.stop", "gate-full-bbbbbbb", 2600, &failed),
                start("gate-full-bbbbbbb-2", 3000, B),
                stop("gate-full-bbbbbbb-2", 3001, "superseded: x began a full gate of bbbbbbb first", B),
            ],
        )]);
        let gates = parse_gates(&tracked);
        assert_eq!(gates.len(), 3, "{gates:#?}");
        assert_eq!(gates[0].end, Some(End { t: T0 + 900, rc: Some(0), cause: None }));
        assert_eq!(gates[1].end, Some(End { t: T0 + 2600, rc: Some(101), cause: Some(Cause::Test) }));
        assert_eq!(gates[2].end.as_ref().unwrap().rc, None, "superseded: never ran");
        assert_eq!(gates.iter().filter(|g| g.ran()).count(), 2);
        assert_eq!(last_pass(&gates).unwrap().commit, A);
    }

    /// The project's defaults: a day's drift, no inputs changed.
    const D: Drift = Drift { every: DRIFT, changed: None };

    /// An unchanged `main` isn't gated again within the drift interval, however long; past it, it
    /// is, once (#15). A moved one is due once its pass is an hour old.
    #[test]
    fn an_unchanged_main_is_never_gated_twice_within_the_drift_interval() {
        let gates = parse_gates(&files(&[("w-1", &[start("gate-full-aaaaaaa", 0, A), stop("gate-full-aaaaaaa", 900, "rc=0", A)])]));
        for later in [3600, 10 * 3600, 900 + DRIFT - 1] {
            assert!(matches!(due(&gates, A, T0 + later, &D), Due::No(w) if w.contains("unchanged")), "{later}");
        }
        assert!(matches!(due(&gates, A, T0 + 900 + DRIFT, &D), Due::Yes(w) if w.contains("drift")));
        let hourly = Drift { every: 3600, changed: None };
        assert!(matches!(due(&gates, A, T0 + 900 + 3600, &hourly), Due::Yes(_)), "the project's own interval");
        // The drift gate passed: unchanged again for a day.
        let again = parse_gates(&files(&[(
            "w-1",
            &[
                start("gate-full-aaaaaaa", 0, A),
                stop("gate-full-aaaaaaa", 900, "rc=0", A),
                start("gate-full-aaaaaaa-2", DRIFT + 900, A),
                stop("gate-full-aaaaaaa-2", DRIFT + 2000, "rc=0", A),
            ],
        )]));
        assert!(matches!(due(&again, A, T0 + DRIFT + 2000 + 3600, &D), Due::No(w) if w.contains("unchanged")));
        // Moved: due once the pass is an hour old.
        assert!(matches!(due(&gates, B, T0 + 1800, &D), Due::No(w) if w.contains("passed only 15 min ago")));
        assert!(matches!(due(&gates, B, T0 + 900 + MIN_AGE, &D), Due::Yes(w) if w.contains("moved since aaaaaaa")));
        assert!(matches!(due(&[], A, T0, &D), Due::Yes(w) if w.contains("no full gate has passed yet")));
    }

    /// The coordinator's `gate --full` on the stacked tree (`kind=full (--full)`, no commit), the
    /// merges inside it, and its stop as `land` closes it once `main` is the tree it tested.
    fn landed_full_gate(sub: &str, at: i64, main: &str) -> Vec<String> {
        vec![
            line("sub.start", sub, at, "local gate: #5 #6; capacity=1 free=9.0; kind=full (--full)"),
            line("merge", sub, at + 1800, ""),
            line(
                "sub.stop",
                sub,
                at + 1900,
                &gate::stop_note(&format!("{}; merged #5 #6", landed(main)), Some(&format!("rc=0 1700s kind=full (--full) head={B}"))),
            ),
        ]
    }

    /// #15: a landed `--full` gate verifies `main`: its stop counts as a passed full gate of that
    /// commit, so `main` isn't due; a smoke's merges after it make it due (once that pass is an
    /// hour old); a `--full` gate that didn't leave `main` at its tree (no `commit=`) counts for
    /// nothing.
    #[test]
    fn a_landed_full_gate_verifies_main_and_a_smoke_after_it_makes_it_due() {
        let coord = landed_full_gate("gate-319", 0, B);
        assert_eq!(coord.last().map(|l| l.contains(&format!("rc=0; kind=full; main={B}; merged #5 #6"))), Some(true), "{coord:?}");
        let gates = parse_gates(&files(&[("coord-1", &coord)]));
        assert_eq!(gates.len(), 1, "{gates:#?}");
        assert_eq!((gates[0].sub.as_str(), gates[0].commit.as_str(), gates[0].passed()), ("gate-319", B, true));
        assert!(open(&gates, T0 + 2000).is_empty());
        for later in [2000, 10 * 3600] {
            assert!(matches!(due(&gates, B, T0 + later, &D), Due::No(w) if w.contains("unchanged since its last passed full gate (gate-319")), "{later}");
        }
        // A smoke's merges after it: main moves past the verified commit, due an hour on.
        assert!(matches!(due(&gates, A, T0 + 2500, &D), Due::No(w) if w.contains("passed only")));
        assert!(
            matches!(due(&gates, A, T0 + 1900 + MIN_AGE, &D), Due::Yes(w) if w.contains("moved since bbbbbbb (gate-319") && w.contains("no landed full gate verified"))
        );
        // Another `--full` batch lands on top: verified again, not due.
        let mut more = coord.clone();
        more.extend(landed_full_gate("gate-320", 3000, A));
        assert!(matches!(due(&parse_gates(&files(&[("coord-1", &more)])), A, T0 + 9 * 3600, &D), Due::No(w) if w.contains("gate-320")));
        // A `--full` gate landed in part, or onto a moved main: no `commit=`, no credit.
        let partial = line("sub.stop", "gate-321", 1900, &gate::stop_note("merged #5", Some(&format!("rc=0 1700s kind=full (--full) head={B}"))));
        assert!(parse_gates(&files(&[("coord-1", &[partial])])).is_empty());
        // A smoke's stop, or a failed full gate's, never counts, `main=` or not; nor does `commit=` on a stop with no start.
        for note in [format!("rc=0; kind=smoke; main={B}"), format!("rc=1; cause=test; kind=full; main={B}"), format!("rc=0; kind=full; commit={B}; merged #5")]
        {
            assert!(parse_gates(&files(&[("coord-1", &[line("sub.stop", "gate-9", 10, &note)])])).is_empty(), "{note}");
        }
    }

    /// #15: inputs the gate reads that changed since its last pass make it due at once, as does
    /// `{prefix}_FULLGATE_DRIFT`; but not while one is open, nor for a `main` that failed one since.
    #[test]
    fn changed_inputs_make_it_due() {
        let changed = Drift { every: DRIFT, changed: Some("the driver moved to 999.1".into()) };
        let passed = parse_gates(&files(&[("w-1", &[start("gate-full-aaaaaaa", 0, A), stop("gate-full-aaaaaaa", 900, "rc=0", A)])]));
        assert!(matches!(due(&passed, A, T0 + 1000, &changed), Due::Yes(w) if w.contains("inputs changed: the driver moved to 999.1")));
        let running = parse_gates(&files(&[("w-1", &[start("gate-full-aaaaaaa", 0, A)])]));
        assert!(matches!(due(&running, A, T0 + 100, &changed), Due::No(w) if w.contains("is gating")));
        let red = parse_gates(&files(&[("w-1", &[start("gate-full-aaaaaaa", 0, A), stop("gate-full-aaaaaaa", 900, "rc=101; cause=test", A)])]));
        assert!(matches!(due(&red, A, T0 + 1000, &changed), Due::No(w) if w.contains("waits for main to move")));
        // A failure before the last pass of the same commit (a red then a green rerun) holds nothing.
        let mut lines = vec![start("gate-full-aaaaaaa", 0, A), stop("gate-full-aaaaaaa", 900, "rc=101; cause=test", A)];
        lines.extend([start("gate-full-aaaaaaa-2", 1000, A), stop("gate-full-aaaaaaa-2", 1900, "rc=0", A)]);
        let green = parse_gates(&files(&[("w-1", &lines)]));
        assert!(matches!(due(&green, A, T0 + 1900 + DRIFT, &D), Due::Yes(w) if w.contains("drift")));
        let r = report(&green, A, T0 + 2000, &D);
        assert!(r.contains("drift: due again 24.0 h after a pass") && r.contains("due: no"), "{r}");
    }

    /// mornycorp/swarm-core#3: a full gate stopped part way (`gate --stop` before its `.rc`,
    /// as a coordinator stopped `gate-full-da0d2db` at 2,035 of 2,082 tests) never counts
    /// as passed, and `main` stays due.
    #[test]
    fn a_full_gate_stopped_part_way_never_passed() {
        let note = gate::stop_note("stopped: the owner paused test runs", None);
        let stopped = line("sub.stop", "gate-full-aaaaaaa", 900, &note);
        let gates = parse_gates(&files(&[("w-1", &[start("gate-full-aaaaaaa", 0, A), stopped])]));
        assert_eq!(gates[0].end.as_ref().map(|e| e.rc), Some(None), "stopped, and no rc");
        assert_eq!(last_pass(&gates), None);
        assert!(matches!(due(&gates, A, T0 + 1800, &D), Due::Yes(w) if w.contains("no full gate has passed yet")));
    }

    #[test]
    fn two_racing_agents_one_gates_and_one_doesnt() {
        let both = files(&[("w-1", &[start("gate-full-bbbbbbb", 10, B)]), ("w-2", &[start("gate-full-bbbbbbb", 12, B)])]);
        let gates = parse_gates(&both);
        assert_eq!(race(&gates, "w-1", "gate-full-bbbbbbb", T0 + 20), Ok(()));
        let e = race(&gates, "w-2", "gate-full-bbbbbbb", T0 + 20).unwrap_err();
        assert!(e.contains("w-1 began a full gate of bbbbbbb first"), "{e}");
        // The same second: the instance decides, the same way for both.
        let tie = parse_gates(&files(&[("w-2", &[start("gate-full-bbbbbbb", 10, B)]), ("w-1", &[start("gate-full-bbbbbbb", 10, B)])]));
        assert_eq!(race(&tie, "w-1", "gate-full-bbbbbbb", T0 + 20), Ok(()));
        assert!(race(&tie, "w-2", "gate-full-bbbbbbb", T0 + 20).is_err());
        // A start that never reached the branch doesn't run.
        assert!(race(&gates, "w-3", "gate-full-bbbbbbb", T0 + 20).unwrap_err().contains("isn't on the tracking branch"));
        // A third agent, later, finds it not due while one is open.
        assert!(matches!(due(&gates, B, T0 + 600, &D), Due::No(w) if w.contains("w-1 is gating bbbbbbb")));
        // The loser's superseded stop leaves the winner's gate the open one.
        let lost = files(&[
            ("w-1", &[start("gate-full-bbbbbbb", 10, B)]),
            ("w-2", &[start("gate-full-bbbbbbb", 12, B), stop("gate-full-bbbbbbb", 13, "superseded: w-1 first", B)]),
        ]);
        let gates = parse_gates(&lost);
        assert_eq!(open(&gates, T0 + 20).len(), 1);
        assert_eq!(race(&gates, "w-1", "gate-full-bbbbbbb", T0 + 20), Ok(()));
    }

    #[test]
    fn a_stale_start_is_abandoned_and_anyone_takes_over() {
        let gates = parse_gates(&files(&[("w-1", &[start("gate-full-bbbbbbb", 0, B)])]));
        assert!(matches!(due(&gates, B, T0 + STALE - 1, &D), Due::No(w) if w.contains("w-1 is gating")));
        assert!(matches!(due(&gates, B, T0 + STALE, &D), Due::Yes(_)), "three hours with no stop: abandoned");
        let mut f = files(&[("w-1", &[start("gate-full-bbbbbbb", 0, B)])]);
        f.push(("w-2".into(), start("gate-full-bbbbbbb", STALE + 5, B)));
        assert_eq!(race(&parse_gates(&f), "w-2", "gate-full-bbbbbbb", T0 + STALE + 10), Ok(()));
        let r = report(&parse_gates(&f), B, T0 + STALE + 10, &D);
        assert!(r.contains("abandoned: gate-full-bbbbbbb of w-1") && r.contains("open: gate-full-bbbbbbb of w-2"), "{r}");
    }

    #[test]
    fn a_transient_failure_reruns_once_and_files_nothing() {
        assert_eq!(next(0, None, false, false), Next::Pass);
        assert_eq!(next(101, Some(Cause::Memory), false, false), Next::Rerun);
        assert_eq!(next(0, None, true, false), Next::Pass, "the rerun passed");
        assert_eq!(next(101, Some(Cause::Link), true, false), Next::Machine, "the rerun failed the same way: nothing filed");
        assert_eq!(next(101, Some(Cause::Gpu), false, false), Next::Rerun, "the GPU under load");
        assert_eq!(next(101, Some(Cause::Python), false, false), Next::Machine, "the gate machine's setup: a rerun fails the same way");
        // Tests that failed under load, which nextest names, are rerun alone first, once.
        for cause in [Cause::Test, Cause::Memory, Cause::GitTemp] {
            assert_eq!(next(100, Some(cause), false, true), Next::RerunTests, "{cause:?}");
        }
        assert_eq!(next(100, Some(Cause::Test), true, true), Next::File, "a test that failed again alone is real");
        assert_eq!(next(100, Some(Cause::Memory), true, false), Next::Machine, "the tests' rerun failed for the machine's cause");
        for real in [Cause::Test, Cause::Build, Cause::Other] {
            assert_eq!(next(101, Some(real), false, false), Next::File, "{real:?}");
        }
        assert_eq!(next(101, None, false, false), Next::File, "no cause: the change's");
        // A commit that failed for the machine's cause is due again an hour on; the change's never.
        let gates = |cause: &str| {
            let lines = [start("gate-full-bbbbbbb", 0, B), stop("gate-full-bbbbbbb", 600, &format!("rc=101; cause={cause}"), B)];
            parse_gates(&files(&[("w-1", &lines)]))
        };
        assert!(matches!(due(&gates("link"), B, T0 + 1200, &D), Due::No(w) if w.contains("cause link")));
        assert!(matches!(due(&gates("link"), B, T0 + 600 + MIN_AGE, &D), Due::Yes(_)));
        assert!(matches!(due(&gates("test"), B, T0 + 100 * 3600, &D), Due::No(w) if w.contains("waits for main to move")));
        assert!(matches!(due(&gates("test"), A, T0 + 1200, &D), Due::Yes(_)), "main moved: due (no pass to wait an hour on)");
    }

    /// A fake gate: each run's result in turn, and every attempt and event it saw.
    struct FakeGate {
        runs: RefCell<Vec<Ran>>,
        seen: RefCell<Vec<String>>,
    }

    impl FakeGate {
        fn attempt(&self, a: &Attempt<'_>) -> Result<Ran, String> {
            self.seen.borrow_mut().push(format!("{} start={:?} tests_of={:?} env={:?}", a.sub, a.start, a.tests_of, a.env));
            assert_eq!((a.commit, a.stop_tail()), (B, tail(B)));
            let mut runs = self.runs.borrow_mut();
            if runs.is_empty() { Err("no more runs".into()) } else { Ok(runs.remove(0)) }
        }
    }

    fn won_with(gate: &FakeGate, dir: &Path) -> (Vec<String>, Result<Outcome, String>) {
        let logged = RefCell::new(Vec::new());
        let env = [("GPU_SWARM_CI_CORPUS".to_owned(), "on".to_owned())];
        let runs = |a: &Attempt<'_>| gate.attempt(a);
        let ctx = Ctx { project: &P, hooks: &crate::Defaults, root: dir };
        let won = Won { ctx, agent: "w-1", sub: "gate-full-bbbbbbb", main: B, gates: &[], env: &env, runs: &runs };
        let outcome = run_won(&won, dir, &|a| logged.borrow_mut().push(a.join(" ")));
        (logged.into_inner(), outcome)
    }

    /// The loop over the consumer's gate: tests that failed under load rerun alone, then the pass
    /// says so; the machine's cause twice files nothing.
    #[test]
    fn a_won_gate_reruns_through_the_consumers_gate() {
        let dir = Path::new(".");
        let failed = |cause, tests| Ran { rc: 100, cause: Some(cause), rerun_tests: tests, ..Ran::default() };
        let gate = FakeGate { runs: RefCell::new(vec![failed(Cause::Test, Some(2)), Ran::default()]), seen: RefCell::default() };
        let (logged, outcome) = won_with(&gate, dir);
        assert_eq!(outcome, Ok(Outcome::Passed(B.into())));
        let seen = gate.seen.borrow();
        assert_eq!(seen[0], "gate-full-bbbbbbb start=None tests_of=None env=[(\"GPU_SWARM_CI_CORPUS\", \"on\")]");
        assert!(seen[1].starts_with("gate-full-bbbbbbb-rerun start=Some(\"rerun of gate-full-bbbbbbb: its failed tests alone (2); full gate of main\") tests_of=Some(\"gate-full-bbbbbbb\")"), "{seen:?}");
        assert_eq!(logged, [format!("sub.stop --agent w-1 --sub gate-full-bbbbbbb-rerun --note rc=0; reran 2 failed tests alone{}", tail(B))]);

        let gate = FakeGate { runs: RefCell::new(vec![failed(Cause::Memory, None), failed(Cause::Memory, None)]), seen: RefCell::default() };
        let (logged, outcome) = won_with(&gate, dir);
        assert_eq!(outcome, Ok(Outcome::Machine(Cause::Memory)));
        assert!(gate.seen.borrow()[1].contains("start=Some(\"rerun of gate-full-bbbbbbb: full gate of main\") tests_of=None"));
        assert!(logged.is_empty(), "the gate logs its own failures: {logged:?}");

        // A run that can't start: the error, nothing logged.
        let gate = FakeGate { runs: RefCell::new(Vec::new()), seen: RefCell::default() };
        assert_eq!(won_with(&gate, dir).1, Err("no more runs".into()));
    }

    #[test]
    fn a_real_failure_names_the_commits_and_the_merges_between() {
        let merges = vec!["1234567 Merge branch '190-help-samples' into 'main'".to_owned()];
        let tests = vec!["swarm-engine::golden commands_replies_match_their_golden_files".to_owned()];
        let f =
            Failure { failing: B, green: Some(A), merges: &merges, cause: Cause::Test, tests: &tests, sub: "gate-full-bbbbbbb", agent: "w-1", log: "D:/x.log" };
        let (title, body) = red_issue(&f);
        assert_eq!(title, "main is red: the full gate of bbbbbbb failed (test)");
        for want in [
            &format!("**Failing commit:** `{B}`")[..],
            &format!("**Last green commit:** `{A}`"),
            "**Merges between them:**\n  - 1234567 Merge branch '190-help-samples' into 'main'\n",
            "**Failing tests:**\n  - swarm-engine::golden commands_replies_match_their_golden_files\n",
            "`gate-full-bbbbbbb` of w-1; its log: `D:/x.log`",
        ] {
            assert!(body.contains(want), "{want}\n---\n{body}");
        }
        let many: Vec<String> = (0..40).map(|i| format!("m{i}")).collect();
        let (_, body) = red_issue(&Failure { green: None, merges: &many, tests: &[], ..f });
        assert!(body.contains("none recorded") && body.contains("unknown: no green commit") && body.contains("none listed"), "{body}");
        let (_, body) = red_issue(&Failure { merges: &many, ..f });
        assert!(body.contains("  - m29\n  - … and 10 more\n") && !body.contains("m30"), "{body}");
    }

    #[test]
    fn sub_ids_are_unique_within_a_session() {
        assert_eq!(sub_id(&[], "w-1", B), "gate-full-bbbbbbb");
        let gates = parse_gates(&files(&[
            ("w-1", &[start("gate-full-bbbbbbb", 0, B), start("gate-full-bbbbbbb-2", 9, B)]),
            ("w-2", &[start("gate-full-aaaaaaa", 0, A)]),
        ]));
        assert_eq!(sub_id(&gates, "w-1", B), "gate-full-bbbbbbb-3");
        assert_eq!(sub_id(&gates, "w-1", A), "gate-full-aaaaaaa", "another's id is no clash");
        for s in [sub_id(&gates, "w-1", B), format!("{}-rerun", sub_id(&gates, "w-1", B))] {
            event::check_id(&s).unwrap();
        }
    }

    #[test]
    fn the_machine_says_whether_it_runs_the_full_gate() {
        let never = || -> bool { panic!("the project's rule is asked only when the variable doesn't say") };
        for p in BOTH {
            let name = env_name(&p);
            assert_eq!(name, format!("{}_FULLGATE", p.env_prefix));
            assert_eq!(capable_by(&name, Some("on"), never), Ok(true), "a Mac told to");
            assert_eq!(capable_by(&name, Some("OFF"), never), Ok(false));
            assert!(capable_by(&name, Some("maybe"), never).unwrap_err().contains(&format!("{name}=maybe: expected on or off")));
            assert_eq!(capable_by(&name, None, || true), Ok(true), "the Windows desktop");
            assert_eq!(capable_by(&name, Some(" "), || false), Ok(false), "the Mac");
        }
        assert_eq!(crate::Defaults.full_gate_capable(), cfg!(windows), "the default rule: a Windows machine");
    }
}
