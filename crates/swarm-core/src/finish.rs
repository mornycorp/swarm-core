//! `cargo xtask finish --agent <id>` (sc#10): the last step of a session, in one command, so its
//! `session.stop` is neither forgotten nor left with nowhere to log it from (#10: a
//! worker removed its last worktree first, and had no checkout left to build `cargo xtask event`
//! in). `--agent` defaults to `{prefix}_AGENT`, as `event`'s does.
//!
//! 1. **What is owed.** The agent's PRs and claimed issues come from its own tracking events
//!    (`claim` and `submit`, less what it released: [`crate::prwatch::agent_mrs`]). A PR still
//!    open, or a claimed issue still open, stops it, naming them, unless `--force`.
//! 2. **The tools.** It installs this xtask outside the checkouts ([`install`],
//!    `cargo xtask install-tools`), so `event` and `finish` run with no worktree left.
//! 3. **Worktrees.** Each worktree of the clone on one of the agent's merged or closed PRs'
//!    branches is removed with what it holds when it is clean: its `target/` first, with no
//!    deadline (tens of GB, sc#12), then `git worktree remove`. A dirty one is named and kept; one
//!    whose directory is already gone is pruned first, so it holds its branch no longer. Never the main worktree (the owner's
//!    checkout), never one whose PR is open, and never the one this xtask runs from (its
//!    executable is in use): for that one it prints the command that removes it.
//! 4. **Branches.** Its merged PRs' local branches are deleted, unless a worktree kept one.
//! 5. **The stop.** It logs `session.stop`, with `--note` or what it did.
//!
//! `--dry-run` says what it would do and does none of it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::checkout::{TOOLS_DIR, same_path};
use crate::process::git;
use crate::{Project, event, github, prwatch};

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask finish [--agent <id>] [--note \"<one line>\"] [--force] [--dry-run]: end your session: check nothing is owed, remove your clean worktrees and merged branches, log session.stop";

/// The usage line of `install-tools`, as [`crate::COMMANDS`] lists it.
pub const INSTALL_USAGE: &str = "cargo xtask install-tools: copy this xtask into the clone's git directory, to run `event` and `finish` with no worktree";

/// One of the agent's PRs, as GitHub has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPr {
    pub number: u64,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub branch: String,
}

impl AgentPr {
    /// One PR in [`github`]'s shape.
    fn from_api(v: &Value) -> Option<Self> {
        Some(AgentPr { number: v.get("number")?.as_u64()?, state: v.get("state")?.as_str()?.to_owned(), branch: v.get("head_ref")?.as_str()?.to_owned() })
    }

    fn open(&self) -> bool {
        self.state == "open"
    }
}

/// What the agent still owes: its open PRs, and its claimed issues still open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owed {
    pub mrs: Vec<u64>,
    pub issues: Vec<u64>,
}

impl Owed {
    pub fn is_empty(&self) -> bool {
        self.mrs.is_empty() && self.issues.is_empty()
    }

    /// `PR #12 open, #5 claimed and open`.
    pub fn describe(&self) -> String {
        let mrs = self.mrs.iter().map(|m| format!("PR #{m} open"));
        let issues = self.issues.iter().map(|i| format!("#{i} claimed and open"));
        mrs.chain(issues).collect::<Vec<_>>().join(", ")
    }
}

/// What the agent owes, from its PRs and the claimed issues GitHub says are open.
pub fn owed(mrs: &[AgentPr], open_issues: &[u64]) -> Owed {
    Owed { mrs: mrs.iter().filter(|m| m.open()).map(|m| m.number).collect(), issues: open_issues.to_vec() }
}

/// A worktree of the clone, as `git worktree list --porcelain` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Its branch, without `refs/heads/`; `None` on a detached HEAD.
    pub branch: Option<String>,
}

/// The worktrees `git worktree list --porcelain` lists, the main one first.
pub fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            out.push(Worktree { path: PathBuf::from(p.trim()), branch: None });
        } else if let (Some(b), Some(last)) = (line.strip_prefix("branch refs/heads/"), out.last_mut()) {
            last.branch = Some(b.trim().to_owned());
        }
    }
    out
}

