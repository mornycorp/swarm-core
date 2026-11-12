//! `prwatch --ready` puts a PR that is ready but holds no work back to draft
//! (mornycorp/swarm-core#42).
//!
//! In a swarm on 2026-11-05/06 one worker's requests came up ready four times with only their
//! claim commit: marked ready by hand (`gh pr ready`, or the button), not by `ready`, which
//! refuses a branch with no work. Each time the coordinator sent it back to draft by hand.
//!
//! So when `--ready` finds such a PR ([`crate::empty`], the test `ready`, `stack` and `land` use)
//! it reads the PR again and, only when it is still open, still ready, still at the head that was
//! checked and not from a fork, converts it to a draft, reads it back (what GitHub says, not what
//! the call answered), and leaves one comment saying why and what to do ([`note`]).
//!
//! **Once per head:** each head it re-drafts is kept (`#<n> <sha>`, in the clone's git directory,
//! [`Redrafts`]), and a head it re-drafted once is not re-drafted again: a PR marked ready again
//! at the same head, after the comment, is a worker that disagrees, and a watch that re-drafts it
//! every poll fights it. `--ready` reports it as `ready but empty` as before, saying it
//! re-drafted it already; it is the coordinator's. A push is a new head, and re-drafted again
//! when it is still empty: a push is the worker doing something, not marking the same state
//! ready again, and each push can bring one re-draft at most.

use std::path::PathBuf;

use serde_json::Value;

use crate::{Project, github};

/// Most heads the record keeps: the newest.
const MAX_RECORD: usize = 500;

/// The record's file, in the clone's common git directory.
pub const RECORD: &str = "prwatch-redrafted";

/// What `--ready` says of a PR it re-drafted at this head already.
pub const AGAIN: &str = "re-drafted once at this head already: not again";

/// What re-drafting asks of GitHub: [`Live`], or a test's fake.
pub trait Forge {
    /// The PR as GitHub says it is now, in [`github`]'s shape.
    fn pr(&mut self, n: u64) -> Result<Value, String>;
    /// Convert the PR `node_id` to a draft.
    fn to_draft(&mut self, node_id: &str) -> Result<(), String>;
    /// Mark PR `n` ready for review.
    fn mark_ready(&mut self, n: u64) -> Result<(), String>;
    /// Comment on PR `n`.
    fn comment(&mut self, n: u64, body: &str) -> Result<(), String>;
}

/// GitHub, through [`github`].
pub struct Live<'a>(pub &'a Project);

impl Forge for Live<'_> {
    fn pr(&mut self, n: u64) -> Result<Value, String> {
        github::pr(self.0, n)
    }
    fn to_draft(&mut self, node_id: &str) -> Result<(), String> {
        github::convert_to_draft(self.0, node_id)
    }
    fn mark_ready(&mut self, n: u64) -> Result<(), String> {
        github::mark_ready(self.0, n)
    }
    fn comment(&mut self, n: u64, body: &str) -> Result<(), String> {
        github::comment(self.0, n, body)
    }
}

/// The PR heads `--ready` has re-drafted, `#<n> <sha>` each, so it re-drafts each at most once.
#[derive(Debug, Default)]
pub struct Redrafts {
    /// Where they are kept (in the clone's git directory, shared by its worktrees); `None`: in
    /// memory only.
    record: Option<PathBuf>,
    done: Vec<String>,
}

impl Redrafts {
    /// The heads kept at `record` (none when it can't be read).
    pub fn load(record: PathBuf) -> Self {
        let text = std::fs::read_to_string(&record).unwrap_or_default();
        let done = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
        Redrafts { record: Some(record), done }
    }

    fn has(&self, key: &str) -> bool {
        self.done.iter().any(|k| k == key)
    }

