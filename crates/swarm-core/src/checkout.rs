//! Which checkout xtask works in, and whether a target directory belongs to it.
//!
//! The compile-time `CARGO_MANIFEST_DIR` names the checkout that built the xtask binary, which is
//! not always the one it runs from: cargo hashes a workspace member's build by its path relative to
//! the workspace, so worktrees sharing a `CARGO_TARGET_DIR` share one xtask binary. The root is
//! found at run time instead: the `CARGO_MANIFEST_DIR` that `cargo run` (the `cargo xtask` alias)
//! sets for the process, else the workspace above the current directory, else the clone an
//! installed copy sits in ([`installed_root`], sc#10), else the compile-time value (the consumer's
//! own xtask's, which it passes in).
//!
//! Sharing the target dir is unsafe beyond that: cargo's freshness check compares file times, so
//! a source file older than another checkout's last build counts as built even when its content
//! differs, and `ci` would test that checkout's code. `ci` therefore records the checkout that uses
//! a target dir in [`Project::checkout_marker`] and refuses one another checkout used last.

use std::path::{Path, PathBuf};

use crate::Project;

/// Where `cargo xtask install-tools` puts its copy of xtask, in the clone's git directory (sc#10).
pub const TOOLS_DIR: &str = "swarm-tools";

/// The workspace root: the parent of `runtime_manifest` (the xtask crate's directory, as `cargo
/// run` sets it) when it is xtask's, else the workspace holding `cwd`, else the main worktree of
/// the clone this executable is installed in, else `compiled`'s parent.
pub fn resolve_root(runtime_manifest: Option<&Path>, cwd: Option<&Path>, compiled: &Path) -> PathBuf {
    if let Some(m) = runtime_manifest
        && m.join("Cargo.toml").is_file()
        && let Some(root) = m.parent().filter(|r| is_workspace(r))
    {
        return root.to_path_buf();
    }
    if let Some(root) = cwd.and_then(|c| c.ancestors().find(|d| is_workspace(d))) {
        return root.to_path_buf();
    }
    if let Some(root) = std::env::current_exe().ok().and_then(|exe| installed_root(&exe)) {
        return root;
    }
    compiled.parent().unwrap_or(compiled).to_path_buf()
}

/// The main worktree of the clone whose git directory holds `exe` in [`TOOLS_DIR`]
/// (`<clone>/.git/swarm-tools/xtask`), or `None` for an executable anywhere else.
pub fn installed_root(exe: &Path) -> Option<PathBuf> {
    let tools = exe.parent().filter(|d| d.file_name().is_some_and(|n| n == TOOLS_DIR))?;
    let git_dir = tools.parent().filter(|d| d.file_name().is_some_and(|n| n == ".git"))?;
    git_dir.parent().filter(|r| is_workspace(r)).map(Path::to_path_buf)
}

/// Is `dir` a workspace root: a `Cargo.toml` with a `[workspace]` table and an `xtask/` crate?
fn is_workspace(dir: &Path) -> bool {
    dir.join("xtask").join("Cargo.toml").is_file()
        && std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]"))
}

/// A path as compared between checkouts: `/` separators, no trailing separator, and on Windows
/// without case.
fn normal(p: &str) -> String {
    let s = p.trim().replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_ascii_lowercase() } else { s.to_owned() }
}

/// Whether `a` and `b` name the same path, however each is spelled ([`normal`]).
pub fn same_path(a: &Path, b: &Path) -> bool {
    normal(&a.to_string_lossy()) == normal(&b.to_string_lossy())
}

/// Whether `root` may use a target dir whose marker holds `marker` (`None`: no marker yet).
pub fn check(project: &Project, marker: Option<&str>, root: &Path, target: &Path) -> Result<(), String> {
    let ours = root.display().to_string();
    match marker.map(str::trim).filter(|m| !m.is_empty()) {
        Some(other) if normal(other) != normal(&ours) => Err(format!(
            "the target dir {} was last used by another checkout, {other}. Cargo would reuse what it built from that \
             checkout's sources wherever this one's files are older, and test the wrong code. \
             Give each worktree its own target dir (CARGO_TARGET_DIR=target/agent-<name>-<worktree>), or \
             delete {} if {other} is gone.",
            target.display(),
            target.join(project.checkout_marker).display()
        )),
        _ => Ok(()),
    }
}

