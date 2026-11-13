//! `cargo xtask prwatch --agent <id>`: tell an agent when its pull
//! requests need it. `--agent` defaults to `{prefix}_AGENT`, as `cargo xtask event`'s does.
//!
//! - **Which PRs:** the agent's own, from its tracking events (`claim` and `submit` with an `mr` in
//!   `tracking/<id>/work.jsonl`, [`crate::event`]: the field holds the PR number), less the issues
//!   it `release`d: every agent shares one GitHub account, so the account can't say whose a PR is.
//! - **What it says:** one line per change, nothing otherwise: a PR's merge state (`dirty`: a
//!   conflict, `behind`, `clean`, …), its checks finishing, new comments, and a PR that left the
//!   open list (merged: rebase what is stacked on it; closed). The first poll only records.
//! - **What its issues need**: the issues its open claimed issues name on a
//!   `Needs` line ([`crate::needs`]), often another agent's, or another repository's
//!   (`mornycorp/swarm-core#4`): a line when one closes (start what
//!   waited on it) or gets a PR. One more call a poll when it has claimed issues (them, with the
//!   issues they need), and two for each other repository they name.
//! - **Cost:** one GitHub GraphQL call per poll (the repository's open PRs, each with its merge
//!   state, its head commit's checks and its comment count) and a `git ls-remote` of the tracking
//!   branch; one more call when a PR leaves the list.
//! - **A send-back** (mornycorp/swarm-core#14): when the coordinator sends one of the agent's open
//!   PRs back (a `sendback`, or a gate that failed for the change's cause naming it, on the
//!   tracking branch), it says so and logs `work --issue <n> --mr <m>` for the agent, once
//!   ([`sendback`]), so a timeline reader sees where the rework starts. Two local git calls more a
//!   poll (the branch's files, read from the tip already fetched), only while it has an open PR.
//! - **The end of a session** (sc#10): when the agent's last open PR leaves the list and none of
//!   its claimed issues is open (one more call, then only), it says to end the session with
//!   `cargo xtask finish` ([`finish_line`], [`crate::finish`]).
//! - **How to run it:** `--once` polls once (a check on every wake: the changes since the last
//!   poll, then each PR's status) and exits; without it, it polls every `--interval` seconds
//!   (default 150) until stopped, printing changes as they come: a Claude Code worker runs it under
//!   Monitor (each line wakes it), others as a background process.
//! - **State:** the last poll, in the clone's git directory (`prwatch-<id>.json`, beside `event`'s
//!   queue), so every worktree of the clone shares it, with the send-backs it logged `work` for.
//! - **`--ready`**: the coordinator's ready-PR watch instead
//!   (docs/swarm-protocol.md › Staying awake). A PR is ready to gate when it is open, not a draft, its
//!   Progress filled in and its branch not `progress-*` ([`ready_line`]); its line carries its head
//!   commit, so a push is news too. A PR whose branch holds no work against its base
//!   ([`crate::empty`]: no commits of its own, only claim commits, or an empty
//!   diff) is `ready but empty`, not `ready`, and is put back to draft with a comment, once per
//!   head (`redrafted: …`; [`redraft`], mornycorp/swarm-core#42), unless `--no-redraft`. One GitHub call a poll, every `--interval` seconds
//!   (default 120), and each PR's head checked once. How it reports is the project's
//!   ([`ReadyWatch`]): a line per PR as it becomes ready or is pushed (`Stream`), or it **exits**
//!   as soon as one does, printing them, against the record of what it last reported (`Exit`); then
//!   after `--max` minutes (default 240) with nothing new it exits saying so. `--once` lists every
//!   ready PR (with `Exit`, marking the new or pushed ones and leaving the record alone).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::needs::{self, Need, Ref};
use crate::project::ReadyWatch;
use crate::{Project, event, github};

pub mod redraft;
pub mod sendback;

/// Shortest poll interval, in seconds.
const MIN_INTERVAL: u64 = 30;

/// One open PR as a poll saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    /// GitHub's merge state, lower case (`clean`, `behind`, `dirty`, `blocked`, …).
    pub merge_state: String,
    /// Its head commit's checks (`success`, `failure`, `pending`, …), if it has any.
    pub checks: Option<String>,
    pub comments: u64,
}

