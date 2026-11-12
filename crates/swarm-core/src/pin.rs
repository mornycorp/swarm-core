//! `cargo xtask swarm-core`: the pin of this
//! crate in a consumer's workspace manifest ([`URL`]), and the assets it ships.
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
//!   of the dependency (or one without a `rev`), a `[patch]` of its source and a `[replace]`, in
//!   the workspace's manifest, a member's or a cargo config, read as TOML however it is spelt, and a
//!   testkit at another `rev` than Swarm Core ([`check_committed`], [`manifest_overrides`]); a
//!   user-level cargo config can still patch it while iterating.

use std::path::Path;

use crate::{Hooks, Project};

/// Swarm Core's repository, as a consumer's workspace manifest names it.
pub const URL: &str = "https://github.com/mornycorp/swarm-core.git";
/// Its package name.
pub const PACKAGE: &str = "swarm-core";
/// Swarm Core's testkit (`swarm_core_testkit`, with the conformance run). An app lists it
/// beside [`PACKAGE`] in its workspace manifest, at the **same** `rev`: `bump` moves both, and the
/// lock holds both at one commit.
pub const TESTKIT: &str = "swarm-core-testkit";
/// What a consumer pins: Swarm Core, and for an app its testkit.
const PINNED: [&str; 2] = [PACKAGE, TESTKIT];

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
        // An app pins the testkit beside Swarm Core, at the same url and rev: one git source, so this moves
        // both packages of the lock (tried with two revs of one repo: `-p` of either moves both, and naming the
        // testkit would fail for a manifest that lists it before the app skeleton uses it).
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
    // At the rev already, the testkit too: nothing to do. A testkit at another rev is moved to it.
    if moved == manifest {
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
    let block = lock.split("[[package]]").find(|b| is_package(b, PACKAGE))?;
    let source = block.lines().find_map(|l| l.trim().strip_prefix("source = \""))?;
    let sha = source.trim_end_matches('"').rsplit('#').next()?;
    is_sha(sha).then(|| sha.to_owned())
}

/// Whether a lock's `[[package]]` block is `name`'s.
fn is_package(block: &str, name: &str) -> bool {
    block.lines().any(|l| l.trim() == format!("name = \"{name}\""))
}

/// Whether a lock's `[[package]]` block is one the pin holds: Swarm Core's or its testkit's.
fn is_its_package(block: &str) -> bool {
    PINNED.iter().any(|name| is_package(block, name))
}

/// Whether a manifest line is `name`'s dependency: `swarm-core = { … }`.
fn declares(line: &str, name: &str) -> bool {
    line.trim_start().starts_with(&format!("{name} = {{"))
}

/// The pinned package whose table a manifest header names (`[workspace.dependencies.swarm-core-testkit]`).
fn pinned_table(header: &str) -> Option<&'static str> {
    let header = header.trim();
    let dependencies = header.starts_with('[') && header.contains("dependencies");
    PINNED.into_iter().find(|name| dependencies && (header.ends_with(&format!(".{name}]")) || header.ends_with(&format!(".\"{name}\"]"))))
}

/// The manifest's lines, each with the pinned package it holds the `rev` of, if any: an inline
/// `swarm-core = { …, rev = "…" }`, or a `rev = "…"` line in a `[….dependencies.swarm-core]`
/// table.
fn pin_lines(manifest: &str) -> Vec<(&str, Option<&'static str>)> {
    let mut table = None;
    manifest
        .split_inclusive('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') {
                table = pinned_table(trimmed);
                return (line, None);
            }
            let inline = PINNED.into_iter().find(|name| declares(line, name));
            let in_table = table.filter(|_| trimmed.strip_prefix("rev").is_some_and(|r| r.trim_start().starts_with('=')));
            (line, inline.or(in_table))
        })
        .collect()
}

/// The byte span of the value of a whole key `rev = "…"` in `line`.
fn rev_span(line: &str) -> Option<(usize, usize)> {
    line.match_indices("rev").find_map(|(at, _)| {
        let whole = !line.get(..at).and_then(|h| h.chars().next_back()).is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
        let value = line.get(at + 3..)?.trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?;
        let start = line.len() - value.len();
        whole.then_some((start, value.find('"')?))
    })
}

