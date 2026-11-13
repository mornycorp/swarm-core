//! PRs readied without their author's test run (mornycorp/swarm-core#18).
//!
//! The coordinator's smoke ([`Kind::Smoke`]) runs no tests: it trusts the `ci --changed` that the
//! author's `cargo xtask ready` ran, and the periodic full gate of `main` covers the rest. `ready
//! --no-ci` skips that run, so its `submit` event says so: its note's first `;`-separated part is
//! [`NO_CI`]. `gate` reads the tracking branch for the PRs it gates (the stack record's, or the
//! issues of the branches `--merge` names) and, when the latest `submit` of one says `no-ci`,
//! runs [`Kind::Changed`] (`ci --changed --base origin/main`) instead of the smoke, its why naming
//! them (`kind=changed (ready --no-ci: #220)`). A later `ready` with its run is a `submit` without
//! the note, and the smoke is enough again. The docs, tooling and full gates run `ci --changed`
//! or more already, and stay.

use std::path::Path;

use serde_json::Value;

use super::Kind;
use crate::Project;
use crate::claim::{issue_of_branch, parse_time, tracking_files};
use crate::stack::{self, Stack};

/// What a `submit` event's note begins with when `ready --no-ci` skipped the author's `ci
/// --changed`.
pub const NO_CI: &str = "no-ci";

/// A PR the gate tests, as it knows it: by number (a stack record's entry), or by its issue (a
/// `--merge` branch, `<issue>-<slug>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gated {
    Pr(u64),
    Issue(u64),
}

/// The PRs a stack record names, bottom first.
pub fn of_stack(record: Option<&Stack>) -> Vec<Gated> {
    record.map(|s| s.entries.iter().map(|e| Gated::Pr(e.mr)).collect()).unwrap_or_default()
}

/// The PRs `--merge` names, by their branches' issues (a branch that names none is left out).
pub fn of_branches(branches: &[String]) -> Vec<Gated> {
    branches.iter().filter_map(|b| issue_of_branch(b)).map(Gated::Issue).collect()
}

/// Whether a `submit`'s note says `ready --no-ci` skipped the author's run.
pub fn skipped_ci(note: &str) -> bool {
    note.split(';').next().is_some_and(|first| first.trim() == NO_CI)
}

/// One `submit` event: when, its PR and issue, and whether it skipped the author's run.
struct Submit {
    t: i64,
    mr: Option<u64>,
    issue: Option<u64>,
    no_ci: bool,
}

/// Every `submit` in the tracking files (instance, `work.jsonl` text); a line that doesn't parse,
/// or has no time, is skipped.
fn submits(files: &[(String, String)]) -> Vec<Submit> {
    let lines = files.iter().flat_map(|(_, text)| text.lines()).filter_map(|l| serde_json::from_str::<Value>(l).ok());
    lines
        .filter(|v| v.get("event").and_then(Value::as_str) == Some("submit"))
        .filter_map(|v| {
            Some(Submit {
                t: parse_time(v.get("t")?.as_str()?)?,
                mr: v.get("mr").and_then(Value::as_u64),
                issue: v.get("issue").and_then(Value::as_u64),
                no_ci: v.get("note").and_then(Value::as_str).is_some_and(skipped_ci),
            })
        })
        .collect()
}

