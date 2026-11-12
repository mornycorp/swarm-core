//! `cargo xtask gate --daemon` : the coordinator's merge loop as a
//! long-running process on the gating machine, run in the gate's worktree with the gate's
//! environment (docs/swarm-protocol.md › The gate daemon). The loop was mechanical, but run by
//! hand it waited on the coordinator's wakes and slipped: ready PRs that conflicted sat unseen for
//! hours, a waiting gate was restacked, a gate was killed mid-build, git ran in the gate's
//! worktree mid-gate, stale rerere resolutions were replayed. Encoded here it is the same every
//! time, and fixable.
//!
//! Each round (every `--interval` seconds, default 60):
//!
//! 1. **Pause** while the off switch is set (`--stop`; a file in the gate's target dir), a
//!    `main-red` issue is open, or GitHub can't say; say so once per change and go
//!    on when it clears ([`policy::pause`]).
//! 2. **List the ready PRs** (open, not drafts, into `main`, Progress filled in:
//!    [`policy::candidate`]). One GitHub marks as conflicting gets a note, once per head, and
//!    waits for its worker; so does one the daemon dropped at this head before.
//! 3. **Order them:** an PR after every open PR whose commits its branch holds, then by the time
//!    it was first seen ready; one that holds a draft's commits waits ([`policy::order`]).
//! 4. **Stack them** (`stack`, with rerere off: the daemon runs with
//!    `GIT_CONFIG_PARAMETERS='rerere.enabled=false'`). A conflict drops the PR and those stacked
//!    on it, and the rest is stacked again. One that conflicts with `main` itself gets the note
//!    "conflicts with main in <files> (after !N landed): rebase onto main and run `cargo xtask
//!    ready`", once per head.
//! 5. **Fold, then gate.** While there is no room for the gate's build (the machine gate lock
//!    held by another, or `capacity` says none), the stack waits up to `--fold` seconds (default
//!    120) and a newly ready PR is folded in by restacking, which is free. Then `gate --sub
//!    gate-<n> --wait --rerun` runs as a child process, and the stack is frozen until it ends:
//!    the daemon never kills a gate, and runs no git in the worktree while it runs.
//! 6. **Pushes during the gate,** read after it: a head with the same patch (a rebase) lands as
//!    gated; a real change waits, with what is above it, for the next round
//!    ([`policy::push`], [`policy::land_prefix`]).
//! 7. **The result.** A pass (or its rerun's) lands with `land`. A failure the machine caused is
//!    no PR's: gated again next round, and after three in a row the off switch is set. Any other
//!    failure is blamed on the PR it points at ([`blame`]): that PR goes back to draft with the
//!    log's excerpt in a note, and the rest is gated again; when none or several match, the
//!    first half of the stack is gated ([`policy::bisect`]). A stack that failed never lands.
//!    A bisection keeps its suspects (the failed gate's PRs) across gates and rounds: a half
//!    that passes lands and clears its PRs, and the culprit is among the rest. One left is
//!    dropped as above; several are bisected next, never put back in a fresh stack. A suspect
//!    that moves or leaves ends the bisection (#46). With [`crate::Hooks::verify_main`], a
//!    stack's first failure at its tests first runs those tests alone on `main` (`verify main
//!    for gate-<n>`, `gate --test-alone`, #48): when one fails there too (a driver update, the
//!    machine), the off switch is set and no PR is sent back (#47). Tests it can't name plainly
//!    aren't checked: the failure is blamed as before. Nor are tests `main` lacks (no `fn <name>`
//!    there, or nextest's "no tests to run"): new in the stack, they are its own (#51).
//!
//! Every decision is printed with a time; those that change state are on the tracking timeline:
//! the gate's own events, `land`'s merges, and a `sendback` for each PR dropped (a conflict's note
//! starts `cause=conflict`). Each gate's note names its step (`daemon gate:`, `regate of <sub>:`,
//! `bisect of <sub>:`, [`policy::gate_note`]); an PR a failure sets aside for a regate or a
//! bisection gets no `sendback`, so a reader doesn't count it as sent back
//! (docs/tracking-format.md › Send-backs,. `--once` runs one round;
//! `--dry-run` says what it would do and changes nothing: no note, no event, no state saved, so it
//! can run beside a live daemon (#44). `--status` prints the daemon's state, which is saved with
//! each decision, so it shows a gate while it runs (#45); `--stop [--note <why>]` sets the off
//! switch and `--resume` clears it (and the PRs held for the coordinator).
//!
//! The decisions are pure ([`policy`], [`blame`]); everything else goes through [`World`], which
//! the tests fake.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::Project;
use crate::gate::Cause;
use crate::stack::{Entry, Stack, StackMr};

pub mod blame;
pub mod policy;
mod real;
#[cfg(test)]
mod tests;

use blame::{Blame, Packages, Touch};
use policy::{Candidate, Pause, Push, Ran, Step};

pub use real::run;

/// The usage, as `gate`'s lists it.
pub const USAGE: &str = "cargo xtask gate --daemon [--once] [--dry-run] [--interval <s>] [--fold <s>] [--agent <id>] | --daemon --status | --daemon --stop [--note <why>] | --daemon --resume";

