//! `cargo xtask tidy`: closed issues lose the claim
//! label.
//!
//! `cargo xtask claim` adds the project's in-progress label (`status::in-progress`); a merge
//! closes the issue through `Closes #n`, and GitHub leaves its labels as they are. So every issue
//! that merged kept the label: on 2026-10-20 more than 100 closed issues in each swarm's repo
//! carried it, and a reopened issue looked claimed.
//!
//! - **`tidy <n>`:** the coordinator's merge step (`cargo xtask land` runs it), once the merge is
//!   confirmed. GitHub closes the issue a moment after the merge, so it waits for that (up to
//!   [`WAIT`]); an issue still open then keeps its label, and the command says so without failing
//!   the merge.
//! - **`tidy`:** every closed issue that still carries the label loses it, whether it merged, was
//!   a duplicate or was abandoned (the one-off clean-up, and a check to run any time).
//! - **`--dry-run`** lists, changes nothing. **`--repo <owner/name>`** points it at another
//!   repository (`example/app-swarm`), default the consumer's own.
//!
//! The server's failures of a `gh api` call are tried again ([`crate::github`]). The decisions
//! are pure functions over GitHub's answers (tested); the calls are thin.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::{Project, github};

/// How long `tidy <n>` waits for GitHub to close an issue after its merge.
const WAIT: Duration = Duration::from_secs(30);
/// How often it asks while waiting.
const POLL: Duration = Duration::from_secs(5);
/// The most pages (100 issues each) one `tidy` reads: 10,000 closed issues.
const MAX_PAGES: u32 = 100;

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask tidy [<issue>] [--dry-run] [--repo <owner/name>]";

/// The whole usage, for `project`.
fn usage(project: &Project) -> String {
    format!(
        "usage: {USAGE}
  <issue>     just this issue, once it is closed (the coordinator's merge step; it waits up to
              30 s for GitHub to close the issue after the merge)
  (no issue)  every closed issue that still carries {}
  --dry-run   list them, change nothing
  --repo      another GitHub repository (owner/name); default {}",
        project.labels.in_progress, project.github.repo
    )
}

#[derive(Debug, Default, PartialEq)]
struct Opts {
    issue: Option<u64>,
    dry_run: bool,
    repo: Option<String>,
}

fn parse_args(project: &Project, args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        match a {
            "--dry-run" => o.dry_run = true,
            "--repo" => {
                let r = it.next().ok_or_else(|| format!("tidy: --repo needs a value\n\n{}", usage(project)))?;
                o.repo = Some(github::check_repo(r).map_err(|e| format!("tidy: {e}"))?.to_owned());
            }
            "-h" | "--help" => return Err(usage(project)),
            n if o.issue.is_none() && !n.starts_with('-') => {
                o.issue = Some(n.trim_start_matches('#').parse().map_err(|_| format!("tidy: `{n}` is not an issue number\n\n{}", usage(project)))?);
            }
            other => return Err(format!("tidy: unknown argument `{other}`\n\n{}", usage(project))),
        }
    }
    Ok(o)
}

/// An issue, as far as tidying goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub number: u64,
    pub state: String,
    /// Whether it carries the project's in-progress label.
    pub labelled: bool,
    pub title: String,
}

/// One issue in [`github`]'s shape; none without a number.
pub fn parse_issue(project: &Project, v: &Value) -> Option<Issue> {
    let label = project.labels.in_progress;
    Some(Issue {
        number: v.get("number")?.as_u64()?,
        state: v.get("state").and_then(Value::as_str).unwrap_or_default().to_owned(),
        labelled: v.get("labels").and_then(Value::as_array).is_some_and(|ls| ls.iter().any(|l| l.as_str() == Some(label))),
        title: v.get("title").and_then(Value::as_str).unwrap_or_default().to_owned(),
    })
}

/// The issues to tidy among `found`: closed and still labelled. The query asks for just those;
/// this checks again, so a wrong answer never unlabels an open issue.
pub fn to_tidy(project: &Project, found: &[Value]) -> Vec<Issue> {
    found.iter().filter_map(|v| parse_issue(project, v)).filter(|i| i.state == "closed" && i.labelled).collect()
}

/// What `tidy <n>` does with an issue as it is now.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Closed and labelled: remove the label.
    Unlabel,
    /// Nothing to do: closed, or open, without the label.
    Done,
    /// Open with the label: wait for GitHub to close it.
    Wait,
}

pub fn step(i: &Issue) -> Step {
    match (i.state.as_str(), i.labelled) {
        (_, false) => Step::Done,
        ("closed", true) => Step::Unlabel,
        _ => Step::Wait,
    }
}

/// `cargo xtask tidy …` ([`USAGE`]) for `project` (`root` is unused: it reads only GitHub).
pub fn run(project: &Project, root: &Path, args: &[&str]) -> Result<(), String> {
    let _ = root;
    let o = parse_args(project, args)?;
    let of = o.repo.as_deref();
    match o.issue {
        Some(n) => one(project, of, n, o.dry_run),
        None => all(project, of, o.dry_run),
    }
}

