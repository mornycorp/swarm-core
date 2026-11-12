//! The gate daemon's decisions, as pure functions ([`super`]): which PRs are ready to gate, when
//! to pause, in which order to stack them, what a push during a gate means, what lands and what
//! to gate after a failure. The daemon's I/O shell ([`super::World`]) feeds them facts.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::Project;
use crate::gate::Cause;
use crate::stack::{self, StackMr};

/// An open PR ready to gate, as the daemon sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub mr: StackMr,
    /// Its head commit.
    pub sha: String,
    /// GitHub says it conflicts with its target branch.
    pub conflict: bool,
    /// When the daemon first saw it ready at any head (seconds since the epoch): the order after
    /// dependencies.
    pub ready_since: i64,
}

impl Candidate {
    pub fn iid(&self) -> u64 {
        self.mr.iid
    }
}

/// The open PR `v` (a row of GitHub's open-PR list), when it is ready to gate: open, not a draft,
/// from this project, into `main`, its Progress filled in and its branch not a progress fold's
/// ([`crate::prwatch::ready_line`], the coordinator's ready watch's test), with a head commit.
/// `ready_since` is when the daemon first saw it ready.
pub fn candidate(project: &Project, v: &Value, ready_since: i64) -> Option<Candidate> {
    crate::prwatch::ready_line(v)?;
    if v.get("base_ref").and_then(Value::as_str) != Some(project.branches.main) {
        return None;
    }
    let mr = stack::parse_mr(project, v).ok()?;
    let sha = v.get("head_sha").and_then(Value::as_str).filter(|s| is_sha(s))?.to_owned();
    // `dirty` is GitHub saying the merge conflicts; `unknown` (not worked out yet) is no conflict.
    let conflict = v.get("merge_state").and_then(Value::as_str) == Some("dirty");
    Some(Candidate { mr, sha, conflict, ready_since })
}

/// A full commit id.
pub fn is_sha(s: &str) -> bool {
    (40..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Why the daemon pauses: the off switch, or an open `main-red` issue. GitHub runs no workflows
/// here (CI is local), so there is no pipeline of `main` to read: a red `main` is an issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pause {
    /// The off switch is set (`gate --daemon --stop`), with the reason it was given.
    Off(String),
    /// Open `main-red` issues.
    MainRed(Vec<u64>),
    /// `main` couldn't be checked (GitHub didn't answer): why.
    Unchecked(String),
}

impl Pause {
    /// The line the daemon says, once per change.
    pub fn describe(&self) -> String {
        match self {
            Pause::Off(why) if why.trim().is_empty() => "paused: the off switch is set (`gate --daemon --resume` clears it)".to_owned(),
            Pause::Off(why) => format!("paused: the off switch is set ({}; `gate --daemon --resume` clears it)", why.trim()),
            Pause::MainRed(issues) => {
                let list: Vec<String> = issues.iter().map(|n| format!("#{n}")).collect();
                format!("paused: main is red ({} open): nothing lands until it closes", list.join(" "))
            }
            Pause::Unchecked(why) => format!("paused: couldn't check main ({why}): nothing lands until it can"),
        }
    }
}

/// Whether to pause, from the off switch (its reason, when set) and the open `main-red` issues,
/// in that order.
pub fn pause(switch: Option<&str>, red: &[(u64, String)]) -> Option<Pause> {
    if let Some(why) = switch {
        return Some(Pause::Off(why.to_owned()));
    }
    (!red.is_empty()).then(|| Pause::MainRed(red.iter().map(|(n, _)| *n).collect()))
}

/// Which open PRs each candidate's branch holds commits of: `x` → the PRs whose head is among the
/// commits `x` adds to `main` (`commits`, by candidate). `open` is every open PR's number and head.
pub fn dependencies(candidates: &[Candidate], open: &[(u64, String)], commits: &BTreeMap<u64, BTreeSet<String>>) -> BTreeMap<u64, BTreeSet<u64>> {
    let mut deps = BTreeMap::new();
    for c in candidates {
        let Some(set) = commits.get(&c.iid()) else { continue };
        let on: BTreeSet<u64> = open.iter().filter(|(m, sha)| *m != c.iid() && *sha != c.sha && set.contains(sha)).map(|(m, _)| *m).collect();
        if !on.is_empty() {
            deps.insert(c.iid(), on);
        }
    }
    deps
}

/// The stacking order: an PR after every PR its branch holds commits of, then by the time it was
/// first seen ready, then by number. An PR that holds commits of an open PR that isn't a
/// candidate (a draft, say), or of one held so, is held, and why: its commits would land
/// unchecked.
pub fn order(candidates: &[Candidate], deps: &BTreeMap<u64, BTreeSet<u64>>) -> (Vec<u64>, Vec<(u64, String)>) {
    let ids: BTreeSet<u64> = candidates.iter().map(Candidate::iid).collect();
    let mut held: BTreeMap<u64, String> = BTreeMap::new();
    // Held, to a fixed point: holding one can hold what is stacked on it.
    loop {
        let more: Vec<(u64, String)> = candidates
            .iter()
            .filter(|c| !held.contains_key(&c.iid()))
            .filter_map(|c| {
                let on = deps.get(&c.iid())?;
                let bad = on.iter().find(|d| !ids.contains(d) || held.contains_key(d))?;
                Some((c.iid(), format!("its branch holds #{bad}'s commits, and #{bad} isn't ready to gate")))
            })
            .collect();
        if more.is_empty() {
            break;
        }
        held.extend(more);
    }
    let mut todo: Vec<&Candidate> = candidates.iter().filter(|c| !held.contains_key(&c.iid())).collect();
    todo.sort_by_key(|c| (c.ready_since, c.iid()));
    (topological(&todo, deps), held.into_iter().collect())
}

/// `todo` (sorted) with each PR after those it depends on (`deps`): Kahn's order, the earliest
/// ready first; a cycle (two branches holding each other's heads) keeps the sorted order.
fn topological(todo: &[&Candidate], deps: &BTreeMap<u64, BTreeSet<u64>>) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::with_capacity(todo.len());
    let ids: BTreeSet<u64> = todo.iter().map(|c| c.iid()).collect();
    while out.len() < todo.len() {
        let free = |c: &&&Candidate| !out.contains(&c.iid()) && deps.get(&c.iid()).is_none_or(|on| on.iter().all(|d| out.contains(d) || !ids.contains(d)));
        let next = todo.iter().find(free).or_else(|| todo.iter().find(|c| !out.contains(&c.iid())));
        let Some(c) = next else { break };
        out.push(c.iid());
    }
    out
}

