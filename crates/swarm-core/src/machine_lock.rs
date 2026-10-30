//! The machine-wide gate lock: one gate builds at a time on the
//! machine, whichever swarm it belongs to, and a gate's build jobs follow free memory.
//!
//! The repo's gate lock ([`crate::lock`]) lives in the clone's git directory, so it only excludes
//! that swarm's gates. On 2026-10-21 one swarm's gate linked with 22 `link.exe` processes
//! holding about 30 GB of the owner's 32 GB desktop; had the other swarm's gate been linking too, both
//! would have failed.
//!
//! - **Where:** `swarm-gate/gate.lock` under the user's local state directory (`%LOCALAPPDATA%` on
//!   Windows, else `$XDG_STATE_HOME` or `~/.local/state`), outside either repo.
//!   `SWARM_GATE_LOCK` names another file (tests). One file for both swarms.
//! - **The line**, the same in both swarms (a cross-repo contract):
//!   `<swarm> <instance> <time> <worktree>`: the swarm ([`Project::name`], such as
//!   `app-swarm`), its instance id, ISO-8601 UTC, and the rest of the line the worktree the
//!   gate runs in.
//! - **Who:** `cargo xtask gate` holds it while it gates ([`Guard`]), and drops it when its `ci`
//!   ends, pass or fail: the build is what fills the machine. Another holder (the other swarm, or
//!   another instance) is waited for with `--wait`. A lock older than [`STALE_SECS`] was left
//!   behind and is taken over; the same instance may take it again.
//! - **Taken atomically:** the line is written beside the lock, then hard-linked to its name,
//!   which fails when the file exists: of two gates that find it free at once, one gets it. A
//!   lock that can't be read is still a lock, aged by its file's time.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::Project;
use crate::lock::{age, describe_age, mtime, now, worktree_of};
use crate::time::{iso_utc, parse_time};

/// A machine lock this old (seconds) was left behind. One value for both swarms, not a
/// [`crate::project::Limits`] setting: the file is shared, so a per-project age would let one
/// swarm take over a lock the other still counts as live. Three hours, as both had it before the
/// move (the longer of the two repo gate locks' ages: a GPU gate that waited for capacity).
pub const STALE_SECS: i64 = 3 * 3600;
/// How long a gate waits for another gate, at most, and how often it looks.
const WAIT_MAX: Duration = Duration::from_secs(3 * 3600);
const WAIT_STEP: Duration = Duration::from_secs(60);
/// A gate's build gets one job per this much free memory (bytes, decimal GB): a test binary's link
/// took about 1.4 GB on the owner's desktop on 2026-10-21.
pub const BYTES_PER_JOB: u64 = 1_500_000_000;

/// A machine lock as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub swarm: String,
    pub instance: String,
    /// When it was taken (seconds since the epoch): its line's time, else its file's.
    pub taken: Option<i64>,
    pub worktree: Option<String>,
}

/// The lock file: `SWARM_GATE_LOCK`, else `swarm-gate/gate.lock` in the user's local state
/// directory; `None` when no such directory can be named.
pub fn path() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(p) = var("SWARM_GATE_LOCK") {
        return Some(p);
    }
    let base = if cfg!(windows) { var("LOCALAPPDATA") } else { var("XDG_STATE_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("state"))) };
    base.map(|b| b.join("swarm-gate").join("gate.lock"))
}

/// The line `<swarm> <instance> <time> <worktree>`; `None` for an empty one.
pub fn parse(text: &str) -> Option<Held> {
    let mut parts = text.trim().splitn(4, char::is_whitespace);
    let swarm = parts.next().filter(|s| !s.is_empty())?.to_owned();
    let instance = parts.next().unwrap_or("?").to_owned();
    let taken = parts.next().and_then(parse_time);
    let worktree = parts.next().map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned);
    Some(Held { swarm, instance, taken, worktree })
}

