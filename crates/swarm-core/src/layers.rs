//! `cargo xtask layers`: a consumer's architecture, enforced (docs/architecture.md in each
//! consumer, from the template `swarm-core init` writes). The consumer describes its workspace in a
//! [`Table`] ([`crate::Hooks::layers`]) and wires the check in as its own command and as a step of
//! its `ci`; a consumer without a table has nothing checked.
//!
//! A consumer may instead (or as well) declare each crate's layer in its own manifest and turn
//! pattern scans on ([`crate::Hooks::architecture`], [`crate::Hooks::scans`]): the same command
//! then runs [`crate::checks`], and with no table holds the exemptions and the crate map from
//! the manifests.
//!
//! - **Layers.** Every package is in the table at a layer, or exempt (apps, tooling); a crate
//!   depends (normally) only on crates in strictly lower layers, and never on an exempt one.
//! - **UI crates stay up.** Nothing below [`Table::ui_layer`] uses [`Table::ui_crates`].
//! - **Dev-only crates** (a testkit) are only ever dev-dependencies.
//! - **Lint headers.** Every manifest has `[lints] workspace = true`; every crate root denies the
//!   panicking lints and switches the code-health limits on (off for its tests); every library
//!   root forbids `unsafe`, but a package the project allows it ([`crate::Hooks::unsafe_crates`]),
//!   whose root instead denies `unsafe_op_in_unsafe_fn` and warns on undocumented `unsafe` blocks.
//! - **Code-health exemptions** are an item's `#[expect(clippy::…, reason = "…")]`, never an
//!   `allow` or crate-wide; their count is printed, a ratchet that should only go down.
//! - **Typed contracts.** In [`Table::reply_readers`] (the code that consumes command replies: the
//!   UI, the CLI, automation), no production line reads a `serde_json::Value` by field
//!   (`v["key"]`, `.get("key")`, `.as_object()`), beyond what [`Table::allowed_reads`] allows.
//! - **No raw NUL bytes** in tracked `.rs`, `.toml` or `.md` files: git merges such a file as
//!   binary, dropping one side without conflict markers.
//! - **The crate map** in AGENTS.md, between [`MAP_START`] and [`MAP_END`], is rendered from the
//!   table; `--write` regenerates it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

/// One package's place.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub package: &'static str,
    /// `None`: exempt (apps, tooling, a testkit); `Some(n)`: layer n, 0 the lowest.
    pub layer: Option<u8>,
    /// How the crate map names its layer (`L0`, `app`, `dev`).
    pub kind: &'static str,
    /// One line: what it holds.
    pub summary: &'static str,
}

/// A library package allowed `unsafe` ([`crate::Hooks::unsafe_crates`]), and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsafe {
    pub package: &'static str,
    /// One line: what it binds that can't be written without `unsafe`.
    pub reason: &'static str,
}

/// What a root allowed `unsafe` carries in place of `#![forbid(unsafe_code)]`.
pub const UNSAFE_LINES: [&str; 2] = ["#![deny(unsafe_op_in_unsafe_fn)]", "#![warn(clippy::undocumented_unsafe_blocks)]"];

/// A file allowed some untyped reply reads, and why: a ratchet, lowered as they are converted.
#[derive(Debug, Clone, Copy)]
pub struct Allowed {
    /// From the workspace root, with `/`.
    pub file: &'static str,
    pub count: usize,
    pub reason: &'static str,
}

/// A consumer's workspace, as `layers` checks it.
#[derive(Debug, Clone, Copy)]
pub struct Table {
    /// Every package, top layer first.
    pub entries: &'static [Entry],
    /// The lowest layer that may use [`Table::ui_crates`].
    pub ui_layer: u8,
    /// The UI toolkit's crates (`egui`, `eframe`, `winit`, …).
    pub ui_crates: &'static [&'static str],
    /// Packages that are only ever dev-dependencies (a testkit).
    pub dev_only: &'static [&'static str],
    /// Source directories (from the root, ending `/`) whose code consumes command replies.
    pub reply_readers: &'static [&'static str],
    /// The untyped reply reads allowed, by file.
    pub allowed_reads: &'static [Allowed],
}

pub const USAGE: &str =
    "cargo xtask layers [--write]: check the workspace's layers, lint headers, typed reply reads and crate map; --write regenerates the crate map in AGENTS.md";

