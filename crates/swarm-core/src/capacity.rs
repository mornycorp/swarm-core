//! `cargo xtask capacity [--helper <name> [--kind build|read]]`:
//! how many more builds this machine takes, and a helper's setup (docs/swarm-protocol.md › Helpers
//! in worktrees).
//!
//! - **Report:** logical cores, free memory, free disk on the target dir's drive, and the builds
//!   running now: other cargo target dirs (in any worktree of this clone) written in the last
//!   [`RECENT`], and the `cargo` and `rustc` processes.
//! - **Suggest:** about one more build per [`CORES_PER_BUILD`] free cores and [`GB_PER_BUILD`] GB
//!   of free memory, at most [`MAX_MORE`], and none with under [`MIN_DISK_GB`] GB of disk free
//!   ([`suggest`]). A build running now holds its cores; free memory already counts it. A figure
//!   that can't be read counts as none: the suggestion errs towards fewer builds.
//! - **Set up** (`--helper <name>`): refused in the clone's main worktree (the owner's checkout:
//!   the first entry of `git worktree list --porcelain`), on a detached HEAD, or when the
//!   suggestion is 0. Otherwise a worktree `<this worktree>-<name>` beside this one, on a new
//!   branch `<this branch>-<name>` at HEAD, its own target dir `target/agent-<you>-<name>` (marked
//!   as that worktree's, [`crate::checkout`]), and the helper's `sub.start` logged with `--note`.
//! - **Read helpers** (`--kind read`: research, review, reading logs or GitHub) build
//!   nothing, so the build suggestion never refuses them and they get no target dir; a worktree
//!   only with `--worktree` (when they will write files), else they read their parent's. At most
//!   [`MAX_READ_HELPERS`] open per worker, counted from its own open sub-agents on the tracking
//!   branch whose id starts with [`READ_PREFIX`].
//!
//! Cores come from the standard library; memory and disk from the OS's own tools: `/proc/meminfo`
//! and `df` on Linux, `sysctl`, `vm_stat` and `df` on macOS; on Windows one PowerShell (CIM for
//! memory, `Get-PSDrive` for disk). Processes from `ps`, or that PowerShell's `Get-Process`. Each
//! call has a deadline. No new crates.
//!
//! What `--helper` needs of the tracking timeline (an agent id's check, the agent's own file, the
//! `sub.start`) comes through [`Timeline`]: [`EventTimeline`] in use, a recording fake in tests.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use crate::Project;
use crate::process::{self, git};

/// A build is about this many cores busy.
pub const CORES_PER_BUILD: u64 = 8;
/// A build is about this much memory, in GB: calibrated on the owner's
/// 32-thread, 32 GB Windows desktop, which both swarms share (docs/development.md › How many
/// builds at once). One gate peaked 7.8 GB above the desktop's
/// own ~21 GB; two at once peaked 7.0 GB above it together (their peaks don't line up) and both
/// passed; four filled the 32 GB and three of the four failed. So 5 GB a build: about 10 GB free
/// there takes two, and a third is refused.
pub const GB_PER_BUILD: u64 = 5;
/// At most this many read helpers open per worker (`--kind read`): they build nothing, but each is
/// another agent spending tokens and the shared GitHub account's calls.
pub const MAX_READ_HELPERS: usize = 4;
/// A read helper's sub-agent id starts with this, so its worker's open read helpers can be counted.
pub const READ_PREFIX: &str = "read-";
/// Never suggest more than this many more builds.
pub const MAX_MORE: u64 = 3;
/// Under this much free disk (GB) on the target dir's drive, no more builds.
pub const MIN_DISK_GB: u64 = 20;
/// A target dir written this recently is a build running now.
pub const RECENT: Duration = Duration::from_secs(5 * 60);
/// How long an OS tool may take.
const DEADLINE: Duration = Duration::from_secs(20);
/// How long `cargo metadata` may take.
const METADATA_DEADLINE: Duration = Duration::from_secs(120);

const GB: u64 = 1 << 30;

/// What the machine has, as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    /// Logical cores.
    pub cores: u64,
    /// Free (available) memory and the total, in bytes; `None` when it couldn't be read.
    pub free_memory: Option<u64>,
    pub total_memory: Option<u64>,
    /// Free disk on the target dir's drive, in bytes.
    pub free_disk: Option<u64>,
    /// Other target dirs written in the last [`RECENT`].
    pub busy_dirs: Vec<PathBuf>,
    /// Running `cargo` and `rustc` processes (`None`: couldn't list them).
    pub cargo: Option<u64>,
    pub rustc: Option<u64>,
}

impl Machine {
    /// Builds running now: the busy target dirs, or one when none is but rustc is compiling.
    pub fn builds(&self) -> u64 {
        let dirs = self.busy_dirs.len() as u64;
        if dirs == 0 && self.rustc.is_some_and(|n| n > 0) { 1 } else { dirs }
    }
}

/// How many more builds, and why: one line per figure.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub more: u64,
    pub basis: Vec<String>,
}

/// `bytes` in GB, one decimal.
fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GB as f64)
}

/// The suggestion's arithmetic: min(free cores / 8, free GB / 5, 3), 0 under 20 GB of disk.
pub fn suggest(m: &Machine) -> Suggestion {
    let builds = m.builds();
    let busy = builds.saturating_mul(CORES_PER_BUILD);
    let free_cores = m.cores.saturating_sub(busy);
    let by_cores = free_cores / CORES_PER_BUILD;
    let mut basis = vec![format!(
        "cores: {} logical - {CORES_PER_BUILD} x {builds} build(s) running = {free_cores} free; / {CORES_PER_BUILD} per build = {by_cores}",
        m.cores
    )];
    let by_memory = match m.free_memory {
        Some(free) => {
            let n = free / (GB_PER_BUILD * GB);
            basis.push(format!("memory: {} free / {GB_PER_BUILD} GB per build = {n}", gb(free)));
            n
        }
        None => {
            basis.push("memory: couldn't read free memory, so 0".to_owned());
            0
        }
    };
    basis.push(format!("cap: {MAX_MORE}"));
    let mut more = by_cores.min(by_memory).min(MAX_MORE);
    match m.free_disk {
        Some(free) if free >= MIN_DISK_GB * GB => basis.push(format!("disk: {} free, at least {MIN_DISK_GB} GB", gb(free))),
        Some(free) => {
            basis.push(format!("disk: {} free, under {MIN_DISK_GB} GB, so 0", gb(free)));
            more = 0;
        }
        None => {
            basis.push("disk: couldn't read free disk, so 0".to_owned());
            more = 0;
        }
    }
    basis.push(format!("more builds: min({by_cores}, {by_memory}, {MAX_MORE}) = {more}"));
    Suggestion { more, basis }
}