    /// Keep `key`, and write the record (the newest [`MAX_RECORD`]).
    fn add(&mut self, key: String) -> Result<(), String> {
        self.done.push(key);
        let skip = self.done.len().saturating_sub(MAX_RECORD);
        self.done.drain(..skip);
        let Some(path) = &self.record else { return Ok(()) };
        let text: String = self.done.iter().map(|k| format!("{k}\n")).collect();
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// What became of a ready PR that holds no work.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Put back to draft and commented; what to add to its line (a comment that failed, say),
    /// else empty.
    Redrafted(String),
    /// Left as it was, and why.
    Left(String),
}

/// The PR `v`'s head commit, when it is one (a full hex id).
fn head(v: &Value) -> Option<&str> {
    v.get("head_sha").and_then(Value::as_str).filter(|s| (40..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether the PR `v` comes from a fork, or GitHub doesn't say.
fn from_fork(v: &Value) -> bool {
    v.get("cross_repo").and_then(Value::as_bool) != Some(false)
}

/// The comment left on PR `n`, re-drafted at `sha` because it holds no work (`why`).
pub fn note(n: u64, sha: &str, why: &str) -> String {
    let short: String = sha.chars().take(10).collect();
    let why = crate::gate::one_line(why, 300);
    format!(
        "Put back to draft by the coordinator's watch (`cargo xtask prwatch --ready`): it was marked ready at `{short}`, and it holds no work ({why}). \
         `gh pr ready` and the Ready for review button mark a PR ready whatever its branch holds: keep it a draft while you work. \
         Finish the work, then run `cargo xtask ready {n} --agent <your instance id>`: that marks it ready. \
         The watch does this once a head: marked ready again at this head, it is left to the coordinator."
    )
}

/// Why the PR as GitHub says it is now (`now`) is not to be re-drafted at `sha`, if it isn't.
fn changed(now: &Value, sha: &str) -> Option<String> {
    let state = now.get("state").and_then(Value::as_str).unwrap_or("gone");
    if state != "open" {
        return Some(format!("it is {state} now: left alone"));
    }
    if now.get("draft").and_then(Value::as_bool) == Some(true) {
        return Some("a draft again already".to_owned());
    }
    if from_fork(now) {
        return Some("from a fork: not re-drafted".to_owned());
    }
    (head(now) != Some(sha)).then(|| "pushed since it was checked: checked again next poll".to_owned())
}

/// Put the open PR `listed` (a row of the open PRs), ready but holding no work (`why`), back to
/// draft with a comment, at most once per head (`redrafts`).
pub fn redraft(redrafts: &mut Redrafts, listed: &Value, why: &str, forge: &mut dyn Forge) -> Outcome {
    let (Some(n), Some(sha)) = (listed.get("number").and_then(Value::as_u64), head(listed)) else {
        return Outcome::Left("no PR number or head commit to re-draft it by".to_owned());
    };
    if from_fork(listed) {
        return Outcome::Left("from a fork: not re-drafted".to_owned());
    }
    let key = format!("#{n} {sha}");
    if redrafts.has(&key) {
        return Outcome::Left(AGAIN.to_owned());
    }
    put_back(redrafts, key, (n, sha, why), forge).unwrap_or_else(|e| Outcome::Left(format!("couldn't re-draft it: {e}")))
}

/// Read PR `n` again, re-draft it, read it back and comment.
fn put_back(redrafts: &mut Redrafts, key: String, (n, sha, why): (u64, &str, &str), forge: &mut dyn Forge) -> Result<Outcome, String> {
    let now = forge.pr(n)?;
    if let Some(why) = changed(&now, sha) {
        return Ok(Outcome::Left(why));
    }
    let node_id = now.get("node_id").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or("GitHub gave no id to convert it by")?;
    forge.to_draft(node_id)?;
    // Kept as soon as it is a draft: whatever follows, this head is not re-drafted again.
    if let Err(e) = redrafts.add(key) {
        eprintln!("prwatch: couldn't keep the record of re-drafted PRs: {e}");
    }
    // Read back: what GitHub says now, not what the call answered.
    let back = forge.pr(n)?;
    if head(&back) != Some(sha) {
        // A push while it was re-drafted: the worker's, with work, perhaps. As it was.
        forge.mark_ready(n)?;
        return Ok(Outcome::Left("pushed while it was re-drafted: marked ready again".to_owned()));
    }
    if back.get("draft").and_then(Value::as_bool) != Some(true) {
        return Err("GitHub still reads it as ready".to_owned());
    }
    Ok(match forge.comment(n, &note(n, sha, why)) {
        Ok(()) => Outcome::Redrafted(String::new()),
        Err(e) => Outcome::Redrafted(format!(" (its comment failed: {e})")),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A fake GitHub holding one PR: `pr` answers it as it is, `to_draft` and `mark_ready` change
    /// its draft flag, `comment` keeps the comment; and every call is recorded. `push_on_draft`
    /// moves its head on the first `to_draft`, as a worker's push would.
    pub(crate) struct FakeGitHub {
        pub pr: Value,
        pub calls: Vec<String>,
        pub comments: Vec<String>,
        pub push_on_draft: bool,
        pub refuse_draft: bool,
    }

    impl FakeGitHub {
        pub fn new(pr: Value) -> Self {
            FakeGitHub { pr, calls: Vec::new(), comments: Vec::new(), push_on_draft: false, refuse_draft: false }
        }
    }

    impl Forge for FakeGitHub {
        fn pr(&mut self, n: u64) -> Result<Value, String> {
            self.calls.push(format!("pr {n}"));
            Ok(self.pr.clone())
        }
        fn to_draft(&mut self, node_id: &str) -> Result<(), String> {
            self.calls.push(format!("draft {node_id}"));
            if !self.refuse_draft {
                self.pr["draft"] = json!(true);
            }
            if std::mem::take(&mut self.push_on_draft) {
                self.pr["head_sha"] = json!("f".repeat(40));
            }
            Ok(())
        }
        fn mark_ready(&mut self, n: u64) -> Result<(), String> {
            self.calls.push(format!("ready {n}"));
            self.pr["draft"] = json!(false);
            Ok(())
        }
        fn comment(&mut self, n: u64, body: &str) -> Result<(), String> {
            self.calls.push(format!("comment {n}"));
            self.comments.push(body.to_owned());
            Ok(())
        }
    }

    /// PR 9, open and ready at [`SHA`], from this repository.
    pub(crate) fn ready_pr() -> Value {
        json!({"number": 9, "state": "open", "draft": false, "head_sha": SHA, "node_id": "PR_x", "cross_repo": false})
    }

    #[test]
    fn a_ready_pr_with_no_work_is_put_back_to_draft_read_back_and_commented() {
        let mut github = FakeGitHub::new(ready_pr());
        let mut redrafts = Redrafts::default();
        let listed = ready_pr();
        assert_eq!(redraft(&mut redrafts, &listed, "only its claim commit", &mut github), Outcome::Redrafted(String::new()));
        assert_eq!(github.calls, ["pr 9", "draft PR_x", "pr 9", "comment 9"]);
        assert!(github.pr["draft"] == true);
        let comment = &github.comments[0];
        assert!(comment.contains("`0123456789`") && comment.contains("only its claim commit") && comment.contains("cargo xtask ready 9"), "{comment}");
        // Marked ready again at the same head: not again, and nothing asked of GitHub.
        github.pr["draft"] = json!(false);
        github.calls.clear();
        assert_eq!(redraft(&mut redrafts, &listed, "only its claim commit", &mut github), Outcome::Left(AGAIN.to_owned()));
        assert!(github.calls.is_empty());
        // A push is a new head: once more.
        let pushed = "a".repeat(40);
        github.pr["head_sha"] = json!(pushed);
        let listed = github.pr.clone();
        assert_eq!(redraft(&mut redrafts, &listed, "an empty diff", &mut github), Outcome::Redrafted(String::new()));
    }

    #[test]
    fn a_pr_that_changed_since_the_list_is_left_alone() {
        let left = |now: Value| {
            let mut github = FakeGitHub::new(now);
            let out = redraft(&mut Redrafts::default(), &ready_pr(), "empty", &mut github);
            assert_eq!(github.calls, ["pr 9"], "read, and nothing changed");
            out
        };
        let with = |key: &str, value: Value| {
            let mut pr = ready_pr();
            pr[key] = value;
            pr
        };
        assert_eq!(left(with("state", json!("merged"))), Outcome::Left("it is merged now: left alone".into()));
        assert_eq!(left(with("draft", json!(true))), Outcome::Left("a draft again already".into()));
        assert_eq!(left(with("cross_repo", json!(true))), Outcome::Left("from a fork: not re-drafted".into()));
        assert_eq!(left(with("head_sha", json!("b".repeat(40)))), Outcome::Left("pushed since it was checked: checked again next poll".into()));
        // A row with no head, or from a fork, asks nothing of GitHub.
        let mut github = FakeGitHub::new(ready_pr());
        for row in [with("head_sha", json!("short")), with("cross_repo", json!(true)), with("cross_repo", Value::Null)] {
            assert!(matches!(redraft(&mut Redrafts::default(), &row, "empty", &mut github), Outcome::Left(_)));
        }
        assert!(github.calls.is_empty());
    }

    #[test]
    fn a_push_during_the_redraft_marks_it_ready_again_and_a_refusal_is_said() {
        let mut github = FakeGitHub::new(ready_pr());
        github.push_on_draft = true;
        let mut redrafts = Redrafts::default();
        assert_eq!(redraft(&mut redrafts, &ready_pr(), "empty", &mut github), Outcome::Left("pushed while it was re-drafted: marked ready again".into()));
        assert_eq!(github.calls, ["pr 9", "draft PR_x", "pr 9", "ready 9"]);
        assert!(github.pr["draft"] == false && github.comments.is_empty());
        let mut github = FakeGitHub::new(ready_pr());
        github.refuse_draft = true;
        let out = redraft(&mut Redrafts::default(), &ready_pr(), "empty", &mut github);
        assert_eq!(out, Outcome::Left("couldn't re-draft it: GitHub still reads it as ready".into()));
    }

    #[test]
    fn the_record_is_kept_in_a_file_and_read_back() {
        let dir = swarm_core_testkit::temp_dir("core-prwatch-redrafted");
        let record = dir.join(RECORD);
        let mut github = FakeGitHub::new(ready_pr());
        assert!(matches!(redraft(&mut Redrafts::load(record.clone()), &ready_pr(), "empty", &mut github), Outcome::Redrafted(_)));
        assert_eq!(std::fs::read_to_string(&record).unwrap(), format!("#9 {SHA}\n"));
        github.pr["draft"] = json!(false);
        assert_eq!(redraft(&mut Redrafts::load(record), &ready_pr(), "empty", &mut github), Outcome::Left(AGAIN.to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
