//! The daemon's real [`World`]: GitHub through [`github`], git in the gate worktree, the gate as a
//! child process, `land` in process; and its command line.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use super::blame::Packages;
use super::policy::{self, Push};
use super::{Daemon, GateRun, Settings, StackRun, State, World};
use crate::process::{git, git_bytes};
use crate::stack::{self, Entry, Stack, StackMr};
use crate::{Hooks, Project, capacity, claim, event, gate, github, land, lock, machine_lock};

/// The daemon's directory in the gate's target dir: its state and its off switch.
pub const DIR: &str = "gate-daemon";
const STATE: &str = "state.json";
const SWITCH: &str = "off";
/// The running daemon's process id: one daemon a gate worktree.
const PID: &str = "daemon.pid";
/// What the daemon adds to git's configuration: no rerere, so a stale recorded resolution is
/// never replayed into a stack.
pub const RERERE_OFF: &str = "'rerere.enabled=false'";

/// `GIT_CONFIG_PARAMETERS` with rerere off added to what it holds (`existing`); `None` when it
/// already says so.
pub fn with_rerere_off(existing: Option<&str>) -> Option<String> {
    match existing.map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) if e.contains(RERERE_OFF) => None,
        Some(e) => Some(format!("{e} {RERERE_OFF}")),
        None => Some(RERERE_OFF.to_owned()),
    }
}

/// The daemon's options.
#[derive(Debug, Default, PartialEq, Eq)]
struct Opts {
    once: bool,
    dry_run: bool,
    status: bool,
    stop: bool,
    resume: bool,
    /// `--retry <pr>`: gate this PR again at the head the daemon dropped, held or noted it at.
    retry: Option<u64>,
    note: Option<String>,
    agent: Option<String>,
    interval: Option<u64>,
    fold: Option<u64>,
}

fn parse_args(args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter().copied().filter(|a| *a != "--daemon");
    while let Some(a) = it.next() {
        let mut value = || it.next().map(str::to_owned).ok_or_else(|| format!("gate --daemon: {a} needs a value\nusage: {}", super::USAGE));
        match a {
            "--once" => o.once = true,
            "--dry-run" => o.dry_run = true,
            "--status" => o.status = true,
            "--stop" => o.stop = true,
            "--resume" => o.resume = true,
            "--retry" => {
                let v = value()?;
                o.retry = Some(v.trim_start_matches('#').parse().map_err(|_| format!("gate --daemon: --retry takes a PR number, not `{v}`"))?);
            }
            "--note" => o.note = Some(value()?),
            "--agent" => o.agent = Some(value()?),
            "--interval" => o.interval = Some(seconds(&value()?, 10)?),
            "--fold" => o.fold = Some(seconds(&value()?, 0)?),
            other => return Err(format!("gate --daemon: unknown argument `{other}`\nusage: {}", super::USAGE)),
        }
    }
    if [o.status, o.stop, o.resume, o.retry.is_some()].iter().filter(|x| **x).count() > 1 {
        return Err(format!("gate --daemon: one of --status, --stop, --resume and --retry\nusage: {}", super::USAGE));
    }
    Ok(o)
}

fn seconds(v: &str, min: u64) -> Result<u64, String> {
    v.parse::<u64>().ok().filter(|s| *s >= min && *s <= 24 * 3600).ok_or_else(|| format!("gate --daemon: `{v}` is not a number of seconds ({min} to 86400)"))
}

