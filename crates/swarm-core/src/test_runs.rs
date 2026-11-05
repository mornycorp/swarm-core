//! The machine's test-run slots: how many test runs go at once on the machine, whichever swarm
//! they belong to (mornycorp/swarm-core#17).
//!
//! On 2026-11-03 a 24 GB unified-memory Mac locked up: one worker's `ready` (`ci --changed`) and
//! another's `claim` (the periodic full gate of `main`) ran their test suites at once, and together
//! wired about 20 GB as GPU memory and pushed 8.6 GB to swap. The machine gate lock
//! ([`crate::machine_lock`]) kept two gates apart, but `ready`'s run took no lock at all.
//!
//! - **Who holds one:** every run of a consumer's `ci` that builds and tests: `gate`'s kinds that
//!   build ([`crate::gate::Kind::builds`]; the periodic full gate's runs are the gate's, kind
//!   full), and `ready`'s `ci --changed` unless the project's planner says the change is docs.
//!   Held while the run goes, dropped when it ends, pass or fail ([`Guard`]).
//! - **How many:** `SWARM_TEST_RUNS` ([`ENV`]: a whole number 1 to [`MAX_RUNS`], the machine's
//!   own, read by every swarm, as they share it), else one per [`GB_PER_RUN`] GB of memory, at
//!   least one ([`limit_from`]): one on a 24 GB Mac, two on a 32 GB desktop (where two gates at
//!   once passed on 2026-10-21). A machine-wide figure, not a [`crate::Project`] setting: the slots
//!   are shared, and a per-project count would let one swarm fill what another counts as free.
//! - **Where:** `test-runs/slot-<i>` beside the machine gate lock, in the same state directory
//!   (`SWARM_GATE_LOCK` moves both, for tests). One line each, the same in every swarm:
//!   `<swarm> <instance> <pid> <time> <what> <worktree>` ([`line`]).
//! - **Taken whole**, as the machine lock is: written beside, then hard-linked to a free slot's
//!   name, which fails when it exists. A run takes one only while fewer than its limit are held
//!   (every slot file counted) and only among slots `0..limit`, so runs that agree on the limit
//!   never pass it.
//! - **Left behind:** a slot whose process is gone ([`alive`]), or older than [`STALE_SECS`], is
//!   taken over ([`judge`]). A line that can't be read is still a slot, aged by its file's time.
//! - **The order, so nothing deadlocks:** the repo's gate lock, then the machine gate lock, then a
//!   test-run slot, then capacity (which doesn't count the asker's own slot); never the other way.
//!   `ready` takes only a slot, and nothing waits for a lock while it holds a slot.
//! - **Waiting:** `ready` waits for a slot; `gate` waits with `--wait`, else refuses. Either says
//!   who holds them, again when that changes. Up to [`WAIT_MAX`].

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::Project;
use crate::lock::{age, describe_age, mtime, now, worktree_of};
use crate::time::{iso_utc, parse_time};

/// The variable that sets the machine's slots.
pub const ENV: &str = "SWARM_TEST_RUNS";
/// Never more slots than this.
pub const MAX_RUNS: u64 = 8;
/// One slot per this much memory (GB). Two full test runs wired about 20 GB of a 24 GB Mac's
/// unified memory, about 10 GB each, and the machine needs the rest for itself and the builds.
pub const GB_PER_RUN: u64 = 16;
/// Added to the total before dividing (GB): what a 32 GB machine's firmware and GPU keep from it,
/// so a desktop that reports 31.9 GB still takes two.
const SLACK_GB: u64 = 4;
/// A slot this old (seconds) was left behind, whatever its process. Longer than the machine
/// lock's: a slot's process is checked too, so the age only catches a pid used again.
pub const STALE_SECS: i64 = 6 * 3600;
/// How long a run waits for a slot, at most, and how often it looks.
pub const WAIT_MAX: Duration = Duration::from_secs(3 * 3600);
const WAIT_STEP: Duration = Duration::from_secs(30);
/// How long `ps` or `tasklist` may take.
const DEADLINE: Duration = Duration::from_secs(20);

const GB: u64 = 1 << 30;

