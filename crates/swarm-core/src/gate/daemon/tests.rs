//! The daemon's rounds against a fake [`World`]: the order of what it does, what it says and
//! what it leaves on GitHub and the timeline.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use serde_json::{Value, json};

use super::blame::Packages;
use super::policy::{Push, Ran};
use super::*;
use crate::project::fixtures::APP_SWARM as P;

fn sha(n: u64) -> String {
    format!("{n:040x}")
}

/// An open PR, ready: `!n`, head `sha(n)`, closing issue `#n+100`.
fn ready(n: u64) -> Value {
    json!({"number": n, "state": "open", "draft": false, "cross_repo": false, "base_ref": "main",
           "head_ref": format!("{n}-work"), "title": format!("PR {n}"), "body": format!("Closes #{}\n**Measured:** 1 s", n + 100),
           "head_sha": sha(n), "merge_state": "clean"})
}

#[derive(Default)]
struct Fake {
    now: Cell<i64>,
    said: RefCell<Vec<String>>,
    switch: RefCell<Option<String>>,
    red: RefCell<Vec<(u64, String)>>,
    open: RefCell<Vec<Value>>,
    /// A head's commits beyond `main`, beyond itself.
    extra_commits: RefCell<BTreeMap<String, Vec<String>>>,
    main_conflicts: RefCell<BTreeMap<String, Vec<String>>>,
    notes: RefCell<Vec<(u64, String)>>,
    redrafts: RefCell<Vec<(u64, String)>>,
    events: RefCell<Vec<String>>,
    /// `stack` stops at these PRs, with this why.
    stops: RefCell<BTreeMap<u64, String>>,
    stacks: RefCell<Vec<Vec<u64>>>,
    current: RefCell<Stack>,
    room: Cell<bool>,
    gate_results: RefCell<VecDeque<GateRun>>,
    gates: RefCell<Vec<(String, String, Vec<u64>)>>,
    touched: RefCell<BTreeMap<u64, Vec<String>>>,
    pushes: RefCell<BTreeMap<u64, Push>>,
    /// `land` stops at this PR.
    land_stop: RefCell<Option<u64>>,
    lands: RefCell<Vec<(Vec<u64>, String)>>,
    closes: RefCell<Vec<(String, String)>>,
    lock_drops: Cell<u32>,
    github_down: Cell<bool>,
    /// The log of each state saved.
    saved: RefCell<Vec<Vec<String>>>,
    /// The tests each check of `main` ran alone.
    verified: RefCell<Vec<Vec<String>>>,
    /// Test names `main` lacks (new in the stack).
    not_on_main: RefCell<Vec<String>>,
    /// The last decision saved when each gate began.
    saved_at_gate: RefCell<Vec<String>>,
}

impl Fake {
    fn with(open: &[u64]) -> Fake {
        let f = Fake { room: Cell::new(true), now: Cell::new(1_000), ..Fake::default() };
        *f.open.borrow_mut() = open.iter().map(|n| ready(*n)).collect();
        f
    }

    fn pass(&self, sub: &str) {
        self.gate_results.borrow_mut().push_back(GateRun { ran: Ran::Passed(sub.into()), log: String::new() });
    }

    fn fail(&self, cause: Option<Cause>, log: &str) {
        self.gate_results.borrow_mut().push_back(GateRun { ran: Ran::Failed(cause), log: log.into() });
    }

    fn said_has(&self, needle: &str) -> bool {
        self.said.borrow().iter().any(|l| l.contains(needle))
    }

    fn gated(&self) -> Vec<Vec<u64>> {
        self.gates.borrow().iter().map(|(_, _, m)| m.clone()).collect()
    }

    /// The daemon's text for each gate's `sub.start` note, in order.
    fn notes(&self) -> Vec<String> {
        self.gates.borrow().iter().map(|(_, n, _)| n.clone()).collect()
    }
}

