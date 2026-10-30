//! `cargo xtask land <pr>… --sub <gate sub>`: the coordinator lands
//! a gated stack.
//!
//! First, **every PR it lands must hold work** ([`with_work`]): against what
//! `stack` put it on, commits of its own beyond claim commits and a diff, so a PR un-drafted by
//! hand with only its claim commit never lands. Then **the stack's top must be the HEAD the gate
//! tested** (its `<sub>.rc`): a stack made or moved since the gate, or the tree of a `stack` that
//! stopped part way, never lands. And **`main` must be green** ([`red_check`], `claim`'s check):
//! while a `main-red` issue is open, only PRs that each close one of them land, or a stack given
//! `--while-red "<why>"`, the why in each merge event's note. Then for each PR `cargo xtask stack`
//! stacked, in order, stopping at the first step that fails and saying which PR and why:
//!
//! 1. **The remote branch is still the recorded pre-rebase tip** (or already this push): a worker's
//!    newer commits are never overwritten ([`remote_state`]). One exception:
//!    a branch a worker moved since `stack` (a rebase onto the same `main` while the gate
//!    ran, say) that, restacked onto what the gate's stack put it on, is the tree the gate tested;
//!    the stacked commit is pushed over it ([`moved`]). A move onto a base the gate didn't test, or
//!    to another tree, stops. And `s-<pr>` is still the rebased tip the gate tested
//!    ([`tested_tip`]).
//! 2. **Force-push the rebased branch, then check the remote is the local commit** (a push that
//!    didn't land is never merged; one the server failed is tried again, [`PUSH_TRIES`]).
//! 3. **Note the gate's summary** on the PR (the gate's log, [`gate_summary`]) and cancel the
//!    workflow runs of its head still queued or running (the gate replaced them).
//! 4. **Wait until it is mergeable,** bringing it up to `main` when GitHub says it is `behind`: a
//!    rebase on the server, or, for a branch the server can't rebase (merge commits) or one `stack`
//!    merged its base into, `main` merged in here and pushed: semi-linear history needs `main`'s
//!    head.
//! 5. **Merge it** with a merge commit at the head it saw, asked again while GitHub refuses it
//!    ([`MERGE_TRIES`]): a docs-only PR with `[skip ci]` in its merge commit; `main`'s workflows
//!    run for the rest.
//!    Before each merge call, **that head must carry the gated change** (swarm-core#9,
//!    [`carries_gated`]): the commit `land` pushed, or one whose tree is the gated change rebased
//!    onto its base, or whose diff has the gated diff's patch-id (the server's rebase). A worker's
//!    push during the wait is anything else: the landing stops there, nothing merged.
//! 6. **Confirm it merged.**
//! 7. **Log `merge --sub <gate sub>`** for each issue its description closes, then `tidy <n>`.
//! 8. **Fast-forward the owner's checkout,** only when it is on `main` with no tracked changes.
//! 9. **Delete the head branch, only after step 6 confirmed the merge** (a branch deleted after
//!    a merge that hadn't happened closes its PR).
//!
//! `land` takes the gate lock first (`cargo xtask lock`; refused while another coordinator holds
//! it), and the gate closes after the last merge (`gate --stop`, which drops the lock). A failure
//! leaves the gate open and the lock held: the coordinator still owns the worktree.
//!
//! The decisions are pure functions; GitHub and the event log sit behind [`Effects`], so the
//! order of the steps is tested with a fake, and the pushes against real repositories.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::process::{git, git_bytes};
use crate::stack::{self, Entry, Stack, short};
use crate::{Project, capacity, claim, event, fullgate, gate, github, lock, tidy};

/// How often GitHub is asked while waiting.
const POLL: Duration = Duration::from_secs(5);
/// Polls for mergeability (5 minutes).
const MERGEABLE_POLLS: u32 = 60;
/// Times a PR `behind` `main` is brought up to it before `land` gives up (`main` moving on).
const UPDATES: u32 = 3;
/// Polls for the merge to be confirmed (2 minutes).
const MERGED_POLLS: u32 = 24;
/// Tries of a merge GitHub refused (it may still be checking the head it was given).
pub const MERGE_TRIES: u32 = 3;
/// Tries of a push that didn't land (the server's "unpacker error" goes through on a retry).
pub const PUSH_TRIES: u32 = 3;
/// The wait before a push is tried again (none in the tests: their origin answers at once).
const PUSH_WAIT: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_secs(5) };
/// The most of the gate's summary that goes on a PR.
const MAX_SUMMARY: usize = 3000;

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask land <pr>… --sub <gate sub> [--agent <instance>] [--summary <text>] [--owner <path> | --no-owner] [--while-red <why>]";

/// The whole usage, for `project`.
fn usage(project: &Project) -> String {
    format!(
        "usage: {USAGE}
  lands the next PRs of this worktree's stack (`cargo xtask stack`), in order, when its top is the
  HEAD the gate tested (<target>/gate-logs/<sub>.rc): checks each PR holds work and each remote
  branch is still its pre-rebase tip (or a worker's push since that, restacked onto the same base,
  is the tree the gate tested), force-pushes the rebased branch and checks it landed, notes the
  gate's summary and cancels its workflow runs, brings it up to main when it is behind (a server
  rebase, or main merged in), merges (docs-only: [skip ci]), confirms the merge, logs `merge --sub <gate sub>` and tidies
  each issue its description closes, fast-forwards the owner's checkout (on main, no tracked
  changes), then deletes the source branch. Then `cargo xtask gate --stop <gate sub>`. Stops at
  the first failure, naming the PR. Refused while a main-red issue is open, unless every PR closes
  one of them.
  --summary   the gate's summary (default: the `CI summary` of <target>/gate-logs/<sub>.log)
  --agent     the coordinator's instance (default {})
  --owner     the owner's checkout to fast-forward (default: the clone's main worktree)
  --while-red land while main is red anyway; the why goes into each merge event's note",
        project.env_name("AGENT")
    )
}

/// What the remote branch says about the recorded tips.
#[derive(Debug, PartialEq, Eq)]
pub enum Remote {
    /// Still the pre-rebase tip: push.
    Unmoved,
    /// Already the rebased tip (an earlier `land` pushed it): don't push again.
    Pushed,
    /// Someone pushed since `stack`: never overwrite it.
    Moved(String),
    /// Gone.
    Missing,
}

/// Where the remote branch is (`remote`), against the recorded pre-rebase tip and the local
/// rebased one.
pub fn remote_state(remote: Option<&str>, old_tip: &str, local: &str) -> Remote {
    match remote {
        None => Remote::Missing,
        Some(r) if r == local => Remote::Pushed,
        Some(r) if r == old_tip => Remote::Unmoved,
        Some(r) => Remote::Moved(r.to_owned()),
    }
}

/// A branch that moved since `stack`, against the stacked commit the gate tested.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Restack {
    /// It holds no commit of `<remote>/<base>` the gate didn't test: where it forks from the base
    /// is in what the PR was stacked onto ([`Entry::onto`]).
    pub base_tested: bool,
    /// The tree of the stacked commit the gate tested.
    pub gated_tree: String,
    /// Its own tree.
    pub own_tree: String,
    /// Its tree restacked onto what the PR was stacked onto; `None` when that conflicts.
    pub tree: Option<String>,
}

/// What `land` makes of a PR's source branch before it pushes.
#[derive(Debug, PartialEq, Eq)]
pub enum Moved {
    /// Not moved since `stack` (or gone, or pushed already): the push checks it, as before.
    No,
    /// A worker pushed `now` since, but restacked onto the base the gate tested it is the gated
    /// tree: push the stacked commit over it. `own`: its own commit is that tree already.
    SameTree { now: String, own: bool },
    /// A worker pushed `now`, onto a base the gate didn't test.
    AnotherBase(String),
    /// A worker pushed `now`, and restacked its tree isn't the gated one (or it conflicts).
    AnotherTree(String),
}

/// The decision for a branch at `remote` ([`remote_state`]); `restack` gives the facts about the
/// commit a worker pushed, asked only when it moved.
pub fn moved(remote: &Remote, restack: impl FnOnce(&str) -> Result<Restack, String>) -> Result<Moved, String> {
    let Remote::Moved(now) = remote else { return Ok(Moved::No) };
    let r = restack(now)?;
    let now = now.clone();
    Ok(if !r.base_tested {
        Moved::AnotherBase(now)
    } else if r.tree.as_deref() == Some(r.gated_tree.as_str()) {
        Moved::SameTree { now, own: r.own_tree == r.gated_tree }
    } else {
        Moved::AnotherTree(now)
    })
}

/// A PR as landing needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MrView {
    /// `open`, `closed` or `merged`.
    pub state: String,
    /// GitHub's merge state, lower case (`clean`, `behind`, `dirty`, `blocked`, `unknown`, …).
    pub merge_state: String,
    /// The head branch's commit on the server.
    pub sha: String,
    /// Its GraphQL id, for the server's rebase.
    pub node_id: String,
}

/// A PR in [`github`]'s shape.
pub fn parse_view(v: &Value) -> MrView {
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    MrView { state: text("state"), merge_state: text("merge_state"), sha: text("head_sha"), node_id: text("node_id") }
}

/// Where a PR stands while waiting to merge it.
#[derive(Debug, PartialEq, Eq)]
pub enum Wait {
    /// Open and mergeable: merge it.
    Ready,
    /// Merged already.
    Merged,
    /// Behind `main`: bring it up to it.
    Behind,
    /// GitHub is still working it out: ask again.
    Again,
    /// It can't get there: why.
    Stuck(String),
}

/// Where `v` stands. `unstable` (a check that isn't required failed or was cancelled, as step 3
/// cancels them) and `has_hooks` merge.
pub fn wait_state(v: &MrView) -> Wait {
    match (v.state.as_str(), v.merge_state.as_str()) {
        ("merged", _) => Wait::Merged,
        ("open", "clean" | "unstable" | "has_hooks") => Wait::Ready,
        ("open", "behind") => Wait::Behind,
        ("open", "dirty") => Wait::Stuck("it conflicts with its base".into()),
        ("open", "draft") => Wait::Stuck("it is a draft".into()),
        ("open", "blocked") => Wait::Stuck("branch protection blocks it (a required review or check)".into()),
        ("open", _) => Wait::Again,
        (other, _) => Wait::Stuck(format!("it is {other}")),
    }
}

/// The merge commit's body: a docs-only PR's says `[skip ci]`, so `main`'s workflows skip it;
/// others keep GitHub's (the PR's title).
pub fn merge_message(title: &str, docs_only: bool) -> Option<String> {
    docs_only.then(|| format!("{title}\n\n[skip ci]"))
}

/// The gate's summary in its log: from its last `CI summary` line on. Refused unless it says
/// every step passed: a failed gate lands nothing.
pub fn gate_summary(log: &str) -> Result<String, String> {
    let summary = gate::ci_summary(log).ok_or("the gate's log has no `CI summary`: did it finish?")?.trim_end();
    let first = summary.lines().next().unwrap_or_default();
    if !(first.contains("all ") && first.contains("steps passed")) || summary.lines().any(|l| l.trim_start().starts_with("FAIL")) {
        return Err(format!("the gate did not pass:\n{summary}"));
    }
    Ok(cap(summary))
}