/// A slot as read: whose run, its process, when it was taken (its line's time, else its file's),
/// what runs (`gate-12`, `ready-<sha>`, `gate-full-<sha>`) and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub swarm: String,
    pub instance: String,
    pub pid: Option<u32>,
    pub taken: Option<i64>,
    pub what: String,
    pub worktree: Option<String>,
}

impl Holder {
    /// `app-swarm's w-1 (ready-abc1234 in D:/dev/x, 12 min old)`.
    pub fn describe(&self, now: i64) -> String {
        let place = self.worktree.as_deref().map(|w| format!(" in {w}")).unwrap_or_default();
        format!("{}'s {} ({}{place}, {})", self.swarm, self.instance, self.what, describe_age(age(self.taken, now)))
    }
}

/// A run that takes a slot: whose, its process, what it is and the worktree it runs in.
#[derive(Clone, Copy, Debug)]
pub struct Run<'a> {
    pub swarm: &'a str,
    pub instance: &'a str,
    pub pid: u32,
    pub what: &'a str,
    pub worktree: &'a str,
}

/// The line `<swarm> <instance> <pid> <time> <what> <worktree>`; `None` for an empty one.
pub fn parse(text: &str) -> Option<Holder> {
    let mut parts = text.trim().splitn(6, char::is_whitespace);
    let swarm = parts.next().filter(|s| !s.is_empty())?.to_owned();
    let instance = parts.next().unwrap_or("?").to_owned();
    let pid = parts.next().and_then(|p| p.parse().ok());
    let taken = parts.next().and_then(parse_time);
    let what = parts.next().filter(|w| !w.is_empty()).unwrap_or("?").to_owned();
    let worktree = parts.next().map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned);
    Some(Holder { swarm, instance, pid, taken, what, worktree })
}

/// The line written for `run` at `now` (whitespace in what it is becomes `-`).
pub fn line(run: &Run<'_>, now: i64) -> String {
    let what: String = run.what.chars().map(|c| if c.is_whitespace() { '-' } else { c }).collect();
    format!("{} {} {} {} {what} {}\n", run.swarm, run.instance, run.pid, iso_utc(u64::try_from(now).unwrap_or(0)), run.worktree)
}

/// How many test runs the machine takes at once, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    pub runs: u64,
    pub basis: String,
}

impl Default for Limit {
    fn default() -> Self {
        Limit { runs: 1, basis: "memory unread, so 1".to_owned() }
    }
}

/// The machine's slots: [`ENV`] (`env`, its value) when it is a whole number 1 to [`MAX_RUNS`],
/// else one per [`GB_PER_RUN`] GB of `total` memory (bytes, with [`SLACK_GB`] added), at least one
/// and at most [`MAX_RUNS`]; one when the memory couldn't be read.
pub fn limit_from(env: Option<&str>, total: Option<u64>) -> Limit {
    if let Some(n) = env.and_then(|v| v.trim().parse::<u64>().ok()).filter(|n| (1..=MAX_RUNS).contains(n)) {
        return Limit { runs: n, basis: format!("{ENV}={n}") };
    }
    match total {
        Some(t) => {
            let runs = (t.saturating_add(SLACK_GB * GB) / (GB_PER_RUN * GB)).clamp(1, MAX_RUNS);
            Limit { runs, basis: format!("{:.1} GB of memory, one per {GB_PER_RUN} GB", t as f64 / GB as f64) }
        }
        None => Limit::default(),
    }
}

/// This machine's slots ([`limit_from`]): the environment's figure, else its memory's.
pub fn limit() -> Limit {
    limit_from(std::env::var(ENV).ok().as_deref(), crate::capacity::total_memory())
}

/// A slot judged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    Free,
    Held(Holder),
    /// Its process is gone, or it is older than [`STALE_SECS`]: anyone may take it.
    LeftBehind(Holder),
}

/// What a slot holding `held` is at `now`, `alive` saying whether a process still runs.
pub fn judge(held: Option<Holder>, now: i64, alive: &dyn Fn(u32) -> bool) -> Slot {
    let Some(h) = held else { return Slot::Free };
    let gone = h.pid.is_some_and(|p| !alive(p));
    if gone || age(h.taken, now) >= STALE_SECS { Slot::LeftBehind(h) } else { Slot::Held(h) }
}

