//! `cargo xtask claim`: atomic claims for the swarm. An
//! agent claims an issue in one step that checks, at that moment, whether anyone else holds it,
//! and only then makes the claim (AGENTS.md §4). On 2026-10-18 one swarm had four
//! claim races, and another swarm's coordinator started work from a listing twenty minutes old, after
//! another agent had claimed it.
//!
//! - **Who holds an issue:** the `status::in-progress` label (and the newest `claimed: <name>, PR
//!   !<m>` comment, for who); an open pull request for it (its branch is `<n>-…`, it closes `#n`,
//!   or its title names `#n`); or another agent's `claim` or `work` event on the tracking branch
//!   in the last 24 hours with no later `stop`, `release` or `merge` ([`crate::event`]).
//! - **The claim:** a branch `<n>-<slug>` from the latest `origin/main` with an empty claim
//!   commit, pushed; a draft PR that closes the issue; the label; the comment; the `claim` event,
//!   then `work` (a claim starts the work, and the timeline marks where).
//!   Steps already done (by the same agent, an earlier run that failed half way) are kept, so the
//!   command can be run again. After the PR exists it checks once more: an older PR for the issue
//!   means another agent won the race, and its own PR is closed again. A branch already on
//!   origin is resumed only when its tip is a claim commit (`Claim #<n>: …`): one with work on it
//!   is another agent's, and is refused; and when GitHub refuses the PR because one for the issue
//!   exists by now, the error names the PR that won the race ([`lost_race`]).
//! - **A hold:** an issue labelled `status::blocked` is refused: a coordinator or the owner
//!   holds work with that label and a comment saying why. `--force` takes it once the hold is
//!   lifted by hand.
//! - **A takeover:** `--take-over` adopts a stale claim's branch and PR, its commits kept
//!   ([`crate::takeover`]): when the coordinator released the claim, or it has been idle for
//!   [`LIVE_SECS`]. `--force` never adopts a branch with work on it, and says to use it.
//! - **The periodic full gate**: on a machine that runs it
//!   ([`crate::fullgate::capable`]) with room for a build, a claim runs the full gate of `main`
//!   when it is due, **after** it holds the issue ([`plan`]: on 2026-10-22 an
//!   issue was claimed twice while a claim's gate ran first). When `main` is red after it (the gate
//!   filed a `main-red` issue, which the agent claims instead, or one is open), the task's claim
//!   is released ([`release_plan`]): its draft PR closed, the label off, a `release --reason
//!   main-red` event and a comment. `--no-fullgate` skips it: the coordinator's claims while it
//!   merges.
//! - **What it needs**: an issue whose description names others on a
//!   `Needs` line ([`crate::needs`]), this project's or another's (`mornycorp/swarm-core#4`),
//!   gets a note listing those still open, and their PRs. It is a note, not
//!   a refusal: a worker can build what doesn't wait on them, and `prwatch` says when they land.
//! - **The guard:** `--check` reads the issue from the current branch's name and warns when
//!   another agent holds it, or it is on hold, or `main` is red (a failure may not be the branch's
//!   own); `--strict`: an error when another agent holds it. `cargo xtask ci --changed` runs it,
//!   and `scripts/hooks/pre-push` does before a push.
//! - **The worktree:** a claim switches the worktree it ran in to the new branch when that
//!   worktree is clean and isn't the owner's (primary) checkout, which stays on `main` (AGENTS.md
//!   §6). `--no-switch` leaves it where it is.
//!
//! The decisions are pure functions over parsed GitHub and tracking data (tested); the `gh` and
//! `git` calls around them are thin.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::process::git;
use crate::{Hooks, Project, event};
use crate::{github, needs};

/// How long a `claim` or `work` event holds an issue with no later `stop` or `release` (the
/// 24-hour rule of AGENTS.md §4 step 6).
pub const LIVE_SECS: i64 = 24 * 3600;

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask claim <issue> --agent <instance> [--name <agent name>] [--slug <s>] [--no-pr] [--no-switch] [--no-fullgate] [--force] [--take-over] | --check [--strict] [--branch <b>]";

