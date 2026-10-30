//! A PR that changes nothing: `ready`, `stack` and `land` refuse it, and
//! `prwatch --ready` reports it as "ready but empty".
//!
//! On 2026-10-27 a PR was un-drafted with only its claim commit (`claim` makes an empty commit
//! "Claim #n: …" so the draft PR has one of its own), and nothing in the coordinator's chain
//! looked at whether it changed anything: the smoke passed, `land` merged it, and the merge closed
//! its issue with none of its work on `main`.
//!
//! A branch has no work when, against where it forked from its base (`git merge-base`): it has no
//! commits, or every commit is a claim commit, or its diff is empty.

use std::path::Path;

use crate::process::git;

/// What every refusal tells the PR's owner to do.
pub const FINISH: &str = "finish the work, or close the PR";

/// How `stack` and `land` refuse a PR that holds no work, `why` from [`why`].
pub fn refusal(why: &str) -> String {
    format!("changes nothing: {why}; {FINISH}")
}

/// Whether `subject` is a claim commit's (`Claim #<n>: <title>`, any issue): `claim`'s commit,
/// with no change of its own.
pub fn is_claim_subject(subject: &str) -> bool {
    let Some(rest) = subject.strip_prefix("Claim #") else { return false };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest.get(digits..).is_some_and(|r| r.starts_with(':'))
}

/// Why a branch whose commits since its base have these `subjects` (one a line) and change these
/// `files` (one a line) holds no work; `None` when it does.
pub fn verdict(subjects: &str, files: &str) -> Option<String> {
    let subjects: Vec<&str> = subjects.lines().map(str::trim).filter(|s| !s.is_empty()).collect();
    if subjects.is_empty() {
        return Some("it has no commits beyond its base".to_owned());
    }
    if subjects.iter().all(|s| is_claim_subject(s)) {
        let n = subjects.len();
        let claims = if n == 1 { "its claim commit".to_owned() } else { format!("{n} claim commits") };
        return Some(format!("it holds only {claims} (`{}`)", subjects.join("`, `")));
    }
    if files.trim().is_empty() {
        return Some("its diff against its base is empty".to_owned());
    }
    None
}

/// Why `head` holds no work against `base` (both commits or refs the clone at `dir` has), from
/// where it forked from it; `None` when it does.
pub fn why(dir: &Path, base: &str, head: &str) -> Result<Option<String>, String> {
    let fork = git(dir, &["merge-base", base, head], None, &[]).map_err(|e| format!("no merge base of {base} and {head}: {e}"))?;
    let fork = fork.trim();
    let subjects = git(dir, &["log", "--format=%s", &format!("{fork}..{head}")], None, &[])?;
    let files = git(dir, &["diff", "--name-only", fork, head], None, &[])?;
    Ok(verdict(&subjects, &files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stack::testgit;

    #[test]
    fn claim_subjects_are_any_issues_claim() {
        assert!(is_claim_subject("Claim #294: Source map: a Layer colour mode"));
        assert!(is_claim_subject("Claim #1:"));
        assert!(!is_claim_subject("Claim #294 Source map"), "no colon");
        assert!(!is_claim_subject("Claim #: x"), "no number");
        assert!(!is_claim_subject("Claim #29x: x"));
        assert!(!is_claim_subject("Source map: Claim #294: x"));
    }

    #[test]
    fn a_branch_holds_work_only_with_a_change_beyond_its_claim() {
        assert_eq!(verdict("", ""), Some("it has no commits beyond its base".into()));
        assert_eq!(verdict("Claim #294: a\n", ""), Some("it holds only its claim commit (`Claim #294: a`)".into()));
        assert_eq!(verdict("Claim #294: a\nClaim #294: a\n", "x.rs\n"), Some("it holds only 2 claim commits (`Claim #294: a`, `Claim #294: a`)".into()));
        assert_eq!(verdict("Revert a fix\na fix\nClaim #294: a\n", ""), Some("its diff against its base is empty".into()));
        assert_eq!(verdict("a fix\nClaim #294: a\n", "x.rs\n"), None);
    }

    #[test]
    fn a_claim_only_branch_is_empty_and_a_real_change_is_not() {
        let dir = swarm_core_testkit::temp_dir("xtask-empty-branches");
        let repo = dir.join("origin.git");
        testgit::bare(&repo);
        testgit::push_commit(&repo, "main", &[], &[("code.rs", "fn a() {}\n")], "first");
        testgit::push_commit(&repo, "295-empty", &["main"], &[], "Claim #295: refuse an empty PR");
        testgit::push_commit(&repo, "296-real", &["main"], &[], "Claim #296: a real change");
        testgit::push_commit(&repo, "296-real", &["296-real"], &[("code.rs", "fn a() { b() }\n")], "a real change");
        // Main moves on after the claim: its later commits are not the branch's.
        testgit::push_commit(&repo, "main", &["main"], &[("more.rs", "fn m() {}\n")], "main moves on");
        let empty = why(&repo, "main", "295-empty").unwrap().unwrap();
        assert!(empty.contains("only its claim commit") && empty.contains("Claim #295"), "{empty}");
        assert_eq!(why(&repo, "main", "296-real").unwrap(), None);
        assert_eq!(why(&repo, "main", "main").unwrap(), Some("it has no commits beyond its base".into()));
        assert!(why(&repo, "main", "no-such-branch").is_err());
    }
}