/// The panicking lints every crate root denies.
pub const DENY_LINE: &str = "#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]";
/// The code-health limits every crate root switches on for production code (thresholds in
/// `clippy.toml`)…
pub const LIMITS_LINE: &str = "#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]";
/// …and off for its tests.
pub const LIMITS_TEST_LINE: &str = "#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]";
/// The lints a unit may be exempted from, only by `#[expect(clippy::…, reason = "…")]`.
pub const LIMIT_LINTS: [&str; 3] = ["cognitive_complexity", "too_many_lines", "too_many_arguments"];
/// The crate map's markers in AGENTS.md.
pub const MAP_START: &str = "<!-- crate-map:start (cargo xtask layers --write) -->";
pub const MAP_END: &str = "<!-- crate-map:end -->";

const GIT_DEADLINE: Duration = Duration::from_secs(30);
const METADATA_DEADLINE: Duration = Duration::from_secs(120);

/// `cargo xtask layers [--write]` for the consumer at `root`.
pub fn run(hooks: &dyn crate::Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    let write = match args {
        [] => false,
        ["--write"] => true,
        _ => return Err(format!("usage: {USAGE}")),
    };
    let (table, manifests) = (hooks.layers(), crate::checks::configured(hooks));
    if table.is_none() && !manifests {
        println!("layers: this project describes no layers (Hooks::layers): nothing to check");
        return Ok(());
    }
    let mut problems = Vec::new();
    if let Some(table) = &table {
        problems.extend(table_problems(hooks, table, root, write)?);
    }
    if manifests {
        problems.extend(manifest_problems(hooks, root, write, table.is_none())?);
    }
    problems.extend(raw_nuls(root));
    if !problems.is_empty() {
        return Err(format!("{} layering problem(s):\n  {}", problems.len(), problems.join("\n  ")));
    }
    if let Some(table) = &table {
        println!("layers: {} packages, all in order", table.entries.len());
    }
    Ok(())
}

/// The table's checks ([`crate::Hooks::layers`]): `cargo metadata` held to it, and its crate map.
fn table_problems(hooks: &dyn crate::Hooks, table: &Table, root: &Path, write: bool) -> Result<Vec<String>, String> {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root).args(["metadata", "--no-deps", "--format-version", "1"]);
    let text = crate::process::output(cmd, None, METADATA_DEADLINE, "cargo metadata")?;
    let meta: Value = serde_json::from_str(&text).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut problems = check_unsafe(table, hooks.unsafe_crates(), &meta, root);
    problems.extend(map_problem(&crate_map(table), root, write, true)?);
    Ok(problems)
}

/// The manifests' checks ([`crate::checks`]: the layers each manifest declares, the scans), which
/// print their own report. With `alone` (no table), the two rules the table path holds are held
/// here too: the code-health exemptions, and AGENTS.md's crate map when it has the markers,
/// rendered from the manifests.
fn manifest_problems(hooks: &dyn crate::Hooks, root: &Path, write: bool, alone: bool) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    match crate::checks::check_with(hooks, root, false) {
        Ok(report) => println!("{report}"),
        Err(findings) => problems.extend(findings.0.iter().map(ToString::to_string)),
    }
    if !alone {
        return Ok(problems);
    }
    let members = crate::checks::declared_members(root);
    let mut exemptions = BTreeMap::new();
    for m in &members {
        for file in rust_files(&root.join(&m.dir).join("src")) {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let shown = file.strip_prefix(root).unwrap_or(&file).display().to_string().replace('\\', "/");
            let (counts, found) = limit_exemptions(&text, &shown);
            problems.extend(found);
            let tally: &mut [usize; 3] = exemptions.entry(m.package.clone()).or_default();
            for (t, c) in tally.iter_mut().zip(counts) {
                *t += c;
            }
        }
    }
    println!("{}", exemptions_line(&exemptions));
    if hooks.architecture().is_some() {
        problems.extend(map_problem(&crate::checks::crate_map(&members), root, write, false)?);
    }
    Ok(problems)
}

/// A workspace member from `cargo metadata`: its name, directory and dependencies (name, dev).
struct Member {
    name: String,
    dir: PathBuf,
    deps: Vec<(String, bool)>,
}

fn members(meta: &Value) -> Vec<Member> {
    let packages = meta.get("packages").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    packages
        .iter()
        .filter_map(|p| {
            let name = p.get("name")?.as_str()?.to_owned();
            let dir = Path::new(p.get("manifest_path")?.as_str()?).parent()?.to_path_buf();
            let deps = p.get("dependencies").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
            let deps = deps.iter().filter_map(|d| Some((d.get("name")?.as_str()?.to_owned(), d.get("kind").and_then(Value::as_str) == Some("dev")))).collect();
            Some(Member { name, dir, deps })
        })
        .collect()
}