/// `manifest` with Swarm Core's `rev` set to `rev`, and its testkit's, which is pinned at the
/// same one (inline or as a table), and the rev Swarm Core had. Refused for a rev that isn't
/// a commit id and a manifest with no `rev` for `swarm-core`.
pub fn set_rev(manifest: &str, rev: &str) -> Result<(String, String), String> {
    if !is_sha(rev) {
        return Err(format!("`{rev}` isn't a commit id (7 to 40 hex digits)"));
    }
    let mut old = None;
    let lines: Vec<String> = pin_lines(manifest)
        .into_iter()
        .map(|(line, pinned)| {
            let Some((start, len)) = pinned.and(rev_span(line)) else { return line.to_owned() };
            if old.is_none() && pinned == Some(PACKAGE) {
                old = line.get(start..start + len).map(str::to_owned);
            }
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
        "Cargo.toml" => {
            pin_lines(text).into_iter().filter(|(_, pinned)| pinned.is_none()).map(|(l, _)| l.trim_end_matches('\n')).collect::<Vec<_>>().join("\n")
        }
        _ => text.to_owned(),
    };
    // A Windows checkout has CRLF where git holds LF: compare the lines, not the bytes.
    let (old, new) = (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"));
    matches!(file, "Cargo.toml" | "Cargo.lock") && old != new && rest(&old) == rest(&new)
}

/// One thing in a committed file that overrides the pin, or holds its two packages apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    /// The file, from the workspace root.
    pub file: String,
    /// The rule a check reports it under: `swarm-core-patch` (an override) or
    /// `swarm-core-testkit-rev` (the testkit at another rev than Swarm Core).
    pub rule: &'static str,
    /// What is wrong, after the file's name (`patches swarm-core`).
    pub problem: String,
    /// What to do about it.
    pub fix: String,
    /// A text on the line it is on, for a line number (the package's name, `patch`, `replace`).
    pub near: &'static str,
}

impl std::fmt::Display for Override {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.file, self.problem, self.fix)
    }
}

/// Where to iterate on Swarm Core instead of an override in the repo.
const KEEP_IT_LOCAL: &str = "keep a path or patch for iterating in a user-level cargo config, never in the repo";

impl Override {
    fn new(file: &str, problem: String, near: &'static str) -> Override {
        let fix = format!("pin a revision (`cargo xtask swarm-core bump`); {KEEP_IT_LOCAL}");
        Override { file: file.to_owned(), rule: "swarm-core-patch", problem, fix, near }
    }
}

/// Whether a dependency `key = value` names one of the pinned packages: which.
fn pinned_dep(key: &str, value: &toml::Value) -> Option<&'static str> {
    let package = value.as_table().and_then(|t| t.get("package")).and_then(toml::Value::as_str).unwrap_or(key);
    PINNED.into_iter().find(|name| *name == package)
}

/// What is wrong with `value`, the dependency on pinned package `name`: a `path`, a `branch`, or
/// no `rev` (a version, a `tag`). A `workspace = true` one is the workspace's (checked there).
fn unpinned(name: &str, value: &toml::Value) -> Option<String> {
    let Some(table) = value.as_table() else { return Some(format!("names {name} without a `rev`")) };
    if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        return None;
    }
    let by = ["path", "branch"].into_iter().find(|k| table.contains_key(*k)).map(|k| format!("pins {name} by `{k}`"));
    by.or_else(|| (!table.contains_key("rev")).then(|| format!("names {name} without a `rev`")))
}

/// The overrides in the dependency tables of a manifest (`file`, parsed as `manifest`): the
/// Swarm Core or its testkit by `path` or `branch`, or without a `rev`. A member's manifest is
/// checked as much as the root's: cargo builds the member with what it names.
pub fn dependency_overrides(file: &str, manifest: &toml::Table) -> Vec<Override> {
    let mut out = Vec::new();
    for (_, _, deps) in crate::checks::dependency_tables(manifest) {
        for (key, value) in deps {
            if let Some(problem) = pinned_dep(key, value).and_then(|name| unpinned(name, value)) {
                out.push(Override::new(file, problem, PACKAGE));
            }
        }
    }
    out
}