/// `mr` and every PR stacked on it (whose branch holds its commits, directly or through another).
pub fn with_dependents(mr: u64, deps: &BTreeMap<u64, BTreeSet<u64>>) -> BTreeSet<u64> {
    let mut out = BTreeSet::from([mr]);
    loop {
        let more: Vec<u64> = deps.iter().filter(|(x, on)| !out.contains(*x) && on.iter().any(|d| out.contains(d))).map(|(x, _)| *x).collect();
        if more.is_empty() {
            return out;
        }
        out.extend(more);
    }
}

/// The note on an PR that conflicts with `main` in `files` (`after`: the PR whose landing made it
/// conflict, when one can be named).
pub fn conflict_note(files: &[String], after: Option<u64>) -> String {
    let after = after.map(|n| format!(" (after #{n} landed)")).unwrap_or_default();
    let files = if files.is_empty() { "files GitHub didn't name".to_owned() } else { files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ") };
    format!("Gate daemon: conflicts with main in {files}{after}: rebase onto main and run `cargo xtask ready`.")
}

/// The PR a merge commit's message names: GitHub's `Merge pull request #123 from owner/branch`,
/// its first line.
pub fn merge_request_number(message: &str) -> Option<u64> {
    let rest = message.lines().next()?.trim().strip_prefix("Merge pull request #")?;
    rest.split_whitespace().next()?.parse().ok()
}

/// The next gate's sub: `gate-<n>`, one past the highest `gate-<n>` a gate log in `names` (file
/// names in the gate-logs directory) has, its reruns' included (`gate-12-rerun.log`); the periodic
/// full gate's (`gate-full-…`) aren't numbered.
pub fn next_sub(names: &[String]) -> String {
    let n = names.iter().filter_map(|f| gate_number(f)).max().map_or(1, |n| n.saturating_add(1));
    format!("gate-{n}")
}

fn gate_number(file: &str) -> Option<u64> {
    let rest = file.strip_prefix("gate-")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let after = rest.get(digits.len()..)?;
    (after.starts_with('.') || after.starts_with('-')).then_some(())?;
    digits.parse().ok()
}

/// What a gate run left in its records: its `<sub>.rc` and `<sub>-rerun.rc` (each `None` when it
/// has none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ran {
    /// It passed, as this sub (the rerun's when only that passed): `land` lands on it.
    Passed(String),
    /// It failed, and still failed its rerun if it had one: the cause the last run names.
    Failed(Option<Cause>),
    /// It never ran (`rc` missing): refused, or stopped before `ci`.
    NotRun,
}

