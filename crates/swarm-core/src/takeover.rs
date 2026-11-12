//! `cargo xtask claim <n> --take-over`: take over a stale claim and keep its branch and PR
//! (mornycorp/swarm-core#16). On 2026-11-03 a worker claimed an issue, opened its draft PR with a
//! commit and went idle. The coordinator handed the issue to another worker, whose `claim --force`
//! refused the branch with work on it and offered only `--slug`: a new branch and PR that drop the
//! work. The takeover was done by hand.
//!
//! - **When:** the claim is stale. The coordinator released it (`cargo xtask event release
//!   --issue <n> --of <instance>`, or a `released: <name>` comment), or its holders' last activity
//!   (their events on the issue, their claim comments, the branch's newest commit) is older than
//!   [`LIVE_SECS`], the protocol's 24-hour rule. A holder whose own `stop` follows its newest
//!   `claim` or `work` holds nothing. Otherwise it refuses, naming each live holder and when it
//!   was last active ([`grounds`]).
//! - **What it adopts:** the issue's open PR (the one the newest claim comment names, when there
//!   are several: [`pick_pr`]) and its branch, fetched; the local branch is made at its head
//!   ([`adopt`]), and the worktree switched to it as a claim's is: the commits are kept. A local
//!   branch with commits origin's lacks is refused ([`ahead`]), before anything is posted.
//! - **What it logs and posts** ([`plan`]): `release --of <old instance> --reason stale` for each
//!   holder the tracking branch doesn't show released; `released: <old name>, taken over by …` for
//!   each claim comment still standing; the label; `claimed: <name>, PR #<m>`; then `claim` and
//!   `work`.
//! - **A hold:** `status::blocked` comes off when its cause is gone: every issue the description's
//!   `Needs` line and the newest comment saying what it waits on name is closed ([`unblock`]).
//!   Otherwise it stays, and the takeover says why. A held issue is idle by design, so an idle
//!   claim on one whose cause may remain is taken over only on the coordinator's say-so
//!   ([`held_back`]).
//! - It runs no periodic full gate, and a red `main` doesn't hold it: the work goes on on the PR's
//!   branch, nothing new from `main`.
//!
//! The decisions are pure functions over the issue's facts (tested); the git and gh calls around
//! them are thin.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::Project;
use crate::claim::{self, ClaimNote, Ctx, LIVE_SECS, Me, Opened, Pr, Tracked};
use crate::needs::{self, Need, Ref};
use crate::process::git;
use crate::{event, github};

/// Someone who held the issue before the takeover: an instance on the tracking branch, a claim
/// comment's agent, or both.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Old {
    pub name: Option<String>,
    pub instance: Option<String>,
    /// Its last activity: its newest event on the issue, or its newest claim comment (seconds since
    /// the Unix epoch).
    pub last: Option<i64>,
    /// The tracking branch shows its newest `claim` or `work` ended: its own `release` or `stop`,
    /// another's `release --of` it, or a merge.
    pub logged: bool,
    /// A `released: <name>` comment follows its newest claim comment.
    pub noted: bool,
    /// Its newest claim comment stands: no `released: <name>` comment after it.
    pub standing: bool,
}

impl Old {
    /// Was it released, on the tracking branch or by a comment?
    pub fn released(&self) -> bool {
        self.logged || self.noted
    }

    /// `name (instance)`, or what is known of it.
    pub fn who(&self) -> String {
        match (&self.name, &self.instance) {
            (Some(n), Some(i)) => format!("{n} ({i})"),
            (Some(n), None) => n.clone(),
            (None, Some(i)) => i.clone(),
            (None, None) => "an agent".into(),
        }
    }
}

/// The instances that claimed or worked on `issue`, each with its agent name. The coordinator's
/// own events on it (a merge, a release `--of`) don't make it a holder.
fn instances_on(issue: u64, tracked: &[Tracked]) -> BTreeMap<String, Option<String>> {
    tracked.iter().filter(|e| e.issue == Some(issue) && (e.event == "claim" || e.event == "work")).map(|e| (e.instance.clone(), e.name.clone())).collect()
}

/// Does release `e` end `instance`'s work: its own (no `of`, or naming itself), or another's
/// naming it?
fn releases(e: &Tracked, instance: &str) -> bool {
    match e.of.as_deref() {
        None => e.instance == instance,
        Some(of) => of == instance,
    }
}

/// `instance`'s hold on `issue` as the tracking branch has it: its last activity on the issue, and
/// whether its newest `claim` or `work` was ended since: a merge, a release of it, or its own
/// `stop`, which [`claim::live_claims`] already reads as the issue free (mornycorp/swarm-core#34).
fn tracked_hold(issue: u64, tracked: &[Tracked], instance: &str, name: Option<String>) -> Old {
    let on: Vec<&Tracked> = tracked.iter().filter(|e| e.issue == Some(issue)).collect();
    let own = || on.iter().filter(|e| e.instance == instance);
    let hold = own().filter(|e| e.event == "claim" || e.event == "work").map(|e| e.t).max().unwrap_or(i64::MIN);
    let ends = |e: &Tracked| e.event == "merge" || (e.event == "release" && releases(e, instance)) || (e.event == "stop" && e.instance == instance);
    let logged = on.iter().any(|e| e.t >= hold && ends(e));
    Old { name, instance: Some(instance.to_owned()), last: own().map(|e| e.t).max(), logged, ..Old::default() }
}

/// The `released: <name>` comments: (name, when).
fn release_notes(notes: &[Value]) -> Vec<(String, String)> {
    notes
        .iter()
        .filter_map(|n| {
            Some((claim::parse_release_note(n.get("body")?.as_str()?)?, n.get("created_at").and_then(Value::as_str).unwrap_or_default().to_owned()))
        })
        .collect()
}

