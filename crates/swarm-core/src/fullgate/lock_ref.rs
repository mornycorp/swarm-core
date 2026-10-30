//! The full gates' lock: one ref on the remote, [`REF`], that only a
//! compare-and-swap moves (`git push --force-with-lease=<ref>:<expected>`). The tracking race
//! ([`super::race`]) orders starts by their timestamps, which each agent's own clock writes, so two
//! agents could each see their own start first and both run. The server takes one swap and refuses
//! the other, whatever the clocks say.
//!
//! The lock is a commit of the empty tree whose message names its holder ([`message`]). It is never
//! released: a holder whose gate stopped, or whose start is [`STALE`], is swapped out ([`free`]).

use std::path::Path;

use super::{FullGate, STALE, field, is_commit};
use crate::process::git;
use crate::stack::{self, short};
use crate::{claim, event};

/// The ref the full gates' lock lives at, on the remote: outside `refs/heads`, so no branch, PR or
/// workflow sees it.
pub const REF: &str = "refs/fullgate/lock";

/// The full gate that holds the lock, as its commit names it, and when it was taken (the taker's
/// clock: read only for a holder whose start isn't on the tracking branch).
#[derive(Debug, Clone, PartialEq)]
pub struct Holder {
    pub instance: String,
    pub sub: String,
    pub commit: String,
    pub t: i64,
}

/// The lock commit's message for `instance`'s full gate `sub` of `commit`.
pub fn message(instance: &str, sub: &str, commit: &str) -> String {
    format!("fullgate lock: {instance} {sub}\n\ninstance={instance}; sub={sub}; commit={commit}\n")
}

/// The holder a lock commit names, from `git log -1 --format=%ct%n%B`: its time, then its message.
/// `None` for a commit that isn't a lock's.
pub fn parse(log: &str) -> Option<Holder> {
    let (t, msg) = log.trim_start().split_once('\n')?;
    let get = |key| field(msg, key).map(str::to_owned);
    Some(Holder { instance: get("instance")?, sub: get("sub")?, commit: get("commit").filter(|c| is_commit(c))?, t: t.trim().parse().ok()? })
}

/// Whether `me`'s full gate `sub` of `main` may take the lock from `held` at `now`, by the full
/// gates on the tracking branch: when nobody holds it, `me` does, or the holder's gate stopped or
/// went stale. A holder that ran `main` already keeps it, so `main` isn't gated twice. `Err` says
/// who holds it.
pub fn free(held: Option<&Holder>, gates: &[FullGate], me: &str, sub: &str, main: &str, now: i64) -> Result<(), String> {
    let Some(h) = held else { return Ok(()) };
    if h.instance == me && h.sub == sub {
        return Ok(());
    }
    let what = format!("{}'s {} of {}", h.instance, h.sub, short(&h.commit));
    match gates.iter().find(|g| g.instance == h.instance && g.sub == h.sub) {
        Some(g) if g.open_at(now) => Err(format!("{what} holds the full gate's lock")),
        Some(g) if g.ran() && g.commit == main => Err(format!("{what} gated it already")),
        Some(_) => Ok(()),
        None if now.saturating_sub(h.t) < STALE => Err(format!("{what} holds the full gate's lock (its start not seen yet)")),
        None => Ok(()),
    }
}

/// The lock as `remote` has it now: its commit and the holder it names (`None` for a commit that
/// isn't a lock's); `None` when nobody ever took it.
pub fn read(root: &Path, remote: &str) -> Result<Option<(String, Option<Holder>)>, String> {
    let Some(tip) = event::fetch_tip(root, remote, REF)? else { return Ok(None) };
    let log = git(root, &["log", "-1", "--format=%ct%n%B", &tip], None, &[])?;
    Ok(Some((tip, parse(&log))))
}