impl Pr {
    /// One PR in [`github`]'s shape.
    fn from_api(v: &Value) -> Option<Self> {
        Some(Pr {
            number: v.get("number")?.as_u64()?,
            title: v.get("title").and_then(Value::as_str).unwrap_or_default().chars().take(80).collect(),
            merge_state: v.get("merge_state").and_then(Value::as_str).unwrap_or("unknown").to_owned(),
            checks: v.get("checks").and_then(Value::as_str).map(str::to_owned),
            comments: v.get("comments").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    fn to_json(&self) -> Value {
        json!({"number": self.number, "title": self.title, "mergeState": self.merge_state, "checks": self.checks, "comments": self.comments})
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(Pr {
            number: v.get("number")?.as_u64()?,
            title: v.get("title").and_then(Value::as_str).unwrap_or_default().to_owned(),
            merge_state: v.get("mergeState").and_then(Value::as_str).unwrap_or_default().to_owned(),
            checks: v.get("checks").and_then(Value::as_str).map(str::to_owned),
            comments: v.get("comments").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// Its status in one line.
    pub fn status(&self) -> String {
        format!("#{} {}, checks {}, {} comments · {}", self.number, self.merge_state, self.checks.as_deref().unwrap_or("none"), self.comments, self.title)
    }
}

/// Checks that are still going (their status changes are not news).
fn running(status: Option<&str>) -> bool {
    matches!(status, Some("pending" | "expected"))
}

/// The PRs an agent's tracking events name: every `claim`, `work` and `submit` with an `mr` (the
/// PR's number), less those of issues it later `release`d. A claim whose hold went on the tracking
/// branch first names its PR on its `work` (mornycorp/swarm-core#96); a `work` a send-back logs
/// names a PR the agent's claim did.
pub fn agent_mrs(jsonl: &str) -> BTreeSet<u64> {
    let mut by_issue: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    let mut mrs = BTreeSet::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let issue = v.get("issue").and_then(Value::as_u64);
        let mr = v.get("mr").and_then(Value::as_u64);
        match (v.get("event").and_then(Value::as_str), issue, mr) {
            (Some("claim" | "work" | "submit"), _, Some(mr)) => {
                mrs.insert(mr);
                if let Some(i) = issue {
                    by_issue.entry(i).or_default().insert(mr);
                }
            }
            (Some("release"), Some(i), _) => {
                for mr in by_issue.remove(&i).unwrap_or_default() {
                    mrs.remove(&mr);
                }
            }
            _ => {}
        }
    }
    mrs
}

/// Most claimed issues whose needs a poll reads: the newest.
const MAX_CLAIMED: usize = 50;

/// The issues an agent's tracking events claim, less those it later `release`d, newest last.
pub fn agent_issues(jsonl: &str) -> Vec<u64> {
    let mut issues: Vec<u64> = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(issue) = v.get("issue").and_then(Value::as_u64) else { continue };
        match v.get("event").and_then(Value::as_str) {
            Some("claim") => {
                issues.retain(|&i| i != issue);
                issues.push(issue);
            }
            Some("release") => issues.retain(|&i| i != issue),
            _ => {}
        }
    }
    let skip = issues.len().saturating_sub(MAX_CLAIMED);
    issues.split_off(skip)
}

/// A merge state GitHub passes through while it works something out (after a push, after a PR
/// is marked ready): not news, and not kept, so the settled state after it is compared with the
/// one before.
fn transient(state: &str) -> bool {
    state == "unknown"
}

/// What changed between two polls, as lines; and the PRs that left the open list. A PR whose
/// state is transient now keeps its state from before ([`transient`]).
pub fn changes(before: &BTreeMap<u64, Pr>, now: &mut BTreeMap<u64, Pr>) -> (Vec<String>, Vec<u64>) {
    let mut lines = Vec::new();
    for (num, n) in now.iter_mut() {
        let Some(b) = before.get(num) else { continue };
        if transient(&n.merge_state) {
            n.merge_state.clone_from(&b.merge_state);
        }
        if b.merge_state != n.merge_state {
            lines.push(format!("#{num} merge state: {} -> {} · {}", b.merge_state, n.merge_state, n.title));
        }
        if b.checks != n.checks && !running(n.checks.as_deref()) {
            lines.push(format!("#{num} checks: {} · {}", n.checks.as_deref().unwrap_or("none"), n.title));
        }
        if n.comments > b.comments {
            let new = n.comments - b.comments;
            lines.push(format!("#{num} {new} new comment{} · {}", if new == 1 { "" } else { "s" }, n.title));
        }
    }
    let left = before.keys().filter(|num| !now.contains_key(num)).copied().collect();
    (lines, left)
}

fn state_path(root: &Path, agent: &str) -> PathBuf {
    event::common_dir(root).join(format!("prwatch-{agent}.json"))
}

/// What one poll saw: the agent's open PRs, and what its claimed issues need.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polled {
    pub mrs: BTreeMap<u64, Pr>,
    pub needs: BTreeMap<Ref, Need>,
    /// The send-backs of its open PRs it logged `work` for ([`sendback::SendBack::key`]).
    pub resumed: BTreeSet<String>,
}

/// The last poll: `{mrs, needs, resumed}`.
fn load(path: &Path) -> Option<Polled> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let list = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();
    let (mrs, needs, resumed) = (list(v.get("mrs")), list(v.get("needs")), list(v.get("resumed")));
    Some(Polled {
        mrs: mrs.iter().filter_map(Pr::from_json).map(|m| (m.number, m)).collect(),
        needs: needs.iter().filter_map(Need::from_json).map(|n| (n.at(), n)).collect(),
        resumed: resumed.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
    })
}

fn save(path: &Path, polled: &Polled) -> Result<(), String> {
    let v = json!({
        "mrs": polled.mrs.values().map(Pr::to_json).collect::<Vec<_>>(),
        "needs": polled.needs.values().map(Need::to_json).collect::<Vec<_>>(),
        "resumed": polled.resumed,
    });
    std::fs::write(path, v.to_string()).map_err(|e| format!("{}: {e}", path.display()))
}

/// The tracking branch's tip on the remote, fetched (`None` before its first event).
fn tracking_tip(project: &Project, root: &Path) -> Result<Option<String>, String> {
    event::fetch_tip(root, project.branches.remote, &format!("refs/heads/{}", project.branches.tracking))
}

/// The agent's file at the tracking branch's commit `tip` (empty before its first event).
fn file_at(root: &Path, tip: &str, agent: &str) -> String {
    crate::process::git(root, &["show", &format!("{tip}:tracking/{agent}/work.jsonl")], None, &[]).unwrap_or_default()
}

/// The agent's tracking file at the remote's tip (empty before its first event).
pub(crate) fn tracking_file(project: &Project, root: &Path, agent: &str) -> Result<String, String> {
    Ok(tracking_tip(project, root)?.map(|tip| file_at(root, &tip, agent)).unwrap_or_default())
}

/// One poll: the agent's open PRs and its issues' needs now, and what to say (send-backs it
/// logged `work` for, which a first poll says too; then changes, PRs that left the list, and needs
/// that closed or were taken).
fn poll(project: &Project, root: &Path, agent: &str, before: Option<&Polled>) -> Result<(Polled, Vec<String>), String> {
    let tip = tracking_tip(project, root)?;
    let events = tip.as_deref().map(|t| file_at(root, t, agent)).unwrap_or_default();
    let open = github::open_prs(project, None)?;
    let mine = agent_mrs(&events);
    let mrs: BTreeMap<u64, Pr> = open.iter().filter_map(Pr::from_api).filter(|m| mine.contains(&m.number)).map(|m| (m.number, m)).collect();
    let mut now = Polled { mrs, needs: poll_needs(project, &agent_issues(&events), &open)?, resumed: before.map(|b| b.resumed.clone()).unwrap_or_default() };
    let mut lines = pick_up_send_backs(project, root, agent, tip.as_deref(), &events, &mut now);
    let Some(before) = before else { return Ok((now, lines)) };
    let (changed, left) = changes(&before.mrs, &mut now.mrs);
    lines.extend(changed);
    let last_left = !left.is_empty() && now.mrs.is_empty();
    lines.extend(left.into_iter().map(|n| left_line(project, &before.mrs, n)));
    lines.extend(needs::changes(&before.needs, &now.needs));
    if last_left && holds_no_claim(project, &agent_issues(&events)) {
        lines.push(finish_line(agent));
    }
    Ok((now, lines))
}

/// Log `work` for each send-back of the agent's open PRs it hasn't picked up ([`sendback::due`]),
/// keep it in `now.resumed` so it is logged once, and say so; `own` is the agent's file at the
/// tracking branch's `tip`. The record keeps only the open PRs' send-backs.
fn pick_up_send_backs(project: &Project, root: &Path, agent: &str, tip: Option<&str>, own: &str, now: &mut Polled) -> Vec<String> {
    let open: BTreeSet<u64> = now.mrs.keys().copied().collect();
    now.resumed.retain(|k| open.iter().any(|m| k.starts_with(&format!("#{m}@"))));
    let Some(tip) = tip.filter(|_| !open.is_empty()) else { return Vec::new() };
    let files = match crate::claim::tracking_files_at(root, tip) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("prwatch: couldn't read the tracking branch for send-backs: {e}");
            return Vec::new();
        }
    };
    let mut lines = Vec::new();
    for r in sendback::due(&files, own, &open, &now.resumed) {
        let title = now.mrs.get(&r.back.mr).map(|m| m.title.as_str()).unwrap_or_default();
        lines.push(r.line(title, &log_work(project, root, agent, &r)));
        now.resumed.insert(r.back.key());
    }
    lines
}