/// What the comments say of `old`'s claim: its newest claim comment is its last activity if it is
/// later than its events, and a release comment after it releases it.
fn note_hold(old: &mut Old, claims: &[ClaimNote], released: &[(String, String)]) {
    let Some(name) = old.name.as_deref() else { return };
    // ISO 8601 times in one zone sort as text.
    let Some(newest) = claims.iter().filter(|c| c.name == name).max_by(|a, b| a.at.cmp(&b.at)) else { return };
    old.noted = released.iter().any(|(n, at)| n == name && at > &newest.at);
    old.standing = !old.noted;
    old.last = old.last.max(claim::parse_time(&newest.at));
}

/// Everyone but `me` who held `issue`: each instance that claimed or worked on it, and each agent
/// whose claim comment names no such instance. Released ones are among them, marked.
pub fn old_holders(issue: u64, tracked: &[Tracked], notes: &[Value], me: &Me) -> Vec<Old> {
    let claims = claim::all_claims(notes);
    let released = release_notes(notes);
    let instances = instances_on(issue, tracked);
    let mut olds: Vec<Old> = instances.iter().filter(|(i, n)| !me.is(i, n.as_deref())).map(|(i, n)| tracked_hold(issue, tracked, i, n.clone())).collect();
    let named: BTreeSet<&str> = instances.values().flatten().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    for c in &claims {
        if me.name.as_deref() != Some(c.name.as_str()) && !named.contains(c.name.as_str()) && seen.insert(c.name.as_str()) {
            olds.push(Old { name: Some(c.name.clone()), ..Old::default() });
        }
    }
    for o in &mut olds {
        note_hold(o, &claims, &released);
    }
    olds
}

/// Has `me` (this instance, or an earlier session of its agent) claimed `issue`?
pub fn is_mine(issue: u64, tracked: &[Tracked], notes: &[Value], me: &Me) -> bool {
    instances_on(issue, tracked).iter().any(|(i, n)| me.is(i, n.as_deref()))
        || claim::all_claims(notes).iter().any(|c| me.name.as_deref() == Some(c.name.as_str()))
}

/// The PR a takeover of `issue` adopts, from `mrs` (the open PRs): the only open one for the
/// issue, or the one the newest claim comment (`newest`) names.
pub fn pick_pr(issue: u64, mrs: &[Pr], newest: Option<&ClaimNote>) -> Result<Pr, String> {
    let open: Vec<&Pr> = mrs.iter().filter(|m| m.state == "open" && m.is_for(issue)).collect();
    if let [only] = open.as_slice() {
        return Ok((*only).clone());
    }
    if let Some(m) = newest.and_then(|n| n.mr).and_then(|m| open.iter().find(|p| p.number == m)) {
        return Ok((*m).clone());
    }
    if open.is_empty() {
        return Err(format!("claim: #{issue} has no open PR to take over: `cargo xtask claim {issue}` makes a new branch and PR"));
    }
    let list: String = open.iter().map(|m| format!("\n  - #{} (branch {})", m.number, m.head_ref)).collect();
    Err(format!("claim: #{issue} has {} open PRs and no claim comment names one:{list}\nClose the ones not to keep, then take over the other", open.len()))
}

/// Why a takeover may go ahead.
#[derive(Debug, Clone, PartialEq)]
pub enum Grounds {
    /// Every earlier holder was released: the coordinator's say-so (or its own).
    Released,
    /// Nobody else held it: it is my own claim, from an earlier session.
    Mine,
    /// Nothing has happened on it since `since` (seconds since the Unix epoch), more than
    /// [`LIVE_SECS`] ago.
    Idle { since: i64 },
}

impl Grounds {
    /// Why, in a few words, at `now`.
    pub fn describe(&self, now: i64) -> String {
        match self {
            Grounds::Released => "its claim was released".into(),
            Grounds::Mine => "it is your own claim".into(),
            Grounds::Idle { since } => format!("idle since {}", ago(*since, now)),
        }
    }
}

/// `2026-11-03T08:43:00Z (26 h ago)`.
fn ago(t: i64, now: i64) -> String {
    let mins = now.saturating_sub(t) / 60;
    let ago = if mins < 120 { format!("{mins} min ago") } else { format!("{} h ago", mins / 60) };
    format!("{} ({ago})", crate::time::iso_utc(u64::try_from(t).unwrap_or(0)))
}

/// Whether a takeover of `issue` may go ahead at `now`, and why: every earlier holder (`olds`)
/// released; none, and the claim is `mine`; or no activity from the unreleased ones nor on the
/// branch (its newest commit `tip_at`) for more than [`LIVE_SECS`]. The error says who is live.
pub fn grounds(issue: u64, olds: &[Old], mine: bool, tip_at: Option<i64>, now: i64) -> Result<Grounds, String> {
    let live: Vec<&Old> = olds.iter().filter(|o| !o.released()).collect();
    if live.is_empty() && !olds.is_empty() {
        return Ok(Grounds::Released);
    }
    if live.is_empty() && mine {
        return Ok(Grounds::Mine);
    }
    match live.iter().filter_map(|o| o.last).chain(tip_at).max() {
        Some(t) if now.saturating_sub(t) > LIVE_SECS => Ok(Grounds::Idle { since: t }),
        _ => Err(not_stale(issue, &live, tip_at, now)),
    }
}