/// The line written for `instance` of `swarm` at `now`, in `worktree`.
pub fn line(swarm: &str, instance: &str, now: i64, worktree: &str) -> String {
    format!("{swarm} {instance} {} {worktree}\n", iso_utc(u64::try_from(now).unwrap_or(0)))
}

/// Why `instance` of `swarm` may not take the machine lock at `now`, if it may not: another
/// holder's lock younger than [`STALE_SECS`].
pub fn refusal(held: Option<&Held>, swarm: &str, instance: &str, now: i64) -> Option<String> {
    let h = held?;
    if h.swarm == swarm && h.instance == instance {
        return None;
    }
    let age = age(h.taken, now);
    (age < STALE_SECS).then(|| format!("machine gate lock held by {}'s {} ({})", h.swarm, h.instance, describe_age(age)))
}

/// The lock at `file`, if there is one. A line that can't be read is a lock of nobody's (`?`),
/// aged by the file's time.
fn read(file: &Path) -> Option<Held> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut held = parse(&text).unwrap_or(Held { swarm: "?".to_owned(), instance: "?".to_owned(), taken: None, worktree: None });
    held.taken = held.taken.or_else(|| mtime(file));
    Some(held)
}

/// Take the machine lock at `file` for `instance` of `swarm` at `now`, in `worktree`.
pub fn take_at(file: &Path, swarm: &str, instance: &str, now: i64, worktree: &str) -> Result<(), String> {
    let held = read(file);
    if let Some(why) = refusal(held.as_ref(), swarm, instance, now) {
        return Err(why);
    }
    match put(file, &line(swarm, instance, now, worktree), held.is_some()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(read(file)
            .and_then(|h| refusal(Some(&h), swarm, instance, now))
            .unwrap_or_else(|| "machine gate lock taken by another gate just now".to_owned())),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// Put the lock `text` at `file`, whole: written beside it, then hard-linked to its name, which
/// fails (`AlreadyExists`) when the file is there. `replace` removes the file first (ours to
/// renew, or left behind); another gate's that goes in meanwhile still wins.
fn put(file: &Path, text: &str, replace: bool) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if replace
        && let Err(e) = std::fs::remove_file(file)
        && e.kind() != ErrorKind::NotFound
    {
        return Err(e);
    }
    let tmp = file.with_extension(format!("lock.{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    let linked = std::fs::hard_link(&tmp, file);
    let _ = std::fs::remove_file(&tmp);
    linked
}

/// Drop the machine lock at `file` when `instance` of `swarm` holds it, or, with `force`, when
/// any instance of `swarm` does; whether it was dropped. Another swarm's lock always stays.
pub fn drop_at(file: &Path, swarm: &str, instance: &str, force: bool) -> Result<bool, String> {
    match read(file) {
        Some(h) if h.swarm == swarm && (h.instance == instance || force) => {
            std::fs::remove_file(file).map(|()| true).map_err(|e| format!("{}: {e}", file.display()))
        }
        _ => Ok(false),
    }
}

/// Who holds the machine lock at `file`, at `now`: `free`, or the holder, its worktree and age.
pub fn show_at(file: &Path, now: i64) -> String {
    match read(file) {
        None => "free".to_owned(),
        Some(h) => {
            let age = age(h.taken, now);
            let stale = if age >= STALE_SECS { "; left behind: anyone may take it" } else { "" };
            let place = h.worktree.as_deref().map(|w| format!(" in {w}")).unwrap_or_default();
            format!("held by {}'s {}{place} ({}{stale})", h.swarm, h.instance, describe_age(age))
        }
    }
}

/// An instance's hold on the machine lock, dropped when it goes out of scope, so a gate that
/// stops on any error never leaves it behind. No lock file (no state directory) holds nothing.
#[derive(Debug)]
pub struct Guard {
    file: Option<PathBuf>,
    swarm: &'static str,
    instance: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(file) = &self.file
            && let Err(e) = drop_at(file, self.swarm, &self.instance, false)
        {
            eprintln!("warning: gate: machine gate lock: {e}");
        }
    }
}

/// Take the machine lock for `instance` of `project`, recording the worktree `worktree` is in (as
/// [`worktree_of`] spells it): at once, or, with `wait`, once the other holder is done (up to
/// [`WAIT_MAX`]). No state directory to put it in is no lock, with a warning: the gate goes on as
/// before.
pub fn take(project: &Project, instance: &str, worktree: &Path, wait: bool) -> Result<Guard, String> {
    hold(path(), project.name, instance, &worktree_of(worktree), wait)
}

/// [`take`] at `file` (`None`: no lock to be had).
fn hold(file: Option<PathBuf>, swarm: &'static str, instance: &str, worktree: &str, wait: bool) -> Result<Guard, String> {
    let Some(file) = file else {
        eprintln!("warning: gate: no local state directory for the machine gate lock; going on without it");
        return Ok(Guard { file: None, swarm, instance: instance.to_owned() });
    };
    let started = Instant::now();
    loop {
        match take_at(&file, swarm, instance, now(), worktree) {
            Ok(()) => return Ok(Guard { file: Some(file), swarm, instance: instance.to_owned() }),
            Err(why) if wait && started.elapsed() < WAIT_MAX => {
                if started.elapsed() < WAIT_STEP {
                    eprintln!("gate: {why}: waiting (every {} s)", WAIT_STEP.as_secs());
                }
                std::thread::sleep(WAIT_STEP);
            }
            Err(why) => return Err(format!("gate: {why}; --wait waits for it")),
        }
    }
}

/// Drop `instance`'s machine lock (with `force`, any instance's of `project`); whether one was
/// dropped.
pub fn drop(project: &Project, instance: &str, force: bool) -> Result<bool, String> {
    path().map_or(Ok(false), |file| drop_at(&file, project.name, instance, force))
}

/// `lock show`'s line for the machine lock.
pub fn show() -> String {
    path().map_or_else(|| "no local state directory".to_owned(), |file| format!("{} ({})", show_at(&file, now()), file.display()))
}

/// How many build jobs a gate's `ci` gets with `free` bytes of memory available on a machine
/// with `cores` logical cores: one per [`BYTES_PER_JOB`], at least 2 and at most `cores`.
/// Unknown memory: half the cores.
pub fn jobs_for(free: Option<u64>, cores: u64) -> u64 {
    let cores = cores.max(2);
    match free {
        Some(bytes) => (bytes / BYTES_PER_JOB).clamp(2, cores),
        None => (cores / 2).max(2),
    }
}

/// The `CARGO_BUILD_JOBS` a gate sets for its `ci` ([`jobs_for`]); `None` when the environment
/// already sets it (`set`), which wins.
pub fn build_jobs(set: Option<&str>, free: Option<u64>, cores: u64) -> Option<String> {
    match set {
        Some(v) if !v.trim().is_empty() => None,
        _ => Some(jobs_for(free, cores).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::BOTH;

    const T: i64 = 1_792_500_000;

    /// This project's swarm name and the other's.
    fn names(p: &Project) -> (&'static str, &'static str) {
        let other = BOTH.iter().map(|q| q.name).find(|n| *n != p.name).unwrap();
        (p.name, other)
    }

    fn files_in(dir: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name()).collect()
    }

    #[test]
    fn the_line_is_both_swarms_format_and_reads_back() {
        let l = line("gpu-swarm", "coord-1", T, "D:/dev/gpu-swarm gate");
        assert!(l.starts_with("gpu-swarm coord-1 20") && l.ends_with(" D:/dev/gpu-swarm gate\n"), "{l}");
        assert_eq!(
            parse(&l),
            Some(Held { swarm: "gpu-swarm".into(), instance: "coord-1".into(), taken: Some(T), worktree: Some("D:/dev/gpu-swarm gate".into()) })
        );
        let l = line("app-swarm", "coord-1", T, "D:/dev/app swarm-coord");
        assert!(l.starts_with("app-swarm coord-1 20"), "{l}");
        assert_eq!(
            parse(&l),
            Some(Held { swarm: "app-swarm".into(), instance: "coord-1".into(), taken: Some(T), worktree: Some("D:/dev/app swarm-coord".into()) })
        );
        // The line exactly as both swarms' copies wrote it before the move.
        assert_eq!(line("gpu-swarm", "coord-9", 1_792_591_200, "D:/dev/si"), "gpu-swarm coord-9 2026-10-21T14:00:00Z D:/dev/si\n");
        let theirs = parse("app-swarm coord-9 2026-10-21T14:00:00Z D:/dev/app-swarm-coord\n").unwrap();
        assert_eq!((theirs.swarm.as_str(), theirs.instance.as_str()), ("app-swarm", "coord-9"));
        assert_eq!(theirs.taken, parse_time("2026-10-21T14:00:00Z"));
        assert_eq!(parse("gpu-swarm coord-9").map(|h| (h.instance, h.taken)), Some(("coord-9".into(), None)));
        assert_eq!(parse("  \n"), None);
    }

    #[test]
    fn another_gates_live_lock_refuses_and_a_stale_one_is_taken_over() {
        for p in &BOTH {
            let (me, other) = names(p);
            let theirs = parse(&line(other, "coord-9", T, "D:/dev/other")).unwrap();
            let why = refusal(Some(&theirs), me, "coord-1", T + 120).unwrap();
            assert!(why.contains(&format!("held by {other}'s coord-9 (2 min old")), "{why}");
            assert!(refusal(Some(&theirs), me, "coord-1", T + STALE_SECS - 1).is_some(), "just live");
            assert!(refusal(Some(&theirs), me, "coord-1", T + STALE_SECS).is_none(), "left behind");
            assert!(refusal(Some(&theirs), other, "coord-9", T + 120).is_none(), "its own holder");
            let ours = parse(&line(me, "coord-1", T, "D:/dev/gate")).unwrap();
            assert!(refusal(Some(&ours), me, "coord-2", T + 60).is_some(), "another instance of this swarm");
            assert!(refusal(Some(&ours), other, "coord-1", T + 60).is_some(), "the same id in the other swarm");
            assert!(refusal(None, me, "coord-1", T).is_none(), "free");
            let future = Held { taken: Some(T + 600), ..ours };
            assert!(refusal(Some(&future), me, "coord-2", T).is_some(), "a lock from the future is young");
        }
    }

    #[test]
    fn take_drop_and_show_through_a_real_file() {
        for p in &BOTH {
            let (me, other) = names(p);
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-machine-lock-file-{me}"));
            let file = dir.join("swarm-gate").join("gate.lock");
            assert_eq!(show_at(&file, T), "free");
            take_at(&file, other, "coord-9", T, "D:/dev/other gate").unwrap();
            assert!(take_at(&file, me, "coord-1", T + 60, "D:/dev/gate").unwrap_err().contains(&format!("held by {other}'s coord-9")));
            assert_eq!(show_at(&file, T + 60), format!("held by {other}'s coord-9 in D:/dev/other gate (1 min old)"));
            assert!(show_at(&file, T + STALE_SECS).ends_with("; left behind: anyone may take it)"));
            assert!(!drop_at(&file, me, "coord-1", false).unwrap());
            assert!(!drop_at(&file, me, "coord-1", true).unwrap(), "a drop, even forced, never removes the other swarm's lock");
            assert!(file.exists());
            // Left behind: taken over.
            take_at(&file, me, "coord-1", T + STALE_SECS, "D:/dev/gate").unwrap();
            assert!(show_at(&file, T + STALE_SECS).starts_with(&format!("held by {me}'s coord-1 in D:/dev/gate")));
            take_at(&file, me, "coord-1", T + STALE_SECS + 60, "D:/dev/gate").unwrap();
            assert_eq!(read(&file).and_then(|h| h.taken), Some(T + STALE_SECS + 60), "the holder takes it again, renewing it");
            assert!(!drop_at(&file, me, "coord-2", false).unwrap(), "another instance's stays");
            assert!(drop_at(&file, me, "coord-2", true).unwrap(), "--force drops another instance's of this swarm");
            assert_eq!(show_at(&file, T), "free");
            assert!(!drop_at(&file, me, "coord-1", false).unwrap(), "a free lock drops quietly");
            // A lock that can't be read is still a lock, aged by its file's time.
            std::fs::write(&file, "").unwrap();
            assert!(take_at(&file, me, "coord-1", now(), "D:/dev/gate").unwrap_err().contains("held by ?'s ?"));
            let left = files_in(file.parent().unwrap());
            assert_eq!(left.len(), 1, "no temporary file is left: {left:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Two gates that both found the lock free: the one whose lock went in first keeps it.
    #[test]
    fn a_lock_that_went_in_after_the_read_is_never_overwritten() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-machine-lock-race");
        let file = dir.join("gate.lock");
        let theirs = line("app-swarm", "coord-9", T, "D:/dev/si");
        put(&file, &theirs, false).unwrap();
        // This gate read the lock as free before theirs went in: its put must not replace it.
        let e = put(&file, &line("gpu-swarm", "coord-1", T, "D:/dev/gate"), false).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs, "theirs stays whole");
        let left = files_in(&dir);
        assert_eq!(left.len(), 1, "no temporary file is left: {left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The guard writes the project's name, and its drop removes its own lock, never another's.
    #[test]
    fn a_guard_holds_the_lock_until_it_goes() {
        for p in &BOTH {
            let (me, other) = names(p);
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-machine-lock-guard-{me}"));
            let file = dir.join("swarm-gate").join("gate.lock");
            let guard = hold(Some(file.clone()), p.name, "coord-1", "D:/dev/gate", false).unwrap();
            assert_eq!(read(&file).map(|h| (h.swarm, h.instance)), Some((me.to_owned(), "coord-1".to_owned())));
            let err = hold(Some(file.clone()), other, "coord-9", "D:/dev/other", false).unwrap_err();
            assert!(err.starts_with(&format!("gate: machine gate lock held by {me}'s coord-1")) && err.ends_with("; --wait waits for it"), "{err}");
            std::mem::drop(guard);
            assert!(!file.exists(), "dropped with its guard");
            // A guard whose lock was taken over leaves the new holder's alone.
            let guard = hold(Some(file.clone()), p.name, "coord-1", "D:/dev/gate", false).unwrap();
            std::fs::write(&file, line(other, "coord-9", T, "D:/dev/other")).unwrap();
            std::mem::drop(guard);
            assert_eq!(read(&file).map(|h| h.swarm), Some(other.to_owned()));
            // No state directory: no lock, and the gate goes on.
            std::mem::drop(hold(None, p.name, "coord-1", "D:/dev/gate", false).unwrap());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn jobs_follow_free_memory_within_two_and_the_cores() {
        let gb = 1_000_000_000;
        assert_eq!(jobs_for(Some(30 * gb), 32), 20);
        assert_eq!(jobs_for(Some(12 * gb), 32), 8);
        assert_eq!(jobs_for(Some(2 * gb), 32), 2, "never below 2");
        assert_eq!(jobs_for(Some(100 * gb), 16), 16, "never above the cores");
        assert_eq!(jobs_for(Some(u64::MAX), 0), 2, "hostile numbers");
        assert_eq!(jobs_for(None, 32), 16, "unknown memory: half the cores");
        assert_eq!(jobs_for(None, 1), 2);
        assert_eq!(build_jobs(None, Some(12 * gb), 32).as_deref(), Some("8"));
        assert_eq!(build_jobs(Some("4"), Some(12 * gb), 32), None, "the environment's own wins");
        assert_eq!(build_jobs(Some(" "), Some(12 * gb), 32).as_deref(), Some("8"));
    }
}