/// Log the agent's `work` on a send-back it picks up, and say how it went: never fails the poll
/// (offline, the event is queued and sent with the next).
fn log_work(project: &Project, root: &Path, agent: &str, r: &sendback::Resume) -> String {
    let logged = format!("logged `work --issue {} --mr {}`", r.issue, r.back.mr);
    if project.env("TRACKING").is_some_and(|v| v.eq_ignore_ascii_case("off")) {
        return format!("tracking is off ({}=off): log `work --issue {} --mr {}` when you start on it", project.env_name("TRACKING"), r.issue, r.back.mr);
    }
    let e = event::Event { issue: Some(r.issue), mr: Some(r.back.mr), note: Some(r.note()), ..event::Event::named("work") };
    if let Err(why) = e.check_as(agent) {
        return format!("couldn't log `work`: {why}");
    }
    match event::send(project, root, &event::queue_path(project, root), &[(agent.to_owned(), e.line(&crate::time::now_iso()))]) {
        Ok(_) => format!("{logged}: your rework starts now"),
        Err(why) => format!("{logged} (queued: {why}): your rework starts now"),
    }
}

/// The open issues among `claimed` (at most 100, the newest), in [`github`]'s shape.
fn open_claimed(project: &Project, claimed: &[u64]) -> Result<Vec<Value>, String> {
    let skip = claimed.len().saturating_sub(100);
    Ok(github::issues(project, None, claimed.get(skip..).unwrap_or_default())?.into_iter().filter(|i| i["state"] == "open").collect())
}

/// Whether none of `claimed` is open: no call when there are none; a failed call says it holds one.
fn holds_no_claim(project: &Project, claimed: &[u64]) -> bool {
    claimed.is_empty() || open_claimed(project, claimed).is_ok_and(|open| open.is_empty())
}

/// What `prwatch` says once the agent's last PR has left the open list and it holds no open claim
/// (sc#10): end the session with `finish`.
pub fn finish_line(agent: &str) -> String {
    format!("no open PR or claim left: if you are done, end your session with `cargo xtask finish --agent {agent}` before you remove your last worktree")
}

/// What the open issues among `claimed` need, each need with its state and PR (from `open`, this
/// repository's open PRs): one call for the claimed issues, and two for each other repository they
/// name.
fn poll_needs(project: &Project, claimed: &[u64], open: &[Value]) -> Result<BTreeMap<Ref, Need>, String> {
    if claimed.is_empty() {
        return Ok(BTreeMap::new());
    }
    let wanted = needs::wanted(project, &open_claimed(project, claimed)?);
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let open_here = crate::claim::parse_prs(open);
    Ok(needs::fetch(project, &wanted, &open_here, &needs::GitHub(project)))
}

/// What to say of PR `n`, which left the open list since `before`: merged (rebase what is stacked
/// on it) or closed, from one call made only now.
fn left_line(project: &Project, before: &BTreeMap<u64, Pr>, n: u64) -> String {
    let title = before.get(&n).map(|m| m.title.clone()).unwrap_or_default();
    let state = github::pr(project, n).ok().and_then(|v| v.get("state").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| "gone".into());
    let what = if state == "merged" { "merged: rebase what is stacked on it" } else { state.as_str() };
    format!("#{n} {what} · {title}")
}

/// The template's unfilled Progress: its first field still holds the template's comment.
pub const UNFILLED: &str = "Measured:** <!--";

/// The line for an open PR (in [`github`]'s shape) that is ready to gate, or `None`: a draft, a
/// progress fold's branch (`progress-*`, the coordinator's own), or a Progress section not filled
/// in. The line names its head commit, so the same PR pushed again reads as a new line.
pub fn ready_line(v: &Value) -> Option<String> {
    let n = v.get("number")?.as_u64()?;
    let branch = v.get("head_ref").and_then(Value::as_str).unwrap_or_default();
    let draft = v.get("draft").and_then(Value::as_bool).unwrap_or(false);
    let body = v.get("body").and_then(Value::as_str).unwrap_or_default();
    if draft || branch.starts_with("progress-") || body.contains(UNFILLED) {
        return None;
    }
    let sha: String = v.get("head_sha").and_then(Value::as_str).unwrap_or_default().chars().take(10).collect();
    let title: String = v.get("title").and_then(Value::as_str).unwrap_or_default().chars().take(100).collect();
    Some(format!("#{n} {sha} {branch} · {title}"))
}

/// The lines among `now` not in `seen`: PRs newly ready, or pushed since.
pub fn newly_ready(seen: &BTreeSet<String>, now: &[String]) -> Vec<String> {
    now.iter().filter(|l| !seen.contains(*l)).cloned().collect()
}

/// What `--ready` says of a ready PR's `line`: `ready: …`, or `ready but
/// empty: …` with why, when its branch holds no work.
pub fn ready_says(line: &str, empty: Option<&str>) -> String {
    match empty {
        None => format!("ready: {line}"),
        Some(why) => format!("ready but empty: {line}: {why}; {}", crate::empty::FINISH),
    }
}