/// At most [`MAX_SUMMARY`] characters of `summary`, `…` marking a cut.
fn cap(summary: &str) -> String {
    let mut s: String = summary.chars().take(MAX_SUMMARY).collect();
    if s.len() < summary.len() {
        s.push('…');
    }
    s
}

/// The note each landed PR gets; `tag` names the gate ([`Project::gate_tag`]).
pub fn note_body(tag: &str, sub: &str, summary: &str, mrs: &[u64]) -> String {
    format!("Local gate `{sub}`{tag} on the rebased stack ({}), before merging:\n\n```text\n{summary}\n```", bangs(mrs))
}

/// Why the stack can't land on gate `sub`, which tested `tested`: its top is another commit.
pub fn untested(stack: &Stack, sub: &str, tested: &str) -> Option<String> {
    let top = stack.entries.last()?;
    (top.new_tip != tested).then(|| {
        format!(
            "the stack's top is {} (#{}), but gate {sub} tested {}: restack and gate it again, never land what the gate didn't test",
            short(&top.new_tip),
            top.mr,
            short(tested)
        )
    })
}

/// Is `local`, `s-<pr>`'s commit, what the gate tested of the PR? The recorded rebased tip
/// (`new_tip`) is; so is a commit `land` made on it and pushed (`main` merged in, step 4, before a
/// rerun), which `remote` (the source branch's tip now) then is. A branch moved any other way since
/// `stack` is not.
pub fn tested_tip(local: &str, new_tip: &str, remote: Option<&str>, descends: impl FnOnce() -> bool) -> bool {
    local == new_tip || (remote == Some(local) && descends())
}

/// The entries `mrs` names: the next ones of the stack, in its order (a PR below one landed must
/// land first, or its commits would ride along unchecked).
pub fn pick<'a>(stack: &'a Stack, mrs: &[u64]) -> Result<&'a [Entry], String> {
    let order: Vec<u64> = stack.entries.iter().map(|e| e.mr).collect();
    if mrs.is_empty() || order.get(..mrs.len()) != Some(mrs) {
        return Err(format!(
            "land the stack's next PRs in its order: {} (stacked by `cargo xtask stack`)",
            if order.is_empty() { "none".into() } else { bangs(&order) }
        ));
    }
    Ok(stack.entries.get(..mrs.len()).unwrap_or_default())
}

/// May `todo` land while `red` (the open `main-red` issues) says `main` is red? The check is
/// `claim`'s ([`claim::main_is_red`]): nothing merges then, unless every PR closes one of them (the
/// fix) or `while_red` says why it must. `Ok` with the note each merge event carries (the why),
/// or the refusal, naming the issues and the PRs that fix none.
pub fn red_check(red: &[(u64, String)], todo: &[Entry], while_red: Option<&str>) -> Result<Option<String>, String> {
    let unfixed: Vec<u64> = todo.iter().filter(|e| !claim::fixes_red(red, &e.issues)).map(|e| e.mr).collect();
    let rule = format!(
        "Nothing merges until it is green again (docs/swarm-protocol.md, the land step), unless every PR landed closes one of them; {} close{} none. Land the fix first, or pass --while-red \"<why>\".",
        bangs(&unfixed),
        if unfixed.len() == 1 { "s" } else { "" }
    );
    match (claim::main_is_red(red, unfixed.is_empty(), &rule), while_red) {
        (None, _) => Ok(None),
        (Some(_), Some(why)) => {
            let issues: Vec<String> = red.iter().map(|(n, _)| format!("#{n}")).collect();
            Ok(Some(format!("landed while main is red ({}): {why}", issues.join(" "))))
        }
        (Some(refusal), None) => Err(format!("land: {refusal}")),
    }
}

/// GitHub, the event log and the gate: everything `land` does outside git.
pub trait Effects {
    fn mr(&self, mr: u64) -> Result<MrView, String>;
    fn note(&self, mr: u64, body: &str) -> Result<(), String>;
    /// Cancel the workflow runs of the PR's head still queued or running.
    fn cancel_runs(&self, mr: u64) -> Result<(), String>;
    /// Rebase the PR `v` onto its base on the server, while its head is still `v.sha`:
    /// `Some(why)` when the server refuses (merge commits, a conflict).
    fn rebase(&self, mr: u64, v: &MrView) -> Result<Option<String>, String>;
    /// Merge at `sha` (GitHub refuses if the branch moved), with `message` or GitHub's own.
    fn merge(&self, mr: u64, sha: &str, message: Option<&str>) -> Result<(), String>;
    fn delete_branch(&self, branch: &str) -> Result<(), String>;
    /// `cargo xtask event merge --issue <issue> --mr <mr> --sub <sub> [--note <note>]`.
    fn log_merge(&self, issue: u64, mr: u64, sub: &str, note: Option<&str>) -> Result<(), String>;
    /// `cargo xtask tidy <issue>`.
    fn tidy(&self, issue: u64) -> Result<(), String>;
    /// `cargo xtask gate --stop <sub> --note <note>`: logs `sub.stop` (`rc=0; <note>`), drops the
    /// lock.
    fn close_gate(&self, sub: &str, note: &str) -> Result<(), String>;
    fn sleep(&self, d: Duration);
}

/// What `land` was asked.
pub struct Plan<'a> {
    pub project: &'a Project,
    pub root: &'a Path,
    pub sub: &'a str,
    pub summary: &'a str,
    /// The HEAD the gate tested (its `<sub>.rc`): the stack's top must be it.
    pub tested: &'a str,
    /// The owner's checkout to fast-forward, if any.
    pub owner: Option<&'a Path>,
    /// The note each merge event carries: why it landed while `main` was red ([`red_check`]).
    pub note: Option<&'a str>,
}

/// Land `mrs` from the worktree's stack; each landed PR leaves the record at once, so a rerun
/// after a failure goes on from there. Returns the PRs landed.
pub fn land(plan: &Plan<'_>, stack: &mut Stack, mrs: &[u64], fx: &dyn Effects) -> Result<Vec<u64>, String> {
    let todo: Vec<Entry> = pick(stack, mrs)?.to_vec();
    if let Some(why) = untested(stack, plan.sub, plan.tested) {
        return Err(format!("land: {why}; the gate stays open and the lock held"));
    }
    let body = note_body(plan.project.gate_tag, plan.sub, plan.summary, mrs);
    let mut landed = Vec::new();
    for e in &todo {
        land_one(plan, &stack.remote, &stack.base, e, &body, fx).map_err(|why| {
            let done = if landed.is_empty() { String::new() } else { format!(" (landed before it: {})", bangs(&landed)) };
            format!("land: #{} stopped: {why}{done}; the gate stays open and the lock held", e.mr)
        })?;
        landed.push(e.mr);
        stack.entries.retain(|x| x.mr != e.mr);
        stack::save(plan.project, plan.root, stack)?;
    }
    fx.close_gate(plan.sub, &format!("merged {}", bangs(&landed)))?;
    Ok(landed)
}

fn bangs(mrs: &[u64]) -> String {
    mrs.iter().map(|m| format!("#{m}")).collect::<Vec<_>>().join(" ")
}

fn land_one(plan: &Plan<'_>, remote: &str, base: &str, e: &Entry, body: &str, fx: &dyn Effects) -> Result<(), String> {
    let root = plan.root;
    let src = &e.source_branch;
    if !stack::valid_branch(src) {
        return Err(format!("source branch `{src}` is not one to push"));
    }
    let local = stacked_tip(root, remote, e)?;

    // 1, 2: never over a worker's newer commits (unless restacked they are the gated tree); never
    // merge a push that didn't land.
    let over = push_over(root, remote, base, e, &local)?;
    push_checked(plan.project, root, remote, src, &over, &local)?;
    // `stack`'s regenerated docs are a commit in that tip: pushed with the PR.
    let regen = if e.regenerated.is_empty() { String::new() } else { format!(", with regenerated {}", e.regenerated.join(", ")) };
    println!("land: #{} pushed {src} at {}{regen}", e.mr, short(&local));

    // 3.
    fx.note(e.mr, body)?;
    if let Err(err) = fx.cancel_runs(e.mr) {
        eprintln!("warning: land: #{}: cancelling its workflow runs: {err}", e.mr);
    }

    // 4, 5.
    merge_when_ready(plan, remote, base, e, fx)?;

    // 6: nothing below runs on a merge that didn't happen.
    confirm_merged(e.mr, fx)?;
    println!("land: #{} merged{}", e.mr, if e.docs_only { " ([skip ci]: docs only)" } else { "" });

    // 7.
    record_merge(plan, e, fx);

    // 8.
    if let Some(owner) = plan.owner {
        println!("land: owner's checkout: {}", fast_forward_owner(owner, remote, base));
    }

    // 9: only now, with the merge confirmed.
    if let Err(err) = fx.delete_branch(src) {
        eprintln!("warning: land: #{}: deleting {src}: {err}", e.mr);
    }
    Ok(())
}

/// `s-<pr>`'s commit, which must still be what the gate tested ([`tested_tip`]).
fn stacked_tip(root: &Path, remote: &str, e: &Entry) -> Result<String, String> {
    let branch = stack::local_branch(e.mr);
    let local = stack::rev(root, &format!("refs/heads/{branch}")).map_err(|_| format!("no local branch {branch}: restack"))?;
    if local == e.new_tip {
        return Ok(local);
    }
    let seen = remote_tip(root, remote, &format!("refs/heads/{}", e.source_branch))?;
    let descends = || git(root, &["merge-base", "--is-ancestor", &e.new_tip, &local], None, &[]).is_ok();
    if tested_tip(&local, &e.new_tip, seen.as_deref(), descends) {
        return Ok(local);
    }
    Err(format!("{branch} is {}, not the {} the stack recorded and the gate tested: restack", short(&local), short(&e.new_tip)))
}

/// Step 1: the tip the push goes over. The recorded pre-rebase tip; or, when a worker pushed since
/// `stack` and that branch restacked onto the base the gate tested is the gated tree, the worker's.
/// Any other move stops the landing.
fn push_over(root: &Path, remote: &str, base: &str, e: &Entry, local: &str) -> Result<String, String> {
    let src = &e.source_branch;
    let seen = remote_tip(root, remote, &format!("refs/heads/{src}"))?;
    let state = remote_state(seen.as_deref(), &e.old_tip, local);
    let was = |now: &str| format!("{remote}/{src} moved since `stack` (was {}, now {})", short(&e.old_tip), short(now));
    let never = "a worker pushed; restack it, never push over it";
    let verdict = moved(&state, |now| restack(root, remote, base, e, now)).map_err(|why| format!("{}: {why}", was(seen.as_deref().unwrap_or_default())))?;
    match verdict {
        Moved::No => Ok(e.old_tip.clone()),
        Moved::SameTree { now, own } => {
            let how = if own { "its own commit is that tree already".to_owned() } else { format!("restacked onto {}", short(&e.onto)) };
            println!("land: #{} moved since stack ({} → {}) but its tree is the one gated ({how}): landing", e.mr, short(&e.old_tip), short(&now));
            Ok(now)
        }
        Moved::AnotherBase(now) => Err(format!("{} onto a base the gate didn't test: {never}", was(&now))),
        Moved::AnotherTree(now) => Err(format!("{}, and restacked onto {} its tree is not the one gated: {never}", was(&now), short(&e.onto))),
    }
}