/// Everything `layers` checks but raw NULs and the crate map, for `meta` (`cargo metadata`).
pub fn check(table: &Table, meta: &Value, root: &Path) -> Vec<String> {
    check_unsafe(table, &[], meta, root)
}

/// [`check`], with the library packages allowed `unsafe`.
pub fn check_unsafe(table: &Table, allowed: &[Unsafe], meta: &Value, root: &Path) -> Vec<String> {
    let members = members(meta);
    let mut problems = registration(table, &members);
    for a in allowed {
        if !members.iter().any(|m| m.name == a.package) {
            problems.push(format!("unsafe_crates lists {}, which is not a workspace member", a.package));
        }
        println!("layers: {} may use `unsafe`: {}", a.package, a.reason);
    }
    let mut exemptions = BTreeMap::new();
    let mut reads = Vec::new();
    for m in &members {
        let Some(me) = entry(table, &m.name) else { continue };
        problems.extend(dependencies(table, me, m));
        let safety = match me.layer {
            None => Safety::Exempt,
            Some(_) if allowed.iter().any(|a| a.package == m.name) => Safety::Allowed,
            Some(_) => Safety::Forbidden,
        };
        problems.extend(headers(m, safety));
        for file in rust_files(&m.dir.join("src")) {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let shown = file.strip_prefix(root).unwrap_or(&file).display().to_string().replace('\\', "/");
            let (counts, found) = limit_exemptions(&text, &shown);
            problems.extend(found);
            let tally: &mut [usize; 3] = exemptions.entry(m.name.clone()).or_default();
            for (t, c) in tally.iter_mut().zip(counts) {
                *t += c;
            }
            if table.reply_readers.iter().any(|d| shown.starts_with(d)) {
                reads.push((shown, production(&text).map(count_reads).sum()));
            }
        }
    }
    problems.extend(ratchet(&reads, table.allowed_reads));
    println!("{}", exemptions_line(&exemptions));
    problems
}

fn entry<'a>(table: &'a Table, name: &str) -> Option<&'a Entry> {
    table.entries.iter().find(|e| e.package == name)
}

/// Every member in the table, and every table entry a member.
fn registration(table: &Table, members: &[Member]) -> Vec<String> {
    let mut problems: Vec<String> = members
        .iter()
        .filter(|m| entry(table, &m.name).is_none())
        .map(|m| format!("{} is not in the layers table (Hooks::layers): register it at the lowest layer it fits", m.name))
        .collect();
    for e in table.entries.iter().filter(|e| !members.iter().any(|m| m.name == e.package)) {
        problems.push(format!("the layers table lists {}, which is not a workspace member", e.package));
    }
    problems
}

/// `m`'s dependencies against the layers.
fn dependencies(table: &Table, me: &Entry, m: &Member) -> Vec<String> {
    let mut problems = Vec::new();
    for (dep, dev) in &m.deps {
        if *dev {
            continue;
        }
        if let Some(layer) = me.layer.filter(|l| *l < table.ui_layer)
            && table.ui_crates.contains(&dep.as_str())
        {
            problems.push(format!("{} (L{layer}) depends on {dep}: UI crates are for L{} and up", m.name, table.ui_layer));
        }
        if table.dev_only.contains(&dep.as_str()) {
            problems.push(format!("{} depends on {dep} outside [dev-dependencies]: it is only ever a dev-dependency", m.name));
            continue;
        }
        let Some(target) = entry(table, dep) else { continue };
        match (me.layer, target.layer) {
            (Some(mine), Some(theirs)) if theirs >= mine => {
                problems.push(format!("{} (L{mine}) depends on {dep} (L{theirs}): only on strictly lower layers", m.name));
            }
            (_, None) => problems.push(format!("{} depends on {dep}, an app or tool", m.name)),
            _ => {}
        }
    }
    problems
}

/// What a package's library root says of `unsafe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Safety {
    /// An app or tool: nothing checked.
    Exempt,
    /// A library in a layer: `#![forbid(unsafe_code)]`.
    Forbidden,
    /// A library the project allows `unsafe`: [`UNSAFE_LINES`].
    Allowed,
}