/// Check the target dir's marker against `root` and record `root` in it.
pub fn claim(project: &Project, target: &Path, root: &Path) -> Result<(), String> {
    let marker = target.join(project.checkout_marker);
    let existing = std::fs::read_to_string(&marker).ok();
    check(project, existing.as_deref(), root, target)?;
    std::fs::create_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
    std::fs::write(&marker, format!("{}\n", root.display())).map_err(|e| format!("{}: {e}", marker.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::BOTH;

    /// A fake workspace: `Cargo.toml` with `[workspace]`, `xtask/Cargo.toml`, `crates/a/src`.
    fn workspace(name: &str) -> PathBuf {
        let root = swarm_core_testkit::temp_dir(name);
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"xtask\"]\n").unwrap();
        std::fs::create_dir_all(root.join("xtask")).unwrap();
        std::fs::write(root.join("xtask").join("Cargo.toml"), "[package]\nname = \"xtask\"\n").unwrap();
        std::fs::create_dir_all(root.join("crates").join("a").join("src")).unwrap();
        root
    }

    #[test]
    fn the_root_is_the_checkout_xtask_runs_from() {
        let (here, built) = (workspace("swarm-core-checkout-root-here"), workspace("swarm-core-checkout-root-built"));
        let compiled = built.join("xtask");
        // `cargo xtask` from `here`: cargo run's CARGO_MANIFEST_DIR names here's xtask.
        assert_eq!(resolve_root(Some(&here.join("xtask")), None, &compiled), here);
        // The binary run directly from a crate inside `here`.
        assert_eq!(resolve_root(None, Some(&here.join("crates").join("a").join("src")), &compiled), here);
        // A runtime manifest that is not an xtask crate in a workspace is ignored.
        assert_eq!(resolve_root(Some(&here.join("crates").join("a")), Some(&here), &compiled), here);
        // Nothing at run time: the compile-time checkout.
        let outside = swarm_core_testkit::temp_dir("swarm-core-checkout-root-outside");
        assert_eq!(resolve_root(None, Some(&outside), &compiled), built);
    }

    /// sc#10: an xtask installed in a clone's `.git/swarm-tools/` finds that clone from anywhere.
    #[test]
    fn an_installed_xtask_finds_its_clone() {
        let here = workspace("swarm-core-checkout-installed");
        let exe = here.join(".git").join(TOOLS_DIR).join("xtask.exe");
        assert_eq!(installed_root(&exe), Some(here.clone()));
        assert_eq!(installed_root(&here.join("target").join("debug").join("xtask.exe")), None);
        assert_eq!(installed_root(&here.join(".git").join("other").join("xtask.exe")), None);
        // Not a git directory, or not beside a workspace: nothing.
        assert_eq!(installed_root(&here.join("git").join(TOOLS_DIR).join("xtask")), None);
        let bare = swarm_core_testkit::temp_dir("swarm-core-checkout-installed-bare");
        assert_eq!(installed_root(&bare.join(".git").join(TOOLS_DIR).join("xtask")), None);
    }

    #[test]
    fn a_target_dir_another_checkout_used_is_refused() {
        for p in BOTH {
            let target = swarm_core_testkit::temp_dir(&format!("swarm-core-checkout-target-{}", p.name));
            let a = workspace(&format!("swarm-core-checkout-claim-a-{}", p.name));
            let b = workspace(&format!("swarm-core-checkout-claim-b-{}", p.name));
            claim(&p, &target, &a).unwrap();
            claim(&p, &target, &a).unwrap();
            let e = claim(&p, &target, &b).unwrap_err();
            assert!(e.contains(&a.display().to_string()), "{e}");
            assert!(e.contains(p.checkout_marker), "the marker to delete is named: {e}");
            // Deleting the marker hands the target dir over.
            std::fs::remove_file(target.join(p.checkout_marker)).unwrap();
            claim(&p, &target, &b).unwrap();
            assert_eq!(std::fs::read_to_string(target.join(p.checkout_marker)).unwrap().trim(), b.display().to_string());
        }
    }

    #[test]
    fn each_project_keeps_its_own_marker() {
        let [si, ns] = BOTH;
        assert_eq!((si.checkout_marker, ns.checkout_marker), ("swarm-checkout.txt", "gpu-checkout.txt"));
    }

    #[test]
    fn the_same_checkout_matches_however_it_is_spelled() {
        let t = Path::new("t");
        for p in BOTH {
            for root in ["D:/dev/app-swarm", "D:/dev/gpu-swarm"] {
                let (r, back) = (Path::new(root), root.replace('/', "\\"));
                assert!(check(&p, None, r, t).is_ok());
                assert!(check(&p, Some(""), r, t).is_ok());
                assert!(check(&p, Some(&format!("{root}/\n")), r, t).is_ok());
                assert!(check(&p, Some(&back), r, t).is_ok());
                assert_eq!(check(&p, Some(&root.replacen("D:/dev", "d:/DEV", 1)), r, t).is_ok(), cfg!(windows));
                assert!(check(&p, Some(&format!("{root}-other")), r, t).is_err());
            }
        }
    }
}