/// `MemAvailable` (else `MemFree`) and `MemTotal` from `/proc/meminfo`, in bytes.
pub fn parse_meminfo(text: &str) -> (Option<u64>, Option<u64>) {
    let field = |name: &str| {
        text.lines().find_map(|l| {
            let rest = l.strip_prefix(name)?.strip_prefix(':')?;
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            kb.checked_mul(1024)
        })
    };
    (field("MemAvailable").or_else(|| field("MemFree")), field("MemTotal"))
}

/// Free memory from macOS's `vm_stat`: free, inactive and speculative pages, at its page size.
pub fn parse_vm_stat(text: &str) -> Option<u64> {
    let page: u64 = text.lines().next()?.split("page size of ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let pages = |name: &str| -> u64 { text.lines().find_map(|l| l.strip_prefix(name)?.trim().trim_end_matches('.').parse::<u64>().ok()).unwrap_or(0) };
    let free = pages("Pages free:").saturating_add(pages("Pages inactive:")).saturating_add(pages("Pages speculative:"));
    free.checked_mul(page)
}

/// Available bytes from `df -Pk <path>`: the second line's fourth column, in KB.
pub fn parse_df(text: &str) -> Option<u64> {
    let kb: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    kb.checked_mul(1024)
}

/// What the OS's tools said: free and total memory and free disk (bytes), and the `cargo` and
/// `rustc` processes running; `None` for what they couldn't say.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Figures {
    pub free_memory: Option<u64>,
    pub total_memory: Option<u64>,
    pub free_disk: Option<u64>,
    pub cargo: Option<u64>,
    pub rustc: Option<u64>,
}

/// The numbers the Windows PowerShell script printed, one a line: available memory (bytes, the standby
/// cache included, as Linux's `MemAvailable`), total memory (KB), free disk (bytes), `cargo` and
/// `rustc` processes; an empty line is one it couldn't read.
pub fn parse_windows(text: &str) -> Figures {
    let mut n = text.lines().map(|l| l.trim().parse::<u64>().ok());
    let mut next = || n.next().flatten();
    Figures { free_memory: next(), total_memory: next().and_then(|k| k.checked_mul(1024)), free_disk: next(), cargo: next(), rustc: next() }
}

/// Every Windows figure from one PowerShell (its start is most of the time; `tasklist` alone took
/// 5 to 18 s here): `{disk}` is the free disk's expression.
const WINDOWS_SCRIPT: &str = "$m = Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory; $o = Get-CimInstance Win32_OperatingSystem; \
    $m.AvailableBytes; $o.TotalVisibleMemorySize; {disk}; \
    @(Get-Process -Name cargo -ErrorAction SilentlyContinue).Count; @(Get-Process -Name rustc -ErrorAction SilentlyContinue).Count";

/// How many processes are `cargo` and how many `rustc`, from `ps -A -o comm=`'s one name a line
/// (a path on macOS; `.exe` and CSV quotes are tolerated).
pub fn count_processes(text: &str) -> (u64, u64) {
    let (mut cargo, mut rustc) = (0, 0);
    for line in text.lines() {
        let name = line.trim().trim_start_matches('"').split('"').next().unwrap_or_default();
        let name = name.rsplit(['/', '\\']).next().unwrap_or_default().to_ascii_lowercase();
        match name.strip_suffix(".exe").unwrap_or(&name) {
            "cargo" => cargo += 1,
            "rustc" => rustc += 1,
            _ => {}
        }
    }
    (cargo, rustc)
}

/// The worktrees `git worktree list --porcelain` lists, the main one first.
pub fn parse_worktrees(porcelain: &str) -> Vec<PathBuf> {
    porcelain.lines().filter_map(|l| l.strip_prefix("worktree ")).map(|p| PathBuf::from(p.trim())).collect()
}

/// A path as compared: canonical when it exists, `/` separators, no case on Windows.
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let s = p.display().to_string().replace('\\', "/");
        let s = s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_owned();
        if cfg!(windows) { s.to_ascii_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// Is `dir` a cargo target dir: cargo's `CACHEDIR.TAG`, or a `debug` or `release` dir in it?
fn is_target_dir(dir: &Path) -> bool {
    dir.join("CACHEDIR.TAG").is_file() || dir.join("debug").is_dir() || dir.join("release").is_dir()
}

/// The newest write to `dir`'s build: the times of its `deps`, `.fingerprint`, `build` and
/// `incremental` to three deep (under a profile, or a target triple's profile), which move as rustc
/// writes into them; their contents aren't walked. Their parents' own times don't count: cargo
/// makes and removes a lock file in the profile dir on every run, a `cargo xtask …` that compiles
/// nothing included, so two agents polling `capacity` once kept each other at 0 builds (sc#1).
fn newest_write(dir: &Path, depth: u32) -> Option<SystemTime> {
    let leaf = dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| matches!(n, "deps" | ".fingerprint" | "build" | "incremental"));
    if leaf {
        return std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    }
    let mut newest = None;
    if depth == 0 {
        return newest;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir())
            && let Some(t) = newest_write(&entry.path(), depth - 1)
        {
            newest = Some(newest.map_or(t, |n| n.max(t)));
        }
    }
    newest
}

/// The target dirs under the worktrees' `target/` (itself, when it is one, and each dir in it
/// that is), but `ours`, written since `since`.
pub fn busy_target_dirs(worktrees: &[PathBuf], ours: &[&Path], since: SystemTime) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for wt in worktrees {
        let target = wt.join("target");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&target)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !matches!(p.file_name().and_then(|n| n.to_str()), Some("debug" | "release")) && is_target_dir(p))
            .collect();
        if target.join("debug").is_dir() || target.join("release").is_dir() {
            dirs.push(target.clone());
        }
        for d in dirs {
            if ours.iter().any(|o| same_path(o, &d)) {
                continue;
            }
            // The default `target/` holds the agent dirs: only its own profiles count for it.
            let written = if d == target { ["debug", "release"].iter().filter_map(|p| newest_write(&d.join(p), 2)).max() } else { newest_write(&d, 3) };
            if written.is_some_and(|t| t >= since) {
                out.push(d);
            }
        }
    }
    out
}