/// The slots' directory: `test-runs` beside the machine gate lock; `None` when no state directory
/// can be named.
pub fn dir() -> Option<PathBuf> {
    crate::machine_lock::path().and_then(|p| p.parent().map(|d| d.join("test-runs")))
}

fn slot_file(dir: &Path, i: u64) -> PathBuf {
    dir.join(format!("slot-{i}"))
}

/// The slot files in `dir` (`slot-<n>`, whatever limit took them), in order.
fn slot_files(dir: &Path) -> Vec<PathBuf> {
    let is_slot = |name: &str| name.strip_prefix("slot-").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_name().to_str().is_some_and(is_slot)).map(|e| e.path()).collect();
    files.sort();
    files
}

/// The slot at `file`, if it is held, and its text. A line that can't be read is nobody's (`?`),
/// aged by the file's time.
fn read(file: &Path) -> Option<(String, Holder)> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut h = parse(&text).unwrap_or_else(|| Holder { swarm: "?".into(), instance: "?".into(), pid: None, taken: None, what: "?".into(), worktree: None });
    h.taken = h.taken.or_else(|| mtime(file));
    Some((text, h))
}

/// The slots held in `dir` at `now` (none left behind), but those of process `except`.
pub fn running_at(dir: &Path, now: i64, alive: &dyn Fn(u32) -> bool, except: Option<u32>) -> Vec<Holder> {
    slot_files(dir)
        .iter()
        .filter_map(|f| read(f))
        .filter_map(|(_, h)| match judge(Some(h), now, alive) {
            Slot::Held(h) => Some(h),
            _ => None,
        })
        .filter(|h| except.is_none_or(|p| h.pid != Some(p)))
        .collect()
}

/// The test runs going on this machine now, but those of process `except` (`capacity` leaves out
/// the asker's own); none when there is no state directory.
pub fn running(except: Option<u32>) -> Vec<Holder> {
    dir().map_or_else(Vec::new, |d| running_at(&d, now(), &alive, except))
}

/// Why a slot wasn't taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Every slot is held, by these.
    Full(Vec<Holder>),
    /// The slots' directory couldn't be written.
    Io(String),
}

/// What a full machine says: who holds its `limit` slots.
pub fn full_note(held: &[Holder], limit: u64, now: i64) -> String {
    let who: Vec<String> = held.iter().map(|h| h.describe(now)).collect();
    let who = if who.is_empty() { "another run just took the last".to_owned() } else { who.join("; ") };
    format!("the machine's {limit} test-run slot(s) are taken: {who}")
}

/// Take a slot in `dir` for `run` at `now`, at most `limit` held: its file.
pub fn take_at(dir: &Path, limit: u64, run: &Run<'_>, now: i64, alive: &dyn Fn(u32) -> bool) -> Result<PathBuf, Refused> {
    let held = running_at(dir, now, alive, None);
    if held.len() as u64 >= limit {
        return Err(Refused::Full(held));
    }
    std::fs::create_dir_all(dir).map_err(|e| Refused::Io(format!("{}: {e}", dir.display())))?;
    let text = line(run, now);
    for i in 0..limit {
        let file = slot_file(dir, i);
        if let Some((old, h)) = read(&file) {
            if !matches!(judge(Some(h), now, alive), Slot::LeftBehind(_)) {
                continue;
            }
            remove_if(&file, &old).map_err(|e| Refused::Io(format!("{}: {e}", file.display())))?;
        }
        match crate::machine_lock::put(&file, &text, false) {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Refused::Io(format!("{}: {e}", file.display()))),
        }
    }
    Err(Refused::Full(running_at(dir, now, alive, None)))
}

/// Remove `file` if it still says `text`: a left-behind slot, not one another run took meanwhile.
fn remove_if(file: &Path, text: &str) -> std::io::Result<()> {
    match std::fs::read_to_string(file) {
        Ok(now) if now == text => match std::fs::remove_file(file) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

/// A run's hold on a slot, dropped when it goes out of scope, so a run that stops on any error
/// never leaves it behind. No state directory holds nothing.
#[derive(Debug)]
pub struct Guard {
    file: Option<PathBuf>,
    pid: u32,
}

impl Guard {
    /// The slot's file, when one is held.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let Some(file) = &self.file else { return };
        if read(file).is_some_and(|(_, h)| h.pid == Some(self.pid))
            && let Err(e) = std::fs::remove_file(file)
            && e.kind() != ErrorKind::NotFound
        {
            eprintln!("warning: test-run slot {}: {e}", file.display());
        }
    }
}

/// How a run waits for a slot: `who` it is (`gate`, `ready`), whether it waits, how often it
/// looks and for how long.
#[derive(Clone, Copy, Debug)]
pub struct Waiting<'a> {
    pub who: &'a str,
    pub wait: bool,
    pub step: Duration,
    pub max: Duration,
}