/// The whole usage, for `project`.
fn usage(project: &Project) -> String {
    format!(
        "usage: cargo xtask claim <issue> [--agent <instance>] [--name <agent name>] [--slug <slug>] [--no-pr] [--no-switch] [--no-fullgate] [--force]
       cargo xtask claim <issue> --take-over [--agent <instance>] [--name <agent name>] [--no-switch]
       cargo xtask claim --check [--strict] [--branch <branch>] [--agent <instance>] [--name <agent name>]
  --agent    your instance id (`event session.start` prints it; else {})
  --name     your agent name for the claim comment (default: your session.start's name)
  --slug     the branch name after `<issue>-` (default: from the issue's title)
  --no-pr    label, comment and event only, no branch or PR (the coordinator's own claims)
  --no-switch  leave this worktree where it is (by default a clean worktree, not the
             primary checkout, switches to the claim's branch)
  --no-fullgate  skip the periodic full gate of main that a machine that can runs when it is
             due, once the issue is held (the coordinator's claims while it merges)
  --force    claim even though someone holds it, on a new branch and PR (a hold lifted by hand)
  --take-over  take over a stale claim and keep its branch and PR: when the coordinator released
             it (`event release --of`), or it has had no activity for 24 hours
  --check    warn when another agent holds the current branch's issue (--strict: fail)",
        project.env_name("AGENT")
    )
}

/// A pull request, as claiming needs it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pr {
    pub number: u64,
    pub state: String,
    pub draft: bool,
    pub head_ref: String,
    pub title: String,
    pub body: String,
    pub html_url: String,
}

impl Pr {
    /// Does this PR work on `issue`: its branch is `<issue>-…`, its body closes `#issue`, or its
    /// title names it? A mere mention in the body doesn't count (another swarm's twin PR mentions
    /// this repo's issues).
    pub fn is_for(&self, issue: u64) -> bool {
        issue_of_branch(&self.head_ref) == Some(issue) || mentions(&self.title, issue) || closes(&self.body, issue)
    }
}

/// The pull requests in a list in [`crate::github`]'s shape; entries without a number are
/// skipped.
pub fn parse_prs(list: &[Value]) -> Vec<Pr> {
    let text = |m: &Value, k: &str| m.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    list.iter()
        .filter_map(|m| {
            Some(Pr {
                number: m.get("number")?.as_u64()?,
                state: text(m, "state"),
                draft: m.get("draft").and_then(Value::as_bool).unwrap_or(false),
                head_ref: text(m, "head_ref"),
                title: text(m, "title"),
                body: text(m, "body"),
                html_url: text(m, "html_url"),
            })
        })
        .collect()
}

/// The issue a branch is for: `271-xtask-claim` is for #271.
pub fn issue_of_branch(branch: &str) -> Option<u64> {
    let (n, rest) = branch.split_once('-')?;
    if rest.is_empty() || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

/// Does `text` name `#issue` (not `#2710` for 271, an HTML entity `&#271;`, nor another
/// repository's `owner/repo#271` or a short name's `ns#271`)?
pub fn mentions(text: &str, issue: u64) -> bool {
    let needle = format!("#{issue}");
    text.match_indices(&needle).any(|(at, _)| {
        let before = text.get(..at).and_then(|s| s.chars().next_back());
        let after = text.get(at + needle.len()..).and_then(|s| s.chars().next());
        !before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '&' | '/' | '-' | '_' | '.')) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// Does `text` close `#issue` with a closing word (`Closes #271`, `fixes #271`)? GitHub's own
/// words, and `implements`, which says the same in this swarm's PRs.
pub fn closes(text: &str, issue: u64) -> bool {
    let lower = text.to_lowercase();
    ["close", "closes", "closed", "closing", "fix", "fixes", "fixed", "fixing", "resolve", "resolves", "resolved", "resolving", "implement", "implements"]
        .iter()
        .any(|w| [format!("{w} #{issue}"), format!("{w}: #{issue}")].iter().any(|p| mentions_at(&lower, p)))
}

/// `pattern` (ending in the issue number) occurs in `text` as words: not inside a longer word or
/// number.
fn mentions_at(text: &str, pattern: &str) -> bool {
    text.match_indices(pattern).any(|(at, _)| {
        let before = text.get(..at).and_then(|s| s.chars().next_back());
        let after = text.get(at + pattern.len()..).and_then(|s| s.chars().next());
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// A claim comment: `claimed: <name>, PR #<m>` (a worker) or `claimed: <name>` (the coordinator).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimNote {
    pub name: String,
    pub mr: Option<u64>,
    pub at: String,
}

/// The claim a comment makes, if it is one.
pub fn parse_claim_note(body: &str) -> Option<(String, Option<u64>)> {
    let rest = body.trim().strip_prefix("claimed:")?.lines().next()?.trim();
    let (name, mr) = match rest.split_once(',') {
        Some((name, tail)) => {
            let mr = tail.split_whitespace().find_map(|w| w.trim_matches(|c: char| !c.is_ascii_digit() && c != '#').strip_prefix('#')?.parse().ok());
            (name, mr)
        }
        None => (rest, None),
    };
    // Agent names have no spaces: anything after the first word is a remark, such as the instance
    // in `claimed: coordinator (cmaughan-dt9-719698), PR #268`.
    let name = name.split_whitespace().next().unwrap_or_default();
    (!name.is_empty()).then(|| (name.to_owned(), mr))
}

/// The newest claim comment among an issue's comments (`body` and `created_at` each).
pub fn latest_claim(notes: &[Value]) -> Option<ClaimNote> {
    notes
        .iter()
        .filter_map(|n| {
            let (name, mr) = parse_claim_note(n.get("body")?.as_str()?)?;
            Some(ClaimNote { name, mr, at: n.get("created_at").and_then(Value::as_str).unwrap_or_default().to_owned() })
        })
        // ISO 8601 times in one zone sort as text.
        .max_by(|a, b| a.at.cmp(&b.at))
}

/// Every claim comment among an issue's comments, oldest first.
pub fn all_claims(notes: &[Value]) -> Vec<ClaimNote> {
    let mut claims: Vec<ClaimNote> = notes
        .iter()
        .filter_map(|n| {
            let (name, mr) = parse_claim_note(n.get("body")?.as_str()?)?;
            Some(ClaimNote { name, mr, at: n.get("created_at").and_then(Value::as_str).unwrap_or_default().to_owned() })
        })
        .collect();
    // ISO 8601 times in one zone sort as text.
    claims.sort_by(|a, b| a.at.cmp(&b.at));
    claims
}

/// The agent a release comment (`released: <name>, <why>`, [`release_comment`]) names.
pub fn parse_release_note(body: &str) -> Option<String> {
    let rest = body.trim().strip_prefix("released:")?;
    let name = rest.split(|c: char| c.is_whitespace() || c == ',').find(|w| !w.is_empty())?;
    Some(name.to_owned())
}

/// The claim comments still standing at `now`: made within [`LIVE_SECS`], and not followed by its
/// agent's release comment, nor by a `release` or `stop` of the issue in its agent's tracking.
/// Oldest first: the first is the claim that holds the issue when claims raced (mornycorp/swarm-core#7).
pub fn active_claims(notes: &[Value], tracked: &[Tracked], issue: u64, now: i64) -> Vec<ClaimNote> {
    let releases: Vec<(String, String)> = notes
        .iter()
        .filter_map(|n| Some((parse_release_note(n.get("body")?.as_str()?)?, n.get("created_at").and_then(Value::as_str).unwrap_or_default().to_owned())))
        .collect();
    all_claims(notes)
        .into_iter()
        .filter(|c| parse_time(&c.at).is_some_and(|at| now.saturating_sub(at) <= LIVE_SECS))
        .filter(|c| !releases.iter().any(|(name, at)| name == &c.name && at > &c.at))
        .filter(|c| !released_since(tracked, issue, c))
        .collect()
}

/// Whether claim `c`'s agent logged its own `release` or `stop` of `issue` after the comment, or
/// another agent (the coordinator) logged `release --of` one of its instances
/// (mornycorp/swarm-core#11).
fn released_since(tracked: &[Tracked], issue: u64, c: &ClaimNote) -> bool {
    let at = parse_time(&c.at).unwrap_or(i64::MAX);
    let is_claimant = |instance: &str| instance == c.name || tracked.iter().any(|o| o.instance == instance && o.name.as_deref() == Some(c.name.as_str()));
    tracked.iter().any(|e| {
        e.issue == Some(issue)
            && e.t >= at
            && match e.of.as_deref() {
                None => e.name.as_deref() == Some(c.name.as_str()) && (e.event == "release" || e.event == "stop"),
                Some(of) => e.event == "release" && is_claimant(of),
            }
    })
}

/// The claim that came first, when it isn't `me`'s: the race `me` lost. `active` is
/// [`active_claims`]' answer, mine among it.
pub fn first_claim<'a>(active: &'a [ClaimNote], me: &str) -> Option<&'a ClaimNote> {
    active.first().filter(|c| c.name != me)
}

/// The newest comment among an issue's comments that isn't a claim, as its first line (at most
/// 200 characters): what a hold says about itself.
pub fn latest_remark(notes: &[Value]) -> Option<String> {
    notes
        .iter()
        .filter_map(|n| {
            let body = n.get("body")?.as_str()?.trim();
            if body.is_empty() || parse_claim_note(body).is_some() {
                return None;
            }
            Some((n.get("created_at").and_then(Value::as_str).unwrap_or_default(), body))
        })
        // ISO 8601 times in one zone sort as text.
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, body)| body.lines().next().unwrap_or_default().chars().take(200).collect())
}

/// Why `issue` is on hold (labelled `project`'s blocked label), if it is: the label and what its
/// newest comment says.
pub fn blocked_hold(project: &Project, issue: u64, facts: &Facts) -> Option<String> {
    let blocked = project.labels.blocked;
    if !facts.labels.iter().any(|l| l == blocked) {
        return None;
    }
    Some(match &facts.remark {
        Some(r) => format!("#{issue} is on hold ({blocked}): {r}"),
        None => format!("#{issue} is on hold ({blocked}); no comment says why"),
    })
}

/// One event on the tracking branch, with the instance that logged it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracked {
    pub instance: String,
    /// The agent's name, from the instance's `session.start`.
    pub name: Option<String>,
    /// Seconds since the Unix epoch.
    pub t: i64,
    pub event: String,
    pub issue: Option<u64>,
    pub mr: Option<u64>,
    pub of: Option<String>,
}

/// Every event in the tracking files (instance, `work.jsonl` text), oldest first per instance.
/// Lines that don't parse, or have no time, are skipped.
pub fn parse_tracking(files: &[(String, String)]) -> Vec<Tracked> {
    let mut all = Vec::new();
    for (instance, text) in files {
        let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let name =
            lines.iter().find(|v| v.get("event").and_then(Value::as_str) == Some("session.start")).and_then(|v| v.get("name")?.as_str()).map(str::to_owned);
        for v in &lines {
            let (Some(event), Some(t)) = (v.get("event").and_then(Value::as_str), v.get("t").and_then(Value::as_str).and_then(parse_time)) else { continue };
            all.push(Tracked {
                instance: instance.clone(),
                name: name.clone(),
                t,
                event: event.to_owned(),
                issue: v.get("issue").and_then(Value::as_u64),
                mr: v.get("mr").and_then(Value::as_u64),
                of: v.get("of").and_then(Value::as_str).map(str::to_owned),
            });
        }
    }
    all
}

pub use crate::time::parse_time;

/// An instance's live hold on an issue: its newest `claim` or `work` event.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveClaim {
    pub instance: String,
    pub name: Option<String>,
    pub event: String,
    pub t: i64,
    /// When its hold began: its newest `claim` event at or before the hold (a later `work` doesn't
    /// move it). Of two live claims on one PR, the earlier holds it.
    pub claimed_at: i64,
    /// The PR its claim named.
    pub mr: Option<u64>,
}

/// The instances holding `issue` at `now`: a `claim` or `work` event in the last [`LIVE_SECS`]
/// with no later `stop` or `release` of its own, no `release` of it by another (`of`), and no
/// `merge` of the issue by anyone, itself included.
pub fn live_claims(events: &[Tracked], issue: u64, now: i64) -> Vec<LiveClaim> {
    let on: Vec<&Tracked> = events.iter().filter(|e| e.issue == Some(issue)).collect();
    let instances: BTreeSet<&str> = on.iter().map(|e| e.instance.as_str()).collect();
    let mut live = Vec::new();
    for inst in instances {
        // Its own events are in file order, which is time order.
        let own: Vec<&&Tracked> = on.iter().filter(|e| e.instance == inst).collect();
        let Some(last_hold) = own.iter().rposition(|e| e.event == "claim" || e.event == "work") else { continue };
        let Some(hold) = own.get(last_hold) else { continue };
        if now.saturating_sub(hold.t) > LIVE_SECS {
            continue;
        }
        // Its own release ends it unless it released another's work (`of`, the coordinator).
        let ended_by_self = own
            .iter()
            .skip(last_hold + 1)
            .any(|e| e.event == "stop" || e.event == "merge" || (e.event == "release" && e.of.as_deref().is_none_or(|o| o == inst)));
        let ended_by_other =
            on.iter().any(|e| e.instance != inst && e.t >= hold.t && (e.event == "merge" || (e.event == "release" && e.of.as_deref() == Some(inst))));
        if ended_by_self || ended_by_other {
            continue;
        }
        let mr = own.iter().take(last_hold + 1).rev().find_map(|e| e.mr);
        let claimed_at = own.iter().take(last_hold + 1).rev().find(|e| e.event == "claim").map_or(hold.t, |e| e.t);
        live.push(LiveClaim { instance: inst.to_owned(), name: hold.name.clone(), event: hold.event.clone(), t: hold.t, claimed_at, mr });
    }
    live
}

/// Who is asking: the instance, its agent name, and the branch it works on.
#[derive(Debug, Clone, Default)]
pub struct Me {
    pub instance: Option<String>,
    pub name: Option<String>,
    pub branch: Option<String>,
}

impl Me {
    /// Is the instance `instance` (agent `name`) me: my instance, or my agent name?
    pub(crate) fn is(&self, instance: &str, name: Option<&str>) -> bool {
        self.instance.as_deref() == Some(instance) || (self.name.is_some() && self.name.as_deref() == name)
    }
}

/// What GitHub and the tracking branch say about an issue.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub labels: Vec<String>,
    pub mrs: Vec<Pr>,
    pub claim_note: Option<ClaimNote>,
    /// Every claim comment still standing ([`active_claims`]), oldest first.
    pub claims: Vec<ClaimNote>,
    pub tracked: Vec<Tracked>,
    /// The newest comment that isn't a claim: why an issue is held, when it is.
    pub remark: Option<String>,
    /// The issue's description: what it needs.
    pub description: String,
}

/// Someone other than [`Me`] holding the issue.
#[derive(Debug, Clone, PartialEq)]
pub enum Holder {
    /// The label is on; the newest claim comment, if any, says by whom.
    Label(Option<ClaimNote>),
    Pr(Pr),
    Tracking(LiveClaim),
}

/// The open PRs of `facts` for `issue` that are mine: named by my own claim (in the tracking
/// branch, or my claim comment), or from my branch when no one else's claim names it. Every agent
/// derives the same branch name for an issue, so the branch alone doesn't make a PR mine: that
/// let a second claim adopt the first's PR (mornycorp/swarm-core#7).
pub fn my_mrs(issue: u64, facts: &Facts, me: &Me, now: i64) -> BTreeSet<u64> {
    let live = live_claims(&facts.tracked, issue, now);
    let theirs: BTreeSet<u64> = live
        .iter()
        .filter(|c| !me.is(&c.instance, c.name.as_deref()))
        .filter_map(|c| c.mr)
        .chain(facts.claims.iter().filter(|n| me.name.as_deref() != Some(n.name.as_str())).filter_map(|n| n.mr))
        .collect();
    let mut mine: BTreeSet<u64> =
        facts.mrs.iter().filter(|m| me.branch.as_deref() == Some(m.head_ref.as_str()) && !theirs.contains(&m.number)).map(|m| m.number).collect();
    mine.extend(live.into_iter().filter(|c| me.is(&c.instance, c.name.as_deref())).filter_map(|c| c.mr));
    if let Some(note) = &facts.claim_note
        && me.name.as_deref() == Some(note.name.as_str())
    {
        mine.extend(note.mr);
    }
    mine
}

/// Everyone but `me` holding `issue` at `now`. `label_needs_name`: the label alone counts, unless
/// the newest claim comment is mine (a claim); with `false`, only a comment naming someone else
/// does (the guard, which can't tell a claim with no comment from its own).
pub fn holders(project: &Project, issue: u64, facts: &Facts, me: &Me, now: i64, label_needs_name: bool) -> Vec<Holder> {
    let mine = my_mrs(issue, facts, me, now);
    let mut out = Vec::new();
    if facts.labels.iter().any(|l| l == project.labels.in_progress) {
        let ours = facts.claim_note.as_ref().is_some_and(|n| me.name.as_deref() == Some(n.name.as_str()) || n.mr.is_some_and(|m| mine.contains(&m)));
        let counts = if label_needs_name { !ours } else { facts.claim_note.is_some() && !ours };
        if counts {
            out.push(Holder::Label(facts.claim_note.clone()));
        }
    }
    let mut seen = BTreeSet::new();
    for m in &facts.mrs {
        if m.state == "open" && m.is_for(issue) && !mine.contains(&m.number) && seen.insert(m.number) {
            out.push(Holder::Pr(m.clone()));
        }
    }
    let live = live_claims(&facts.tracked, issue, now);
    // My hold's start; another live claim on a PR we share holds it when it began first.
    let my_start = live.iter().filter(|c| me.is(&c.instance, c.name.as_deref())).map(|c| (c.claimed_at, c.instance.clone())).min();
    for c in live {
        if me.is(&c.instance, c.name.as_deref()) {
            continue;
        }
        let shared = c.mr.is_some_and(|m| mine.contains(&m));
        let first = my_start.as_ref().is_none_or(|(t, inst)| (c.claimed_at, c.instance.as_str()) < (*t, inst.as_str()));
        if !shared || first {
            out.push(Holder::Tracking(c));
        }
    }
    out
}

/// Who, other than `me`, holds `issue` so that `me` mustn't push its branch: another agent's live
/// claim that began first ([`holders`]' tracking holders), or another agent's claim comment that
/// stands before mine ([`first_claim`]). A label or another PR alone doesn't stop a push: they
/// can be stale, or a sibling's. Each a line ([`describe`]).
pub fn push_holders(project: &Project, issue: u64, facts: &Facts, me: &Me, now: i64) -> Vec<String> {
    let mut out: Vec<String> = holders(project, issue, facts, me, now, false)
        .into_iter()
        .filter(|h| matches!(h, Holder::Tracking(_)))
        .map(|h| describe(project.labels.in_progress, &h, now))
        .collect();
    if let Some(name) = me.name.as_deref()
        && let Some(first) = first_claim(&facts.claims, name)
        && facts.claims.iter().any(|c| c.name == name)
    {
        out.push(format!("{} claimed it first ({}), by its comment on the issue", first.name, first.at));
    }
    out
}

/// [`push_holders`] for the issue of `branch` (`<n>-<slug>`), read now; `agent` is the instance
/// pushing. Empty for a branch that names no issue.
pub fn held_against(project: &Project, root: &Path, branch: &str, agent: &str) -> Result<Vec<String>, String> {
    let Some(issue) = issue_of_branch(branch) else { return Ok(Vec::new()) };
    let (facts, _, _) = facts(project, root, issue)?;
    let name = name_of(&facts.tracked, agent);
    let me = Me { instance: Some(agent.to_owned()), name, branch: Some(branch.to_owned()) };
    Ok(push_holders(project, issue, &facts, &me, now()))
}

/// One line saying who holds the issue, and where; `label` is the claim label.
pub fn describe(label: &str, h: &Holder, now: i64) -> String {
    match h {
        Holder::Label(Some(n)) => {
            let mr = n.mr.map(|m| format!(", PR #{m}")).unwrap_or_default();
            format!("labelled {label}: claimed by {}{mr} at {}", n.name, n.at)
        }
        Holder::Label(None) => format!("labelled {label}, with no claim comment"),
        Holder::Pr(m) => {
            let draft = if m.draft { "draft " } else { "" };
            format!("open {draft}PR #{} `{}` (branch {}) {}", m.number, m.title, m.head_ref, m.html_url)
        }
        Holder::Tracking(c) => {
            let mr = c.mr.map(|m| format!(", PR #{m}")).unwrap_or_default();
            let mins = now.saturating_sub(c.t) / 60;
            let ago = if mins < 120 { format!("{mins} min ago") } else { format!("{} h ago", mins / 60) };
            format!(
                "{} ({}): `{}` at {}{mr}, {ago}, not stopped or released",
                c.name.as_deref().unwrap_or("an agent"),
                c.instance,
                c.event,
                crate::time::iso_utc(u64::try_from(c.t).unwrap_or(0))
            )
        }
    }
}

/// A branch slug from an issue title: lower case words joined by `-`, at most 32 characters.
pub fn slugify(title: &str) -> String {
    let words: Vec<String> = title.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect();
    let mut slug = String::new();
    for w in words {
        if !slug.is_empty() && slug.len() + 1 + w.len() > 32 {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(w.get(..32).unwrap_or(&w));
    }
    if slug.is_empty() { "work".into() } else { slug }
}

/// Is `slug` fit for a branch name (letters, digits, `-`)?
pub fn check_slug(slug: &str) -> Result<(), String> {
    let ok = !slug.is_empty() && slug.len() <= 60 && !slug.starts_with('-') && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if ok { Ok(()) } else { Err(format!("--slug `{slug}`: use letters, digits and `-` (at most 60)")) }
}

/// The draft PR's description: the template with its `Closes #` line filled in, or just that line.
pub fn pr_body(template: Option<&str>, issue: u64) -> String {
    let closes = format!("Closes #{issue}");
    match template {
        Some(t) if t.lines().any(|l| l.trim_start().starts_with("Closes #")) => {
            t.lines().map(|l| if l.trim_start().starts_with("Closes #") { closes.as_str() } else { l }).collect::<Vec<_>>().join("\n") + "\n"
        }
        Some(t) => format!("{closes}\n\n{t}"),
        None => format!("{closes}\n"),
    }
}

/// The claim comment.
pub fn claim_comment(name: &str, mr: Option<u64>) -> String {
    match mr {
        Some(m) => format!("claimed: {name}, PR #{m}"),
        None => format!("claimed: {name}"),
    }
}

/// An earlier run's PR to keep: the open PR from the claim's own `branch`, which the branch
/// step has checked ends in this issue's claim commit. A run that stopped after opening it, before
/// its comment and events, left nothing else saying it is mine.
pub fn own_mr<'a>(mrs: &'a [Pr], branch: &str) -> Option<&'a Pr> {
    mrs.iter().find(|m| m.state == "open" && m.head_ref == branch)
}

/// GitHub refused to open the claim's PR from `branch` (`error`), and `mrs` are the issue's PRs
/// now: an earlier run's own PR is kept ([`own_mr`]); another branch's open PR for the issue won
/// the race ([`lost_race`]); else the refusal as it was.
pub fn after_refusal(issue: u64, branch: &str, mrs: &[Pr], error: String) -> Result<Pr, String> {
    match own_mr(mrs, branch) {
        Some(m) => Ok(m.clone()),
        None => Err(lost_race(issue, mrs, error)),
    }
}

/// The error for a refused claim PR: the race lost to the oldest open PR for `issue`, if there is
/// one, else GitHub's own error `e`.
pub fn lost_race(issue: u64, mrs: &[Pr], e: String) -> String {
    match mrs.iter().filter(|m| m.state == "open" && m.is_for(issue)).min_by_key(|m| m.number) {
        Some(w) => format!("claim: lost the claim race to #{} ({}); pick another issue ({e})", w.number, w.head_ref),
        None => e,
    }
}

/// After my PR `mine` exists: an older open PR for the issue that isn't mine won the race.
pub fn race_winner(issue: u64, mrs: &[Pr], mine: u64) -> Option<&Pr> {
    mrs.iter().filter(|m| m.state == "open" && m.is_for(issue) && m.number < mine).min_by_key(|m| m.number)
}

// ---- I/O: thin wrappers around GitHub and git. ----

/// Every open PR of the repository: those for `issue` are among them (one from a `<n>-…` branch, or
/// one that closes or names it, [`Pr::is_for`]).
fn prs_for(project: &Project, issue: u64) -> Result<Vec<Pr>, String> {
    let _ = issue;
    Ok(parse_prs(&github::open_prs(project, None)?))
}

