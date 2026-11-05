//! `cargo xtask ready <pr> --agent <id> [--onto <base>]`: submit a pull request as
//! AGENTS.md §4 step 5 says, in one command, as `claim` starts one. Run in the PR's worktree:
//!
//! 1. **Checks:** the PR is open, its branch is the one checked out here, the tree is clean, and
//!    its Progress section is filled in (prwatch's ready test, [`crate::prwatch::UNFILLED`]). And
//!    it holds work ([`crate::empty`]): a branch with no commits beyond its base, only claim
//!    commits or an empty diff is refused, checked against the base before the rebase and again
//!    after it (on 2026-10-27 a PR was un-drafted with only its claim commit, and merged).
//! 2. **Rebase** onto the latest `origin/main` (fetched now), or `--onto <base>` for a PR stacked
//!    on another's branch. A conflict stops it there, the rebase left in progress and its files
//!    named: resolve them, `git rebase --continue`, and run `ready` again. Rebasing just before
//!    submitting moves conflict resolution to the author and makes the local gate test the
//!    change on the `main` the coordinator will stack it on; what lands while it waits is
//!    `prwatch`'s `conflict` (docs/swarm-protocol.md › Watching your PRs).
//! 3. **Gate:** the consumer's `cargo xtask ci --changed` on the rebased tree (`--base` the same
//!    base), run as this xtask's own exe (the `ci` contract). Some of its checks run inside xtask
//!    (`layers`' typed contracts, its code-health limits), so a rebase that brought in xtask's own
//!    changes would be judged by the rules of the xtask built before it.
//!    So when the commits since this xtask was built touch one of
//!    [`Project::xtask_rules`] (`xtask/`, the workspace manifest, the lock file, `.cargo/`), the
//!    gate runs through the rebased tree's own xtask, built into a target dir of its own (the
//!    running one can't be replaced on Windows); otherwise through this one. The same choice is made again for
//!    second gate (step 4). Each gate runs inside `sub.start --sub ready-<head> --name ready` and
//!    its `sub.stop` (`rc=0` or `rc=1`), the worker's own gate the Merge Train reads as stage 4; named apart from `gate`, the coordinator's gates, which the Gates tab counts.
//!    Its log, `.rc` record and junit file are kept as `<target>/gate-logs/ready-<head>.*`. A
//!    failure that is only failed tests is rerun once, each test alone, then the doctests and the
//!    steps it skipped, as `ready-<head>-rerun`; a pass goes on to step 4
//!    (mornycorp/swarm-core#14, [`own_gate`]).
//! 4. **Check the base once more**: the gate takes minutes, and a PR merged
//!    meanwhile makes the branch stale before it is pushed. So it fetches again; when the base
//!    moved, it rebases once more. Clean, with the PR's own diff unchanged (`git patch-id` of
//!    `base..head`, before and after), it pushes without gating again: the coordinator's smoke
//!    tests the combination, as for every merge. Clean with a changed diff (git resolved
//!    something itself), it gates again. A conflict aborts the rebase, leaves the branch as gated
//!    and stops before pushing, naming the files. One re-check only: a base that moves again is
//!    `stack`'s to handle.
//! 5. **Submit:** push (`--force-with-lease`), mark the PR ready for review (no GitHub Actions
//!    run: the coordinator's local gate checks it), and log the `submit` event, whose note names the tests a
//!    rerun passed alone. Whether it is draft is read again here, not taken from step 1: a gate
//!    sends PRs back to draft while this one runs ([`mark_ready`]).
//!
//! `--no-ci` skips steps 3 and 4 (a docs-only PR whose gate just passed, say); `--no-rerun` stops
//! at failed tests as at any failure; `--dry-run` checks and says what it would do.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::claim::issue_of_branch;
use crate::{Hooks, Project, capacity, event, gate, github};

pub mod own_gate;

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask ready <pr> --agent <id> [--onto <base>] [--no-ci] [--no-rerun] [--dry-run]";

#[derive(Debug, Default, PartialEq, Eq)]
struct Opts {
    mr: u64,
    agent: String,
    onto: Option<String>,
    no_ci: bool,
    no_rerun: bool,
    dry_run: bool,
}