/// `m`'s manifest and crate roots carry the lint headers, and its library root what `safety` says
/// of `unsafe`.
fn headers(m: &Member, safety: Safety) -> Vec<String> {
    let mut problems = Vec::new();
    let toml = std::fs::read_to_string(m.dir.join("Cargo.toml")).unwrap_or_default().replace("\r\n", "\n");
    if !toml.contains("[lints]\nworkspace = true") {
        problems.push(format!("{}: Cargo.toml must have `[lints] workspace = true`", m.name));
    }
    for root_file in ["src/lib.rs", "src/main.rs"] {
        let Ok(text) = std::fs::read_to_string(m.dir.join(root_file)) else { continue };
        for line in [DENY_LINE, LIMITS_LINE, LIMITS_TEST_LINE] {
            if !text.lines().any(|l| l.trim() == line) {
                problems.push(format!("{}/{root_file}: the crate root must carry `{line}`", m.name));
            }
        }
        if root_file != "src/lib.rs" {
            continue;
        }
        match safety {
            Safety::Forbidden if !text.contains("#![forbid(unsafe_code)]") => {
                problems.push(format!("{}/{root_file}: a library root must carry `#![forbid(unsafe_code)]`", m.name));
            }
            Safety::Allowed => {
                for line in UNSAFE_LINES.iter().filter(|l| !text.lines().any(|t| t.trim() == **l)) {
                    problems.push(format!("{}/{root_file}: a library root allowed `unsafe` must carry `{line}`", m.name));
                }
            }
            _ => {}
        }
    }
    problems
}

/// The code-health exemptions in one file (`shown`, its path): how many units each of
/// [`LIMIT_LINTS`] exempts, and the problems.
pub fn limit_exemptions(text: &str, shown: &str) -> ([usize; 3], Vec<String>) {
    let mut counts = [0usize; 3];
    let mut problems = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        let lints: Vec<usize> = (0..LIMIT_LINTS.len()).filter(|&k| LIMIT_LINTS.get(k).is_some_and(|l| line.contains(&format!("clippy::{l}")))).collect();
        if lints.is_empty() || line == LIMITS_LINE || line == LIMITS_TEST_LINE || line.starts_with("//") {
            continue;
        }
        let at = format!("{shown}:{}", i + 1);
        if line.starts_with("#![") {
            problems.push(format!("{at}: no crate-wide lint attribute for the code-health limits; exempt the unit with #[expect(…, reason = \"…\")]"));
        } else if line.starts_with("#[allow(") {
            problems.push(format!("{at}: exempt with #[expect(…, reason = \"…\")], not #[allow]: an expect goes away with the fix"));
        } else if line.starts_with("#[expect(") {
            if !line.contains("reason = \"") {
                problems.push(format!("{at}: a code-health exemption needs a `reason`"));
            }
            for k in lints {
                if let Some(c) = counts.get_mut(k) {
                    *c += 1;
                }
            }
        }
    }
    (counts, problems)
}

/// The exemptions in all, by lint and by crate: the ratchet.
pub fn exemptions_line(by_crate: &BTreeMap<String, [usize; 3]>) -> String {
    let per_lint: Vec<usize> = (0..LIMIT_LINTS.len()).map(|k| by_crate.values().map(|c| c.get(k).copied().unwrap_or(0)).sum()).collect();
    let total: usize = per_lint.iter().sum();
    let lints: Vec<String> = LIMIT_LINTS.iter().zip(&per_lint).map(|(l, n)| format!("{l} {n}")).collect();
    let crates: Vec<String> = by_crate.iter().map(|(c, n)| (c, n.iter().sum::<usize>())).filter(|(_, n)| *n > 0).map(|(c, n)| format!("{c} {n}")).collect();
    format!("layers: code-health exemptions: {total} ({}; {})", lints.join(", "), crates.join(", "))
}

/// Hold each file's untyped reads to its allowance (none: 0), and every allowance to a file read.
fn ratchet(found: &[(String, usize)], allowed: &[Allowed]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, n) in found {
        let allowance = allowed.iter().find(|a| a.file == name);
        let max = allowance.map_or(0, |a| a.count);
        if *n > max {
            let why = allowance.map_or(String::new(), |a| format!(" ({})", a.reason));
            problems.push(format!(
                "{name}: {n} untyped reply read(s) (`v[\"…\"]`, `.get(\"…\")`, `.as_object()`): read the command's typed result; the layers table allows {max}{why}"
            ));
        } else if *n < max {
            problems.push(format!("{name}: {n} untyped reply read(s), fewer than the {max} the layers table allows: lower its entry to {n}"));
        }
    }
    for a in allowed.iter().filter(|a| !found.iter().any(|(name, _)| name == a.file)) {
        problems.push(format!("the layers table allows untyped reads in {}, which isn't a reply reader's source: remove the entry", a.file));
    }
    problems
}