/// The refusal of a takeover of a claim that isn't stale: who is live, since when, and what would
/// make it stale.
fn not_stale(issue: u64, live: &[&Old], tip_at: Option<i64>, now: i64) -> String {
    let mut lines: Vec<String> = live
        .iter()
        .map(|o| match o.last {
            Some(t) => format!("{} was last active at {}, and nothing released it", o.who(), ago(t, now)),
            None => format!("{}: nothing released it", o.who()),
        })
        .collect();
    if let Some(t) = tip_at {
        lines.push(format!("the branch's newest commit is from {}", ago(t, now)));
    }
    let list: String = lines.iter().map(|l| format!("\n  - {l}")).collect();
    let of = live.iter().find_map(|o| o.instance.as_deref()).unwrap_or("<its instance>");
    format!(
        "claim: #{issue}'s claim isn't stale, so it can't be taken over:{list}
A takeover needs the coordinator's say-so (`cargo xtask event release --issue {issue} --of {of} --reason stale`) or 24 hours with no activity (docs/swarm-protocol.md › Picking work, step 3)."
    )
}

/// The issues `issue`'s hold waits on: those its description's `Needs` line names, and those the
/// newest comment saying what it waits on names (`Blocked on owner/repo#194 …`).
pub fn hold_cause(project: &Project, issue: u64, description: &str, notes: &[Value]) -> BTreeSet<Ref> {
    let newest = notes
        .iter()
        .filter_map(|n| {
            let refs = needs::parse_hold_for(project, n.get("body")?.as_str()?);
            (!refs.is_empty()).then(|| (n.get("created_at").and_then(Value::as_str).unwrap_or_default(), refs))
        })
        // ISO 8601 times in one zone sort as text.
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, refs)| refs)
        .unwrap_or_default();
    needs::parse_for(project, description).into_iter().chain(newest).filter(|r| *r != Ref::here(issue)).collect()
}

/// Whether a hold whose cause is `cause` can come off, as `found` (each issue's state) says: when
/// every issue in it is closed, those issues; else why it stays.
pub fn unblock(cause: &BTreeSet<Ref>, found: &BTreeMap<Ref, Need>) -> Result<String, String> {
    if cause.is_empty() {
        return Err("no comment or Needs line names an issue it waits on, so the takeover can't tell whether its cause is gone".into());
    }
    let pending: Vec<String> = cause
        .iter()
        .filter_map(|r| match found.get(r) {
            Some(n) if n.state == "closed" => None,
            Some(n) => Some(format!("{} is {}", r.label(), n.state)),
            None => Some(format!("{} couldn't be read", r.label())),
        })
        .collect();
    if pending.is_empty() {
        Ok(cause.iter().map(Ref::label).collect::<Vec<_>>().join(", "))
    } else {
        Err(format!("its cause may not be gone: {}", pending.join("; ")))
    }
}

/// Whether a takeover on `grounds` must wait for a hold on the issue, whose [`unblock`] decision is
/// `hold` (`None`: not on hold): an idle claim's hold that may still have its cause holds the
/// takeover too, since a held issue is idle by design; the coordinator's say-so, or my own claim,
/// goes ahead and leaves the label on.
pub fn held_back(issue: u64, grounds: &Grounds, hold: Option<&Result<String, String>>) -> Option<String> {
    match (grounds, hold) {
        (Grounds::Idle { .. }, Some(Err(why))) => Some(format!(
            "claim: #{issue} is on hold, and {why}: an idle claim on a held issue is taken over only on the coordinator's say-so (`cargo xtask event release --issue {issue} --of <its instance> --reason stale`)"
        )),
        _ => None,
    }
}

/// Is `branch` (a PR's head, from GitHub) a plain branch name to hand git: letters, digits, `-`,
/// `_`, `.`, `/`, not starting with `-`?
pub fn check_branch(branch: &str) -> Result<(), String> {
    let ok = !branch.is_empty()
        && branch.len() <= 200
        && !branch.starts_with(['-', '/', '.'])
        && !branch.ends_with(['/', '.'])
        && !branch.contains("..")
        && !branch.ends_with(".lock")
        && branch.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if ok { Ok(()) } else { Err(format!("claim: the PR's branch `{branch}` isn't a name to take over (letters, digits, `-`, `_`, `.`, `/`)")) }
}

/// One step of a takeover once its checks have passed, in [`plan`]'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    /// The PR's branch fetched, and the local branch made at its head.
    Adopt,
    /// `release --of <instance> --reason stale` for each earlier holder not released.
    Release,
    /// `released: <name>, taken over by …` for each claim comment still standing.
    Released,
    /// The claim label.
    Label,
    /// `claimed: <name>, PR #<m>`.
    Comment,
    /// The `claim` and `work` events.
    Events,
    /// `status::blocked` off when its cause is gone, or a note why it stays.
    Unblock,
    /// Say what was taken over, and switch the worktree to its branch.
    Report,
}

/// A takeover's steps: the branch first (nothing is posted for a branch that can't be had), the
/// old claims released before the new one is made, so the timeline reads in order, and the
/// report and switch last. No periodic full gate: the work goes on on the PR's branch.
pub fn plan() -> [Take; 8] {
    [Take::Adopt, Take::Release, Take::Released, Take::Label, Take::Comment, Take::Events, Take::Unblock, Take::Report]
}

/// Where [`adopt`] left the local branch.
#[derive(Debug, Clone, PartialEq)]
pub enum Adopted {
    /// Made, at origin's head.
    Made,
    /// It was at origin's head already.
    Same,
    /// It was behind, and moved up to origin's head.
    Moved,
    /// It was behind, and couldn't be moved (checked out in a worktree): why.
    Behind(String),
    /// It has commits origin's head doesn't: the take-over is refused (mornycorp/swarm-core#34).
    Ahead,
}