fn parse_args(args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut mr = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--agent" => o.agent = it.next().ok_or("ready: --agent needs your instance id")?.to_string(),
            "--onto" => o.onto = Some(it.next().ok_or("ready: --onto needs a branch")?.to_string()),
            "--no-ci" => o.no_ci = true,
            "--no-rerun" => o.no_rerun = true,
            "--dry-run" => o.dry_run = true,
            n if mr.is_none() && n.trim_start_matches('#').parse::<u64>().is_ok() => mr = n.trim_start_matches('#').parse().ok(),
            other => return Err(format!("ready: unknown argument `{other}`\nusage: {USAGE}")),
        }
    }
    o.mr = mr.ok_or_else(|| format!("ready: which pull request?\nusage: {USAGE}"))?;
    if o.agent.is_empty() {
        return Err(format!("ready: --agent <your instance id> (it logs the submit event)\nusage: {USAGE}"));
    }
    Ok(o)
}

/// What the PR (in [`github`]'s shape) must be before it is submitted: open, from `branch`, its
/// Progress filled in. Its head branch, or why not.
fn checked(pr: &Value, branch: &str) -> Result<String, String> {
    let n = pr["number"].as_u64().unwrap_or_default();
    if pr["state"] != "open" {
        return Err(format!("ready: #{n} is {}", pr["state"].as_str().unwrap_or("not open")));
    }
    let head = pr["head_ref"].as_str().unwrap_or_default();
    if head != branch {
        return Err(format!("ready: #{n} is branch `{head}`, and `{branch}` is checked out here: run it in the PR's worktree"));
    }
    if pr["body"].as_str().unwrap_or_default().contains(crate::prwatch::UNFILLED) {
        return Err(format!("ready: #{n}'s Progress section isn't filled in (AGENTS.md §4 step 5): write it first"));
    }
    Ok(head.to_owned())
}

/// Refuse to push `branch` when another agent holds its issue: a second claim on one branch once
/// force-pushed its work over the first's (mornycorp/swarm-core#7). GitHub or the tracking branch
/// unreadable: a warning, and the push goes ahead.
fn not_held(project: &Project, root: &Path, branch: &str, agent: &str) -> Result<(), String> {
    match crate::claim::held_against(project, root, branch, agent) {
        Ok(held) if held.is_empty() => Ok(()),
        Ok(held) => {
            let list: String = held.iter().map(|h| format!("\n  - {h}")).collect();
            Err(format!(
                "ready: another agent holds the issue of `{branch}`:{list}\nThe earlier claim keeps it (AGENTS.md §4): release yours (`cargo xtask event release --issue <n> --reason duplicate`), or agree a takeover on the issue."
            ))
        }
        Err(e) => {
            eprintln!("warning: ready: couldn't check who holds the issue of `{branch}` ({e}); going ahead");
            Ok(())
        }
    }
}

/// Refuse PR `mr` when the branch checked out in `dir` holds no work against `base`.
fn has_work(dir: &Path, base: &str, mr: u64) -> Result<(), String> {
    match crate::empty::why(dir, base, "HEAD")? {
        None => Ok(()),
        Some(why) => Err(format!("ready: #{mr} changes nothing against {base}: {why}; {}", crate::empty::FINISH)),
    }
}

/// How a rebase went.
#[derive(Debug, PartialEq, Eq)]
enum Rebased {
    /// Onto `base`; whether it moved the branch.
    Done { moved: bool },
    /// Stopped on conflicts in these files, the rebase left in progress.
    Conflict(Vec<String>),
}

/// Rebase the branch checked out in `dir` onto `base`.
fn rebase(dir: &Path, base: &str) -> Result<Rebased, String> {
    let before = crate::process::git(dir, &["rev-parse", "HEAD"], None, &[])?;
    if crate::process::git(dir, &["rebase", base], None, &[]).is_ok() {
        let after = crate::process::git(dir, &["rev-parse", "HEAD"], None, &[])?;
        return Ok(Rebased::Done { moved: before != after });
    }
    let files = crate::process::git(dir, &["diff", "--name-only", "--diff-filter=U"], None, &[])?;
    let files: Vec<String> = files.lines().map(str::to_owned).collect();
    if files.is_empty() {
        // Not a conflict (a dirty tree, a bad base): put things back and say so.
        let _ = crate::process::git(dir, &["rebase", "--abort"], None, &[]);
        return Err(format!("ready: `git rebase {base}` failed without a conflict (is the tree clean?)"));
    }
    Ok(Rebased::Conflict(files))
}

fn clean(dir: &Path) -> Result<(), String> {
    let status = crate::process::git(dir, &["status", "--porcelain", "--untracked-files=no"], None, &[])?;
    if status.trim().is_empty() { Ok(()) } else { Err(format!("ready: the tree has changes not committed:\n{status}")) }
}