/// Holders as compared while waiting: who and what, not how old.
fn names(held: &[Holder]) -> Vec<(String, String, String)> {
    held.iter().map(|h| (h.swarm.clone(), h.instance.clone(), h.what.clone())).collect()
}

/// Take a slot in `dir` (`None`: none to be had) for `run`, at most `limit` held: at once, or
/// waiting as `how` says, saying who holds them when that changes.
pub fn hold(dir: Option<PathBuf>, limit: &Limit, run: &Run<'_>, how: &Waiting<'_>, alive: &dyn Fn(u32) -> bool) -> Result<Guard, String> {
    let who = how.who;
    let Some(dir) = dir else {
        eprintln!("warning: {who}: no local state directory for the machine's test-run slots; going on without one");
        return Ok(Guard { file: None, pid: run.pid });
    };
    let started = Instant::now();
    let mut seen = None;
    loop {
        match take_at(&dir, limit.runs, run, now(), alive) {
            Ok(file) => {
                println!("{who}: test-run slot {} of {} ({})", file.file_name().and_then(|n| n.to_str()).unwrap_or("?"), limit.runs, limit.basis);
                return Ok(Guard { file: Some(file), pid: run.pid });
            }
            Err(Refused::Io(e)) => return Err(format!("{who}: the machine's test-run slots: {e}")),
            Err(Refused::Full(held)) if how.wait && started.elapsed() < how.max => {
                if seen.as_ref() != Some(&names(&held)) {
                    eprintln!("{who}: {}: waiting (every {} s)", full_note(&held, limit.runs, now()), how.step.as_secs());
                    seen = Some(names(&held));
                }
                std::thread::sleep(how.step);
            }
            Err(Refused::Full(held)) if how.wait => {
                return Err(format!("{who}: {} still, after {} min", full_note(&held, limit.runs, now()), how.max.as_secs() / 60));
            }
            Err(Refused::Full(held)) => return Err(format!("{who}: {}; --wait waits for one", full_note(&held, limit.runs, now()))),
        }
    }
}

/// Take a test-run slot for `instance` of `project`, running `what` in the worktree `root` is in,
/// as `who` (`gate`, `ready`): at once, or, with `wait`, once one is free (up to [`WAIT_MAX`]).
pub fn take(project: &Project, instance: &str, root: &Path, what: &str, wait: bool, who: &str) -> Result<Guard, String> {
    let worktree = worktree_of(root);
    let run = Run { swarm: project.name, instance, pid: std::process::id(), what, worktree: &worktree };
    hold(dir(), &limit(), &run, &Waiting { who, wait, step: WAIT_STEP, max: WAIT_MAX }, &alive)
}

/// `lock show`'s line for the slots: who holds them, or `none`.
pub fn show() -> String {
    let Some(d) = dir() else { return "no local state directory".to_owned() };
    let (held, limit) = (running_at(&d, now(), &alive, None), limit());
    let who: Vec<String> = held.iter().map(|h| h.describe(now())).collect();
    let who = if who.is_empty() { "none".to_owned() } else { who.join("; ") };
    format!("{} of {} ({}): {who}", held.len(), limit.runs, limit.basis)
}