/// The PRs among `gated` whose latest `submit` in `files` skipped the author's run: their numbers,
/// as those submits name them (the issue's, for a submit naming no PR), in `gated`'s order, each
/// once.
pub fn untested(files: &[(String, String)], gated: &[Gated]) -> Vec<u64> {
    let all = submits(files);
    let mut out = Vec::new();
    for g in gated {
        let of = |s: &&Submit| match *g {
            Gated::Pr(n) => s.mr == Some(n),
            Gated::Issue(n) => s.issue == Some(n),
        };
        // The latest; of two at one second, the later read.
        let latest = all.iter().filter(of).reduce(|a, b| if b.t >= a.t { b } else { a });
        if let Some(s) = latest.filter(|s| s.no_ci) {
            let n = s.mr.or(s.issue).unwrap_or_default();
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// The gate's kind once the PRs readied without their run (`untested`) are known: the smoke
/// becomes [`Kind::Changed`], its why naming them; any other kind stays.
pub fn kind_for(planned: (Kind, String), untested: &[u64]) -> (Kind, String) {
    match planned {
        (Kind::Smoke, _) if !untested.is_empty() => {
            let prs: Vec<String> = untested.iter().map(|n| format!("#{n}")).collect();
            (Kind::Changed, format!("ready --no-ci: {}", prs.join(" ")))
        }
        other => other,
    }
}

/// [`kind_for`] the tree at `root`: for a smoke, the PRs it gates (the branches `merge` names, else
/// the worktree's stack record) are looked up on the tracking branch, fetched now. A record or a
/// tracking branch that can't be read is a warning, and the smoke stays.
pub fn kind(project: &Project, root: &Path, merge: &[String], planned: (Kind, String)) -> (Kind, String) {
    if planned.0 != Kind::Smoke {
        return planned;
    }
    let gated = if merge.is_empty() {
        match stack::load_any(project, root) {
            Ok(record) => of_stack(record.as_ref()),
            Err(e) => return warned(planned, &format!("the stack record: {e}")),
        }
    } else {
        of_branches(merge)
    };
    if gated.is_empty() {
        return planned;
    }
    match tracking_files(project, root) {
        Ok(files) => kind_for(planned, &untested(&files, &gated)),
        Err(e) => warned(planned, &format!("the tracking branch: {e}")),
    }
}

/// `planned`, having said that whether a PR was readied without its run couldn't be read (`what`).
fn warned(planned: (Kind, String), what: &str) -> (Kind, String) {
    eprintln!("warning: gate: couldn't tell whether a PR was readied with --no-ci ({what}); the smoke stays");
    planned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::BOTH;
    use crate::stack::Entry;

    fn file(instance: &str, lines: &[&str]) -> (String, String) {
        (instance.to_owned(), lines.join("\n"))
    }

    const NO_CI_220: &str = r#"{"event":"submit","issue":210,"mr":220,"note":"no-ci","t":"2026-11-03T10:00:00Z"}"#;
    const TESTED_221: &str = r#"{"event":"submit","issue":211,"mr":221,"t":"2026-11-03T10:05:00Z"}"#;

    #[test]
    fn a_batch_proofs_full_gate_stays_full() {
        // mornycorp/swarm-core#53: under the batch proof the stack's gate is already the whole
        // `ci`, so a PR readied without its run changes nothing.
        let planned = (Kind::Full, "batch proof; a product crate".to_owned());
        assert_eq!(kind_for(planned.clone(), &[9]), planned);
    }

    #[test]
    fn a_submits_note_says_when_ready_skipped_its_run() {
        assert!(skipped_ci("no-ci"));
        assert!(skipped_ci(" no-ci ; and more"));
        for not in ["", "no-cii", "gate-1 failed only at its test step; rerun alone and passed: no-ci", "ci=no-ci"] {
            assert!(!skipped_ci(not), "{not:?}");
        }
    }

    /// The planner's choice: a smoke whose stack holds a PR readied with --no-ci runs `ci
    /// --changed`, its why naming it; without one, or for any other kind, it stays.
    #[test]
    fn a_smoke_with_a_no_ci_submit_in_its_stack_runs_ci_changed() {
        let files = [file("w-1", &[NO_CI_220]), file("w-2", &[TESTED_221, "not json"])];
        let smoke = (Kind::Smoke, "reaches model".to_owned());
        let untested = untested(&files, &[Gated::Pr(221), Gated::Pr(220)]);
        assert_eq!(untested, [220]);
        let (kind, why) = kind_for(smoke.clone(), &untested);
        assert_eq!((kind, why.as_str()), (Kind::Changed, "ready --no-ci: #220"));
        assert_eq!(super::super::kind_field(kind, &why), "kind=changed (ready --no-ci: #220)");
        assert_eq!(kind.ci_args(), ["--changed", "--base", "origin/main"]);
        assert!(kind.builds() && Kind::Smoke < kind && kind < Kind::Full);
        // No such PR in the stack: the smoke.
        assert_eq!(kind_for(smoke.clone(), &self::untested(&files, &[Gated::Pr(221)])), smoke);
        assert_eq!(kind_for(smoke.clone(), &self::untested(&[], &[Gated::Pr(220)])), smoke, "no tracking: nothing says so");
        // The docs, tooling and full gates run `ci --changed` or more already.
        for k in [Kind::Docs, Kind::Tooling, Kind::Full] {
            assert_eq!(kind_for((k, "x".into()), &[220]), (k, "x".into()));
        }
        // Two of them, in the stack's order.
        let both = [file("w-1", &[NO_CI_220, &TESTED_221.replace(r#""t""#, r#""note":"no-ci; again","t""#)])];
        assert_eq!(kind_for(smoke, &self::untested(&both, &[Gated::Pr(221), Gated::Pr(220)])).1, "ready --no-ci: #221 #220");
    }

    /// Only a PR's latest submit counts: readied again with its run, the smoke is enough; readied
    /// again with --no-ci after a tested submit, it isn't.
    #[test]
    fn only_the_latest_submit_of_a_pr_counts() {
        let tested_later = r#"{"event":"submit","issue":210,"mr":220,"t":"2026-11-03T11:00:00Z"}"#;
        assert!(untested(&[file("w-1", &[NO_CI_220]), file("w-3", &[tested_later])], &[Gated::Pr(220)]).is_empty());
        let no_ci_later = r#"{"event":"submit","issue":210,"mr":220,"note":"no-ci","t":"2026-11-03T12:00:00Z"}"#;
        assert_eq!(untested(&[file("w-1", &[no_ci_later, tested_later])], &[Gated::Pr(220)]), [220], "by time, not by file order");
        // Another PR of the same issue, or a claim's note, is not this PR's submit.
        let claim = r#"{"event":"claim","issue":210,"mr":220,"note":"no-ci","t":"2026-11-03T13:00:00Z"}"#;
        assert!(untested(&[file("w-1", &[claim])], &[Gated::Pr(220)]).is_empty());
        assert!(untested(&[file("w-1", &[NO_CI_220])], &[Gated::Pr(219)]).is_empty());
    }

    /// `--merge` names branches: their issues find the submits.
    #[test]
    fn merged_branches_are_found_by_their_issues() {
        let gated = of_branches(&["210-fix-it".into(), "release-of-frees-claim".into(), "211-other".into()]);
        assert_eq!(gated, [Gated::Issue(210), Gated::Issue(211)]);
        assert_eq!(untested(&[file("w-1", &[NO_CI_220, TESTED_221])], &gated), [220]);
        let entry = |mr: u64| Entry { mr, source_branch: format!("{mr}-x"), ..Entry::default() };
        let record = Stack { remote: "origin".into(), base: "main".into(), entries: vec![entry(220), entry(221)] };
        assert_eq!(of_stack(Some(&record)), [Gated::Pr(220), Gated::Pr(221)]);
        assert!(of_stack(None).is_empty());
    }

    /// Through git, for both fixture projects: the worktree's stack record names #220, whose
    /// submit on the origin's tracking branch says `no-ci`, and the smoke becomes `ci --changed`;
    /// with the PR readied again with its run, it stays the smoke.
    #[test]
    fn the_gate_reads_the_stack_and_the_tracking_branch() {
        for project in BOTH {
            let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-gate-untested-{}", project.name));
            let (remote, clone) = (dir.join("remote.git"), dir.join("clone"));
            crate::process::git(&dir, &["init", "--quiet", "--bare", remote.to_str().unwrap()], None, &[]).unwrap();
            crate::process::git(&dir, &["init", "--quiet", clone.to_str().unwrap()], None, &[]).unwrap();
            crate::process::git(&clone, &["remote", "add", project.branches.remote, remote.to_str().unwrap()], None, &[]).unwrap();
            let smoke = (Kind::Smoke, "reaches model".to_owned());
            assert_eq!(kind(&project, &clone, &[], smoke.clone()), smoke, "{}: no record, no PRs to look up", project.name);
            let entry = Entry { mr: 220, source_branch: "210-x".into(), ..Entry::default() };
            stack::save(&project, &clone, &Stack { remote: "origin".into(), base: "main".into(), entries: vec![entry] }).unwrap();
            assert_eq!(kind(&project, &clone, &[], smoke.clone()), smoke, "{}: no tracking branch yet", project.name);
            let send = |line: &str| crate::event::send(&project, &clone, &dir.join("queue.jsonl"), &[("w-1".to_owned(), line.to_owned())]).unwrap();
            send(NO_CI_220);
            let changed = (Kind::Changed, "ready --no-ci: #220".to_owned());
            assert_eq!(kind(&project, &clone, &[], smoke.clone()), changed, "{}", project.name);
            assert_eq!(kind(&project, &clone, &["210-x".into()], smoke.clone()), changed, "{}: --merge", project.name);
            assert_eq!(kind(&project, &clone, &[], (Kind::Docs, "x".into())).0, Kind::Docs, "{}", project.name);
            send(r#"{"event":"submit","issue":210,"mr":220,"t":"2026-11-03T11:00:00Z"}"#);
            assert_eq!(kind(&project, &clone, &[], smoke.clone()), smoke, "{}: readied again with its run", project.name);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