/// `cargo xtask ready …` ([`USAGE`]) for `project`.
pub fn run(project: &Project, hooks: &dyn Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    let o = parse_args(args)?;
    let branch = crate::process::git(root, &["rev-parse", "--abbrev-ref", "HEAD"], None, &[])?.trim().to_owned();
    let mr = github::pr(project, o.mr)?;
    checked(&mr, &branch)?;
    clean(root)?;
    not_held(project, root, &branch, &o.agent)?;
    let base = o.onto.clone().unwrap_or_else(|| format!("origin/{}", project.branches.main));
    has_work(root, &base, o.mr)?;
    if o.dry_run {
        let gate = if o.no_ci { "" } else { ", cargo xtask ci --changed, check the base again" };
        println!("ready --dry-run: #{} on `{branch}`: would rebase onto {base}{gate}, push, mark it ready for review", o.mr);
        return Ok(());
    }
    // What this xtask was built from: the branch as it was before the rebase.
    let built_from = rev(root, "HEAD")?;
    crate::process::git(root, &["fetch", "--quiet", "origin"], None, &[])?;
    match rebase(root, &base)? {
        Rebased::Conflict(files) => {
            return Err(format!(
                "ready: rebasing onto {base} stopped on conflicts in {}: resolve them, `git rebase --continue`, then run ready again",
                files.join(", ")
            ));
        }
        Rebased::Done { moved } => println!("ready: {} onto {base}", if moved { "rebased" } else { "already" }),
    }
    // The rebase drops commits `base` already has: what is left must still be work.
    has_work(root, &base, o.mr)?;
    let mut reran = Vec::new();
    if o.no_ci {
        push(root)?;
    } else {
        let logs = capacity::our_target(root)?.join(gate::LOG_DIR);
        let ci = |args: &[String]| ci_command(project, root, &built_from, args);
        let g = own_gate::OwnGate { project, hooks, agent: &o.agent, root, base: &base, logs: &logs, rerun: !o.no_rerun, ci: &ci };
        let mut log = |args: &[&str]| log_event(project, root, args);
        // Each gate is named for the HEAD it tests: a second one, after the base moved, is its own.
        gated_push(root, &base, &mut || {
            reran.extend(g.run(&mut log)?);
            Ok(())
        })?;
    }
    submit(project, root, &o, &mr, &reran)
}

/// Log an event (AGENTS.md › Tracking): never fails `ready`; offline, it queues.
fn log_event(project: &Project, root: &Path, args: &[&str]) {
    if let Err(e) = event::run(project, root, args) {
        eprintln!("warning: the {} event: {e}", args.first().unwrap_or(&"?"));
    }
}

/// Which xtask gates the tree checked out in `dir`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateWith {
    /// This one: nothing it is built from changed since `built_from`.
    ThisXtask,
    /// The tree's own, rebuilt: one of [`Project::xtask_rules`] changed since this xtask was built.
    Rebuilt,
}

/// Which xtask gates `dir`'s `HEAD`, this one having been built from `built_from`.
fn gate_with(project: &Project, dir: &Path, built_from: &str) -> Result<GateWith, String> {
    let mut args = vec!["diff", "--name-only", built_from, "HEAD", "--"];
    args.extend(project.xtask_rules);
    let changed = crate::process::git(dir, &args, None, &[])?;
    Ok(if changed.trim().is_empty() { GateWith::ThisXtask } else { GateWith::Rebuilt })
}

/// The command that runs the consumer's `ci <args>` at `root`: with this xtask, or the rebased
/// tree's own when one of [`Project::xtask_rules`] changed since this one was built from
/// `built_from`.
fn ci_command(project: &Project, root: &Path, built_from: &str, args: &[String]) -> Result<Command, String> {
    match gate_with(project, root, built_from)? {
        GateWith::ThisXtask => {
            let exe = std::env::current_exe().map_err(|e| format!("ready: this xtask's path: {e}"))?;
            Ok(this_ci_command(project, &exe, root, args))
        }
        GateWith::Rebuilt => {
            let mut ci = Command::new(rebuilt_xtask(root)?);
            ci.current_dir(root).env_remove("CARGO_MANIFEST_DIR").arg("ci").args(args);
            Ok(ci)
        }
    }
}

/// `ci <args>` at `root`, as this xtask's own exe (the `ci` contract). [`gate_with`] has just
/// found this exe built from the tree's xtask rules, by content, so it says so to a consumer that
/// checks by modification time (`{prefix}_XTASK_STALE=ok`): a rebase that replays commits onto a
/// base without them writes their files again, the same bytes with new times
/// (mornycorp/swarm-core#2).
fn this_ci_command(project: &Project, exe: &Path, root: &Path, args: &[String]) -> Command {
    let mut ci = Command::new(exe);
    ci.current_dir(root).env(project.env_name("XTASK_STALE"), "ok").arg("ci").args(args);
    ci
}