/// The most gates one round runs (a failure's regate and bisection included).
const MAX_GATES: usize = 8;
/// How often a pause is said again while it lasts, so a watch on the log sees it
/// .
const PAUSE_AGAIN: i64 = 30 * 60;
/// The decisions the state keeps for `--status`.
const KEEP_LOG: usize = 30;
/// A land that stops at the same PR head this many times holds it for the coordinator.
const LAND_STOPS: u32 = 2;

/// How `stack` went.
#[derive(Debug)]
pub enum StackRun {
    Built(Stack),
    /// It stopped at PR `at`; `stack` is what was stacked below it.
    Stopped {
        stack: Stack,
        at: u64,
        why: String,
    },
    /// Before the first PR.
    Failed(String),
}

/// How a gate went: its result and its log (with its rerun's, when it had one).
#[derive(Debug)]
pub struct GateRun {
    pub ran: Ran,
    pub log: String,
}

/// Everything the daemon does outside its decisions: GitHub, git, the gate, `land`, the clock and
/// its state. The real one is [`real`]'s; the tests' is a fake.
pub trait World {
    fn now(&self) -> i64;
    /// Print a decision.
    fn say(&self, line: &str);
    fn sleep(&self, d: Duration);
    /// The off switch's reason, when it is set.
    fn switch(&self) -> Option<String>;
    fn set_switch(&self, why: &str) -> Result<(), String>;
    fn save(&self, state: &State) -> Result<(), String>;
    fn main_red(&self) -> Result<Vec<(u64, String)>, String>;
    /// The project's open PRs, as GitHub lists them (and the remote fetched).
    fn open_mrs(&self) -> Result<Vec<Value>, String>;
    /// The commits `sha` adds to `main`.
    fn commits(&self, sha: &str) -> Result<BTreeSet<String>, String>;
    /// The files `sha` conflicts in with `main` (none: it merges cleanly).
    fn main_conflict(&self, sha: &str) -> Result<Vec<String>, String>;
    /// The PR whose merge into `main` last changed `files`.
    fn landed_after(&self, files: &[String]) -> Option<u64>;
    fn note(&self, mr: u64, body: &str) -> Result<(), String>;
    /// Put `mr` back to draft with the note `body`, when it is still open, ready and at `sha`:
    /// whether it did.
    fn redraft(&self, mr: u64, sha: &str, body: &str) -> Result<bool, String>;
    /// Log an event (the daemon's agent is added).
    fn event(&self, args: &[&str]);
    fn stack(&self, mrs: &[StackMr]) -> StackRun;
    /// Room for the gate's build now: the machine gate lock and capacity.
    fn room(&self) -> bool;
    fn next_sub(&self) -> String;
    fn gate(&self, sub: &str, note: &str) -> GateRun;
    /// Run `tests` (`<binary id> <test>`), each alone, on `main` with nothing stacked, as gate
    /// `sub` (`gate --test-alone`).
    fn gate_main(&self, sub: &str, note: &str, tests: &[String]) -> GateRun;
    /// Whether `main` has a test function named `name` (the last segment of a test's path): a
    /// test it lacks is new in the stack, so `main` can't fail it.
    fn on_main(&self, name: &str) -> bool;
    /// The files an entry changes, from where it was stacked.
    fn touched(&self, e: &Entry) -> Vec<String>;
    fn packages(&self) -> Packages;
    /// The gate's root, as its log spells paths.
    fn root(&self) -> String;
    /// What each entry's source branch did since it was stacked.
    fn pushes(&self, stack: &Stack) -> Vec<Push>;
    /// `land <mrs> --sub <sub>`: the PRs it landed, or why it stopped and those it landed first.
    fn land(&self, mrs: &[u64], sub: &str) -> Result<Vec<u64>, (String, Vec<u64>)>;
    /// `gate --stop <sub>`.
    fn close(&self, sub: &str, note: &str);
    /// Drop the gate lock (a failed gate keeps it).
    fn drop_lock(&self);
}

/// What the daemon keeps between rounds and restarts (`<target>/gate-daemon/state.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Why it is paused, when it is.
    pub paused: Option<String>,
    /// The head each PR was told it conflicts with `main` at.
    pub noted: BTreeMap<u64, String>,
    /// The head each PR was dropped at for a gate's failure: not gated again at it.
    pub dropped: BTreeMap<u64, String>,
    /// PRs held for the coordinator at a head, and why: not gated again at it (`--resume` clears
    /// them).
    pub held: BTreeMap<u64, (String, String)>,
    /// When each ready PR was first seen ready.
    pub seen: BTreeMap<u64, i64>,
    /// A bisection's suspects, until it finds the culprit.
    pub suspects: Option<Suspects>,
    /// The latest decisions.
    pub log: Vec<String>,
}

/// The PRs a failed gate's culprit is among, while a bisection looks for it: carried across
/// gates and rounds, and narrowed by each gate of half of them, until one is left, which is
/// blamed .
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suspects {
    /// The last gate that failed with all of them.
    pub of: String,
    /// Its failure's cause.
    pub cause: Option<Cause>,
    /// Its log's excerpt, for the culprit's note.
    pub excerpt: String,
    /// Each suspect, at the head it was gated at.
    pub mrs: Vec<Suspect>,
}