/// The target dir this xtask was built into (`<target>/debug/xtask`), when it is one.
fn our_target_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?.to_path_buf();
    is_target_dir(&dir).then_some(dir)
}

/// This checkout's target dir: the one this xtask was built into, else cargo's for the workspace
/// at `root` ([`target_dir`]).
pub fn our_target(root: &Path) -> Result<PathBuf, String> {
    our_target_dir().map_or_else(|| target_dir(root), Ok)
}

/// The target directory cargo builds into for the workspace at `root`: `cargo metadata`'s, so
/// `CARGO_TARGET_DIR` and cargo's config count. Cargo is `$CARGO` when set (as `cargo run` sets it).
pub fn target_dir(root: &Path) -> Result<PathBuf, String> {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.current_dir(root).args(["metadata", "--format-version", "1", "--no-deps"]);
    let out = process::output(c, None, METADATA_DEADLINE, "cargo metadata")?;
    parse_target_directory(&out)
}

/// `target_directory` from `cargo metadata`'s JSON.
pub fn parse_target_directory(json: &str) -> Result<PathBuf, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("cargo metadata: bad JSON: {e}"))?;
    v.get("target_directory").and_then(serde_json::Value::as_str).map(PathBuf::from).ok_or_else(|| "cargo metadata: no target_directory".to_owned())
}

fn tool(program: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(program);
    c.args(args);
    process::output(c, None, DEADLINE, program).ok()
}

/// The OS's figures for a target dir at `path`.
fn figures(path: &Path) -> Figures {
    let at = path.display().to_string();
    if cfg!(windows) {
        // The drive letter is checked, so nothing from the path reaches the script but a letter.
        let drive = at.trim_start_matches(['\\', '?']).chars().next().filter(char::is_ascii_alphabetic);
        let disk = drive.map_or_else(|| "''".to_owned(), |d| format!("(Get-PSDrive -Name {d} -PSProvider FileSystem).Free"));
        let script = WINDOWS_SCRIPT.replace("{disk}", &disk);
        return parse_windows(&tool("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", &script]).unwrap_or_default());
    }
    let (free_memory, total_memory) = if cfg!(target_os = "macos") {
        (tool("vm_stat", &[]).as_deref().and_then(parse_vm_stat), tool("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.trim().parse().ok()))
    } else {
        std::fs::read_to_string("/proc/meminfo").map(|t| parse_meminfo(&t)).unwrap_or((None, None))
    };
    let free_disk = tool("df", &["-Pk", &at]).as_deref().and_then(parse_df);
    let (cargo, rustc) = tool("ps", &["-A", "-o", "comm="]).map_or((None, None), |t| {
        let (c, r) = count_processes(&t);
        (Some(c), Some(r))
    });
    Figures { free_memory, total_memory, free_disk, cargo, rustc }
}

/// Read the machine, for a checkout at `root` building into `target`.
pub fn read(root: &Path, target: &Path) -> Machine {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let disk_at = if target.exists() { target } else { root };
    let f = figures(disk_at);
    let worktrees = git(root, &["worktree", "list", "--porcelain"], None, &[]).map(|p| parse_worktrees(&p)).unwrap_or_else(|_| vec![root.to_path_buf()]);
    let since = SystemTime::now().checked_sub(RECENT).unwrap_or(SystemTime::UNIX_EPOCH);
    // Nor the dir the running xtask was built into: this command, not a build. The periodic full
    // gate builds into another checkout's, just after its claim built
    // this xtask.
    let mine = our_target_dir();
    let ours: Vec<&Path> = std::iter::once(target).chain(mine.as_deref()).collect();
    let busy_dirs = busy_target_dirs(&worktrees, &ours, since);
    Machine { cores, free_memory: f.free_memory, total_memory: f.total_memory, free_disk: f.free_disk, busy_dirs, cargo: f.cargo, rustc: f.rustc }
}

/// The report: what was read, then the suggestion and its basis.
pub fn report(m: &Machine, s: &Suggestion, target: &Path) -> String {
    let mut out = String::from("capacity (cargo xtask capacity):\n");
    out.push_str(&format!("  cores:  {} logical\n", m.cores));
    let memory = match (m.free_memory, m.total_memory) {
        (Some(f), Some(t)) => format!("{} free of {}", gb(f), gb(t)),
        (Some(f), None) => format!("{} free", gb(f)),
        _ => "couldn't read".to_owned(),
    };
    out.push_str(&format!("  memory: {memory}\n"));
    out.push_str(&format!(
        "  disk:   {} on the target dir's drive ({})\n",
        m.free_disk.map_or_else(|| "couldn't read".to_owned(), |d| format!("{} free", gb(d))),
        target.display()
    ));
    let count = |n: Option<u64>| n.map_or_else(|| "?".to_owned(), |n| n.to_string());
    out.push_str(&format!(
        "  builds: {} running now: {} other target dir(s) written in the last {} min; {} cargo and {} rustc process(es) (this command's cargo among them)\n",
        m.builds(),
        m.busy_dirs.len(),
        RECENT.as_secs() / 60,
        count(m.cargo),
        count(m.rustc)
    ));
    for d in &m.busy_dirs {
        out.push_str(&format!("          {}\n", d.display().to_string().replace('\\', "/")));
    }
    out.push_str(&format!("suggest: {} more build(s)\n", s.more));
    for b in &s.basis {
        out.push_str(&format!("  {b}\n"));
    }
    out
}

/// A helper's name: letters, digits, `-` and `_`, 1 to 32 of them, starting with a letter or
/// digit (it names a directory, a branch and a `--sub` id).
pub fn check_helper(name: &str) -> Result<(), String> {
    let ok = (1..=32).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if ok { Ok(()) } else { Err(format!("capacity: helper name `{name}`: use 1 to 32 letters, digits, `-` or `_`, starting with a letter or digit")) }
}

/// A helper as set up.
#[derive(Clone, Debug, PartialEq)]
pub struct Helper {
    pub name: String,
    pub worktree: PathBuf,
    pub branch: String,
    /// Its target dir, absolute, and as `CARGO_TARGET_DIR` inside its worktree.
    pub target: PathBuf,
    pub target_env: String,
}

/// Who `you` is in the helper's target dir name: the target dir's `agent-<you>`, else this
/// worktree's directory name.
pub fn you(root: &Path, target: Option<&Path>) -> String {
    target
        .and_then(|t| t.file_name()?.to_str()?.strip_prefix("agent-").map(str::to_owned))
        .filter(|s| !s.is_empty())
        .or_else(|| root.file_name().and_then(|n| n.to_str()).map(str::to_owned))
        .unwrap_or_else(|| "agent".to_owned())
}

/// Make helper `name`'s worktree beside `root`, its branch off `root`'s, and its target dir.
/// Refused in the clone's main worktree, on a detached HEAD, and when either already exists.
pub fn set_up(project: &Project, root: &Path, name: &str, you: &str) -> Result<Helper, String> {
    set_up_with(project, root, name, you, true)
}

/// [`set_up`], with its own target dir only when `with_target` (a read helper builds nothing).
pub fn set_up_with(project: &Project, root: &Path, name: &str, you: &str, with_target: bool) -> Result<Helper, String> {
    check_helper(name)?;
    let worktrees = parse_worktrees(&git(root, &["worktree", "list", "--porcelain"], None, &[])?);
    if worktrees.first().is_none_or(|main| same_path(main, root)) {
        return Err(format!(
            "capacity: {} is the clone's main worktree (the owner's checkout, AGENTS.md §6): run --helper from your own worktree",
            root.display()
        ));
    }
    let branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"], None, &[])
        .map(|b| b.trim().to_owned())
        .ok()
        .filter(|b| !b.is_empty())
        .ok_or("capacity: this worktree's HEAD is detached: a helper branches off yours, so switch to your branch first")?;
    let branch = format!("{branch}-{name}");
    let dir = root.file_name().and_then(|n| n.to_str()).ok_or("capacity: this worktree's directory has no name")?;
    let worktree = root.parent().ok_or("capacity: this worktree has no parent directory")?.join(format!("{dir}-{name}"));
    if worktree.exists() {
        return Err(format!("capacity: {} exists already: another helper's? Check `git worktree list`", worktree.display()));
    }
    if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")], None, &[]).is_ok() {
        return Err(format!("capacity: branch {branch} exists already: another helper's? Pick another name, or delete it"));
    }
    let path = worktree.display().to_string();
    git(root, &["worktree", "add", "--quiet", "-b", &branch, &path, "HEAD"], None, &[])?;
    let target_env = format!("target/agent-{you}-{name}");
    let target = worktree.join("target").join(format!("agent-{you}-{name}"));
    if with_target {
        crate::checkout::claim(project, &target, &worktree)?;
    }
    Ok(Helper { name: name.to_owned(), worktree, branch, target, target_env })
}

