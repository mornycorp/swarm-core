//! The files both swarms keep the same (phase 4): the swarm protocol,
//! the lessons and the progress-docs standard (`docs/`), the GitHub templates (`.github/`), the
//! pre-push hook and the label script (`scripts/`), and the `complexity-analyzer` and `add-command` skills
//! (`.claude/skills/`). They live here, in `crates/swarm-core/assets/`
//! at the path each takes in a consumer, built into the crate ([`ASSETS`]), so a consumer's copy
//! is the one its pinned revision ships.
//!
//! - **`cargo xtask swarm-core sync-assets`** ([`sync`]) writes them into the consumer, removes the
//!   ones an earlier revision shipped and this one doesn't, and lists them in [`MANIFEST`].
//! - **`--check`** ([`check`]), which the consumer's gate runs, fails when a copy differs from what
//!   the pinned revision writes: a pinned copy is changed here, then bumped.
//! - **Placeholders.** The docs, templates and scripts are templates: `{{name}}` takes the
//!   consumer's value, from its [`Project`] (`env`, `project`, `github.host`, `github.repo`,
//!   `checkout_marker`, `stack_state`, `full_gate_worktree`, and
//!   `gate_lock_max_age`, its `limits.gate_lock_max_age_secs` in words) or its
//!   [`Hooks::asset_vars`] (such as `product`, `cli`, `ui_crate`), or the prefix of its package
//!   names (`crate_prefix`, `crate_prefix_ident`: [`Hooks::changed`]'s `package_prefix`, as `my-`
//!   and `my_`, for the app skeleton, [`crate::skeleton`]; or, when that is empty, a `crate_prefix`
//!   in its [`Hooks::asset_vars`]). One it doesn't give is refused.
//!   The skills are copied as they are, but their README and `add-command` (its commands name
//!   the app's CLI, `{{cli}}`, and its environment prefix, `{{env}}`): see [`TEMPLATED`].
//!
//! Line endings: an asset is written with LF and compared without regard to CR, as a Windows
//! checkout may hold either.

mod list;

use std::path::Path;

use crate::{Hooks, Project};

pub use list::ASSETS;

/// One file Swarm Core ships: its path in a consumer (from the root, `/` separators) and its
/// text as this revision has it.
#[derive(Debug, Clone, Copy)]
pub struct Asset {
    pub path: &'static str,
    pub text: &'static str,
}

/// The list of the pinned copies a consumer holds, written by [`sync`]: what a later sync removes
/// when Swarm Core stops shipping one, and what a reader is told not to edit.
pub const MANIFEST: &str = "docs/swarm-core-assets.md";

/// The assets that are copied as they are, with no placeholders: the skills' own files.
const VERBATIM: &str = ".claude/skills/";

/// The assets a consumer runs as programs: written executable on Unix.
const EXECUTABLE: &[&str] = &["scripts/hooks/pre-push"];

/// What under [`VERBATIM`] is a template all the same: the skills' README, and every file of
/// `add-command` (a path ending in `/` is a folder), whose commands differ by app.
const TEMPLATED: &[&str] = &[".claude/skills/README.md", ".claude/skills/add-command/"];

impl Asset {
    /// Whether its `{{name}}` placeholders take the consumer's values.
    pub fn is_template(&self) -> bool {
        !self.path.starts_with(VERBATIM) || TEMPLATED.iter().any(|t| self.path == *t || (t.ends_with('/') && self.path.starts_with(t)))
    }
}

/// The prefix of the workspace's package names, which the app skeleton's crates take (`my-` for
/// `my-engine`): [`crate::changed::Rules::package_prefix`], so a project that has told `ci
/// --changed` its prefix has told the skeleton too; when that is empty, `crate_prefix` in
/// [`Hooks::asset_vars`]. `None` for neither.
fn crate_prefix(hooks: &dyn Hooks) -> Option<&'static str> {
    let own = || hooks.asset_vars().iter().find(|(k, _)| *k == "crate_prefix").map(|(_, v)| *v);
    Some(hooks.changed().package_prefix).filter(|p| !p.is_empty()).or_else(own).filter(|p| !p.is_empty())
}

