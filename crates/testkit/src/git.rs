//! Real git repositories for tests: commits use a fixed identity and no signing, so they work on
//! any machine, and no command starts git's auto-maintenance.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Run git in `dir`; panics with git's message on failure (tests only).
pub fn git(dir: &Path, args: &[&str]) -> String {
    git_env(dir, args, &[])
}

/// [`git`], with author and committer dates set to `date` (`2026-10-17T10:00:00Z`) when given.
pub fn git_at(dir: &Path, args: &[&str], date: Option<&str>) -> String {
    match date {
        Some(date) => git_env(dir, args, &[("GIT_AUTHOR_DATE", date), ("GIT_COMMITTER_DATE", date)]),
        None => git_env(dir, args, &[]),
    }
}

/// [`git`] with extra environment variables, such as `GIT_COMMITTER_DATE`.
#[allow(clippy::panic)]
pub fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=swarm-test", "-c", "user.email=swarm-test@example.invalid", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(env.iter().copied())
        .output()
        .unwrap_or_else(|e| panic!("cannot run git: {e}"));
    if !out.status.success() {
        panic!("git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A bare `origin.git` under `root` and a clone of it, `work`, with one commit on `main` pushed:
/// the shape every command that fetches and pushes runs against.
pub struct Remote {
    pub origin: PathBuf,
    pub work: PathBuf,
}

/// Make a [`Remote`] under `root` (a [`crate::temp_dir`]).
pub fn remote(root: &Path) -> Remote {
    let origin = root.join("origin.git");
    let work = root.join("work");
    let _ = std::fs::create_dir_all(&origin);
    git(&origin, &["init", "--bare", "--quiet", "--initial-branch=main"]);
    git(root, &["clone", "--quiet", &origin.to_string_lossy(), &work.to_string_lossy()]);
    git(&work, &["commit", "--quiet", "--allow-empty", "-m", "first"]);
    git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    Remote { origin, work }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_remote_has_main_pushed() {
        let r = super::remote(&crate::temp_dir("testkit-remote"));
        assert_eq!(super::git(&r.origin, &["rev-parse", "main"]), super::git(&r.work, &["rev-parse", "HEAD"]));
    }
}