/// Move the lock from `expected` (`None`: nobody took it) to a new commit naming `me`'s full gate
/// `sub` of `main`, and return that commit. The server refuses the swap when the lock moved since
/// it was read: another agent took it first.
pub fn swap(root: &Path, remote: &str, expected: Option<&str>, me: &str, sub: &str, main: &str) -> Result<String, String> {
    let email = "fullgate@swarm.invalid";
    let ident = [("GIT_AUTHOR_NAME", me), ("GIT_AUTHOR_EMAIL", email), ("GIT_COMMITTER_NAME", me), ("GIT_COMMITTER_EMAIL", email)];
    let tree = git(root, &["mktree"], Some(b""), &[])?;
    let commit = git(root, &["commit-tree", tree.trim(), "-m", &message(me, sub, main)], None, &ident)?;
    let commit = commit.trim();
    let lease = format!("--force-with-lease={REF}:{}", expected.unwrap_or_default());
    match stack::git_slow(root, &["push", "--quiet", "--no-verify", &lease, remote, &format!("{commit}:{REF}")], &[]) {
        Ok(_) => Ok(commit.to_owned()),
        Err(e) if e.contains("stale info") => Err("another agent took the full gate's lock first".to_owned()),
        Err(e) => Err(format!("the full gate's lock: {}", event::one_line(&e))),
    }
}

/// Take the lock for `me`'s full gate `sub` of `main`, by the full gates on the tracking branch
/// ([`free`]), or say who holds it.
pub fn take(root: &Path, remote: &str, gates: &[FullGate], me: &str, sub: &str, main: &str) -> Result<(), String> {
    let (tip, held) = read(root, remote)?.unzip();
    free(held.flatten().as_ref(), gates, me, sub, main, claim::now())?;
    swap(root, remote, tip.as_deref(), me, sub, main).map(drop)
}

#[cfg(test)]
mod tests {
    use super::super::End;
    use super::*;
    use swarm_core_testkit::git::{self as tgit, Remote};

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const NOW: i64 = 1_790_000_000;

    fn gate(instance: &str, sub: &str, commit: &str, started: i64, rc: Option<Option<i32>>) -> FullGate {
        let end = rc.map(|rc| End { t: started + 600, rc, cause: None });
        FullGate { instance: instance.into(), sub: sub.into(), commit: commit.into(), started, end }
    }

    fn holder(instance: &str, sub: &str, commit: &str, t: i64) -> Holder {
        Holder { instance: instance.into(), sub: sub.into(), commit: commit.into(), t }
    }