/// The read helpers open in a worker's own tracking file: sub-agents started with an id beginning
/// [`READ_PREFIX`] and not stopped since. Hostile lines are skipped.
pub fn open_read_helpers(jsonl: &str) -> usize {
    let mut open = std::collections::BTreeSet::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(sub) = v.get("sub").and_then(|s| s.as_str()).filter(|s| s.starts_with(READ_PREFIX)) else { continue };
        match v.get("event").and_then(|e| e.as_str()) {
            Some("sub.start") => {
                open.insert(sub.to_owned());
            }
            Some("sub.stop") => {
                open.remove(sub);
            }
            _ => {}
        }
    }
    open.len()
}

/// What `--helper` needs of the tracking timeline: the `event` module's own functions, passed in. Each method is the one of the same name there.
pub trait Timeline {
    /// Whether `id` can name an agent (`event::check_id`).
    fn check_id(&self, id: &str) -> Result<(), String>;
    /// The commit `remote`'s ref `src` is at now, fetched into the clone at `root` (no ref
    /// written); `None` when the remote has no such ref (`event::fetch_tip`).
    fn fetch_tip(&self, root: &Path, remote: &str, src: &str) -> Result<Option<String>, String>;
    /// Log one event: `cargo xtask event <args>` (`event::run`).
    fn log(&self, root: &Path, args: &[&str]) -> Result<(), String>;
}

/// The [`Timeline`] `cargo xtask capacity` uses: the `event` module's own functions, for `project`.
pub struct EventTimeline<'a>(pub &'a Project);

impl Timeline for EventTimeline<'_> {
    fn check_id(&self, id: &str) -> Result<(), String> {
        crate::event::check_id(id)
    }

    fn fetch_tip(&self, root: &Path, remote: &str, src: &str) -> Result<Option<String>, String> {
        crate::event::fetch_tip(root, remote, src)
    }

    fn log(&self, root: &Path, args: &[&str]) -> Result<(), String> {
        crate::event::run(self.0, root, args)
    }
}

/// `agent`'s own tracking file on the project's tracking branch, fetched now (empty before it has
/// one).
fn own_tracking(project: &Project, timeline: &dyn Timeline, root: &Path, agent: &str) -> Result<String, String> {
    let src = format!("refs/heads/{}", project.branches.tracking);
    let Some(tip) = timeline.fetch_tip(root, project.branches.remote, &src)? else {
        return Ok(String::new());
    };
    Ok(git(root, &["show", &format!("{tip}:tracking/{agent}/work.jsonl")], None, &[]).unwrap_or_default())
}

/// The agent a helper's events go to: `--agent`, else the project's `<PREFIX>_AGENT` (`from_env`).
fn agent_of(project: &Project, given: Option<String>, from_env: Option<String>) -> Result<String, String> {
    given.or(from_env).filter(|a| !a.trim().is_empty()).ok_or_else(|| {
        let var = project.env_name("AGENT");
        format!("capacity: --helper logs the helper's sub.start: pass --agent <your instance> (or set {var})")
    })
}

/// The helper's `sub.start` (`cargo xtask event` arguments).
pub fn sub_start(helper: &str, role: &str, note: &str, agent: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = ["sub.start", "--sub", helper, "--name", role, "--note", note].iter().map(|s| (*s).to_owned()).collect();
    if let Some(a) = agent {
        v.extend(["--agent".to_owned(), a.to_owned()]);
    }
    v
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Build,
    Read,
}