impl World for Fake {
    fn now(&self) -> i64 {
        self.now.get()
    }
    fn say(&self, line: &str) {
        self.said.borrow_mut().push(line.to_owned());
    }
    fn sleep(&self, _: Duration) {}
    fn switch(&self) -> Option<String> {
        self.switch.borrow().clone()
    }
    fn set_switch(&self, why: &str) -> Result<(), String> {
        *self.switch.borrow_mut() = Some(why.to_owned());
        Ok(())
    }
    fn save(&self, state: &State) -> Result<(), String> {
        self.saved.borrow_mut().push(state.log.clone());
        Ok(())
    }
    fn main_red(&self) -> Result<Vec<(u64, String)>, String> {
        if self.github_down.get() {
            return Err("gh: HTTP 502".into());
        }
        Ok(self.red.borrow().clone())
    }
    fn open_mrs(&self) -> Result<Vec<Value>, String> {
        Ok(self.open.borrow().clone())
    }
    fn commits(&self, head: &str) -> Result<BTreeSet<String>, String> {
        let mut set = BTreeSet::from([head.to_owned()]);
        set.extend(self.extra_commits.borrow().get(head).cloned().unwrap_or_default());
        Ok(set)
    }
    fn main_conflict(&self, head: &str) -> Result<Vec<String>, String> {
        Ok(self.main_conflicts.borrow().get(head).cloned().unwrap_or_default())
    }
    fn landed_after(&self, _: &[String]) -> Option<u64> {
        Some(862)
    }
    fn note(&self, mr: u64, body: &str) -> Result<(), String> {
        self.notes.borrow_mut().push((mr, body.to_owned()));
        Ok(())
    }
    fn redraft(&self, mr: u64, _: &str, body: &str) -> Result<bool, String> {
        self.redrafts.borrow_mut().push((mr, body.to_owned()));
        Ok(true)
    }
    fn event(&self, args: &[&str]) {
        self.events.borrow_mut().push(args.join(" "));
    }
    fn stack(&self, mrs: &[StackMr]) -> StackRun {
        self.stacks.borrow_mut().push(mrs.iter().map(|m| m.iid).collect());
        let mut stack = Stack { remote: "origin".into(), base: "main".into(), entries: Vec::new() };
        for m in mrs {
            if let Some(why) = self.stops.borrow().get(&m.iid) {
                return StackRun::Stopped { stack, at: m.iid, why: why.clone() };
            }
            stack.entries.push(Entry {
                mr: m.iid,
                source_branch: m.source_branch.clone(),
                title: m.title.clone(),
                issues: m.issues.clone(),
                old_tip: sha(m.iid),
                new_tip: format!("{:040x}", m.iid + 0x1000),
                ..Entry::default()
            });
        }
        *self.current.borrow_mut() = stack.clone();
        StackRun::Built(stack)
    }
    fn room(&self) -> bool {
        self.room.get()
    }
    fn next_sub(&self) -> String {
        format!("gate-{}", self.gates.borrow().len() + 1)
    }
    fn gate_main(&self, sub: &str, note: &str, tests: &[String]) -> GateRun {
        self.verified.borrow_mut().push(tests.to_vec());
        self.gates.borrow_mut().push((sub.into(), note.into(), Vec::new()));
        self.gate_results.borrow_mut().pop_front().expect("a gate the test didn't script")
    }
    fn on_main(&self, name: &str) -> bool {
        !self.not_on_main.borrow().iter().any(|n| n == name)
    }
    fn gate(&self, sub: &str, note: &str) -> GateRun {
        self.gates.borrow_mut().push((sub.into(), note.into(), entries(&self.current.borrow())));
        let last = self.saved.borrow().last().and_then(|l| l.last().cloned()).unwrap_or_default();
        self.saved_at_gate.borrow_mut().push(last);
        self.gate_results.borrow_mut().pop_front().expect("a gate the test didn't script")
    }
    fn touched(&self, e: &Entry) -> Vec<String> {
        self.touched.borrow().get(&e.mr).cloned().unwrap_or_default()
    }
    fn packages(&self) -> Packages {
        Packages(vec![("my-engine".into(), "crates/engine".into())])
    }
    fn root(&self) -> String {
        "/gate".into()
    }
    fn pushes(&self, stack: &Stack) -> Vec<Push> {
        stack.entries.iter().map(|e| self.pushes.borrow().get(&e.mr).cloned().unwrap_or(Push::None)).collect()
    }
    fn land(&self, mrs: &[u64], sub: &str) -> Result<Vec<u64>, (String, Vec<u64>)> {
        self.lands.borrow_mut().push((mrs.to_vec(), sub.into()));
        match *self.land_stop.borrow() {
            Some(at) if mrs.contains(&at) => {
                let landed: Vec<u64> = mrs.iter().copied().take_while(|m| *m != at).collect();
                Err((format!("land: #{at} stopped: the server refused"), landed))
            }
            _ => Ok(mrs.to_vec()),
        }
    }
    fn close(&self, sub: &str, note: &str) {
        self.closes.borrow_mut().push((sub.into(), note.into()));
    }
    fn drop_lock(&self) {
        self.lock_drops.set(self.lock_drops.get() + 1);
    }
}

const SETTINGS: Settings = Settings { dry_run: false, fold: Duration::from_secs(120), verify_main: false };

fn daemon(f: &Fake) -> Daemon<'_> {
    Daemon::new(&P, f, SETTINGS, State::default())
}

#[test]
fn a_ready_stack_is_stacked_gated_and_landed() {
    let f = Fake::with(&[1, 2]);
    f.pass("gate-1");
    let mut d = daemon(&f);
    assert_eq!(d.round(), Round::Gated);
    assert_eq!(*f.stacks.borrow(), vec![vec![1, 2]]);
    assert_eq!(f.gated(), vec![vec![1, 2]]);
    assert_eq!(f.notes(), ["daemon gate: #1 #2"]);
    assert_eq!(*f.lands.borrow(), vec![(vec![1, 2], "gate-1".to_owned())]);
    assert!(f.said_has("landed #1 #2 on gate-1"));
    assert!(f.events.borrow().is_empty(), "land logs the merges: {:?}", f.events.borrow());
    // A log line for --status, with its time.
    assert!(d.state.log.iter().any(|l| l.starts_with("1970-01-01T00:16:40Z landed")), "{:?}", d.state.log);
}