/// The tree at `root`'s own xtask, built into `<target>/ready-xtask` (never over the running one).
fn rebuilt_xtask(root: &Path) -> Result<PathBuf, String> {
    println!("ready: the rebase changed xtask's own rules: gating with the rebased xtask");
    let target = capacity::target_dir(root)?.join("ready-xtask");
    let mut build = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    build.current_dir(root).env_remove("CARGO_MANIFEST_DIR").args(["build", "--quiet", "-p", "xtask", "--target-dir"]).arg(&target);
    run_status(build, "cargo build -p xtask (the rebased tree's)")?;
    Ok(target.join("debug").join(format!("xtask{}", std::env::consts::EXE_SUFFIX)))
}

/// Run `cmd` with its output on ours; an error, named `what`, unless it succeeds.
fn run_status(mut cmd: Command, what: &str) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what} failed ({status})")) }
}

fn rev(dir: &Path, name: &str) -> Result<String, String> {
    Ok(crate::process::git(dir, &["rev-parse", name], None, &[])?.trim().to_owned())
}

fn push(dir: &Path) -> Result<(), String> {
    crate::process::git(dir, &["push", "--quiet", "--force-with-lease", "origin", "HEAD"], None, &[]).map(drop)
}

/// The commits the gate ran on: the base's and the branch's.
struct Gated {
    base: String,
    head: String,
}

/// What checking the base once more, after the gate, found.
#[derive(Debug, PartialEq, Eq)]
enum Recheck {
    /// It hasn't moved: push as gated.
    Unmoved,
    /// It moved by `merges` (its first-parent commits) and the branch rebased onto it cleanly;
    /// `same` when the PR's own diff is the one gated.
    Rebased { merges: usize, same: bool },
    /// Rebasing onto it stopped on conflicts in `files`: aborted, the branch left as gated.
    Conflict { merges: usize, files: Vec<String> },
}

/// The diff `base..head` as `git patch-id --stable` sees it: the same id for the same change,
/// whatever its line numbers.
fn diff_id(dir: &Path, base: &str, head: &str) -> Result<String, String> {
    let diff = crate::process::git(dir, &["diff", base, head], None, &[])?;
    crate::process::git(dir, &["patch-id", "--stable"], Some(diff.as_bytes()), &[])
}

/// Fetch, and when `base` moved since `gated`, rebase onto it once more.
fn recheck(dir: &Path, base: &str, gated: &Gated) -> Result<Recheck, String> {
    crate::process::git(dir, &["fetch", "--quiet", "origin"], None, &[])?;
    let now = rev(dir, base)?;
    if now == gated.base {
        return Ok(Recheck::Unmoved);
    }
    let range = format!("{}..{now}", gated.base);
    let merges = crate::process::git(dir, &["rev-list", "--count", "--first-parent", &range], None, &[])?.trim().parse().unwrap_or_default();
    if let Rebased::Conflict(files) = rebase(dir, &now)? {
        crate::process::git(dir, &["rebase", "--abort"], None, &[])?;
        return Ok(Recheck::Conflict { merges, files });
    }
    let same = diff_id(dir, &gated.base, &gated.head)? == diff_id(dir, &now, "HEAD")?;
    Ok(Recheck::Rebased { merges, same })
}

/// How `ready` says `base` moved by `merges` while it gated.
fn moved(base: &str, merges: usize) -> String {
    format!("ready: {base} moved ({merges} merge{}) during the gate", if merges == 1 { "" } else { "s" })
}

/// Gate the rebased branch, check `base` once more, and push: without gating again when
/// the PR's diff is unchanged, after a second gate when it changed, not at all on a conflict.
fn gated_push(dir: &Path, base: &str, gate: &mut dyn FnMut() -> Result<(), String>) -> Result<(), String> {
    let gated = Gated { base: rev(dir, base)?, head: rev(dir, "HEAD")? };
    gate()?;
    match recheck(dir, base, &gated)? {
        Recheck::Unmoved => {}
        Recheck::Rebased { merges, same: true } => {
            println!("{}; rebased cleanly, the PR's diff unchanged; pushing", moved(base, merges));
        }
        Recheck::Rebased { merges, same: false } => {
            println!("{}; rebased cleanly, but the PR's diff changed: gating again", moved(base, merges));
            gate()?;
        }
        Recheck::Conflict { merges, files } => {
            return Err(format!(
                "{}, and rebasing onto it stops on conflicts in {}: nothing pushed, the branch left as gated. Rebase onto {base}, resolve them, `git rebase --continue`, then run ready again",
                moved(base, merges),
                files.join(", ")
            ));
        }
    }
    push(dir)
}