/// Why the open PR `v`'s branch holds no work against its base branch, `None` when it does,
/// from the clone at `root`; each head (`head_sha`) checked once, in `verdicts`. A branch pushed
/// since the listing has a tip that isn't its head: no verdict, and none kept; the next poll
/// decides (mornycorp/swarm-core#32).
fn emptiness(root: &Path, v: &Value, verdicts: &mut BTreeMap<String, Option<String>>) -> Result<Option<String>, String> {
    let sha = v.get("head_sha").and_then(Value::as_str).unwrap_or_default();
    if let Some(known) = verdicts.get(sha) {
        return Ok(known.clone());
    }
    let branch = v.get("head_ref").and_then(Value::as_str).unwrap_or_default();
    let target = v.get("base_ref").and_then(Value::as_str).filter(|b| !b.is_empty()).unwrap_or("main");
    if !crate::stack::valid_branch(branch) || !crate::stack::valid_branch(target) {
        return Err(format!("`{branch}` into `{target}`: not branches to check"));
    }
    let base = format!("refs/remotes/origin/{target}");
    event::fetch(root, "origin", &format!("refs/heads/{target}"), &base)?;
    let tip = event::fetch_tip(root, "origin", &format!("refs/heads/{branch}"))?.ok_or_else(|| format!("origin/{branch} is gone"))?;
    if !sha.is_empty() && tip != sha {
        return Ok(None);
    }
    let why = crate::empty::why(root, &base, &tip)?;
    if !sha.is_empty() {
        verdicts.insert(sha.to_owned(), why.clone());
    }
    Ok(why)
}

/// What `--ready` says of a PR it put back to draft: no longer ready, so no `ready:` line.
pub fn redrafted_says(line: &str, why: &str, extra: &str) -> String {
    format!("redrafted: {line}: {why}; put back to draft, with a note{extra}")
}

/// What one ready PR's line is, given whether it holds work (`empty`: why not) and, for an empty
/// one, what re-drafting it came to (`outcome`; `None` with `--no-redraft`): the line, and
/// whether it is still ready.
fn assess(line: &str, empty: Option<&str>, outcome: Option<redraft::Outcome>) -> (String, bool) {
    match (empty, outcome) {
        (Some(why), Some(redraft::Outcome::Redrafted(extra))) => (redrafted_says(line, why, &extra), false),
        (Some(why), Some(redraft::Outcome::Left(left))) => (ready_says(line, Some(&format!("{why} ({left})"))), true),
        (empty, _) => (ready_says(line, empty), true),
    }
}

/// The repository's open PRs ready to gate, one line each ([`ready_says`]), and the lines of
/// those `redrafts` put back to draft for holding no work ([`redraft`]; none with `None`, which
/// only reports).
fn ready_mrs(
    project: &Project,
    root: &Path,
    verdicts: &mut BTreeMap<String, Option<String>>,
    mut redrafts: Option<&mut redraft::Redrafts>,
) -> Result<(Vec<String>, Vec<String>), String> {
    let (mut ready, mut redrafted) = (Vec::new(), Vec::new());
    for v in github::open_prs(project, None)? {
        let Some(line) = ready_line(&v) else { continue };
        // A check that fails says so and reads as ready: the gate still sees the PR.
        let empty = emptiness(root, &v, verdicts).unwrap_or_else(|e| {
            eprintln!("prwatch: couldn't check whether {line} holds work: {e}");
            None
        });
        let outcome = match (&empty, redrafts.as_deref_mut()) {
            (Some(why), Some(redrafts)) => Some(redraft::redraft(redrafts, &v, why, &mut redraft::Live(project))),
            _ => None,
        };
        let (says, still_ready) = assess(&line, empty.as_deref(), outcome);
        if still_ready { ready.push(says) } else { redrafted.push(says) }
    }
    Ok((ready, redrafted))
}

/// The record of the heads `--ready` re-drafted, unless it only reports (`--no-redraft`).
fn redrafts(root: &Path, every: Every) -> Option<redraft::Redrafts> {
    every.redraft.then(|| redraft::Redrafts::load(event::common_dir(root).join(redraft::RECORD)))
}

/// How `--ready` polls: every `interval`, for at most `max` when given (`Exit` defaults it).
#[derive(Clone, Copy, Debug)]
struct Every {
    once: bool,
    /// Put a ready PR that holds no work back to draft (not with `--no-redraft`).
    redraft: bool,
    interval: Duration,
    max: Option<Duration>,
}

/// `prwatch --ready`, as the project reports ([`ReadyWatch`]).
fn watch_ready(project: &Project, root: &Path, every: Every) -> Result<(), String> {
    match project.ready_watch {
        ReadyWatch::Stream => stream_ready(project, root, every),
        ReadyWatch::Exit { record } => exit_on_ready(project, root, &event::common_dir(root).join(record), every),
    }
}

/// `Stream`: say each PR that becomes ready (or ready but empty), or is pushed, once.
fn stream_ready(project: &Project, root: &Path, every: Every) -> Result<(), String> {
    let (mut seen, mut verdicts) = (BTreeSet::new(), BTreeMap::new());
    let mut redrafts = redrafts(root, every);
    let started = std::time::Instant::now();
    loop {
        match ready_mrs(project, root, &mut verdicts, redrafts.as_mut()) {
            Ok((now, redrafted)) => {
                // Each is said once: its head is in the record, and it is a draft now.
                redrafted.iter().for_each(|l| println!("{l}"));
                for l in newly_ready(&seen, &now) {
                    println!("{l}");
                }
                seen.extend(now);
            }
            Err(e) if every.once => return Err(e),
            Err(e) => eprintln!("prwatch: {e}"),
        }
        if every.once || every.max.is_some_and(|m| started.elapsed() >= m) {
            return Ok(());
        }
        std::thread::sleep(every.interval);
    }
}

/// The ready PRs `--ready` last reported (`Exit`), shared by the clone's worktrees.
fn load_ready(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path).map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect()).unwrap_or_default()
}

/// `Exit`: exit as soon as a PR is newly ready or pushed since the `record` (or, `once`, list them
/// all, marking those, and leave the record alone).
fn exit_on_ready(project: &Project, root: &Path, record: &Path, every: Every) -> Result<(), String> {
    let max = every.max.unwrap_or(Duration::from_secs(240 * 60));
    let mut verdicts = BTreeMap::new();
    let mut redrafts = redrafts(root, every);
    let started = std::time::Instant::now();
    loop {
        let seen = load_ready(record);
        // A PR put back to draft is said, and is no reason to exit: nothing is ready to gate.
        let polled = ready_mrs(project, root, &mut verdicts, redrafts.as_mut()).map(|(now, redrafted)| {
            redrafted.iter().for_each(|l| println!("{l}"));
            now
        });
        match polled {
            Ok(now) if every.once => {
                let new = newly_ready(&seen, &now);
                for l in &now {
                    println!("{l}{}", if new.contains(l) { "  (new or pushed)" } else { "" });
                }
                if now.is_empty() {
                    println!("prwatch: no PR is ready");
                }
                return Ok(());
            }
            Ok(now) => {
                let new = newly_ready(&seen, &now);
                if !new.is_empty() {
                    std::fs::write(record, now.iter().map(|l| format!("{l}\n")).collect::<String>()).map_err(|e| format!("{}: {e}", record.display()))?;
                    println!("ready PRs new or pushed since last seen:");
                    for l in new {
                        println!("{l}");
                    }
                    return Ok(());
                }
            }
            Err(e) if every.once => return Err(e),
            Err(e) => eprintln!("prwatch: {e}"),
        }
        if started.elapsed() >= max {
            println!("prwatch: no PR newly ready or pushed in {} min; re-arm the watch", max.as_secs() / 60);
            return Ok(());
        }
        std::thread::sleep(every.interval);
    }
}