/// One of a bisection's [`Suspects`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suspect {
    pub mr: u64,
    pub sha: String,
    /// The issues it closes.
    pub issues: Vec<u64>,
}

impl Suspect {
    fn of(e: &Entry) -> Suspect {
        Suspect { mr: e.mr, sha: e.old_tip.clone(), issues: e.issues.clone() }
    }
}

impl Suspects {
    fn iids(&self) -> Vec<u64> {
        self.mrs.iter().map(|s| s.mr).collect()
    }

    fn to_json(&self) -> Value {
        let mrs: Vec<Value> = self.mrs.iter().map(|s| json!({"mr": s.mr, "sha": s.sha, "issues": s.issues})).collect();
        json!({"of": self.of, "cause": self.cause.map(Cause::name), "excerpt": self.excerpt, "mrs": mrs})
    }

    fn from_json(v: &Value) -> Option<Suspects> {
        let mrs = v.get("mrs")?.as_array()?.iter().filter_map(|s| {
            let issues = s.get("issues").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            Some(Suspect { mr: s.get("mr")?.as_u64()?, sha: s.get("sha")?.as_str()?.to_owned(), issues })
        });
        Some(Suspects {
            of: v.get("of")?.as_str()?.to_owned(),
            cause: v.get("cause").and_then(Value::as_str).and_then(Cause::parse),
            excerpt: v.get("excerpt").and_then(Value::as_str).unwrap_or_default().to_owned(),
            mrs: mrs.collect(),
        })
    }
}

impl State {
    pub fn to_json(&self) -> Value {
        let held: BTreeMap<String, Value> = self.held.iter().map(|(m, (sha, why))| (m.to_string(), json!({"sha": sha, "why": why}))).collect();
        json!({
            "paused": self.paused, "noted": keyed(&self.noted), "dropped": keyed(&self.dropped), "held": held,
            "seen": self.seen.iter().map(|(m, t)| (m.to_string(), json!(t))).collect::<BTreeMap<_, _>>(),
            "suspects": self.suspects.as_ref().map(Suspects::to_json), "log": self.log,
        })
    }

    /// A state [`State::to_json`] wrote; what can't be read is left out.
    pub fn from_json(v: &Value) -> State {
        let map = |k: &str| v.get(k).and_then(Value::as_object).cloned().unwrap_or_default();
        let heads = |k: &str| map(k).iter().filter_map(|(m, s)| Some((m.parse().ok()?, s.as_str()?.to_owned()))).collect();
        State {
            paused: v.get("paused").and_then(Value::as_str).map(str::to_owned),
            noted: heads("noted"),
            dropped: heads("dropped"),
            held: map("held")
                .iter()
                .filter_map(|(m, h)| Some((m.parse().ok()?, (h.get("sha")?.as_str()?.to_owned(), h.get("why")?.as_str()?.to_owned()))))
                .collect(),
            seen: map("seen").iter().filter_map(|(m, t)| Some((m.parse().ok()?, t.as_i64()?))).collect(),
            suspects: v.get("suspects").and_then(Suspects::from_json),
            log: v.get("log").and_then(Value::as_array).map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
        }
    }

    /// Whether `c` waits at its head: dropped there, held there, or noted conflicting there.
    fn waits(&self, c: &Candidate) -> Option<String> {
        let at = |m: &BTreeMap<u64, String>| m.get(&c.iid()) == Some(&c.sha);
        if at(&self.dropped) {
            return Some("dropped at this head after a gate failure".to_owned());
        }
        if let Some((_, why)) = self.held.get(&c.iid()).filter(|(sha, _)| *sha == c.sha) {
            return Some(format!("held for the coordinator: {why}"));
        }
        at(&self.noted).then(|| "conflicts with main at this head".to_owned())
    }
}

fn keyed(m: &BTreeMap<u64, String>) -> BTreeMap<String, String> {
    m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// The daemon's settings.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub dry_run: bool,
    /// How long a stack waits for room, folding in new PRs, before the gate runs anyway (and
    /// waits itself, frozen).
    pub fold: Duration,
    /// Gate `main` alone before sending an PR back for a stack's first failure at its tests
    /// ([`crate::Hooks::verify_main`]).
    pub verify_main: bool,
}

/// What a round did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Round {
    Paused,
    Idle,
    /// Stacked, waiting for room.
    Waiting,
    /// `--dry-run`: what it would gate.
    WouldGate(Vec<u64>),
    Gated,
}

/// The ready PRs to gate, in order, and their dependencies.
#[derive(Clone)]
struct Plan {
    order: Vec<Candidate>,
    deps: BTreeMap<u64, BTreeSet<u64>>,
}

/// The daemon: its state and what it holds between rounds.
pub struct Daemon<'a> {
    project: &'a Project,
    w: &'a dyn World,
    settings: Settings,
    pub state: State,
    /// What is stacked now (PR and head, in order), and its record.
    stacked: Option<(Vec<(u64, String)>, Stack)>,
    /// When the stack began waiting for room.
    waiting_since: Option<i64>,
    machine_failures: u32,
    land_stops: BTreeMap<(u64, String), u32>,
    /// The last line said under each key, so a state is said once per change.
    said: BTreeMap<String, String>,
    /// When the pause line was last said, to say it again every [`PAUSE_AGAIN`].
    paused_said: i64,
}