/// The value of placeholder `name` for `project`: its settings first, then its own values.
pub(crate) fn var(project: &Project, hooks: &dyn Hooks, name: &str) -> Option<String> {
    match name {
        "crate_prefix" => return crate_prefix(hooks).map(str::to_owned),
        "crate_prefix_ident" => return crate_prefix(hooks).map(|p| p.replace('-', "_")),
        _ => {}
    }
    let builtin = match name {
        "env" => Some(project.env_prefix),
        "project" => Some(project.name),
        "github.host" => Some(project.github.host),
        "github.repo" => Some(project.github.repo),
        "checkout_marker" => Some(project.checkout_marker),
        "stack_state" => Some(project.stack_state),
        "full_gate_worktree" => Some(project.full_gate_worktree),
        "gate_lock_max_age" => return Some(duration(project.limits.gate_lock_max_age_secs)),
        _ => None,
    };
    builtin.or_else(|| hooks.asset_vars().iter().find(|(k, _)| *k == name).map(|(_, v)| *v)).map(str::to_owned)
}

/// `secs` as a reader says it: whole hours (`2 hours`, `1 hour`), else minutes.
fn duration(secs: i64) -> String {
    let unit = |n: i64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    if secs > 0 && secs % 3600 == 0 { unit(secs / 3600, "hour") } else { unit(secs / 60, "minute") }
}

/// Whether `name` can be a placeholder's name: lowercase letters, digits, `_` and `.`.
fn is_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

/// The placeholders `text` uses, in order (a name may repeat).
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("{{") {
        let after = rest.get(at + 2..).unwrap_or_default();
        let Some(end) = after.find("}}") else { break };
        let name = after.get(..end).unwrap_or_default();
        if is_name(name) {
            out.push(name);
            rest = after.get(end + 2..).unwrap_or_default();
        } else {
            rest = after;
        }
    }
    out
}

/// `asset` as `project` keeps it: LF line endings, and a template's placeholders filled in. An
/// error names a placeholder the project gives no value.
pub fn render(project: &Project, hooks: &dyn Hooks, asset: &Asset) -> Result<String, String> {
    let text = asset.text.replace("\r\n", "\n");
    if !asset.is_template() {
        return Ok(text);
    }
    fill(project, hooks, asset.path, text)
}

/// `text`, the file at `path`, with its placeholders filled in for `project`. An error names a
/// placeholder the project gives no value.
pub fn fill(project: &Project, hooks: &dyn Hooks, path: &str, mut text: String) -> Result<String, String> {
    let mut names = placeholders(&text);
    names.sort_unstable();
    names.dedup();
    let names: Vec<String> = names.into_iter().map(str::to_owned).collect();
    for name in names {
        let value = var(project, hooks, &name).ok_or_else(|| format!("{}: the project gives no value for `{{{{{name}}}}}` (Hooks::asset_vars)", path))?;
        text = text.replace(&format!("{{{{{name}}}}}", name = name), &value);
    }
    Ok(text)
}

/// What [`MANIFEST`] holds for this revision's assets.
pub fn manifest() -> String {
    let mut s = String::from(
        "# Pinned assets\n\n\
         Written by `cargo xtask swarm-core sync-assets`: the files below are pinned copies from\n\
         [swarm-core](https://github.com/mornycorp/swarm-core) \
         (`crates/swarm-core/assets/`), at the revision `Cargo.lock` pins. Don't edit them here: change\n\
         them in Swarm Core, then `cargo xtask swarm-core bump` and `sync-assets`. The gate's\n\
         `sync-assets --check` fails on a copy that differs.\n\n",
    );
    for a in ASSETS {
        s.push_str(&format!("- `{}`\n", a.path));
    }
    s
}