/// What `finish` does with one of the agent's worktrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    Remove,
    /// Kept: changes or untracked files in it.
    Dirty,
    /// Kept: its PR is open (with `--force`).
    OpenMr,
    /// Kept: this xtask runs from it.
    Running,
}

/// What `finish` will do: each of the agent's worktrees' fate, and the merged branches to delete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub worktrees: Vec<(PathBuf, Fate)>,
    pub branches: Vec<String>,
}

/// The plan for `worktrees` (the main one first, never touched) and the agent's `mrs`: a worktree
/// is the agent's when it is on one of their branches. `running` is the worktree this xtask runs
/// from, if any; `dirty` says whether a worktree has changes.
pub fn plan(worktrees: &[Worktree], mrs: &[AgentPr], running: Option<&Path>, dirty: &dyn Fn(&Path) -> bool) -> Plan {
    let mut out = Plan::default();
    for w in worktrees.iter().skip(1) {
        let Some(mr) = w.branch.as_deref().and_then(|b| mrs.iter().find(|m| m.branch == b)) else { continue };
        let fate = if running.is_some_and(|r| same_path(r, &w.path)) {
            Fate::Running
        } else if mr.open() {
            Fate::OpenMr
        } else if dirty(&w.path) {
            Fate::Dirty
        } else {
            Fate::Remove
        };
        out.worktrees.push((w.path.clone(), fate));
    }
    let kept = |b: &str| worktrees.iter().any(|w| w.branch.as_deref() == Some(b) && !out.worktrees.contains(&(w.path.clone(), Fate::Remove)));
    out.branches = mrs.iter().filter(|m| m.state == "merged" && crate::stack::valid_branch(&m.branch) && !kept(&m.branch)).map(|m| m.branch.clone()).collect();
    out.branches.dedup();
    out
}

/// What the plan says of each worktree it keeps, as lines.
pub fn kept_lines(plan: &Plan, main: &Path) -> Vec<String> {
    plan.worktrees
        .iter()
        .filter_map(|(path, fate)| match fate {
            Fate::Remove => None,
            Fate::Dirty => Some(format!("kept {}: it has changes; commit or discard them, then `git worktree remove` it", path.display())),
            Fate::OpenMr => Some(format!("kept {}: its PR is open", path.display())),
            Fate::Running => {
                Some(format!("kept {}: this xtask runs from it; remove it with `git -C {} worktree remove {}`", path.display(), main.display(), path.display()))
            }
        })
        .collect()
}

/// Carry out `plan` in the clone whose main worktree is `main`: remove the worktrees, then delete
/// the branches. Returns what happened, a line each; a step that fails is said and the rest goes on.
pub fn apply(main: &Path, plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    for (path, _) in plan.worktrees.iter().filter(|(_, f)| *f == Fate::Remove) {
        let at = path.to_string_lossy();
        lines.push(match remove_worktree(main, path) {
            Ok(()) => format!("removed {at}"),
            Err(e) => {
                // A removal cut short leaves the worktree registered, holding its branch.
                let _ = git(main, &["worktree", "prune"], None, &[]);
                format!("couldn't remove {at}: {e}")
            }
        });
    }
    for b in &plan.branches {
        lines.push(match git(main, &["branch", "-D", "--quiet", b], None, &[]) {
            Ok(_) => format!("deleted branch {b}"),
            Err(e) => format!("couldn't delete branch {b}: {e}"),
        });
    }
    lines
}

/// How long `git worktree remove` may take once the target dir is gone, for the other ignored
/// files it deletes (sc#12).
const REMOVE_DEADLINE: Duration = Duration::from_secs(600);