impl<'a> Daemon<'a> {
    pub fn new(project: &'a Project, w: &'a dyn World, settings: Settings, state: State) -> Self {
        Daemon {
            project,
            w,
            settings,
            state,
            stacked: None,
            waiting_since: None,
            machine_failures: 0,
            land_stops: BTreeMap::new(),
            said: BTreeMap::new(),
            paused_said: 0,
        }
    }

    /// Say a decision and keep it for `--status`, saved now: a gate runs for minutes, and
    /// `--status` shows it while it runs .
    fn say(&mut self, line: &str) {
        self.w.say(line);
        self.state.log.push(format!("{} {line}", crate::time::iso_utc(u64::try_from(self.w.now()).unwrap_or(0))));
        let skip = self.state.log.len().saturating_sub(KEEP_LOG);
        self.state.log.drain(..skip);
        self.save();
    }

    /// Say `line` under `key` unless it was the last said there.
    fn say_once(&mut self, key: &str, line: &str) {
        if self.said.get(key).map(String::as_str) != Some(line) {
            self.said.insert(key.to_owned(), line.to_owned());
            self.say(line);
        }
    }

    /// Save the state; a dry run saves nothing, so it can run beside a live daemon without
    /// clobbering its state .
    fn save(&mut self) {
        if self.settings.dry_run {
            return;
        }
        if let Err(e) = self.w.save(&self.state) {
            self.w.say(&format!("warning: couldn't save the daemon's state: {e}"));
        }
    }

    /// One round ([the module's steps](self)); the state is saved after it.
    pub fn round(&mut self) -> Round {
        let r = self.round_inner();
        self.save();
        r
    }

    fn round_inner(&mut self) -> Round {
        if self.paused() {
            return Round::Paused;
        }
        let Some(plan) = self.plan() else { return Round::Idle };
        let (first, step) = self.bisecting(&plan);
        if self.settings.dry_run {
            let mrs: Vec<u64> = first.order.iter().map(Candidate::iid).collect();
            self.say(&format!("dry run: would stack and gate {}", policy::bangs(&mrs)));
            return Round::WouldGate(mrs);
        }
        let Some(stack) = self.stacked_for(&first) else { return Round::Idle };
        if !self.w.room() && self.waited() < self.settings.fold.as_secs() {
            let mrs = entries(&stack);
            self.say_once("wait", &format!("stacked {}: waiting for room to build, folding in newly ready PRs", policy::bangs(&mrs)));
            return Round::Waiting;
        }
        self.waiting_since = None;
        self.stacked = None;
        self.gate_loop(&plan, stack, step);
        Round::Gated
    }

    /// What to stack first: half of a bisection's suspects while they are all still ready at the
    /// heads they failed at (its next step), else the whole plan. A suspect that moved or left
    /// ends the bisection: the full stack's gate finds the failure again if it is still there.
    fn bisecting(&mut self, plan: &Plan) -> (Plan, Step) {
        let Some(s) = self.state.suspects.clone() else { return (plan.clone(), Step::First) };
        let moved = s.mrs.iter().find(|x| !plan.order.iter().any(|c| c.iid() == x.mr && c.sha == x.sha));
        if let Some(x) = moved {
            self.say(&format!("the bisection of {} ends: #{} moved or isn't ready; gating as usual", s.of, x.mr));
            self.state.suspects = None;
            return (plan.clone(), Step::First);
        }
        let half = policy::bisect(&s.iids());
        self.say_once("bisect", &format!("bisecting {}'s suspects {}: gating {} next", s.of, policy::bangs(&s.iids()), policy::bangs(&half)));
        let order = plan.order.iter().filter(|c| half.contains(&c.iid())).cloned().collect();
        (Plan { order, deps: plan.deps.clone() }, Step::Bisect(s.of))
    }

    /// Seconds the stack has waited for room.
    fn waited(&self) -> u64 {
        self.waiting_since.map_or(0, |t| u64::try_from(self.w.now().saturating_sub(t)).unwrap_or(0))
    }

    /// Whether to pause now, said once per change.
    fn paused(&mut self) -> bool {
        let p = self.pause_now();
        let line = p.as_ref().map(Pause::describe);
        let now = self.w.now();
        if line != self.state.paused {
            let say = line.clone().unwrap_or_else(|| "resumed: main is green and the switch is off".to_owned());
            self.say(&say);
            self.state.paused = line;
            self.paused_said = now;
        } else if let Some(line) = line.filter(|_| now.saturating_sub(self.paused_said) >= PAUSE_AGAIN) {
            self.say(&format!("still {line}"));
            self.paused_said = now;
        }
        p.is_some()
    }

    /// Why to pause now: [`policy::pause`]'s, or that `main` couldn't be checked (GitHub didn't
    /// answer), which pauses too, until it answers.
    fn pause_now(&self) -> Option<Pause> {
        match self.w.main_red() {
            Err(e) => Some(Pause::Unchecked(first_line(&format!("its main-red issues: {e}")))),
            Ok(red) => policy::pause(self.w.switch().as_deref(), &red),
        }
    }