/// The overrides in a workspace's root manifest (`file`, parsed as `manifest`): those of its own
/// dependency tables ([`dependency_overrides`]), of `[workspace.dependencies]` (inline or as a
/// table), a `[patch]` of Swarm Core (as a section, dotted keys or inline) and a `[replace]`; and
/// the testkit at another `rev` than Swarm Core.
pub fn manifest_overrides(file: &str, manifest: &toml::Table) -> Vec<Override> {
    let mut out = dependency_overrides(file, manifest);
    let workspace_deps = manifest.get("workspace").and_then(toml::Value::as_table).and_then(|w| w.get("dependencies")).and_then(toml::Value::as_table);
    let mut revs = Vec::new();
    for (key, value) in workspace_deps.into_iter().flatten() {
        let Some(name) = pinned_dep(key, value) else { continue };
        match unpinned(name, value) {
            Some(problem) => out.push(Override::new(file, problem, PACKAGE)),
            None => revs.extend(value.get("rev").and_then(toml::Value::as_str).map(|r| (name, r))),
        }
    }
    let rev_of = |name: &str| revs.iter().find(|(n, _)| *n == name).map(|(_, r)| *r);
    if let (Some(core), Some(testkit)) = (rev_of(PACKAGE), rev_of(TESTKIT))
        && core != testkit
    {
        let problem = format!("pins {TESTKIT} at {testkit}, not at {PACKAGE}'s {core}: they are one pin");
        let fix = format!("`cargo xtask swarm-core bump --rev {core}` moves the testkit to it");
        out.push(Override { file: file.to_owned(), rule: "swarm-core-testkit-rev", problem, fix, near: TESTKIT });
    }
    out.extend(patch_overrides(file, manifest));
    if manifest.get("replace").and_then(toml::Value::as_table).is_some_and(|r| r.keys().any(|k| k.contains(PACKAGE))) {
        out.push(Override::new(file, format!("replaces {PACKAGE} (`[replace]`)"), "replace"));
    }
    out
}

/// The overrides in a cargo config (`file`, parsed as `config`): a `[patch]` of Swarm Core.
pub fn config_overrides(file: &str, config: &toml::Table) -> Vec<Override> {
    patch_overrides(file, config)
}

/// A `patch` of Swarm Core in `table` (a manifest or a cargo config), however the TOML
/// spells it: a `[patch."<its url>"]` source, or a pinned package patched under any source.
fn patch_overrides(file: &str, table: &toml::Table) -> Vec<Override> {
    let sources = table.get("patch").and_then(toml::Value::as_table);
    let patched = sources
        .into_iter()
        .flatten()
        .any(|(source, entries)| source.contains(PACKAGE) || entries.as_table().is_some_and(|e| e.iter().any(|(key, value)| pinned_dep(key, value).is_some())));
    if patched { vec![Override::new(file, format!("patches {PACKAGE}"), "patch")] } else { Vec::new() }
}

/// What overrides the pin in the committed workspace manifest (`manifest`) or cargo config
/// (`config`), as [`manifest_overrides`] and [`config_overrides`] find it; a file that isn't TOML is
/// one too, since what it overrides can't be told. A whole workspace (its members, the legacy
/// `.cargo/config`) is [`check_committed`]'s.
pub fn overrides(manifest: &str, config: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (file, text) in [("Cargo.toml", manifest), (".cargo/config.toml", config)] {
        match text.parse::<toml::Table>() {
            Ok(table) if file == "Cargo.toml" => out.extend(manifest_overrides(file, &table).iter().map(Override::to_string)),
            Ok(table) => out.extend(config_overrides(file, &table).iter().map(Override::to_string)),
            Err(e) => out.push(format!("{file} is not valid TOML ({}), so what it overrides can't be told: fix it", e.message().trim())),
        }
    }
    out
}