/// The tracking branch's files, fetched now: (instance, `work.jsonl` text); none before the
/// branch exists.
pub fn tracking_files(project: &Project, root: &Path) -> Result<Vec<(String, String)>, String> {
    let Some(tip) = event::fetch_tip(root, project.branches.remote, &format!("refs/heads/{}", project.branches.tracking))? else {
        return Ok(Vec::new());
    };
    tracking_files_at(root, &tip)
}

/// The tracking branch's files at the commit `tip`, which the clone has: (instance, `work.jsonl`
/// text).
pub fn tracking_files_at(root: &Path, tip: &str) -> Result<Vec<(String, String)>, String> {
    let listing = git(root, &["ls-tree", "-r", tip, "--", "tracking"], None, &[])?;
    let entries = parse_ls_tree(&listing);
    let ids: String = entries.iter().map(|(_, id)| format!("{id}\n")).collect();
    let batch = git(root, &["cat-file", "--batch"], Some(ids.as_bytes()), &[])?;
    Ok(parse_batch(&batch, &entries))
}

/// `git ls-tree -r` lines for `tracking/<instance>/work.jsonl`: (instance, blob id).
pub fn parse_ls_tree(listing: &str) -> Vec<(String, String)> {
    listing
        .lines()
        .filter_map(|l| {
            let (meta, path) = l.split_once('\t')?;
            let id = meta.split_whitespace().nth(2)?;
            let instance = path.strip_prefix("tracking/")?.strip_suffix("/work.jsonl")?;
            event::check_id(instance).ok()?;
            Some((instance.to_owned(), id.to_owned()))
        })
        .collect()
}

/// `git cat-file --batch` output for `entries`' blobs, in order: (instance, text).
pub fn parse_batch(out: &str, entries: &[(String, String)]) -> Vec<(String, String)> {
    let bytes = out.as_bytes();
    let mut at = 0usize;
    let mut files = Vec::new();
    for (instance, _) in entries {
        let Some(nl) = bytes.get(at..).and_then(|b| b.iter().position(|c| *c == b'\n')) else { break };
        let header = String::from_utf8_lossy(bytes.get(at..at + nl).unwrap_or_default()).into_owned();
        at += nl + 1;
        let Some(size) = header.split_whitespace().nth(2).and_then(|s| s.parse::<usize>().ok()) else { continue };
        let Some(body) = bytes.get(at..at.saturating_add(size)) else { break };
        files.push((instance.clone(), String::from_utf8_lossy(body).into_owned()));
        at = at.saturating_add(size).saturating_add(1);
    }
    files
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// An issue as GitHub and the tracking branch say it is now.
struct Read {
    facts: Facts,
    title: String,
    state: String,
    /// Its comments, in [`github`]'s shape.
    notes: Vec<Value>,
}

/// Everything GitHub and the tracking branch say about `issue`, and its title and state.
fn facts(project: &Project, root: &Path, issue: u64) -> Result<(Facts, String, String), String> {
    read(project, root, issue).map(|r| (r.facts, r.title, r.state))
}

/// [`facts`], with the issue's comments.
fn read(project: &Project, root: &Path, issue: u64) -> Result<Read, String> {
    let info = github::issue(project, None, issue)?;
    let text = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let labels = info.get("labels").and_then(Value::as_array).map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let notes = github::comments(project, issue)?;
    let tracked = parse_tracking(&tracking_files(project, root)?);
    let mrs = prs_for(project, issue)?;
    let claims = active_claims(&notes, &tracked, issue, now());
    let facts = Facts { labels, mrs, claim_note: latest_claim(&notes), claims, tracked, remark: latest_remark(&notes), description: text("body") };
    Ok(Read { facts, title: text("title"), state: text("state"), notes })
}

/// What `issue` (described by `description`, with its open PRs `open`) needs that is still open, a line each with its PR: none when it needs nothing; one call for this
/// project's needs, two for each other project's.
fn open_needs(project: &Project, issue: u64, description: &str, open: &[Pr]) -> Vec<String> {
    let wanted: BTreeMap<needs::Ref, BTreeSet<u64>> =
        needs::parse_for(project, description).into_iter().filter(|n| *n != needs::Ref::here(issue)).map(|n| (n, BTreeSet::from([issue]))).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    needs::fetch(project, &wanted, open, &needs::GitHub(project)).into_values().filter(|n| n.state == "open").map(|n| n.status()).collect()
}

/// Say what `issue` needs that is still open: a note, never a refusal.
fn note_needs(project: &Project, issue: u64, facts: &Facts) {
    // The project's open PRs are among the issue's facts already.
    let open: Vec<Pr> = facts.mrs.iter().filter(|m| m.state == "open").cloned().collect();
    let still = open_needs(project, issue, &facts.description, &open);
    if !still.is_empty() {
        println!("claim: note: #{issue} needs work that is still open; build what doesn't wait on it (`cargo xtask prwatch` says when it lands):");
        for line in still {
            println!("  {line}");
        }
    }
}

/// Issues in [`github`]'s shape: `(number, title)` each.
pub fn parse_issues(list: &[Value]) -> Vec<(u64, String)> {
    list.iter().filter_map(|i| Some((i.get("number")?.as_u64()?, i.get("title").and_then(Value::as_str).unwrap_or_default().to_owned()))).collect()
}

/// The open `main-red` issues.
pub fn main_red(project: &Project) -> Result<Vec<(u64, String)>, String> {
    github::labelled(project, None, project.labels.main_red, "open", 1).map(|v| parse_issues(&v))
}

/// Why a claim of `issue` (labelled `labels`) must wait while `red` (the open `main-red` issues)
/// says `main` is red, if it must. The coordinator's own claims (`--no-mr`: no branch from `main`)
/// and a claim of a `main-red` issue itself (the fix) go ahead.
pub fn red_hold(project: &Project, red: &[(u64, String)], issue: u64, labels: &[String], no_mr: bool) -> Option<String> {
    let exempt = no_mr || labels.iter().any(|l| l == project.labels.main_red) || fixes_red(red, &[issue]);
    main_is_red(
        red,
        exempt,
        "No work starts from main until it is green again (docs/swarm-protocol.md, step 3): wait for the issue to close, or keep working on a branch you already have.",
    )
}

/// Does `issues` (an issue claimed, or those a PR closes) name one of `red`, the open `main-red`
/// issues? Its work is the fix, which a red `main` never holds.
pub fn fixes_red(red: &[(u64, String)], issues: &[u64]) -> bool {
    red.iter().any(|(n, _)| issues.contains(n))
}

/// The one check `claim` and `land` share: while `red` (the open `main-red` issues)
/// says `main` is red, why the work must wait, naming the issues, then `rule`; `None` when `main`
/// is green or the work is `exempt` (each caller says when: the fix, or a reason given).
pub fn main_is_red(red: &[(u64, String)], exempt: bool, rule: &str) -> Option<String> {
    if red.is_empty() || exempt {
        return None;
    }
    let list: String = red.iter().map(|(n, t)| format!("\n  - #{n} {t}")).collect();
    Some(format!("main is red:{list}\n{rule}"))
}

/// The name my instance gave at `session.start`.
fn name_of(tracked: &[Tracked], instance: &str) -> Option<String> {
    tracked.iter().find(|e| e.instance == instance).and_then(|e| e.name.clone())
}

#[derive(Default)]
struct Opts {
    issue: Option<u64>,
    agent: Option<String>,
    name: Option<String>,
    slug: Option<String>,
    branch: Option<String>,
    force: bool,
    take_over: bool,
    no_mr: bool,
    no_switch: bool,
    no_fullgate: bool,
    check: bool,
    strict: bool,
}

fn parse_args(project: &Project, args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().map(str::to_owned).ok_or_else(|| format!("claim: {flag} needs a value\n\n{}", usage(project)));
        match a {
            "--agent" => o.agent = Some(value(a)?),
            "--name" => o.name = Some(value(a)?),
            "--slug" => o.slug = Some(value(a)?),
            "--branch" => o.branch = Some(value(a)?),
            "--force" => o.force = true,
            "--take-over" => o.take_over = true,
            "--no-pr" => o.no_mr = true,
            "--no-switch" => o.no_switch = true,
            "--no-fullgate" => o.no_fullgate = true,
            "--check" => o.check = true,
            "--strict" => o.strict = true,
            "-h" | "--help" => return Err(usage(project)),
            n if o.issue.is_none() && !n.starts_with("--") => {
                o.issue = Some(n.trim_start_matches('#').parse().map_err(|_| format!("claim: `{n}` is not an issue number\n\n{}", usage(project)))?);
            }
            other => return Err(format!("claim: unknown argument `{other}`\n\n{}", usage(project))),
        }
    }
    o.agent = o.agent.or_else(|| project.env("AGENT").filter(|s| !s.is_empty()));
    if let Some(a) = &o.agent {
        event::check_id(a).map_err(|e| format!("--agent: {e}"))?;
    }
    if let Some(s) = &o.slug {
        check_slug(s)?;
    }
    Ok(o)
}

/// `cargo xtask claim …` ([`USAGE`]) for `project`.
pub fn run(project: &Project, hooks: &dyn Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    let o = parse_args(project, args)?;
    if o.check {
        if o.issue.is_some() || o.force || o.take_over || o.no_mr || o.no_switch || o.no_fullgate || o.slug.is_some() {
            return Err(format!("claim --check takes no issue, --force, --take-over, --no-pr, --no-switch, --no-fullgate or --slug\n\n{}", usage(project)));
        }
        return check(project, root, &o);
    }
    if o.strict || o.branch.is_some() {
        return Err(format!("claim: --strict and --branch go with --check\n\n{}", usage(project)));
    }
    if o.take_over && (o.no_mr || o.slug.is_some()) {
        return Err(format!("claim: --take-over keeps the claim's own branch and PR: it takes no --no-pr or --slug\n\n{}", usage(project)));
    }
    claim(&Ctx { project, hooks, root }, &o)
}

/// What every step of a claim works with: whose, how, where.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub project: &'a Project,
    pub hooks: &'a dyn Hooks,
    pub root: &'a Path,
}

/// The variable that turns the guard off (`{prefix}_CLAIM_CHECK=off`): a claim's own push sets it.
fn check_off(project: &Project) -> String {
    project.env_name("CLAIM_CHECK")
}

/// The guard: warn when another agent holds the branch's issue; an error with `--strict`. It
/// can't check (offline, no gh): a warning, never an error, so it doesn't block a push.
fn check(project: &Project, root: &Path, o: &Opts) -> Result<(), String> {
    if project.env("CLAIM_CHECK").is_some_and(|v| v.eq_ignore_ascii_case("off")) || std::env::var_os("GITHUB_ACTIONS").is_some() {
        println!("claim check: off");
        return Ok(());
    }
    let branch = match &o.branch {
        Some(b) => b.clone(),
        None => git(root, &["rev-parse", "--abbrev-ref", "HEAD"], None, &[])?.trim().to_owned(),
    };
    // A red main: a failure may be someone else's breakage, not this branch's.
    if let Ok(red) = main_red(project)
        && !red.is_empty()
    {
        let list: Vec<String> = red.iter().map(|(n, t)| format!("#{n} {t}")).collect();
        eprintln!("warning: main is red ({}): a failure may not be yours; nothing merges until it is green (docs/swarm-protocol.md, step 3)", list.join("; "));
    }
    let Some(issue) = issue_of_branch(&branch) else {
        println!("claim check: branch `{branch}` names no issue (<n>-<slug>); nothing to check");
        return Ok(());
    };
    let (facts, _, _) = match facts(project, root, issue) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("warning: claim check: couldn't check #{issue} ({e})");
            return Ok(());
        }
    };
    if let Some(why) = blocked_hold(project, issue, &facts) {
        eprintln!("warning: claim check: {why}");
    }
    let name = o.name.clone().or_else(|| o.agent.as_deref().and_then(|a| name_of(&facts.tracked, a)));
    let me = Me { instance: o.agent.clone(), name, branch: Some(branch.clone()) };
    let now = now();
    let held = holders(project, issue, &facts, &me, now, false);
    if held.is_empty() {
        println!("claim check: #{issue} (branch {branch}): nobody else holds it");
        return Ok(());
    }
    eprintln!("warning: another agent holds #{issue}, which branch {branch} is for:");
    for h in &held {
        eprintln!("  - {}", describe(project.labels.in_progress, h, now));
    }
    eprintln!("  The earlier claim keeps it (AGENTS.md §4): release yours, or agree a takeover on the issue.");
    if o.strict { Err(format!("claim check: #{issue} is held by another agent")) } else { Ok(()) }
}

/// The holds a claim of `issue` waits for: a hold on the issue (`--force` takes it when its holder
/// has said so), and a red main, which holds everyone but a `--no-mr` claim and the main-red issue
/// itself. Main's colour unknown (GitHub can't say): a warning, and the claim goes ahead.
fn check_holds(project: &Project, issue: u64, facts: &Facts, o: &Opts) -> Result<(), String> {
    if let Some(why) = blocked_hold(project, issue, facts) {
        if o.take_over {
            println!("claim: {why}; the takeover clears the hold once its cause is gone, else leaves it");
        } else if !o.force {
            return Err(format!(
                "claim: {why}
Pick another issue. Only the one who held it lifts a hold: take the label off, or pass --force when they have said so."
            ));
        } else {
            println!("claim: taking #{issue} despite its hold (--force): {why}");
        }
    }
    match main_red(project) {
        Ok(red) => {
            if let Some(why) = red_hold(project, &red, issue, &facts.labels, o.no_mr) {
                return Err(format!("claim: #{issue} must wait: {why}"));
            }
        }
        Err(e) => eprintln!("warning: claim: couldn't check whether main is red ({e}); going ahead"),
    }
    Ok(())
}

/// One step of a claim once its checks have passed, in [`plan`]'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The branch `<n>-<slug>` from `origin/main` with its claim commit, pushed.
    Branch,
    /// The draft PR (or an earlier run's, resumed).
    Pr,
    /// The claim label.
    Label,
    /// The `claimed: <name>, PR !<m>` comment.
    Comment,
    /// The `claim` and `work` events.
    Events,
    /// An older PR for the issue won the race: mine is closed and released.
    Race,
    /// The periodic full gate of `main` when it is due, with the issue held.
    FullGate,
    /// Say what was claimed, and switch the worktree to its branch.
    Report,
}

