//! `cargo xtask lock take|drop|show [--agent <id>]`: the
//! coordinator's gate lock.
//!
//! One coordinator gates at a time, whether a live session or a scheduled run (docs/swarm-protocol.md ›
//! Staying awake). It takes the lock before it touches its gate worktree (a merge or a rebase is
//! already a write) and drops it when its merges are done.
//!
//! - **Where:** [`Project::gate_lock_file`] (`swarm-gate.lock`) in the clone's
//!   common git directory, so every worktree of the clone sees one lock. It holds one line: the
//!   holder's instance id, when it was taken, and the worktree it was taken in,
//!   so a command that would change that worktree's tree ([`held_elsewhere`], `cargo
//!   xtask claim`'s switch) refuses while another holder's gate runs there.
//! - **Take:** refused while another holder's lock is younger than the project's
//!   [`crate::project::Limits::gate_lock_max_age_secs`] (two hours, say,
//!   longer for a project whose GPU gate that waited for capacity runs long); the same holder may take it again
//!   (which renews it). A lock older than that was left behind and is taken over. A lock whose
//!   line can't be read is aged by the file's own time.
//! - **Drop:** only the holder's own, unless `--force`. `show` says who holds it and where, or
//!   `free`.
//!
//! The machine-wide gate lock both swarms share ([`crate::machine_lock`])
//! is the gate's own: `show` names its holder on a second line, and `drop` drops this
//! instance's (with `--force`, any instance's of this swarm; never the other swarm's), for a gate
//! that was killed while it held it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::Project;
use crate::event::{check_id, common_dir};
use crate::machine_lock;
use crate::process::git;
use crate::time::{iso_utc, now_secs, parse_time};

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask lock take|drop|show [--agent <instance>] [--force]";

/// A lock as read: who holds it, and since when (seconds since the epoch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock {
    pub holder: String,
    pub taken: Option<i64>,
    /// The worktree it was taken in; `None` in a line written before it was recorded.
    pub worktree: Option<String>,
}

/// The lock file's line: `<holder> <time> [<worktree>]`. The worktree is the rest of the line,
/// so a path with spaces reads whole.
pub fn parse(text: &str) -> Option<Lock> {
    let mut parts = text.trim().splitn(3, char::is_whitespace);
    let holder = parts.next().filter(|h| !h.is_empty())?.to_owned();
    let taken = parts.next().and_then(parse_time);
    let worktree = parts.next().map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned);
    Some(Lock { holder, taken, worktree })
}

/// The line written for `holder` at `now`, taken in `worktree`.
pub fn line(holder: &str, now: i64, worktree: &str) -> String {
    format!("{holder} {} {worktree}\n", iso_utc(u64::try_from(now).unwrap_or(0)))
}

/// Why `me` may not take `project`'s lock at `now`, if it may not: another holder's lock younger
/// than its maximum age. `taken` is the lock's time (its line's, else its file's).
pub fn refusal(project: &Project, lock: Option<&Lock>, taken: Option<i64>, me: &str, now: i64) -> Option<String> {
    let lock = lock?;
    if lock.holder == me {
        return None;
    }
    let max = project.limits.gate_lock_max_age_secs;
    let age = age(taken, now);
    (age < max).then(|| format!("gate lock held by {} ({}, younger than {} h): wait, or ask it to drop it", lock.holder, describe_age(age), max / 3600))
}

/// A lock's age at `now`, in seconds, from its time `taken`. No time at all, or one in the future,
/// is as good as new, so a lock nobody can date is never stolen.
pub(crate) fn age(taken: Option<i64>, now: i64) -> i64 {
    taken.map_or(0, |t| now.saturating_sub(t)).max(0)
}

/// `secs` as `1 h 05 min old` or `12 min old`.
pub fn describe_age(secs: i64) -> String {
    let m = secs / 60;
    if m >= 60 { format!("{} h {:02} min old", m / 60, m % 60) } else { format!("{m} min old") }
}

/// `project`'s lock file in the clone `root` is in.
pub fn path(project: &Project, root: &Path) -> PathBuf {
    common_dir(root).join(project.gate_lock_file)
}

/// Now, in seconds since the epoch.
pub(crate) fn now() -> i64 {
    i64::try_from(now_secs()).unwrap_or(i64::MAX)
}

/// The modification time of `file`, in seconds since the epoch.
pub(crate) fn mtime(file: &Path) -> Option<i64> {
    let t = std::fs::metadata(file).ok()?.modified().ok()?;
    i64::try_from(t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs()).ok()
}