    /// The lock's commit names its holder, and reads back; a commit that isn't a lock's names none.
    #[test]
    fn a_lock_commit_names_its_holder() {
        let log = format!("{NOW}\n{}", message("w-1", "gate-full-aaaaaaa", A));
        assert_eq!(parse(&log), Some(holder("w-1", "gate-full-aaaaaaa", A, NOW)));
        for bad in ["", "123", "x\ninstance=w-1; sub=s; commit=aaaaaaa", &format!("{NOW}\ninstance=w-1; sub=s; commit=main"), &format!("{NOW}\nmerge main")] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    /// The lock is free when nobody holds it, its holder is this gate, or the holder's gate stopped
    /// or went stale; it is held while the holder's gate runs, or its start isn't seen yet, and a
    /// holder that ran this `main` keeps it.
    #[test]
    fn the_lock_is_free_once_its_holders_gate_stops() {
        let sub = "gate-full-bbbbbbb";
        let free_ = |held: Option<&Holder>, gates: &[FullGate]| free(held, gates, "w-2", sub, B, NOW);
        assert_eq!(free_(None, &[]), Ok(()), "nobody took it");
        assert_eq!(free(Some(&holder("w-2", sub, B, NOW)), &[], "w-2", sub, B, NOW), Ok(()), "this gate holds it");
        let h = holder("w-1", "gate-full-aaaaaaa", A, NOW - 60);
        let running = gate("w-1", "gate-full-aaaaaaa", A, NOW - 60, None);
        assert!(free_(Some(&h), std::slice::from_ref(&running)).unwrap_err().contains("w-1's gate-full-aaaaaaa of aaaaaaa holds"));
        assert!(free_(Some(&h), &[]).unwrap_err().contains("not seen yet"), "its start may still be on its way");
        assert_eq!(free_(Some(&holder("w-1", "gate-full-aaaaaaa", A, NOW - STALE)), &[]), Ok(()), "unseen and stale");
        assert_eq!(free_(Some(&h), &[gate("w-1", "gate-full-aaaaaaa", A, NOW - STALE, None)]), Ok(()), "a stale start");
        for rc in [Some(0), Some(1), None] {
            assert_eq!(free_(Some(&h), &[gate("w-1", "gate-full-aaaaaaa", A, NOW - 900, Some(rc))]), Ok(()), "stopped: {rc:?}");
        }
        let same = holder("w-1", "gate-full-bbbbbbb", B, NOW - 900);
        let ran = gate("w-1", "gate-full-bbbbbbb", B, NOW - 900, Some(Some(0)));
        assert!(free_(Some(&same), &[ran]).unwrap_err().contains("gated it already"), "main isn't gated twice");
        let superseded = gate("w-1", "gate-full-bbbbbbb", B, NOW - 900, Some(None));
        assert_eq!(free_(Some(&same), &[superseded]), Ok(()), "a superseded start never ran");
    }

    /// The server takes one swap and refuses the other: two clones that read the lock unheld both
    /// try to create it, and only the first does; a swap from the lock as it was read before it
    /// moved is refused.
    #[test]
    fn the_server_takes_one_swap_and_refuses_the_other() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-fullgate-lock-swap");
        let Remote { origin, work } = tgit::remote(&dir);
        let other = dir.join("other");
        tgit::git(&dir, &["clone", "--quiet", &origin.to_string_lossy(), &other.to_string_lossy()]);
        assert_eq!(read(&work, "origin").unwrap(), None, "nobody took it yet");
        let first = swap(&work, "origin", None, "w-1", "gate-full-aaaaaaa", A).unwrap();
        let e = swap(&other, "origin", None, "w-2", "gate-full-aaaaaaa", A).unwrap_err();
        assert!(e.contains("took the full gate's lock first"), "{e}");
        let (tip, held) = read(&other, "origin").unwrap().unwrap();
        assert_eq!(tip, first);
        assert_eq!(held.map(|h| (h.instance, h.sub, h.commit)), Some(("w-1".into(), "gate-full-aaaaaaa".into(), A.into())));
        let second = swap(&other, "origin", Some(&first), "w-2", "gate-full-bbbbbbb", B).unwrap();
        let e = swap(&work, "origin", Some(&first), "w-1", "gate-full-bbbbbbb", B).unwrap_err();
        assert!(e.contains("took the full gate's lock first"), "a stale lease: {e}");
        assert_eq!(read(&work, "origin").unwrap().map(|(tip, _)| tip), Some(second));
        assert!(tgit::git(&origin, &["for-each-ref", "refs/heads"]).lines().all(|l| !l.contains("fullgate")), "no branch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `take`: the first gate takes the lock; another's is refused while the first runs, and takes
    /// it once the first has stopped.
    #[test]
    fn a_gate_takes_the_lock_only_from_one_that_stopped() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-fullgate-lock-take");
        let Remote { work, .. } = tgit::remote(&dir);
        let now = claim::now();
        let a = gate("w-1", "gate-full-aaaaaaa", A, now, None);
        take(&work, "origin", std::slice::from_ref(&a), "w-1", "gate-full-aaaaaaa", A).unwrap();
        let b = gate("w-2", "gate-full-aaaaaaa", A, now - 5, None);
        let e = take(&work, "origin", &[a.clone(), b.clone()], "w-2", "gate-full-aaaaaaa", A).unwrap_err();
        assert!(e.contains("w-1's gate-full-aaaaaaa of aaaaaaa holds"), "an earlier clock wins nothing: {e}");
        let stopped = gate("w-1", "gate-full-aaaaaaa", A, now, Some(Some(0)));
        let next = gate("w-2", "gate-full-bbbbbbb", B, now, None);
        take(&work, "origin", &[stopped, next], "w-2", "gate-full-bbbbbbb", B).unwrap();
        let held = read(&work, "origin").unwrap().and_then(|(_, h)| h).unwrap();
        assert_eq!((held.instance.as_str(), held.commit.as_str()), ("w-2", B));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