/// Fetch `branch` from origin and make the local branch at its head, its commits kept: made when
/// missing, moved up when behind, [`Adopted::Ahead`] when it has commits of its own.
pub fn adopt(root: &Path, branch: &str) -> Result<Adopted, String> {
    let remote = format!("refs/remotes/origin/{branch}");
    event::fetch(root, "origin", &format!("refs/heads/{branch}"), &remote)?;
    let tip = git(root, &["rev-parse", "--verify", "--quiet", &format!("{remote}^{{commit}}")], None, &[])?.trim().to_owned();
    let Ok(have) = git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")], None, &[]) else {
        git(root, &["branch", "--quiet", "--track", branch, &format!("origin/{branch}")], None, &[])?;
        return Ok(Adopted::Made);
    };
    let have = have.trim();
    if have == tip {
        return Ok(Adopted::Same);
    }
    if git(root, &["merge-base", "--is-ancestor", have, &tip], None, &[]).is_err() {
        return Ok(Adopted::Ahead);
    }
    Ok(match git(root, &["branch", "--quiet", "--force", branch, &tip], None, &[]) {
        Ok(_) => Adopted::Moved,
        Err(e) => Adopted::Behind(e.lines().next().unwrap_or_default().to_owned()),
    })
}

/// The refusal of a take-over whose local `branch` has commits origin's doesn't: pushing it would
/// push the old holder's unpushed work as the new holder's, and resetting it would lose that work.
pub fn ahead(branch: &str) -> String {
    format!(
        "claim: branch {branch} here has commits origin's doesn't (the old holder's unpushed work?): taking it over would push them, or lose them. Push or drop them first (`git log origin/{branch}..{branch}`), then take over again"
    )
}

/// When commit `tip` was committed (seconds since the Unix epoch), if git says.
pub fn commit_time(root: &Path, tip: &str) -> Option<i64> {
    git(root, &["log", "-1", "--format=%ct", tip], None, &[]).ok()?.trim().parse().ok()
}

/// `cargo xtask claim <n> --take-over`, once [`claim`]'s first checks (`o`) have passed.
pub(crate) fn run(ctx: &Ctx<'_>, o: Opened, no_switch: bool) -> Result<(), String> {
    let t = checked(ctx, o, no_switch)?;
    for step in plan() {
        t.take(step)?;
    }
    Ok(())
}

/// A takeover that passed its checks.
struct Takeover<'a> {
    ctx: Ctx<'a>,
    o: Opened,
    no_switch: bool,
    pr: Pr,
    olds: Vec<Old>,
    why: String,
    /// On hold: whether it can come off ([`unblock`]).
    hold: Option<Result<String, String>>,
}

/// The takeover's checks: a PR to adopt, its branch on origin, and the claim stale ([`grounds`]).
fn checked<'a>(ctx: &Ctx<'a>, o: Opened, no_switch: bool) -> Result<Takeover<'a>, String> {
    let issue = o.issue;
    let me = Me { instance: Some(o.agent.clone()), name: Some(o.name.clone()), branch: None };
    let olds = old_holders(issue, &o.facts.tracked, &o.notes, &me);
    let pr = pick_pr(issue, &o.facts.mrs, o.facts.claim_note.as_ref())?;
    check_branch(&pr.head_ref)?;
    let tip = event::fetch_tip(ctx.root, "origin", &format!("refs/heads/{}", pr.head_ref))?
        .ok_or_else(|| format!("claim: PR #{}'s branch {} isn't on origin: nothing to take over", pr.number, pr.head_ref))?;
    let now = claim::now();
    let mine = is_mine(issue, &o.facts.tracked, &o.notes, &me);
    let grounds = grounds(issue, &olds, mine, commit_time(ctx.root, &tip), now)?;
    let hold = hold(ctx.project, &o);
    if let Some(refused) = held_back(issue, &grounds, hold.as_ref()) {
        return Err(refused);
    }
    let why = grounds.describe(now);
    println!("claim: taking over #{issue}, PR #{} (branch {}): {why}", pr.number, pr.head_ref);
    Ok(Takeover { ctx: *ctx, o, no_switch, pr, olds, why, hold })
}

/// When the issue `o` is on hold, whether the hold can come off: what it waits on, read now.
fn hold(project: &Project, o: &Opened) -> Option<Result<String, String>> {
    if !o.facts.labels.iter().any(|l| l == project.labels.blocked) {
        return None;
    }
    let cause = hold_cause(project, o.issue, &o.facts.description, &o.notes);
    let wanted: BTreeMap<Ref, BTreeSet<u64>> = cause.iter().map(|r| (r.clone(), BTreeSet::from([o.issue]))).collect();
    let open: Vec<Pr> = o.facts.mrs.iter().filter(|m| m.state == "open").cloned().collect();
    let found = if wanted.is_empty() { BTreeMap::new() } else { needs::fetch(project, &wanted, &open, &needs::GitHub(project)) };
    Some(unblock(&cause, &found))
}