/// The lock at `file`, if any, and its time: its line's, else the file's own.
fn read(file: &Path) -> Option<(Lock, Option<i64>)> {
    let text = std::fs::read_to_string(file).ok()?;
    // An empty or unreadable line is still a lock: of nobody, aged by the file.
    let lock = parse(&text).unwrap_or(Lock { holder: "?".to_owned(), taken: None, worktree: None });
    let taken = lock.taken.or_else(|| mtime(file));
    Some((lock, taken))
}

/// Take `project`'s lock at `file` for `me` at `now`, in `worktree`.
pub fn take_at(project: &Project, file: &Path, me: &str, now: i64, worktree: &str) -> Result<(), String> {
    let current = read(file);
    if let Some(why) = refusal(project, current.as_ref().map(|(l, _)| l), current.as_ref().and_then(|(_, t)| *t), me, now) {
        return Err(why);
    }
    // Written beside it, then renamed: a reader never sees half a line.
    let tmp = file.with_extension(format!("lock.{}", std::process::id()));
    std::fs::write(&tmp, line(me, now, worktree)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", file.display())
    })
}

/// Drop the lock at `file`: only `me`'s own, unless `force`. A free lock drops quietly.
pub fn drop_at(file: &Path, me: &str, force: bool) -> Result<(), String> {
    let Some((lock, _)) = read(file) else { return Ok(()) };
    if lock.holder != me && !force {
        return Err(format!("gate lock held by {}, not {me}: not dropped (--force drops another's)", lock.holder));
    }
    std::fs::remove_file(file).map_err(|e| format!("{}: {e}", file.display()))
}

/// Who holds `project`'s lock at `file`, at `now`: `free`, or the holder, where and its age.
pub fn show_at(project: &Project, file: &Path, now: i64) -> String {
    match read(file) {
        None => "free".to_owned(),
        Some((lock, taken)) => {
            let age = age(taken, now);
            let stale = if age >= project.limits.gate_lock_max_age_secs { "; left behind: anyone may take it" } else { "" };
            let place = lock.worktree.map(|w| format!(" in {w}")).unwrap_or_default();
            format!("held by {}{place} ({}{stale})", lock.holder, describe_age(age))
        }
    }
}

/// The worktree `root` is in, as the locks record it: its top level, canonical and `/`-separated.
pub fn worktree_of(root: &Path) -> String {
    let top = git(root, &["rev-parse", "--show-toplevel"], None, &[]).map_or_else(|_| root.to_path_buf(), |t| PathBuf::from(t.trim()));
    same_path(&top)
}

/// `p` canonical (when it can be) and `/`-separated, without Windows' verbatim `//?/` prefix, so
/// two spellings of one worktree compare equal.
fn same_path(p: &Path) -> String {
    let c = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let s = c.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_owned()
}

/// Why the worktree `worktree` must not change its tree now, if it must not: another holder's
/// live lock (younger than `project`'s maximum age) was taken in it, so its gate may be running
/// there.
pub fn held_elsewhere_at(project: &Project, lock: Option<&Lock>, taken: Option<i64>, me: &str, now: i64, worktree: &str) -> Option<String> {
    let lock = lock?;
    let age = age(taken, now);
    (lock.holder != me && age < project.limits.gate_lock_max_age_secs && lock.worktree.as_deref() == Some(worktree)).then(|| {
        format!(
            "{worktree} is the gate's worktree, held by {} ({}): only `cargo xtask stack`, `gate` and `land` change it (docs/swarm-protocol.md › Staying awake)",
            lock.holder,
            describe_age(age)
        )
    })
}

/// [`held_elsewhere_at`] for the worktree `root` is in, now.
pub fn held_elsewhere(project: &Project, root: &Path, me: &str) -> Option<String> {
    let (lock, taken) = read(&path(project, root))?;
    held_elsewhere_at(project, Some(&lock), taken, me, now(), &worktree_of(root))
}

/// Take `project`'s lock in the clone `root` is in, for `me`, now, recording `root`'s worktree.
pub fn take(project: &Project, root: &Path, me: &str) -> Result<(), String> {
    take_at(project, &path(project, root), me, now(), &worktree_of(root))
}

/// Drop `me`'s lock in the clone `root` is in.
pub fn drop(project: &Project, root: &Path, me: &str, force: bool) -> Result<(), String> {
    drop_at(&path(project, root), me, force)
}