/// The facts about `now`, the commit a worker pushed to the PR's branch since `stack`: its base
/// and its tree restacked onto [`Entry::onto`] (`git merge-tree`: no checkout, branch or ref of the
/// gate's worktree changes).
fn restack(root: &Path, remote: &str, base: &str, e: &Entry, now: &str) -> Result<Restack, String> {
    if e.onto.is_empty() {
        return Err("the stack's record names no base to restack it onto".into());
    }
    event::fetch_tip(root, remote, &format!("refs/heads/{}", e.source_branch))?;
    let base_tip = event::fetch_tip(root, remote, &format!("refs/heads/{base}"))?.ok_or_else(|| format!("{remote}/{base} is gone"))?;
    let fork = git(root, &["merge-base", now, &base_tip], None, &[]).map(|f| f.trim().to_owned());
    let base_tested = fork.is_ok_and(|f| git(root, &["merge-base", "--is-ancestor", &f, &e.onto], None, &[]).is_ok());
    let restacked = git(root, &["merge-tree", "--write-tree", &e.onto, now], None, &[]).ok();
    Ok(Restack {
        base_tested,
        gated_tree: tree_of(root, &e.new_tip)?,
        own_tree: tree_of(root, now)?,
        tree: restacked.and_then(|t| t.lines().next().map(str::to_owned)),
    })
}

/// The tree of commit `c`.
fn tree_of(root: &Path, c: &str) -> Result<String, String> {
    Ok(git(root, &["rev-parse", "--verify", "--quiet", &format!("{c}^{{tree}}")], None, &[]).map_err(|_| format!("no commit `{c}`"))?.trim().to_owned())
}

/// Refuse to land `todo` when any of them holds no work: against what
/// `stack` put it on ([`Entry::onto`]; `<remote>/<base>` in a record from before), its rebased tip
/// has no commits of its own, only claim commits, or an empty diff. Checked before anything is
/// pushed.
pub fn with_work(root: &Path, stack: &Stack, todo: &[Entry]) -> Result<(), String> {
    for e in todo {
        let onto = if e.onto.is_empty() { format!("refs/remotes/{}/{}", stack.remote, stack.base) } else { e.onto.clone() };
        if let Some(why) = crate::empty::why(root, &onto, &e.new_tip)? {
            return Err(format!("land: #{} {}; nothing landed", e.mr, crate::empty::refusal(&why)));
        }
    }
    Ok(())
}

/// Step 7: each closed issue's merge event inside the plan's gate, with its note, and its label
/// tidied. Neither undoes the merge, so a failure warns.
fn record_merge(plan: &Plan<'_>, e: &Entry, fx: &dyn Effects) {
    for &n in &e.issues {
        if let Err(err) = fx.log_merge(n, e.mr, plan.sub, plan.note) {
            eprintln!("warning: land: #{}: the merge event for #{n}: {err}", e.mr);
        }
        if let Err(err) = fx.tidy(n) {
            eprintln!("warning: land: #{}: tidy #{n}: {err}", e.mr);
        }
    }
}

/// Steps 4 and 5: the wait, bringing the PR up to `main` while it is behind (the server's rebase,
/// or `main` merged in), and the merge at the head seen, tried again while GitHub refuses it.
/// Returns once the merge was asked for, or the PR merged already.
fn merge_when_ready(plan: &Plan<'_>, remote: &str, base: &str, e: &Entry, fx: &dyn Effects) -> Result<(), String> {
    let mut merged_in = e.merged_base;
    if merged_in {
        // `stack` merged its base in: keep it so (a server rebase would replay the merges).
        merge_base_in(plan, remote, base, e)?;
    }
    let message = merge_message(&e.title, e.docs_only);
    let (mut refused, mut updates) = (0, 0);
    loop {
        let sha = match wait_mergeable(e.mr, fx)? {
            Ready::Merged => return Ok(()),
            Ready::At(sha) => sha,
            Ready::Behind(_) if updates >= UPDATES => return Err(format!("still behind {base} after bringing it up {UPDATES} times")),
            Ready::Behind(v) => {
                updates += 1;
                merged_in = bring_up(e.mr, &v, merged_in, fx, &|| merge_base_in(plan, remote, base, e))?;
                continue;
            }
        };
        // Only the gated change merges: a worker's push while it waited is never landed.
        gated_head(plan.root, remote, base, e, &sha)?;
        match fx.merge(e.mr, &sha, message.as_deref()) {
            Ok(()) => return Ok(()),
            Err(err) => {
                refused += 1;
                if refused >= MERGE_TRIES {
                    return Err(format!("the merge was refused {MERGE_TRIES} times: {err}"));
                }
                eprintln!("land: #{}: the merge at {} was refused ({err}); asking again", e.mr, short(&sha));
                fx.sleep(POLL);
            }
        }
    }
}

/// Step 4's update of PR `mr`, behind its base (`v`): the server's rebase, unless the base was
/// merged in already (`merged_in`, kept so) or the server refuses, when `merge_in` merges it in
/// here. Whether the base is merged in now.
fn bring_up(mr: u64, v: &MrView, merged_in: bool, fx: &dyn Effects, merge_in: &dyn Fn() -> Result<(), String>) -> Result<bool, String> {
    if !merged_in {
        match fx.rebase(mr, v)? {
            None => return Ok(false),
            Some(why) => println!("land: #{mr}: the server's rebase was refused ({why}); merging its base into the branch"),
        }
    }
    merge_in()?;
    Ok(true)
}

/// What waiting for mergeability ended in.
enum Ready {
    At(String),
    Merged,
    Behind(MrView),
}

/// Step 4's wait: the head to merge at once the PR is mergeable, or that it is behind.
fn wait_mergeable(mr: u64, fx: &dyn Effects) -> Result<Ready, String> {
    for _ in 0..MERGEABLE_POLLS {
        let v = fx.mr(mr)?;
        match wait_state(&v) {
            Wait::Ready => return Ok(Ready::At(v.sha)),
            Wait::Merged => return Ok(Ready::Merged),
            Wait::Behind => return Ok(Ready::Behind(v)),
            Wait::Again => fx.sleep(POLL),
            Wait::Stuck(why) => return Err(why),
        }
    }
    Err("not mergeable after 5 minutes".into())
}

/// The head the server would merge, against the change the gate tested (swarm-core#9).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Head {
    /// It is the commit `land` pushed (the stacked tip, or `land`'s merge of the base on it).
    pub pushed: bool,
    /// Its tree.
    pub tree: String,
    /// The gated change rebased onto its base: the tree it would have; `None` when that conflicts.
    pub gated_tree: Option<String>,
    /// The patch-id of its diff against its base; `None` when that diff is empty.
    pub patch_id: Option<String>,
    /// The patch-id of the gated diff (from what `stack` put the PR on to the tip the gate tested).
    pub gated_patch_id: Option<String>,
}

/// Does `h` carry the gated change: the pushed commit, or the same tree after the same rebase,
/// or the same patch?
pub fn carries_gated(h: &Head) -> bool {
    h.pushed || h.gated_tree.as_deref() == Some(h.tree.as_str()) || (h.patch_id.is_some() && h.patch_id == h.gated_patch_id)
}

/// Step 5's check, before each merge call: `sha`, the head the server would merge, carries the
/// gated change ([`carries_gated`]); otherwise stop, as [`push_over`] does, merging nothing.
fn gated_head(root: &Path, remote: &str, base: &str, e: &Entry, sha: &str) -> Result<(), String> {
    let h = head_facts(root, remote, base, e, sha)?;
    if carries_gated(&h) {
        return Ok(());
    }
    Err(format!(
        "{remote}/{} is {} on the server, not the change the gate tested ({}): a worker pushed while it waited; restack it, never merge it",
        e.source_branch,
        short(sha),
        short(&e.new_tip)
    ))
}

/// The facts about `sha` for [`carries_gated`]: git only (`merge-tree`, `diff-tree`, `patch-id`),
/// no checkout, branch or ref of the gate's worktree changes.
fn head_facts(root: &Path, remote: &str, base: &str, e: &Entry, sha: &str) -> Result<Head, String> {
    let pushed = stack::rev(root, &format!("refs/heads/{}", stack::local_branch(e.mr)))?;
    if sha == pushed || sha == e.new_tip {
        return Ok(Head { pushed: true, ..Head::default() });
    }
    event::fetch_tip(root, remote, &format!("refs/heads/{}", e.source_branch))?;
    let base_tip = event::fetch_tip(root, remote, &format!("refs/heads/{base}"))?.ok_or_else(|| format!("{remote}/{base} is gone"))?;
    let unknown = |_| format!("the server's head of {} is {}, which {remote} doesn't have: can't check it is the gated change", e.source_branch, short(sha));
    let tree = tree_of(root, sha).map_err(unknown)?;
    let fork = git(root, &["merge-base", sha, &base_tip], None, &[]).map_err(|_| format!("{} shares no history with {remote}/{base}", short(sha)))?;
    let fork = fork.trim();
    let onto = if e.onto.is_empty() { format!("refs/remotes/{remote}/{base}") } else { e.onto.clone() };
    let gated_tree = git(root, &["merge-tree", "--write-tree", "--merge-base", &onto, fork, &e.new_tip], None, &[]).ok();
    Ok(Head {
        pushed: false,
        tree,
        gated_tree: gated_tree.and_then(|t| t.lines().next().map(str::to_owned)),
        patch_id: patch_id(root, fork, sha)?,
        gated_patch_id: patch_id(root, &onto, &e.new_tip)?,
    })
}

/// The stable patch-id of the diff from `from` to `to`; `None` when there is no diff.
fn patch_id(root: &Path, from: &str, to: &str) -> Result<Option<String>, String> {
    let diff = git_bytes(root, &["diff-tree", "-p", "--no-color", "--full-index", "--no-ext-diff", "--no-textconv", from, to], None, &[])?;
    if diff.is_empty() {
        return Ok(None);
    }
    let id = git(root, &["patch-id", "--stable"], Some(&diff), &[])?;
    Ok(id.split_whitespace().next().map(str::to_owned))
}

/// Step 6: the PR's state is `merged`, asked until it is (2 minutes), or why not.
fn confirm_merged(mr: u64, fx: &dyn Effects) -> Result<(), String> {
    let mut state = String::new();
    for i in 0..MERGED_POLLS {
        state = fx.mr(mr)?.state;
        if state == "merged" {
            return Ok(());
        }
        if i + 1 < MERGED_POLLS {
            fx.sleep(POLL);
        }
    }
    Err(format!("its state is `{state}`, not merged, after 2 minutes: the branch is kept"))
}