    /// The ready PRs to gate now, in order; `None` when there are none.
    fn plan(&mut self) -> Option<Plan> {
        let open = match self.w.open_mrs() {
            Ok(o) => o,
            Err(e) => {
                self.say_once("list", &format!("couldn't list the open PRs: {e}"));
                return None;
            }
        };
        let candidates = self.candidates(&open);
        let todo = self.waiting_out(candidates);
        if todo.is_empty() {
            self.say_once("idle", "idle: no PR ready to gate");
            return None;
        }
        let heads: Vec<(u64, String)> = open.iter().filter_map(|v| Some((v.get("number")?.as_u64()?, v.get("head_sha")?.as_str()?.to_owned()))).collect();
        let commits: BTreeMap<u64, BTreeSet<String>> = todo.iter().filter_map(|c| Some((c.iid(), self.w.commits(&c.sha).ok()?))).collect();
        let deps = policy::dependencies(&todo, &heads, &commits);
        let (order, held) = policy::order(&todo, &deps);
        for (mr, why) in held {
            self.say_once(&format!("held-{mr}"), &format!("#{mr} waits: {why}"));
        }
        self.said.remove("idle");
        let order: Vec<Candidate> = order.iter().filter_map(|m| todo.iter().find(|c| c.iid() == *m).cloned()).collect();
        (!order.is_empty()).then_some(Plan { order, deps })
    }

    /// The ready PRs among `open`, each with when it was first seen ready (kept in the state).
    fn candidates(&mut self, open: &[Value]) -> Vec<Candidate> {
        let now = self.w.now();
        let cands: Vec<Candidate> = open
            .iter()
            .filter_map(|v| {
                let iid = v.get("number")?.as_u64()?;
                policy::candidate(self.project, v, self.state.seen.get(&iid).copied().unwrap_or(now))
            })
            .collect();
        self.state.seen = cands.iter().map(|c| (c.iid(), c.ready_since)).collect();
        cands
    }

    /// `cands` less those that wait for their worker: GitHub says they conflict (noted, once per
    /// head), or the daemon dropped, held or noted them at this head.
    fn waiting_out(&mut self, cands: Vec<Candidate>) -> Vec<Candidate> {
        let mut todo = Vec::new();
        for c in cands {
            if c.conflict && self.state.noted.get(&c.iid()) != Some(&c.sha) {
                let files = self.w.main_conflict(&c.sha).unwrap_or_default();
                self.conflicted(&c, &self.real_conflicts(files));
            }
            let why = if c.conflict { Some("GitHub says it conflicts with main".to_owned()) } else { self.state.waits(&c) };
            match why {
                Some(why) => self.say_once(&format!("waits-{}", c.iid()), &format!("#{} at {} waits for its worker: {why}", c.iid(), short(&c.sha))),
                None => todo.push(c),
            }
        }
        todo
    }

    /// The conflicted files `stack` can't resolve itself: a generated doc is regenerated.
    fn real_conflicts(&self, files: Vec<String>) -> Vec<String> {
        files.into_iter().filter(|f| self.project.generated(f).is_none()).collect()
    }

    /// Tell `c`'s worker it conflicts with `main` in `files`, once per head: a note, and a
    /// `sendback` for each issue it closes.
    fn conflicted(&mut self, c: &Candidate, files: &[String]) {
        if self.state.noted.get(&c.iid()) == Some(&c.sha) {
            return;
        }
        let list = if files.is_empty() { "unnamed files".to_owned() } else { files.join(", ") };
        if self.settings.dry_run {
            self.say(&format!("dry run: would drop #{} at {}: conflicts with main in {list}; would note it", c.iid(), short(&c.sha)));
            return;
        }
        let after = self.w.landed_after(files);
        let note = policy::conflict_note(files, after);
        let posted = self.w.note(c.iid(), &note);
        let send = format!("cause=conflict: {list}");
        for issue in &c.mr.issues {
            self.w.event(&["sendback", "--issue", &issue.to_string(), "--mr", &c.iid().to_string(), "--note", &send]);
        }
        let how = match posted {
            Ok(()) => "noted".to_owned(),
            Err(e) => format!("its note failed: {e}"),
        };
        self.state.noted.insert(c.iid(), c.sha.clone());
        self.say(&format!("dropped #{} at {}: conflicts with main in {list}; {how}", c.iid(), short(&c.sha)));
    }

    /// The stack for `plan`: the one stacked already when it is the same PRs at the same heads,
    /// else a new one ([`Daemon::stack_dropping`]).
    fn stacked_for(&mut self, plan: &Plan) -> Option<Stack> {
        let key: Vec<(u64, String)> = plan.order.iter().map(|c| (c.iid(), c.sha.clone())).collect();
        if let Some((k, s)) = &self.stacked
            && *k == key
        {
            return Some(s.clone());
        }
        let stack = self.stack_dropping(plan.order.clone(), &plan.deps)?;
        self.stacked = Some((key, stack.clone()));
        self.waiting_since.get_or_insert(self.w.now());
        Some(stack)
    }