/// Remove the worktree at `path` from the clone whose main worktree is `main`. Its `target/` goes
/// first, with no deadline: a worker's holds tens of GB (29 GB took 36 s on Windows), past git's
/// 20 s, which once cut a removal short (sc#12). Not when the repo tracks files there.
fn remove_worktree(main: &Path, path: &Path) -> Result<(), String> {
    let target = path.join("target");
    let tracked = git(path, &["ls-files", "--", "target"], None, &[]).map_or(true, |s| !s.trim().is_empty());
    if target.is_dir() && !tracked {
        std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    let mut cmd = Command::new("git");
    cmd.current_dir(main).args(["worktree", "remove"]).arg(path).env("GIT_TERMINAL_PROMPT", "0");
    crate::process::output(cmd, None, REMOVE_DEADLINE, "git worktree remove").map(drop)
}

/// Copy this xtask into the clone's git directory ([`TOOLS_DIR`]), where no worktree's removal
/// takes it; its path. Run from there, it finds the clone by where it is
/// ([`crate::checkout::installed_root`]). A copy already running there is left as it is.
pub fn install(root: &Path) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("install-tools: where is this xtask? {e}"))?;
    let name = exe.file_name().ok_or("install-tools: this xtask has no file name")?;
    let dir = event::common_dir(root).join(TOOLS_DIR);
    let dest = dir.join(name);
    if same_path(&exe, &dest) {
        return Ok(dest);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::copy(&exe, &dest).map_err(|e| format!("install-tools: copying to {}: {e}", dest.display()))?;
    Ok(dest)
}

/// `cargo xtask install-tools`.
pub fn run_install(root: &Path, args: &[&str]) -> Result<(), String> {
    if let Some(a) = args.first() {
        return Err(format!("install-tools: unknown argument `{a}` (usage: {INSTALL_USAGE})"));
    }
    let dest = install(root)?;
    println!("install-tools: {}", dest.display());
    println!("  run `{} event …` or `{} finish …` from anywhere, with no worktree", dest.display(), dest.display());
    Ok(())
}

/// What `finish` was asked.
#[derive(Debug, Default, PartialEq)]
struct Args {
    agent: Option<String>,
    note: Option<String>,
    force: bool,
    dry_run: bool,
}

fn parse_args(args: &[&str]) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--agent" => out.agent = Some(it.next().ok_or("--agent takes an instance id")?.to_string()),
            "--note" => out.note = Some(it.next().ok_or("--note takes a line")?.to_string()),
            "--force" => out.force = true,
            "--dry-run" => out.dry_run = true,
            other => return Err(format!("finish: unknown argument `{other}` (usage: {USAGE})")),
        }
    }
    Ok(out)
}

/// What GitHub says of the agent: its PRs, and which of its claimed issues are open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub mrs: Vec<AgentPr>,
    pub open_issues: Vec<u64>,
}

/// Ask GitHub about the agent whose tracking events are `events`: one call for its PRs, one more
/// when it claimed issues.
fn ask_github(project: &Project, events: &str) -> Result<Asked, String> {
    let numbers: Vec<u64> = prwatch::agent_mrs(events).into_iter().take(100).collect();
    let mut out = Asked::default();
    if !numbers.is_empty() {
        out.mrs = github::prs(project, &numbers)?.iter().filter_map(AgentPr::from_api).collect();
    }
    let claimed = prwatch::agent_issues(events);
    if !claimed.is_empty() {
        let found = github::issues(project, None, &claimed)?;
        out.open_issues = found.iter().filter(|i| i["state"] == "open").filter_map(|i| i.get("number")?.as_u64()).collect();
    }
    Ok(out)
}

/// `cargo xtask finish …` ([`USAGE`]) for `project`, from the clone at `root`.
pub fn run(project: &Project, root: &Path, args: &[&str]) -> Result<(), String> {
    let a = parse_args(args)?;
    let agent = a.agent.clone().or_else(|| project.env("AGENT").filter(|s| !s.is_empty())).ok_or_else(|| {
        format!("finish: which agent? pass --agent <id> (your instance id from `cargo xtask event session.start`) or set {}", project.env_name("AGENT"))
    })?;
    event::check_id(&agent)?;
    let events = prwatch::tracking_file(project, root, &agent)?;
    let asked = ask_github(project, &events)?;
    finish(project, root, &agent, &asked, &a)
}