#[test]
fn nothing_ready_is_idle_and_said_once() {
    let f = Fake::with(&[]);
    let mut d = daemon(&f);
    assert_eq!(d.round(), Round::Idle);
    assert_eq!(d.round(), Round::Idle);
    assert_eq!(f.said.borrow().iter().filter(|l| l.starts_with("idle")).count(), 1);
}

#[test]
fn it_pauses_once_per_change_and_resumes() {
    let f = Fake::with(&[1]);
    *f.switch.borrow_mut() = Some("owner asked".into());
    let mut d = daemon(&f);
    assert_eq!(d.round(), Round::Paused);
    assert_eq!(d.round(), Round::Paused);
    *f.switch.borrow_mut() = None;
    *f.red.borrow_mut() = vec![(77, "main fails".into())];
    assert_eq!(d.round(), Round::Paused);
    *f.red.borrow_mut() = vec![];
    f.pass("gate-1");
    assert_eq!(d.round(), Round::Gated);
    let pauses: Vec<String> = f.said.borrow().iter().filter(|l| l.starts_with("paused") || l.starts_with("resumed")).cloned().collect();
    assert_eq!(pauses.len(), 3, "{pauses:?}");
    assert!(pauses[0].contains("owner asked") && pauses[1].contains("#77") && pauses[2].starts_with("resumed"));
    assert!(f.stacks.borrow().len() == 1, "nothing stacked while paused");
}

#[test]
fn it_pauses_while_github_cant_say_whether_main_is_red() {
    let f = Fake::with(&[1]);
    f.github_down.set(true);
    let mut d = daemon(&f);
    assert_eq!(d.round(), Round::Paused);
    assert!(f.said_has("couldn't check main") && f.stacks.borrow().is_empty());
    f.github_down.set(false);
    f.pass("gate-1");
    assert_eq!(d.round(), Round::Gated);
}

#[test]
fn a_pr_github_says_conflicts_is_noted_once_per_head_and_left_out() {
    let f = Fake::with(&[1, 2]);
    f.open.borrow_mut()[0]["merge_state"] = json!("dirty");
    f.main_conflicts.borrow_mut().insert(sha(1), vec!["src/a.rs".into(), "docs/scorecard.md".into()]);
    f.pass("gate-1");
    f.pass("gate-2");
    let mut d = daemon(&f);
    d.round();
    d.round();
    assert_eq!(f.gated(), vec![vec![2], vec![2]]);
    let notes = f.notes.borrow();
    assert_eq!(notes.len(), 1, "once per head");
    // The generated doc is regenerated by `stack`: not named.
    assert_eq!(notes[0], (1, "Gate daemon: conflicts with main in `src/a.rs` (after #862 landed): rebase onto main and run `cargo xtask ready`.".to_owned()));
    assert_eq!(*f.events.borrow(), vec!["sendback --issue 101 --mr 1 --note cause=conflict: src/a.rs".to_owned()]);
    drop(notes);
    // A push is a new head: noted again when it still conflicts.
    f.open.borrow_mut()[0]["head_sha"] = json!(sha(11));
    f.main_conflicts.borrow_mut().insert(sha(11), vec!["src/a.rs".into()]);
    f.pass("gate-3");
    d.round();
    assert_eq!(f.notes.borrow().len(), 2);
}

#[test]
fn a_stack_conflict_with_main_drops_the_mr_and_those_on_it() {
    // #3's branch holds #2's commits; #2 conflicts with main.
    let f = Fake::with(&[1, 2, 3]);
    f.extra_commits.borrow_mut().insert(sha(3), vec![sha(2)]);
    f.stops.borrow_mut().insert(2, "conflicts outside the progress and generated docs, resolve by hand: src/b.rs".into());
    f.main_conflicts.borrow_mut().insert(sha(2), vec!["src/b.rs".into()]);
    f.pass("gate-1");
    let mut d = daemon(&f);
    d.round();
    assert_eq!(*f.stacks.borrow(), vec![vec![1, 2, 3], vec![1]]);
    assert_eq!(f.gated(), vec![vec![1]]);
    assert_eq!(f.notes.borrow().len(), 1);
    assert!(f.said_has("dropped #2"));
}

#[test]
fn a_conflict_with_the_mrs_below_only_waits_for_them() {
    let f = Fake::with(&[1, 2]);
    f.stops.borrow_mut().insert(2, "conflicts outside the progress and generated docs, resolve by hand: src/b.rs".into());
    f.pass("gate-1");
    let mut d = daemon(&f);
    d.round();
    assert_eq!(f.gated(), vec![vec![1]]);
    assert!(f.notes.borrow().is_empty() && f.events.borrow().is_empty());
    assert!(f.said_has("#2 conflicts with the PRs stacked below it"));
    // An PR the stack refuses on its own (no work, say) is held at its head.
    let g = Fake::with(&[5]);
    g.stops.borrow_mut().insert(5, "#5 (5-work): it holds no work".into());
    let mut d = daemon(&g);
    assert_eq!(d.round(), Round::Idle);
    assert_eq!(d.state.held.get(&5).map(|(s, _)| s.clone()), Some(sha(5)));
    assert_eq!(d.round(), Round::Idle);
    assert_eq!(g.stacks.borrow().len(), 1, "not stacked again at the same head");
}