    /// Stack `list`; a stop drops the PR it stopped at and those stacked on it, and stacks the
    /// rest again. `None`: nothing stacked.
    fn stack_dropping(&mut self, mut list: Vec<Candidate>, deps: &BTreeMap<u64, BTreeSet<u64>>) -> Option<Stack> {
        for _ in 0..=list.len() {
            if list.is_empty() {
                return None;
            }
            let mrs: Vec<StackMr> = list.iter().map(|c| c.mr.clone()).collect();
            match self.w.stack(&mrs) {
                StackRun::Built(s) => {
                    self.say(&format!("stacked {}", policy::bangs(&entries(&s))));
                    return Some(s);
                }
                StackRun::Failed(why) => {
                    self.say_once("stack", &format!("couldn't stack: {}", first_line(&why)));
                    return None;
                }
                StackRun::Stopped { stack, at, why } => {
                    let c = list.iter().find(|c| c.iid() == at).cloned()?;
                    self.stopped_at(&c, stack.entries.is_empty(), &why);
                    let out = policy::with_dependents(at, deps);
                    list.retain(|c| !out.contains(&c.iid()));
                }
            }
        }
        None
    }

    /// `stack` stopped at `c` (`first`: with nothing below it): a conflict with `main` is noted;
    /// one with the PRs below waits for them to land; anything else holds it at this head.
    fn stopped_at(&mut self, c: &Candidate, first: bool, why: &str) {
        let files = self.w.main_conflict(&c.sha).map(|f| self.real_conflicts(f)).unwrap_or_default();
        if !files.is_empty() {
            self.conflicted(c, &files);
        } else if first {
            let why = first_line(why);
            self.say(&format!("held #{} at {}: stack stopped at it: {why}", c.iid(), short(&c.sha)));
            self.state.held.insert(c.iid(), (c.sha.clone(), why));
        } else {
            self.say(&format!("#{} conflicts with the PRs stacked below it, not main: it waits for them to land", c.iid()));
        }
    }

    /// Gate `stack`; after a failure, gate what is left (without the PR it points at, or the first
    /// half), up to [`MAX_GATES`] gates. Each gate's note names its step and the PRs it gates
    /// ([`policy::gate_note`]).
    fn gate_loop(&mut self, plan: &Plan, mut stack: Stack, mut step: Step) {
        for _ in 0..MAX_GATES {
            let sub = self.w.next_sub();
            let mrs = entries(&stack);
            self.say(&format!("gating {} as {sub}", policy::bangs(&mrs)));
            let run = self.w.gate(&sub, &policy::gate_note(&step, &mrs));
            let next = match run.ran {
                Ran::Passed(standing) => {
                    let landed = self.land(&stack, &standing);
                    match self.after_pass(&mrs, &sub) {
                        Some(next) if landed => next,
                        _ => return,
                    }
                }
                Ran::NotRun => {
                    self.say(&format!("{sub} didn't run (refused, or stopped before its `ci`): gated again next round"));
                    return self.w.drop_lock();
                }
                Ran::Failed(cause) => {
                    self.w.drop_lock();
                    match self.after_failure(plan, &stack, (&sub, &step), cause, &run.log) {
                        Some(next) => next,
                        None => return,
                    }
                }
            };
            let list: Vec<Candidate> = plan.order.iter().filter(|c| next.0.contains(&c.iid())).cloned().collect();
            let Some(s) = self.stack_dropping(list, &plan.deps) else { return };
            stack = s;
            step = next.1;
        }
        self.say(&format!("{MAX_GATES} gates this round: the rest waits for the next"));
    }

    /// After gate `sub` of `stack` failed (`cause`, its `log`): what to gate next and why (a
    /// regate or a bisection of `sub`); `None`: nothing this round. Only an PR the failure is
    /// blamed on gets a `sendback`; one set aside (stacked on it, or out of a bisection's half)
    /// gets none: it is not sent back (docs/tracking-format.md › Send-backs).
    fn after_failure(&mut self, plan: &Plan, stack: &Stack, (sub, step): (&str, &Step), cause: Option<Cause>, log: &str) -> Option<(Vec<u64>, Step)> {
        if policy::machine_failure(cause) {
            self.machine_failure(sub, cause);
            return None;
        }
        self.machine_failures = 0;
        let failure = blame::failure(log, &self.w.root(), self.project.golden_binary);
        let at_tests = !failure.tests.is_empty() && failure.files.is_empty();
        if self.settings.verify_main && *step == Step::First && at_tests && !self.main_passes(sub, &failure) {
            self.state.suspects = None;
            return None;
        }
        let mrs = entries(stack);
        let touched: Vec<Touch> = stack.entries.iter().map(|e| Touch { mr: e.mr, files: self.w.touched(e) }).collect();
        let blamed = match blame::blame(&failure, &touched, &self.w.packages()) {
            Blame::One(m, why) => Some((m, why)),
            _ if mrs.len() == 1 => mrs.first().map(|m| (*m, "it was the only PR in the stack".to_owned())),
            other => {
                let half = policy::bisect(&mrs);
                let who = if let Blame::Several(s) = other { format!("it points at {}", policy::bangs(&s)) } else { "it names no PR's change".to_owned() };
                self.say(&format!("{sub} failed and {who}: bisecting, gating {} next", policy::bangs(&half)));
                let mrs = stack.entries.iter().map(Suspect::of).collect();
                self.state.suspects = Some(Suspects { of: sub.to_owned(), cause, excerpt: crate::gate::summary(log), mrs });
                return Some((half, Step::Bisect(sub.to_owned())));
            }
        };
        self.state.suspects = None;
        let (m, why) = blamed?;
        let e = stack.entries.iter().find(|e| e.mr == m)?;
        self.drop_failed(&Suspect::of(e), sub, cause, &why, &crate::gate::summary(log));
        let out = policy::with_dependents(m, &plan.deps);
        let rest: Vec<u64> = mrs.into_iter().filter(|x| !out.contains(x)).collect();
        (!rest.is_empty()).then(|| (rest, Step::Regate(sub.to_owned())))
    }