/// A consumer's `cargo xtask layers` check: no committed file of the workspace at `root` overrides
/// the pin: the root manifest, each member's, `.cargo/config.toml` and the legacy `.cargo/config`
/// (one that exists but can't be read or parsed is a finding too).
pub fn check_committed(root: &Path) -> Result<(), String> {
    let found = crate::checks::pin_findings(root);
    if found.is_empty() {
        return Ok(());
    }
    for o in &found {
        println!("  - {o}");
    }
    Err(format!("{} override(s) of Swarm Core (docs/development.md › Swarm Core)", found.len()))
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

    /// An app pins Swarm Core's testkit beside it, at the same commit: one bump moves both
    /// lines of the manifest and both entries of the lock, and a diff that only moves them is the pin's.
    #[test]
    fn a_pin_that_holds_the_testkit_moves_both_together() {
        let (a, b) = ("a28c7554ddbfc7826a2c2db42628c2ac7133ead6", "0123456789abcdef0123456789abcdef01234567");
        let testkit = |sha: &str| format!("{TESTKIT} = {{ git = \"{URL}\", rev = \"{sha}\" }}\n");
        let both = |sha: &str| manifest(&format!("{}{}", PIN.replace(a, sha), testkit(sha)));
        let (moved, old) = set_rev(&both(a), b).unwrap();
        assert_eq!((moved.as_str(), old.as_str()), (both(b).as_str(), a), "both lines, and Swarm Core's old rev");
        let lock_both =
            |sha: &str| format!("{}\n[[package]]\nname = \"{TESTKIT}\"\nversion = \"0.1.0\"\nsource = \"git+{URL}?rev={sha}#{sha}\"\n", lock(sha, "1.0.0"));
        assert_eq!(pinned(&lock_both(a)).as_deref(), Some(a), "Swarm Core's commit, whichever entry comes first");
        assert!(only_moves_the_pin("Cargo.toml", &both(a), &both(b)));
        assert!(only_moves_the_pin("Cargo.lock", &lock_both(a), &lock_both(b)));
        assert!(!only_moves_the_pin("Cargo.lock", &lock_both(a), &lock_both(b).replace("1.0.0", "1.0.1")));
        assert!(overrides(&both(a), "").is_empty());
        let by_path = manifest(&format!("{PIN}{TESTKIT} = {{ path = \"../swarm-core/crates/testkit\" }}\n"));
        assert_eq!(overrides(&by_path, "").len(), 1, "the testkit pinned by path is an override");
        let patch = format!("[patch.\"{URL}\"]\n{TESTKIT} = {{ path = \"../swarm-core/crates/testkit\" }}\n");
        assert!(overrides(&both(a), &patch).iter().any(|p| p.starts_with(".cargo/config.toml patches")));
        assert!(overrides(&format!("{}{patch}", both(a)), "").iter().any(|p| p.starts_with("Cargo.toml patches")));
    }

    /// A bump that fails changes nothing: a short id is refused before the manifest is touched,
    /// and a failed lock update puts the manifest back.
    #[test]
    fn a_failed_bump_leaves_the_manifest_as_it_was() {
        let dir = swarm_core_testkit::temp_dir("core-pin-failed-bump");
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

    /// Every spelling TOML allows is read as TOML, not as lines.
    #[test]
    fn an_override_in_any_toml_spelling_is_refused() {
        let table_form = "[workspace.dependencies.swarm-core]\npath = \"../swarm-core/crates/swarm-core\"\n";
        assert!(overrides(table_form, "").iter().any(|o| o.starts_with("Cargo.toml pins swarm-core by `path`")), "{:?}", overrides(table_form, ""));
        let dotted = format!("patch.\"{URL}\".swarm-core = {{ path = \"../sf\" }}\n{}", manifest(PIN));
        assert!(overrides(&dotted, "").iter().any(|o| o.starts_with("Cargo.toml patches swarm-core")));
        let inline = format!("patch = {{ \"{URL}\" = {{ swarm-core = {{ path = \"../sf\" }} }} }}\n");
        assert!(overrides(&manifest(PIN), &inline).iter().any(|o| o.starts_with(".cargo/config.toml patches swarm-core")));
        let replace = format!("{}[replace]\n\"swarm-core:0.1.0\" = {{ path = \"../sf\" }}\n", manifest(PIN));
        assert!(overrides(&replace, "").iter().any(|o| o.starts_with("Cargo.toml replaces swarm-core")));
        let renamed = manifest("sf = { package = \"swarm-core\", path = \"../sf\" }\n");
        assert_eq!(overrides(&renamed, "").len(), 1, "renamed: {:?}", overrides(&renamed, ""));
        assert_eq!(overrides(&manifest("swarm-core = \"0.1\"\n"), "").len(), 1, "a version is no rev");
        let own = "[package]\nname = \"x\"\n[dependencies]\nswarm-core = { git = \"x\", branch = \"main\" }\n";
        assert!(overrides(own, "").iter().any(|o| o.starts_with("Cargo.toml pins swarm-core by `branch`")));
        assert!(overrides(&manifest(PIN), "[patch\n").iter().any(|o| o.starts_with(".cargo/config.toml is not valid TOML")));
        // A member's manifest, on its own: a `workspace = true` dependency is the root's.
        let member = |dep: &str| dependency_overrides("a/Cargo.toml", &format!("[package]\nname = \"a\"\n[dev-dependencies]\n{dep}\n").parse().unwrap());
        assert!(member("swarm-core-testkit = { workspace = true }").is_empty());
        assert_eq!(
            member("swarm-core = { path = \"../../sf\" }").first().map(|o| o.to_string()).unwrap_or_default().split(':').next(),
            Some("a/Cargo.toml pins swarm-core by `path`")
        );
    }

    /// The testkit is one pin with Swarm Core: `bump` moves its table form too, a rev that
    /// differs is reported, and `bump --rev <current>` puts it back in step.
    #[test]
    fn the_testkit_moves_with_swarm_core_in_any_form_and_a_drift_is_reported() {
        let (a, b, c) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        let table = |f: &str, t: &str| {
            format!(
                "[workspace.dependencies]\nswarm-core = {{ git = \"{URL}\", rev = \"{f}\" }}\n\n[workspace.dependencies.swarm-core-testkit]\ngit = \"{URL}\"\nrev = \"{t}\"\n"
            )
        };
        let (moved, old) = set_rev(&table(&a, &a), &b).unwrap();
        assert_eq!((moved, old), (table(&b, &b), a.clone()));
        let swarm_core_table = format!("[workspace.dependencies.swarm-core]\ngit = \"{URL}\"\nrevision = 1\nrev = \"{a}\"\n");
        assert_eq!(set_rev(&swarm_core_table, &b).unwrap(), (swarm_core_table.replace(&a, &b), a.clone()));
        assert!(only_moves_the_pin("Cargo.toml", &table(&a, &a), &table(&b, &b)));
        // A drift: overrides says so, with the fix.
        assert!(overrides(&table(&a, &a), "").is_empty());
        let drift = overrides(&table(&a, &c), "");
        assert_eq!(
            drift,
            [format!(
                "Cargo.toml pins {TESTKIT} at {c}, not at {PACKAGE}'s {a}: they are one pin: `cargo xtask swarm-core bump --rev {a}` moves the testkit to it"
            )]
        );
        let inline = manifest(&format!("{}{TESTKIT} = {{ git = \"{URL}\", rev = \"{c}\" }}\n", PIN.replace("a28c7554ddbfc7826a2c2db42628c2ac7133ead6", &a)));
        assert_eq!(overrides(&inline, "").len(), 1);
        // `bump --rev <Swarm Core's>` moves the testkit to it rather than saying it's there.
        let dir = swarm_core_testkit::temp_dir("core-pin-drift");
        std::fs::write(dir.join("Cargo.toml"), table(&a, &c)).unwrap();
        let mut updated = false;
        bump_to(&dir, &a, || {
            updated = true;
            Ok(())
        })
        .unwrap();
        assert!(updated && std::fs::read_to_string(dir.join("Cargo.toml")).unwrap() == table(&a, &a));
        bump_to(&dir, &a, || panic!("in step: nothing to update")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