/// A claim's steps after its checks. The issue is held (the branch
/// and draft PR, the label, the comment, the events) and the race checked before the periodic full
/// gate runs, so nobody takes it in the gate's 10 to 25 minutes: on 2026-10-22 an issue was
/// claimed twice in that window. The report and the worktree's switch come last, after a gate that
/// didn't release it. `--no-mr` (`no_mr`) has no branch, PR or race; `--no-fullgate` no gate.
pub fn plan(no_mr: bool, no_fullgate: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    if !no_mr {
        steps.extend([Step::Branch, Step::Pr]);
    }
    steps.extend([Step::Label, Step::Comment, Step::Events]);
    if !no_mr {
        steps.push(Step::Race);
    }
    if !no_fullgate {
        steps.push(Step::FullGate);
    }
    steps.push(Step::Report);
    steps
}

/// One step of releasing a claim after the full gate found `main` red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undo {
    /// Close the claim's draft PR (an open PR holds the issue); its branch is kept.
    ClosePr,
    /// Take the claim label off.
    Label,
    /// Log `release --reason main-red`.
    Event,
    /// Comment why ([`release_comment`]).
    Comment,
}

/// The release of a claim whose full gate found `main` red, `mr`: it has a draft PR.
pub fn release_plan(mr: bool) -> Vec<Undo> {
    let mut steps = Vec::new();
    if mr {
        steps.push(Undo::ClosePr);
    }
    steps.extend([Undo::Label, Undo::Event, Undo::Comment]);
    steps
}

/// After the full gate: why the claim of `issue` is released, if it is. `filed`: the
/// `main-red` issue the gate filed and claimed for this agent, who works on it instead; `hold`:
/// what [`red_hold`] says now (a `main-red` issue that was open already, or that another agent
/// filed while the gate ran, holds the claim as it would have held a claim made after the gate).
pub fn after_gate(issue: u64, filed: Option<u64>, hold: Option<String>) -> Option<String> {
    match filed {
        Some(n) => Some(format!("the full gate of main failed: filed main-red #{n} and claimed it; work on it instead of #{issue}")),
        None => hold,
    }
}

/// The comment on an issue whose claim the full gate released (`why`): not a claim comment.
pub fn release_comment(name: &str, why: &str) -> String {
    format!("released: {name}, {why}")
}

/// The atomic claim: its checks, then [`plan`]'s steps.
fn claim(ctx: &Ctx<'_>, o: &Opts) -> Result<(), String> {
    if o.take_over {
        return crate::takeover::run(ctx, opened(ctx, o)?, o.no_switch);
    }
    let mut c = checked(ctx, o)?;
    for step in plan(o.no_mr, o.no_fullgate) {
        c.take(step)?;
    }
    Ok(())
}

/// A claim that passed its checks, and what its steps have made so far.
struct Claiming<'a> {
    ctx: Ctx<'a>,
    root: &'a Path,
    o: &'a Opts,
    issue: u64,
    agent: String,
    name: String,
    title: String,
    branch: String,
    facts: Facts,
    /// My own open PR for the issue, from an earlier run: [`Step::Pr`] keeps it.
    resumed: Option<Pr>,
    mr: Option<Pr>,
}

/// What the checks every claim (a takeover's too) starts with found.
pub(crate) struct Opened {
    pub issue: u64,
    pub agent: String,
    pub name: String,
    pub title: String,
    pub facts: Facts,
    /// The issue's comments, in [`github`]'s shape.
    pub notes: Vec<Value>,
}

/// The checks every claim starts with: an issue and an agent named, the issue open, not on hold
/// (a takeover looks at a hold later), `main` not red; its needs and the capabilities it wants
/// noted.
fn opened(ctx: &Ctx<'_>, o: &Opts) -> Result<Opened, String> {
    let (project, root) = (ctx.project, ctx.root);
    let issue = o.issue.ok_or_else(|| format!("claim: which issue?\n\n{}", usage(project)))?;
    let agent = o.agent.clone().ok_or_else(|| {
        format!("claim: which agent? pass --agent <instance> (`cargo xtask event session.start` prints it) or set {}", project.env_name("AGENT"))
    })?;
    let main = project.branches.main;
    event::fetch(root, "origin", &format!("refs/heads/{main}"), &format!("refs/remotes/origin/{main}"))?;
    let Read { facts, title, state, notes } = read(project, root, issue)?;
    if state != "open" {
        return Err(format!("claim: #{issue} is {state}, not open"));
    }
    check_holds(project, issue, &facts, o)?;
    note_needs(project, issue, &facts);
    if let Some(w) = crate::capabilities::lacking(&ctx.hooks.capabilities(root), issue, &facts.labels) {
        eprintln!("{w}");
    }
    let name = o
        .name
        .clone()
        .or_else(|| name_of(&facts.tracked, &agent))
        .ok_or_else(|| format!("claim: no session.start for {agent} on the tracking branch; pass --name <your agent name>"))?;
    event::check_id(&name).map_err(|e| format!("--name: {e}"))?;
    Ok(Opened { issue, agent, name, title, facts, notes })
}

/// The claim's checks: [`opened`]'s, and nobody else holds the issue.
fn checked<'a>(ctx: &Ctx<'a>, o: &'a Opts) -> Result<Claiming<'a>, String> {
    let (project, root) = (ctx.project, ctx.root);
    let Opened { issue, agent, name, title, facts, notes: _ } = opened(ctx, o)?;
    let now = now();
    // Resuming: my own open PR (from an earlier run) keeps its branch.
    let mut me = Me { instance: Some(agent.clone()), name: Some(name.clone()), branch: None };
    let resumed = my_mrs(issue, &facts, &me, now).into_iter().find_map(|iid| facts.mrs.iter().find(|m| m.number == iid && m.state == "open")).cloned();
    let branch = match (&resumed, &o.slug) {
        (Some(m), _) => m.head_ref.clone(),
        (None, Some(s)) => format!("{issue}-{s}"),
        (None, None) => format!("{issue}-{}", slugify(&title)),
    };
    me.branch = Some(branch.clone());
    let held = holders(project, issue, &facts, &me, now, true);
    if !held.is_empty() {
        let list: String = held.iter().map(|h| format!("\n  - {}", describe(project.labels.in_progress, h, now))).collect();
        if !o.force {
            return Err(format!(
                "#{issue} is held by another agent:{list}\nPick another issue. To take over a stale claim and keep its branch and PR (the coordinator released it, or it has been idle 24 hours), pass --take-over."
            ));
        }
        println!("claim: taking over #{issue} (--force):{list}");
    }
    Ok(Claiming { ctx: *ctx, root, o, issue, agent, name, title, branch, facts, resumed, mr: None })
}

impl Claiming<'_> {
    fn mr_iid(&self) -> Option<u64> {
        self.mr.as_ref().map(|m| m.number)
    }

    /// One of [`plan`]'s steps. The label and the comment are kept when an earlier run of the
    /// claim made them.
    fn take(&mut self, step: Step) -> Result<(), String> {
        let (project, root, issue) = (self.ctx.project, self.root, self.issue);
        match step {
            Step::Branch => ensure_branch(project, root, &self.branch, issue, &self.title),
            Step::Pr => {
                // Mine from the events or the comment, else the one from my branch.
                let earlier = self.resumed.take().or_else(|| own_mr(&self.facts.mrs, &self.branch).cloned());
                let m = match earlier {
                    Some(m) => m,
                    None => draft_pr(project, root, issue, &self.title, &self.branch)?,
                };
                self.mr = Some(m);
                Ok(())
            }
            Step::Label if self.facts.labels.iter().any(|l| l == project.labels.in_progress) => Ok(()),
            Step::Label => edit_label(project, issue, true),
            Step::Comment if self.facts.claim_note.as_ref().is_some_and(|n| n.name == self.name && n.mr == self.mr_iid()) => Ok(()),
            Step::Comment => post_note(project, issue, &claim_comment(&self.name, self.mr_iid())),
            Step::Events => claim_events(&self.agent, issue, self.mr_iid())
                .iter()
                .try_for_each(|ev| event::run(project, root, &ev.iter().map(String::as_str).collect::<Vec<_>>())),
            Step::Race => self.race(),
            Step::FullGate => self.full_gate(),
            Step::Report => {
                print_claimed(self);
                Ok(())
            }
        }
    }

    /// The race: another agent's PR for the issue opened before mine wins (AGENTS.md §4 step 3).
    /// When mine lost, it is closed, the release is logged, and the claim fails. Then
    /// [`Claiming::comment_race`].
    fn race(&self) -> Result<(), String> {
        self.pr_race()?;
        self.comment_race()
    }

    /// Two claims that made one PR (the same branch name: the second adopted the first's) race by
    /// their claim comments, which GitHub orders: the first standing one holds the issue. Each
    /// claim looks after posting its own, so whichever comes second sees the first and releases
    /// (mornycorp/swarm-core#7). `--force`, a takeover, skips it.
    fn comment_race(&self) -> Result<(), String> {
        let (project, issue) = (self.ctx.project, self.issue);
        if self.o.force {
            return Ok(());
        }
        let read = github::comments(project, issue).and_then(|notes| Ok((notes, parse_tracking(&tracking_files(project, self.root)?))));
        let (notes, tracked) = match read {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: claim: couldn't check the claim comments for a race ({e}); `cargo xtask claim --check` does it later");
                return Ok(());
            }
        };
        let active = active_claims(&notes, &tracked, issue, now());
        let Some(first) = first_claim(&active, &self.name) else { return Ok(()) };
        Err(self.lose_to(first))
    }

    /// Release a claim that lost to `first`'s earlier one: its own PR closed (not one it shares
    /// with `first`), the label left to `first`, the release logged and commented.
    fn lose_to(&self, first: &ClaimNote) -> String {
        let (project, issue) = (self.ctx.project, self.issue);
        let why = format!("lost the claim race to {} (claimed first, {})", first.name, first.at);
        let shared = self.mr.as_ref().is_some_and(|m| first.mr == Some(m.number));
        if let Some(m) = self.mr.as_ref().filter(|_| !shared)
            && let Err(e) = close_pr(project, m.number)
        {
            eprintln!("warning: claim: closing #{}: {e}", m.number);
        }
        let issue_s = issue.to_string();
        if let Err(e) = event::run(project, self.root, &["release", "--agent", &self.agent, "--issue", &issue_s, "--reason", "duplicate", "--note", &why]) {
            eprintln!("warning: claim: logging the release: {e}");
        }
        if let Err(e) = post_note(project, issue, &release_comment(&self.name, &why)) {
            eprintln!("warning: claim: the release comment: {e}");
        }
        let kept = if shared { format!(" (its PR #{} and branch are {}'s: leave them)", self.mr_iid().unwrap_or_default(), first.name) } else { String::new() };
        format!("claim: {why}; released #{issue}{kept}; pick another issue")
    }

    /// An older open PR for the issue than mine won the race: mine is closed and released.
    fn pr_race(&self) -> Result<(), String> {
        let Some(mine) = &self.mr else { return Ok(()) };
        let project = self.ctx.project;
        let others: Vec<Pr> = match prs_for(project, self.issue) {
            Ok(mrs) => mrs.into_iter().filter(|m| m.number != mine.number).collect(),
            Err(e) => {
                eprintln!("warning: claim: couldn't check for a racing PR ({e}); `cargo xtask claim --check` does it later");
                Vec::new()
            }
        };
        let Some(w) = race_winner(self.issue, &others, mine.number) else { return Ok(()) };
        let note = format!("lost the claim race to #{} ({})", w.number, w.head_ref);
        let _ = close_pr(project, mine.number);
        let _ =
            event::run(project, self.root, &["release", "--agent", &self.agent, "--issue", &self.issue.to_string(), "--reason", "duplicate", "--note", &note]);
        Err(format!("claim: {note}: closed #{}; pick another issue (branch {} is left for you to delete)", mine.number, self.branch))
    }

    /// The periodic full gate of `main`, with the issue held; a `main-red` result releases the
    /// claim ([`after_gate`]).
    fn full_gate(&self) -> Result<(), String> {
        let project = self.ctx.project;
        let held = self.mr.as_ref().map(|m| format!(" (draft PR #{})", m.number)).unwrap_or_default();
        println!("claim: #{} is held{held}; the periodic full gate of main runs first when it is due", self.issue);
        let Some(outcome) = crate::fullgate::before_claim(&self.ctx, &self.agent) else { return Ok(()) };
        let filed = match outcome {
            crate::fullgate::Outcome::Red(n) => n,
            _ => None,
        };
        let hold = match main_red(project) {
            Ok(red) => red_hold(project, &red, self.issue, &self.facts.labels, self.o.no_mr),
            Err(e) => {
                eprintln!("warning: claim: couldn't check whether main is red after the full gate ({e}); keeping the claim");
                None
            }
        };
        match after_gate(self.issue, filed, hold) {
            Some(why) => Err(self.release(&why)),
            None => Ok(()),
        }
    }

    /// Release the claim ([`release_plan`]) because `why`; the error the claim ends with. A step
    /// that fails is a warning: the rest still run.
    fn release(&self, why: &str) -> String {
        let (project, issue) = (self.ctx.project, self.issue);
        let issue_s = issue.to_string();
        for undo in release_plan(self.mr.is_some()) {
            let done = match undo {
                Undo::ClosePr => self.mr_iid().map_or(Ok(()), |m| close_pr(project, m)),
                Undo::Label => edit_label(project, issue, false),
                Undo::Event => {
                    let reason = project.labels.main_red;
                    event::run(project, self.root, &["release", "--agent", &self.agent, "--issue", &issue_s, "--reason", reason, "--note", why])
                }
                Undo::Comment => post_note(project, issue, &release_comment(&self.name, why)),
            };
            if let Err(e) = done {
                eprintln!("warning: claim: releasing #{issue} ({undo:?}): {e}");
            }
        }
        let kept = self.mr.as_ref().map(|_| format!(" (branch {} is kept: claim it again once main is green)", self.branch)).unwrap_or_default();
        format!("claim: {why}; released #{issue}{kept}")
    }
}