/// [`Ran`] from gate `sub`'s records.
pub fn ran(sub: &str, rc: Option<&str>, rerun_rc: Option<&str>) -> Ran {
    let passed = |l: &str| crate::gate::passed_head(l).is_ok();
    let cause = |l: &str| crate::gate::field(l, "cause").and_then(Cause::parse);
    match (rc, rerun_rc) {
        (None, _) => Ran::NotRun,
        (Some(l), _) if passed(l) => Ran::Passed(sub.to_owned()),
        (Some(_), Some(r)) if passed(r) => Ran::Passed(format!("{sub}-rerun")),
        (Some(_), Some(r)) => Ran::Failed(cause(r)),
        (Some(l), None) => Ran::Failed(cause(l)),
    }
}

/// A failure the machine caused (a transient cause the gate's own rerun didn't clear, or the
/// gate machine's setup): no PR is to blame, and nothing is dropped.
pub fn machine_failure(cause: Option<Cause>) -> bool {
    cause.is_some_and(|c| c.transient() || c.setup())
}

/// The machine failures in a row after which the daemon sets its off switch: the machine needs
/// the coordinator.
pub const MACHINE_FAILURES: u32 = 3;

/// What a stacked PR's source branch did while its gate ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Push {
    /// No push: still the head that was stacked (or the stacked commit itself).
    None,
    /// Pushed, but the same patch: a rebase. `land` lands the gated tree.
    Rebased,
    /// Pushed a change: the gate didn't test it.
    Changed,
    /// The branch is gone.
    Gone,
}

/// [`Push`] from the branch's head now (`now`), the head stacked (`old`), the stacked commit
/// (`stacked`), and the patch-ids of each head's own change against `main` (`old_patch`,
/// `now_patch`; `None`: no change, or it couldn't be read).
pub fn push(now: Option<&str>, old: &str, stacked: &str, old_patch: Option<&str>, now_patch: Option<&str>) -> Push {
    match now {
        None => Push::Gone,
        Some(n) if n == old || n == stacked => Push::None,
        Some(_) if old_patch.is_some() && old_patch == now_patch => Push::Rebased,
        Some(_) => Push::Changed,
    }
}

/// What lands of a gated stack (`mrs`, in order, with what each branch did meanwhile): every PR
/// below the first that pushed a change (or lost its branch); that one and those above it wait
/// for the next round.
pub fn land_prefix(mrs: &[u64], pushes: &[Push]) -> (Vec<u64>, Vec<u64>) {
    let stop = pushes.iter().position(|p| matches!(p, Push::Changed | Push::Gone)).unwrap_or(mrs.len()).min(mrs.len());
    let (land, wait) = mrs.split_at(stop);
    (land.to_vec(), wait.to_vec())
}

/// The first half of a failed stack to gate next when no one PR is to blame: at least one.
pub fn bisect(mrs: &[u64]) -> Vec<u64> {
    let half = (mrs.len() / 2).max(1);
    mrs.get(..half).unwrap_or(mrs).to_vec()
}

/// `#1 #2`.
pub fn bangs(mrs: &[u64]) -> String {
    mrs.iter().map(|m| format!("#{m}")).collect::<Vec<_>>().join(" ")
}

/// Which of a round's gates the daemon runs: its `sub.start` note says so first
/// ([`gate_note`]; docs/tracking-format.md › Gates,.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A stack's first gate.
    First,
    /// What is left of gate `.0`'s stack once the PR its failure points at, and those stacked
    /// on it, were dropped.
    Regate(String),
    /// The first half of gate `.0`'s stack, when its failure points at no one PR.
    Bisect(String),
    /// `main` alone, after gate `.0`'s stack failed at its tests (#47): it gates no PR.
    VerifyMain(String),
}

/// The daemon's text for a gate's `sub.start` note (the gate adds its kind and capacity): its
/// step, then the PRs it gates, and no other: `daemon gate: #1 #2`, `regate of gate-3: #1`,
/// `bisect of gate-3: #1 #2`. A reader takes every `#<m>` a gate's note names as gated by it, so a
/// regate never names the PR it dropped.
pub fn gate_note(step: &Step, mrs: &[u64]) -> String {
    let list = bangs(mrs);
    match step {
        Step::First => format!("daemon gate: {list}"),
        Step::Regate(of) => format!("regate of {of}: {list}"),
        Step::Bisect(of) => format!("bisect of {of}: {list}"),
        Step::VerifyMain(of) => format!("verify main for {of}"),
    }
}