impl Takeover<'_> {
    /// One of [`plan`]'s steps. The label and the claim comment are kept when they are there.
    fn take(&self, step: Take) -> Result<(), String> {
        let (project, o) = (self.ctx.project, &self.o);
        match step {
            Take::Adopt => self.adopt(),
            Take::Release => self.release(),
            Take::Released => self.released(),
            Take::Label if o.facts.labels.iter().any(|l| l == project.labels.in_progress) => Ok(()),
            Take::Label => claim::edit_label(project, o.issue, true),
            Take::Comment if o.facts.claim_note.as_ref().is_some_and(|n| n.name == o.name && n.mr == Some(self.pr.number)) => Ok(()),
            Take::Comment => claim::post_note(project, o.issue, &claim::claim_comment(&o.name, Some(self.pr.number))),
            Take::Events => claim::claim_events(&o.agent, o.issue, Some(self.pr.number))
                .iter()
                .try_for_each(|ev| event::run(project, self.ctx.root, &ev.iter().map(String::as_str).collect::<Vec<_>>())),
            Take::Unblock => {
                self.unblock();
                Ok(())
            }
            Take::Report => {
                self.report();
                Ok(())
            }
        }
    }

    fn adopt(&self) -> Result<(), String> {
        let branch = &self.pr.head_ref;
        match adopt(self.ctx.root, branch)? {
            Adopted::Made => println!("claim: made branch {branch} at origin's head"),
            Adopted::Same => println!("claim: branch {branch} is at origin's head"),
            Adopted::Moved => println!("claim: moved branch {branch} up to origin's head"),
            Adopted::Behind(e) => {
                eprintln!("warning: claim: branch {branch} here is behind origin's and couldn't be moved ({e}): `git merge --ff-only origin/{branch}` on it")
            }
            Adopted::Ahead => return Err(ahead(branch)),
        }
        Ok(())
    }

    /// Log the release of each earlier holder the tracking branch doesn't show released.
    fn release(&self) -> Result<(), String> {
        let (project, o) = (self.ctx.project, &self.o);
        let (issue, note) = (o.issue.to_string(), format!("taken over by {}: {}", o.name, self.why));
        let unreleased: BTreeSet<&str> = self.olds.iter().filter(|h| !h.logged).filter_map(|h| h.instance.as_deref()).collect();
        for of in unreleased {
            event::run(project, self.ctx.root, &["release", "--agent", &o.agent, "--issue", &issue, "--of", of, "--reason", "stale", "--note", &note])?;
        }
        Ok(())
    }

    /// Comment the release of each earlier holder whose claim comment still stands.
    fn released(&self) -> Result<(), String> {
        let (project, o) = (self.ctx.project, &self.o);
        let why = format!("taken over by {}, PR #{} and its branch kept ({})", o.name, self.pr.number, self.why);
        let standing: BTreeSet<&str> = self.olds.iter().filter(|h| h.standing).filter_map(|h| h.name.as_deref()).collect();
        standing.into_iter().try_for_each(|name| claim::post_note(project, o.issue, &claim::release_comment(name, &why)))
    }

    /// Take `status::blocked` off when its cause is gone; else say why it stays.
    fn unblock(&self) {
        let (project, o) = (self.ctx.project, &self.o);
        let blocked = project.labels.blocked;
        match &self.hold {
            None => {}
            Some(Ok(closed)) => match github::remove_label(project, None, o.issue, blocked) {
                Ok(()) => println!("claim: took {blocked} off #{}: its cause is gone ({closed} closed)", o.issue),
                Err(e) => eprintln!("warning: claim: taking {blocked} off #{} ({closed} closed): {e}", o.issue),
            },
            Some(Err(why)) => println!("claim: left {blocked} on #{}: {why}; whoever put it on takes it off", o.issue),
        }
    }

    /// Say what was taken over, from whom, and switch the worktree (not with `--no-switch`).
    /// Agents parse the first line, a claim's: keep its wording.
    fn report(&self) {
        let (project, o, pr) = (self.ctx.project, &self.o, &self.pr);
        let draft = if pr.draft { "draft " } else { "" };
        println!("claimed #{} as {}: branch {}, {draft}PR #{} {}", o.issue, o.name, pr.head_ref, pr.number, pr.html_url);
        let from: Vec<String> = self.olds.iter().map(Old::who).collect();
        let from = if from.is_empty() { "nobody's claim".to_owned() } else { from.join(", ") };
        println!("took over from {from}: the branch's commits are kept ({})", self.why);
        claim::report_switch(project, self.ctx.root, &pr.head_ref, (!self.no_switch).then_some(o.agent.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claim::{latest_claim, parse_time};
    use crate::project::fixtures::{APP_SWARM as P, BOTH};
    use serde_json::json;

    fn t(s: &str) -> i64 {
        parse_time(s).unwrap()
    }

    fn note(body: &str, at: &str) -> Value {
        json!({ "body": body, "created_at": at })
    }

    /// An agent's tracking file: its session, then `(event, mr, of, time)` on `issue`.
    fn agent(instance: &str, name: &str, issue: u64, events: &[(&str, Option<u64>, Option<&str>, &str)]) -> (String, String) {
        let mut lines = vec![format!(r#"{{"event":"session.start","name":"{name}","role":"worker","t":"2026-11-03T07:00:00Z"}}"#)];
        for (event, mr, of, at) in events {
            let mr = mr.map(|m| format!(r#","mr":{m}"#)).unwrap_or_default();
            let of = of.map(|o| format!(r#","of":"{o}""#)).unwrap_or_default();
            lines.push(format!(r#"{{"event":"{event}","issue":{issue}{mr}{of},"t":"{at}"}}"#));
        }
        (instance.to_owned(), lines.join("\n"))
    }

    fn pr(number: u64, branch: &str, title: &str) -> Pr {
        Pr {
            number,
            state: "open".into(),
            draft: true,
            head_ref: branch.into(),
            title: title.into(),
            body: String::new(),
            html_url: format!("https://github.com/example/app-swarm/pull/{number}"),
        }
    }

    fn worker_43() -> Me {
        Me { instance: Some("mini-43".into()), name: Some("worker-43".into()), branch: None }
    }

    /// swarmception#92 on 2026-11-03: worker-38 claimed it (PR #93), committed, said it was
    /// blocked, and went quiet at 08:43, its last event `last` (`work`: nothing ended its hold;
    /// `stop`: it stopped); the coordinator says so in a comment at 09:50.
    fn issue_92_with(coordinator_released: bool, last: &str) -> (Vec<Tracked>, Vec<Value>) {
        // The coordinator's own events on the issue: an earlier claim's release.
        let mut coordinator = vec![("release", None, Some("mini-11"), "2026-11-03T08:00:00Z")];
        if coordinator_released {
            coordinator.push(("release", None, Some("mini-38"), "2026-11-03T09:51:00Z"));
        }
        let tracked = claim::parse_tracking(&[
            agent(
                "mini-38",
                "worker-38",
                92,
                &[("claim", Some(93), None, "2026-11-03T08:30:00Z"), ("work", None, None, "2026-11-03T08:30:05Z"), (last, None, None, "2026-11-03T08:43:00Z")],
            ),
            agent("desk-coord", "coordinator", 92, &coordinator),
        ]);
        let notes = vec![
            note("claimed: worker-38, PR #93", "2026-11-03T08:30:10Z"),
            note(
                "Blocked on mornycorp/swarm-timeline#194 (the fix is the widget's). The test is on #93 and fails until the pin moves. Once #194 merges, bump it.",
                "2026-11-03T08:42:00Z",
            ),
            note("Coordinator: swarm-timeline#194 merged; PR #93 has been idle since 08:43, so worker-43 takes it over.", "2026-11-03T09:50:00Z"),
        ];
        (tracked, notes)
    }

    /// [`issue_92_with`] where worker-38 went quiet with nothing logged that ends its hold.
    fn issue_92(coordinator_released: bool) -> (Vec<Tracked>, Vec<Value>) {
        issue_92_with(coordinator_released, "work")
    }

    /// mornycorp/swarm-core#34: a holder's own `stop` on the issue ends its hold, as
    /// `claim::live_claims` reads it: no 24-hour wait, and no `release --of` to log for it.
    #[test]
    fn a_holder_that_logged_stop_holds_nothing() {
        let (tracked, notes) = issue_92_with(false, "stop");
        let olds = old_holders(92, &tracked, &notes, &worker_43());
        assert!(olds[0].logged && olds[0].released(), "{olds:?}");
        let now = t("2026-11-03T10:00:00Z");
        assert_eq!(grounds(92, &olds, false, Some(t("2026-11-03T08:41:00Z")), now), Ok(Grounds::Released));
        assert!(claim::live_claims(&tracked, 92, now).iter().all(|c| c.instance != "mini-38"), "the claim side agrees");
        // A stop before a later `work` doesn't end that work.
        let stopped_early = claim::parse_tracking(&[agent(
            "mini-38",
            "worker-38",
            92,
            &[("claim", Some(93), None, "2026-11-03T08:30:00Z"), ("stop", None, None, "2026-11-03T08:31:00Z"), ("work", None, None, "2026-11-03T08:43:00Z")],
        )]);
        let olds = old_holders(92, &stopped_early, &notes, &worker_43());
        assert!(!olds[0].released(), "{olds:?}");
    }

    /// mornycorp/swarm-core#34: a local branch with commits origin's lacks is refused, before
    /// anything is posted (the branch is the plan's first step), saying how to go on.
    #[test]
    fn a_local_branch_ahead_of_origins_refuses_the_take_over() {
        assert_eq!(plan()[0], Take::Adopt, "nothing is posted before the branch is had");
        let why = ahead("92-a11y-test");
        assert!(why.contains("has commits origin's doesn't") && why.contains("`git log origin/92-a11y-test..92-a11y-test`"), "{why}");
    }

    #[test]
    fn a_live_claim_is_refused_naming_its_holder_and_the_way_to_release_it() {
        let (tracked, notes) = issue_92(false);
        let olds = old_holders(92, &tracked, &notes, &worker_43());
        assert_eq!(olds.len(), 1, "the coordinator's release doesn't make it a holder: {olds:?}");
        assert_eq!(olds[0].who(), "worker-38 (mini-38)");
        assert_eq!(olds[0].last, Some(t("2026-11-03T08:43:00Z")), "its last event");
        assert!(!olds[0].released() && olds[0].standing);
        let now = t("2026-11-03T10:00:00Z");
        let refused = grounds(92, &olds, false, Some(t("2026-11-03T08:41:00Z")), now).unwrap_err();
        assert!(refused.contains("worker-38 (mini-38) was last active at 2026-11-03T08:43:00Z (77 min ago)"), "{refused}");
        assert!(refused.contains("newest commit is from 2026-11-03T08:41:00Z"), "{refused}");
        assert!(refused.contains("`cargo xtask event release --issue 92 --of mini-38 --reason stale`") && refused.contains("24 hours"), "{refused}");
    }

    #[test]
    fn the_coordinators_release_makes_a_claim_stale_at_once() {
        let (tracked, notes) = issue_92(true);
        let olds = old_holders(92, &tracked, &notes, &worker_43());
        assert!(olds[0].logged && olds[0].released(), "{olds:?}");
        assert!(olds[0].standing, "its claim comment still stands: the takeover comments its release");
        let now = t("2026-11-03T10:00:00Z");
        assert_eq!(grounds(92, &olds, false, Some(t("2026-11-03T09:59:00Z")), now), Ok(Grounds::Released), "even with a fresh commit");
        // A release comment by hand says the same.
        let (tracked, mut notes) = issue_92(false);
        notes.push(note("released: worker-38, taken over by worker-43 on the coordinator's word", "2026-11-03T09:55:00Z"));
        let olds = old_holders(92, &tracked, &notes, &worker_43());
        assert!(olds[0].noted && !olds[0].standing && !olds[0].logged, "{olds:?}");
        assert_eq!(grounds(92, &olds, false, None, now), Ok(Grounds::Released));
    }

    #[test]
    fn a_claim_idle_past_the_threshold_is_stale() {
        let (tracked, notes) = issue_92(false);
        let olds = old_holders(92, &tracked, &notes, &worker_43());
        let last = t("2026-11-03T08:43:00Z");
        let next_day = last + LIVE_SECS + 60;
        assert_eq!(grounds(92, &olds, false, Some(t("2026-11-03T08:41:00Z")), next_day), Ok(Grounds::Idle { since: last }));
        assert_eq!(Grounds::Idle { since: last }.describe(next_day), "idle since 2026-11-03T08:43:00Z (24 h ago)");
        // Exactly 24 hours is still live; a commit since is activity.
        assert!(grounds(92, &olds, false, None, last + LIVE_SECS).is_err());
        assert!(grounds(92, &olds, false, Some(next_day - 3600), next_day).is_err(), "a commit an hour ago");
    }

    #[test]
    fn my_own_claim_is_mine_to_resume_and_an_unclaimed_pr_needs_its_branch_idle() {
        let (tracked, notes) = issue_92(false);
        // worker-38 itself, in a new session: nobody else held it.
        let again = Me { instance: Some("mini-38b".into()), name: Some("worker-38".into()), branch: None };
        let olds = old_holders(92, &tracked, &notes, &again);
        assert!(olds.is_empty(), "{olds:?}");
        assert!(is_mine(92, &tracked, &notes, &again) && !is_mine(92, &tracked, &notes, &worker_43()));
        let now = t("2026-11-03T10:00:00Z");
        assert_eq!(grounds(92, &olds, true, Some(now - 60), now), Ok(Grounds::Mine));
        // No claim names the PR at all: only its branch's age says it is abandoned.
        assert!(grounds(92, &[], false, Some(now - 60), now).unwrap_err().contains("newest commit"));
        assert_eq!(grounds(92, &[], false, Some(now - LIVE_SECS - 1), now), Ok(Grounds::Idle { since: now - LIVE_SECS - 1 }));
        assert!(grounds(92, &[], false, None, now).is_err(), "nothing known: refused");
    }

    /// A claim comment with no tracking instance behind it is a holder too, by its name; and of
    /// two holders, both must be stale.
    #[test]
    fn every_holder_counts_whether_it_logged_or_only_commented() {
        let (tracked, mut notes) = issue_92(true);
        notes.push(note("claimed: rowan, PR #93", "2026-11-03T09:58:00Z"));
        let olds = old_holders(92, &tracked, &notes, &worker_43());
        let rowan = olds.iter().find(|o| o.name.as_deref() == Some("rowan")).unwrap();
        assert_eq!((rowan.instance.as_deref(), rowan.last, rowan.standing), (None, Some(t("2026-11-03T09:58:00Z")), true));
        let now = t("2026-11-03T10:00:00Z");
        let refused = grounds(92, &olds, false, None, now).unwrap_err();
        assert!(refused.contains("rowan was last active") && !refused.contains("worker-38"), "worker-38 was released: {refused}");
        assert!(refused.contains("--of <its instance>"), "{refused}");
        // My own comment isn't another's claim.
        notes.push(note("claimed: worker-43, PR #93", "2026-11-03T09:59:00Z"));
        assert!(old_holders(92, &tracked, &notes, &worker_43()).iter().all(|o| o.name.as_deref() != Some("worker-43")));
    }

    #[test]
    fn the_pr_to_adopt_is_the_only_one_or_the_one_the_claim_names() {
        let mrs = [pr(93, "92-a11y-test", "A11y test (#92)"), pr(94, "77-other", "Other (#77)")];
        assert_eq!(pick_pr(92, &mrs, None).map(|m| m.number), Ok(93));
        let none = pick_pr(92, &mrs[1..], None).unwrap_err();
        assert!(none.contains("no open PR to take over") && none.contains("cargo xtask claim 92"), "{none}");
        let two = [pr(93, "92-a11y-test", "A11y test (#92)"), pr(95, "92-again", "Again (#92)")];
        let refused = pick_pr(92, &two, None).unwrap_err();
        assert!(refused.contains("2 open PRs") && refused.contains("#95 (branch 92-again)"), "{refused}");
        let named = latest_claim(&[note("claimed: worker-38, PR #95", "2026-11-03T08:30:10Z")]);
        assert_eq!(pick_pr(92, &two, named.as_ref()).map(|m| m.head_ref), Ok("92-again".into()));
        let closed = Pr { state: "closed".into(), ..pr(96, "92-old", "Old (#92)") };
        assert_eq!(pick_pr(92, &[closed, mrs[0].clone()], None).map(|m| m.number), Ok(93), "a closed PR isn't adopted");
    }

    #[test]
    fn a_hold_comes_off_only_when_every_issue_it_waits_on_is_closed() {
        for project in BOTH {
            let (_, notes) = issue_92(false);
            let own = format!("Needs {}#5.", project.github.repo);
            let cause = hold_cause(&project, 92, &own, &notes);
            let timeline = Ref { project: Some("mornycorp/swarm-timeline".into()), iid: 194 };
            assert_eq!(cause, BTreeSet::from([Ref::here(5), timeline.clone()]), "{}: the first clause only, not #93 or #194 later", project.name);
            let need = |r: &Ref, state: &str| Need {
                project: r.project.clone(),
                iid: r.iid,
                title: String::new(),
                state: state.into(),
                by: BTreeSet::new(),
                mr: None,
            };
            let found = |a: &str, b: &str| BTreeMap::from([(Ref::here(5), need(&Ref::here(5), a)), (timeline.clone(), need(&timeline, b))]);
            assert_eq!(unblock(&cause, &found("closed", "closed")), Ok("#5, mornycorp/swarm-timeline#194".into()));
            assert_eq!(unblock(&cause, &found("closed", "open")), Err("its cause may not be gone: mornycorp/swarm-timeline#194 is open".into()));
            let unread = BTreeMap::from([(Ref::here(5), need(&Ref::here(5), "closed"))]);
            assert!(unblock(&cause, &unread).unwrap_err().contains("mornycorp/swarm-timeline#194 couldn't be read"));
            // A hold that names nothing (the owner's) stays.
            let owners = [note("On hold (the owner): start nothing new.", "2026-11-03T08:00:00Z")];
            let none = hold_cause(&project, 92, "", &owners);
            assert!(none.is_empty() && unblock(&none, &BTreeMap::new()).unwrap_err().contains("can't tell"));
            // The issue itself is never its own cause.
            assert!(hold_cause(&project, 5, &own, &[]).is_empty());
        }
    }

    /// A held issue is idle by design: an idle claim on it is taken over only once its cause is
    /// gone, or on the coordinator's say-so, which leaves the label on.
    #[test]
    fn an_idle_claim_on_a_held_issue_waits_for_the_coordinator() {
        let idle = Grounds::Idle { since: t("2026-11-01T08:00:00Z") };
        let stays: Result<String, String> = Err("its cause may not be gone: #5 is open".into());
        let refused = held_back(92, &idle, Some(&stays)).unwrap();
        assert!(refused.contains("#5 is open") && refused.contains("--of <its instance>"), "{refused}");
        assert_eq!(held_back(92, &idle, Some(&Ok("#5".into()))), None, "its cause is gone");
        assert_eq!(held_back(92, &idle, None), None, "not on hold");
        assert_eq!(held_back(92, &Grounds::Released, Some(&stays)), None);
        assert_eq!(held_back(92, &Grounds::Mine, Some(&stays)), None);
    }

    #[test]
    fn the_old_claim_is_released_before_the_new_one_is_made() {
        let steps = plan();
        let at = |s: Take| steps.iter().position(|x| *x == s).unwrap();
        assert_eq!(steps[0], Take::Adopt, "nothing is posted for a branch that can't be had");
        assert!(at(Take::Release) < at(Take::Events) && at(Take::Released) < at(Take::Comment));
        assert_eq!(steps.last(), Some(&Take::Report));
    }

    #[test]
    fn only_plain_branch_names_are_handed_to_git() {
        for ok in ["92-a11y-test", "feature/92_x.y"] {
            assert_eq!(check_branch(ok), Ok(()), "{ok}");
        }
        for bad in ["", "--upload-pack=x", "a..b", "a b", "x.lock", "/x", "x/", "a;b", &"x".repeat(201)] {
            assert!(check_branch(bad).is_err(), "{bad}");
        }
    }

    /// Through git with a bare remote: another clone's branch with commits on it is adopted in a
    /// linked worktree, which switches to it with the commits there; then kept in step as origin
    /// moves on, and a local commit of its own is never thrown away.
    #[test]
    fn a_branch_with_commits_is_adopted_and_kept() {
        use swarm_core_testkit::git::{git as tgit, git_at};
        let dir = swarm_core_testkit::temp_dir("takeover-adopt");
        let remote = swarm_core_testkit::git::remote(&dir);
        // Another machine's worker: its branch, with two commits, pushed.
        let theirs = dir.join("theirs");
        tgit(&dir, &["clone", "--quiet", remote.origin.to_str().unwrap(), theirs.to_str().unwrap()]);
        tgit(&theirs, &["switch", "--quiet", "-c", "92-a11y-test"]);
        for (file, date) in [("a.txt", "2026-11-03T08:35:00Z"), ("b.txt", "2026-11-03T08:41:00Z")] {
            std::fs::write(theirs.join(file), "work\n").unwrap();
            tgit(&theirs, &["add", file]);
            git_at(&theirs, &["commit", "--quiet", "-m", file], Some(date));
        }
        tgit(&theirs, &["push", "--quiet", "origin", "92-a11y-test"]);
        let theirs_tip = tgit(&theirs, &["rev-parse", "HEAD"]).trim().to_owned();

        // Mine: a linked worktree of the clone, as a worker's is.
        let mine = dir.join("mine");
        tgit(&remote.work, &["worktree", "add", "--quiet", "--detach", mine.to_str().unwrap()]);
        let tip = event::fetch_tip(&mine, "origin", "refs/heads/92-a11y-test").unwrap().unwrap();
        assert_eq!(tip, theirs_tip);
        assert_eq!(commit_time(&mine, &tip), Some(t("2026-11-03T08:41:00Z")), "the idle clock reads the newest commit");
        assert_eq!(adopt(&mine, "92-a11y-test"), Ok(Adopted::Made));
        assert_eq!(tgit(&mine, &["rev-parse", "refs/heads/92-a11y-test"]).trim(), theirs_tip);
        assert_eq!(adopt(&mine, "92-a11y-test"), Ok(Adopted::Same), "a second run");
        assert_eq!(claim::switch_to(&P, &mine, "92-a11y-test", false, "mini-43"), Ok(claim::Switch::Switched));
        assert!(mine.join("a.txt").exists() && mine.join("b.txt").exists(), "the commits are kept");

        // Origin moves on while the branch is checked out here: it can't be moved, and says so.
        std::fs::write(theirs.join("c.txt"), "more\n").unwrap();
        tgit(&theirs, &["add", "c.txt"]);
        tgit(&theirs, &["commit", "--quiet", "-m", "c"]);
        tgit(&theirs, &["push", "--quiet", "origin", "92-a11y-test"]);
        assert!(matches!(adopt(&mine, "92-a11y-test"), Ok(Adopted::Behind(_))));
        // Not checked out anywhere: moved up.
        tgit(&mine, &["switch", "--quiet", "--detach"]);
        assert_eq!(adopt(&mine, "92-a11y-test"), Ok(Adopted::Moved));
        // A local commit origin hasn't: kept.
        tgit(&mine, &["switch", "--quiet", "92-a11y-test"]);
        std::fs::write(mine.join("d.txt"), "mine\n").unwrap();
        tgit(&mine, &["add", "d.txt"]);
        tgit(&mine, &["commit", "--quiet", "-m", "d"]);
        let local = tgit(&mine, &["rev-parse", "HEAD"]);
        assert_eq!(adopt(&mine, "92-a11y-test"), Ok(Adopted::Ahead));
        assert_eq!(tgit(&mine, &["rev-parse", "HEAD"]), local);
        // A branch origin doesn't have is an error, not a panic.
        assert!(adopt(&mine, "92-missing").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