/// The rest of `finish`, once GitHub has answered: everything here is git and the event.
fn finish(project: &Project, root: &Path, agent: &str, asked: &Asked, a: &Args) -> Result<(), String> {
    let owed = owed(&asked.mrs, &asked.open_issues);
    let refusal = format!("{agent} still owes {}: finish them, release them, or pass --force", owed.describe());
    if !owed.is_empty() && !a.force && !a.dry_run {
        return Err(format!("finish: {refusal}"));
    }
    // A worktree whose directory is gone (a removal cut short, sc#12) no longer holds its branch.
    if !a.dry_run {
        let _ = git(root, &["worktree", "prune"], None, &[]);
    }
    let worktrees = parse_worktrees(&git(root, &["worktree", "list", "--porcelain"], None, &[])?);
    let main = worktrees.first().map_or_else(|| root.to_path_buf(), |w| w.path.clone());
    let running = running_in(&worktrees);
    let dirty = |p: &Path| git(p, &["status", "--porcelain"], None, &[]).map_or(true, |s| !s.trim().is_empty());
    let mut plan = plan(&worktrees, &asked.mrs, running.as_deref(), &dirty);
    plan.branches.retain(|b| git(&main, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{b}")], None, &[]).is_ok());
    if a.dry_run {
        if !owed.is_empty() && !a.force {
            println!("finish: would stop here: {refusal}");
        }
        say_dry_run(&plan, &main, agent);
        return Ok(());
    }
    match install(root) {
        Ok(dest) => println!("finish: tools installed: {} (`event` runs from there with no worktree)", dest.display()),
        Err(e) => eprintln!("warning: finish: {e}"),
    }
    // Leave the directory a removal takes, or Windows refuses to delete it.
    let removing = |cwd: &Path| cwd.ancestors().any(|d| plan.worktrees.iter().any(|(p, f)| *f == Fate::Remove && same_path(d, p)));
    if std::env::current_dir().is_ok_and(|cwd| removing(&cwd)) {
        std::env::set_current_dir(&main).map_err(|e| format!("finish: {}: {e}", main.display()))?;
    }
    let done = apply(&main, &plan);
    for l in done.iter().chain(&kept_lines(&plan, &main)) {
        println!("finish: {l}");
    }
    let note = stop_note(a.note.as_deref(), &owed, &done);
    event::run(project, &main, &["session.stop", "--agent", agent, "--note", &note])
}

/// The worktree among `worktrees` this xtask's executable is in, if any.
fn running_in(worktrees: &[Worktree]) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    worktrees.iter().skip(1).find(|w| exe.ancestors().any(|a| same_path(a, &w.path))).map(|w| w.path.clone())
}

fn say_dry_run(plan: &Plan, main: &Path, agent: &str) {
    for (path, _) in plan.worktrees.iter().filter(|(_, f)| *f == Fate::Remove) {
        println!("finish: would remove {}", path.display());
    }
    for b in &plan.branches {
        println!("finish: would delete branch {b}");
    }
    for l in kept_lines(plan, main) {
        println!("finish: would have {l}");
    }
    println!("finish: would log session.stop for {agent} (--dry-run: nothing done)");
}

/// The stop's note: `note` when given; else what `finish` did, and what was still owed when forced.
pub fn stop_note(note: Option<&str>, owed: &Owed, done: &[String]) -> String {
    if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
        return n.to_owned();
    }
    let removed = done.iter().filter(|l| l.starts_with("removed ")).count();
    let deleted = done.iter().filter(|l| l.starts_with("deleted branch ")).count();
    let mut note = format!("finish: {removed} worktree(s) removed, {deleted} branch(es) deleted");
    if !owed.is_empty() {
        note.push_str(&format!("; forced, still owing {}", owed.describe()));
    }
    note
}

#[cfg(test)]
mod tests;