/// The agent to watch: `--agent`, else `{prefix}_AGENT` (`env_agent`), as `cargo xtask event`
/// takes it.
fn which_agent(project: &Project, flag: Option<String>, env_agent: Option<String>) -> Result<String, String> {
    let agent = flag.or_else(|| env_agent.filter(|s| !s.is_empty())).ok_or_else(|| {
        format!("prwatch: which agent? pass --agent <id> (your instance id from `cargo xtask event session.start`) or set {}", project.env_name("AGENT"))
    })?;
    event::check_id(&agent)?;
    Ok(agent)
}

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str =
    "cargo xtask prwatch [--agent <id>] [--once] [--interval <seconds>] | --ready [--once] [--interval <seconds>] [--max <minutes>] [--no-redraft]";

/// What `prwatch` was asked: one agent's PRs, or every ready one; once, or every `interval`
/// seconds (for at most `max` minutes, with `--ready`).
#[derive(Debug, Default, PartialEq)]
struct Args {
    agent: Option<String>,
    once: bool,
    ready: bool,
    interval: Option<u64>,
    max: Option<u64>,
    /// `--no-redraft`: `--ready` only reports a ready PR that holds no work.
    no_redraft: bool,
}

fn parse_args(args: &[&str]) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--agent" => out.agent = it.next().map(|s| (*s).to_owned()),
            "--once" => out.once = true,
            "--ready" => out.ready = true,
            "--no-redraft" => out.no_redraft = true,
            "--interval" => {
                out.interval = Some(it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("--interval takes seconds")?);
            }
            "--max" => out.max = Some(it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("--max takes minutes")?),
            other => return Err(format!("prwatch: unknown argument `{other}` (usage: {USAGE})")),
        }
    }
    Ok(out)
}

/// `cargo xtask prwatch …` ([`USAGE`]) for `project`.
pub fn run(project: &Project, root: &Path, args: &[&str]) -> Result<(), String> {
    let Args { agent, once, ready, interval, max, no_redraft } = parse_args(args)?;
    if ready {
        if agent.is_some() {
            return Err("prwatch: --ready watches every open PR, not one agent's: drop --agent".to_owned());
        }
        let interval = Duration::from_secs(interval.unwrap_or(120).max(MIN_INTERVAL));
        return watch_ready(project, root, Every { once, redraft: !no_redraft, interval, max: max.map(|m| Duration::from_secs(m.saturating_mul(60))) });
    }
    if max.is_some() {
        return Err("prwatch: --max goes with --ready".to_owned());
    }
    if no_redraft {
        return Err("prwatch: --no-redraft goes with --ready".to_owned());
    }
    let agent = which_agent(project, agent, project.env("AGENT"))?;
    watch_agent(project, root, &agent, once, Duration::from_secs(interval.unwrap_or(150).max(MIN_INTERVAL)))
}