/// Step 4's fallback: merge `<remote>/<base>` into the PR's local branch (unless it is in it
/// already) and push that over the tip `land` pushed.
fn merge_base_in(plan: &Plan<'_>, remote: &str, base: &str, e: &Entry) -> Result<(), String> {
    let root = plan.root;
    let branch = stack::local_branch(e.mr);
    let tracking = format!("refs/remotes/{remote}/{base}");
    event::fetch(root, remote, &format!("refs/heads/{base}"), &tracking)?;
    let pushed = stack::rev(root, &format!("refs/heads/{branch}"))?;
    if git(root, &["merge-base", "--is-ancestor", &tracking, &pushed], None, &[]).is_ok() {
        return Ok(());
    }
    git(root, &["checkout", "--quiet", &branch], None, &[])?;
    stack::merge_in(plan.project, root, &tracking).map_err(|why| format!("merging {remote}/{base} into {branch}: {why}"))?;
    let merged = stack::rev(root, "HEAD")?;
    push_checked(plan.project, root, remote, &e.source_branch, &pushed, &merged)?;
    println!("land: #{} merged {base} in and pushed {} at {}", e.mr, e.source_branch, short(&merged));
    Ok(())
}

/// Steps 1 and 2: check the remote branch, force-push `local` over exactly what was checked, and
/// check the remote is `local` afterwards. A push that didn't land and left the branch where it
/// was is tried again, up to [`PUSH_TRIES`] in all.
pub fn push_checked(project: &Project, root: &Path, remote: &str, branch: &str, old_tip: &str, local: &str) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    let no_check = project.env_name("CLAIM_CHECK");
    let mut last = String::new();
    for attempt in 0..PUSH_TRIES {
        let seen = remote_tip(root, remote, &refname)?;
        match remote_state(seen.as_deref(), old_tip, local) {
            Remote::Pushed => return Ok(()),
            Remote::Missing => return Err(format!("{remote}/{branch} is gone")),
            Remote::Moved(now) => {
                return Err(format!(
                    "{remote}/{branch} moved since `stack` (was {}, now {}): a worker pushed; restack it, never push over it",
                    short(old_tip),
                    short(&now)
                ));
            }
            Remote::Unmoved if attempt > 0 => std::thread::sleep(PUSH_WAIT),
            Remote::Unmoved => {}
        }
        // The lease makes the check and the push one step: a push in between is refused.
        let lease = format!("--force-with-lease={refname}:{old_tip}");
        let pushed = stack::git_slow(root, &["push", "--quiet", &lease, remote, &format!("{local}:{refname}")], &[(&no_check, "off")]);
        let now = remote_tip(root, remote, &refname)?;
        if now.as_deref() == Some(local) {
            return Ok(());
        }
        let why = pushed.err().map(|e| format!(" ({e})")).unwrap_or_default();
        last = format!("the push didn't land: {remote}/{branch} is {}, not {}{why}: not merging", now.as_deref().map_or("missing", short), short(local));
    }
    Err(last)
}

/// The remote's tip of `refname`, asked now (`ls-remote`).
fn remote_tip(root: &Path, remote: &str, refname: &str) -> Result<Option<String>, String> {
    let listing = git(root, &["ls-remote", remote, refname], None, &[])?;
    Ok(listing.lines().find_map(|l| l.split_once('\t').filter(|(_, r)| *r == refname).map(|(id, _)| id.to_owned())))
}

/// Step 8: fast-forward the owner's checkout to `<remote>/<base>`, only when it is on `base`
/// with no tracked changes (untracked files are left alone); what happened, in a few words.
/// Never stashes, resets or switches branches there, and never fails the landing.
pub fn fast_forward_owner(owner: &Path, remote: &str, base: &str) -> String {
    let on = git(owner, &["symbolic-ref", "--quiet", "--short", "HEAD"], None, &[]).unwrap_or_default();
    if on.trim() != base {
        return format!("left alone (on {}, not {base})", if on.trim().is_empty() { "a detached HEAD" } else { on.trim() });
    }
    match git(owner, &["status", "--porcelain", "--untracked-files=no"], None, &[]) {
        Ok(s) if s.trim().is_empty() => {}
        Ok(_) => return "left alone (tracked changes)".into(),
        Err(e) => return format!("left alone ({e})"),
    }
    let tracking = format!("refs/remotes/{remote}/{base}");
    let r = event::fetch(owner, remote, &format!("refs/heads/{base}"), &tracking)
        .and_then(|()| stack::git_slow(owner, &["merge", "--ff-only", "--quiet", &tracking], &[]));
    match r {
        Ok(_) => format!("fast-forwarded to {}", git(owner, &["log", "--oneline", "-1"], None, &[]).unwrap_or_default().trim()),
        Err(e) => format!("left alone ({e})"),
    }
}

/// The clone's main worktree (the owner's checkout), when it isn't `root` itself.
fn default_owner(root: &Path) -> Option<PathBuf> {
    let listing = git(root, &["worktree", "list", "--porcelain"], None, &[]).ok()?;
    let first = PathBuf::from(listing.lines().next()?.strip_prefix("worktree ")?);
    let same = |a: &Path, b: &Path| a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b);
    (!same(&first, root)).then_some(first)
}

// ---------------------------------------------------------------------------------------------
// The real effects: GitHub (the server's failures tried again), the event log and the gate.

struct Real<'a> {
    project: &'a Project,
    root: &'a Path,
    agent: String,
}