/// The paths an earlier [`MANIFEST`] lists, those that are safe to touch only: relative, inside
/// the root, no `..`.
fn listed(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("- `")?.strip_suffix('`'))
        .filter(|p| !p.is_empty() && !p.starts_with('/') && !p.contains(':') && !p.contains('\\') && !p.split('/').any(|c| c == ".." || c.is_empty()))
        .map(str::to_owned)
        .collect()
}

/// What differs at `root` from what this revision writes: each path, and how.
pub fn differences(project: &Project, hooks: &dyn Hooks, root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let read = |p: &str| std::fs::read_to_string(root.join(p)).ok().map(|t| t.replace("\r\n", "\n"));
    for a in ASSETS {
        let want = render(project, hooks, a)?;
        match read(a.path) {
            None => out.push(format!("{}: missing", a.path)),
            Some(have) if have != want => out.push(format!("{}: differs", a.path)),
            Some(_) => {}
        }
    }
    let old = read(MANIFEST);
    if old.as_deref() != Some(manifest().as_str()) {
        out.push(format!("{MANIFEST}: {}", if old.is_none() { "missing" } else { "differs" }));
    }
    for stale in old.as_deref().map(listed).unwrap_or_default() {
        if !ASSETS.iter().any(|a| a.path == stale) && root.join(&stale).exists() {
            out.push(format!("{stale}: no longer shipped"));
        }
    }
    Ok(out)
}

/// `cargo xtask swarm-core sync-assets --check`: an error when a pinned copy at `root` differs.
pub fn check(project: &Project, hooks: &dyn Hooks, root: &Path) -> Result<(), String> {
    let diff = differences(project, hooks, root)?;
    if diff.is_empty() {
        println!("swarm-core: {} pinned assets match swarm-core {}", ASSETS.len(), crate::VERSION);
        return Ok(());
    }
    for d in &diff {
        println!("  - {d}");
    }
    Err(format!(
        "{} pinned asset(s) differ from the pinned swarm-core: change them in Swarm Core, then `cargo xtask swarm-core bump`; or run `cargo xtask swarm-core sync-assets` ({MANIFEST})",
        diff.len()
    ))
}

/// Write `text` at `root`/`path`, its directory made; executable when [`EXECUTABLE`] says so.
fn write(root: &Path, path: &str, text: &str) -> Result<(), String> {
    let at = root.join(path);
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&at, text).map_err(|e| format!("{path}: {e}"))?;
    if EXECUTABLE.contains(&path) {
        set_executable(&at).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

/// Mark the file at `at` executable (Unix; elsewhere, nothing to do).
fn set_executable(at: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = at;
        Ok(())
    }
}

/// `cargo xtask swarm-core sync-assets`: write every asset at `root`, remove the ones an earlier
/// revision shipped and this one doesn't, and write [`MANIFEST`]. A file that already matches is
/// left as it is.
pub fn sync(project: &Project, hooks: &dyn Hooks, root: &Path) -> Result<(), String> {
    let rendered: Vec<(&str, String)> = ASSETS.iter().map(|a| render(project, hooks, a).map(|t| (a.path, t))).collect::<Result<_, _>>()?;
    let read = |p: &str| std::fs::read_to_string(root.join(p)).ok().map(|t| t.replace("\r\n", "\n"));
    let mut written = 0;
    for (path, text) in &rendered {
        if read(path).as_deref() != Some(text.as_str()) {
            write(root, path, text)?;
            written += 1;
        }
    }
    let mut removed = 0;
    for stale in read(MANIFEST).as_deref().map(listed).unwrap_or_default() {
        if !ASSETS.iter().any(|a| a.path == stale) && root.join(&stale).is_file() {
            std::fs::remove_file(root.join(&stale)).map_err(|e| format!("{stale}: {e}"))?;
            removed += 1;
        }
    }
    write(root, MANIFEST, &manifest())?;
    println!("swarm-core: {} pinned assets from swarm-core {}: {written} written, {removed} removed ({MANIFEST})", ASSETS.len(), crate::VERSION);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