/// Report the claim `c`, and switch this worktree to its branch when it has one (not with
/// `--no-switch`). Agents parse these lines: keep their wording.
fn print_claimed(c: &Claiming<'_>) {
    let (project, root, issue, name, branch) = (c.ctx.project, c.root, c.issue, &c.name, &c.branch);
    let switch_for = (!c.o.no_switch).then_some(c.agent.as_str());
    match &c.mr {
        Some(m) => {
            println!("claimed #{issue} as {name}: branch {branch}, draft PR #{} {}", m.number, m.html_url);
            report_switch(project, root, branch, switch_for);
        }
        None => println!("claimed #{issue} as {name} (label, comment, event; no branch or PR)"),
    }
}

/// Switch the worktree at `root` to `branch` for the agent `switch_for` (`None`: `--no-switch`),
/// and say where it is ([`switch_to`]). Agents parse these lines: keep their wording.
pub(crate) fn report_switch(project: &Project, root: &Path, branch: &str, switch_for: Option<&str>) {
    let hint = format!("`git switch {branch}` (or `git worktree add ../{}-{branch} {branch}`)", project.name);
    match switch_to(project, root, branch, switch_for.is_none(), switch_for.unwrap_or_default()) {
        Ok(Switch::Switched) => println!("switched this worktree to {branch}"),
        Ok(Switch::Already) => println!("this worktree is on {branch}"),
        Ok(Switch::Off) => println!("next: {hint}"),
        Ok(Switch::Primary) => println!("next: {hint}; this is the primary checkout, which claim leaves where it is (AGENTS.md §6)"),
        Ok(Switch::Dirty(files)) => println!("next: {hint}; this worktree has changes, so claim left it where it is:{files}"),
        Ok(Switch::GateHeld(why)) => println!("next: {hint}; claim left this worktree where it is: {why}"),
        Err(e) => println!("next: {hint} (claim couldn't switch: {e})"),
    }
}

/// Open the claim's draft PR from `branch`, described by the repo's PR template.
fn draft_pr(project: &Project, root: &Path, issue: u64, title: &str, branch: &str) -> Result<Pr, String> {
    let template = std::fs::read_to_string(root.join(project.pr_template)).ok().map(|t| t.replace("\r\n", "\n"));
    let body = pr_body(template.as_deref(), issue);
    match github::create_draft_pr(project, branch, project.branches.main, &format!("{title} (#{issue})"), &body) {
        Ok(v) => parse_prs(&[v]).into_iter().next().ok_or_else(|| "claim: GitHub's answer to the new PR has no number".to_owned()),
        // Refused: mine after all, or another's that won the race.
        Err(e) => after_refusal(issue, branch, &prs_for(project, issue).unwrap_or_default(), e),
    }
}

/// The events a claim logs: `claim` (with its PR), then `work`, since an agent claims what it
/// starts on at once (many claims had no `work`, so the timeline couldn't mark the start).
pub fn claim_events(agent: &str, issue: u64, mr: Option<u64>) -> [Vec<String>; 2] {
    let mut claim: Vec<String> = ["claim", "--agent", agent, "--issue", &issue.to_string()].map(str::to_owned).into();
    if let Some(m) = mr {
        claim.extend(["--mr".to_owned(), m.to_string()]);
    }
    let work = ["work", "--agent", agent, "--issue", &issue.to_string()].map(str::to_owned).into();
    [claim, work]
}

/// Comment `body` on `issue`.
pub(crate) fn post_note(project: &Project, issue: u64, body: &str) -> Result<(), String> {
    github::comment(project, issue, body)
}

/// Put the claim label on `issue` (`add`) or take it off.
pub(crate) fn edit_label(project: &Project, issue: u64, add: bool) -> Result<(), String> {
    let label = project.labels.in_progress;
    if add { github::add_label(project, None, issue, label) } else { github::remove_label(project, None, issue, label) }
}

/// Close PR `number` (a claim's draft PR that lost its race, or was released).
fn close_pr(project: &Project, number: u64) -> Result<(), String> {
    github::close_pr(project, number)
}

/// Where a claim leaves the worktree it ran in.
#[derive(Debug, PartialEq)]
pub enum Switch {
    /// It was clean and linked: now on the claim's branch.
    Switched,
    /// It was on the branch already.
    Already,
    /// `--no-switch`.
    Off,
    /// The primary checkout (the clone's own working tree): never switched (AGENTS.md §6).
    Primary,
    /// It has uncommitted changes to tracked files, listed (a few, one a line).
    Dirty(String),
    /// Another holder's live gate lock was taken in it: only
    /// `stack`, `gate` and `land` change the gate's worktree. Why, as
    /// [`crate::lock::held_elsewhere`] says.
    GateHeld(String),
}

/// Switch the worktree at `root` to `branch` when it is clean, isn't the primary checkout and isn't
/// the gate's worktree while another holder (not `me`) gates there. Untracked files don't count:
/// `git switch` keeps them, and refuses if one is in the way.
pub fn switch_to(project: &Project, root: &Path, branch: &str, no_switch: bool, me: &str) -> Result<Switch, String> {
    if no_switch {
        return Ok(Switch::Off);
    }
    if let Some(why) = crate::lock::held_elsewhere(project, root, me) {
        return Ok(Switch::GateHeld(why));
    }
    let head = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"], None, &[]).unwrap_or_default();
    if head.trim() == branch {
        return Ok(Switch::Already);
    }
    let path = |arg: &str| -> Result<PathBuf, String> {
        let p = git(root, &["rev-parse", "--path-format=absolute", arg], None, &[])?;
        let p = PathBuf::from(p.trim());
        Ok(p.canonicalize().unwrap_or(p))
    };
    if path("--git-dir")? == path("--git-common-dir")? {
        return Ok(Switch::Primary);
    }
    let status = git(root, &["status", "--porcelain", "--untracked-files=no"], None, &[])?;
    if !status.trim().is_empty() {
        let mut files: String = status.lines().take(5).map(|l| format!("\n  {}", l.trim_end())).collect();
        if status.lines().count() > 5 {
            files.push_str("\n  …");
        }
        return Ok(Switch::Dirty(files));
    }
    git(root, &["switch", "--quiet", branch], None, &[])?;
    Ok(Switch::Switched)
}

/// The refusal of a claim whose `branch` is on origin with work on it (its tip's `subject`):
/// `--force` never adopts another's work, `--take-over` does (mornycorp/swarm-core#16).
pub fn adopt_instead(branch: &str, issue: u64, subject: &str) -> String {
    format!(
        "claim: branch {branch} is already on origin with work on it (`{subject}`): another agent's? Check #{issue}. To take over its stale claim and keep that work and its PR, pass --take-over instead of --force; or pass --slug for another name"
    )
}

/// Whether a branch tip's commit `subject` is an earlier run's claim commit for `issue`
/// (`Claim #<issue>: <title>`), so the branch can be resumed.
pub fn is_claim_commit(subject: &str, issue: u64) -> bool {
    subject.strip_prefix(&format!("Claim #{issue}")).is_some_and(|rest| rest.starts_with(':'))
}