#[test]
fn while_there_is_no_room_new_mrs_are_folded_in_then_it_gates() {
    let f = Fake::with(&[1]);
    f.room.set(false);
    let mut d = daemon(&f);
    assert_eq!(d.round(), Round::Waiting);
    assert_eq!(d.round(), Round::Waiting);
    assert_eq!(f.stacks.borrow().len(), 1, "the same PRs at the same heads aren't restacked");
    f.open.borrow_mut().push(ready(2));
    f.now.set(1_060);
    assert_eq!(d.round(), Round::Waiting);
    assert_eq!(*f.stacks.borrow(), vec![vec![1], vec![1, 2]]);
    // The fold's time is up: the gate runs (and waits itself, frozen).
    f.now.set(1_121);
    f.pass("gate-1");
    assert_eq!(d.round(), Round::Gated);
    assert_eq!(f.gated(), vec![vec![1, 2]]);
}

#[test]
fn a_failure_in_one_mrs_file_drops_it_and_gates_the_rest() {
    let f = Fake::with(&[1, 2, 3]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/src/cmd.rs".into()]);
    f.touched.borrow_mut().insert(3, vec!["crates/engine/src/other.rs".into()]);
    f.fail(Some(Cause::Build), "error[E0308]: mismatched types\n  --> crates/engine/src/cmd.rs:3:1\nerror: could not compile `my-engine`\n");
    f.pass("gate-2");
    let mut d = daemon(&f);
    d.round();
    assert_eq!(f.gated(), vec![vec![1, 2, 3], vec![1, 3]]);
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3", "regate of gate-1: #1 #3"], "the regate never names the PR it dropped");
    let r = f.redrafts.borrow();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, 2);
    assert!(r[0].1.contains("`crates/engine/src/cmd.rs`") && r[0].1.contains("error[E0308]"), "{}", r[0].1);
    assert_eq!(
        *f.events.borrow(),
        vec![
            "sendback --issue 102 --mr 2 --sub gate-1 --note cause=build: it changed `crates/engine/src/cmd.rs`, where the build or a check failed".to_owned()
        ]
    );
    assert_eq!(*f.lands.borrow(), vec![(vec![1, 3], "gate-2".to_owned())]);
    assert_eq!(d.state.dropped.get(&2), Some(&sha(2)));
    assert_eq!(f.lock_drops.get(), 1, "the failed gate's lock");
    // Not gated again at that head.
    f.pass("gate-3");
    *f.open.borrow_mut() = vec![ready(2), ready(4)];
    d.round();
    assert_eq!(f.gated().last(), Some(&vec![4]));
}

#[test]
fn a_failure_no_mr_matches_bisects_and_never_lands_the_failed_stack() {
    let f = Fake::with(&[1, 2, 3, 4]);
    f.fail(Some(Cause::Test), "        FAIL [   1.2s] other-pkg::it some::test\n");
    f.pass("gate-2");
    f.pass("gate-3");
    let mut d = daemon(&f);
    d.round();
    // gate-2 passed: the culprit is #3 or #4. gate-3 (#3) passed: it is #4.
    assert_eq!(f.gated(), vec![vec![1, 2, 3, 4], vec![1, 2], vec![3]]);
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3 #4", "bisect of gate-1: #1 #2", "bisect of gate-1: #3"]);
    assert_eq!(*f.lands.borrow(), vec![(vec![1, 2], "gate-2".to_owned()), (vec![3], "gate-3".to_owned())]);
    assert_eq!(f.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [4]);
    assert!(f.redrafts.borrow()[0].1.contains("gate-1 failed with it, and gate-3 passed"), "{:?}", f.redrafts.borrow());
    assert_eq!(d.state.suspects, None);
    // A stack of one that fails is that PR's.
    let g = Fake::with(&[7]);
    g.fail(Some(Cause::Other), "something\n");
    let mut d = daemon(&g);
    d.round();
    assert_eq!(g.redrafts.borrow().len(), 1);
    assert!(g.lands.borrow().is_empty());
    assert!(g.redrafts.borrow()[0].1.contains("the only PR in the stack"));
}

/// The `sendback` the daemon logs for `!mr` (closing `#mr+100`), dropped for gate `sub`'s build
/// failure in `crates/engine/src/cmd.rs`.
fn cmd_rs_sendback(mr: u64, sub: &str) -> String {
    format!("sendback --issue {} --mr {mr} --sub {sub} --note cause=build: it changed `crates/engine/src/cmd.rs`, where the build or a check failed", mr + 100)
}

const CMD_RS_ERROR: &str = "error[E0308]: mismatched types\n  --> crates/engine/src/cmd.rs:3:1\nerror: could not compile `my-engine`\n";