impl Effects for Real<'_> {
    fn mr(&self, mr: u64) -> Result<MrView, String> {
        Ok(parse_view(&github::pr(self.project, mr)?))
    }
    fn note(&self, mr: u64, body: &str) -> Result<(), String> {
        github::comment(self.project, mr, body)
    }
    fn cancel_runs(&self, mr: u64) -> Result<(), String> {
        let sha = self.mr(mr)?.sha;
        for id in github::cancel_runs(self.project, &sha)? {
            println!("land: #{mr}: workflow run {id} cancelled");
        }
        Ok(())
    }
    fn rebase(&self, mr: u64, v: &MrView) -> Result<Option<String>, String> {
        let _ = mr;
        github::rebase_pr(self.project, &v.node_id, &v.sha)
    }
    fn merge(&self, mr: u64, sha: &str, message: Option<&str>) -> Result<(), String> {
        github::merge_pr(self.project, mr, sha, message)
    }
    fn delete_branch(&self, branch: &str) -> Result<(), String> {
        if !stack::valid_branch(branch) {
            return Err(format!("`{branch}` is not a branch to delete"));
        }
        github::delete_branch(self.project, branch)
    }
    fn log_merge(&self, issue: u64, mr: u64, sub: &str, note: Option<&str>) -> Result<(), String> {
        let (issue, mr) = (issue.to_string(), mr.to_string());
        let mut args = vec!["merge", "--agent", &self.agent, "--issue", &issue, "--mr", &mr, "--sub", sub];
        if let Some(n) = note {
            args.extend(["--note", n]);
        }
        event::run(self.project, self.root, &args)
    }
    fn tidy(&self, issue: u64) -> Result<(), String> {
        tidy::run(self.project, self.root, &[&issue.to_string()])
    }
    fn close_gate(&self, sub: &str, note: &str) -> Result<(), String> {
        let note = match capacity::our_target(self.root).and_then(|t| verified_main(self.project, self.root, &t.join(gate::LOG_DIR), sub)) {
            Ok(Some(main)) => {
                println!("land: main {} is the tree full gate {sub} tested: it counts as a passed full gate of main", short(&main));
                format!("{}; {note}", fullgate::landed(&main))
            }
            Ok(None) => note.to_owned(),
            Err(e) => {
                eprintln!("warning: land: whether main is the tree gate {sub} tested: {e}");
                note.to_owned()
            }
        };
        gate::close(self.project, self.root, &self.agent, sub, &note)
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// `main`'s commit, fetched now, when gate `sub` (its `<sub>.rc` in `logs`) was a full gate that
/// passed and `main` is exactly the tree it tested: every PR of its stack landed, onto the `main`
/// it was stacked on (mornycorp/swarm-core#15). `land` then closes the gate with
/// [`fullgate::landed`], so the periodic full gate counts it as a pass of `main`. `None` for any
/// other kind, or another tree (a stack landed in part, a `main` that moved meanwhile).
pub fn verified_main(project: &Project, root: &Path, logs: &Path, sub: &str) -> Result<Option<String>, String> {
    let rc = logs.join(format!("{sub}.rc"));
    let line = std::fs::read_to_string(&rc).map_err(|e| format!("{}: {e}", rc.display()))?;
    if gate::rc_kind(&line) != Some(gate::Kind::Full) {
        return Ok(None);
    }
    let tested = gate::passed_head(&line)?;
    let (remote, branch) = (project.branches.remote, project.branches.main);
    stack::git_slow(root, &["fetch", "--quiet", remote, branch], &[])?;
    let main = stack::rev(root, &format!("refs/remotes/{remote}/{branch}"))?;
    Ok((tree_of(root, &main)? == tree_of(root, tested)?).then_some(main))
}

#[derive(Debug, PartialEq, Eq)]
enum Owner {
    Default,
    Path(PathBuf),
    None,
}

#[derive(Debug, PartialEq, Eq)]
struct Opts {
    mrs: Vec<u64>,
    sub: String,
    agent: Option<String>,
    summary: Option<String>,
    owner: Owner,
    /// Why it lands while `main` is red.
    while_red: Option<String>,
}

fn parse_args(project: &Project, args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts { mrs: Vec::new(), sub: String::new(), agent: None, summary: None, owner: Owner::Default, while_red: None };
    let mut it = args.iter().copied();
    let value = |it: &mut dyn Iterator<Item = &str>, flag: &str| {
        it.next().map(str::to_owned).ok_or_else(|| format!("land: {flag} needs a value\n\n{}", usage(project)))
    };
    while let Some(a) = it.next() {
        match a {
            "--sub" => o.sub = value(&mut it, a)?,
            "--agent" => o.agent = Some(value(&mut it, a)?),
            "--summary" => o.summary = Some(value(&mut it, a)?),
            "--owner" => o.owner = Owner::Path(PathBuf::from(value(&mut it, a)?)),
            "--no-owner" => o.owner = Owner::None,
            "--while-red" => {
                let why = value(&mut it, a)?;
                if why.trim().is_empty() {
                    return Err(format!("land: --while-red needs a reason\n\n{}", usage(project)));
                }
                o.while_red = Some(why);
            }
            "-h" | "--help" => return Err(usage(project)),
            n if !n.starts_with('-') => {
                let m: u64 = n.trim_start_matches('#').parse().map_err(|_| format!("land: `{n}` is not a PR number\n\n{}", usage(project)))?;
                if o.mrs.contains(&m) {
                    return Err(format!("land: #{m} given twice"));
                }
                o.mrs.push(m);
            }
            other => return Err(format!("land: unknown option `{other}`\n\n{}", usage(project))),
        }
    }
    if o.mrs.is_empty() {
        return Err(format!("land: which PRs?\n\n{}", usage(project)));
    }
    if o.sub.is_empty() {
        return Err(format!("land: --sub <gate sub> is required: the merges are logged inside the gate\n\n{}", usage(project)));
    }
    event::check_id(&o.sub)?;
    Ok(o)
}

/// What gate `sub` left in `logs`: its summary (`given`, or its log's) and the HEAD it tested (its
/// `<sub>.rc`, which must say it passed), with a rerun's evidence.
fn gate_record(logs: &Path, sub: &str, given: Option<&str>) -> Result<(String, String), String> {
    let summary = match given {
        Some(s) => s.to_owned(),
        None => {
            let log = logs.join(format!("{sub}.log"));
            let text = std::fs::read_to_string(&log).map_err(|e| format!("land: the gate's log {}: {e} (or pass --summary)", log.display()))?;
            gate_summary(&text).map_err(|e| format!("land: {e}"))?
        }
    };
    let rc = logs.join(format!("{sub}.rc"));
    let line = std::fs::read_to_string(&rc).map_err(|e| format!("land: the gate's record {}: {e}: gate the stack first", rc.display()))?;
    let tested = gate::passed_head(&line).map_err(|e| format!("land: gate {sub}: {e}"))?;
    let summary = match gate::rerun_evidence(logs, &line) {
        Some(e) => cap(&format!("{summary}\n\n{e}")),
        None => summary,
    };
    Ok((summary, tested.to_owned()))
}

/// `cargo xtask land …` ([`USAGE`]) for `project`.
pub fn run(project: &Project, root: &Path, args: &[&str]) -> Result<(), String> {
    let o = parse_args(project, args)?;
    // Everything is checked before anything is pushed.
    let agent = lock::agent(project, o.agent.clone(), "land")?;
    let mut stack = stack::load(project, root)?;
    let todo = pick(&stack, &o.mrs)?;
    with_work(root, &stack, todo)?;
    stack::check_clean(root)?;
    let (summary, tested) = gate_record(&capacity::our_target(root)?.join(gate::LOG_DIR), &o.sub, o.summary.as_deref())?;
    // A red main: nothing merges but its fix, or with a reason.
    let red = claim::main_red(project).map_err(|e| format!("land: couldn't check whether main is red: {e}"))?;
    let note = red_check(&red, todo, o.while_red.as_deref())?;
    let owner = match o.owner {
        Owner::Default => default_owner(root),
        Owner::Path(p) => Some(p),
        Owner::None => None,
    };
    // The gate's lock is the coordinator's while it lands (refused while another holds it).
    lock::take(project, root, &agent)?;
    let plan = Plan { project, root, sub: &o.sub, summary: &summary, tested: &tested, owner: owner.as_deref(), note: note.as_deref() };
    let landed = land(&plan, &mut stack, &o.mrs, &Real { project, root, agent })?;
    println!("land: merged {}; gate {} closed", bangs(&landed), o.sub);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::APP_SWARM as P;
    use serde_json::json;
    use std::cell::RefCell;

    #[test]
    fn a_branch_that_moved_is_never_pushed_over() {
        assert_eq!(remote_state(Some("old"), "old", "new"), Remote::Unmoved);
        assert_eq!(remote_state(Some("new"), "old", "new"), Remote::Pushed);
        assert_eq!(remote_state(Some("worker"), "old", "new"), Remote::Moved("worker".into()));
        assert_eq!(remote_state(None, "old", "new"), Remote::Missing);
    }

    /// A branch a worker moved since `stack` lands only when it is on the base the gate
    /// tested and, restacked onto it, is the gated tree.
    #[test]
    fn a_moved_branch_lands_only_with_the_gated_tree_on_the_gated_base() {
        let never = |_: &str| -> Result<Restack, String> { panic!("asked git though the branch didn't move") };
        for not_moved in [Remote::Unmoved, Remote::Pushed, Remote::Missing] {
            assert_eq!(moved(&not_moved, never), Ok(Moved::No), "{not_moved:?}");
        }
        let facts = |base_tested, own: &str, tree: Option<&str>| Restack {
            base_tested,
            gated_tree: "gated".into(),
            own_tree: own.into(),
            tree: tree.map(str::to_owned),
        };
        let worker = Remote::Moved("w".into());
        let asked = |r: Restack| {
            move |now: &str| {
                assert_eq!(now, "w");
                Ok(r)
            }
        };
        // Rebased onto the same base: its own tree is the gated one.
        assert_eq!(moved(&worker, asked(facts(true, "gated", Some("gated")))), Ok(Moved::SameTree { now: "w".into(), own: true }));
        // On an older base, reworded: restacked it is the gated tree.
        assert_eq!(moved(&worker, asked(facts(true, "older", Some("gated")))), Ok(Moved::SameTree { now: "w".into(), own: false }));
        // A new commit: another tree, or one that conflicts restacked.
        assert_eq!(moved(&worker, asked(facts(true, "fix", Some("fix")))), Ok(Moved::AnotherTree("w".into())));
        assert_eq!(moved(&worker, asked(facts(true, "fix", None))), Ok(Moved::AnotherTree("w".into())));
        // Onto a newer main the gate didn't test, whatever the trees say.
        assert_eq!(moved(&worker, asked(facts(false, "gated", Some("gated")))), Ok(Moved::AnotherBase("w".into())));
        assert_eq!(moved(&worker, |_| Err("no base".into())), Err("no base".into()));
    }

    #[test]
    fn waiting_until_it_is_mergeable() {
        let v = |state: &str, merge_state: &str| MrView { state: state.into(), merge_state: merge_state.into(), sha: "abc".into(), node_id: "PR_1".into() };
        assert_eq!(wait_state(&v("open", "clean")), Wait::Ready);
        assert_eq!(wait_state(&v("open", "unstable")), Wait::Ready, "a cancelled check that isn't required");
        assert_eq!(wait_state(&v("open", "unknown")), Wait::Again);
        assert_eq!(wait_state(&v("open", "")), Wait::Again);
        assert_eq!(wait_state(&v("open", "behind")), Wait::Behind);
        assert_eq!(wait_state(&v("merged", "")), Wait::Merged);
        assert!(matches!(wait_state(&v("open", "dirty")), Wait::Stuck(w) if w.contains("conflicts")));
        assert!(matches!(wait_state(&v("open", "blocked")), Wait::Stuck(w) if w.contains("branch protection")));
        assert!(matches!(wait_state(&v("closed", "")), Wait::Stuck(w) if w.contains("closed")));
        let parsed = parse_view(&json!({"number": 1, "state": "open", "merge_state": "clean", "head_sha": "f00", "node_id": "PR_9"}));
        assert_eq!(parsed, MrView { state: "open".into(), merge_state: "clean".into(), sha: "f00".into(), node_id: "PR_9".into() });
    }

    #[test]
    fn docs_only_merges_skip_mains_workflows() {
        assert_eq!(merge_message("Fold the batch", true).as_deref(), Some("Fold the batch\n\n[skip ci]"));
        assert_eq!(merge_message("Code", false), None);
    }

    /// mornycorp/swarm-core#15: a full gate whose merges left `main` at exactly the tree it
    /// tested verifies that commit of `main` (the server's merge commit: another commit, the same
    /// tree); a smoke's, one landed onto a `main` that moved since, and a failed one don't.
    #[test]
    fn a_landed_full_gate_verifies_main_only_at_its_tree() {
        use crate::stack::testgit;
        let dir = swarm_core_testkit::temp_dir("xtask-land-verified-main");
        let (origin, coord, logs) = (dir.join("origin.git"), dir.join("coord"), dir.join("logs"));
        testgit::bare(&origin);
        let mut h = testgit::History::default();
        let base = h.commit("main", &[], &[("README.md", "base")], "base");
        h.commit("s-1", &[&base], &[("one.rs", "1")], "the stack's top");
        let tested = h.import(&origin).remove(1);
        testgit::clone(&origin, &coord);
        std::fs::create_dir_all(&logs).unwrap();
        let record = |line: &str| std::fs::write(logs.join("gate-9.rc"), format!("{line} head={tested}\n")).unwrap();
        let verified = || verified_main(&P, &coord, &logs, "gate-9");
        record("rc=0 1700s kind=full (--full)");
        assert_eq!(verified(), Ok(None), "not merged yet: main isn't its tree");
        let merged = testgit::push_commit(&origin, "main", &["main", "s-1"], &[("one.rs", "1")], "Merge branch 's-1'");
        assert_eq!(verified(), Ok(Some(merged)));
        record("rc=0 300s kind=smoke (reaches every package)");
        assert_eq!(verified(), Ok(None), "a smoke verifies no main");
        record("rc=1 1700s cause=test kind=full (--full)");
        assert!(verified().unwrap_err().contains("did not pass"));
        record("rc=0 1700s kind=full (--full)");
        testgit::push_commit(&origin, "main", &["main"], &[("two.rs", "2")], "another merge");
        assert_eq!(verified(), Ok(None), "main moved past its tree");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_passed_gate_lands() {
        let ok = "==> cargo fmt\n...\nCI summary: all 6 steps passed (fmt, clippy, test, layers, wasm, scorecard) in 7.1 min\nexit 0\n";
        assert_eq!(gate_summary(ok).unwrap(), "CI summary: all 6 steps passed (fmt, clippy, test, layers, wasm, scorecard) in 7.1 min\nexit 0");
        let failed = "CI summary:\n  ok    fmt\n  FAIL  test: cargo test failed\n  skip  layers\n";
        assert!(gate_summary(failed).unwrap_err().contains("did not pass"));
        assert!(gate_summary("still building").unwrap_err().contains("no `CI summary`"));
        // An earlier run's pass doesn't count past a later failure.
        assert!(gate_summary(&format!("{ok}{failed}")).is_err());
        assert!(gate_summary(&format!("{ok}{}", "x".repeat(10_000))).unwrap().ends_with('…'));
    }

    #[test]
    fn each_mr_notes_the_gate_as_the_project_names_it() {
        let body = note_body(P.gate_tag, "gate-9", "CI summary: all 7 steps passed", &[1, 2]);
        assert!(body.starts_with("Local gate `gate-9` on the rebased stack (#1 #2), before merging:"), "{body}");
        let ns = crate::project::fixtures::GPU_SWARM;
        assert!(note_body(ns.gate_tag, "gate-9", "s", &[1]).starts_with("Local gate `gate-9` (GPU) on"));
    }

    /// The gate's record, with its kind's free text before the HEAD (the second project's way) and
    /// without (the first project's): a pass names the HEAD it tested; a failure, or no record, lands nothing.
    #[test]
    fn a_gate_record_names_the_head_it_tested() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-land-record");
        let head = "e".repeat(40);
        let summary = "CI summary: all 8 steps passed";
        std::fs::write(dir.join("gate-9.rc"), format!("rc=0 10s kind=tooling (builds only xtask) head={head}\n")).unwrap();
        std::fs::write(dir.join("gate-9.log"), format!("x\n{summary}\n")).unwrap();
        assert_eq!(gate_record(&dir, "gate-9", None).unwrap(), (summary.to_owned(), head.clone()));
        assert_eq!(gate_record(&dir, "gate-9", Some("given")).unwrap(), ("given".to_owned(), head.clone()));
        std::fs::write(dir.join("gate-8.rc"), format!("rc=1 10s cause=test kind=full (--full) head={head}\n")).unwrap();
        assert!(gate_record(&dir, "gate-8", Some("given")).unwrap_err().contains("did not pass"));
        assert!(gate_record(&dir, "gate-7", Some("given")).unwrap_err().contains("gate the stack first"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn entry(mr: u64) -> Entry {
        Entry {
            mr,
            source_branch: format!("{mr}-x"),
            title: format!("PR {mr}"),
            issues: vec![mr + 1000],
            old_tip: "o".into(),
            new_tip: "n".into(),
            onto: "b".into(),
            docs_only: false,
            merged_base: false,
            regenerated: Vec::new(),
        }
    }

    #[test]
    fn the_stack_lands_from_the_bottom_in_order() {
        let stack = Stack { remote: "origin".into(), base: "main".into(), entries: vec![entry(1), entry(2), entry(3)] };
        assert_eq!(pick(&stack, &[1, 2]).unwrap().len(), 2);
        assert_eq!(pick(&stack, &[1, 2, 3]).unwrap().len(), 3);
        for bad in [&[2][..], &[1, 3], &[2, 1], &[1, 2, 3, 4], &[]] {
            assert!(pick(&stack, bad).unwrap_err().contains("#1 #2 #3"), "{bad:?}");
        }
    }

    /// While a `main-red` issue is open, only its fix lands (every PR closes one), or a
    /// stack given a reason, which each merge event's note carries.
    #[test]
    fn a_red_main_lands_only_its_fix_or_with_a_reason() {
        let red = [(221, "main is red: the full gate of".to_owned()), (222, "main is red: clippy".to_owned())];
        let fix = Entry { issues: vec![221], ..entry(1) };
        let other = entry(2);
        // Open, and the PR closes it.
        assert_eq!(red_check(&red, std::slice::from_ref(&fix), None), Ok(None));
        // Open, and it doesn't: refused, naming the issues and the PR.
        let why = red_check(&red, std::slice::from_ref(&other), None).unwrap_err();
        assert!(why.contains("#221 main is red: the full gate of") && why.contains("#222 main is red: clippy"), "{why}");
        assert!(why.contains("#2 closes none") && why.contains("--while-red"), "{why}");
        // A stack with the fix under another PR: every PR must close one.
        let why = red_check(&red, &[fix.clone(), other.clone()], None).unwrap_err();
        assert!(why.contains("#2 closes none") && !why.contains("#1"), "{why}");
        // Open, with --while-red: lands, the why noted.
        assert_eq!(
            red_check(&red, &[fix, other.clone()], Some("#222 is a flake")),
            Ok(Some("landed while main is red (#221 #222): #222 is a flake".to_owned()))
        );
        // None open: lands, no note, whatever was given.
        assert_eq!(red_check(&[], std::slice::from_ref(&other), None), Ok(None));
        assert_eq!(red_check(&[], &[other], Some("why")), Ok(None));
    }

    #[test]
    fn only_the_head_the_gate_tested_lands() {
        let mut stack = Stack { remote: "origin".into(), base: "main".into(), entries: vec![entry(1), entry(2)] };
        stack.entries[1].new_tip = "top".into();
        assert_eq!(untested(&stack, "gate-9", "top"), None);
        let e = untested(&stack, "gate-9", "n").unwrap();
        assert!(e.contains("top is top (#2)") && e.contains("gate-9 tested n"), "{e}");
        assert_eq!(untested(&Stack::default(), "gate-9", "n"), None, "nothing to land: `pick` refuses it");
    }

    /// A gate whose only failures were tests that then passed alone lands on its rerun's
    /// record, for the HEAD both tested, and each PR's note says how it was verified; the failed
    /// gate, and a rerun whose test failed again, land nothing.
    #[test]
    fn a_verified_rerun_lands_with_its_evidence_and_a_failed_one_does_not() {
        let logs = swarm_core_testkit::temp_dir("xtask-land-rerun");
        let head = "c".repeat(40);
        let write = |name: &str, text: &str| std::fs::write(logs.join(name), text).unwrap();
        write("gate-9.rc", &format!("rc=1 700s cause=test kind=full head={head}\n"));
        write(
            "gate-9.log",
            "        FAIL [ 300.1s] (921/921) swarm-engine::panic_hunt every_command\nerror: test run failed\n\nCI summary:\n  ok    fmt\n  ok    clippy\n  FAIL  test: cargo nextest run failed\n  skip  layers\n",
        );
        write("gate-9-rerun.rc", &format!("rc=0 40s kind=full head={head} of=gate-9 rerun=swarm-engine::panic_hunt::every_command\n"));
        write("gate-9-rerun.log", "\nCI summary: all 3 steps passed (alone swarm-engine::panic_hunt every_command, doctests, layers) in 0.7 min\n");
        let (summary, tested) = gate_record(&logs, "gate-9-rerun", None).unwrap();
        assert_eq!(tested, head);
        assert!(summary.starts_with("CI summary: all 3 steps passed (alone swarm-engine::panic_hunt every_command"), "{summary}");
        assert!(summary.contains("Verified by a rerun of gate `gate-9`") && summary.contains("  swarm-engine::panic_hunt::every_command\n"), "{summary}");
        assert!(summary.ends_with("gate-9's CI summary:\n  ok    fmt\n  ok    clippy\n  FAIL  test: cargo nextest run failed\n  skip  layers"), "{summary}");
        assert!(gate_record(&logs, "gate-9", None).unwrap_err().contains("did not pass"));
        // The test failed again alone: the rerun's record is no pass, whatever summary is given.
        write("gate-9-rerun.rc", &format!("rc=1 12s cause=test kind=full head={head} of=gate-9 rerun=swarm-engine::panic_hunt::every_command\n"));
        assert!(gate_record(&logs, "gate-9-rerun", Some("CI summary: all 3 steps passed")).unwrap_err().contains("did not pass"));
        // A gate that isn't a rerun notes its summary alone.
        write("gate-10.rc", &format!("rc=0 700s kind=full head={head}\n"));
        assert_eq!(gate_record(&logs, "gate-10", Some("CI summary: all 6 steps passed")).unwrap().0, "CI summary: all 6 steps passed");
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn arguments() {
        let o = parse_args(&P, &["186", "#187", "--sub", "gate-171", "--no-owner"]).unwrap();
        assert_eq!(o, Opts { mrs: vec![186, 187], sub: "gate-171".into(), agent: None, summary: None, owner: Owner::None, while_red: None });
        let o = parse_args(&P, &["186", "--sub", "gate-171", "--while-red", "the red is #221's flake"]).unwrap();
        assert_eq!(o.while_red.as_deref(), Some("the red is #221's flake"));
        for bad in [
            &["186"][..],
            &["--sub", "g"],
            &["1", "--sub"],
            &["1", "--sub", "a b"],
            &["1", "1", "--sub", "g"],
            &["1", "--sub", "g", "--force"],
            &["1", "--sub", "g", "--while-red"],
            &["1", "--sub", "g", "--while-red", " "],
        ] {
            assert!(parse_args(&P, bad).is_err(), "{bad:?}");
        }
    }

    // --- The order of the steps, with GitHub faked and the pushes real. ---

    use crate::stack::testgit::{self, tgit};

    /// GitHub and the log, recording each call; `merges` says whether a merge takes. A PR's
    /// head is its branch's on `origin`, as the server would report it, and it is `behind` while
    /// `origin`'s `main` isn't in it.
    struct Fake {
        origin: PathBuf,
        /// Runs at the first poll: a push while `land` waits.
        during_wait: RefCell<Option<Box<dyn FnOnce()>>>,
        calls: RefCell<Vec<String>>,
        merged: RefCell<std::collections::BTreeSet<u64>>,
        merges: bool,
        /// Polls answered "unknown" before the state.
        checking: RefCell<u32>,
        /// PRs whose server rebase is refused (merge commits).
        refuse_rebase: std::collections::BTreeSet<u64>,
        /// Merges refused before one takes.
        refuse_merges: RefCell<u32>,
    }

    impl Fake {
        fn new(w: &World, merges: bool) -> Self {
            Self {
                origin: w.origin.clone(),
                during_wait: RefCell::default(),
                calls: RefCell::default(),
                merged: RefCell::default(),
                merges,
                checking: RefCell::new(1),
                refuse_rebase: std::collections::BTreeSet::new(),
                refuse_merges: RefCell::new(0),
            }
        }
        fn log(&self, s: String) {
            self.calls.borrow_mut().push(s);
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Effects for Fake {
        fn mr(&self, mr: u64) -> Result<MrView, String> {
            if let Some(push) = self.during_wait.borrow_mut().take() {
                push();
            }
            let merged = self.merged.borrow().contains(&mr);
            let head = tgit(&self.origin, &["for-each-ref", "--format=%(objectname)", &format!("refs/heads/{mr}-*")]).trim().to_owned();
            let behind = crate::process::git(&self.origin, &["merge-base", "--is-ancestor", "refs/heads/main", &head], None, &[]).is_err();
            let mut checking = self.checking.borrow_mut();
            let merge_state = if *checking > 0 {
                *checking -= 1;
                "unknown"
            } else if behind {
                "behind"
            } else {
                "clean"
            };
            self.log(format!("mr {mr}"));
            Ok(MrView { state: if merged { "merged" } else { "open" }.into(), merge_state: merge_state.into(), sha: head, node_id: format!("PR_{mr}") })
        }
        fn note(&self, mr: u64, body: &str) -> Result<(), String> {
            assert!(body.contains("CI summary"), "{body}");
            self.log(format!("note {mr}"));
            Ok(())
        }
        fn cancel_runs(&self, mr: u64) -> Result<(), String> {
            self.log(format!("cancel {mr}"));
            Ok(())
        }
        fn rebase(&self, mr: u64, v: &MrView) -> Result<Option<String>, String> {
            assert_eq!(v.node_id, format!("PR_{mr}"));
            self.log(format!("rebase {mr}"));
            Ok(self.refuse_rebase.contains(&mr).then(|| "GitHub can't rebase this branch".to_owned()))
        }
        fn merge(&self, mr: u64, sha: &str, message: Option<&str>) -> Result<(), String> {
            self.log(format!("merge {mr} {sha} {}", message.map_or("-", |m| if m.contains("[skip ci]") { "[skip ci]" } else { m })));
            let mut refuse = self.refuse_merges.borrow_mut();
            if *refuse > 0 {
                *refuse -= 1;
                return Err("gh api repos/o/r/pulls/1/merge: Base branch was modified (HTTP 405)".into());
            }
            if self.merges {
                self.merged.borrow_mut().insert(mr);
            }
            Ok(())
        }
        fn delete_branch(&self, branch: &str) -> Result<(), String> {
            self.log(format!("delete {branch}"));
            Ok(())
        }
        fn log_merge(&self, issue: u64, mr: u64, sub: &str, note: Option<&str>) -> Result<(), String> {
            self.log(format!("event merge #{issue} #{mr} {sub}{}", note.map(|n| format!(" --note {n}")).unwrap_or_default()));
            Ok(())
        }
        fn tidy(&self, issue: u64) -> Result<(), String> {
            self.log(format!("tidy {issue}"));
            Ok(())
        }
        fn close_gate(&self, sub: &str, note: &str) -> Result<(), String> {
            self.log(format!("gate --stop {sub} {note}"));
            Ok(())
        }
        fn sleep(&self, _: Duration) {}
    }

    struct World {
        origin: PathBuf,
        coord: PathBuf,
        owner: PathBuf,
    }

    /// An origin with `main` and two PR branches; the coordinator's clone, with them stacked as
    /// `s-<pr>` and recorded as `stack` leaves them; and, with `owner`, the owner's clone, on
    /// `main` from before it moved on. The commits are written straight into the repositories
    /// (`testgit::History`) and the stack is made here, not by `stack::build` (its own tests
    /// cover it): a few git processes where pushes from a worker's clone and the rebases took
    /// about a hundred.
    fn world(name: &str, owner: bool) -> (World, Stack) {
        let dir = swarm_core_testkit::temp_dir(&format!("xtask-land-{name}"));
        let w = World { origin: dir.join("origin.git"), coord: dir.join("coord"), owner: dir.join("owner") };
        testgit::bare(&w.origin);
        // One is docs only, two (stacked on it) code.
        let mut h = testgit::History::default();
        let first = h.commit("main", &[], &[("README.md", "first")], "first");
        let one = h.commit("1-one", &[&first], &[("one.md", "1-one")], "1-one");
        h.commit("2-two", &[&one], &[("two.rs", "2-two")], "2-two");
        let tips = h.import(&w.origin);
        if owner {
            testgit::clone(&w.origin, &w.owner);
        }
        // Main moves on, so the stack's pushes rewrite both branches.
        testgit::push_commit(&w.origin, "main", &["main"], &[("main.md", "main moves on")], "main moves on");
        testgit::clone(&w.origin, &w.coord);
        let mut h = testgit::History::default();
        let s1 = h.commit("s-1", &["refs/remotes/origin/main"], &[("one.md", "1-one")], "1-one");
        h.commit("s-2", &[&s1], &[("two.rs", "2-two")], "2-two");
        let stacked = h.import(&w.coord);
        let main = tgit(&w.coord, &["rev-parse", "refs/remotes/origin/main"]).trim().to_owned();
        let stack = recorded(&tips[1..], &stacked, &main);
        stack::save(&P, &w.coord, &stack).unwrap();
        (w, stack)
    }

    /// The record `stack` makes of PRs 1 (docs only) and 2: their branches' tips before (`old`)
    /// and after (`new`) the rebase, the first stacked onto `main`, the second onto the first.
    fn recorded(old: &[String], new: &[String], main: &str) -> Stack {
        let entry = |i: usize, branch: &str, docs_only| {
            let mr = i as u64 + 1;
            Entry {
                mr,
                source_branch: branch.into(),
                title: format!("PR {mr}"),
                issues: vec![mr + 100],
                old_tip: old[i].clone(),
                new_tip: new[i].clone(),
                onto: if i == 0 { main.to_owned() } else { new[i - 1].clone() },
                docs_only,
                merged_base: false,
                regenerated: Vec::new(),
            }
        };
        Stack { remote: "origin".into(), base: "main".into(), entries: vec![entry(0, "1-one", true), entry(1, "2-two", false)] }
    }

    fn top(stack: &Stack) -> String {
        stack.entries.last().unwrap().new_tip.clone()
    }

    fn origin_tip(w: &World, branch: &str) -> String {
        tgit(&w.origin, &["rev-parse", &format!("refs/heads/{branch}")]).trim().to_owned()
    }

    /// A PR un-drafted with only its claim commit was once landed. A PR with no work of its
    /// own against what it was stacked onto is refused before anything is pushed.
    #[test]
    fn an_mr_with_only_its_claim_commit_never_lands() {
        let (w, mut stack) = world("empty", false);
        assert_eq!(with_work(&w.coord, &stack, &stack.entries), Ok(()), "real changes land");
        let mut old = stack.clone();
        old.entries[0].onto.clear();
        assert_eq!(with_work(&w.coord, &old, &old.entries[..1]), Ok(()), "a record from before #222: onto <remote>/<base>");

        let below = top(&stack);
        let claim = testgit::push_commit(&w.coord, "s-3", &["s-2"], &[], "Claim #3: nothing yet");
        let mut e = stack.entries[1].clone();
        (e.mr, e.source_branch, e.old_tip, e.new_tip, e.onto) = (3, "3-three".into(), claim.clone(), claim, below);
        stack.entries.push(e);
        let why = with_work(&w.coord, &stack, &stack.entries).unwrap_err();
        assert!(why.contains("#3 changes nothing") && why.contains("only its claim commit") && why.contains("close the PR"), "{why}");
    }

    #[test]
    fn a_stack_lands_in_order_and_deletes_each_branch_only_after_its_merge() {
        let (w, mut stack) = world("ok", true);
        let summary = "CI summary: all 6 steps passed";
        // Landed while main is red: each merge event carries the why.
        let plan = Plan { project: &P, root: &w.coord, sub: "gate-9", summary, tested: &top(&stack), owner: Some(&w.owner), note: Some("red: why") };
        let fake = Fake::new(&w, true);
        assert_eq!(land(&plan, &mut stack, &[1, 2], &fake).unwrap(), vec![1, 2]);

        // Each branch on origin is its rebased tip.
        assert_eq!(origin_tip(&w, "1-one"), tgit(&w.coord, &["rev-parse", "s-1"]).trim());
        assert_eq!(origin_tip(&w, "2-two"), tgit(&w.coord, &["rev-parse", "s-2"]).trim());
        let calls = fake.calls();
        let at = |c: &str| calls.iter().position(|x| x == c).unwrap_or_else(|| panic!("no `{c}` in {calls:#?}"));
        // Per PR: note, cancel, merge (no rebase: each is up to main), the merge confirmed, its
        // event and tidy, then the delete.
        let merges = [format!("merge 1 {} [skip ci]", origin_tip(&w, "1-one")), format!("merge 2 {} -", origin_tip(&w, "2-two"))];
        for (m, merge) in [(1, merges[0].as_str()), (2, merges[1].as_str())] {
            assert!(at(&format!("note {m}")) < at(&format!("cancel {m}")));
            assert!(at(&format!("cancel {m}")) < at(merge));
            assert!(!calls.contains(&format!("rebase {m}")), "{calls:?}");
            let confirmed = calls.iter().enumerate().skip(at(merge)).find(|(_, c)| **c == format!("mr {m}")).map(|(i, _)| i).unwrap();
            assert!(confirmed < at(&format!("event merge #{} #{m} gate-9 --note red: why", m + 100)));
            assert!(at(&format!("tidy {}", m + 100)) < at(&format!("delete {m}-{}", if m == 1 { "one" } else { "two" })));
        }
        assert!(at("delete 1-one") < at("note 2"));
        assert_eq!(calls.last().unwrap(), "gate --stop gate-9 merged #1 #2");
        // The record is used up; the owner's checkout was on main and clean: fast-forwarded.
        assert!(stack::load(&P, &w.coord).is_err());
        assert_eq!(tgit(&w.owner, &["rev-parse", "HEAD"]).trim(), origin_tip(&w, "main"));
    }

    /// Main moved on, so the PR is behind; the server refuses to rebase (merge commits): main is
    /// merged into the branch and pushed, and the merge goes on without another server rebase. A
    /// merge GitHub refuses once is asked again (`MERGE_TRIES`).
    #[test]
    fn a_refused_rebase_merges_main_in_and_a_refused_merge_is_asked_again() {
        let (w, mut stack) = world("refused", false);
        // Main moves on after the stack was made, so merging it in makes a new commit.
        testgit::push_commit(&w.origin, "main", &["main"], &[("later.md", "later")], "main moves on again");
        let summary = "CI summary: all 6 steps passed";
        let plan = Plan { project: &P, root: &w.coord, sub: "gate-9", summary, tested: &top(&stack), owner: None, note: Some("red: why") };
        let mut fake = Fake::new(&w, true);
        fake.refuse_rebase.insert(1);
        *fake.refuse_merges.borrow_mut() = 1;
        assert_eq!(land(&plan, &mut stack, &[1], &fake).unwrap(), vec![1]);
        let tip = origin_tip(&w, "1-one");
        tgit(&w.coord, &["merge-base", "--is-ancestor", &origin_tip(&w, "main"), &tip]);
        tgit(&w.coord, &["rev-parse", "--verify", "--quiet", &format!("{tip}^2")]);
        let calls = fake.calls();
        assert_eq!(calls.iter().filter(|c| c.starts_with("merge 1")).count(), 2, "{calls:?}");
        assert_eq!(calls.iter().filter(|c| c.starts_with("rebase 1")).count(), 1, "{calls:?}");
        assert!(calls.contains(&"delete 1-one".to_owned()));
        assert!(calls.contains(&"event merge #101 #1 gate-9 --note red: why".to_owned()), "{calls:?}");
        // A merge refused every time stops, the branch kept.
        let (w, mut stack) = world("refused-always", false);
        let plan = Plan { root: &w.coord, tested: &top(&stack), ..plan };
        let fake = Fake::new(&w, true);
        *fake.refuse_merges.borrow_mut() = MERGE_TRIES;
        let e = land(&plan, &mut stack, &[1], &fake).unwrap_err();
        assert!(e.contains(&format!("the merge was refused {MERGE_TRIES} times")), "{e}");
        assert!(!fake.calls().iter().any(|c| c.starts_with("delete")), "{:?}", fake.calls());
    }

    /// `s-<pr>` is what the gate tested: the recorded tip, or `land`'s own pushed merge of `main`
    /// on it; nothing else.
    #[test]
    fn only_the_tip_the_gate_tested_or_lands_own_merge_on_it_is_pushed() {
        let never = || -> bool { panic!("asked git though the tip is the recorded one") };
        assert!(tested_tip("new", "new", None, never));
        assert!(tested_tip("merged", "new", Some("merged"), || true), "main merged in and pushed by land");
        assert!(!tested_tip("merged", "new", Some("merged"), || false), "not on the tested tip");
        assert!(!tested_tip("moved", "new", Some("old"), || true), "moved here, not pushed");
        assert!(!tested_tip("moved", "new", None, || true));
    }

    #[test]
    fn a_branch_a_worker_moved_is_left_alone_and_nothing_merges() {
        let (w, mut stack) = world("moved", false);
        // The worker pushes again after the stack was made.
        let workers = testgit::push_commit(&w.origin, "1-one", &["1-one"], &[("late.md", "late")], "the worker's late fix");
        assert_eq!(origin_tip(&w, "1-one"), workers);

        let plan =
            Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &top(&stack), owner: None, note: None };
        let fake = Fake::new(&w, true);
        let e = land(&plan, &mut stack, &[1, 2], &fake).unwrap_err();
        assert!(e.contains("#1 stopped") && e.contains("moved since `stack`") && e.contains("gate stays open"), "{e}");
        assert!(e.contains("its tree is not the one gated"), "{e}");
        assert_eq!(origin_tip(&w, "1-one"), workers, "the worker's commit survives");
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        assert_eq!(stack::load(&P, &w.coord).unwrap().entries.len(), 2);
    }

    /// The worker rebased its branch onto the `main` the stack was made on while the gate
    /// ran (the same patch, another commit): its tree is the gated one, so the stacked commit is
    /// pushed over it and the PR lands.
    #[test]
    fn a_branch_rebased_onto_the_gated_base_with_the_gated_tree_lands() {
        let (w, mut stack) = world("rebased", false);
        let workers = testgit::push_commit(&w.origin, "1-one", &["main"], &[("one.md", "1-one")], "1-one, rebased by its worker");
        assert_ne!(workers, stack.entries[0].new_tip);
        let plan =
            Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &top(&stack), owner: None, note: None };
        let fake = Fake::new(&w, true);
        assert_eq!(land(&plan, &mut stack, &[1], &fake).unwrap(), vec![1]);
        assert_eq!(origin_tip(&w, "1-one"), tgit(&w.coord, &["rev-parse", "s-1"]).trim(), "the stacked commit pushed over the worker's");
        let calls = fake.calls();
        assert!(calls.iter().any(|c| c.starts_with("merge 1")) && calls.contains(&"delete 1-one".to_owned()), "{calls:?}");
        // Nothing of the gate's worktree moved: its branches are as `stack` left them.
        assert_eq!(tgit(&w.coord, &["rev-parse", "s-2"]).trim(), top(&stack));
    }

    /// The worker rebased onto a newer `main` than the gate tested: never pushed over.
    #[test]
    fn a_branch_rebased_onto_another_base_is_left_alone() {
        let (w, mut stack) = world("newer", false);
        testgit::push_commit(&w.origin, "main", &["main"], &[("later.md", "later")], "main moves on again");
        let workers = testgit::push_commit(&w.origin, "1-one", &["main"], &[("one.md", "1-one")], "1-one, on the newer main");
        let plan =
            Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &top(&stack), owner: None, note: None };
        let fake = Fake::new(&w, true);
        let e = land(&plan, &mut stack, &[1], &fake).unwrap_err();
        assert!(e.contains("#1 stopped") && e.contains("onto a base the gate didn't test"), "{e}");
        assert_eq!(origin_tip(&w, "1-one"), workers, "the worker's commit survives");
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    }

    #[test]
    fn a_merge_that_never_happens_deletes_nothing_and_keeps_the_gate_open() {
        let (w, mut stack) = world("unmerged", false);
        let plan =
            Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &top(&stack), owner: None, note: None };
        let fake = Fake::new(&w, false);
        let e = land(&plan, &mut stack, &[1], &fake).unwrap_err();
        assert!(e.contains("#1 stopped") && e.contains("not merged"), "{e}");
        let calls = fake.calls();
        assert!(calls.iter().any(|c| c.starts_with("merge 1")), "{calls:?}");
        assert!(!calls.iter().any(|c| c.starts_with("delete") || c.starts_with("event") || c.starts_with("tidy") || c.starts_with("gate")), "{calls:?}");
        // The branch is still there, at the pushed tip; a rerun goes on (it's pushed already).
        assert_eq!(origin_tip(&w, "1-one"), tgit(&w.coord, &["rev-parse", "s-1"]).trim());
        assert_eq!(remote_state(Some(&origin_tip(&w, "1-one")), &stack.entries[0].old_tip, &stack.entries[0].new_tip), Remote::Pushed);
        assert_eq!(stack::load(&P, &w.coord).unwrap().entries.len(), 2);
    }

    /// swarm-core#9: what the server would merge must carry the gated change: the pushed
    /// commit, the gated tree after the same rebase, or the gated patch.
    #[test]
    fn only_a_head_that_carries_the_gated_change_merges() {
        assert!(carries_gated(&Head { pushed: true, ..Head::default() }));
        let h = |tree: &str, gated_tree: Option<&str>, id: Option<&str>, gated_id: Option<&str>| Head {
            pushed: false,
            tree: tree.into(),
            gated_tree: gated_tree.map(str::to_owned),
            patch_id: id.map(str::to_owned),
            gated_patch_id: gated_id.map(str::to_owned),
        };
        assert!(carries_gated(&h("t", Some("t"), Some("p"), Some("q"))), "the server's rebase: the same tree");
        assert!(carries_gated(&h("t", Some("u"), Some("p"), Some("p"))), "the same patch");
        assert!(carries_gated(&h("t", None, Some("p"), Some("p"))), "the same patch, though restacking conflicts");
        assert!(!carries_gated(&h("fix", Some("t"), Some("f"), Some("p"))), "a worker's fix");
        assert!(!carries_gated(&h("fix", None, None, None)), "two empty diffs are not the gated change");
    }

    /// swarm-core#9: a worker pushes to the branch while `land` waits for it to be mergeable;
    /// the server's head is then the worker's commit, and it is never merged.
    #[test]
    fn a_push_while_land_waits_is_never_merged() {
        let (w, mut stack) = world("raced", false);
        let plan =
            Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &top(&stack), owner: None, note: None };
        let fake = Fake::new(&w, true);
        let origin = w.origin.clone();
        *fake.during_wait.borrow_mut() = Some(Box::new(move || {
            testgit::push_commit(&origin, "1-one", &["1-one"], &[("late.md", "late")], "the worker's fix, ungated");
        }));
        let e = land(&plan, &mut stack, &[1, 2], &fake).unwrap_err();
        assert!(e.contains("#1 stopped") && e.contains("not the change the gate tested") && e.contains("gate stays open"), "{e}");
        let workers = origin_tip(&w, "1-one");
        assert!(e.contains(short(&workers)), "{e}");
        let calls = fake.calls();
        assert!(!calls.iter().any(|c| ["merge", "delete", "event", "tidy", "gate"].iter().any(|p| c.starts_with(p))), "{calls:?}");
        assert_eq!(tgit(&w.origin, &["log", "-1", "--format=%s", "refs/heads/1-one"]).trim(), "the worker's fix, ungated", "the worker's commit survives");
        assert_eq!(stack::load(&P, &w.coord).unwrap().entries.len(), 2);
    }

    /// swarm-core#9: the server's rebase of the pushed commit onto a newer `main` (or a
    /// worker's rebase of the same patch) is the gated change on another base: it merges, at
    /// that head.
    #[test]
    fn the_servers_rebase_of_the_gated_change_merges() {
        let (w, mut stack) = world("server-rebase", false);
        let plan =
            Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &top(&stack), owner: None, note: None };
        let fake = Fake::new(&w, true);
        let origin = w.origin.clone();
        *fake.during_wait.borrow_mut() = Some(Box::new(move || {
            testgit::push_commit(&origin, "main", &["main"], &[("later.md", "later")], "main moves on again");
            testgit::push_commit(&origin, "1-one", &["main"], &[("one.md", "1-one")], "1-one, rebased by the server");
        }));
        assert_eq!(land(&plan, &mut stack, &[1], &fake).unwrap(), vec![1]);
        let rebased = origin_tip(&w, "1-one");
        assert_ne!(rebased, tgit(&w.coord, &["rev-parse", "s-1"]).trim());
        let calls = fake.calls();
        assert!(calls.contains(&format!("merge 1 {rebased} [skip ci]")), "{calls:?}");
        assert!(calls.contains(&"delete 1-one".to_owned()), "{calls:?}");
    }

    #[test]
    fn a_push_that_did_not_land_is_never_merged() {
        let (w, mut stack) = world("rejected", false);
        // The origin refuses every push to that branch.
        let hooks = w.origin.join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("pre-receive"), "#!/bin/sh\necho refused >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(hooks.join("pre-receive"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let before = origin_tip(&w, "1-one");
        let plan =
            Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &top(&stack), owner: None, note: None };
        let fake = Fake::new(&w, true);
        let e = land(&plan, &mut stack, &[1], &fake).unwrap_err();
        assert!(e.contains("the push didn't land") && e.contains("not merging"), "{e}");
        assert_eq!(origin_tip(&w, "1-one"), before);
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
    }

    /// A stack the gate didn't test, or a stacked branch moved since, never lands.
    #[test]
    fn what_the_gate_did_not_test_is_never_pushed() {
        let (w, mut stack) = world("untested", false);
        let before = (origin_tip(&w, "1-one"), origin_tip(&w, "2-two"));
        let fake = Fake::new(&w, true);
        // The gate tested the PR below the top (or a tree an aborted stack left).
        let below = stack.entries[0].new_tip.clone();
        let plan = Plan { project: &P, root: &w.coord, sub: "gate-9", summary: "CI summary: all 6 steps passed", tested: &below, owner: None, note: None };
        let e = land(&plan, &mut stack, &[1], &fake).unwrap_err();
        assert!(e.contains("gate-9 tested") && e.contains("never land what the gate didn't test"), "{e}");
        // The record's top is what the gate tested, but s-1 moved since the stack.
        tgit(&w.coord, &["branch", "--quiet", "-f", "s-1", "s-2"]);
        let plan = Plan { tested: &top(&stack), ..plan };
        let e = land(&plan, &mut stack, &[1], &fake).unwrap_err();
        assert!(e.contains("#1 stopped") && e.contains("not the") && e.contains("the gate tested: restack"), "{e}");
        assert_eq!((origin_tip(&w, "1-one"), origin_tip(&w, "2-two")), before, "nothing pushed");
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        assert_eq!(stack::load(&P, &w.coord).unwrap().entries.len(), 2);
    }

    #[test]
    fn the_owners_checkout_moves_only_on_a_clean_main() {
        let (w, _) = world("owner", true);
        let main = origin_tip(&w, "main");
        std::fs::write(w.owner.join("README.md"), "the owner's edit").unwrap();
        assert!(fast_forward_owner(&w.owner, "origin", "main").contains("tracked changes"));
        tgit(&w.owner, &["checkout", "--quiet", "--", "README.md"]);
        tgit(&w.owner, &["checkout", "--quiet", "-b", "topic"]);
        assert!(fast_forward_owner(&w.owner, "origin", "main").contains("on topic"));
        tgit(&w.owner, &["checkout", "--quiet", "main"]);
        assert_ne!(tgit(&w.owner, &["rev-parse", "HEAD"]).trim(), main);
        assert!(fast_forward_owner(&w.owner, "origin", "main").contains("fast-forwarded"));
        assert_eq!(tgit(&w.owner, &["rev-parse", "HEAD"]).trim(), main);
    }
}