/// Watch `agent`'s PRs: poll, say what changed (with `once`, each PR's status too), remember it,
/// and, unless `once`, again every `interval`.
fn watch_agent(project: &Project, root: &Path, agent: &str, once: bool, interval: Duration) -> Result<(), String> {
    let path = state_path(root, agent);
    let mut before = load(&path);
    loop {
        match poll(project, root, agent, before.as_ref()) {
            Ok((now, lines)) => {
                for l in &lines {
                    println!("{l}");
                }
                if once {
                    report_once(&now, before.is_none());
                }
                save(&path, &now)?;
                before = Some(now);
            }
            // A failed poll says so once and tries again next time; --once reports it.
            Err(e) if once => return Err(e),
            Err(e) => eprintln!("prwatch: {e}"),
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(interval);
    }
}

/// `--once`'s look at `now`: each PR's status and each open need's, after a word on a first poll.
fn report_once(now: &Polled, first: bool) {
    if first {
        println!("prwatch: first poll: {} open PR(s) and {} need(s) recorded", now.mrs.len(), now.needs.len());
    }
    for m in now.mrs.values() {
        println!("{}", m.status());
    }
    for n in now.needs.values().filter(|n| n.state == "open") {
        println!("needs: {}", n.status());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::APP_SWARM as P;

    #[test]
    fn arguments_parse_as_they_did() {
        assert_eq!(parse_args(&[]).unwrap(), Args::default());
        let a = parse_args(&["--agent", "m-1", "--once", "--interval", "60"]).unwrap();
        assert_eq!(a, Args { agent: Some("m-1".into()), once: true, ready: false, interval: Some(60), max: None, no_redraft: false });
        assert_eq!(parse_args(&["--ready", "--max", "30"]).unwrap().max, Some(30));
        assert_eq!(parse_args(&["--max", "x"]).unwrap_err(), "--max takes minutes");
        assert!(parse_args(&["--ready"]).unwrap().ready);
        assert!(parse_args(&["--ready", "--no-redraft"]).unwrap().no_redraft && !parse_args(&["--ready"]).unwrap().no_redraft);
        assert_eq!(parse_args(&["--agent"]).unwrap().agent, None, "no value: asked for later");
        assert_eq!(parse_args(&["--interval", "x"]).unwrap_err(), "--interval takes seconds");
        assert!(parse_args(&["--bogus"]).unwrap_err().contains("unknown argument `--bogus`"));
    }

    /// sc#10: the line after the last PR names the agent's own `finish` command.
    #[test]
    fn the_last_merge_points_at_finish() {
        let line = finish_line("pc-a0b1c2");
        assert!(line.contains("cargo xtask finish --agent pc-a0b1c2") && line.contains("before you remove your last worktree"), "{line}");
        assert!(holds_no_claim(&P, &[]), "no claims: no call, none held");
    }

    fn pr(number: u64, state: &str, checks: Option<&str>, comments: u64) -> Pr {
        Pr { number, title: format!("PR {number}"), merge_state: state.into(), checks: checks.map(str::to_owned), comments }
    }

    #[test]
    fn an_agents_prs_are_its_claims_and_submits_less_its_releases() {
        let jsonl = r#"{"t":"x","event":"session.start","name":"a"}
{"t":"x","event":"claim","issue":10,"mr":100}
{"t":"x","event":"submit","issue":10,"mr":100}
{"t":"x","event":"claim","issue":11,"mr":101}
{"t":"x","event":"release","issue":11,"reason":"duplicate"}
{"t":"x","event":"claim","issue":12,"mr":102}
not json
{"t":"x","event":"work","issue":12}"#;
        assert_eq!(agent_mrs(jsonl).into_iter().collect::<Vec<_>>(), [100, 102]);
        assert_eq!(agent_issues(jsonl), [10, 12], "#277: the issues whose needs it watches");
        // A claim held on the tracking branch first names its PR on its `work`.
        let held = format!("{jsonl}\n{{\"t\":\"x\",\"event\":\"claim\",\"issue\":13}}\n{{\"t\":\"x\",\"event\":\"work\",\"issue\":13,\"mr\":103}}");
        assert_eq!(agent_mrs(&held).into_iter().collect::<Vec<_>>(), [100, 102, 103]);
        assert_eq!(agent_issues(&held), [10, 12, 13]);
        let many: String = (1..=80).map(|i| format!("{{\"event\":\"claim\",\"issue\":{i}}}\n")).collect();
        let newest = agent_issues(&format!("{many}{{\"event\":\"claim\",\"issue\":3}}"));
        assert_eq!((newest.len(), newest.first(), newest.last()), (MAX_CLAIMED, Some(&32), Some(&3)), "the newest, a reclaim counting as new");
    }

    #[test]
    fn only_changes_are_news() {
        let before: BTreeMap<u64, Pr> =
            [pr(1, "behind", Some("pending"), 2), pr(2, "clean", Some("success"), 0), pr(3, "dirty", None, 0)].into_iter().map(|m| (m.number, m)).collect();
        let mut now: BTreeMap<u64, Pr> =
            [pr(1, "dirty", Some("failure"), 4), pr(2, "clean", Some("pending"), 0), pr(4, "clean", None, 0)].into_iter().map(|m| (m.number, m)).collect();
        let (lines, left) = changes(&before, &mut now);
        assert_eq!(lines, ["#1 merge state: behind -> dirty · PR 1", "#1 checks: failure · PR 1", "#1 2 new comments · PR 1"]);
        assert_eq!(left, [3], "#3 left the open list; #4 is new: recorded, not news");
        let same = now.clone();
        let (lines, left) = changes(&same, &mut now);
        assert!(lines.is_empty() && left.is_empty(), "nothing changed, nothing said");
        // Marked ready: draft, then unknown while GitHub works it out (kept as draft), then clean:
        // one line, from the state before to the settled one.
        let draft: BTreeMap<u64, Pr> = [(5, pr(5, "draft", None, 0))].into_iter().collect();
        let mut computing: BTreeMap<u64, Pr> = [(5, pr(5, "unknown", None, 0))].into_iter().collect();
        assert!(changes(&draft, &mut computing).0.is_empty());
        assert_eq!(computing[&5].merge_state, "draft");
        let mut settled: BTreeMap<u64, Pr> = [(5, pr(5, "clean", None, 0))].into_iter().collect();
        assert_eq!(changes(&computing, &mut settled).0, ["#5 merge state: draft -> clean · PR 5"]);
    }

    /// Through git: the agent's own events, as `cargo xtask event` pushed them, name its PRs; its
    /// state lives in the clone's git directory.
    #[test]
    fn an_agents_prs_are_read_from_the_tracking_branch() {
        let dir = swarm_core_testkit::temp_dir("xtask-prwatch");
        let remote = dir.join("remote.git");
        crate::process::git(&dir, &["init", "--quiet", "--bare", remote.to_str().unwrap()], None, &[]).unwrap();
        let clone = dir.join("clone");
        crate::process::git(&dir, &["init", "--quiet", clone.to_str().unwrap()], None, &[]).unwrap();
        crate::process::git(&clone, &["remote", "add", "origin", remote.to_str().unwrap()], None, &[]).unwrap();
        assert_eq!(tracking_file(&P, &clone, "pc-1"), Ok(String::new()), "no tracking branch yet");
        let lines = [
            ("pc-1", r#"{"t":"2026-10-18T15:00:00Z","event":"claim","issue":96,"mr":103}"#),
            ("pc-2", r#"{"t":"2026-10-18T15:01:00Z","event":"claim","issue":95,"mr":102}"#),
            ("pc-1", r#"{"t":"2026-10-18T15:02:00Z","event":"submit","issue":94,"mr":101}"#),
        ];
        let events: Vec<(String, String)> = lines.iter().map(|(a, l)| ((*a).to_owned(), (*l).to_owned())).collect();
        assert_eq!(event::send(&P, &clone, &dir.join("queue.jsonl"), &events), Ok(3));
        assert_eq!(agent_mrs(&tracking_file(&P, &clone, "pc-1").unwrap()).into_iter().collect::<Vec<_>>(), [101, 103]);
        assert_eq!(tracking_file(&P, &clone, "pc-3"), Ok(String::new()), "an agent with no events has no PRs");
        let state = state_path(&clone, "pc-1");
        assert!(state.ends_with(".git/prwatch-pc-1.json"), "{}", state.display());
        let mrs: BTreeMap<u64, Pr> = [(7, pr(7, "dirty", Some("failure"), 3))].into_iter().collect();
        let need = Need { project: None, iid: 267, title: "S4".into(), state: "open".into(), by: [269].into_iter().collect(), mr: Some(293) };
        let polled =
            Polled { mrs, needs: [(Ref::here(267), need)].into_iter().collect(), resumed: ["#7@2026-10-18T15:03:00Z".to_owned()].into_iter().collect() };
        save(&state, &polled).unwrap();
        assert_eq!(load(&state), Some(polled));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// mornycorp/swarm-core#14, through git: the coordinator's `sendback` of the agent's open PR
    /// logs the agent's `work` on it once, which the next poll, and a poll with no record, see.
    #[test]
    fn a_send_back_logs_the_workers_work_once() {
        let dir = swarm_core_testkit::temp_dir("xtask-prwatch-sendback");
        let remote = dir.join("remote.git");
        crate::process::git(&dir, &["init", "--quiet", "--bare", remote.to_str().unwrap()], None, &[]).unwrap();
        let clone = dir.join("clone");
        crate::process::git(&dir, &["init", "--quiet", clone.to_str().unwrap()], None, &[]).unwrap();
        crate::process::git(&clone, &["remote", "add", "origin", remote.to_str().unwrap()], None, &[]).unwrap();
        let lines = [
            ("pc-w", r#"{"event":"claim","issue":7,"mr":9,"t":"2026-10-18T15:00:00Z"}"#),
            ("pc-w", r#"{"event":"submit","issue":7,"mr":9,"t":"2026-10-18T15:30:00Z"}"#),
            ("pc-c", r#"{"event":"sendback","issue":7,"mr":9,"note":"cause=conflict: Cargo.toml","t":"2026-10-18T16:00:00Z"}"#),
        ];
        let events: Vec<(String, String)> = lines.iter().map(|(a, l)| ((*a).to_owned(), (*l).to_owned())).collect();
        assert_eq!(event::send(&P, &clone, &dir.join("queue.jsonl"), &events), Ok(3));
        let poll_once = |resumed: BTreeSet<String>| {
            let tip = tracking_tip(&P, &clone).unwrap();
            let own = file_at(&clone, tip.as_deref().unwrap(), "pc-w");
            let mut now = Polled { mrs: [(9, pr(9, "clean", None, 1))].into_iter().collect(), needs: BTreeMap::new(), resumed };
            let said = pick_up_send_backs(&P, &clone, "pc-w", tip.as_deref(), &own, &mut now);
            (said, now.resumed)
        };
        let (said, resumed) = poll_once(["#4@2026-10-01T00:00:00Z".to_owned()].into_iter().collect());
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("#9 sent back (2026-10-18T16:00:00Z): cause=conflict: Cargo.toml; logged `work --issue 7 --mr 9`"), "{}", said[0]);
        assert_eq!(resumed.into_iter().collect::<Vec<_>>(), ["#9@2026-10-18T16:00:00Z"], "a closed PR's record is dropped");
        let own = tracking_file(&P, &clone, "pc-w").unwrap();
        let work: Vec<&str> = own.lines().filter(|l| l.contains(r#""event":"work""#)).collect();
        assert_eq!(work.len(), 1, "{own}");
        let v: Value = serde_json::from_str(work[0]).unwrap();
        assert_eq!((v["issue"].as_u64(), v["mr"].as_u64()), (Some(7), Some(9)));
        assert_eq!(v["note"], "rework: #9 sent back 2026-10-18T16:00:00Z: cause=conflict: Cargo.toml");
        let (again, _) = poll_once(["#9@2026-10-18T16:00:00Z".to_owned()].into_iter().collect());
        assert!(again.is_empty(), "logged once: {again:?}");
        let (fresh, _) = poll_once(BTreeSet::new());
        assert!(fresh.is_empty(), "the work on the branch says it was picked up: {fresh:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pr_is_ready_when_not_a_draft_its_progress_filled_in() {
        let filled = "## Progress\n\n- **Measured:** 12 tests\n";
        let unfilled = "## Progress\n\n- **Measured:** <!-- numbers -->\n";
        let pr =
            |n: u64, draft: bool, branch: &str, body: &str| json!({"number": n, "title": format!("PR {n}"), "draft": draft, "head_ref": branch, "body": body});
        assert_eq!(ready_line(&pr(1, false, "170-gate", filled)), Some("#1  170-gate · PR 1".into()));
        assert_eq!(ready_line(&pr(2, true, "171-merge", filled)), None, "a draft");
        assert_eq!(ready_line(&pr(3, false, "progress-2026-10-20", filled)), None, "the coordinator's progress fold");
        assert_eq!(ready_line(&pr(4, false, "172-gates", unfilled)), None, "its Progress not filled in");
        assert_eq!(ready_line(&json!({"title": "no number"})), None);
        assert_eq!(ready_line(&json!({"number": 6})), Some("#6   · ".into()), "missing fields read as empty");
        let mut seen = BTreeSet::new();
        let lines = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(newly_ready(&seen, &lines(&["#1 a", "#2 b"])), ["#1 a", "#2 b"], "the first poll names every ready PR");
        seen.extend(lines(&["#1 a", "#2 b"]));
        assert_eq!(newly_ready(&seen, &lines(&["#1 a", "#2 b", "#3 c"])), ["#3 c"], "then only the new");
        seen.insert("#3 c".to_owned());
        assert!(newly_ready(&seen, &lines(&["#3 c"])).is_empty());
    }

    /// A PR was once reported ready with only its claim commit, and merged. Such a PR is
    /// "ready but empty"; a real change is ready. Each head is checked once.
    #[test]
    fn a_pr_with_only_its_claim_commit_is_ready_but_empty() {
        use crate::stack::testgit;
        let dir = swarm_core_testkit::temp_dir("xtask-prwatch-empty");
        let (origin, clone) = (dir.join("origin.git"), dir.join("clone"));
        testgit::bare(&origin);
        testgit::push_commit(&origin, "main", &[], &[("code.rs", "fn a() {}\n")], "first");
        testgit::clone(&origin, &clone);
        let claim = testgit::push_commit(&origin, "9-empty", &["main"], &[], "Claim #9: nothing yet");
        let real = testgit::push_commit(&origin, "10-real", &["main"], &[("code.rs", "fn a() { b() }\n")], "a real change");
        testgit::push_commit(&origin, "main", &["main"], &[("more.rs", "fn m() {}\n")], "main moves on");
        let mr = |sha: &str, branch: &str| json!({"number": 9, "head_sha": sha, "head_ref": branch, "base_ref": "main"});
        let mut verdicts = BTreeMap::new();

        let why = emptiness(&clone, &mr(&claim, "9-empty"), &mut verdicts).unwrap().unwrap();
        assert!(why.contains("only its claim commit"), "{why}");
        assert_eq!(emptiness(&clone, &mr(&real, "10-real"), &mut verdicts), Ok(None));
        assert_eq!(verdicts.len(), 2);
        // Known heads aren't fetched again (this branch doesn't exist).
        assert!(emptiness(&clone, &mr(&claim, "11-gone"), &mut verdicts).unwrap().is_some());
        assert!(emptiness(&clone, &mr("", "11-gone"), &mut verdicts).unwrap_err().contains("gone"));
        assert!(emptiness(&clone, &mr("", "-x"), &mut verdicts).is_err(), "never an option to git");

        assert_eq!(ready_says("#10 PR (10-real)", None), "ready: #10 PR (10-real)");
        let says = ready_says("#9 PR (9-empty)", Some(&why));
        assert!(says.starts_with("ready but empty: #9 PR (9-empty): it holds only its claim commit") && says.ends_with("close the PR"), "{says}");
        // What becomes of it: re-drafted, it is no longer ready; left (once a head), it still is,
        // with why; and with `--no-redraft`, or work in it, the line is as it was.
        let line = "#9 PR (9-empty)";
        assert_eq!(
            assess(line, Some("empty"), Some(redraft::Outcome::Redrafted(String::new()))),
            ("redrafted: #9 PR (9-empty): empty; put back to draft, with a note".to_owned(), false)
        );
        let (left, still) = assess(line, Some("empty"), Some(redraft::Outcome::Left(redraft::AGAIN.to_owned())));
        assert!(still && left.starts_with("ready but empty: #9 PR (9-empty): empty (re-drafted once at this head already: not again); "), "{left}");
        assert_eq!(assess(line, Some("empty"), None), (ready_says(line, Some("empty")), true));
        assert_eq!(assess(line, None, None), ("ready: #9 PR (9-empty)".to_owned(), true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// mornycorp/swarm-core#32: the listing gave the claim commit as the head, then the branch was
    /// pushed with work before the fetch. The verdict would be the new tip's, cached under the old
    /// head: there is none, and the next poll, with the new head, decides.
    #[test]
    fn a_branch_pushed_between_the_listing_and_the_fetch_gets_no_verdict() {
        use crate::stack::testgit;
        let dir = swarm_core_testkit::temp_dir("xtask-prwatch-moved");
        let (origin, clone) = (dir.join("origin.git"), dir.join("clone"));
        testgit::bare(&origin);
        testgit::push_commit(&origin, "main", &[], &[("code.rs", "fn a() {}\n")], "first");
        testgit::clone(&origin, &clone);
        let listed = testgit::push_commit(&origin, "9-moved", &["main"], &[], "Claim #9: nothing yet");
        let pushed = testgit::push_commit(&origin, "9-moved", &["9-moved"], &[("code.rs", "fn a() { b() }\n")], "the work");
        let mr = |sha: &str| json!({"number": 9, "head_sha": sha, "head_ref": "9-moved", "base_ref": "main"});
        let mut verdicts = BTreeMap::new();

        assert_eq!(emptiness(&clone, &mr(&listed), &mut verdicts), Ok(None), "no verdict for a head the branch no longer has");
        assert!(verdicts.is_empty(), "and none kept: {verdicts:?}");
        assert_eq!(emptiness(&clone, &mr(&pushed), &mut verdicts), Ok(None), "the new head holds work");
        assert_eq!(verdicts.get(&pushed), Some(&None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arguments_are_checked() {
        let root = Path::new(".");
        assert!(run(&P, root, &["--agent", "pc-1", "--bogus"]).unwrap_err().contains("unknown argument"));
        assert!(run(&P, root, &["--ready", "--agent", "pc-1"]).unwrap_err().contains("drop --agent"));
        assert!(run(&P, root, &["--ready", "--interval", "x"]).unwrap_err().contains("--interval"));
        assert!(run(&P, root, &["--agent", "pc-1", "--interval", "soon"]).unwrap_err().contains("--interval"));
        assert!(run(&P, root, &["--agent", "../x", "--once"]).is_err());
        assert!(run(&P, root, &["--ready", "--max", "soon"]).unwrap_err().contains("--max"));
        assert!(run(&P, root, &["--agent", "pc-1", "--max", "5"]).unwrap_err().contains("--max goes with --ready"));
        assert!(run(&P, root, &["--agent", "pc-1", "--no-redraft"]).unwrap_err().contains("--no-redraft goes with --ready"));
    }

    /// `--agent`, else the project's `{prefix}_AGENT`, as `cargo xtask event` takes it.
    #[test]
    fn the_agent_is_the_flag_or_the_projects_variable() {
        for p in crate::project::fixtures::BOTH {
            assert_eq!(which_agent(&p, Some("pc-1".into()), Some("pc-2".into())), Ok("pc-1".into()));
            assert_eq!(which_agent(&p, None, Some("pc-2".into())), Ok("pc-2".into()));
            assert!(which_agent(&p, None, Some(String::new())).unwrap_err().contains(&p.env_name("AGENT")));
            assert!(which_agent(&p, None, None).unwrap_err().contains("--agent"));
            assert!(which_agent(&p, None, Some("../x".into())).is_err());
        }
    }

    /// `Exit`: what it reported is kept, and only a newly ready or pushed
    /// PR is news; `Stream` keeps no record.
    #[test]
    fn the_ready_watch_reports_as_the_project_does() {
        use crate::project::fixtures::GPU_SWARM;
        assert_eq!(P.ready_watch, ReadyWatch::Stream);
        let ReadyWatch::Exit { record } = GPU_SWARM.ready_watch else { panic!("the second project's watch exits") };
        let dir = swarm_core_testkit::temp_dir("swarm-core-prwatch-ready-record");
        let path = dir.join(record);
        assert!(load_ready(&path).is_empty(), "no record yet");
        std::fs::write(&path, "ready: #1 abc 1-x · a\n\nready: #2 def 2-y · b\n").unwrap();
        let seen = load_ready(&path);
        assert_eq!(seen.len(), 2);
        let now = ["ready: #1 abc 1-x · a".to_owned(), "ready: #2 0f0 2-y · b".to_owned()];
        assert_eq!(newly_ready(&seen, &now), ["ready: #2 0f0 2-y · b"], "#2 was pushed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_rows_read_with_what_they_have() {
        let m = Pr::from_api(&json!({"number": 7, "title": "T", "merge_state": "dirty", "checks": "failure", "comments": 3})).unwrap();
        assert_eq!(m, Pr { number: 7, title: "T".into(), merge_state: "dirty".into(), checks: Some("failure".into()), comments: 3 });
        assert_eq!(Pr::from_json(&m.to_json()), Some(m.clone()));
        assert_eq!(Pr::from_api(&json!({"number": 8, "merge_state": null})).map(|m| m.merge_state), Some("unknown".into()));
        assert!(Pr::from_api(&json!({"title": "no number"})).is_none());
        assert!(m.status().starts_with("#7 dirty, checks failure, 3 comments"));
    }
}