/// `lock drop`: `me`'s machine hold first (`machine`, the machine lock's file), then `project`'s
/// repo lock at `file`, each said as it drops (`say`). The machine hold is this instance's own
/// whoever holds the repo's lock, so a repo lock held by another (without `force`) refuses after
/// the machine hold is gone, never keeping it (the order one consumer's copy had before the move, #1).
fn drop_holds(project: &Project, file: &Path, machine: Option<&Path>, me: &str, force: bool, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    if let Some(m) = machine
        && machine_lock::drop_at(m, project.name, me, force)?
    {
        say("machine gate lock dropped");
    }
    drop_at(file, me, force)?;
    say("gate lock dropped");
    Ok(())
}

/// The agent: `--agent` (`given`), else `project`'s `{prefix}_AGENT`, as `cargo xtask event`
/// takes it.
pub fn agent(project: &Project, given: Option<String>, what: &str) -> Result<String, String> {
    agent_from(project, given.or_else(|| project.env("AGENT")), what)
}

/// [`agent`] with the environment's value already read into `found`.
fn agent_from(project: &Project, found: Option<String>, what: &str) -> Result<String, String> {
    let a =
        found.filter(|a| !a.trim().is_empty()).ok_or_else(|| format!("{what}: which agent? pass --agent <instance> (or set {})", project.env_name("AGENT")))?;
    check_id(&a)?;
    Ok(a)
}

/// `lock`'s arguments after the action: `--agent` and `--force`.
fn options(rest: &[&str]) -> Result<(Option<String>, bool), String> {
    let mut given = None;
    let mut force = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--agent" => given = Some(it.next().ok_or("lock: --agent needs an instance")?.to_string()),
            "--force" => force = true,
            other => return Err(format!("lock: unknown argument `{other}`\nusage: {USAGE}")),
        }
    }
    Ok((given, force))
}