/// Mark ready, log `submit`: the branch is pushed. Its note names the tests that passed only when
/// rerun alone (`reran`), for the coordinator (mornycorp/swarm-core#14).
fn submit(project: &Project, root: &Path, o: &Opts, pr: &Value, reran: &[own_gate::Reran]) -> Result<(), String> {
    mark_ready(&mut || github::pr(project, o.mr), &mut || github::mark_ready(project, o.mr))?;
    println!("ready: #{} pushed and ready for review", o.mr);
    let mut ev = vec!["submit".to_owned(), "--agent".to_owned(), o.agent.clone(), "--mr".to_owned(), o.mr.to_string()];
    let branch = pr["head_ref"].as_str().unwrap_or_default();
    if let Some(issue) = issue_of_branch(branch) {
        ev.extend(["--issue".to_owned(), issue.to_string()]);
    }
    if !reran.is_empty() {
        let note = reran.iter().map(own_gate::Reran::note).collect::<Vec<_>>().join("; ");
        ev.extend(["--note".to_owned(), gate::one_line(&note, event::MAX_NOTE)]);
    }
    // Never fails the submit (AGENTS.md › Tracking): offline, the event queues.
    if let Err(e) = event::run(project, root, &ev.iter().map(String::as_str).collect::<Vec<_>>()) {
        eprintln!("warning: the submit event: {e}");
    }
    Ok(())
}