/// A source file's production lines: before its test module, comment lines left out.
fn production(text: &str) -> impl Iterator<Item = &str> {
    let lines: Vec<&str> = text.lines().collect();
    let test_module = |w: &[&str]| w.first().is_some_and(|l| l.trim() == "#[cfg(test)]") && w.get(1).is_some_and(|l| l.trim_start().starts_with("mod "));
    let end = lines.windows(2).position(test_module).unwrap_or(lines.len());
    lines.into_iter().take(end).filter(|l| !l.trim_start().starts_with("//"))
}

/// The untyped reads on a line: `x["key"]` (an index, not an array literal), `.get("key")` and
/// `.as_object()`.
pub fn count_reads(line: &str) -> usize {
    let indexed = line.match_indices("[\"").filter(|(i, _)| indexes_after(line, *i) && key_then(line, i + 2, "\"]")).count();
    let got = line.match_indices(".get(\"").filter(|(i, m)| key_then(line, i + m.len(), "\")")).count();
    indexed + got + line.matches(".as_object()").count()
}

/// Whether the `[` at byte `at` indexes what comes before it (`v[`, `f()[`), not an array literal.
fn indexes_after(line: &str, at: usize) -> bool {
    line.get(..at).and_then(|before| before.chars().next_back()).is_some_and(|c| c.is_alphanumeric() || "_)]".contains(c))
}

/// Whether `line` holds, from byte `at`, a key (letters, digits, `_`) and then `close`.
fn key_then(line: &str, at: usize, close: &str) -> bool {
    let rest = line.get(at..).unwrap_or("");
    let key = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
    key > 0 && rest.get(key..).is_some_and(|r| r.starts_with(close))
}

/// The crate map as AGENTS.md shows it.
pub fn crate_map(table: &Table) -> String {
    let width = table.entries.iter().map(|e| e.package.len()).max().unwrap_or(0);
    let mut s = String::from("```text\n");
    for e in table.entries {
        s.push_str(&format!(" {:<4} {:<width$}  {}\n", e.kind, e.package, e.summary));
    }
    s.push_str("```\n");
    s
}

/// AGENTS.md's crate map against `map`: rewritten with `write`, else a problem when stale. An
/// AGENTS.md without the markers (or none at all) is an error when the map is `required` (the
/// table's), and nothing to hold otherwise (a project that keeps no generated map).
fn map_problem(map: &str, root: &Path, write: bool, required: bool) -> Result<Option<String>, String> {
    let agents = root.join("AGENTS.md");
    let text = match std::fs::read_to_string(&agents) {
        Ok(text) => text.replace("\r\n", "\n"),
        Err(_) if !required => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", agents.display())),
    };
    let Some(wanted) = with_map(&text, map) else {
        return if required { Err(format!("AGENTS.md has no crate map markers ({MAP_START} … {MAP_END})")) } else { Ok(None) };
    };
    if wanted == text {
        return Ok(None);
    }
    if !write {
        return Ok(Some("AGENTS.md's crate map is stale: run `cargo xtask layers --write`".into()));
    }
    std::fs::write(&agents, &wanted).map_err(|e| format!("{}: {e}", agents.display()))?;
    println!("layers: rewrote the crate map in AGENTS.md");
    Ok(None)
}

/// `text` with `map` between the markers; `None` without them.
fn with_map(text: &str, map: &str) -> Option<String> {
    let a = text.find(MAP_START)?;
    let b = text.get(a..)?.find(MAP_END)? + a;
    Some(format!("{}{MAP_START}\n{map}{}", text.get(..a)?, text.get(b..)?))
}

/// Tracked `.rs`, `.toml` and `.md` files holding a raw NUL byte, as `path:lines`.
fn raw_nuls(root: &Path) -> Vec<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(root).args(["ls-files", "-z", "--", "*.rs", "*.toml", "*.md"]);
    let listed = match crate::process::output(cmd, None, GIT_DEADLINE, "git ls-files") {
        Ok(l) => l,
        Err(e) => return vec![format!("cannot list the tracked files to look for raw NUL bytes: {e}")],
    };
    listed
        .split('\0')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let lines = nul_lines(&std::fs::read(root.join(p)).ok()?);
            let at = lines.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
            (!lines.is_empty()).then(|| {
                format!("{p}:{at}: a raw NUL byte makes git merge the file as binary, dropping one side without conflict markers: write it `\\u{{0}}`")
            })
        })
        .collect()
}

/// The lines (1-based) of `bytes` holding a NUL.
fn nul_lines(bytes: &[u8]) -> Vec<usize> {
    bytes.split(|&b| b == b'\n').enumerate().filter(|(_, l)| l.contains(&0)).map(|(k, _)| k + 1).collect()
}

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(rust_files(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests;
