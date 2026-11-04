//! A gate of the same unchanged stack straight after a failure is its rerun
//! (mornycorp/swarm-core#20).
//!
//! A reader counts a failed gate's PRs as sent back unless their next gate is a rerun saying so
//! (`rerun of <sub>:`, docs/tracking-format.md › Send-backs). A coordinator that just runs `gate`
//! again on the stack that failed, with no such note, turned a flake into a send-back. So `gate`
//! writes the note itself: each gate records what it gates ([`of_stack`], [`of_merge`],
//! [`of_head`]) in [`LAST`] beside its logs, and the next gate whose record is the same, while
//! that last gate's `.rc` says it failed, starts as its rerun ([`rerun_of`], [`note`]). Every
//! reader's rule stays as it is.
//!
//! - **The same stack** is the same PRs, in the same order, at the same heads (each entry's
//!   pre-rebase tip: a new push is a new head), stacked onto the same `main`: a restack of an
//!   unchanged stack is the same, though its rebased commits are new. `--merge` branches are the
//!   same `origin/main` and the same branches at the same commits; a tree with no stack record is
//!   the same HEAD.
//! - **The last gate** is this worktree's (its target dir's `gate-logs`): one that passed, has no
//!   result (stopped part way) or was followed by a gate of something else is rerun by nothing.
//!   A gate that reuses the last gate's own `sub` is not called its rerun: a reader keys gates by
//!   `sub`, and the two would read as one.

use std::path::Path;

use super::{BASE, tokens};
use crate::stack::{self, Stack};

/// The file in `gate-logs` that names the last gate and what it gated: `<sub> <what>`.
pub const LAST: &str = "last-gate";

/// What a stack record gates: `<remote>/<base>@<its tip> #<pr>@<head> …`, the heads before the
/// rebase; `None` for an empty record, or one from before the record named its base.
pub fn of_stack(record: &Stack) -> Option<String> {
    let onto = record.entries.first().map(|e| e.onto.as_str()).filter(|o| !o.is_empty())?;
    let mut s = format!("{}/{}@{onto}", record.remote, record.base);
    for e in &record.entries {
        s.push_str(&format!(" #{}@{}", e.mr, e.old_tip));
    }
    Some(s)
}

/// What `--merge` gates: `origin/main@<sha> <branch>@<sha> …`, each at the commit merged.
pub fn of_merge(base: &str, branches: &[(String, String)]) -> String {
    let mut s = format!("{BASE}@{base}");
    for (b, sha) in branches {
        s.push_str(&format!(" {b}@{sha}"));
    }
    s
}

/// What a tree with no stack record gates: its HEAD.
pub fn of_head(head: &str) -> String {
    format!("HEAD@{head}")
}

/// The gate a gate `sub` of `gated` reruns: the last gate (`last`, [`LAST`]'s text), when it gated
/// the same and failed (`last_rc`, its `.rc` line: `rc=` not 0) and isn't `sub` itself.
pub fn rerun_of<'a>(last: &'a str, last_rc: Option<&str>, gated: &str, sub: &str) -> Option<&'a str> {
    let (of, what) = last.trim().split_once(' ')?;
    let rc: i64 = tokens(last_rc?).next()?.strip_prefix("rc=")?.parse().ok()?;
    (rc != 0 && what == gated && of != sub && !of.is_empty()).then_some(of)
}

/// A gate's start note: the coordinator's, as the rerun of `of` when there is one (`rerun of <of>:
/// <note>`), unless the note says what it reruns already.
pub fn note(note: &str, of: Option<&str>) -> String {
    match of {
        Some(of) if !note.trim_start().starts_with("rerun of ") => format!("rerun of {of}: {}", note.trim()),
        _ => note.to_owned(),
    }
}

/// What the tree at `root` gates: the branches `merge` names, at the commits `origin` has, else
/// the worktree's stack record, else HEAD (`head`, the commit it tests). A ref or record that
/// can't be read is HEAD too: a merge commit or a restack is new, so nothing is called a rerun.
pub fn gated(project: &crate::Project, root: &Path, merge: &[String], head: &str) -> String {
    if !merge.is_empty() {
        let tips: Result<Vec<(String, String)>, String> = merge.iter().map(|b| Ok((b.clone(), stack::rev(root, &format!("origin/{b}"))?))).collect();
        return match (stack::rev(root, BASE), tips) {
            (Ok(base), Ok(tips)) => of_merge(&base, &tips),
            _ => of_head(head),
        };
    }
    match stack::load_any(project, root) {
        Ok(Some(record)) => of_stack(&record).unwrap_or_else(|| of_head(head)),
        _ => of_head(head),
    }
}