struct Opts {
    helper: Option<String>,
    role: Option<String>,
    note: Option<String>,
    agent: Option<String>,
    kind: Kind,
    worktree: bool,
}

const USAGE: &str =
    "usage: cargo xtask capacity [--helper <name> --note \"<what it does>\" [--kind build|read] [--worktree] [--role <what it is>] [--agent <instance>]]";

fn parse_args(args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts { helper: None, role: None, note: None, agent: None, kind: Kind::Build, worktree: false };
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        let mut value = || it.next().map(|v| (*v).to_owned()).ok_or_else(|| format!("capacity: {a} needs a value\n{USAGE}"));
        match a {
            "--helper" => o.helper = Some(value()?),
            "--role" => o.role = Some(value()?),
            "--kind" => {
                o.kind = match value()?.as_str() {
                    "build" => Kind::Build,
                    "read" => Kind::Read,
                    other => return Err(format!("capacity: --kind is build or read, not `{other}`\n{USAGE}")),
                }
            }
            "--worktree" => o.worktree = true,
            "--note" => o.note = Some(value()?),
            "--agent" => o.agent = Some(value()?),
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("capacity: unknown argument `{other}`\n{USAGE}")),
        }
    }
    if o.helper.is_none() && (o.note.is_some() || o.agent.is_some() || o.kind == Kind::Read || o.worktree) {
        return Err(format!("capacity: --note, --agent, --kind and --worktree go with --helper\n{USAGE}"));
    }
    if o.worktree && o.kind == Kind::Build {
        return Err(format!("capacity: --worktree is for --kind read (a build helper always gets one)\n{USAGE}"));
    }
    if let Some(h) = &o.helper {
        check_helper(h)?;
        if o.note.as_deref().is_none_or(|n| n.trim().is_empty()) {
            return Err(format!("capacity: --helper needs --note \"<what it does>\" for its sub.start\n{USAGE}"));
        }
    }
    Ok(o)
}

/// A helper being started: who it is for and what its `sub.start` says.
struct Start<'a> {
    project: &'a Project,
    timeline: &'a dyn Timeline,
    root: &'a Path,
    /// `you` in its names ([`you`]).
    you: String,
    agent: String,
    note: String,
}

impl Start<'_> {
    /// Log the helper's `sub.start` as `sub`.
    fn log(&self, sub: &str, role: &str) -> Result<(), String> {
        let args = sub_start(sub, role, &self.note, Some(&self.agent));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.timeline.log(self.root, &args)
    }
}

/// `cargo xtask capacity [--helper <name> --note <what>]`. The helper's events go through
/// `timeline` (the `event` module's, [`Timeline`]).
pub fn run(project: &Project, root: &Path, args: &[&str], timeline: &dyn Timeline) -> Result<(), String> {
    let o = parse_args(args)?;
    let ours = our_target_dir();
    let target = our_target(root)?;
    let machine = read(root, &target);
    let s = suggest(&machine);
    print!("{}", report(&machine, &s, &target));
    let Some(name) = o.helper else { return Ok(()) };
    if o.kind == Kind::Build && s.more == 0 {
        return Err(format!(
            "capacity: no room for build helper {name}: this machine takes no more builds now (see above); do it yourself, wait, or use --kind read if it builds nothing"
        ));
    }
    let agent = agent_of(project, o.agent, project.env("AGENT"))?;
    timeline.check_id(&agent)?;
    let start = Start { project, timeline, root, you: you(root, ours.as_deref()), agent, note: o.note.unwrap_or_default() };
    match o.kind {
        Kind::Read => start_read_helper(&start, &name, o.role, o.worktree),
        Kind::Build => start_build_helper(&start, &name, o.role),
    }
}

/// A read helper: counted against [`MAX_READ_HELPERS`], a worktree only when `worktree`, no
/// target dir.
fn start_read_helper(st: &Start, name: &str, role: Option<String>, worktree: bool) -> Result<(), String> {
    let agent = &st.agent;
    let open = open_read_helpers(&own_tracking(st.project, st.timeline, st.root, agent)?);
    if open >= MAX_READ_HELPERS {
        return Err(format!("capacity: {agent} has {open} read helpers open already, the most per worker ({MAX_READ_HELPERS}): stop one first"));
    }
    let sub = format!("{READ_PREFIX}{name}");
    let worktree = if worktree { Some(set_up_with(st.project, st.root, name, &st.you, false)?) } else { None };
    st.log(&sub, &role.unwrap_or_else(|| "read".to_owned()))?;
    println!("read helper {sub} ({} of {MAX_READ_HELPERS} open):", open + 1);
    match &worktree {
        Some(h) => println!("  worktree:   {} on {} (no target dir: it builds nothing)", h.worktree.display(), h.branch),
        None => println!("  worktree:   none: it reads yours ({})", st.root.display()),
    }
    println!(
        "  events:     --agent {agent} --sub {sub} on any issue event it causes; `cargo xtask event sub.stop --agent {agent} --sub {sub}` once, when done"
    );
    Ok(())
}