/// Mark the PR ready for review (`ready`), reading it first (`read`): `run` read it before the
/// gate, minutes ago, and its draft state may have changed since (on 2026-10-28 a coordinator's
/// gate sent it back to draft). Read again after: a PR still draft is an error, as its checks
/// wouldn't start.
fn mark_ready(read: &mut dyn FnMut() -> Result<Value, String>, ready: &mut dyn FnMut() -> Result<(), String>) -> Result<(), String> {
    if read()?["draft"] != true {
        return Ok(());
    }
    ready()?;
    let after = read()?;
    if after["draft"] == true {
        return Err(format!("ready: #{} is still draft after marking it ready: mark it ready by hand (`gh pr ready`)", after["number"]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::APP_SWARM as P;
    use crate::stack::testgit::{self, tgit};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn options() {
        let o = parse_args(&["#42", "--agent", "a-1", "--onto", "41-base", "--no-ci"]).unwrap();
        assert_eq!(o, Opts { mr: 42, agent: "a-1".into(), onto: Some("41-base".into()), no_ci: true, no_rerun: false, dry_run: false });
        assert!(parse_args(&["42", "--agent", "a", "--dry-run"]).unwrap().dry_run);
        assert!(parse_args(&["42", "--agent", "a", "--no-rerun"]).unwrap().no_rerun);
        for bad in [&["--agent", "a"][..], &["42"], &["42", "--agent"], &["42", "--agent", "a", "--nope"], &["42", "43", "--agent", "a"]] {
            assert!(parse_args(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_pr_is_checked_before_it_is_submitted() {
        let filled = "## Progress\n\n- **Measured:** 12 tests\n";
        let pr = |state: &str, branch: &str, body: &str| json!({"number": 7, "state": state, "head_ref": branch, "body": body});
        assert_eq!(checked(&pr("open", "7-x", filled), "7-x"), Ok("7-x".into()));
        assert!(checked(&pr("merged", "7-x", filled), "7-x").unwrap_err().contains("is merged"));
        assert!(checked(&pr("open", "7-x", filled), "8-y").unwrap_err().contains("run it in the PR's worktree"));
        let unfilled = "## Progress\n\n- **Measured:** <!-- numbers -->\n";
        assert!(checked(&pr("open", "7-x", unfilled), "7-x").unwrap_err().contains("Progress"));
    }

    /// #8: `run` read the PR ready before the gate, and a coordinator's gate sent it back to
    /// draft during it. The PR is read again and marked ready; one ready already is left alone.
    #[test]
    fn a_pr_sent_back_to_draft_during_the_gate_is_marked_ready_again() {
        let reads = std::cell::RefCell::new(vec![json!({"number": 7, "draft": true}), json!({"number": 7, "draft": false})]);
        let marked = std::cell::Cell::new(0);
        mark_ready(&mut || Ok(reads.borrow_mut().remove(0)), &mut || {
            marked.set(marked.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(marked.get(), 1);
        // Ready already (marked by hand during the gate, say): nothing to do.
        mark_ready(&mut || Ok(json!({"number": 7, "draft": false})), &mut || panic!("not marked again")).unwrap();
    }

    #[test]
    fn a_pr_still_draft_after_marking_is_an_error() {
        let err = mark_ready(&mut || Ok(json!({"number": 7, "draft": true})), &mut || Ok(())).unwrap_err();
        assert!(err.contains("#7 is still draft"), "{err}");
    }

    /// A clone with `main` on a bare remote, `file` its only file.
    fn repo(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = swarm_core_testkit::temp_dir(name);
        let (remote, clone) = (dir.join("remote.git"), dir.join("clone"));
        crate::process::git(&dir, &["init", "--quiet", "--bare", "-b", "main", remote.to_str().unwrap()], None, &[]).unwrap();
        crate::process::git(&dir, &["clone", "--quiet", remote.to_str().unwrap(), clone.to_str().unwrap()], None, &[]).unwrap();
        // A rebase commits: an identity of its own, whatever the machine's (CI's runner has none).
        for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgsign", "false")] {
            crate::process::git(&clone, &["config", k, v], None, &[]).unwrap();
        }
        commit(&clone, "file", "base\n");
        crate::process::git(&clone, &["push", "--quiet", "origin", "HEAD:main"], None, &[]).unwrap();
        (dir, clone)
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        crate::process::git(dir, &["add", file], None, &[]).unwrap();
        crate::process::git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", file], None, &[]).unwrap();
    }

    /// `main` moves on the remote (another PR merged) while `7-x` waits.
    fn main_moves(clone: &Path, file: &str, text: &str) {
        crate::process::git(clone, &["checkout", "--quiet", "-b", "other", "origin/main"], None, &[]).unwrap();
        commit(clone, file, text);
        crate::process::git(clone, &["push", "--quiet", "origin", "HEAD:main"], None, &[]).unwrap();
        crate::process::git(clone, &["checkout", "--quiet", "7-x"], None, &[]).unwrap();
        crate::process::git(clone, &["fetch", "--quiet", "origin"], None, &[]).unwrap();
    }

    #[test]
    fn a_branch_rebases_onto_main_that_moved() {
        let (dir, clone) = repo("xtask-ready-rebase");
        crate::process::git(&clone, &["checkout", "--quiet", "-b", "7-x"], None, &[]).unwrap();
        commit(&clone, "mine", "my change\n");
        main_moves(&clone, "theirs", "their change\n");
        assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Done { moved: true }));
        assert!(clone.join("theirs").is_file() && clone.join("mine").is_file(), "both changes");
        assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Done { moved: false }), "already on it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conflict_stops_the_rebase_and_names_its_files() {
        let (dir, clone) = repo("xtask-ready-conflict");
        crate::process::git(&clone, &["checkout", "--quiet", "-b", "7-x"], None, &[]).unwrap();
        commit(&clone, "file", "mine\n");
        main_moves(&clone, "file", "theirs\n");
        assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Conflict(vec!["file".into()])));
        assert!(clean(&clone).is_err(), "left in progress for the author to resolve");
        let _ = crate::process::git(&clone, &["rebase", "--abort"], None, &[]);
        assert_eq!(clean(&clone), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A branch with only its claim commit is refused, before the rebase and after it; one
    /// with a real change passes; one whose change `main` has since taken is refused once rebased.
    #[test]
    fn an_mr_with_no_work_beyond_its_claim_is_refused() {
        let (dir, clone) = repo("xtask-ready-empty");
        crate::process::git(&clone, &["checkout", "--quiet", "-b", "7-x"], None, &[]).unwrap();
        crate::process::git(&clone, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", "Claim #7: x"], None, &[])
            .unwrap();
        let why = has_work(&clone, "origin/main", 7).unwrap_err();
        assert!(why.contains("#7 changes nothing") && why.contains("only its claim commit") && why.contains("close the PR"), "{why}");
        main_moves(&clone, "theirs", "their change\n");
        assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Done { moved: true }));
        assert!(has_work(&clone, "origin/main", 7).unwrap_err().contains("only its claim commit"), "and after the rebase");

        commit(&clone, "mine", "my change\n");
        assert_eq!(has_work(&clone, "origin/main", 7), Ok(()), "a real change passes");

        // The same change merged to main by another PR: the rebase drops it, nothing is left.
        crate::process::git(&clone, &["checkout", "--quiet", "-b", "other2", "origin/main"], None, &[]).unwrap();
        commit(&clone, "mine", "my change\n");
        crate::process::git(&clone, &["push", "--quiet", "origin", "HEAD:main"], None, &[]).unwrap();
        crate::process::git(&clone, &["checkout", "--quiet", "7-x"], None, &[]).unwrap();
        crate::process::git(&clone, &["fetch", "--quiet", "origin"], None, &[]).unwrap();
        assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Done { moved: true }));
        assert!(has_work(&clone, "origin/main", 7).is_err(), "nothing of its own once rebased");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The PR's file, twelve lines.
    const FILE: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";

    /// A clone of an origin whose `main` has [`FILE`] and whose PR branch `7-x` changes it to
    /// `mine`, checked out on `7-x`. The directory, the origin and the clone.
    fn gated_repo(name: &str, mine: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = swarm_core_testkit::temp_dir(name);
        let (origin, clone) = (dir.join("origin.git"), dir.join("clone"));
        testgit::bare(&origin);
        let mut h = testgit::History::default();
        let main = h.commit("main", &[], &[("file", FILE)], "main");
        h.commit("7-x", &[&main], &[("file", mine)], "mine");
        h.import(&origin);
        testgit::clone(&origin, &clone);
        tgit(&clone, &["checkout", "--quiet", "7-x"]);
        (dir, origin, clone)
    }

    /// A gate that counts its runs, and merges `theirs` (files) to the origin's `main` while it
    /// runs, as another PR landing during a 15-minute gate does.
    fn gate_that_merges<'a>(origin: &'a Path, runs: &'a mut usize, theirs: &'a [(&'a str, &'a str)]) -> impl FnMut() -> Result<(), String> + 'a {
        move || {
            *runs += 1;
            if !theirs.is_empty() {
                testgit::push_commit(origin, "main", &["main"], theirs, "theirs");
            }
            Ok(())
        }
    }

    fn pushed(origin: &Path) -> String {
        tgit(origin, &["rev-parse", "7-x"]).trim().to_owned()
    }

    #[test]
    fn main_unmoved_pushes_as_gated() {
        let (dir, origin, clone) = gated_repo("xtask-ready-unmoved", &FILE.replace("b\n", "B\n"));
        tgit(&clone, &["commit", "--quiet", "--allow-empty", "-m", "a fix"]);
        let mut runs = 0;
        assert_eq!(gated_push(&clone, "origin/main", &mut gate_that_merges(&origin, &mut runs, &[])), Ok(()));
        assert_eq!(runs, 1);
        assert_eq!(pushed(&origin), rev(&clone, "HEAD").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_moved_cleanly_pushes_without_a_second_gate() {
        let (dir, origin, clone) = gated_repo("xtask-ready-moved-clean", &FILE.replace("b\n", "B\n"));
        let mut runs = 0;
        let theirs = [("theirs", "their change\n")];
        assert_eq!(gated_push(&clone, "origin/main", &mut gate_that_merges(&origin, &mut runs, &theirs)), Ok(()));
        assert_eq!(runs, 1, "the PR's diff is the one gated");
        assert_eq!(pushed(&origin), rev(&clone, "HEAD").unwrap());
        assert_eq!(rev(&clone, "HEAD^").unwrap(), tgit(&origin, &["rev-parse", "main"]).trim(), "onto the newer main");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_moved_with_a_changed_diff_gates_again_and_rechecks_once() {
        // main makes one of the PR's two changes itself: git takes it cleanly, and the PR's diff
        // shrinks. The second gate merges yet another PR, which is `stack`'s to handle.
        let (dir, origin, clone) = gated_repo("xtask-ready-moved-changed", &FILE.replace("b\n", "B\n").replace("k\n", "K\n"));
        let mut runs = 0;
        let theirs = [("file", &*FILE.replace("k\n", "K\n"))];
        assert_eq!(gated_push(&clone, "origin/main", &mut gate_that_merges(&origin, &mut runs, &theirs)), Ok(()));
        assert_eq!(runs, 2, "gated again");
        assert_eq!(pushed(&origin), rev(&clone, "HEAD").unwrap());
        assert_ne!(rev(&clone, "HEAD^").unwrap(), tgit(&origin, &["rev-parse", "main"]).trim(), "one re-check only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_moved_with_a_conflict_stops_before_pushing() {
        let (dir, origin, clone) = gated_repo("xtask-ready-moved-conflict", &FILE.replace("d\n", "mine\n"));
        let (gated, before) = (rev(&clone, "HEAD").unwrap(), pushed(&origin));
        let mut runs = 0;
        let theirs = [("file", &*FILE.replace("d\n", "theirs\n"))];
        let err = gated_push(&clone, "origin/main", &mut gate_that_merges(&origin, &mut runs, &theirs)).unwrap_err();
        assert!(err.contains("(1 merge)") && err.contains("conflicts in file") && err.contains("nothing pushed"), "{err}");
        assert_eq!(runs, 1);
        assert_eq!(pushed(&origin), before, "nothing pushed");
        assert_eq!(rev(&clone, "HEAD").unwrap(), gated, "the branch left as gated");
        assert_eq!(clean(&clone), Ok(()), "the rebase aborted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What each gate of `gated_push` would gate with, this xtask built from `built_from`.
    fn gates_with(clone: &Path, built_from: &str, theirs: &[(&str, &str)]) -> Vec<GateWith> {
        let origin = clone.parent().unwrap().join("origin.git");
        let mut seen = Vec::new();
        let mut gate = || {
            seen.push(gate_with(&P, clone, built_from)?);
            if seen.len() == 1 && !theirs.is_empty() {
                testgit::push_commit(&origin, "main", &["main"], theirs, "theirs");
            }
            Ok(())
        };
        gated_push(clone, "origin/main", &mut gate).unwrap();
        seen
    }

    /// Gating with this exe, `ready` tells the consumer's `ci` the exe is current: the rebase may
    /// have given unchanged files new times, which such a consumer's `ci` checks (#2).
    #[test]
    fn this_xtask_gates_as_current() {
        for project in crate::project::fixtures::BOTH {
            let ci = this_ci_command(&project, Path::new("xtask"), Path::new("."), &own_gate::scope("origin/main"));
            let var = project.env_name("XTASK_STALE");
            let set: Vec<_> = ci.get_envs().filter(|(k, _)| *k == var.as_str()).map(|(_, v)| v.and_then(|v| v.to_str())).collect();
            assert_eq!(set, [Some("ok")], "{}", project.name);
            assert_eq!(ci.get_args().collect::<Vec<_>>(), ["ci", "--changed", "--base", "origin/main"]);
        }
    }

    #[test]
    fn a_rebase_that_changes_xtask_gates_with_the_rebased_xtask() {
        // main changes an xtask rule (#249 lowered an allow-list entry) while the PR waits.
        let (dir, origin, clone) = gated_repo("xtask-ready-rules", &FILE.replace("b\n", "B\n"));
        let built_from = rev(&clone, "HEAD").unwrap();
        testgit::push_commit(&origin, "main", &["main"], &[("xtask/src/contracts.rs", "count: 1\n")], "a rule");
        tgit(&clone, &["fetch", "--quiet", "origin"]);
        assert_eq!(gate_with(&P, &clone, &built_from), Ok(GateWith::ThisXtask), "before the rebase: the rules it was built from");
        assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Done { moved: true }));
        assert_eq!(gate_with(&P, &clone, &built_from), Ok(GateWith::Rebuilt), "the rebased rules, not the old");
        assert_eq!(gates_with(&clone, &built_from, &[]), [GateWith::Rebuilt]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rebase_that_leaves_xtask_alone_gates_in_process() {
        let (dir, origin, clone) = gated_repo("xtask-ready-no-rules", &FILE.replace("b\n", "B\n"));
        let built_from = rev(&clone, "HEAD").unwrap();
        testgit::push_commit(&origin, "main", &["main"], &[("crates/ui/src/lib.rs", "fn ui() {}\n")], "not a rule");
        tgit(&clone, &["fetch", "--quiet", "origin"]);
        assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Done { moved: true }));
        assert_eq!(gates_with(&clone, &built_from, &[]), [GateWith::ThisXtask]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_manifest_the_lock_file_and_cargo_config_build_xtask_too() {
        for (k, file) in ["Cargo.lock", "Cargo.toml", ".cargo/config.toml"].into_iter().enumerate() {
            let (dir, origin, clone) = gated_repo(&format!("xtask-ready-builds-{k}"), &FILE.replace("b\n", "B\n"));
            let built_from = rev(&clone, "HEAD").unwrap();
            testgit::push_commit(&origin, "main", &["main"], &[(file, "changed\n")], file);
            tgit(&clone, &["fetch", "--quiet", "origin"]);
            assert_eq!(rebase(&clone, "origin/main"), Ok(Rebased::Done { moved: true }));
            assert_eq!(gate_with(&P, &clone, &built_from), Ok(GateWith::Rebuilt), "{file}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn the_second_gate_after_a_recheck_chooses_again() {
        // During the first gate main changes the PR's own line (so its diff changes and #231 gates
        // again) and an xtask rule: the second gate is the rebased xtask's.
        let (dir, _origin, clone) = gated_repo("xtask-ready-recheck-rules", &FILE.replace("b\n", "B\n").replace("k\n", "K\n"));
        let built_from = rev(&clone, "HEAD").unwrap();
        let theirs = [("file", &*FILE.replace("k\n", "K\n")), ("xtask/src/layers.rs", "a new rule\n")];
        assert_eq!(gates_with(&clone, &built_from, &theirs), [GateWith::ThisXtask, GateWith::Rebuilt]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