/// The gate a gate `sub` of `gated` reruns, from the records in `dir` (`gate-logs`): [`LAST`] and
/// that gate's `.rc`.
pub fn rerun_in(dir: &Path, gated: &str, sub: &str) -> Option<String> {
    let last = std::fs::read_to_string(dir.join(LAST)).ok()?;
    let of = last.trim().split_once(' ')?.0;
    if crate::event::check_id(of).is_err() {
        return None;
    }
    let rc = std::fs::read_to_string(dir.join(format!("{of}.rc"))).ok();
    rerun_of(&last, rc.as_deref(), gated, sub).map(str::to_owned)
}

/// Record gate `sub`, of `gated`, as the last in `dir`: a failure is a warning, and leaves no
/// record of an earlier gate to be taken for this one's.
pub fn record(dir: &Path, sub: &str, gated: &str) {
    let path = dir.join(LAST);
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, format!("{sub} {gated}\n"))) {
        let _ = std::fs::remove_file(&path);
        eprintln!("warning: gate: {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::BOTH;
    use crate::stack::Entry;

    fn entry(mr: u64, old_tip: &str) -> Entry {
        Entry { mr, source_branch: format!("{mr}-x"), old_tip: old_tip.into(), new_tip: format!("{old_tip}-rebased"), onto: String::new(), ..Entry::default() }
    }

    fn stack(main: &str, prs: &[(u64, &str)]) -> Stack {
        let mut entries: Vec<Entry> = prs.iter().map(|&(mr, tip)| entry(mr, tip)).collect();
        if let Some(first) = entries.first_mut() {
            first.onto = main.to_owned();
        }
        Stack { remote: "origin".into(), base: "main".into(), entries }
    }

    const FAILED: &str = "rc=101 300s cause=test kind=smoke (reaches model) head=abc";

    #[test]
    fn a_stack_is_its_prs_at_their_heads_onto_main() {
        let s = stack("m1", &[(9, "a1"), (10, "b1")]);
        assert_eq!(of_stack(&s).as_deref(), Some("origin/main@m1 #9@a1 #10@b1"));
        // A restack makes new rebased tips: the same stack.
        let mut restacked = s.clone();
        restacked.entries.iter_mut().for_each(|e| e.new_tip.push('2'));
        assert_eq!(of_stack(&restacked), of_stack(&s));
        assert_eq!(of_stack(&stack("", &[(9, "a1")])), None, "a record from before `onto`");
        assert_eq!(of_stack(&stack("m1", &[])), None);
        assert_eq!(of_merge("m1", &[("9-x".into(), "a1".into())]), "origin/main@m1 9-x@a1");
        assert_eq!(of_head("abc"), "HEAD@abc");
    }

    /// The decision: the last gate failed on the same stack, and this gate is another.
    #[test]
    fn a_silent_rerun_of_a_failed_stack_is_its_rerun() {
        let same = of_stack(&stack("m1", &[(9, "a1"), (10, "b1")])).unwrap();
        let last = format!("gate-mr9 {same}\n");
        assert_eq!(rerun_of(&last, Some(FAILED), &same, "gate-mr9-2"), Some("gate-mr9"));
        // A transient failure too: the reader doesn't count it, and the note is still true.
        assert_eq!(rerun_of(&last, Some("rc=1 9s cause=memory kind=smoke head=abc"), &same, "gate-mr9-2"), Some("gate-mr9"));
        // A failed rerun is rerun again: a chain the reader follows.
        let rerun = format!("gate-mr9-rerun {same}");
        assert_eq!(rerun_of(&rerun, Some(FAILED), &same, "gate-mr9-3"), Some("gate-mr9-rerun"));

        assert_eq!(note("local gate: #9 #10 (#7 #8)", Some("gate-mr9")), "rerun of gate-mr9: local gate: #9 #10 (#7 #8)");
        assert_eq!(note("rerun of gate-mr9: by hand", Some("gate-mr9")), "rerun of gate-mr9: by hand", "it says so already");
        assert_eq!(note("local gate: #9", None), "local gate: #9");
    }

    /// A stack that changed (a new head, another PR, another `main`), a last gate that passed or
    /// has no result, a gate of something else since, and the same `sub` again: no rerun.
    #[test]
    fn a_changed_stack_or_a_last_gate_that_did_not_fail_is_no_rerun() {
        let same = of_stack(&stack("m1", &[(9, "a1"), (10, "b1")])).unwrap();
        let last = format!("gate-mr9 {same}");
        for changed in [
            stack("m1", &[(9, "a2"), (10, "b1")]),
            stack("m1", &[(9, "a1")]),
            stack("m1", &[(9, "a1"), (10, "b1"), (11, "c1")]),
            stack("m1", &[(10, "b1"), (9, "a1")]),
            stack("m2", &[(9, "a1"), (10, "b1")]),
        ] {
            let now = of_stack(&changed).unwrap();
            assert_eq!(rerun_of(&last, Some(FAILED), &now, "gate-mr9-2"), None, "{now}");
        }
        for rc in [Some("rc=0 300s kind=smoke head=abc"), None, Some(""), Some("garbled")] {
            assert_eq!(rerun_of(&last, rc, &same, "gate-mr9-2"), None, "{rc:?}");
        }
        let superseded = format!("gate-mr11 {}", of_stack(&stack("m1", &[(11, "c1")])).unwrap());
        assert_eq!(rerun_of(&superseded, Some(FAILED), &same, "gate-mr9-2"), None, "a gate of another stack came between");
        assert_eq!(rerun_of(&last, Some(FAILED), &same, "gate-mr9"), None, "the same sub");
        assert_eq!(rerun_of("", Some(FAILED), &same, "gate-mr9-2"), None);
        assert_eq!(rerun_of(&last, Some(FAILED), &of_head("abc"), "gate-mr9-2"), None, "a tree with no record isn't the stack");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        crate::process::git(dir, args, None, &[]).unwrap()
    }

    /// Through git and the gate's records, for both fixture projects: the worktree's stack record
    /// is what is gated, and a second gate of it after the first failed is its rerun; a new head
    /// isn't, nor a gate after one that passed, nor one after a gate of another stack.
    #[test]
    fn the_gate_reads_its_stack_and_its_last_record() {
        for project in BOTH {
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-gate-silent-{}", project.name));
            let (clone, logs) = (dir.join("clone"), dir.join("target").join(super::super::LOG_DIR));
            git(&dir, &["init", "--quiet", clone.to_str().unwrap()]);
            git(&clone, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "--quiet", "--allow-empty", "-m", "first"]);
            let head = git(&clone, &["rev-parse", "HEAD"]).trim().to_owned();
            let name = project.name;
            assert_eq!(gated(&project, &clone, &[], &head), of_head(&head), "{name}: no record: HEAD");
            assert_eq!(rerun_in(&logs, &of_head(&head), "gate-1"), None, "{name}: no last gate");
            assert_eq!(gated(&project, &clone, &["9-x".into()], &head), of_head(&head), "{name}: --merge of a branch origin hasn't");
            git(&clone, &["update-ref", "refs/remotes/origin/main", &head]);
            git(&clone, &["update-ref", "refs/remotes/origin/9-x", &head]);
            let merged = of_merge(&head, &[("9-x".into(), head.clone())]);
            assert_eq!(gated(&project, &clone, &["9-x".into()], "a-merge-commit"), merged, "{name}: --merge");

            stack::save(&project, &clone, &stack("m1", &[(9, "a1"), (10, "b1")])).unwrap();
            let first = gated(&project, &clone, &[], &head);
            assert_eq!(first, "origin/main@m1 #9@a1 #10@b1", "{name}");
            record(&logs, "gate-1", &first);
            std::fs::write(logs.join("gate-1.rc"), FAILED).unwrap();
            assert_eq!(rerun_in(&logs, &first, "gate-2").as_deref(), Some("gate-1"), "{name}: the same stack after a failure");

            stack::save(&project, &clone, &stack("m1", &[(9, "a2"), (10, "b1")])).unwrap();
            let pushed = gated(&project, &clone, &[], &head);
            assert_eq!(rerun_in(&logs, &pushed, "gate-2"), None, "{name}: #9 has a new head");

            std::fs::write(logs.join("gate-1.rc"), "rc=0 300s kind=smoke head=abc").unwrap();
            assert_eq!(rerun_in(&logs, &first, "gate-2"), None, "{name}: the last gate passed");
            std::fs::write(logs.join("gate-1.rc"), FAILED).unwrap();
            record(&logs, "gate-2", &pushed);
            std::fs::write(logs.join("gate-2.rc"), FAILED).unwrap();
            assert_eq!(rerun_in(&logs, &first, "gate-3"), None, "{name}: superseded by a gate of another stack");
            std::fs::write(logs.join(LAST), "../x origin/main@m1 #9@a1 #10@b1").unwrap();
            assert_eq!(rerun_in(&logs, &first, "gate-3"), None, "{name}: a last gate that names no gate");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
