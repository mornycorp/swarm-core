//! `cargo xtask swarm-core`: the pin of this
//! crate in a consumer's workspace manifest ([`URL`]), and the assets it ships .
//!
//! - **`--version`** prints the pinned crate's version and the commit `Cargo.lock` holds.
//! - **`bump [--rev <sha>]`** moves the pin: the manifest's `rev` (default: Swarm Core's
//!   `main`, from `git ls-remote`), then `cargo update -p swarm-core`, so the lock moves with
//!   it and nothing else does. A bump isn't everything's change: `ci --changed` takes a manifest
//!   and lock diff that only moves Swarm Core as Swarm Core's, which reaches the packages
//!   that depend on it ([`only_moves_the_pin`], [`crate::changed::SWARM_CORE_PIN`]).
//! - **`sync-assets [--check]`** writes the pinned copies of the shared docs, templates, scripts
//!   and skills ([`crate::assets`]), or checks them.
//! - **`init`** writes the starting points a new consumer owns once copied ([`crate::templates`]):
//!   the ones it doesn't have yet, never overwriting.
//! - **No committed override:** a consumer's `cargo xtask layers` refuses a `path` or `branch` form
//!   of the dependency and a `[patch]` of its source in the workspace's manifest or cargo config
//!   ([`check_committed`]); a user-level cargo config can still patch it while iterating.

use std::path::Path;

use crate::{Hooks, Project};

/// Swarm Core's repository, as a consumer's workspace manifest names it.
pub const URL: &str = "https://github.com/mornycorp/swarm-core.git";
/// Its package name.
pub const PACKAGE: &str = "swarm-core";

pub const USAGE: &str = "cargo xtask swarm-core --version | bump [--rev <sha>] | sync-assets [--check] | init";

/// `cargo xtask swarm-core <args>` for `project`, in the consumer's workspace `root`.
pub fn run(project: &Project, hooks: &dyn Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    match args {
        ["--version"] => {
            let lock = std::fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
            let commit = pinned(&lock).ok_or("Cargo.lock pins no swarm-core: run `cargo xtask swarm-core bump`")?;
            println!("{PACKAGE} {} at {commit}", crate::VERSION);
            Ok(())
        }
        ["bump"] => bump(root, None),
        ["bump", "--rev", rev] => bump(root, Some(rev)),
        ["sync-assets"] => crate::assets::sync(project, hooks, root),
        ["sync-assets", "--check"] => crate::assets::check(project, hooks, root),
        ["init"] => {
            let today = crate::time::now_iso();
            crate::templates::init(project, hooks, root, today.get(..10).unwrap_or(&today)).map(drop)
        }
        _ => Err(format!("usage: {USAGE}")),
    }
}

/// Move the pin to `rev` (default: Swarm Core's `main`), and the lock with it.
fn bump(root: &Path, rev: Option<&str>) -> Result<(), String> {
    let rev = match rev {
        Some(r) => r.to_owned(),
        None => main_of_swarm_core(root)?,
    };
    bump_to(root, &rev, || {
        let mut cargo = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cargo.current_dir(root).args(["update", "-p", PACKAGE]);
        crate::process::output(cargo, None, std::time::Duration::from_secs(300), "cargo update -p swarm-core").map(drop)
    })
}

/// Move the manifest's pin to the whole commit id `rev`, then the lock (`update`). A short id is
/// refused, since cargo fetches a git `rev` it can't resolve as a ref only by its whole id; a
/// failed update puts the manifest back, so a bump that fails changes nothing.
fn bump_to(root: &Path, rev: &str, update: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    if is_sha(rev) && rev.len() < 40 {
        return Err(format!("`{rev}` is short: give the whole commit id (`git rev-parse {rev}`), as cargo fetches a git rev by it"));
    }
    let path = root.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path).map_err(|e| format!("Cargo.toml: {e}"))?;
    let (moved, old) = set_rev(&manifest, rev)?;
    if old == rev {
        println!("{PACKAGE} is pinned at {rev} already");
        return Ok(());
    }
    std::fs::write(&path, moved).map_err(|e| format!("Cargo.toml: {e}"))?;
    if let Err(e) = update() {
        let put_back = std::fs::write(&path, &manifest).err().map(|w| format!("; putting Cargo.toml back: {w}")).unwrap_or_default();
        return Err(format!(
            "{e}
the pin stays at {}{put_back}",
            short(&old)
        ));
    }
    println!("{PACKAGE}: {} -> {} (Cargo.toml and Cargo.lock; ci --changed tests what depends on it)", short(&old), short(rev));
    Ok(())
}