    /// Gate `main` alone after gate `sub`'s stack failed at its tests (`failure`), before any PR
    /// is sent back for it: whether `main` passes here. When it fails too, `main` is red on this
    /// machine (a driver update, the machine itself): the off switch is set and nothing is sent
    /// back. A verify that didn't run sends nothing back either; the stack is gated again next
    /// round .
    fn main_passes(&mut self, sub: &str, failure: &blame::Failure) -> bool {
        let specs: Vec<String> = failure.tests.iter().map(|t| format!("{} {}", t.binary, t.name)).collect();
        let tests = specs.join(", ");
        if let Some(why) = unverifiable(&specs) {
            self.say(&format!("{sub} failed at {tests}: main not checked ({why}); blaming its PRs"));
            return true;
        }
        let (old, new): (Vec<&blame::FailedTest>, Vec<&blame::FailedTest>) = failure.tests.iter().partition(|t| self.w.on_main(leaf(&t.name)));
        if old.is_empty() {
            let new: Vec<String> = new.iter().map(|t| format!("{} {}", t.binary, t.name)).collect();
            self.say(&format!("{sub} failed at {}, which main doesn't have: the stack's; blaming its PRs", new.join(", ")));
            return true;
        }
        let specs: Vec<String> = old.iter().map(|t| format!("{} {}", t.binary, t.name)).collect();
        let v = self.w.next_sub();
        self.say(&format!("{sub} failed at {tests}: running {} alone on main as {v} before sending any PR back", specs.join(", ")));
        let run = self.w.gate_main(&v, &policy::gate_note(&Step::VerifyMain(sub.to_owned()), &[]), &specs);
        match run.ran {
            Ran::Passed(standing) => {
                self.w.close(&standing, "main passed: the failure is the stack's");
                self.say(&format!("{v} passed: those tests pass on main here, so {sub}'s failure is its PRs'"));
                true
            }
            Ran::NotRun => {
                self.w.drop_lock();
                self.say(&format!("{v} didn't run: nothing sent back for {sub}; gated again next round"));
                false
            }
            Ran::Failed(cause) if policy::machine_failure(cause) => {
                self.w.drop_lock();
                self.machine_failure(&v, cause);
                false
            }
            Ran::Failed(_) if run.log.contains(NO_TESTS) => {
                self.w.drop_lock();
                self.w.close(&v, "a failed test isn't on main: the failure is the stack's");
                self.say(&format!("{v}: a failed test isn't on main (\"{NO_TESTS}\"): {sub}'s failure is its PRs'"));
                true
            }
            Ran::Failed(_) => {
                self.w.drop_lock();
                let why = format!(
                    "main fails on this machine too ({v}, after {sub} failed at {tests}): nothing sent back; fix main or the machine, then `gate --daemon --resume`"
                );
                if let Err(e) = self.w.set_switch(&why) {
                    self.say(&format!("couldn't set the off switch: {e}"));
                }
                self.say(&format!("off switch set: {why}"));
                false
            }
        }
    }

    /// After gate `sub` of `gated` passed: when it was half of a bisection's suspects, the culprit
    /// is among the rest. One left is blamed (back to draft, a `sendback`); several are bisected
    /// next. `None`: nothing more to gate this round.
    fn after_pass(&mut self, gated: &[u64], sub: &str) -> Option<(Vec<u64>, Step)> {
        let s = self.state.suspects.take()?;
        if !gated.iter().all(|m| s.mrs.iter().any(|x| x.mr == *m)) {
            return None;
        }
        let rest: Vec<Suspect> = s.mrs.iter().filter(|x| !gated.contains(&x.mr)).cloned().collect();
        match rest.as_slice() {
            [] => None,
            [one] => {
                let why = format!("{} failed with it, and {sub} passed with the rest of {}'s suspects", s.of, s.of);
                self.drop_failed(one, &s.of, s.cause, &why, &s.excerpt);
                None
            }
            _ => {
                let iids: Vec<u64> = rest.iter().map(|x| x.mr).collect();
                let half = policy::bisect(&iids);
                self.say(&format!("{sub} passed: {}'s culprit is among {}; bisecting, gating {} next", s.of, policy::bangs(&iids), policy::bangs(&half)));
                let of = s.of.clone();
                self.state.suspects = Some(Suspects { mrs: rest, ..s });
                Some((half, Step::Bisect(of)))
            }
        }
    }