/// `cargo xtask gate --daemon …` ([`super::USAGE`]) in the gate worktree `root`.
pub fn run(project: &Project, hooks: &dyn Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    let o = parse_args(args)?;
    let dir = capacity::our_target(root)?.join(DIR);
    if o.status {
        return status(&dir);
    }
    if o.stop {
        let why = o.note.clone().unwrap_or_else(|| "stopped by `gate --daemon --stop`".to_owned());
        write(&dir, SWITCH, &why)?;
        println!("gate --daemon: off switch set ({why}); a gate it runs now runs to its end, no gate starts after it, and a pass after it isn't landed");
        return Ok(());
    }
    if o.resume {
        return resume(&dir);
    }
    if let Some(pr) = o.retry {
        return retry(&dir, pr);
    }
    // Rerere off for the daemon's whole process tree: only on its own path, never plain `stack`'s.
    if let Some(params) = with_rerere_off(std::env::var("GIT_CONFIG_PARAMETERS").ok().as_deref()) {
        return again_with(&params);
    }
    let agent = lock::agent(project, o.agent.clone(), "gate --daemon")?;
    super::check_worktree(root)?;
    // One daemon a gate worktree: two would share the gate lock's name and each other's state.
    // A dry run changes nothing, so it may run beside one.
    let _only = if o.dry_run { None } else { Some(Only::take(&dir)?) };
    let w = Real::new(project, hooks, root, agent, dir, o.dry_run)?;
    let settings = Settings { dry_run: o.dry_run, fold: Duration::from_secs(o.fold.unwrap_or(120)), verify_main: hooks.verify_main() };
    let mut d = Daemon::new(project, &w, settings, w.load());
    let every = Duration::from_secs(o.interval.unwrap_or(60));
    w.say(&format!("started in {} (every {} s{})", root.display(), every.as_secs(), if o.dry_run { ", dry run" } else { "" }));
    loop {
        d.round();
        if o.once || o.dry_run {
            return Ok(());
        }
        std::thread::sleep(every);
    }
}