/// The claim branch on origin and here: an empty claim commit on the latest `origin/main`, so the
/// draft PR has a commit of its own (docs/swarm-protocol.md › Limits: an empty draft PR is marked merged
/// with its base). An existing branch on origin is kept only when its tip is an earlier run's
/// claim commit: one with work on it is another agent's.
fn ensure_branch(project: &Project, root: &Path, branch: &str, issue: u64, title: &str) -> Result<(), String> {
    let heads = git(root, &["ls-remote", "--heads", "origin", &format!("refs/heads/{branch}")], None, &[])?;
    if !heads.trim().is_empty() {
        // An earlier run's branch ends in its claim commit; one with work on it is another
        // agent's that took the same name (two claims raced so).
        let tip = event::fetch_tip(root, "origin", &format!("refs/heads/{branch}"))?.unwrap_or_default();
        let subject = git(root, &["log", "-1", "--format=%s", &tip], None, &[]).unwrap_or_default();
        if !is_claim_commit(subject.trim(), issue) {
            return Err(adopt_instead(branch, issue, subject.trim()));
        }
    } else {
        let base = format!("origin/{}", project.branches.main);
        let tree = git(root, &["rev-parse", &format!("{base}^{{tree}}")], None, &[])?;
        let message = format!("Claim #{issue}: {title}");
        let commit = git(root, &["commit-tree", tree.trim(), "-p", &base, "-m", &message], None, &[])?;
        // The guard would check the claim it is part of: it has just been checked.
        git(root, &["push", "--quiet", "origin", &format!("{}:refs/heads/{branch}", commit.trim())], None, &[(&check_off(project), "off")])?;
    }
    event::fetch(root, "origin", &format!("refs/heads/{branch}"), &format!("refs/remotes/origin/{branch}"))?;
    if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")], None, &[]).is_err() {
        git(root, &["branch", "--quiet", "--track", branch, &format!("origin/{branch}")], None, &[])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::APP_SWARM as P;

    const LABEL: &str = "status::in-progress";
    const BLOCKED: &str = "status::blocked";
    const MAIN_RED: &str = "main-red";
    use serde_json::json;

    /// 2026-10-18T07:07:32Z: the moment the coordinator claimed R10a.
    const COORDINATOR_CLAIMED: &str = "2026-10-18T07:07:32Z";

    fn t(s: &str) -> i64 {
        parse_time(s).unwrap()
    }

    /// The tracking branch as it was on 2026-10-18 for #253: claude-mac claimed it at 06:49.
    fn r10a_tracking() -> Vec<(String, String)> {
        vec![
            (
                "mxv9wfm7rr-ed6886".into(),
                [
                    r#"{"event":"session.start","machine":"mxv9wfm7rr","name":"claude-mac","role":"worker","t":"2026-10-17T08:16:39Z"}"#,
                    r#"{"event":"claim","issue":253,"mr":246,"t":"2026-10-18T06:49:51Z"}"#,
                    r#"{"event":"work","issue":253,"t":"2026-10-18T06:49:55Z"}"#,
                ]
                .join("\n"),
            ),
            (
                "cmaughan-dt9-719698".into(),
                r#"{"event":"session.start","machine":"cmaughan-dt9","name":"coordinator","role":"coordinator","t":"2026-10-17T06:10:51Z"}"#.into(),
            ),
        ]
    }

    fn mr(number: u64, branch: &str, title: &str, body: &str) -> Pr {
        Pr {
            number,
            state: "open".into(),
            draft: true,
            head_ref: branch.into(),
            title: title.into(),
            body: body.into(),
            html_url: format!("https://github.com/example/gpu-swarm/pull/{number}"),
        }
    }

    fn coordinator() -> Me {
        Me { instance: Some("cmaughan-dt9-719698".into()), name: Some("coordinator".into()), branch: Some("253-r10a-native".into()) }
    }

    /// The R10a duplicate. At the moment the coordinator claimed #253, the label, claude-mac's
    /// draft PR and its tracking claim all said someone held it; the claim is refused naming all
    /// three.
    #[test]
    fn the_r10a_claim_is_refused_naming_who_holds_it() {
        let facts = Facts {
            labels: vec![LABEL.into(), "area::replay".into()],
            mrs: vec![mr(246, "253-d3d12-replay-foundations", "R10a: D3D12 replay foundations (#253)", "Closes #253")],
            claim_note: Some(ClaimNote { name: "claude-mac".into(), mr: Some(246), at: "2026-10-18T06:50:02Z".into() }),
            claims: vec![],
            tracked: parse_tracking(&r10a_tracking()),
            remark: None,
            description: String::new(),
        };
        let now = t(COORDINATOR_CLAIMED);
        let held = holders(&P, 253, &facts, &coordinator(), now, true);
        assert_eq!(held.len(), 3, "{held:?}");
        let lines: Vec<String> = held.iter().map(|h| describe(LABEL, h, now)).collect();
        assert_eq!(lines[0], "labelled status::in-progress: claimed by claude-mac, PR #246 at 2026-10-18T06:50:02Z");
        assert!(lines[1].starts_with("open draft PR #246 `R10a: D3D12 replay foundations (#253)` (branch 253-d3d12-replay-foundations)"), "{}", lines[1]);
        assert_eq!(lines[2], "claude-mac (mxv9wfm7rr-ed6886): `work` at 2026-10-18T06:49:55Z, PR #246, 17 min ago, not stopped or released");
        // Any one of them is enough: the tracking claim alone (the label not set yet, no PR).
        let only_tracking = Facts { labels: vec![], mrs: vec![], claim_note: None, ..facts.clone() };
        assert_eq!(holders(&P, 253, &only_tracking, &coordinator(), now, true).len(), 1);
        // And the same view for claude-mac itself: nobody else holds it.
        let mac = Me { instance: Some("mxv9wfm7rr-ed6886".into()), name: Some("claude-mac".into()), branch: Some("253-d3d12-replay-foundations".into()) };
        assert_eq!(holders(&P, 253, &facts, &mac, now, true), vec![]);
        // A later session of the same agent (a new instance, the same name) resumes its claim.
        let mac_again = Me { instance: Some("mxv9wfm7rr-f00000".into()), branch: None, ..mac };
        assert_eq!(holders(&P, 253, &facts, &mac_again, now, true), vec![]);
        assert_eq!(my_mrs(253, &facts, &mac_again, now), BTreeSet::from([246]));
    }

    /// A free issue: nothing holds it, so the claim goes ahead.
    #[test]
    fn an_unclaimed_issue_is_free() {
        let facts = Facts { tracked: parse_tracking(&r10a_tracking()), ..Facts::default() };
        assert!(holders(&P, 271, &facts, &coordinator(), t(COORDINATOR_CLAIMED), true).is_empty());
    }

    /// A claim lives 24 hours unless its agent stops or releases it, the coordinator releases it
    /// (`of`), or the issue is merged.
    #[test]
    fn a_tracking_claim_ends_with_stop_release_merge_or_24_hours() {
        let claim = r#"{"event":"claim","issue":253,"mr":246,"t":"2026-10-18T06:49:51Z"}"#;
        let start = r#"{"event":"session.start","name":"claude-mac","role":"worker","t":"2026-10-17T08:16:39Z"}"#;
        let with = |more: &[&str]| parse_tracking(&[("mac-1".into(), [start, claim].iter().chain(more).copied().collect::<Vec<_>>().join("\n"))]);
        let now = t("2026-10-18T08:00:00Z");
        assert_eq!(live_claims(&with(&[]), 253, now).len(), 1);
        assert_eq!(live_claims(&with(&[]), 253, now)[0].name.as_deref(), Some("claude-mac"));
        assert!(live_claims(&with(&[]), 253, t("2026-10-19T06:49:52Z")).is_empty(), "24 hours on");
        assert!(live_claims(&with(&[r#"{"event":"stop","issue":253,"reason":"done","t":"2026-10-18T07:16:53Z"}"#]), 253, now).is_empty());
        assert!(live_claims(&with(&[r#"{"event":"release","issue":253,"t":"2026-10-18T07:00:00Z"}"#]), 253, now).is_empty());
        // `submit` doesn't end it: the PR is still to merge.
        assert_eq!(live_claims(&with(&[r#"{"event":"submit","issue":253,"mr":246,"t":"2026-10-18T07:16:48Z"}"#]), 253, now).len(), 1);
        // Work again after a pause holds it again.
        let resumed = with(&[
            r#"{"event":"stop","issue":253,"reason":"paused","t":"2026-10-18T07:00:00Z"}"#,
            r#"{"event":"work","issue":253,"t":"2026-10-18T07:30:00Z"}"#,
        ]);
        let live = live_claims(&resumed, 253, now);
        assert_eq!((live.len(), live[0].event.as_str(), live[0].mr), (1, "work", Some(246)));
        // Another instance: the coordinator's merge, or its release `of` this one.
        let mut events = with(&[]);
        let coordinator = |line: &str| parse_tracking(&[("coord-1".into(), line.into())]);
        events.extend(coordinator(r#"{"event":"merge","issue":253,"mr":246,"t":"2026-10-18T07:42:29Z"}"#));
        assert!(live_claims(&events, 253, now).is_empty());
        let mut events = with(&[]);
        events.extend(coordinator(r#"{"event":"release","issue":253,"reason":"stale","of":"mac-1","t":"2026-10-18T07:40:00Z"}"#));
        assert!(live_claims(&events, 253, now).is_empty());
        // A release of someone else's claim leaves this one live.
        let mut events = with(&[]);
        events.extend(coordinator(r#"{"event":"release","issue":253,"reason":"stale","of":"other-1","t":"2026-10-18T07:40:00Z"}"#));
        assert_eq!(live_claims(&events, 253, now).len(), 1);
        // Other issues and junk lines don't count.
        let junk = parse_tracking(&[(
            "x-1".into(),
            "not json\n{\"event\":\"claim\",\"issue\":9}\n{\"event\":\"claim\",\"issue\":7,\"t\":\"2026-10-18T07:00:00Z\"}".into(),
        )]);
        assert_eq!(junk.len(), 1);
        assert!(live_claims(&junk, 253, now).is_empty());
    }

    /// A PR that merely mentions the issue (#271's own description named !246 and !247) doesn't
    /// hold it: only an open PR that is for the issue does.
    #[test]
    fn only_open_mrs_for_the_issue_hold_it() {
        assert!(mr(1, "253-x", "t", "").is_for(253));
        assert!(mr(1, "feature", "R10a part 1 (#253)", "Part of #253").is_for(253));
        assert!(mr(1, "feature", "t", "Some text.\n\nCloses #253").is_for(253));
        assert!(mr(1, "feature", "t", "fixes #253, and more").is_for(253));
        assert!(!mr(1, "feature", "t", "Part of #253; see !246").is_for(253), "a mention isn't a claim");
        assert!(!mr(1, "2530-x", "(#2530)", "Closes #2530").is_for(253));
        assert!(!mr(1, "feature", "t", "encloses #253").is_for(253));
        let facts = Facts {
            mrs: vec![
                Pr { state: "merged".into(), ..mr(246, "253-a", "R10a (#253)", "Closes #253") },
                Pr { state: "closed".into(), ..mr(247, "253-b", "(#253)", "") },
            ],
            ..Facts::default()
        };
        assert!(holders(&P, 253, &facts, &Me::default(), 0, true).is_empty());
    }

    #[test]
    fn mentions_need_the_whole_number() {
        assert!(mentions("Closes #271.", 271));
        assert!(mentions("(#271)", 271));
        assert!(!mentions("example/gpu-swarm#271", 271), "another repository's issue");
        assert!(!mentions("ns#271", 271), "a short name's");
        assert!(!mentions("#2710", 271));
        assert!(!mentions("&#271;", 271));
        assert!(!mentions("271", 271));
        assert!(closes("Closes: #271", 271));
        assert!(closes("CLOSES #271", 271));
        assert!(!closes("Closes #2710", 271));
    }

    #[test]
    fn branches_name_their_issue() {
        assert_eq!(issue_of_branch("271-xtask-claim"), Some(271));
        assert_eq!(issue_of_branch("253-r10a-native"), Some(253));
        for none in ["main", "claude/intelligent-goodall", "271", "271-", "-271-x", "v2-thing", "tracking", "12a-x"] {
            assert_eq!(issue_of_branch(none), None, "{none}");
        }
    }

    /// The label: on its own (no comment), or a comment naming someone else, holds the issue for a
    /// claim; the guard only counts a comment naming someone else, since it can't tell a claim with
    /// no comment from its own.
    #[test]
    fn the_label_counts_unless_the_claim_comment_is_mine() {
        let me = Me { instance: Some("pc-1".into()), name: Some("worker-a".into()), branch: Some("9-x".into()) };
        let labelled = Facts { labels: vec![LABEL.into()], ..Facts::default() };
        assert_eq!(holders(&P, 9, &labelled, &me, 0, true), vec![Holder::Label(None)]);
        assert!(holders(&P, 9, &labelled, &me, 0, false).is_empty());
        let coordinator = ClaimNote { name: "coordinator".into(), mr: None, at: "2026-10-18T09:00:00Z".into() };
        let theirs = Facts { claim_note: Some(coordinator.clone()), ..labelled.clone() };
        assert_eq!(holders(&P, 9, &theirs, &me, 0, false), vec![Holder::Label(Some(coordinator))]);
        let mine = Facts { claim_note: Some(ClaimNote { name: "worker-a".into(), mr: Some(5), at: String::new() }), ..labelled.clone() };
        assert!(holders(&P, 9, &mine, &me, 0, true).is_empty());
        // Without my name (the guard run with no --agent): the comment's PR is my branch's.
        let anon = Me { instance: None, name: None, branch: Some("9-x".into()) };
        let mine_by_mr = Facts { mrs: vec![mr(5, "9-x", "t", "Closes #9")], ..mine };
        assert!(holders(&P, 9, &mine_by_mr, &anon, 0, false).is_empty());
        assert!(describe(LABEL, &Holder::Label(None), 0).contains("no claim comment"));
    }

    /// The coordinator's comment on #255 names its instance after its name. The name is the
    /// first word, so the coordinator still owns its label once its tracking claim has ended.
    #[test]
    fn a_claim_comment_name_is_its_first_word() {
        assert_eq!(parse_claim_note("claimed: coordinator (cmaughan-dt9-719698), PR #268"), Some(("coordinator".into(), Some(268))));
        assert_eq!(parse_claim_note("claimed: claude-mac, PR #270 (stacked on #269, #262)"), Some(("claude-mac".into(), Some(270))));
        let me = Me { instance: Some("cmaughan-dt9-719698".into()), name: Some("coordinator".into()), branch: None };
        let notes = [json!({"body": "claimed: coordinator (cmaughan-dt9-719698), PR #268", "created_at": "2026-10-18T03:46:04.237-07:00"})];
        // No tracking claim left to name #268 as mine: only the comment's name says it is.
        let facts = Facts { labels: vec![LABEL.into()], claim_note: latest_claim(&notes), ..Facts::default() };
        assert_eq!(holders(&P, 255, &facts, &me, t("2026-10-19T12:00:00Z"), true), vec![]);
    }

    /// The coordinator claimed #254 at 08:07Z and logged its own merge of !266 at 11:00Z.
    /// A merge ends the claim whoever logs it; releasing another agent's work (`of`) doesn't end
    /// the logger's own.
    #[test]
    fn a_merge_ends_a_claim_even_when_its_own_agent_logs_it() {
        let file = |more: &str| {
            let lines = [
                r#"{"event":"session.start","name":"coordinator","role":"coordinator","t":"2026-10-17T06:10:51Z"}"#,
                r#"{"event":"claim","issue":254,"mr":255,"t":"2026-10-18T08:07:07Z"}"#,
                more,
            ];
            parse_tracking(&[("cmaughan-dt9-719698".into(), lines.join("\n"))])
        };
        let now = t("2026-10-18T12:35:00Z");
        assert!(live_claims(&file(r#"{"event":"merge","issue":254,"mr":266,"t":"2026-10-18T11:00:05Z"}"#), 254, now).is_empty());
        let released_other = file(r#"{"event":"release","issue":254,"reason":"stale","of":"mac-1","t":"2026-10-18T09:00:00Z"}"#);
        assert_eq!(live_claims(&released_other, 254, now).len(), 1, "releasing another's work keeps its own claim");
        assert!(live_claims(&file(r#"{"event":"release","issue":254,"of":"cmaughan-dt9-719698","t":"2026-10-18T09:00:00Z"}"#), 254, now).is_empty());
    }

    #[test]
    fn claim_comments_are_parsed_and_the_newest_wins() {
        assert_eq!(parse_claim_note("claimed: claude-mac, PR #246"), Some(("claude-mac".into(), Some(246))));
        assert_eq!(parse_claim_note("claimed: coordinator"), Some(("coordinator".into(), None)));
        assert_eq!(parse_claim_note("claimed: coordinator (taking over)"), Some(("coordinator".into(), None)));
        assert_eq!(parse_claim_note("  claimed: w, PR #12.\nmore"), Some(("w".into(), Some(12))));
        assert_eq!(parse_claim_note("claimed:"), None);
        assert_eq!(parse_claim_note("I claimed: x"), None);
        let notes = [
            json!({"body": "claimed: a, PR #1", "created_at": "2026-10-18T06:00:00Z"}),
            json!({"body": "unrelated", "created_at": "2026-10-18T08:00:00Z"}),
            json!({"body": "claimed: b, PR #2", "created_at": "2026-10-18T07:00:00Z"}),
        ];
        assert_eq!(latest_claim(&notes).map(|n| (n.name, n.mr)), Some(("b".into(), Some(2))));
        assert_eq!(latest_claim(&[json!({"message": "404"})]), None);
        assert_eq!(claim_comment("w", Some(263)), "claimed: w, PR #263");
        assert_eq!(claim_comment("coordinator", None), "claimed: coordinator");
    }

    #[test]
    fn prs_parse_from_githubs_shape() {
        let v = [
            json!({"number": 246, "state": "open", "draft": true, "head_ref": "253-a", "title": "T", "body": null, "html_url": "u"}),
            json!({"state": "open"}),
        ];
        assert_eq!(
            parse_prs(&v),
            vec![Pr { number: 246, state: "open".into(), draft: true, head_ref: "253-a".into(), title: "T".into(), body: String::new(), html_url: "u".into() }]
        );
        assert!(parse_prs(&[json!({"message": "Bad credentials"})]).is_empty());
    }

    /// On 2026-10-21 another swarm's twin PR named this repo's issue in its title, and held it. Only a PR whose branch, title (`#190` alone) or
    /// closing line is for the issue holds it: a mention of another repo's issue isn't.
    #[test]
    fn another_repos_issue_named_in_a_title_holds_nothing() {
        let twin = mr(203, "386-claim-project-prs", "[sync] claim: count only this repo's PRs (#386)", "Closes #386");
        assert!(!twin.is_for(190), "example/app-swarm#190 is not #190");
        assert_eq!(race_winner(190, std::slice::from_ref(&twin), 207), None);
        let ours = mr(207, "190-claim-project-prs", "claim: count only this repo's PRs (#190)", "Closes #190");
        assert!(ours.is_for(190));
    }

    /// The race window that remains between the checks and the PR: the older PR wins.
    #[test]
    fn the_older_mr_wins_a_race() {
        let mrs = vec![mr(250, "9-a", "(#9)", ""), mr(248, "9-b", "(#9)", ""), mr(260, "10-c", "(#10)", "")];
        assert_eq!(race_winner(9, &mrs, 255).map(|m| m.number), Some(248));
        assert_eq!(race_winner(9, &mrs, 247), None);
        assert_eq!(race_winner(10, &mrs, 255), None);
    }

    /// A re-run keeps the PR an earlier run opened from the claim's own branch, though no
    /// comment or event says it is mine; another branch's PR, or a closed one, isn't kept.
    #[test]
    fn a_rerun_keeps_its_own_earlier_mr() {
        let branch = "283-source-names-and-search";
        let mine = mr(307, branch, "Draft: Source map names (#283)", "Closes #283");
        let other = mr(306, "283-someone-else", "Draft: the same (#283)", "Closes #283");
        let closed = Pr { state: "closed".into(), ..mr(305, branch, "(#283)", "") };
        assert_eq!(own_mr(&[other.clone(), mine.clone()], branch).map(|m| m.number), Some(307));
        assert_eq!(own_mr(std::slice::from_ref(&other), branch), None, "another branch's");
        assert_eq!(own_mr(std::slice::from_ref(&closed), branch), None, "a closed one");
    }

    /// GitHub refusing the claim's PR: a PR from my branch is mine (an earlier run's, seen
    /// only now); another branch's open PR for the issue won the race; else GitHub's error.
    #[test]
    fn a_refused_mr_is_mine_a_rivals_or_the_error() {
        let branch = "283-source-names-and-search";
        let refused = "gh api repos/example/app-swarm/pulls: A pull request already exists for cmaughan:283-source-names-and-search. (HTTP 422)".to_owned();
        let mine = mr(307, branch, "Draft: Source map names (#283)", "Closes #283");
        assert_eq!(after_refusal(283, branch, std::slice::from_ref(&mine), refused.clone()).map(|m| m.number), Ok(307), "not a lost race to myself");
        let rival = mr(301, "283-other", "Draft: the same (#283)", "Closes #283");
        let lost = after_refusal(283, branch, std::slice::from_ref(&rival), refused.clone()).unwrap_err();
        assert!(lost.contains("lost the claim race to #301 (283-other)"), "{lost}");
        assert_eq!(after_refusal(283, branch, &[], refused.clone()), Err(refused), "nothing to blame: GitHub's error");
    }

    /// GitHub refused the claim's PR because one for the issue exists by now: a lost
    /// race, named as such, to the oldest open one; any other refusal keeps GitHub's message.
    #[test]
    fn a_refused_claim_mr_names_the_race_it_lost() {
        let refused = "gh api repos/example/gpu-swarm/pulls: Validation Failed (HTTP 422)".to_owned();
        let mrs = [
            mr(343, "348-mine", "Draft: claim (#348)", "Closes #348"),
            Pr { state: "closed".into(), ..mr(300, "348-old", "(#348)", "") },
            mr(299, "12-other", "Something else (#12)", "Closes #12"),
            mr(344, "348-later", "Draft: claim again (#348)", "Closes #348"),
        ];
        let e = lost_race(348, &mrs, refused.clone());
        assert!(e.starts_with("claim: lost the claim race to #343 (348-mine); pick another issue") && e.contains("422"), "{e}");
        assert_eq!(lost_race(348, &mrs[1..3], refused.clone()), refused, "no open PR for the issue: GitHub's own error");
    }

    /// An issue that names only itself needs nothing, and nothing is asked of GitHub.
    #[test]
    fn an_issue_never_needs_itself() {
        let open = [mr(4, "3-phase2", "Phase 2", "")];
        let lines = open_needs(&P, 300, "Needs #300 itself, and nothing else.", &open);
        assert!(lines.is_empty(), "an issue never needs itself: {lines:?}");
        assert!(open_needs(&P, 300, "Needs example/app-swarm#300.", &open).is_empty(), "its own path is itself");
    }

    #[test]
    fn slugs_and_descriptions() {
        assert_eq!(slugify("xtask claim: atomic claims and a push-time guard"), "xtask-claim-atomic-claims-and-a");
        assert_eq!(slugify("R10a: D3D12 replay foundations"), "r10a-d3d12-replay-foundations");
        assert_eq!(slugify("!!!"), "work");
        assert_eq!(slugify(&"a".repeat(100)).len(), 32);
        assert!(check_slug("d3d12-replay").is_ok());
        for bad in ["", "a b", "-x", "x/y", "ü"] {
            assert!(check_slug(bad).is_err(), "{bad}");
        }
        let template = "## What\n\nClose #<!-- issue -->\n\nCloses #<!-- issue number; the merge closes it -->\n\n## Progress\n";
        assert_eq!(pr_body(Some(template), 271), "## What\n\nClose #<!-- issue -->\n\nCloses #271\n\n## Progress\n");
        assert_eq!(pr_body(Some("## What\n"), 271), "Closes #271\n\n## What\n");
        assert_eq!(pr_body(None, 271), "Closes #271\n");
    }

    #[test]
    fn tracking_files_are_read_from_git_output() {
        let listing = "100644 blob aaaa\ttracking/mac-1/work.jsonl\n100644 blob bbbb\ttracking/../x/work.jsonl\n100644 blob cccc\ttracking/pc-2/work.jsonl\n100644 blob dddd\tREADME.md\n";
        let entries = parse_ls_tree(listing);
        assert_eq!(entries, vec![("mac-1".into(), "aaaa".into()), ("pc-2".into(), "cccc".into())]);
        let batch = "aaaa blob 6\nline1\n\ncccc blob 3\nÿ\n\n";
        assert_eq!(parse_batch(batch, &entries), vec![("mac-1".into(), "line1\n".into()), ("pc-2".into(), "ÿ\n".into())]);
        // A truncated answer keeps what it has.
        assert_eq!(parse_batch("aaaa blob 600\nshort", &entries), vec![]);
        assert_eq!(parse_batch("", &entries), vec![]);
    }

    /// Through git: events `cargo xtask event` pushed to a remote's tracking branch are read back
    /// and decide the claim.
    #[test]
    fn the_tracking_branch_is_read_through_git() {
        let dir = std::env::temp_dir().join(format!("swarm-xtask-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let remote = dir.join("remote.git");
        git(&dir, &["init", "--quiet", "--bare", remote.to_str().unwrap()], None, &[]).unwrap();
        let clone = dir.join("clone");
        git(&dir, &["init", "--quiet", clone.to_str().unwrap()], None, &[]).unwrap();
        git(&clone, &["remote", "add", "origin", remote.to_str().unwrap()], None, &[]).unwrap();
        let queue = dir.join("queue.jsonl");
        // No tracking branch yet: nobody has logged anything.
        assert_eq!(tracking_files(&P, &clone), Ok(Vec::new()));
        let events: Vec<(String, String)> =
            r10a_tracking().into_iter().flat_map(|(a, text)| text.lines().map(|l| (a.clone(), l.to_owned())).collect::<Vec<_>>()).collect();
        assert_eq!(event::send(&P, &clone, &queue, &events).unwrap(), 4);
        let files = tracking_files(&P, &clone).unwrap();
        assert_eq!(files.len(), 2, "{files:?}");
        let tracked = parse_tracking(&files);
        let facts = Facts { tracked, ..Facts::default() };
        let held = holders(&P, 253, &facts, &coordinator(), t(COORDINATOR_CLAIMED), true);
        assert!(matches!(held.as_slice(), [Holder::Tracking(c)] if c.name.as_deref() == Some("claude-mac") && c.mr == Some(246)), "{held:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A claim logs `claim` (with its PR, if any), then `work`.
    #[test]
    fn a_claim_logs_its_claim_then_work() {
        let [claim, work] = claim_events("mxv9wfm7rr-822ee2", 114, Some(120));
        assert_eq!(claim, ["claim", "--agent", "mxv9wfm7rr-822ee2", "--issue", "114", "--mr", "120"]);
        assert_eq!(work, ["work", "--agent", "mxv9wfm7rr-822ee2", "--issue", "114"]);
        let [claim, _] = claim_events("cmaughan-dt9-719698", 110, None);
        assert_eq!(claim, ["claim", "--agent", "cmaughan-dt9-719698", "--issue", "110"], "a --no-mr claim names no PR");
        // Each is an event the writer takes.
        for ev in claim_events("pc-1", 5, Some(6)) {
            let ev: Vec<&str> = ev.iter().map(String::as_str).collect();
            assert!(event::parse(&P, &ev, None, || "pc".into()).is_ok_and(|e| e.is_some()), "{ev:?}");
        }
    }

    /// While `main` is red, a claim waits, naming the red issues; the coordinator's own
    /// claims and a claim of the red issue itself go ahead.
    #[test]
    fn a_red_main_holds_claims() {
        let red = parse_issues(&[json!({"number": 140, "title": "main is red: clippy on Linux", "state": "open"}), json!({"title": "no number"})]);
        assert_eq!(red, [(140, "main is red: clippy on Linux".to_owned())]);
        let why = red_hold(&P, &red, 133, &["type::task".into()], false).unwrap();
        assert!(why.contains("#140 main is red: clippy on Linux") && why.contains("step 3"), "{why}");
        assert_eq!(red_hold(&P, &red, 140, &[], false), None, "the red issue itself: its fix");
        assert_eq!(red_hold(&P, &red, 141, &[MAIN_RED.into()], false), None, "another main-red issue");
        assert_eq!(red_hold(&P, &red, 133, &[], true), None, "the coordinator's own claim starts nothing from main");
        assert_eq!(red_hold(&P, &[], 133, &[], false), None, "green: go ahead");
        // The shared check: `land` asks it too.
        assert!(fixes_red(&red, &[133, 140]) && !fixes_red(&red, &[133]) && !fixes_red(&[], &[140]));
        assert_eq!(main_is_red(&red, true, "rule"), None, "exempt");
        assert_eq!(main_is_red(&[], false, "rule"), None, "green");
        assert_eq!(main_is_red(&red, false, "rule").as_deref(), Some("main is red:\n  - #140 main is red: clippy on Linux\nrule"));
        assert!(parse_issues(&[json!({"message": "401"})]).is_empty());
    }

    #[test]
    fn arguments_are_checked() {
        assert_eq!(parse_args(&P, &["#271", "--agent", "pc-1", "--slug", "x"]).map(|o| (o.issue, o.slug)), Ok((Some(271), Some("x".into()))));
        assert!(parse_args(&P, &["abc"]).is_err());
        assert!(parse_args(&P, &["1", "2"]).is_err());
        assert!(parse_args(&P, &["1", "--agent", "../x"]).is_err());
        assert!(parse_args(&P, &["1", "--slug", "a b"]).is_err());
        assert!(parse_args(&P, &["1", "--agent"]).is_err());
        let root = Path::new(".");
        assert!(run(&P, &crate::Defaults, root, &["--check", "5"]).unwrap_err().contains("takes no issue"));
        assert!(run(&P, &crate::Defaults, root, &["5", "--strict"]).unwrap_err().contains("go with --check"));
    }

    #[test]
    fn only_a_claim_commit_is_resumed() {
        assert!(is_claim_commit("Claim #104: View > Auto Refresh with no swarm open", 104));
        assert!(!is_claim_commit("Claim #1040: another issue", 104), "not a prefix of another number");
        assert!(!is_claim_commit("View > Auto Refresh with no swarm says no swarm is open (#104)", 104), "work on it");
        assert!(!is_claim_commit("Claim #104", 104));
        assert!(!is_claim_commit("", 104));
    }

    /// A claim switches the clean worktree it ran in to its branch; a worktree with changes,
    /// the primary checkout and `--no-switch` are left where they are.
    #[test]
    fn a_claim_switches_a_clean_linked_worktree_only() {
        use swarm_core_testkit::git::git as tgit;
        let dir = swarm_core_testkit::temp_dir("xtask-claim-switch");
        let primary = dir.join("primary");
        std::fs::create_dir_all(&primary).unwrap();
        tgit(&primary, &["init", "--quiet"]);
        std::fs::write(primary.join("a.txt"), "a\n").unwrap();
        tgit(&primary, &["add", "a.txt"]);
        tgit(&primary, &["commit", "--quiet", "-m", "first"]);
        tgit(&primary, &["branch", "113-claim-switches"]);
        tgit(&primary, &["branch", "114-other"]);
        let linked = dir.join("linked");
        tgit(&primary, &["worktree", "add", "--quiet", "--detach", linked.to_str().unwrap()]);
        let head = |p: &Path| tgit(p, &["rev-parse", "--abbrev-ref", "HEAD"]).trim().to_owned();

        // The primary checkout stays on main, clean or not.
        assert_eq!(switch_to(&P, &primary, "114-other", false, "me"), Ok(Switch::Primary));
        assert_eq!(head(&primary), "main");
        // --no-switch: left detached.
        assert_eq!(switch_to(&P, &linked, "113-claim-switches", true, "me"), Ok(Switch::Off));
        assert_eq!(head(&linked), "HEAD");
        // A change to a tracked file: left where it is, naming the file.
        std::fs::write(linked.join("a.txt"), "changed\n").unwrap();
        assert!(matches!(switch_to(&P, &linked, "113-claim-switches", false, "me"), Ok(Switch::Dirty(f)) if f.contains("a.txt")));
        assert_eq!(head(&linked), "HEAD");
        tgit(&linked, &["checkout", "--quiet", "--", "a.txt"]);
        // Clean (an untracked file doesn't count): switched, and a second claim run says so.
        std::fs::write(linked.join("notes.txt"), "mine\n").unwrap();
        assert_eq!(switch_to(&P, &linked, "113-claim-switches", false, "me"), Ok(Switch::Switched));
        assert_eq!(head(&linked), "113-claim-switches");
        assert!(linked.join("notes.txt").exists(), "untracked files stay");
        assert_eq!(switch_to(&P, &linked, "113-claim-switches", false, "me"), Ok(Switch::Already));
        // A branch that doesn't exist is an error, not a panic; the worktree stays.
        assert!(switch_to(&P, &linked, "999-missing", false, "me").is_err());
        assert_eq!(head(&linked), "113-claim-switches");
        // Another agent's gate lock, taken in this worktree: left where it is, saying why;
        // the holder itself may switch it, and once the lock is dropped anyone may.
        crate::lock::take(&P, &linked, "sched-1").unwrap();
        assert!(matches!(switch_to(&P, &linked, "114-other", false, "me"), Ok(Switch::GateHeld(why)) if why.contains("held by sched-1")));
        assert_eq!(head(&linked), "113-claim-switches");
        assert!(matches!(switch_to(&P, &primary, "114-other", false, "me"), Ok(Switch::Primary)), "another worktree isn't held");
        assert_eq!(switch_to(&P, &linked, "114-other", false, "sched-1"), Ok(Switch::Switched));
        crate::lock::drop(&P, &linked, "sched-1", false).unwrap();
        assert_eq!(switch_to(&P, &linked, "113-claim-switches", false, "me"), Ok(Switch::Switched));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_switch_is_an_argument_of_a_claim_not_a_check() {
        assert!(parse_args(&P, &["5", "--no-switch"]).is_ok_and(|o| o.no_switch));
        assert!(run(&P, &crate::Defaults, Path::new("."), &["--check", "--no-switch"]).unwrap_err().contains("--no-switch"));
        assert!(parse_args(&P, &["5", "--no-fullgate"]).is_ok_and(|o| o.no_fullgate), "#196");
        assert!(run(&P, &crate::Defaults, Path::new("."), &["--check", "--no-fullgate"]).unwrap_err().contains("--no-fullgate"));
    }

    #[test]
    fn a_blocked_issue_is_held_with_what_its_newest_remark_says() {
        let notes = [
            json!({"body": "On hold (the owner): start nothing new.", "created_at": "2026-10-20T06:29:43Z"}),
            json!({"body": "claimed: rowan, PR #180", "created_at": "2026-10-20T06:38:22Z"}),
            json!({"body": "Earlier remark\nsecond line", "created_at": "2026-10-19T10:00:00Z"}),
        ];
        assert_eq!(latest_remark(&notes).as_deref(), Some("On hold (the owner): start nothing new."));
        let held = Facts { labels: vec![BLOCKED.into(), "area::ci".into()], remark: latest_remark(&notes), ..Facts::default() };
        assert_eq!(blocked_hold(&P, 166, &held).as_deref(), Some("#166 is on hold (status::blocked): On hold (the owner): start nothing new."));
        let silent = Facts { labels: vec![BLOCKED.into()], ..Facts::default() };
        assert_eq!(blocked_hold(&P, 7, &silent).as_deref(), Some("#7 is on hold (status::blocked); no comment says why"));
        let free = Facts { labels: vec![LABEL.into()], remark: Some("why".into()), ..Facts::default() };
        assert_eq!(blocked_hold(&P, 7, &free), None, "only status::blocked holds an issue");
    }

    #[test]
    fn a_remark_is_one_capped_line_and_hostile_notes_are_skipped() {
        let long = "x".repeat(500);
        let notes = [
            json!({"body": long, "created_at": "2026-10-20T00:00:00Z"}),
            json!({"body": 5}),
            json!({"created_at": "z"}),
            json!("not a note"),
            json!({"body": "  "}),
        ];
        assert_eq!(latest_remark(&notes).map(|r| r.chars().count()), Some(200));
        assert_eq!(latest_remark(&[json!({"not": "a comment"})]), None);
    }

    /// The issue is held (branch, PR, label, comment, events) and the race checked
    /// before the periodic full gate runs; the report and the switch come last. On 2026-10-22
    /// an issue was claimed twice while a claim ran its gate first.
    #[test]
    fn a_claim_holds_its_issue_before_the_full_gate_runs() {
        use Step::*;
        assert_eq!(plan(false, false), [Branch, Pr, Label, Comment, Events, Race, FullGate, Report]);
        assert_eq!(plan(false, true), [Branch, Pr, Label, Comment, Events, Race, Report], "--no-fullgate");
        assert_eq!(plan(true, false), [Label, Comment, Events, FullGate, Report], "--no-mr: no branch, PR or race");
        assert_eq!(plan(true, true), [Label, Comment, Events, Report]);
        for (no_mr, no_fullgate) in [(false, false), (true, false)] {
            let steps = plan(no_mr, no_fullgate);
            let at = |s: Step| steps.iter().position(|x| *x == s);
            let gate = at(FullGate).unwrap();
            for held in [Branch, Pr, Label, Comment, Events, Race] {
                assert!(at(held).is_none_or(|i| i < gate), "{held:?} before the gate");
            }
            assert_eq!(steps.last(), Some(&Report), "the switch after a gate that kept the claim");
        }
    }

    /// A full gate that finds main red releases the task's claim: its draft PR
    /// closed, the label off, the `release --reason main-red` event, a comment that isn't a claim
    /// comment.
    #[test]
    fn a_red_full_gate_releases_the_claim() {
        assert_eq!(release_plan(true), [Undo::ClosePr, Undo::Label, Undo::Event, Undo::Comment]);
        assert_eq!(release_plan(false), [Undo::Label, Undo::Event, Undo::Comment], "--no-mr has no PR to close");
        // The gate filed and claimed main-red #225: work on that instead.
        let why = after_gate(219, Some(225), red_hold(&P, &[(225, "main is red".into())], 219, &[], false)).unwrap();
        assert!(why.contains("filed main-red #225 and claimed it") && why.contains("instead of #219"), "{why}");
        assert!(after_gate(219, Some(225), None).is_some(), "filed, though main-red couldn't be read back");
        // Another agent's main-red, open after the gate: held as a claim after the gate would be.
        let hold = red_hold(&P, &[(226, "main is red: clippy".into())], 219, &[], false);
        assert!(after_gate(219, None, hold).unwrap().contains("#226 main is red: clippy"));
        // A green main, or a main-red issue's own claim (the fix), keeps the claim.
        assert_eq!(after_gate(219, None, red_hold(&P, &[], 219, &[], false)), None);
        assert_eq!(after_gate(226, None, red_hold(&P, &[(226, "main is red".into())], 226, &[], false)), None);
        let comment = release_comment("ivy", &why);
        assert!(comment.starts_with("released: ivy, the full gate"), "{comment}");
        assert_eq!(parse_claim_note(&comment), None, "a release is no claim");
    }

    // ---- mornycorp/swarm-core#7: claims that raced onto one branch and PR. ----

    /// An agent's tracking file: its session, then `(event, mr, time)` on `issue`.
    fn agent(instance: &str, name: &str, issue: u64, events: &[(&str, Option<u64>, &str)]) -> (String, String) {
        let mut lines = vec![format!(r#"{{"event":"session.start","name":"{name}","role":"worker","t":"2026-10-31T10:00:00Z"}}"#)];
        for (event, mr, at) in events {
            let mr = mr.map(|m| format!(r#","mr":{m}"#)).unwrap_or_default();
            lines.push(format!(r#"{{"event":"{event}","issue":{issue}{mr},"t":"{at}"}}"#));
        }
        (instance.to_owned(), lines.join("\n"))
    }

    fn note(body: &str, at: &str) -> Value {
        json!({ "body": body, "created_at": at })
    }

    fn me(instance: &str, name: &str, branch: &str) -> Me {
        Me { instance: Some(instance.into()), name: Some(name.into()), branch: Some(branch.into()) }
    }

    /// mc-engine#18: cedar claimed (PR #38, branch `18-env`); birch claimed 90 s later, derived
    /// the same branch, and adopted cedar's PR as its own.
    #[test]
    fn a_later_claim_doesn_t_adopt_another_agent_s_pr_from_the_same_branch() {
        let notes = [note("claimed: cedar, PR #38", "2026-10-31T12:25:50Z")];
        let tracked =
            parse_tracking(&[agent("mac-cedar", "cedar", 18, &[("claim", Some(38), "2026-10-31T12:25:49Z"), ("work", None, "2026-10-31T12:25:50Z")])]);
        let now = t("2026-10-31T12:27:20Z");
        let facts = Facts {
            labels: vec![LABEL.into()],
            mrs: vec![mr(38, "18-env", "Environment (#18)", "Closes #18")],
            claim_note: latest_claim(&notes),
            claims: active_claims(&notes, &tracked, 18, now),
            tracked,
            ..Facts::default()
        };
        let birch = me("mac-birch", "birch", "18-env");
        assert!(my_mrs(18, &facts, &birch, now).is_empty(), "the branch's PR is cedar's: its claim names it");
        let held = holders(&P, 18, &facts, &birch, now, true);
        assert_eq!(held.len(), 3, "the label, the PR and cedar's claim: {held:?}");
        // cedar, resuming, is held by nobody.
        assert!(holders(&P, 18, &facts, &me("mac-cedar", "cedar", "18-env"), now, true).is_empty());
    }

    /// mc-engine#61: three claims 2 s apart, each on PR #62 before the others' events landed.
    fn three_way() -> (Facts, i64) {
        let notes = [
            note("claimed: cedar, PR #62", "2026-10-31T12:57:47Z"),
            note("claimed: alder, PR #62", "2026-10-31T12:57:49Z"),
            note("claimed: birch, PR #62", "2026-10-31T12:57:51Z"),
        ];
        let tracked = parse_tracking(&[
            agent("mac-cedar", "cedar", 61, &[("claim", Some(62), "2026-10-31T12:57:48Z"), ("work", None, "2026-10-31T13:10:00Z")]),
            agent("mac-alder", "alder", 61, &[("claim", Some(62), "2026-10-31T12:57:50Z"), ("work", None, "2026-10-31T12:57:51Z")]),
            agent("mac-birch", "birch", 61, &[("claim", Some(62), "2026-10-31T12:57:52Z")]),
        ]);
        let now = t("2026-10-31T13:15:00Z");
        let facts = Facts {
            labels: vec![LABEL.into()],
            mrs: vec![mr(62, "61-claim-race", "Claim race (#61)", "Closes #61")],
            claim_note: latest_claim(&notes),
            claims: active_claims(&notes, &tracked, 61, now),
            tracked,
            ..Facts::default()
        };
        (facts, now)
    }

    #[test]
    fn of_claims_on_one_pr_the_first_holds_it_whatever_work_came_after() {
        let (facts, now) = three_way();
        let cedar = me("mac-cedar", "cedar", "61-claim-race");
        assert!(holders(&P, 61, &facts, &cedar, now, false).is_empty(), "cedar's later `work` doesn't make it later");
        assert!(push_holders(&P, 61, &facts, &cedar, now).is_empty());
        for (instance, name) in [("mac-alder", "alder"), ("mac-birch", "birch")] {
            let later = me(instance, name, "61-claim-race");
            let held = holders(&P, 61, &facts, &later, now, false);
            assert!(held.iter().any(|h| matches!(h, Holder::Tracking(c) if c.name.as_deref() == Some("cedar"))), "{name}: {held:?}");
            let refused = push_holders(&P, 61, &facts, &later, now);
            assert!(refused.iter().any(|r| r.contains("cedar")), "{name}'s ready is refused: {refused:?}");
        }
        let live = live_claims(&facts.tracked, 61, now);
        let cedar_claim = live.iter().find(|c| c.instance == "mac-cedar").unwrap();
        assert_eq!((cedar_claim.claimed_at, cedar_claim.t), (t("2026-10-31T12:57:48Z"), t("2026-10-31T13:10:00Z")));
    }

    #[test]
    fn the_first_standing_claim_comment_wins_the_race() {
        let (facts, _) = three_way();
        assert_eq!(first_claim(&facts.claims, "cedar"), None);
        assert_eq!(first_claim(&facts.claims, "alder").map(|c| c.name.as_str()), Some("cedar"));
        assert_eq!(first_claim(&facts.claims, "birch").map(|c| c.name.as_str()), Some("cedar"));

        // cedar released (a comment), then alder's claim is the first standing one.
        let notes = [
            note("claimed: cedar, PR #62", "2026-10-31T12:57:47Z"),
            note("claimed: alder, PR #62", "2026-10-31T12:57:49Z"),
            note("released: cedar, lost the claim race", "2026-10-31T12:58:30Z"),
        ];
        let now = t("2026-10-31T13:00:00Z");
        let active = active_claims(&notes, &[], 61, now);
        assert_eq!(first_claim(&active, "alder"), None);
        // A release logged only on the tracking branch counts too.
        let tracked =
            parse_tracking(&[agent("mac-alder", "alder", 61, &[("claim", Some(62), "2026-10-31T12:57:50Z"), ("release", None, "2026-10-31T12:59:00Z")])]);
        let active = active_claims(&notes, &tracked, 61, now);
        assert!(active.is_empty(), "{active:?}");
        // A coordinator's `release --of` an instance frees that instance's claim, and only its
        // (mornycorp/swarm-core#11).
        let notes = [note("claimed: cedar, PR #62", "2026-10-31T12:57:47Z")];
        let freed = |of: &str| {
            let mut tracked =
                parse_tracking(&[agent("mac-cedar", "cedar", 61, &[("claim", Some(62), "2026-10-31T12:57:48Z")]), agent("mac-coord", "coordinator", 61, &[])]);
            tracked.push(Tracked {
                instance: "mac-coord".into(),
                name: Some("coordinator".into()),
                t: t("2026-10-31T12:59:00Z"),
                event: "release".into(),
                issue: Some(61),
                mr: None,
                of: Some(of.into()),
            });
            active_claims(&notes, &tracked, 61, now).is_empty()
        };
        assert!(freed("mac-cedar"));
        assert!(!freed("mac-alder"));
        // A claim comment older than a live claim's life no longer stands.
        let stale = [note("claimed: cedar, PR #62", "2026-10-29T12:00:00Z"), note("claimed: alder, PR #62", "2026-10-31T12:57:49Z")];
        assert_eq!(first_claim(&active_claims(&stale, &[], 61, now), "alder"), None);
    }

    /// `--take-over` is a claim's argument, never a check's, and keeps the claim's own branch.
    #[test]
    fn take_over_is_an_argument_of_a_claim_with_its_own_branch() {
        use crate::project::fixtures::BOTH;
        for project in BOTH {
            assert!(parse_args(&project, &["92", "--take-over"]).is_ok_and(|o| o.take_over && !o.force), "{}", project.name);
            let root = Path::new(".");
            assert!(run(&project, &crate::Defaults, root, &["--check", "--take-over"]).unwrap_err().contains("--take-over"));
            for extra in [["--slug", "x"].as_slice(), ["--no-pr"].as_slice()] {
                let args: Vec<&str> = ["92", "--agent", "pc-1", "--take-over"].iter().chain(extra).copied().collect();
                assert!(run(&project, &crate::Defaults, root, &args).unwrap_err().contains("keeps the claim's own branch and PR"), "{extra:?}");
            }
            assert!(usage(&project).contains("--take-over"));
        }
        assert!(USAGE.contains("[--take-over]"));
    }

    /// swarmception#92: a branch with work on it on origin is refused, as before, and the refusal
    /// names `--take-over` as the way to keep that work (mornycorp/swarm-core#16). Its earlier
    /// run's claim commit is still resumed.
    #[test]
    fn a_branch_with_work_on_it_is_refused_naming_the_takeover() {
        use crate::project::fixtures::BOTH;
        use swarm_core_testkit::git::git as tgit;
        let dir = swarm_core_testkit::temp_dir("xtask-claim-work-on-branch");
        let remote = swarm_core_testkit::git::remote(&dir);
        tgit(&remote.work, &["switch", "--quiet", "-c", "92-a11y-test"]);
        tgit(&remote.work, &["commit", "--quiet", "--allow-empty", "-m", "Claim #92: A11y test"]);
        tgit(&remote.work, &["push", "--quiet", "origin", "92-a11y-test"]);
        tgit(&remote.work, &["switch", "--quiet", "main"]);
        for project in BOTH {
            assert_eq!(ensure_branch(&project, &remote.work, "92-a11y-test", 92, "A11y test"), Ok(()), "{}: a claim commit resumes", project.name);
        }
        tgit(&remote.work, &["switch", "--quiet", "92-a11y-test"]);
        tgit(&remote.work, &["commit", "--quiet", "--allow-empty", "-m", "The a11y test (#92)"]);
        tgit(&remote.work, &["push", "--quiet", "origin", "92-a11y-test"]);
        tgit(&remote.work, &["switch", "--quiet", "main"]);
        for project in BOTH {
            let refused = ensure_branch(&project, &remote.work, "92-a11y-test", 92, "A11y test").unwrap_err();
            assert!(refused.contains("with work on it (`The a11y test (#92)`)") && refused.contains("pass --take-over instead of --force"), "{refused}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_comments_name_their_agent() {
        assert_eq!(parse_release_note(&release_comment("cedar", "lost the claim race to alder")), Some("cedar".into()));
        assert_eq!(parse_release_note("released: alder — cedar claimed #5 first"), Some("alder".into()));
        assert_eq!(parse_release_note("claimed: alder, PR #26"), None);
        assert_eq!(parse_release_note("released:"), None);
        let claims =
            all_claims(&[note("claimed: b, PR #2", "2026-10-31T12:00:02Z"), note("hello", "2026-10-31T12:00:00Z"), note("claimed: a", "2026-10-31T12:00:01Z")]);
        assert_eq!(claims.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }
}