/// `cargo xtask lock …` ([`USAGE`]) in the clone `root` is in.
pub fn run(project: &Project, root: &Path, args: &[&str]) -> Result<(), String> {
    let (action, rest) = args.split_first().ok_or(format!("usage: {USAGE}"))?;
    let (given, force) = options(rest)?;
    let file = path(project, root);
    match *action {
        "take" => {
            let me = agent(project, given, "lock take")?;
            take_at(project, &file, &me, now(), &worktree_of(root))?;
            println!("gate lock taken by {me} ({})", file.display());
        }
        "drop" => {
            let me = agent(project, given, "lock drop")?;
            drop_holds(project, &file, machine_lock::path().as_deref(), &me, force, &mut |s| println!("{s}"))?;
        }
        "show" => println!("{}\nmachine: {}", show_at(project, &file, now()), machine_lock::show()),
        other => return Err(format!("lock: unknown action `{other}`\nusage: {USAGE}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::{APP_SWARM, BOTH, GPU_SWARM};
    use swarm_core_testkit::git::git as tgit;

    const T: i64 = 1_792_238_400; // 2026-10-17T12:00:00Z

    #[test]
    fn a_lock_line_reads_back() {
        for wt in ["D:/dev/app swarm-coord", "D:/dev/gpu-swarm gate"] {
            assert_eq!(parse(&line("coord-1", T, wt)), Some(Lock { holder: "coord-1".into(), taken: Some(T), worktree: Some(wt.into()) }));
        }
        assert_eq!(parse("coord-1 not-a-time"), Some(Lock { holder: "coord-1".into(), taken: None, worktree: None }));
        assert_eq!(parse("coord-1 1970-01-01T00:00:10Z").map(|l| l.worktree), Some(None), "a line from before the worktree was recorded");
        assert_eq!(parse("  \n"), None);
    }

    #[test]
    fn only_another_holders_young_lock_refuses() {
        let theirs = Lock { holder: "coord-1".into(), taken: Some(T), worktree: None };
        for p in &BOTH {
            let max = p.limits.gate_lock_max_age_secs;
            assert_eq!(refusal(p, None, None, "coord-2", T), None, "free");
            assert_eq!(refusal(p, Some(&theirs), Some(T), "coord-1", T + 60), None, "the same holder takes it again");
            let why = refusal(p, Some(&theirs), Some(T), "coord-2", T + 65 * 60).unwrap();
            assert!(why.contains("coord-1") && why.contains("1 h 05 min old") && why.contains(&format!("younger than {} h", max / 3600)), "{why}");
            assert!(refusal(p, Some(&theirs), Some(T), "coord-2", T + max - 1).is_some(), "{}: just young", p.name);
            assert_eq!(refusal(p, Some(&theirs), Some(T), "coord-2", T + max), None, "{}: left behind", p.name);
            assert!(refusal(p, Some(&theirs), None, "coord-2", T + 10 * max).is_some(), "a lock nobody can date is never stolen");
            assert!(refusal(p, Some(&theirs), Some(T + 600), "coord-2", T).is_some(), "a lock from the future is young");
        }
        // The two swarms' maximum ages, as each had them before the move.
        assert_eq!(refusal(&APP_SWARM, Some(&theirs), Some(T), "coord-2", T + 2 * 3600), None, "2 hours for the first project");
        let why = refusal(&GPU_SWARM, Some(&theirs), Some(T), "coord-2", T + 125 * 60).unwrap();
        assert!(why.contains("2 h 05 min old") && why.contains("3 h"), "three hours: {why}");
    }

    /// `lock drop` frees this instance's machine hold even when another agent holds the repo's lock
    /// and it refuses (#1): the machine hold is the instance's own.
    #[test]
    fn a_refused_repo_lock_still_drops_the_machine_hold() {
        for p in &BOTH {
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-lock-drop-{}", p.name));
            let (file, machine) = (dir.join(p.gate_lock_file), dir.join("gate.lock"));
            take_at(p, &file, "other-1", T, "D:/dev/other").unwrap();
            machine_lock::take_at(&machine, p.name, "me-1", T, "D:/dev/me").unwrap();
            let mut said = Vec::new();
            let e = drop_holds(p, &file, Some(&machine), "me-1", false, &mut |s| said.push(s.to_owned())).unwrap_err();
            assert!(e.contains("held by other-1"), "{e}");
            assert_eq!(said, ["machine gate lock dropped"], "{}", p.name);
            assert!(!machine_lock::drop_at(&machine, p.name, "me-1", false).unwrap(), "{}: the machine hold is gone", p.name);
            // Its own repo lock: both drop, the machine's first.
            drop_at(&file, "other-1", true).unwrap();
            take_at(p, &file, "me-1", T, "D:/dev/me").unwrap();
            machine_lock::take_at(&machine, p.name, "me-1", T, "D:/dev/me").unwrap();
            said.clear();
            drop_holds(p, &file, Some(&machine), "me-1", false, &mut |s| said.push(s.to_owned())).unwrap();
            assert_eq!(said, ["machine gate lock dropped", "gate lock dropped"], "{}", p.name);
        }
    }

    /// Through git: the lock lives in the common git directory, so a worktree of the clone sees the
    /// lock its main checkout took.
    #[test]
    fn every_worktree_of_a_clone_shares_the_lock() {
        for p in &BOTH {
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-lock-share-{}", p.name));
            let main = dir.join("main");
            tgit(&dir, &["init", "--quiet", main.to_str().unwrap()]);
            tgit(&main, &["commit", "--quiet", "--allow-empty", "-m", "root"]);
            let wt = dir.join("wt");
            tgit(&main, &["worktree", "add", "--quiet", "-b", "side", wt.to_str().unwrap()]);
            let max = p.limits.gate_lock_max_age_secs;
            let canon = |p: PathBuf| p.parent().unwrap().canonicalize().unwrap();
            assert_eq!(canon(path(p, &main)), canon(path(p, &wt)), "one lock for the clone");
            assert!(path(p, &main).ends_with(p.gate_lock_file), "{}", path(p, &main).display());
            assert_eq!(show_at(p, &path(p, &wt), T), "free");
            take_at(p, &path(p, &main), "coord-1", T, "D:/dev/main").unwrap();
            assert!(show_at(p, &path(p, &wt), T + 60).starts_with("held by coord-1 in D:/dev/main (1 min old"));
            assert!(take_at(p, &path(p, &wt), "coord-2", T + 60, "D:/dev/wt").unwrap_err().contains("held by coord-1"));
            assert!(drop_at(&path(p, &wt), "coord-2", false).unwrap_err().contains("not dropped"));
            take_at(p, &path(p, &wt), "coord-1", T + 120, "D:/dev/wt").unwrap();
            assert_eq!(parse(&std::fs::read_to_string(path(p, &main)).unwrap()).and_then(|l| l.taken), Some(T + 120), "taking it again renews it");
            assert!(show_at(p, &path(p, &main), T + 120 + max).ends_with("; left behind: anyone may take it)"));
            take_at(p, &path(p, &wt), "coord-2", T + 120 + max, "D:/dev/wt").unwrap();
            assert!(show_at(p, &path(p, &main), T + 120 + max).starts_with("held by coord-2"));
            drop_at(&path(p, &main), "coord-2", false).unwrap();
            assert_eq!(show_at(p, &path(p, &wt), T), "free");
            drop_at(&path(p, &main), "coord-2", false).unwrap();
            // A lock that can't be read is still a lock, aged by its file's time.
            std::fs::write(path(p, &main), "").unwrap();
            assert!(take_at(p, &path(p, &main), "coord-1", now(), "D:/dev/main").unwrap_err().contains("held by ?"));
            drop_at(&path(p, &main), "coord-1", true).unwrap();
            // The worktree as recorded: one spelling for the main checkout, from any directory in it.
            let sub = main.join("sub");
            std::fs::create_dir_all(&sub).unwrap();
            assert_eq!(worktree_of(&sub), worktree_of(&main));
            assert!(!worktree_of(&main).contains('\\') && !worktree_of(&main).starts_with("//?/"), "{}", worktree_of(&main));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn arguments_are_checked() {
        let root = Path::new(".");
        for p in &BOTH {
            assert!(run(p, root, &[]).unwrap_err().contains("usage"));
            assert!(run(p, root, &["grab"]).unwrap_err().contains("unknown action"));
            assert!(run(p, root, &["take", "--agent"]).unwrap_err().contains("needs an instance"));
            assert!(run(p, root, &["take", "--agent", "../x"]).is_err());
            assert!(run(p, root, &["show", "--bogus"]).unwrap_err().contains("unknown argument"));
        }
    }

    #[test]
    fn the_agent_is_given_or_the_projects_variable() {
        assert_eq!(agent_from(&APP_SWARM, Some("coord-1".into()), "lock take").unwrap(), "coord-1");
        assert!(agent_from(&APP_SWARM, None, "lock take").unwrap_err().contains("or set APP_SWARM_AGENT"));
        assert!(agent_from(&GPU_SWARM, Some(" ".into()), "gate").unwrap_err().contains("gate: which agent? pass --agent <instance> (or set GPU_SWARM_AGENT)"));
        for bad in ["../x", ".hidden", "a b", &"x".repeat(crate::event::MAX_ID + 1)] {
            assert!(agent_from(&GPU_SWARM, Some(bad.to_owned()), "lock").is_err(), "{bad}");
        }
        assert_eq!(agent(&GPU_SWARM, Some("sched-1".into()), "stack").unwrap(), "sched-1", "--agent wins over the environment");
    }

    #[test]
    fn a_live_lock_holds_the_worktree_it_was_taken_in_against_others() {
        for p in &BOTH {
            let max = p.limits.gate_lock_max_age_secs;
            let lock = parse(&line("sched-1", T, "D:/dev/gate")).unwrap();
            let held = held_elsewhere_at(p, Some(&lock), lock.taken, "coord-1", T + 60, "D:/dev/gate").unwrap();
            assert!(held.contains("D:/dev/gate is the gate's worktree, held by sched-1 (1 min old)"), "{held}");
            assert_eq!(held_elsewhere_at(p, Some(&lock), lock.taken, "sched-1", T + 60, "D:/dev/gate"), None, "the holder's own worktree");
            assert_eq!(held_elsewhere_at(p, Some(&lock), lock.taken, "coord-1", T + 60, "D:/dev/gate2"), None, "another worktree");
            assert!(held_elsewhere_at(p, Some(&lock), lock.taken, "coord-1", T + max - 1, "D:/dev/gate").is_some(), "just live");
            assert_eq!(held_elsewhere_at(p, Some(&lock), lock.taken, "coord-1", T + max, "D:/dev/gate"), None, "a lock left behind");
            let old = parse("sched-1 1970-01-01T00:00:10Z").unwrap();
            assert_eq!(held_elsewhere_at(p, Some(&old), old.taken, "coord-1", 70, "D:/dev/gate"), None, "an old line names no worktree");
            assert_eq!(held_elsewhere_at(p, None, None, "coord-1", T, "D:/dev/gate"), None, "no lock");
        }
    }

    #[test]
    fn ages_read_as_people_say_them() {
        assert_eq!(describe_age(0), "0 min old");
        assert_eq!(describe_age(59 * 60 + 59), "59 min old");
        assert_eq!(describe_age(65 * 60), "1 h 05 min old");
        assert_eq!(age(None, T), 0);
        assert_eq!(age(Some(T + 5), T), 0, "the future is new");
        assert_eq!(age(Some(i64::MIN), i64::MAX), i64::MAX, "no overflow on a hostile time");
    }
}
