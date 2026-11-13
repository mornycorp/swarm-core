//! The claim's hold on the tracking branch (mornycorp/swarm-core#96), where the project turns
//! it on ([`crate::Hooks::claim_holds_first`]): once its checks pass, and before its branch, its
//! PR, its label or its comment, a claim logs its `claim` event, pushed only onto the tracking
//! branch it read and found free.
//!
//! - **One holder:** git refuses a push that isn't a fast-forward, so of two claims of one issue
//!   at once only one lands. The other's push is refused; it reads the branch again
//!   ([`event::send_unless`]), finds the first one's claim there and is refused, naming who holds
//!   the issue ([`judge`]). It has made nothing else yet, so there is nothing to undo. On
//!   2026-11-13 two issues of one swarm were each claimed twice within a minute: each pair read
//!   the forge before either had made anything, so both went ahead.
//! - **Free:** no other instance has a live claim of the issue on the branch
//!   ([`super::live_claims`]: a `claim` or `work` in the last 24 hours with no later `stop`,
//!   `release` or `merge`). A claim is the claimant's own only by its instance id: two sessions
//!   under one agent name are two claimants.
//! - **Run again:** the claimant's own live claim is kept, and nothing is sent ([`Held::Already`]).
//! - **`--force`:** sent whoever holds the issue.
//! - **Its PR** goes on the `work` event the claim logs once the PR exists
//!   ([`super::work_event`]): the claim event comes first, so it names none.
//! - **Tracking off** (`<PREFIX>_TRACKING=off`): no hold ([`Held::Off`]), as no events.
//!
//! A hold that can't be pushed (offline, say) is an error, never queued: the claim stops before
//! it makes anything.

use std::path::Path;

use super::{Holder, LiveClaim, describe, live_claims, parse_tracking, tracking_files_at};
use crate::Project;
use crate::event::{self, Verdict};

/// What [`take`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// The claim landed on the tracking branch.
    Now,
    /// The claimant's own live claim was there already: a claim run again.
    Already,
    /// Tracking is off (`<PREFIX>_TRACKING=off`): no hold.
    Off,
}

/// Whether a claim of `issue` by the instance `agent` at `now` goes on the tracking branch whose
/// `files` (instance, `work.jsonl` text) are these: refused when another instance holds it (unless
/// `force`), skipped when `agent` holds it already, else sent.
pub fn judge(project: &Project, files: &[(String, String)], issue: u64, agent: &str, now: i64, force: bool) -> Verdict {
    let (mine, others): (Vec<LiveClaim>, Vec<LiveClaim>) = live_claims(&parse_tracking(files), issue, now).into_iter().partition(|c| c.instance == agent);
    if !others.is_empty() && !force {
        let list: String = others.into_iter().map(|c| format!("\n  - {}", describe(project.labels.in_progress, &Holder::Tracking(c), now))).collect();
        return Verdict::Refuse(format!(
            "claim: #{issue} is held by another agent on the tracking branch:{list}\nPick another issue. (Its claim landed first; yours made nothing.)"
        ));
    }
    if mine.is_empty() { Verdict::Send } else { Verdict::Skip }
}