/// Whether process `pid` still runs: `/proc` on Linux, `ps -p` on macOS, `tasklist` on Windows,
/// each within a deadline. A tool that can't say counts as running: a slot is never taken over
/// on a guess.
pub fn alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    if cfg!(target_os = "linux") && Path::new("/proc/self").exists() {
        return Path::new("/proc").join(pid.to_string()).exists();
    }
    let (program, args) = if cfg!(windows) {
        ("tasklist", vec!["/FI".to_owned(), format!("PID eq {pid}"), "/NH".to_owned(), "/FO".to_owned(), "CSV".to_owned()])
    } else {
        ("ps", vec!["-p".to_owned(), pid.to_string(), "-o".to_owned(), "pid=".to_owned()])
    };
    let mut c = Command::new(program);
    c.args(&args);
    match crate::process::captured(c, None, DEADLINE, program) {
        Ok((ok, out, _)) => says_alive(cfg!(windows), ok, &String::from_utf8_lossy(&out), pid),
        Err(_) => true,
    }
}

/// What `ps -p <pid> -o pid=` (its exit and output) or `tasklist`'s CSV (`windows`) says of `pid`.
pub fn says_alive(windows: bool, ok: bool, out: &str, pid: u32) -> bool {
    if windows { out.contains(&format!("\"{pid}\"")) } else { ok && out.split_whitespace().any(|p| p == pid.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::BOTH;
    use std::cell::Cell;

    const T: i64 = 1_792_500_000;

    fn run<'a>(p: &'a Project, instance: &'a str, pid: u32, what: &'a str) -> Run<'a> {
        Run { swarm: p.name, instance, pid, what, worktree: "D:/dev/a swarm-w1" }
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn the_line_reads_back_in_every_swarm() {
        for p in &BOTH {
            let l = line(&run(p, "w-1", 4242, "ready-abc1234"), T);
            assert!(l.starts_with(&format!("{} w-1 4242 20", p.name)) && l.ends_with(" ready-abc1234 D:/dev/a swarm-w1\n"), "{l}");
            let h = parse(&l).unwrap();
            assert_eq!(
                h,
                Holder {
                    swarm: p.name.into(),
                    instance: "w-1".into(),
                    pid: Some(4242),
                    taken: Some(T),
                    what: "ready-abc1234".into(),
                    worktree: Some("D:/dev/a swarm-w1".into())
                }
            );
            assert_eq!(h.describe(T + 120), format!("{}'s w-1 (ready-abc1234 in D:/dev/a swarm-w1, 2 min old)", p.name));
        }
        // What it runs never splits the line.
        let p = &BOTH[0];
        assert_eq!(parse(&line(&run(p, "w-1", 1, "a gate"), T)).map(|h| h.what), Some("a-gate".to_owned()));
        assert_eq!(parse("app-swarm w-1 notapid").map(|h| (h.pid, h.taken, h.what)), Some((None, None, "?".to_owned())));
        assert_eq!(parse(" \n"), None);
    }

    /// One slot per 16 GB, the 4 GB a machine keeps for itself counted: one on the 24 GB Mac, two
    /// on the 32 GB desktop; `SWARM_TEST_RUNS` wins when it is a whole number 1 to 8.
    #[test]
    fn the_machines_slots_follow_its_memory_unless_it_says() {
        let gb = |n: u64| Some(n * GB);
        assert_eq!(limit_from(None, gb(24)).runs, 1, "the 24 GB Mac");
        assert_eq!(limit_from(None, Some(31 * GB + GB * 9 / 10)).runs, 2, "a 32 GB desktop that reports 31.9 GB");
        assert_eq!(limit_from(None, gb(16)).runs, 1);
        assert_eq!(limit_from(None, gb(8)).runs, 1, "never none");
        assert_eq!(limit_from(None, gb(64)).runs, 4);
        assert_eq!(limit_from(None, gb(1024)).runs, MAX_RUNS);
        assert_eq!(limit_from(None, Some(u64::MAX)).runs, MAX_RUNS, "no overflow");
        assert_eq!(limit_from(None, None), Limit::default());
        assert_eq!(Limit::default().runs, 1, "unread memory errs towards fewer");
        assert!(limit_from(None, gb(24)).basis.contains("24.0 GB of memory, one per 16 GB"));
        assert_eq!(limit_from(Some(" 2 "), gb(24)), Limit { runs: 2, basis: "SWARM_TEST_RUNS=2".into() });
        for bad in ["0", "9", "-1", "two", ""] {
            assert_eq!(limit_from(Some(bad), gb(24)).runs, 1, "{bad}");
        }
    }

    #[test]
    fn a_slot_whose_process_is_gone_or_too_old_is_left_behind() {
        let h = parse(&line(&run(&BOTH[0], "w-1", 7, "gate-3"), T)).unwrap();
        let (up, down) = (|_: u32| true, |_: u32| false);
        assert_eq!(judge(None, T, &up), Slot::Free);
        assert_eq!(judge(Some(h.clone()), T + 60, &up), Slot::Held(h.clone()));
        assert_eq!(judge(Some(h.clone()), T + 60, &down), Slot::LeftBehind(h.clone()), "its process is gone");
        assert_eq!(judge(Some(h.clone()), T + STALE_SECS - 1, &up), Slot::Held(h.clone()));
        assert_eq!(judge(Some(h.clone()), T + STALE_SECS, &up), Slot::LeftBehind(h.clone()), "a pid used again");
        assert!(matches!(judge(Some(Holder { taken: Some(T + 600), ..h.clone() }), T, &up), Slot::Held(_)), "the future is young");
        let unread = Holder { pid: None, ..h };
        assert!(matches!(judge(Some(unread), T + 60, &down), Slot::Held(_)), "no pid to check: its age decides");
    }

    /// Through real files: acquire, refuse while full (naming who holds them), release, a holder
    /// whose process is gone taken over, and runs that disagree on the limit counted.
    #[test]
    fn slots_are_taken_refused_released_and_reclaimed() {
        for p in &BOTH {
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-test-runs-{}", p.name)).join("test-runs");
            let dead = std::cell::RefCell::new(Vec::<u32>::new());
            let alive = |pid: u32| !dead.borrow().contains(&pid);
            // One slot: the first run takes it, the second is refused and told whose it is.
            let a = take_at(&dir, 1, &run(p, "w-1", 101, "ready-aaaaaaa"), T, &alive).unwrap();
            assert_eq!(a, dir.join("slot-0"));
            let Err(Refused::Full(held)) = take_at(&dir, 1, &run(p, "w-2", 102, "gate-full-bbbbbbb"), T + 60, &alive) else { panic!("not refused") };
            assert_eq!(held.iter().map(|h| (h.instance.as_str(), h.pid)).collect::<Vec<_>>(), [("w-1", Some(101))]);
            assert!(full_note(&held, 1, T + 60).contains(&format!("1 test-run slot(s) are taken: {}'s w-1 (ready-aaaaaaa in", p.name)));
            // A machine of two: the second takes slot 1, a third is refused.
            assert_eq!(take_at(&dir, 2, &run(p, "w-2", 102, "gate-4"), T + 60, &alive).unwrap(), dir.join("slot-1"));
            assert!(matches!(take_at(&dir, 2, &run(p, "w-3", 103, "gate-5"), T + 60, &alive), Err(Refused::Full(h)) if h.len() == 2));
            // w-1's process is gone; a run that counts one slot still sees slot 1 held: it never
            // runs beside a run of a machine that counts two.
            dead.borrow_mut().push(101);
            assert!(matches!(take_at(&dir, 1, &run(p, "w-3", 103, "gate-5"), T + 60, &alive), Err(Refused::Full(h)) if h.len() == 1 && h[0].instance == "w-2"));
            // w-2's is gone too: the left-behind slot 0 is taken over, and the list leaves both out.
            dead.borrow_mut().push(102);
            assert_eq!(running_at(&dir, T + 60, &alive, None), []);
            assert_eq!(take_at(&dir, 1, &run(p, "w-3", 103, "gate-5"), T + 120, &alive).unwrap(), dir.join("slot-0"));
            assert_eq!(read(&dir.join("slot-0")).and_then(|(_, h)| h.pid), Some(103));
            assert!(read(&dir.join("slot-1")).is_some(), "slot 1 is past this run's limit: left for one that counts it");
            assert_eq!(running_at(&dir, T + 120, &alive, None).iter().map(|h| h.instance.as_str()).collect::<Vec<_>>(), ["w-3"]);
            assert_eq!(running_at(&dir, T + 120, &alive, Some(103)), [], "capacity leaves out the asker's own");
            // No temporary file is left, and a line that can't be read is still a slot.
            assert_eq!(files_in(&dir), ["slot-0", "slot-1"]);
            std::fs::write(dir.join("slot-1"), "").unwrap();
            let unread = running_at(&dir, now(), &alive, None);
            assert!(unread.iter().any(|h| h.swarm == "?"), "{unread:?}");
            let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        }
    }

    /// The guard holds its slot until it goes, and never removes another run's.
    #[test]
    fn a_guard_holds_its_slot_until_it_goes() {
        for p in &BOTH {
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-test-runs-guard-{}", p.name));
            let now_how = Waiting { who: "gate", wait: false, step: Duration::from_millis(1), max: Duration::ZERO };
            let one = Limit { runs: 1, basis: "test".into() };
            let up = |_: u32| true;
            let g = hold(Some(dir.clone()), &one, &run(p, "w-1", 201, "gate-1"), &now_how, &up).unwrap();
            assert_eq!(g.file(), Some(dir.join("slot-0").as_path()));
            let e = hold(Some(dir.clone()), &one, &run(p, "w-2", 202, "gate-2"), &now_how, &up).unwrap_err();
            assert!(
                e.starts_with(&format!("gate: the machine's 1 test-run slot(s) are taken: {}'s w-1 (gate-1", p.name)) && e.ends_with("; --wait waits for one"),
                "{e}"
            );
            std::mem::drop(g);
            assert!(!dir.join("slot-0").exists(), "dropped with its guard");
            // A guard whose slot was taken over leaves the new holder's alone.
            let g = hold(Some(dir.clone()), &one, &run(p, "w-1", 201, "gate-1"), &now_how, &up).unwrap();
            std::fs::write(dir.join("slot-0"), line(&run(p, "w-9", 209, "ready-x"), T)).unwrap();
            std::mem::drop(g);
            assert_eq!(read(&dir.join("slot-0")).and_then(|(_, h)| h.pid), Some(209));
            // No state directory: no slot, and the run goes on.
            assert_eq!(hold(None, &one, &run(p, "w-1", 201, "gate-1"), &now_how, &up).unwrap().file(), None);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A run that waits takes the slot once its holder's process is gone, and one that waits too
    /// long says so.
    #[test]
    fn a_waiting_run_takes_the_slot_once_it_is_free() {
        let p = &BOTH[1];
        let dir = swarm_core_testkit::temp_dir("swarm-core-test-runs-wait");
        let one = Limit { runs: 1, basis: "test".into() };
        let looks = Cell::new(0u32);
        // The holder's process runs for the first three looks, then is gone.
        let alive = |pid: u32| {
            looks.set(looks.get() + 1);
            pid != 301 || looks.get() <= 3
        };
        take_at(&dir, 1, &run(p, "w-1", 301, "gate-full-a"), now(), &|_| true).unwrap();
        let waiting = Waiting { who: "ready", wait: true, step: Duration::from_millis(5), max: Duration::from_secs(60) };
        let g = hold(Some(dir.clone()), &one, &run(p, "w-2", 302, "ready-b"), &waiting, &alive).unwrap();
        assert_eq!(read(&dir.join("slot-0")).and_then(|(_, h)| h.pid), Some(302));
        assert!(looks.get() > 3, "it looked again");
        // Held for good: a run that waits gives up after its time.
        let brief = Waiting { max: Duration::from_millis(20), ..waiting };
        let e = hold(Some(dir.clone()), &one, &run(p, "w-3", 303, "gate-9"), &brief, &|_| true).unwrap_err();
        assert!(e.starts_with("ready: the machine's 1 test-run slot(s) are taken") && e.contains("w-2 (ready-b") && e.contains("still, after 0 min"), "{e}");
        std::mem::drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// This process runs; one that exited doesn't.
    #[test]
    fn a_process_that_exited_is_not_alive() {
        assert!(alive(std::process::id()));
        let mut child = Command::new("git").arg("--version").stdout(std::process::Stdio::null()).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!alive(pid), "{pid} exited");
        assert!(says_alive(false, true, "  4242\n", 4242));
        assert!(!says_alive(false, false, "", 4242));
        assert!(!says_alive(false, true, "42420\n", 4242));
        assert!(says_alive(true, true, "\"cargo.exe\",\"4242\",\"Console\",\"1\",\"10 K\"\r\n", 4242));
        assert!(!says_alive(true, true, "INFO: No tasks are running which match the specified criteria.\r\n", 4242));
    }
}