/// Run this command again with `GIT_CONFIG_PARAMETERS` set to `params`, and end as it ends.
fn again_with(params: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("gate --daemon: this xtask's path: {e}"))?;
    let status = Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("GIT_CONFIG_PARAMETERS", params)
        .status()
        .map_err(|e| format!("gate --daemon: starting it with rerere off: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("gate --daemon ended: {status}")) }
}

fn write(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_state(dir: &Path) -> State {
    let text = std::fs::read_to_string(dir.join(STATE)).unwrap_or_default();
    serde_json::from_str::<Value>(&text).map(|v| State::from_json(&v)).unwrap_or_default()
}

/// `--status`: the switch, the pause, the PRs waiting and the latest decisions.
fn status(dir: &Path) -> Result<(), String> {
    let s = read_state(dir);
    match std::fs::read_to_string(dir.join(SWITCH)) {
        Ok(why) => println!("off switch: set ({})", why.trim()),
        Err(_) => println!("off switch: off"),
    }
    println!("paused: {}", s.paused.as_deref().unwrap_or("no"));
    for (m, sha) in &s.dropped {
        println!("dropped: #{m} at {} (a gate's failure)", stack::short(sha));
    }
    for (m, (sha, why)) in &s.held {
        println!("held: #{m} at {}: {why}", stack::short(sha));
    }
    for (m, sha) in &s.noted {
        println!("conflicts noted: #{m} at {}", stack::short(sha));
    }
    if let Some(b) = &s.suspects {
        println!("bisecting {}: the culprit is among {}", b.of, b.mrs.iter().map(|x| format!("#{}", x.mr)).collect::<Vec<_>>().join(" "));
    }
    for l in &s.log {
        println!("  {l}");
    }
    Ok(())
}

/// The daemon's hold on its directory: its process id in [`PID`], removed when it ends.
struct Only(PathBuf);

impl Only {
    /// Refused while the process another daemon wrote there still runs.
    fn take(dir: &Path) -> Result<Only, String> {
        let path = dir.join(PID);
        let other = std::fs::read_to_string(&path).ok().and_then(|t| t.trim().parse::<u32>().ok()).filter(|pid| *pid != std::process::id());
        if let Some(pid) = other.filter(|pid| crate::test_runs::alive(*pid)) {
            return Err(format!("gate --daemon: another daemon runs here (process {pid}, {}): stop it first, or run `--dry-run` beside it", path.display()));
        }
        write(dir, PID, &std::process::id().to_string())?;
        Ok(Only(path))
    }
}

impl Drop for Only {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `--retry <pr>`: what the daemon kept of `pr` (dropped, held or noted at a head) is forgotten,
/// so it is gated again as it stands: for a PR the daemon blamed wrongly, marked ready again.
fn retry(dir: &Path, pr: u64) -> Result<(), String> {
    let mut s = read_state(dir);
    let kept = [s.dropped.remove(&pr).is_some(), s.held.remove(&pr).is_some(), s.noted.remove(&pr).is_some()];
    if !kept.contains(&true) {
        println!("gate --daemon: #{pr} isn't dropped, held or noted: nothing to retry");
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&s.to_json()).map_err(|e| e.to_string())?;
    write(dir, STATE, &text)?;
    println!("gate --daemon: #{pr} is gated again once it is ready (a running daemon reads this at its next start: restart it)");
    Ok(())
}

/// `--resume`: the switch off and the held PRs released.
fn resume(dir: &Path) -> Result<(), String> {
    match std::fs::remove_file(dir.join(SWITCH)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", dir.join(SWITCH).display())),
        _ => {}
    }
    let mut s = read_state(dir);
    let held = std::mem::take(&mut s.held);
    if dir.exists() {
        let text = serde_json::to_string_pretty(&s.to_json()).map_err(|e| e.to_string())?;
        write(dir, STATE, &text)?;
    }
    let list: Vec<String> = held.keys().map(|m| format!("#{m}")).collect();
    println!("gate --daemon: off switch cleared{}", if list.is_empty() { String::new() } else { format!("; released {}", list.join(" ")) });
    Ok(())
}

/// The real world of a daemon in the gate worktree `root`.
struct Real<'a> {
    project: &'a Project,
    hooks: &'a dyn Hooks,
    root: &'a Path,
    agent: String,
    dir: PathBuf,
    logs: PathBuf,
    commits: RefCell<BTreeMap<String, BTreeSet<String>>>,
    packages: RefCell<Option<Packages>>,
    /// A dry run: nothing is pruned from the gate worktree's refs, which a live gate may be using.
    dry_run: bool,
}

impl<'a> Real<'a> {
    fn new(project: &'a Project, hooks: &'a dyn Hooks, root: &'a Path, agent: String, dir: PathBuf, dry_run: bool) -> Result<Self, String> {
        let logs = capacity::our_target(root)?.join(gate::LOG_DIR);
        Ok(Real { project, hooks, root, agent, dir, logs, commits: RefCell::default(), packages: RefCell::default(), dry_run })
    }

    fn load(&self) -> State {
        read_state(&self.dir)
    }

    fn main_ref(&self) -> String {
        format!("refs/remotes/{}/{}", self.project.branches.remote, self.project.branches.main)
    }

    /// `gate --sub <sub> <args> --agent <ours>` as a child process, then what its records (and its
    /// rerun's) say.
    fn run_gate(&self, sub: &str, args: &[&str]) -> GateRun {
        let child = std::env::current_exe()
            .and_then(|exe| Command::new(exe).current_dir(self.root).args(["gate", "--sub", sub]).args(args).args(["--agent", &self.agent]).status());
        if let Err(e) = &child {
            self.say(&format!("couldn't start {sub}: {e}"));
        }
        let rc = self.read_log(&format!("{sub}.rc"));
        let rerun_rc = self.read_log(&format!("{sub}-rerun.rc"));
        let log = format!("{}\n{}", self.read_log(&format!("{sub}.log")).unwrap_or_default(), self.read_log(&format!("{sub}-rerun.log")).unwrap_or_default());
        GateRun { ran: policy::ran(sub, rc.as_deref(), rerun_rc.as_deref()), log }
    }

    fn read_log(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.logs.join(name)).ok()
    }

    /// The patch-id of `sha`'s own change against `main` (from where it forks).
    fn own_patch(&self, sha: &str) -> Option<String> {
        let fork = git(self.root, &["merge-base", sha, &self.main_ref()], None, &[]).ok()?;
        let diff =
            git_bytes(self.root, &["diff-tree", "-p", "--no-color", "--full-index", "--no-ext-diff", "--no-textconv", fork.trim(), sha], None, &[]).ok()?;
        if diff.is_empty() {
            return None;
        }
        git(self.root, &["patch-id", "--stable"], Some(&diff), &[]).ok()?.split_whitespace().next().map(str::to_owned)
    }

    /// The PR as GitHub says it is now.
    fn mr(&self, mr: u64) -> Result<Value, String> {
        github::pr(self.project, mr)
    }

    /// Why `now` (the PR as GitHub says it) isn't to be re-drafted at `sha`, if it isn't.
    fn moved(&self, now: &Value, sha: &str) -> Option<String> {
        let state = now.get("state").and_then(Value::as_str).unwrap_or("gone");
        if state != "open" {
            return Some(format!("it is {state}"));
        }
        if now.get("draft").and_then(Value::as_bool) == Some(true) {
            return Some("a draft already".to_owned());
        }
        (now.get("head_sha").and_then(Value::as_str) != Some(sha)).then(|| "pushed since it was gated".to_owned())
    }
}

impl World for Real<'_> {
    fn now(&self) -> i64 {
        i64::try_from(crate::time::now_secs()).unwrap_or(0)
    }
    fn say(&self, line: &str) {
        println!("{} gate daemon: {line}", crate::time::now_iso());
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn switch(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join(SWITCH)).ok().map(|s| s.trim().to_owned())
    }
    fn set_switch(&self, why: &str) -> Result<(), String> {
        write(&self.dir, SWITCH, why)
    }
    fn save(&self, state: &State) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&state.to_json()).map_err(|e| e.to_string())?;
        write(&self.dir, STATE, &text)
    }
    fn main_red(&self) -> Result<Vec<(u64, String)>, String> {
        claim::main_red(self.project)
    }
    fn file_main_red(&self, title: &str, body: &str) -> Result<u64, String> {
        github::create_issue(self.project, title, body, &[self.project.labels.main_red, self.project.labels.bug])
    }
    fn open_mrs(&self) -> Result<Vec<Value>, String> {
        let prune: &[&str] = if self.dry_run { &[] } else { &["--prune"] };
        let mut fetch = vec!["fetch", "--quiet"];
        fetch.extend(prune);
        fetch.push(self.project.branches.remote);
        stack::git_slow(self.root, &fetch, &[])?;
        github::open_prs(self.project, None)
    }
    fn commits(&self, sha: &str) -> Result<BTreeSet<String>, String> {
        if let Some(c) = self.commits.borrow().get(sha) {
            return Ok(c.clone());
        }
        let out = git(self.root, &["rev-list", "--max-count=500", &format!("{}..{sha}", self.main_ref())], None, &[])?;
        let set: BTreeSet<String> = out.lines().map(str::to_owned).collect();
        self.commits.borrow_mut().insert(sha.to_owned(), set.clone());
        Ok(set)
    }
    fn main_conflict(&self, sha: &str) -> Result<Vec<String>, String> {
        let args = ["merge-tree", "--write-tree", "--name-only", "--no-messages", &self.main_ref(), sha];
        match crate::process::git_status_output(self.root, &args, crate::process::GIT_DEADLINE).map_err(|e| e.to_string())? {
            (0, _) => Ok(Vec::new()),
            (1, out) => Ok(out.lines().skip(1).map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()),
            (code, _) => Err(format!("git merge-tree of {} and main: exit {code}", stack::short(sha))),
        }
    }
    fn landed_after(&self, files: &[String]) -> Option<u64> {
        let main = self.main_ref();
        let mut args = vec!["log", "-1", "--first-parent", "--format=%B", main.as_str(), "--"];
        args.extend(files.iter().map(String::as_str));
        policy::merge_request_number(&git(self.root, &args, None, &[]).ok()?)
    }
    fn note(&self, mr: u64, body: &str) -> Result<(), String> {
        github::comment(self.project, mr, body)
    }
    fn redraft(&self, mr: u64, sha: &str, body: &str) -> Result<bool, String> {
        let now = self.mr(mr)?;
        if self.moved(&now, sha).is_some() {
            return Ok(false);
        }
        let node_id = now.get("node_id").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or("GitHub gave no id to convert it by")?;
        github::convert_to_draft(self.project, node_id)?;
        // Read back: what GitHub says now, not what the call answered.
        if self.mr(mr)?.get("draft").and_then(Value::as_bool) != Some(true) {
            return Err("GitHub still reads it as ready".to_owned());
        }
        self.note(mr, body)?;
        Ok(true)
    }
    fn event(&self, args: &[&str]) {
        let mut all = args.to_vec();
        all.extend(["--agent", self.agent.as_str()]);
        gate::log_event(self.project, self.root, &all);
    }
    fn stack(&self, mrs: &[StackMr]) -> StackRun {
        if let Err(e) = lock::take(self.project, self.root, &self.agent) {
            return StackRun::Failed(e);
        }
        let regen = |root: &Path, doc: &stack::Generated| self.hooks.regenerate(root, doc);
        let (remote, base) = (self.project.branches.remote, self.project.branches.main);
        let (stack, stop) = match stack::build(self.project, self.root, remote, base, mrs, &regen) {
            Ok(s) => (s, None),
            Err(stack::Stopped::Early(why)) => return StackRun::Failed(why),
            Err(stack::Stopped::At { stack, why }) => (stack, Some(why)),
        };
        if let Err(e) = stack::save(self.project, self.root, &stack) {
            return StackRun::Failed(e);
        }
        match stop {
            None => StackRun::Built(stack),
            Some(why) => match mrs.get(stack.entries.len()) {
                Some(m) => StackRun::Stopped { at: m.iid, stack, why },
                None => StackRun::Failed(why),
            },
        }
    }
    fn room(&self) -> bool {
        let lock_free = machine_lock::path().is_none_or(|file| {
            let held = std::fs::read_to_string(file).ok().and_then(|t| machine_lock::parse(&t));
            machine_lock::refusal(held.as_ref(), self.project.name, &self.agent, self.now()).is_none()
        });
        lock_free && capacity::our_target(self.root).is_ok_and(|t| gate::room(self.root, &t, self.hooks.gate_priority()).0)
    }
    fn next_sub(&self) -> String {
        let names: Vec<String> = std::fs::read_dir(&self.logs).map(|d| d.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect()).unwrap_or_default();
        policy::next_sub(&names)
    }
    fn gate(&self, sub: &str, note: &str) -> GateRun {
        self.run_gate(sub, &["--note", note, "--wait", "--rerun"])
    }
    fn gate_main(&self, sub: &str, note: &str, tests: &[String]) -> GateRun {
        let not_run = |why: String| {
            self.say(&format!("couldn't check out main for {sub}: {why}"));
            GateRun { ran: policy::Ran::NotRun, log: String::new() }
        };
        if let Err(e) = lock::take(self.project, self.root, &self.agent) {
            return not_run(e);
        }
        let regen = |root: &Path, doc: &stack::Generated| self.hooks.regenerate(root, doc);
        let (remote, base) = (self.project.branches.remote, self.project.branches.main);
        let empty = match stack::build(self.project, self.root, remote, base, &[], &regen) {
            Ok(s) => s,
            Err(stack::Stopped::Early(why) | stack::Stopped::At { why, .. }) => return not_run(why),
        };
        let checked_out =
            stack::save(self.project, self.root, &empty).and_then(|()| stack::git_slow(self.root, &["checkout", "-q", "--detach", &self.main_ref()], &[]));
        if let Err(e) = checked_out {
            return not_run(e);
        }
        let mut args = vec!["--note", note, "--wait"];
        for t in tests {
            args.extend(["--test-alone", t.as_str()]);
        }
        self.run_gate(sub, &args)
    }
    fn on_main(&self, name: &str) -> bool {
        // `git grep` exits 1 when nothing matches; anything else (a match, an error) is taken as
        // there, so the check runs it.
        let pattern = format!("fn {name}");
        let found =
            crate::process::git_code(self.root, &["grep", "-q", "-w", "-F", "-e", &pattern, &self.main_ref(), "--", "*.rs"], crate::process::GIT_DEADLINE);
        !found.is_ok_and(|code| code == 1)
    }
    fn touched(&self, e: &Entry) -> Vec<String> {
        let onto = if e.onto.is_empty() { self.main_ref() } else { e.onto.clone() };
        git(self.root, &["diff", "--name-only", &onto, &e.new_tip], None, &[]).map(|o| o.lines().map(str::to_owned).collect()).unwrap_or_default()
    }
    fn packages(&self) -> Packages {
        if let Some(p) = self.packages.borrow().as_ref() {
            return p.clone();
        }
        let files = git(self.root, &["ls-files", "--", "Cargo.toml", "*/Cargo.toml"], None, &[]).unwrap_or_default();
        let manifests: Vec<(String, String)> = files.lines().filter_map(|f| Some((f.to_owned(), std::fs::read_to_string(self.root.join(f)).ok()?))).collect();
        let p = Packages::from_manifests(&manifests);
        *self.packages.borrow_mut() = Some(p.clone());
        p
    }
    fn root(&self) -> String {
        self.root.canonicalize().unwrap_or_else(|_| self.root.to_path_buf()).to_string_lossy().into_owned()
    }
    fn pushes(&self, stack: &Stack) -> Vec<Push> {
        let (remote, main) = (self.project.branches.remote, self.project.branches.main);
        if let Err(e) = event::fetch(self.root, remote, &format!("refs/heads/{main}"), &self.main_ref()) {
            self.say(&format!("warning: fetching {main}: {e}"));
        }
        let push = |e: &Entry| {
            let now = event::fetch_tip(self.root, remote, &format!("refs/heads/{}", e.source_branch)).ok().flatten();
            let moved = now.as_deref().is_some_and(|n| n != e.old_tip && n != e.new_tip);
            let (old_patch, now_patch) = if moved { (self.own_patch(&e.old_tip), now.as_deref().and_then(|n| self.own_patch(n))) } else { (None, None) };
            policy::push(now.as_deref(), &e.old_tip, &e.new_tip, old_patch.as_deref(), now_patch.as_deref())
        };
        stack.entries.iter().map(push).collect()
    }
    fn land(&self, mrs: &[u64], sub: &str) -> Result<Vec<u64>, (String, Vec<u64>)> {
        let names: Vec<String> = mrs.iter().map(u64::to_string).collect();
        let mut args: Vec<&str> = names.iter().map(String::as_str).collect();
        args.extend(["--sub", sub, "--agent", self.agent.as_str()]);
        let result = land::run(self.project, self.root, &args);
        // What is left in the stack's record didn't land. A record that can't be read says
        // nothing of what landed: none is claimed, and the coordinator is told.
        let landed: Vec<u64> = match stack::load_any(self.project, self.root) {
            Ok(left) => {
                let left: Vec<u64> = left.map(|s| super::entries(&s)).unwrap_or_default();
                mrs.iter().copied().filter(|m| !left.contains(m)).collect()
            }
            Err(e) => {
                let why =
                    format!("land ran, but the stack's record can't be read ({}): what landed is unknown; read `git log` on main", gate::one_line(&e, 120));
                self.close(sub, &why);
                return Err((why, Vec::new()));
            }
        };
        match result {
            Ok(()) => Ok(landed),
            Err(e) => {
                let note =
                    format!("landed {}; stopped: {}", if landed.is_empty() { "nothing".to_owned() } else { policy::bangs(&landed) }, gate::one_line(&e, 120));
                self.close(sub, &note);
                Err((e, landed))
            }
        }
    }
    fn close(&self, sub: &str, note: &str) {
        if let Err(e) = gate::close(self.project, self.root, &self.agent, sub, note) {
            self.say(&format!("warning: closing {sub}: {e}"));
        }
    }
    fn drop_lock(&self) {
        if let Err(e) = lock::drop(self.project, self.root, &self.agent, false) {
            self.say(&format!("warning: dropping the gate lock: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerere_is_turned_off_once_beside_what_git_is_given() {
        assert_eq!(with_rerere_off(None).as_deref(), Some(RERERE_OFF));
        assert_eq!(with_rerere_off(Some("  ")).as_deref(), Some(RERERE_OFF));
        assert_eq!(with_rerere_off(Some("'core.x=1'")), Some(format!("'core.x=1' {RERERE_OFF}")));
        assert_eq!(with_rerere_off(Some(&format!("'core.x=1' {RERERE_OFF}"))), None);
    }

    #[test]
    fn the_options_parse_and_one_action_at_a_time() {
        let o = parse_args(&["--daemon", "--once", "--dry-run", "--interval", "30", "--fold", "0", "--agent", "a-1"]).unwrap();
        assert_eq!(o, Opts { once: true, dry_run: true, interval: Some(30), fold: Some(0), agent: Some("a-1".into()), ..Opts::default() });
        assert!(parse_args(&["--stop", "--note", "why"]).unwrap().stop);
        assert!(parse_args(&["--stop", "--resume"]).is_err());
        assert!(parse_args(&["--interval", "5"]).is_err());
        assert!(parse_args(&["--interval"]).is_err());
        assert!(parse_args(&["--bogus"]).is_err());
        assert_eq!(parse_args(&["--retry", "#12"]).unwrap().retry, Some(12));
        assert!(parse_args(&["--retry", "twelve"]).is_err() && parse_args(&["--retry", "12", "--resume"]).is_err());
    }

    /// #45: one daemon a gate worktree, by its process id; a dead one's file is no hold.
    #[test]
    fn a_second_daemon_is_refused_while_the_first_runs() {
        let dir = swarm_core_testkit::temp_dir("core-daemon-only").join(DIR);
        let held = Only::take(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(PID)).unwrap(), std::process::id().to_string());
        // Our own id is ours to take again; another live process's is refused.
        drop(Only::take(&dir).unwrap());
        assert!(!dir.join(PID).exists(), "removed when it ends");
        std::mem::forget(held);
        let mut other = std::process::Command::new(std::env::current_exe().unwrap()).arg("--list").stdout(std::process::Stdio::null()).spawn().unwrap();
        std::fs::write(dir.join(PID), other.id().to_string()).unwrap();
        let refused = Only::take(&dir);
        let _ = other.wait();
        assert!(refused.is_err_and(|e| e.contains("another daemon runs here")));
        // That process has ended: its file is no hold.
        assert!(Only::take(&dir).is_ok());
    }

    /// #45: `--retry` forgets what the daemon kept of one PR, and only that one.
    #[test]
    fn retry_forgets_one_pr() {
        let dir = swarm_core_testkit::temp_dir("core-daemon-retry").join(DIR);
        let mut s = State::default();
        s.dropped.insert(4, "a".repeat(40));
        s.dropped.insert(5, "b".repeat(40));
        s.held.insert(4, ("a".repeat(40), "why".into()));
        write(&dir, STATE, &serde_json::to_string(&s.to_json()).unwrap()).unwrap();
        retry(&dir, 4).unwrap();
        let after = read_state(&dir);
        assert!(!after.dropped.contains_key(&4) && !after.held.contains_key(&4) && after.dropped.contains_key(&5));
        retry(&dir, 9).unwrap();
        assert_eq!(read_state(&dir), after, "nothing to retry changes nothing");
    }

    #[test]
    fn stop_status_and_resume_use_the_switch_in_the_target_dir() {
        let dir = swarm_core_testkit::temp_dir("core-daemon-switch").join(DIR);
        write(&dir, SWITCH, "owner asked").unwrap();
        let mut s = State::default();
        s.held.insert(5, ("a".repeat(40), "land stopped".into()));
        write(&dir, STATE, &s.to_json().to_string()).unwrap();
        status(&dir).unwrap();
        resume(&dir).unwrap();
        assert!(!dir.join(SWITCH).exists());
        assert!(read_state(&dir).held.is_empty());
        // A second resume, with nothing to clear, is fine.
        resume(&dir).unwrap();
    }
}