/// Hold `issue` for the instance `agent` on `project`'s tracking branch, from the clone at `root`:
/// its `claim` event, sent only onto a branch [`judge`] finds free. An error names the holder, or
/// why the hold couldn't be pushed.
pub fn take(project: &Project, root: &Path, agent: &str, issue: u64, force: bool) -> Result<Held, String> {
    if project.env("TRACKING").is_some_and(|v| v.eq_ignore_ascii_case("off")) {
        return Ok(Held::Off);
    }
    // Events queued earlier (this session's `session.start`, say) go first, so the branch names
    // whose the hold is.
    let queue = event::queue_path(project, root);
    if queue.exists()
        && let Err(e) = event::send(project, root, &queue, &[])
    {
        eprintln!("warning: claim: queued events not sent ({e}); they stay queued");
    }
    let n = issue.to_string();
    let (_, claim) = event::parse(project, &["claim", "--agent", agent, "--issue", &n], None, String::new)?.ok_or("claim: no claim event")?;
    let line = claim.line(&crate::time::now_iso());
    let decide = |tip: Option<&str>| -> Result<Verdict, String> {
        let files = match tip {
            Some(t) => tracking_files_at(root, t)?,
            None => Vec::new(),
        };
        Ok(judge(project, &files, issue, agent, super::now(), force))
    };
    let verdict = event::send_unless(project, root, agent, &line, &decide)
        .map_err(|e| format!("claim: couldn't hold #{issue} on the tracking branch ({e}); nothing was made: run the claim again"))?;
    match verdict {
        Verdict::Send => Ok(Held::Now),
        Verdict::Skip => Ok(Held::Already),
        Verdict::Refuse(why) => Err(why),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    use super::*;
    use crate::claim::tracking_files;
    use crate::event::tests::{Point, at, pending};
    use crate::process::git;
    use crate::project::fixtures::APP_SWARM;

    /// The app swarm's settings, under a prefix no environment sets (so not its `TRACKING=off`).
    const P: Project = Project { env_prefix: "SWARM_CORE_TEST_NEVER_SET", ..APP_SWARM };

    /// A bare origin in a fresh directory, and `agents` clones of it, each an agent on its own
    /// machine that has logged its `session.start` as `(instance, name)`.
    fn origin_and_clones(name: &str, agents: &[(&str, &str)]) -> (PathBuf, Vec<PathBuf>) {
        let dir = swarm_core_testkit::temp_dir(&format!("core-hold-{name}"));
        let remote = dir.join("origin.git");
        git(&dir, &["init", "--quiet", "--bare", remote.to_str().unwrap()], None, &[]).unwrap();
        let mut clones = Vec::new();
        for (instance, agent) in agents {
            let clone = dir.join(instance);
            git(&dir, &["init", "--quiet", clone.to_str().unwrap()], None, &[]).unwrap();
            git(&clone, &["remote", "add", "origin", remote.to_str().unwrap()], None, &[]).unwrap();
            let start = ["session.start", "--agent", instance, "--name", agent, "--role", "worker"];
            let (_, e) = event::parse(&P, &start, None, || "pc".into()).unwrap().unwrap();
            event::send(&P, &clone, &dir.join(format!("{instance}.queue")), &[((*instance).to_owned(), e.line(&crate::time::now_iso()))]).unwrap();
            clones.push(clone);
        }
        (dir, clones)
    }

    /// The instances holding `issue` on the origin's tracking branch now, read from `clone`.
    fn holders_of(clone: &Path, issue: u64) -> Vec<String> {
        live_claims(&parse_tracking(&tracking_files(&P, clone).unwrap()), issue, crate::claim::now()).into_iter().map(|c| c.instance).collect()
    }

    /// mornycorp/swarm-core#96: two claims of one issue at once. Alder's claim reads the
    /// branch, finds the issue free and commits its claim; before it pushes, birch's claim lands.
    /// Alder's push is refused, it reads the branch again and is refused naming birch. Exactly one
    /// holder, every time: the hook puts birch's push in that window rather than racing threads
    /// until it happens.
    #[test]
    fn of_two_claims_at_once_one_holds_and_the_other_is_refused_naming_it() {
        let (dir, clones) = origin_and_clones("race", &[("pc-a1", "alder"), ("pc-b2", "birch")]);
        let (alder, birch) = (clones[0].clone(), clones[1].clone());
        let rival = birch.clone();
        at(Point::Push, &alder, move || assert_eq!(take(&P, &rival, "pc-b2", 7, false), Ok(Held::Now)));
        let refused = take(&P, &alder, "pc-a1", 7, false).unwrap_err();
        assert_eq!(pending(), 0, "birch claimed between alder's read and its push");
        assert!(refused.contains("#7 is held") && refused.contains("birch (pc-b2): `claim`"), "{refused}");
        assert_eq!(holders_of(&alder, 7), ["pc-b2"]);
        // Alder logged nothing: its file has only its session's start.
        let files = tracking_files(&P, &birch).unwrap();
        let mine = files.iter().find(|(i, _)| i == "pc-a1").map(|(_, t)| t.lines().count());
        assert_eq!(mine, Some(1), "{files:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same race between two threads, each a clone of its own, let go at once: whichever
    /// lands, exactly one holds the issue and the other is refused naming it. Every wait is
    /// bounded: the git calls by their deadlines, the results by a timeout.
    #[test]
    fn two_claims_racing_from_two_clones_leave_one_holder() {
        let agents = [("pc-a1", "alder"), ("pc-b2", "birch")];
        let (dir, clones) = origin_and_clones("threads", &agents);
        for issue in [11, 12, 13] {
            let start = Arc::new(Barrier::new(2));
            let (tx, rx) = mpsc::channel();
            for ((instance, _), clone) in agents.iter().zip(&clones) {
                let (start, tx, clone, instance) = (Arc::clone(&start), tx.clone(), clone.clone(), *instance);
                std::thread::spawn(move || {
                    start.wait();
                    let _ = tx.send((instance, take(&P, &clone, instance, issue, false)));
                });
            }
            let mut results: Vec<(&str, Result<Held, String>)> = (0..2).map(|_| rx.recv_timeout(Duration::from_secs(600)).expect("a claim ended")).collect();
            results.sort_by_key(|(_, r)| r.is_err());
            let [(winner, won), (loser, lost)] = [&results[0], &results[1]];
            assert_eq!(won, &Ok(Held::Now), "#{issue}: {results:?}");
            let why = lost.as_ref().expect_err("one claim is refused");
            assert!(why.contains(&format!("({winner})")) && !why.contains(&format!("({loser})")), "#{issue}: {why}");
            assert_eq!(holders_of(&clones[0], issue), [*winner], "#{issue}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A claim run again keeps its own hold and sends nothing; `--force` holds beside another; a
    /// released claim holds nothing.
    #[test]
    fn a_hold_is_kept_on_a_rerun_forced_past_another_and_ended_by_a_release() {
        let (dir, clones) = origin_and_clones("rerun", &[("pc-a1", "alder"), ("pc-b2", "birch"), ("pc-c3", "cedar")]);
        let (alder, birch, cedar) = (&clones[0], &clones[1], &clones[2]);
        let commits = || git(alder, &["ls-remote", "origin", "refs/heads/tracking"], None, &[]).unwrap();
        assert_eq!(take(&P, alder, "pc-a1", 5, false), Ok(Held::Now));
        let after = commits();
        assert_eq!(take(&P, alder, "pc-a1", 5, false), Ok(Held::Already));
        assert_eq!(commits(), after, "a rerun sends nothing");
        assert!(take(&P, birch, "pc-b2", 5, false).unwrap_err().contains("alder (pc-a1)"));
        assert_eq!(take(&P, birch, "pc-b2", 5, true), Ok(Held::Now), "--force");
        let release = ["release", "--agent", "pc-a1", "--issue", "5"];
        let (_, e) = event::parse(&P, &release, None, String::new).unwrap().unwrap();
        event::send(&P, alder, &dir.join("a.queue"), &[("pc-a1".into(), e.line(&crate::time::now_iso()))]).unwrap();
        let why = take(&P, cedar, "pc-c3", 5, false).unwrap_err();
        assert!(why.contains("birch (pc-b2)") && !why.contains("pc-a1"), "alder released it: {why}");
        assert_eq!(holders_of(cedar, 5), ["pc-b2"]);
        assert_eq!(judge(&P, &[], 9, "pc-c3", crate::claim::now(), false), Verdict::Send, "no branch yet: free");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
