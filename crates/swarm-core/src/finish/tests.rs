use super::*;
use crate::project::fixtures::APP_SWARM;
use swarm_core_testkit::git::{git as tgit, remote};

fn mr(number: u64, state: &str, branch: &str) -> AgentPr {
    AgentPr { number, state: state.to_owned(), branch: branch.to_owned() }
}

fn wt(path: &str, branch: Option<&str>) -> Worktree {
    Worktree { path: PathBuf::from(path), branch: branch.map(str::to_owned) }
}

#[test]
fn worktrees_are_read_with_their_branches() {
    let porcelain = "worktree D:/dev/si\nHEAD 1111\nbranch refs/heads/main\n\nworktree D:/dev/si-rowan\nHEAD 2222\nbranch refs/heads/311-idle\n\nworktree D:/dev/si-x\nHEAD 3333\ndetached\n";
    assert_eq!(parse_worktrees(porcelain), [wt("D:/dev/si", Some("main")), wt("D:/dev/si-rowan", Some("311-idle")), wt("D:/dev/si-x", None)]);
}

/// An open PR or an open claimed issue is owed, and named.
#[test]
fn what_is_owed_is_named() {
    let mrs = [mr(12, "open", "a"), mr(13, "merged", "b"), mr(14, "closed", "c"), mr(15, "open", "d")];
    let owed = owed(&mrs, &[5]);
    assert_eq!(owed, Owed { mrs: vec![12, 15], issues: vec![5] });
    assert_eq!(owed.describe(), "PR #12 open, PR #15 open, #5 claimed and open");
    assert!(super::owed(&[mr(13, "merged", "b")], &[]).is_empty());
}

/// Only the agent's worktrees, never the main one: clean ones go, a dirty one, one whose PR is
/// open and the one xtask runs from stay; a merged PR's branch goes unless a kept worktree has it.
#[test]
fn the_plan_removes_only_the_agents_clean_worktrees() {
    let worktrees = [
        wt("D:/dev/si", Some("1-main-is-mine-too")),
        wt("D:/dev/si-a", Some("1-a")),
        wt("D:/dev/si-b", Some("2-b")),
        wt("D:/dev/si-c", Some("3-c")),
        wt("D:/dev/si-d", Some("4-d")),
        wt("D:/dev/si-other", Some("9-someone-elses")),
        wt("D:/dev/si-detached", None),
    ];
    let mrs = [
        mr(1, "merged", "1-main-is-mine-too"),
        mr(2, "merged", "1-a"),
        mr(3, "merged", "2-b"),
        mr(4, "open", "3-c"),
        mr(5, "closed", "4-d"),
        mr(6, "merged", "5-no-worktree"),
    ];
    let dirty = |p: &Path| p.ends_with("si-b");
    let plan = plan(&worktrees, &mrs, Some(Path::new("d:\\dev\\SI-D")), &dirty);
    let fates: Vec<(&str, Fate)> = plan.worktrees.iter().map(|(p, f)| (p.to_str().unwrap(), *f)).collect();
    let running = if cfg!(windows) { Fate::Running } else { Fate::Remove };
    assert_eq!(fates, [("D:/dev/si-a", Fate::Remove), ("D:/dev/si-b", Fate::Dirty), ("D:/dev/si-c", Fate::OpenMr), ("D:/dev/si-d", running)]);
    // 1's branch is the main worktree's, 2-b a kept worktree's; closed PRs' branches stay.
    assert_eq!(plan.branches, ["1-a", "5-no-worktree"]);

    let kept = kept_lines(&plan, Path::new("D:/dev/si"));
    assert!(kept.iter().any(|l| l.contains("si-b") && l.contains("has changes")), "{kept:?}");
    assert!(kept.iter().any(|l| l.contains("si-c") && l.contains("PR is open")), "{kept:?}");
    assert_eq!(kept.iter().any(|l| l.contains("git -C D:/dev/si worktree remove D:/dev/si-d")), cfg!(windows), "{kept:?}");
}

#[test]
fn the_stop_note_says_what_finish_did() {
    let done = ["removed D:/dev/si-a".to_owned(), "deleted branch 1-a".to_owned(), "couldn't remove D:/dev/si-b: busy".to_owned()];
    assert_eq!(stop_note(None, &Owed::default(), &done), "finish: 1 worktree(s) removed, 1 branch(es) deleted");
    let owed = Owed { mrs: vec![7], issues: vec![] };
    assert_eq!(stop_note(None, &owed, &[]), "finish: 0 worktree(s) removed, 0 branch(es) deleted; forced, still owing PR #7 open");
    assert_eq!(stop_note(Some("no unclaimed issues left"), &owed, &done), "no unclaimed issues left");
    assert_eq!(stop_note(Some("  "), &Owed::default(), &[]), "finish: 0 worktree(s) removed, 0 branch(es) deleted");
}

#[test]
fn finish_takes_its_flags() {
    assert_eq!(
        parse_args(&["--agent", "pc-1", "--force", "--dry-run", "--note", "done"]).unwrap(),
        Args { agent: Some("pc-1".into()), note: Some("done".into()), force: true, dry_run: true }
    );
    assert!(parse_args(&["--agent"]).is_err());
    assert!(parse_args(&["--bogus"]).unwrap_err().contains("usage"));
}