/// A build helper: its worktree, branch and target dir.
fn start_build_helper(st: &Start, name: &str, role: Option<String>) -> Result<(), String> {
    let agent = &st.agent;
    let h = set_up(st.project, st.root, name, &st.you)?;
    st.log(&h.name, &role.unwrap_or_else(|| "helper".to_owned()))?;
    println!("helper {}:", h.name);
    println!("  worktree:   {}", h.worktree.display());
    println!(
        "  branch:     {} (off {}; bring its commits back with rebase or cherry-pick; only you push the PR branch)",
        h.branch,
        h.branch.trim_end_matches(&format!("-{}", h.name))
    );
    println!("  target dir: CARGO_TARGET_DIR={} (in its worktree)", h.target_env);
    println!(
        "  events:     --agent {agent} --sub {} on any issue event it causes; `cargo xtask event sub.stop --agent {agent} --sub {}` once, when done",
        h.name, h.name
    );
    println!(
        "  cleanup:    check `git worktree list`, then `git worktree remove {}`, delete its target dir, `git branch -D {}`",
        h.worktree.display(),
        h.branch
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::{APP_SWARM, BOTH, GPU_SWARM};

    fn machine(cores: u64, free_gb: Option<u64>, disk_gb: Option<u64>, busy: usize) -> Machine {
        Machine {
            cores,
            free_memory: free_gb.map(|g| g * GB),
            total_memory: None,
            free_disk: disk_gb.map(|g| g * GB),
            busy_dirs: (0..busy).map(|i| PathBuf::from(format!("t{i}"))).collect(),
            cargo: Some(0),
            rustc: Some(0),
        }
    }

    /// One more build per 8 free cores and 5 GB free, at most 3; none under 20 GB of disk.
    #[test]
    fn the_suggestion_is_its_arithmetic() {
        let more = |m: Machine| suggest(&m).more;
        // A 32-core, 64 GB machine with nothing running: 4 by cores, 4 by memory, capped at 3.
        assert_eq!(more(machine(32, Some(64), Some(500), 0)), 3);
        // Two builds running hold 16 of its cores: 2 by cores.
        assert_eq!(more(machine(32, Some(64), Some(500), 2)), 2);
        // Memory decides: 10 GB free is two builds (the owner's desktop, both swarms'), 9 is one, 4 none.
        assert_eq!(more(machine(32, Some(10), Some(500), 0)), 2);
        assert_eq!(more(machine(32, Some(9), Some(500), 0)), 1);
        assert_eq!(more(machine(32, Some(4), Some(500), 0)), 0);
        // Cores decide: 16 cores with one build running leaves 8, one more.
        assert_eq!(more(machine(16, Some(64), Some(500), 1)), 1);
        assert_eq!(more(machine(8, Some(64), Some(500), 1)), 0);
        assert_eq!(more(machine(4, Some(64), Some(500), 0)), 0);
        // More builds running than cores: none, not an overflow.
        assert_eq!(more(machine(8, Some(64), Some(500), 9)), 0);
        // Disk under 20 GB: none, whatever else; at 20 GB, the rest decides.
        assert_eq!(more(machine(64, Some(128), Some(19), 0)), 0);
        assert_eq!(more(machine(64, Some(128), Some(20), 0)), 3);
        // A figure that couldn't be read counts as none.
        assert_eq!(more(machine(64, None, Some(500), 0)), 0);
        assert_eq!(more(machine(64, Some(128), None, 0)), 0);
        // rustc compiling with no busy target dir is one build.
        let mut m = machine(16, Some(64), Some(500), 0);
        m.rustc = Some(5);
        assert_eq!((m.builds(), more(m)), (1, 1));
        // Each number is printed with its basis.
        let s = suggest(&machine(32, Some(64), Some(500), 2));
        let text = s.basis.join("\n");
        for part in [
            "32 logical",
            "8 x 2 build(s) running = 16 free",
            "/ 8 per build = 2",
            "64.0 GB free / 5 GB per build = 12",
            "cap: 3",
            "500.0 GB free",
            "min(2, 12, 3) = 2",
        ] {
            assert!(text.contains(part), "{part} in {text}");
        }
        assert!(suggest(&machine(64, Some(128), Some(19), 0)).basis.join("\n").contains("under 20 GB, so 0"));
    }

    #[test]
    fn os_figures_parse() {
        let meminfo = "MemTotal:       65793324 kB\nMemFree:         1000000 kB\nMemAvailable:   40000000 kB\n";
        assert_eq!(parse_meminfo(meminfo), (Some(40_000_000 * 1024), Some(65_793_324 * 1024)));
        assert_eq!(parse_meminfo("MemFree: 10 kB\n"), (Some(10_240), None));
        assert_eq!(parse_meminfo("garbage"), (None, None));
        let vm = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               10000.\nPages active:                            50000.\nPages inactive:                          20000.\nPages speculative:                        1000.\n";
        assert_eq!(parse_vm_stat(vm), Some(31_000 * 16_384));
        assert_eq!(parse_vm_stat("no header"), None);
        let df =
            "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/disk3s5    971350180 500000000 400000000      56% /System/Volumes/Data\n";
        assert_eq!(parse_df(df), Some(400_000_000 * 1024));
        assert_eq!(parse_df("Filesystem\n"), None);
        let w = parse_windows("10031513600\r\n33457472\r\n1197333721088\r\n7\r\n4\r\n");
        let all = Figures {
            free_memory: Some(10_031_513_600),
            total_memory: Some(33_457_472 * 1024),
            free_disk: Some(1_197_333_721_088),
            cargo: Some(7),
            rustc: Some(4),
        };
        assert_eq!(w, all);
        // No drive letter: its line is empty, and the rest keep their places.
        assert_eq!(parse_windows("10031513600\n33457472\n\n7\n0\n"), Figures { free_disk: None, rustc: Some(0), ..all });
        assert_eq!(parse_windows(""), Figures::default());
        let tasklist = "\"cargo.exe\",\"1\",\"Console\",\"1\",\"10 K\"\n\"rustc.exe\",\"2\",\"Console\",\"1\",\"10 K\"\n\"rustc.exe\",\"3\",\"Console\",\"1\",\"10 K\"\n\"cargo-watch.exe\",\"4\"\n";
        assert_eq!(count_processes(tasklist), (1, 2));
        assert_eq!(count_processes("cargo\n/Users/r/.rustup/toolchains/stable/bin/rustc\nrustc\nrust-analyzer\n"), (1, 2));
        let porcelain = "worktree D:/dev/app-swarm\nHEAD abc\nbranch refs/heads/main\n\nworktree D:/dev/app-swarm-alder\nHEAD def\ndetached\n";
        assert_eq!(parse_worktrees(porcelain), [PathBuf::from("D:/dev/app-swarm"), PathBuf::from("D:/dev/app-swarm-alder")]);
        let porcelain = "worktree D:/dev/gpu-swarm/.claude/worktrees/x\nHEAD abc\nbranch refs/heads/main\n\nworktree D:/dev/gpu-swarm/.claude/worktrees/x-alder\nHEAD def\ndetached\n";
        assert_eq!(
            parse_worktrees(porcelain),
            [PathBuf::from("D:/dev/gpu-swarm/.claude/worktrees/x"), PathBuf::from("D:/dev/gpu-swarm/.claude/worktrees/x-alder")]
        );
        let meta = r#"{"packages":[],"target_directory":"D:\\dev\\app-swarm\\target","workspace_root":"D:\\dev\\app-swarm"}"#;
        assert_eq!(parse_target_directory(meta), Ok(PathBuf::from(r"D:\dev\app-swarm\target")));
        assert!(parse_target_directory("{}").is_err_and(|e| e.contains("no target_directory")));
        assert!(parse_target_directory("not json").is_err_and(|e| e.contains("bad JSON")));
    }

    /// Read helpers are counted from the worker's own file: started `read-…` and not stopped.
    #[test]
    fn open_read_helpers_are_counted() {
        let lines = [
            r#"{"event":"session.start","t":"2026-10-18T10:00:00Z"}"#,
            r#"{"event":"sub.start","sub":"read-a","name":"read","t":"2026-10-18T10:01:00Z"}"#,
            r#"{"event":"sub.start","sub":"read-b","name":"read","t":"2026-10-18T10:02:00Z"}"#,
            r#"{"event":"sub.start","sub":"gate-1","name":"gate","t":"2026-10-18T10:03:00Z"}"#,
            r#"{"event":"sub.stop","sub":"read-a","t":"2026-10-18T10:04:00Z"}"#,
            r#"{"event":"sub.start","sub":"read-c","t":"2026-10-18T10:05:00Z"}"#,
            "not json",
            r#"{"event":"sub.stop","sub":"read-zz","t":"2026-10-18T10:06:00Z"}"#,
        ];
        assert_eq!(open_read_helpers(&lines.join("\n")), 2, "read-b and read-c; gate-1 is no read helper");
        assert_eq!(open_read_helpers(""), 0);
    }

    #[test]
    fn helper_names_are_checked() {
        for ok in ["gate", "gate-2", "ui_sweep", "a"] {
            assert!(check_helper(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-gate", "a/b", "a b", "a..b", "x".repeat(33).as_str(), "--force", "über"] {
            assert!(check_helper(bad).is_err(), "{bad}");
        }
        assert!(parse_args(&["--helper", "gate"]).is_err_and(|e| e.contains("--note")));
        assert!(
            parse_args(&["--helper", "gate", "--note", "gate #126"])
                .is_ok_and(|o| o.helper.as_deref() == Some("gate") && o.role.is_none() && o.kind == Kind::Build)
        );
        assert!(parse_args(&["--helper", "look", "--note", "x", "--kind", "read", "--worktree"]).is_ok_and(|o| o.kind == Kind::Read && o.worktree));
        assert!(parse_args(&["--helper", "look", "--note", "x", "--kind", "fast"]).is_err());
        assert!(parse_args(&["--helper", "b", "--note", "x", "--worktree"]).is_err_and(|e| e.contains("--kind read")));
        assert!(parse_args(&["--kind", "read"]).is_err());
        assert!(parse_args(&["--note", "x"]).is_err());
        assert!(parse_args(&["--helper"]).is_err());
        assert!(parse_args(&["--frobnicate"]).is_err());
        assert!(parse_args(&[]).is_ok_and(|o| o.helper.is_none()));
        assert_eq!(
            sub_start("gate", "gate", "local gate: #126", Some("dt9-1")),
            ["sub.start", "--sub", "gate", "--name", "gate", "--note", "local gate: #126", "--agent", "dt9-1"]
        );
        assert_eq!(you(Path::new("D:/dev/app-swarm-alder"), Some(Path::new("D:/dev/app-swarm-alder/target/agent-alder"))), "alder");
        assert_eq!(you(Path::new("D:/dev/app-swarm-alder"), Some(Path::new("D:/dev/app-swarm-alder/target"))), "app-swarm-alder");
        assert_eq!(you(Path::new("D:/dev/app-swarm-alder"), None), "app-swarm-alder");
        let ns = Path::new("D:/dev/gpu-swarm/.claude/worktrees/x-alder");
        assert_eq!(you(ns, Some(&ns.join("target").join("agent-alder"))), "alder");
        assert_eq!(you(ns, Some(&ns.join("target"))), "x-alder");
        assert_eq!(you(ns, None), "x-alder");
    }

    /// The agent is `--agent`, else the project's own `<PREFIX>_AGENT`; the error names that one.
    #[test]
    fn the_helpers_agent_comes_from_the_projects_variable() {
        for p in BOTH {
            assert_eq!(agent_of(&p, Some("dt9-1".into()), Some("other".into())), Ok("dt9-1".to_owned()));
            assert_eq!(agent_of(&p, None, Some("dt9-2".into())), Ok("dt9-2".to_owned()));
            let e = agent_of(&p, Some("  ".into()), None).unwrap_err();
            assert!(e.contains(&format!("(or set {}_AGENT)", p.env_prefix)), "{e}");
        }
        assert!(agent_of(&APP_SWARM, None, None).unwrap_err().contains("APP_SWARM_AGENT"));
        assert!(agent_of(&GPU_SWARM, None, None).unwrap_err().contains("GPU_SWARM_AGENT"));
    }

    /// A timeline that records what it was asked, with a tracking branch whose tip is `tip`.
    struct Recorder {
        tip: Option<String>,
        asked: std::cell::RefCell<Vec<String>>,
    }

    impl Timeline for Recorder {
        fn check_id(&self, id: &str) -> Result<(), String> {
            self.asked.borrow_mut().push(format!("check_id {id}"));
            Ok(())
        }

        fn fetch_tip(&self, _root: &Path, remote: &str, src: &str) -> Result<Option<String>, String> {
            self.asked.borrow_mut().push(format!("fetch_tip {remote} {src}"));
            Ok(self.tip.clone())
        }

        fn log(&self, _root: &Path, args: &[&str]) -> Result<(), String> {
            self.asked.borrow_mut().push(format!("log {}", args.join(" ")));
            Ok(())
        }
    }

    /// The worker's own file is read at the tip of the project's tracking branch on its remote.
    #[test]
    fn open_read_helpers_are_read_from_the_projects_tracking_branch() {
        use swarm_core_testkit::git::git as tgit;
        let dir = swarm_core_testkit::temp_dir("swarm-core-capacity-own-tracking");
        tgit(&dir, &["init", "--quiet"]);
        std::fs::create_dir_all(dir.join("tracking").join("dt9-1")).unwrap();
        let line = r#"{"event":"sub.start","sub":"read-a","t":"2026-10-18T10:01:00Z"}"#;
        std::fs::write(dir.join("tracking").join("dt9-1").join("work.jsonl"), format!("{line}\n")).unwrap();
        tgit(&dir, &["add", "."]);
        tgit(&dir, &["commit", "--quiet", "-m", "events"]);
        let tip = tgit(&dir, &["rev-parse", "HEAD"]).trim().to_owned();
        for p in BOTH {
            let t = Recorder { tip: Some(tip.clone()), asked: Default::default() };
            let file = own_tracking(&p, &t, &dir, "dt9-1").unwrap();
            assert_eq!(open_read_helpers(&file), 1);
            assert_eq!(*t.asked.borrow(), [format!("fetch_tip {} refs/heads/{}", p.branches.remote, p.branches.tracking)]);
            // Another agent's file isn't there: none open. No tracking branch yet: none either.
            assert_eq!(own_tracking(&p, &t, &dir, "dt9-2").unwrap(), "");
            let none = Recorder { tip: None, asked: Default::default() };
            assert_eq!(own_tracking(&p, &none, &dir, "dt9-1").unwrap(), "");
        }
        // The timeline in use checks ids as `cargo xtask event` does.
        assert!(EventTimeline(&APP_SWARM).check_id("dt9-1").is_ok() && EventTimeline(&GPU_SWARM).check_id("../x").is_err());
        let st = Start {
            project: &APP_SWARM,
            timeline: &Recorder { tip: None, asked: Default::default() },
            root: &dir,
            you: "alder".into(),
            agent: "dt9-1".into(),
            note: "look".into(),
        };
        st.log("read-x", "read").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A helper is never set up in the clone's main worktree (the owner's checkout) or on a
    /// detached HEAD; from a worker's worktree on its branch it gets a worktree beside it, a branch
    /// off it and a target dir of its own, named for the helper.
    #[test]
    fn a_helper_is_set_up_beside_a_worktree_never_in_the_main_one() {
        for p in BOTH {
            helper_set_up(&p);
        }
    }

    fn helper_set_up(p: &Project) {
        use swarm_core_testkit::git::git as tgit;
        let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-capacity-helper-{}", p.name));
        let main = dir.join("repo");
        std::fs::create_dir_all(&main).unwrap();
        tgit(&main, &["init", "--quiet"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        tgit(&main, &["add", "a.txt"]);
        tgit(&main, &["commit", "--quiet", "-m", "first"]);
        let worker = dir.join("repo-alder");
        tgit(&main, &["worktree", "add", "--quiet", "--detach", worker.to_str().unwrap()]);

        // The main worktree: refused, nothing made.
        let e = set_up(p, &main, "gate", "alder").unwrap_err();
        assert!(e.contains("main worktree") && e.contains("owner"), "{e}");
        assert!(!dir.join("repo-gate").exists());
        // A detached HEAD: refused.
        assert!(set_up(p, &worker, "gate", "alder").unwrap_err().contains("detached"));
        // A bad name: refused before anything is made.
        tgit(&worker, &["switch", "--quiet", "-c", "121-capacity"]);
        assert!(set_up(p, &worker, "../x", "alder").is_err());

        let h = set_up(p, &worker, "gate", "alder").unwrap();
        assert_eq!(h.worktree, dir.join("repo-alder-gate"));
        assert_eq!(h.branch, "121-capacity-gate");
        assert_eq!(h.target_env, "target/agent-alder-gate");
        assert_eq!(h.target, h.worktree.join("target").join("agent-alder-gate"));
        let head = tgit(&h.worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
        assert_eq!(head.trim(), "121-capacity-gate");
        assert_eq!(tgit(&h.worktree, &["rev-parse", "HEAD"]), tgit(&worker, &["rev-parse", "HEAD"]), "off the worker's branch");
        // Its target dir is marked as its worktree's, so no other checkout builds there.
        let marker = std::fs::read_to_string(h.target.join(p.checkout_marker)).unwrap();
        assert!(same_path(Path::new(marker.trim()), &h.worktree), "{marker}");
        assert!(tgit(&main, &["worktree", "list"]).contains("repo-alder-gate"));
        // The same helper twice: refused, the first left alone.
        assert!(set_up(p, &worker, "gate", "alder").unwrap_err().contains("exists already"));
        // A helper's own worktree isn't the main one either: it can set up its own.
        assert!(set_up(p, &h.worktree, "sub", "alder").is_ok_and(|s| s.branch == "121-capacity-gate-sub"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory opened so its times can be set (Windows opens one only with backup semantics).
    fn open_dir(d: &Path) -> std::io::Result<std::fs::File> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS.
            std::fs::OpenOptions::new().access_mode(0x100).custom_flags(0x0200_0000).open(d)
        }
        #[cfg(not(windows))]
        std::fs::File::open(d)
    }

    /// Builds running now: target dirs in any worktree written in the last 5 minutes, but ours.
    #[test]
    fn busy_target_dirs_are_those_written_recently() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-capacity-busy");
        let (a, b) = (dir.join("a"), dir.join("b"));
        let mk = |p: &Path| std::fs::create_dir_all(p.join("debug").join("deps")).unwrap();
        mk(&a.join("target").join("agent-x"));
        mk(&a.join("target").join("agent-ours"));
        mk(&a.join("target").join("agent-old"));
        mk(&b.join("target"));
        std::fs::create_dir_all(a.join("target").join("not-a-target")).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        let age = |p: &Path| {
            for d in [p.to_path_buf(), p.join("debug"), p.join("debug").join("deps")] {
                open_dir(&d).and_then(|f| f.set_modified(old)).unwrap_or_else(|e| panic!("{}: {e}", d.display()));
            }
        };
        age(&a.join("target").join("agent-old"));
        // Only cargo ran here (its lock file moved the profile dir's time), no rustc: not a build.
        let idle = a.join("target").join("agent-idle");
        mk(&idle);
        open_dir(&idle.join("debug").join("deps")).and_then(|f| f.set_modified(old)).unwrap();
        let since = SystemTime::now() - RECENT;
        let ours = a.join("target").join("agent-ours");
        let busy = busy_target_dirs(&[a.clone(), b.clone(), dir.join("missing")], &[&ours], since);
        assert_eq!(busy, [a.join("target").join("agent-x"), b.join("target")]);
        // The full gate's target and the running xtask's are both its own.
        let xtask = a.join("target").join("agent-x");
        assert_eq!(busy_target_dirs(&[a.clone(), b.clone()], &[&ours, &xtask], since), [b.join("target")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