/// The note on an PR the daemon put back to draft for gate `sub`'s failure (`cause`), the
/// evidence that names it (`why`) and the log's excerpt.
pub fn failure_note(iid: u64, sub: &str, cause: Option<Cause>, why: &str, excerpt: &str) -> String {
    let cause = cause.map_or("unknown", Cause::name);
    let excerpt: String = excerpt.chars().take(2500).collect();
    format!(
        "Put back to draft by the gate daemon (`cargo xtask gate --daemon`): gate `{sub}` failed (cause `{cause}`), and the failure points at this PR: {why}.\n\n```\n{}\n```\n\n\
         Fix it, then run `cargo xtask ready {iid} --agent <your instance id>`: that marks it ready, and the daemon gates it again. The rest of the stack was gated without it.",
        excerpt.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::APP_SWARM as P;
    use serde_json::json;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const D: &str = "dddddddddddddddddddddddddddddddddddddddd";

    fn row(iid: u64, sha: &str) -> Value {
        json!({"number": iid, "state": "open", "draft": false, "cross_repo": false, "base_ref": "main",
               "head_ref": format!("{iid}-work"), "title": format!("PR {iid}"), "body": "Closes #7\n**Measured:** 3 s", "head_sha": sha,
               "merge_state": "clean"})
    }

    fn cand(iid: u64, sha: &str, since: i64) -> Candidate {
        candidate(&P, &row(iid, sha), since).unwrap()
    }

    #[test]
    fn only_ready_mrs_into_main_are_candidates() {
        let c = cand(5, A, 10);
        assert_eq!((c.iid(), c.sha.as_str(), c.conflict, c.ready_since, c.mr.issues.clone()), (5, A, false, 10, vec![7]));
        let mut draft = row(5, A);
        draft["draft"] = json!(true);
        let mut other = row(5, A);
        other["base_ref"] = json!("release");
        let mut unfilled = row(5, A);
        unfilled["body"] = json!(format!("Closes #7 {}", crate::prwatch::UNFILLED));
        let mut fork = row(5, A);
        fork["cross_repo"] = json!(true);
        let mut no_head = row(5, A);
        no_head["head_sha"] = json!("abc");
        for v in [draft, other, unfilled, fork, no_head] {
            assert_eq!(candidate(&P, &v, 0), None, "{v}");
        }
        let mut conflict = row(5, A);
        conflict["merge_state"] = json!("dirty");
        assert!(candidate(&P, &conflict, 0).unwrap().conflict);
        // Not worked out yet is no conflict: `stack` finds a real one.
        let mut unknown = row(5, A);
        unknown["merge_state"] = json!("unknown");
        assert!(!candidate(&P, &unknown, 0).unwrap().conflict);
    }

    #[test]
    fn it_pauses_for_the_switch_then_a_red_main() {
        let red = vec![(9, "main fails".to_owned())];
        assert_eq!(pause(Some("owner asked"), &red), Some(Pause::Off("owner asked".into())));
        assert_eq!(pause(None, &red), Some(Pause::MainRed(vec![9])));
        assert_eq!(pause(None, &[]), None);
        assert!(Pause::MainRed(vec![9, 10]).describe().contains("#9 #10"));
        assert!(Pause::Off(String::new()).describe().contains("--resume"));
    }

    #[test]
    fn an_mr_goes_after_the_mrs_its_branch_holds_then_by_ready_time() {
        // #3 holds #2's head; #2 was ready last; #1 holds a draft's (#9) head.
        let cands = [cand(1, A, 5), cand(2, B, 30), cand(3, C, 10), cand(4, D, 20)];
        let open = vec![(1, A.to_owned()), (2, B.to_owned()), (3, C.to_owned()), (4, D.to_owned()), (9, "e".repeat(40))];
        let commits = BTreeMap::from([
            (1, BTreeSet::from([A.to_owned(), "e".repeat(40)])),
            (2, BTreeSet::from([B.to_owned()])),
            (3, BTreeSet::from([B.to_owned(), C.to_owned()])),
            (4, BTreeSet::from([D.to_owned()])),
        ]);
        let deps = dependencies(&cands, &open, &commits);
        assert_eq!(deps, BTreeMap::from([(1, BTreeSet::from([9])), (3, BTreeSet::from([2]))]));
        let (order, held) = order(&cands, &deps);
        assert_eq!(order, vec![4, 2, 3]);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].0, 1);
        assert!(held[0].1.contains("#9"), "{held:?}");
        // What is stacked on a held PR is held too.
        let deps2 = BTreeMap::from([(1, BTreeSet::from([9])), (3, BTreeSet::from([1]))]);
        let (order2, held2) = super::order(&cands, &deps2);
        assert_eq!(order2, vec![4, 2]);
        assert_eq!(held2.iter().map(|(m, _)| *m).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(with_dependents(2, &deps), BTreeSet::from([2, 3]));
        assert_eq!(with_dependents(4, &deps), BTreeSet::from([4]));
    }

    #[test]
    fn a_cycle_keeps_the_ready_order() {
        let cands = [cand(1, A, 2), cand(2, B, 1)];
        let deps = BTreeMap::from([(1, BTreeSet::from([2])), (2, BTreeSet::from([1]))]);
        assert_eq!(order(&cands, &deps).0, vec![2, 1]);
    }

    #[test]
    fn the_conflict_note_names_the_files_and_the_mr_that_landed() {
        let n = conflict_note(&["src/a.rs".into(), "docs/b.md".into()], Some(862));
        assert_eq!(n, "Gate daemon: conflicts with main in `src/a.rs`, `docs/b.md` (after #862 landed): rebase onto main and run `cargo xtask ready`.");
        assert!(conflict_note(&[], None).contains("GitHub didn't name"));
        assert_eq!(merge_request_number("Merge pull request #867 from example/867-work\n\nG11e"), Some(867));
        assert_eq!(merge_request_number("G11e\n\nMerge pull request #867 from example/867-work"), None, "its first line only");
        assert_eq!(merge_request_number("a plain commit"), None);
    }

    #[test]
    fn gates_are_numbered_on_from_the_logs() {
        let names: Vec<String> =
            ["gate-490.log", "gate-491-rerun.rc", "gate-full-0e2492f.log", "gate-x.log", "ready-1.log", "gate-12.rc"].map(String::from).into();
        assert_eq!(next_sub(&names), "gate-492");
        assert_eq!(next_sub(&[]), "gate-1");
    }

    #[test]
    fn a_gate_passes_as_its_rerun_when_only_that_passed() {
        let pass = "rc=0 30s kind=smoke head=abc";
        let fail = "rc=1 30s cause=test kind=smoke head=abc";
        let rerun_fail = "rc=1 9s cause=build kind=smoke head=abc of=gate-3 rerun=x";
        assert_eq!(ran("gate-3", None, None), Ran::NotRun);
        assert_eq!(ran("gate-3", Some(pass), None), Ran::Passed("gate-3".into()));
        assert_eq!(ran("gate-3", Some(fail), Some("rc=0 9s kind=smoke head=abc of=gate-3 rerun=x")), Ran::Passed("gate-3-rerun".into()));
        assert_eq!(ran("gate-3", Some(fail), Some(rerun_fail)), Ran::Failed(Some(Cause::Build)));
        assert_eq!(ran("gate-3", Some(fail), None), Ran::Failed(Some(Cause::Test)));
        assert!(machine_failure(Some(Cause::Memory)) && machine_failure(Some(Cause::Python)));
        assert!(!machine_failure(Some(Cause::Test)) && !machine_failure(None));
    }

    #[test]
    fn a_rebase_during_the_gate_lands_and_a_change_waits_with_what_is_above_it() {
        assert_eq!(push(Some("old"), "old", "new", None, None), Push::None);
        assert_eq!(push(Some("new"), "old", "new", None, None), Push::None);
        assert_eq!(push(Some("w"), "old", "new", Some("p"), Some("p")), Push::Rebased);
        assert_eq!(push(Some("w"), "old", "new", Some("p"), Some("q")), Push::Changed);
        assert_eq!(push(Some("w"), "old", "new", None, None), Push::Changed);
        assert_eq!(push(None, "old", "new", None, None), Push::Gone);
        let mrs = [1, 2, 3];
        assert_eq!(land_prefix(&mrs, &[Push::None, Push::Rebased, Push::None]), (vec![1, 2, 3], vec![]));
        assert_eq!(land_prefix(&mrs, &[Push::None, Push::Changed, Push::None]), (vec![1], vec![2, 3]));
        assert_eq!(land_prefix(&mrs, &[Push::Gone, Push::None, Push::None]), (vec![], vec![1, 2, 3]));
    }

    #[test]
    fn bisecting_gates_the_first_half() {
        assert_eq!(bisect(&[1, 2, 3, 4]), vec![1, 2]);
        assert_eq!(bisect(&[1, 2, 3]), vec![1]);
        assert_eq!(bisect(&[1]), vec![1]);
        assert_eq!(bangs(&[1, 2]), "#1 #2");
        let n = failure_note(9, "gate-4", Some(Cause::Build), "it changed `src/a.rs`, where the build failed", "error[E0308]\n");
        assert!(n.contains("gate `gate-4` failed (cause `build`)") && n.contains("cargo xtask ready 9") && n.contains("error[E0308]"), "{n}");
    }
}