/// A clone with the agent's worktrees: `wt-a` (merged, clean), `wt-b` (merged, an untracked
/// file), `wt-c` (someone else's), and a merged branch `4-d` with no worktree.
fn clone_with_worktrees(name: &str) -> (PathBuf, PathBuf) {
    let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-finish-{name}"));
    let r = remote(&dir);
    for (wt, branch) in [("wt-a", "1-a"), ("wt-b", "2-b"), ("wt-c", "3-c")] {
        tgit(&r.work, &["worktree", "add", "--quiet", "-b", branch, &dir.join(wt).to_string_lossy()]);
    }
    tgit(&r.work, &["branch", "4-d"]);
    std::fs::write(dir.join("wt-b").join("notes.txt"), "not committed").unwrap();
    (dir, r.work)
}

fn agents_mrs() -> Asked {
    Asked { mrs: vec![mr(1, "merged", "1-a"), mr(2, "merged", "2-b"), mr(4, "merged", "4-d")], open_issues: vec![] }
}

fn branches(work: &Path) -> String {
    tgit(work, &["branch", "--format=%(refname:short)"])
}

/// sc#10: `finish` removes the agent's clean worktree, keeps the dirty one and another's, deletes
/// its merged branches, installs the tools and logs `session.stop` on the tracking branch.
#[test]
fn finish_cleans_up_then_logs_the_stop() {
    let (dir, work) = clone_with_worktrees("cleans");
    finish(&APP_SWARM, &work, "pc-f00d01", &agents_mrs(), &Args::default()).unwrap();
    assert!(!dir.join("wt-a").exists(), "the clean worktree is removed");
    assert!(dir.join("wt-b").join("notes.txt").exists(), "the dirty one is kept");
    assert!(dir.join("wt-c").exists(), "another agent's is never touched");
    let left = branches(&work);
    assert!(!left.contains("1-a") && !left.contains("4-d"), "merged branches deleted: {left}");
    assert!(left.contains("2-b") && left.contains("3-c"), "a kept worktree's branch and another's stay: {left}");
    let exe = std::env::current_exe().unwrap();
    assert!(work.join(".git").join(TOOLS_DIR).join(exe.file_name().unwrap()).is_file(), "the tools are installed");
    let stops = tgit(&dir.join("origin.git"), &["show", "tracking:tracking/pc-f00d01/work.jsonl"]);
    assert!(stops.contains(r#""event":"session.stop""#) && stops.contains("1 worktree(s) removed, 2 branch(es) deleted"), "{stops}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// sc#12: a worktree's ignored target dir goes with it (deleted first, outside git's deadline), and
/// a worktree a removal cut short (its directory gone, still registered) is pruned, so its merged
/// branch can be deleted.
#[test]
fn finish_removes_a_target_dir_and_prunes_a_half_removed_worktree() {
    let dir = swarm_core_testkit::temp_dir("swarm-core-finish-target");
    let r = remote(&dir);
    std::fs::write(r.work.join(".gitignore"), "target/\n").unwrap();
    tgit(&r.work, &["add", ".gitignore"]);
    tgit(&r.work, &["commit", "--quiet", "-m", "ignore target"]);
    for (wt, branch) in [("wt-a", "1-a"), ("wt-e", "5-e")] {
        tgit(&r.work, &["worktree", "add", "--quiet", "-b", branch, &dir.join(wt).to_string_lossy()]);
    }
    let build = dir.join("wt-a").join("target").join("agent-a").join("debug");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("xtask.exe"), vec![0u8; 1 << 20]).unwrap();
    // wt-e's removal was cut short: its directory is gone, git still lists it.
    std::fs::remove_dir_all(dir.join("wt-e")).unwrap();

    let asked = Asked { mrs: vec![mr(1, "merged", "1-a"), mr(5, "merged", "5-e")], open_issues: vec![] };
    finish(&APP_SWARM, &r.work, "pc-f00d03", &asked, &Args::default()).unwrap();
    assert!(!dir.join("wt-a").exists(), "the worktree and its target dir are removed");
    let left = branches(&r.work);
    assert!(!left.contains("1-a") && !left.contains("5-e"), "both merged branches deleted: {left}");
    assert!(!tgit(&r.work, &["worktree", "list"]).contains("wt-e"), "the half-removed worktree is pruned");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An open PR or claim stops `finish` before it touches anything; `--force` goes on and says what
/// was owed in the stop; `--dry-run` does nothing.
#[test]
fn finish_refuses_while_work_is_owed() {
    let (dir, work) = clone_with_worktrees("owed");
    let mut asked = agents_mrs();
    asked.mrs.push(mr(5, "open", "3-c"));
    asked.open_issues = vec![9];
    let e = finish(&APP_SWARM, &work, "pc-f00d02", &asked, &Args::default()).unwrap_err();
    assert!(e.contains("PR #5 open, #9 claimed and open") && e.contains("--force"), "{e}");
    assert!(dir.join("wt-a").exists() && branches(&work).contains("4-d"), "nothing done");

    for force in [false, true] {
        finish(&APP_SWARM, &work, "pc-f00d02", &asked, &Args { force, dry_run: true, ..Args::default() }).unwrap();
        assert!(dir.join("wt-a").exists() && branches(&work).contains("4-d"), "--dry-run says what is owed and does nothing");
    }

    finish(&APP_SWARM, &work, "pc-f00d02", &asked, &Args { force: true, ..Args::default() }).unwrap();
    assert!(!dir.join("wt-a").exists() && dir.join("wt-c").exists(), "forced: its open PR's worktree is kept");
    let stops = tgit(&dir.join("origin.git"), &["show", "tracking:tracking/pc-f00d02/work.jsonl"]);
    assert!(stops.contains("forced, still owing PR #5 open, #9 claimed and open"), "{stops}");
    let _ = std::fs::remove_dir_all(&dir);
}