fn one(project: &Project, of: Option<&str>, n: u64, dry_run: bool) -> Result<(), String> {
    let label = project.labels.in_progress;
    let mut waited = Duration::ZERO;
    loop {
        let v = github::issue(project, of, n)?;
        let issue = parse_issue(project, &v).ok_or_else(|| format!("tidy: GitHub's answer for #{n} has no number"))?;
        match step(&issue) {
            Step::Done => {
                println!("tidy: #{n} carries no {label}");
                return Ok(());
            }
            Step::Unlabel if dry_run => {
                println!("tidy: #{n} is closed and would lose {label} (--dry-run)");
                return Ok(());
            }
            Step::Unlabel => {
                github::remove_label(project, of, n, label)?;
                println!("tidy: #{n} is closed: removed {label}");
                return Ok(());
            }
            Step::Wait if waited >= WAIT || dry_run => {
                println!("tidy: #{n} is still {}: it keeps {label} (did its pull request close it?)", issue.state);
                return Ok(());
            }
            Step::Wait => {
                std::thread::sleep(POLL);
                waited += POLL;
            }
        }
    }
}

/// Every closed issue of `of` that carries the label, read whole before any changes: an
/// unlabelling changes the answer under a later page.
fn closed_and_labelled(project: &Project, of: Option<&str>) -> Result<Vec<Issue>, String> {
    let mut found = to_tidy(project, &github::labelled(project, of, project.labels.in_progress, "closed", MAX_PAGES)?);
    found.sort_by_key(|i| i.number);
    found.dedup_by_key(|i| i.number);
    Ok(found)
}

fn all(project: &Project, of: Option<&str>, dry_run: bool) -> Result<(), String> {
    let label = project.labels.in_progress;
    let found = closed_and_labelled(project, of)?;
    if found.is_empty() {
        println!("tidy: no closed issue carries {label}");
        return Ok(());
    }
    let mut failed = 0usize;
    for i in &found {
        if dry_run {
            println!("  #{} {}", i.number, i.title);
        } else if let Err(e) = github::remove_label(project, of, i.number, label) {
            eprintln!("warning: tidy: #{}: {e}", i.number);
            failed += 1;
        }
    }
    let done = found.len().saturating_sub(failed);
    if dry_run {
        println!("tidy: {} closed issues carry {label} (--dry-run: left as they are)", found.len());
    } else {
        println!("tidy: removed {label} from {done} closed issues");
    }
    if failed > 0 { Err(format!("tidy: {failed} of {} issues kept the label", found.len())) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::{APP_SWARM, BOTH, GPU_SWARM};
    use serde_json::json;

    #[test]
    fn only_closed_labelled_issues_are_tidied() {
        let found = [
            json!({"number": 120, "state": "closed", "labels": ["area::ci", "status::in-progress"], "title": "merged"}),
            json!({"number": 121, "state": "open", "labels": ["status::in-progress"], "title": "still worked"}),
            json!({"number": 122, "state": "closed", "labels": ["type::task"], "title": "already tidy"}),
            json!({"state": "closed", "labels": ["status::in-progress"], "title": "no number"}),
            json!({"number": -3, "state": "closed", "labels": ["status::in-progress"]}),
            json!({"number": 123, "state": "closed", "labels": "status::in-progress"}),
            json!("junk"),
        ];
        for p in &BOTH {
            let got: Vec<u64> = to_tidy(p, &found).iter().map(|i| i.number).collect();
            assert_eq!(got, vec![120], "{}", p.name);
            assert!(to_tidy(p, &[json!({"message": "Not Found"})]).is_empty());
        }
    }

    #[test]
    fn one_issue_waits_for_its_close() {
        let issue = |state: &str, labelled| Issue { number: 1, state: state.into(), labelled, title: String::new() };
        assert_eq!(step(&issue("closed", true)), Step::Unlabel);
        assert_eq!(step(&issue("open", true)), Step::Wait);
        assert_eq!(step(&issue("closed", false)), Step::Done);
        assert_eq!(step(&issue("open", false)), Step::Done);
    }

    #[test]
    fn arguments() {
        for p in &BOTH {
            assert_eq!(parse_args(p, &[]).unwrap(), Opts::default());
            assert_eq!(parse_args(p, &["#137", "--dry-run"]).unwrap(), Opts { issue: Some(137), dry_run: true, repo: None });
            assert_eq!(parse_args(p, &["352"]).unwrap().issue, Some(352));
            assert_eq!(parse_args(p, &["--repo", "example/gpu-swarm"]).unwrap().repo.as_deref(), Some("example/gpu-swarm"));
            assert!(parse_args(p, &["1", "2"]).is_err());
            assert!(parse_args(p, &["--repo"]).is_err());
            assert!(parse_args(p, &["--force"]).is_err());
            assert!(parse_args(p, &["abc"]).is_err());
            for bad in ["a?b", "../x", "a/b/c", "x"] {
                assert!(parse_args(p, &["--repo", bad]).is_err(), "{bad}");
            }
            assert!(run(p, Path::new("."), &["--bogus"]).unwrap_err().contains("unknown argument"));
        }
    }

    #[test]
    fn the_usage_names_each_projects_label_and_default() {
        let si = parse_args(&APP_SWARM, &["--help"]).unwrap_err();
        assert!(si.starts_with("usage: cargo xtask tidy [<issue>] [--dry-run] [--repo <owner/name>]"), "{si}");
        assert!(si.contains("still carries status::in-progress") && si.ends_with("default example/app-swarm"), "{si}");
        assert!(parse_args(&GPU_SWARM, &["-h"]).unwrap_err().ends_with("default example/gpu-swarm"));
    }
}