/// Swarm Core's `main`, from `git ls-remote`.
fn main_of_swarm_core(root: &Path) -> Result<String, String> {
    let out = crate::process::git(root, &["ls-remote", URL, "refs/heads/main"], None, &[])?;
    let sha = out.split_whitespace().next().unwrap_or_default();
    if is_sha(sha) { Ok(sha.to_owned()) } else { Err(format!("git ls-remote {URL} refs/heads/main: no commit in `{}`", out.trim())) }
}

/// Whether `rev` is a full or abbreviated commit id: 7 to 40 hex digits.
pub fn is_sha(rev: &str) -> bool {
    (7..=40).contains(&rev.len()) && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(rev: &str) -> &str {
    rev.get(..8).unwrap_or(rev)
}

/// The commit the lock pins Swarm Core at: its package's `source = "git+…#<sha>"`.
pub fn pinned(lock: &str) -> Option<String> {
    let block = lock.split("[[package]]").find(|b| is_its_package(b))?;
    let source = block.lines().find_map(|l| l.trim().strip_prefix("source = \""))?;
    let sha = source.trim_end_matches('"').rsplit('#').next()?;
    is_sha(sha).then(|| sha.to_owned())
}

/// Whether a lock's `[[package]]` block is Swarm Core's.
fn is_its_package(block: &str) -> bool {
    block.lines().any(|l| l.trim() == format!("name = \"{PACKAGE}\""))
}

/// Whether a manifest line is Swarm Core's dependency: `swarm-core = { … }`.
fn declares_the_pin(line: &str) -> bool {
    line.trim_start().starts_with(&format!("{PACKAGE} = {{"))
}

/// `manifest` with Swarm Core's `rev` set to `rev`, and the rev it had. Refused for a rev that
/// isn't a commit id and a manifest with no `swarm-core = { …, rev = "…" }` line.
pub fn set_rev(manifest: &str, rev: &str) -> Result<(String, String), String> {
    if !is_sha(rev) {
        return Err(format!("`{rev}` isn't a commit id (7 to 40 hex digits)"));
    }
    let mut old = None;
    let lines: Vec<String> = manifest
        .split_inclusive('\n')
        .map(|line| {
            let Some(at) = line.find("rev = \"").filter(|_| old.is_none() && declares_the_pin(line)) else {
                return line.to_owned();
            };
            let start = at + "rev = \"".len();
            let Some(len) = line.get(start..).and_then(|r| r.find('"')) else { return line.to_owned() };
            old = line.get(start..start + len).map(str::to_owned);
            format!("{}{rev}{}", line.get(..start).unwrap_or_default(), line.get(start + len..).unwrap_or_default())
        })
        .collect();
    let old = old.ok_or_else(|| format!("the workspace manifest has no `{PACKAGE} = {{ git = …, rev = \"…\" }}` line"))?;
    Ok((lines.concat(), old))
}

/// Whether `old` to `new` (two versions of the workspace's `Cargo.toml` or `Cargo.lock`, named
/// by `file`) only moves Swarm Core's pin: the same, once Swarm Core's line (the manifest)
/// or its package entry (the lock) is set aside, and not the same as a whole.
pub fn only_moves_the_pin(file: &str, old: &str, new: &str) -> bool {
    let rest = |text: &str| match file {
        "Cargo.lock" => text.split("[[package]]").filter(|b| !is_its_package(b)).collect::<Vec<_>>().concat(),
        "Cargo.toml" => text.lines().filter(|l| !declares_the_pin(l)).collect::<Vec<_>>().join("\n"),
        _ => text.to_owned(),
    };
    // A Windows checkout has CRLF where git holds LF: compare the lines, not the bytes.
    let (old, new) = (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"));
    matches!(file, "Cargo.toml" | "Cargo.lock") && old != new && rest(&old) == rest(&new)
}

/// What overrides the pin in the committed workspace manifest (`manifest`) or cargo config
/// (`config`): a `path` or `branch` form of the dependency, or a `[patch]` of Swarm Core.
pub fn overrides(manifest: &str, config: &str) -> Vec<String> {
    let mut out = Vec::new();
    // The dependency itself, outside any `[patch…]` section (a patch is the check below).
    let mut patch_section = false;
    for line in manifest.lines().map(str::trim_start) {
        if line.starts_with('[') {
            patch_section = line.starts_with("[patch");
        } else if !patch_section && declares_the_pin(line) && (line.contains("path =") || line.contains("branch =") || !line.contains("rev =")) {
            out.push(format!("Cargo.toml pins {PACKAGE} by `path` or `branch`, or without `rev`: pin a revision (`cargo xtask swarm-core bump`)"));
        }
    }
    for (name, text) in [("Cargo.toml", manifest), (".cargo/config.toml", config)] {
        if patches(text) {
            out.push(format!("{name} patches {PACKAGE}: keep a patch for iterating in a user-level cargo config, never committed"));
        }
    }
    out
}

/// A consumer's `cargo xtask layers` check: its committed manifest and cargo config don't
/// override the pin ([`overrides`]).
pub fn check_committed(root: &Path) -> Result<(), String> {
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();
    let found = overrides(&read("Cargo.toml"), &read(".cargo/config.toml"));
    if found.is_empty() {
        return Ok(());
    }
    for o in &found {
        println!("  - {o}");
    }
    Err(format!("{} override(s) of Swarm Core (docs/development.md › Swarm Core)", found.len()))
}

/// Whether `text` (TOML) patches Swarm Core: a `[patch."…swarm-core…"]` section, or a
/// `swarm-core` entry in any `[patch…]` section.
fn patches(text: &str) -> bool {
    let mut in_patch = false;
    for l in text.lines().map(str::trim_start) {
        if l.starts_with('[') {
            in_patch = l.starts_with("[patch");
            if in_patch && l.contains(PACKAGE) {
                return true;
            }
        } else if in_patch && l.strip_prefix(PACKAGE).is_some_and(|r| r.trim_start().starts_with('=')) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "swarm-core = { git = \"https://github.com/mornycorp/swarm-core.git\", rev = \"a28c7554ddbfc7826a2c2db42628c2ac7133ead6\" }\n";

    fn manifest(pin: &str) -> String {
        format!("[workspace.dependencies]\n# The swarm tooling\n{pin}serde = \"1\"\n")
    }

    fn lock(sha: &str, serde: &str) -> String {
        format!(
            "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"{serde}\"\n\n[[package]]\nname = \"swarm-core\"\nversion = \"0.1.0\"\nsource = \"git+{URL}?rev={sha}#{sha}\"\n\n[[package]]\nname = \"xtask\"\ndependencies = [\n \"swarm-core\",\n]\n"
        )
    }

    #[test]
    fn the_lock_names_the_pinned_commit() {
        assert_eq!(pinned(&lock("a28c7554ddbfc7826a2c2db42628c2ac7133ead6", "1.0.0")).as_deref(), Some("a28c7554ddbfc7826a2c2db42628c2ac7133ead6"));
        assert_eq!(pinned("version = 4\n"), None);
        assert_eq!(pinned(&lock("not-a-sha", "1")), None);
    }

    #[test]
    fn bump_sets_the_rev_and_nothing_else() {
        let m = manifest(PIN);
        let (moved, old) = set_rev(&m, "0123456789abcdef0123456789abcdef01234567").unwrap();
        assert_eq!(old, "a28c7554ddbfc7826a2c2db42628c2ac7133ead6");
        assert_eq!(moved, m.replace("a28c7554ddbfc7826a2c2db42628c2ac7133ead6", "0123456789abcdef0123456789abcdef01234567"));
        assert!(set_rev(&m, "--evil").unwrap_err().contains("isn't a commit id"));
        assert!(set_rev(&m, "main").is_err() && set_rev(&m, &"a".repeat(41)).is_err());
        assert!(set_rev("[workspace.dependencies]\nserde = \"1\"\n", "abcdef1").unwrap_err().contains("no `swarm-core"));
    }

    /// A bump that fails changes nothing: a short id is refused before the manifest is touched,
    /// and a failed lock update puts the manifest back.
    #[test]
    fn a_failed_bump_leaves_the_manifest_as_it_was() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-pin-failed-bump");
        let path = dir.join("Cargo.toml");
        std::fs::write(&path, manifest(PIN)).unwrap();
        let full = "0123456789abcdef0123456789abcdef01234567";
        let e = bump_to(&dir, "0123456", || panic!("no update for a short id")).unwrap_err();
        assert!(e.contains("is short") && e.contains("git rev-parse 0123456"), "{e}");
        let e = bump_to(&dir, full, || Err("revspec not found".to_owned())).unwrap_err();
        assert!(e.contains("revspec not found") && e.contains("the pin stays at a28c7554"), "{e}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), manifest(PIN));
        bump_to(&dir, full, || Ok(())).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), manifest(&PIN.replace("a28c7554ddbfc7826a2c2db42628c2ac7133ead6", full)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_diff_that_only_moves_the_pin_is_told_from_any_other() {
        let (a, b) = ("a28c7554ddbfc7826a2c2db42628c2ac7133ead6", "0123456789abcdef0123456789abcdef01234567");
        let (m_a, m_b) = (manifest(PIN), manifest(&PIN.replace(a, b)));
        assert!(only_moves_the_pin("Cargo.toml", &m_a, &m_b));
        assert!(only_moves_the_pin("Cargo.lock", &lock(a, "1.0.0"), &lock(b, "1.0.0")));
        assert!(!only_moves_the_pin("Cargo.lock", &lock(a, "1.0.0"), &lock(b, "1.0.1")), "another package moved too");
        assert!(!only_moves_the_pin("Cargo.toml", &m_a, &m_b.replace("serde = \"1\"", "serde = \"2\"")));
        assert!(!only_moves_the_pin("Cargo.toml", &m_a, &m_a), "no change at all isn't a bump");
        assert!(!only_moves_the_pin("clippy.toml", "a", "b"));
        let added = format!("{}\n[[package]]\nname = \"new\"\n", lock(a, "1.0.0"));
        assert!(!only_moves_the_pin("Cargo.lock", &lock(a, "1.0.0"), &added));
    }

    #[test]
    fn a_committed_override_is_refused() {
        assert!(overrides(&manifest(PIN), "[alias]\nxtask = \"run -p xtask --\"\n").is_empty());
        let by_path = manifest("swarm-core = { path = \"../swarm-core/crates/swarm-core\" }\n");
        let by_branch = manifest("swarm-core = { git = \"ssh://x/y.git\", branch = \"main\" }\n");
        assert_eq!((overrides(&by_path, "").len(), overrides(&by_branch, "").len()), (1, 1));
        let patch = format!("[patch.\"{URL}\"]\nswarm-core = {{ path = \"../swarm-core/crates/swarm-core\" }}\n");
        assert!(overrides(&manifest(PIN), &patch).iter().any(|p| p.starts_with(".cargo/config.toml patches")));
        assert!(overrides(&format!("{}{patch}", manifest(PIN)), "").iter().any(|p| p.starts_with("Cargo.toml patches")));
        let other = format!("{}[patch.crates-io]\nserde = {{ path = \"../serde\" }}\n", manifest(PIN));
        assert!(overrides(&other, "[patch.crates-io]\nserde = { path = \"../serde\" }\n").is_empty(), "another crate's patch is no override");
        let by_name = format!("{}[patch.crates-io]\nswarm-core = {{ path = \"../f\" }}\n", manifest(PIN));
        assert_eq!(overrides(&by_name, "").len(), 1, "Swarm Core patched under another section");
    }
}