// #43: a regate or a bisection sets PRs aside without sending them back. The timeline holds a
// `sendback` for the PR a failure is blamed on and for no other, and each gate's note names its
// step and the PRs it gates, so a reader counts only the culprit as sent back.

#[test]
fn a_regate_sets_aside_what_is_stacked_on_the_culprit_without_a_sendback() {
    // #3's branch holds #2's commits; the failure is #2's.
    let f = Fake::with(&[1, 2, 3]);
    f.extra_commits.borrow_mut().insert(sha(3), vec![sha(2)]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/src/cmd.rs".into()]);
    f.fail(Some(Cause::Build), CMD_RS_ERROR);
    f.pass("gate-2");
    let mut d = daemon(&f);
    d.round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3", "regate of gate-1: #1"]);
    assert_eq!(*f.events.borrow(), [cmd_rs_sendback(2, "gate-1")], "#1 and #3 are not sent back");
    assert_eq!(*f.lands.borrow(), [(vec![1], "gate-2".to_owned())]);
    assert_eq!(d.state.dropped.keys().copied().collect::<Vec<_>>(), [2], "#3 waits on #2, not dropped");
}

#[test]
fn a_bisection_that_loses_a_suspect_ends_and_sends_none_back() {
    let f = Fake::with(&[1, 2, 3, 4]);
    f.fail(Some(Cause::Test), "        FAIL [   1.2s] other-pkg::it some::test\n");
    f.pass("gate-2");
    f.gate_results.borrow_mut().push_back(GateRun { ran: Ran::NotRun, log: String::new() });
    let mut d = daemon(&f);
    d.round();
    assert_eq!(d.state.suspects.as_ref().map(Suspects::iids), Some(vec![3, 4]), "gate-3 didn't run: the suspects wait");
    // #3 pushed: the culprit may be gone, so the bisection ends and the rest is gated as usual.
    *f.open.borrow_mut() = vec![ready(3), ready(4)];
    f.open.borrow_mut()[0]["head_sha"] = json!(sha(13));
    f.pass("gate-4");
    d.round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3 #4", "bisect of gate-1: #1 #2", "bisect of gate-1: #3", "daemon gate: #3 #4"]);
    assert!(f.said_has("the bisection of gate-1 ends: #3 moved"));
    assert!(f.events.borrow().is_empty(), "no PR was sent back: {:?}", f.events.borrow());
    assert!(f.redrafts.borrow().is_empty() && d.state.dropped.is_empty() && d.state.suspects.is_none());
}

#[test]
fn a_bisection_whose_half_passes_blames_the_one_suspect_left() {
    // a swarm's gate-500 (#46): gate-1 and its first half both fail with nothing to blame; the
    // half's first half passes and lands. The one left is the culprit: dropped with gate-2's
    // excerpt, never stacked again with the others.
    let f = Fake::with(&[1, 2, 3, 4]);
    f.fail(Some(Cause::Test), "        FAIL [   1.2s] other-pkg::it some::test\n");
    f.fail(Some(Cause::Test), "        FAIL [   1.2s] other-pkg::it some::test\n");
    f.pass("gate-3");
    let mut d = daemon(&f);
    d.round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3 #4", "bisect of gate-1: #1 #2", "bisect of gate-2: #1"]);
    assert_eq!(*f.lands.borrow(), [(vec![1], "gate-3".to_owned())]);
    assert_eq!(f.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [2]);
    let events = f.events.borrow().clone();
    assert_eq!(events.len(), 1);
    assert!(events[0].starts_with("sendback --issue 102 --mr 2 --sub gate-2 --note cause=test: gate-2 failed with it"), "{events:?}");
    assert!(f.said_has("dropped #2 at"));
    // Next round: #2 is a draft; #3 and #4 are gated as a stack of their own.
    *f.open.borrow_mut() = vec![ready(3), ready(4)];
    f.pass("gate-4");
    d.round();
    assert_eq!(f.notes().last().map(String::as_str), Some("daemon gate: #3 #4"));
    assert_eq!(f.events.borrow().len(), 1, "only #2 was sent back");
}

#[test]
fn a_bisection_goes_on_in_the_next_round_with_half_its_suspects() {
    let f = Fake::with(&[1, 2, 3, 4]);
    f.fail(Some(Cause::Test), "        FAIL [   1.2s] other-pkg::it some::test\n");
    f.gate_results.borrow_mut().push_back(GateRun { ran: Ran::NotRun, log: String::new() });
    let mut d = daemon(&f);
    d.round();
    // Restarted from its saved state, with a new PR ready: it gates half the suspects, not a
    // fresh stack, and #5 waits.
    d = Daemon::new(&P, &f, SETTINGS, State::from_json(&d.state.to_json()));
    f.open.borrow_mut().push(ready(5));
    f.pass("gate-3");
    f.fail(Some(Cause::Test), "        FAIL [   1.2s] other-pkg::it some::test\n");
    d.round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3 #4", "bisect of gate-1: #1 #2", "bisect of gate-1: #1 #2", "bisect of gate-1: #3"]);
    // gate-4, #3 alone, failed: it is #3's.
    assert_eq!(f.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [3]);
    assert!(d.state.suspects.is_none());
    *f.open.borrow_mut() = vec![ready(4), ready(5)];
    f.pass("gate-5");
    d.round();
    assert_eq!(f.notes().last().map(String::as_str), Some("daemon gate: #4 #5"));
}

#[test]
fn a_bisection_that_blames_a_culprit_sends_back_only_it() {
    // #2 and #3 both changed the file the build fails in: gate-1 points at several, so it bisects.
    let f = Fake::with(&[1, 2, 3, 4]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/src/cmd.rs".into()]);
    f.touched.borrow_mut().insert(3, vec!["crates/engine/src/cmd.rs".into()]);
    f.fail(Some(Cause::Build), CMD_RS_ERROR);
    // gate-2, the first half (#1 #2), fails the same way: #2 alone changed the file there.
    f.fail(Some(Cause::Build), CMD_RS_ERROR);
    f.pass("gate-3");
    let mut d = daemon(&f);
    d.round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3 #4", "bisect of gate-1: #1 #2", "regate of gate-2: #1"]);
    assert_eq!(*f.events.borrow(), [cmd_rs_sendback(2, "gate-2")]);
    assert_eq!(f.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [2]);
    assert_eq!(*f.lands.borrow(), [(vec![1], "gate-3".to_owned())]);
    // #3 and #4, set aside by the bisection, are gated next round: still only #2 sent back.
    f.pass("gate-4");
    *f.open.borrow_mut() = vec![ready(3), ready(4)];
    d.round();
    assert_eq!(f.notes().last().map(String::as_str), Some("daemon gate: #3 #4"));
    assert_eq!(*f.events.borrow(), [cmd_rs_sendback(2, "gate-2")]);
}

#[test]
fn machine_failures_drop_nothing_and_three_set_the_switch() {
    let f = Fake::with(&[1]);
    let mut d = daemon(&f);
    for _ in 0..3 {
        f.fail(Some(Cause::Memory), "os error 1455");
        d.round();
    }
    assert!(f.redrafts.borrow().is_empty());
    assert!(f.switch.borrow().as_deref().is_some_and(|s| s.contains("3 gates in a row")), "{:?}", f.switch.borrow());
    assert_eq!(d.round(), Round::Paused);
}

#[test]
fn a_push_during_the_gate_waits_with_what_is_above_it() {
    let f = Fake::with(&[1, 2, 3]);
    f.pushes.borrow_mut().insert(1, Push::Rebased);
    f.pushes.borrow_mut().insert(2, Push::Changed);
    f.pass("gate-1");
    let mut d = daemon(&f);
    d.round();
    assert_eq!(*f.lands.borrow(), vec![(vec![1], "gate-1".to_owned())]);
    assert!(f.said_has("#2 pushed a change while gate-1 ran"));
    // The first one changed: nothing lands and the gate is closed.
    let g = Fake::with(&[1]);
    g.pushes.borrow_mut().insert(1, Push::Changed);
    g.pass("gate-1");
    daemon(&g).round();
    assert!(g.lands.borrow().is_empty());
    assert_eq!(g.closes.borrow().len(), 1);
}

#[test]
fn a_dry_run_says_what_it_would_gate_and_changes_nothing() {
    let f = Fake::with(&[2, 1]);
    let mut d = Daemon::new(&P, &f, Settings { dry_run: true, ..SETTINGS }, State::default());
    assert_eq!(d.round(), Round::WouldGate(vec![1, 2]));
    assert!(f.stacks.borrow().is_empty() && f.gates.borrow().is_empty());
    assert!(f.saved.borrow().is_empty(), "a dry run saves no state: a live daemon's would be clobbered");
}

#[test]
fn a_dry_run_says_it_would_note_a_conflict_and_notes_nothing() {
    //: run beside a live daemon, a dry run posted a real note and
    // logged a `sendback`.
    let f = Fake::with(&[1, 2]);
    f.open.borrow_mut()[0]["merge_state"] = json!("dirty");
    f.main_conflicts.borrow_mut().insert(sha(1), vec!["src/a.rs".into()]);
    let mut d = Daemon::new(&P, &f, Settings { dry_run: true, ..SETTINGS }, State::default());
    assert_eq!(d.round(), Round::WouldGate(vec![2]));
    assert!(f.notes.borrow().is_empty() && f.events.borrow().is_empty() && f.saved.borrow().is_empty());
    assert!(f.said_has("dry run: would drop #1 at"), "{:?}", f.said.borrow());
    assert!(f.said_has("conflicts with main in src/a.rs; would note it"));
}

#[test]
fn the_state_is_saved_with_each_decision_so_status_shows_a_running_gate() {
    //: saved only at a round's end, `--status` showed the round
    // before for the whole of a 20-minute gate.
    let f = Fake::with(&[1]);
    f.pass("gate-1");
    daemon(&f).round();
    let at_gate = f.saved_at_gate.borrow();
    assert_eq!(at_gate.len(), 1);
    assert!(at_gate[0].ends_with("gating #1 as gate-1"), "{at_gate:?}");
}

#[test]
fn an_mr_on_a_drafts_commits_waits() {
    let f = Fake::with(&[1, 2]);
    let mut draft = ready(9);
    draft["draft"] = json!(true);
    f.open.borrow_mut().push(draft);
    f.extra_commits.borrow_mut().insert(sha(2), vec![sha(9)]);
    f.pass("gate-1");
    daemon(&f).round();
    assert_eq!(f.gated(), vec![vec![1]]);
    assert!(f.said_has("#2 waits: its branch holds #9's commits"));
}

#[test]
fn a_land_that_stops_twice_at_one_head_holds_it() {
    let f = Fake::with(&[1, 2]);
    *f.land_stop.borrow_mut() = Some(2);
    f.pass("gate-1");
    f.pass("gate-2");
    let mut d = daemon(&f);
    d.round();
    assert!(d.state.held.is_empty());
    *f.open.borrow_mut() = vec![ready(2)];
    d.round();
    assert_eq!(d.state.held.get(&2).map(|(s, _)| s.clone()), Some(sha(2)));
    f.pass("gate-3");
    assert_eq!(d.round(), Round::Idle);
}

#[test]
fn a_passed_gate_never_lands_once_main_went_red() {
    let f = Fake::with(&[1]);
    let mut d = daemon(&f);
    let plan = d.plan().unwrap();
    let stack = d.stacked_for(&plan).unwrap();
    *f.red.borrow_mut() = vec![(5, "red".into())];
    d.land(&stack, "gate-1");
    assert!(f.lands.borrow().is_empty());
    assert_eq!(f.closes.borrow().len(), 1);
}

#[test]
fn the_state_reads_back_what_it_wrote() {
    let mut s = State { paused: Some("paused: x".into()), ..State::default() };
    s.noted.insert(1, sha(1));
    s.dropped.insert(2, sha(2));
    s.held.insert(3, (sha(3), "why".into()));
    s.seen.insert(4, 1_000);
    s.log.push("t line".into());
    assert_eq!(State::from_json(&s.to_json()), s);
    assert_eq!(State::from_json(&json!({"noted": {"x": 1}, "log": 3})), State::default());
}

// #47: with `verify_main`, a stack's first failure at its tests gates `main` alone before any PR
// is sent back.

const VERIFY: Settings = Settings { verify_main: true, ..SETTINGS };
const D3D12_FAIL: &str = "        FAIL [   2.0s] (1/3) my-engine::replay d3d12::replays_a_frame\n";

#[test]
fn main_failing_here_too_sets_the_switch_and_sends_nothing_back() {
    // a GPU driver update broke tests on main itself, which no other machine's run
    // doesn't run; the daemon sent back two innocent PRs.
    let f = Fake::with(&[1, 2, 3]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/tests/replay/d3d12.rs".into()]);
    f.fail(Some(Cause::Test), D3D12_FAIL);
    f.fail(Some(Cause::Test), D3D12_FAIL);
    let mut d = Daemon::new(&P, &f, VERIFY, State::default());
    d.round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3", "verify main for gate-1"]);
    assert_eq!(f.gated(), vec![vec![1, 2, 3], vec![]], "the verify gates no PR");
    assert_eq!(*f.verified.borrow(), [vec!["my-engine::replay d3d12::replays_a_frame".to_owned()]], "it runs the failed tests on main (#48)");
    assert!(f.redrafts.borrow().is_empty() && f.events.borrow().is_empty() && d.state.dropped.is_empty());
    assert!(f.switch.borrow().as_deref().is_some_and(|w| w.contains("main fails on this machine too (gate-2, after gate-1 failed")), "{:?}", f.switch.borrow());
    // Paused until the coordinator resumes it.
    assert_eq!(d.round(), Round::Paused);
}

#[test]
fn main_passing_here_blames_the_stack_as_before() {
    let f = Fake::with(&[1, 2, 3]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/tests/replay/d3d12.rs".into()]);
    f.fail(Some(Cause::Test), D3D12_FAIL);
    f.pass("gate-2");
    f.pass("gate-3");
    let mut d = Daemon::new(&P, &f, VERIFY, State::default());
    d.round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2 #3", "verify main for gate-1", "regate of gate-1: #1 #3"]);
    assert_eq!(*f.closes.borrow(), [("gate-2".to_owned(), "main passed: the failure is the stack's".to_owned())]);
    assert_eq!(f.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [2]);
    assert_eq!(*f.lands.borrow(), [(vec![1, 3], "gate-3".to_owned())]);
    assert!(f.switch.borrow().is_none());
}

#[test]
fn only_a_first_failure_at_the_tests_is_verified() {
    // A build error names the PR's own file: no verify.
    let f = Fake::with(&[1, 2]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/src/cmd.rs".into()]);
    f.fail(Some(Cause::Build), CMD_RS_ERROR);
    f.pass("gate-2");
    Daemon::new(&P, &f, VERIFY, State::default()).round();
    assert_eq!(f.notes(), ["daemon gate: #1 #2", "regate of gate-1: #1"]);
    // A bisection's half that fails is not verified again.
    let g = Fake::with(&[1, 2, 3, 4]);
    g.fail(Some(Cause::Test), D3D12_FAIL);
    g.pass("gate-2");
    g.fail(Some(Cause::Test), D3D12_FAIL);
    g.pass("gate-4");
    Daemon::new(&P, &g, VERIFY, State::default()).round();
    assert_eq!(g.notes(), ["daemon gate: #1 #2 #3 #4", "verify main for gate-1", "bisect of gate-1: #1 #2", "bisect of gate-3: #1"]);
    assert_eq!(g.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [2]);
}

#[test]
fn tests_it_cant_name_plainly_are_not_checked_on_main() {
    // #48: `gate --test-alone` refuses a test it can't name plainly; the failure is blamed as
    // before #47 rather than never.
    let f = Fake::with(&[1, 2]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/tests/replay/d3d12.rs".into()]);
    f.fail(
        Some(Cause::Test),
        "        FAIL [   2.0s] (1/3) my-engine::replay d3d12::replays_a_frame
        FAIL [   2.0s] (2/3) my-engine::replay d3d12::case-with-dash
",
    );
    f.pass("gate-2");
    Daemon::new(&P, &f, VERIFY, State::default()).round();
    assert!(f.verified.borrow().is_empty());
    assert!(f.said_has("main not checked"), "{:?}", f.said.borrow());
    assert_eq!(f.notes(), ["daemon gate: #1 #2", "regate of gate-1: #1"]);
}

// #51: a failed test new in the stack is its own, never `main` failing.

#[test]
fn a_failed_test_main_lacks_is_the_stacks() {
    // a swarm's gate-544: the test was new in #973; run alone on main it matched nothing, and
    // the daemon paused for "main fails here too".
    let f = Fake::with(&[1, 2]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/tests/replay/d3d12.rs".into()]);
    f.not_on_main.borrow_mut().push("replays_a_frame".into());
    f.fail(Some(Cause::Test), D3D12_FAIL);
    f.pass("gate-2");
    let mut d = Daemon::new(&P, &f, VERIFY, State::default());
    d.round();
    assert!(f.verified.borrow().is_empty(), "nothing to check on main");
    assert!(f.said_has("which main doesn't have: the stack's"), "{:?}", f.said.borrow());
    assert_eq!(f.notes(), ["daemon gate: #1 #2", "regate of gate-1: #1"]);
    assert_eq!(f.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [2]);
    assert!(f.switch.borrow().is_none());
    // Of two failed tests, only the one main has is checked there.
    let g = Fake::with(&[1, 2]);
    g.not_on_main.borrow_mut().push("replays_a_frame".into());
    g.fail(
        Some(Cause::Test),
        "        FAIL [   2.0s] (1/3) my-engine::replay d3d12::replays_a_frame\n        FAIL [   2.0s] (2/3) my-engine::it scrubber::a_tooltip\n",
    );
    g.pass("gate-2");
    g.pass("gate-3");
    Daemon::new(&P, &g, VERIFY, State::default()).round();
    assert_eq!(*g.verified.borrow(), [vec!["my-engine::it scrubber::a_tooltip".to_owned()]]);
}

#[test]
fn a_check_that_finds_no_such_test_on_main_blames_the_stack() {
    // A test `fn` search misses (a macro's): nextest's own "no tests to run" says the same.
    let f = Fake::with(&[1, 2]);
    f.touched.borrow_mut().insert(2, vec!["crates/engine/tests/replay/d3d12.rs".into()]);
    f.fail(Some(Cause::Test), D3D12_FAIL);
    f.fail(Some(Cause::Test), "error: no tests to run\nerror: ci failed at `alone my-engine::replay d3d12::replays_a_frame`\n");
    f.pass("gate-3");
    let mut d = Daemon::new(&P, &f, VERIFY, State::default());
    d.round();
    assert!(f.switch.borrow().is_none(), "main isn't red: {:?}", f.switch.borrow());
    assert_eq!(f.notes(), ["daemon gate: #1 #2", "verify main for gate-1", "regate of gate-1: #1"]);
    assert_eq!(f.redrafts.borrow().iter().map(|(m, _)| *m).collect::<Vec<_>>(), [2]);
}

#[test]
fn a_pause_is_said_again_every_half_hour() {
    let f = Fake::with(&[1]);
    *f.switch.borrow_mut() = Some("checking the machine".into());
    let mut d = daemon(&f);
    d.round();
    f.now.set(f.now.get() + 10 * 60);
    d.round();
    assert_eq!(f.said.borrow().iter().filter(|l| l.contains("paused: the off switch is set")).count(), 1, "said once per change");
    f.now.set(f.now.get() + 25 * 60);
    d.round();
    assert!(f.said.borrow().last().is_some_and(|l| l.starts_with("still paused: the off switch is set (checking the machine")), "{:?}", f.said.borrow());
}