    /// A failure the machine caused: no PR's; after [`policy::MACHINE_FAILURES`] in a row, the
    /// off switch, for the coordinator.
    fn machine_failure(&mut self, sub: &str, cause: Option<Cause>) {
        self.machine_failures = self.machine_failures.saturating_add(1);
        let cause = cause.map_or("?", Cause::name);
        self.say(&format!("{sub} failed for the machine (cause {cause}): no PR dropped; gated again next round"));
        if self.machine_failures >= policy::MACHINE_FAILURES {
            let why = format!("{} gates in a row failed for the machine (last: {sub}, cause {cause}): check the machine", self.machine_failures);
            if let Err(e) = self.w.set_switch(&why) {
                self.say(&format!("couldn't set the off switch: {e}"));
            }
            self.say(&format!("off switch set: {why}"));
            self.machine_failures = 0;
        }
    }

    /// Drop `e`, which gate `sub`'s failure points at (`why`): back to draft with the log's
    /// excerpt, a `sendback` for each issue it closes, and not gated again at this head.
    fn drop_failed(&mut self, e: &Suspect, sub: &str, cause: Option<Cause>, why: &str, excerpt: &str) {
        let body = policy::failure_note(e.mr, sub, cause, why, excerpt);
        let how = match self.w.redraft(e.mr, &e.sha, &body) {
            Ok(true) => "put back to draft with a note".to_owned(),
            Ok(false) => "not put back to draft: it moved since".to_owned(),
            Err(err) => format!("couldn't put it back to draft: {err}"),
        };
        let send = format!("cause={}: {}", cause.map_or("unknown", Cause::name), crate::gate::one_line(why, 200));
        for issue in &e.issues {
            self.w.event(&["sendback", "--issue", &issue.to_string(), "--mr", &e.mr.to_string(), "--sub", sub, "--note", &send]);
        }
        self.state.dropped.insert(e.mr, e.sha.clone());
        self.say(&format!("dropped #{} at {}: {sub} failed and {why}; {how}", e.mr, short(&e.sha)));
    }

    /// Land the passed gate `sub`'s `stack`, but for a branch that pushed a change meanwhile and
    /// those above it; never while paused (`false`: paused, so gate nothing more).
    fn land(&mut self, stack: &Stack, sub: &str) -> bool {
        self.machine_failures = 0;
        if let Some(p) = self.pause_now() {
            self.say(&format!("{sub} passed, but not landed: {}", p.describe()));
            self.w.close(sub, "passed; not landed: the gate daemon paused");
            return false;
        }
        let mrs = entries(stack);
        let (land, wait) = policy::land_prefix(&mrs, &self.w.pushes(stack));
        if !wait.is_empty() {
            self.say(&format!(
                "{} pushed a change while {sub} ran: it and those above it wait for the next round",
                wait.first().map_or(String::new(), |m| format!("#{m}"))
            ));
        }
        if land.is_empty() {
            self.w.close(sub, "passed; landed nothing: a branch pushed a change while it ran");
            return true;
        }
        match self.w.land(&land, sub) {
            Ok(landed) => self.say(&format!("landed {} on {sub}", policy::bangs(&landed))),
            Err((why, landed)) => self.land_stopped(stack, sub, &why, &landed),
        }
        true
    }

    /// `land` stopped (`why`) after landing `landed`: the PR it stopped at is held for the
    /// coordinator when it stops there again at the same head.
    fn land_stopped(&mut self, stack: &Stack, sub: &str, why: &str, landed: &[u64]) {
        let at = stack.entries.iter().find(|e| !landed.contains(&e.mr));
        self.say(&format!("land on {sub} stopped after {}: {}", policy::bangs(landed), first_line(why)));
        let Some(e) = at else { return };
        let n = self.land_stops.entry((e.mr, e.old_tip.clone())).or_insert(0);
        *n = n.saturating_add(1);
        if *n >= LAND_STOPS {
            let why = format!("land stopped at it {n} times: {}", first_line(why));
            self.say(&format!("held #{} at {}: {why}", e.mr, short(&e.old_tip)));
            self.state.held.insert(e.mr, (e.old_tip.clone(), why));
        }
    }
}

/// nextest's error when a test run alone (`--no-tests fail`) matches no test: the test isn't
/// there .
const NO_TESTS: &str = "no tests to run";

/// A test's own name, the last segment of its path (`module::tests::name`).
fn leaf(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// Why `specs` can't be run alone on `main` (`gate --test-alone` takes them): none, too many, or
/// one it can't name plainly. `None`: they can.
fn unverifiable(specs: &[String]) -> Option<String> {
    if specs.len() > crate::gate::rerun::MAX_TESTS {
        return Some(format!("{} failed tests, more than {}", specs.len(), crate::gate::rerun::MAX_TESTS));
    }
    specs.iter().find_map(|s| crate::gate::rerun::Test::parse(s).err())
}

/// A stack's PRs, in order.
pub fn entries(stack: &Stack) -> Vec<u64> {
    stack.entries.iter().map(|e| e.mr).collect()
}

fn short(sha: &str) -> &str {
    crate::stack::short(sha)
}

fn first_line(text: &str) -> String {
    crate::gate::one_line(text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default(), 300)
}

/// Whether `dir` (the gate worktree) can be gated from: no rebase or merge in progress.
pub fn check_worktree(root: &Path) -> Result<(), String> {
    match crate::gate::tree_refusal(root) {
        Some(why) => Err(format!("gate --daemon: {why}")),
        None => Ok(()),
    }
}
