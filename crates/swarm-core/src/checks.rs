//! The architecture's checks , held once here and configured by each
//! app, so a consumer's `cargo xtask layers` is a few lines and keeps only its own rules. The rules
//! are the ones of the pinned contract, `docs/app-architecture.md`; each names its check there.
//!
//! - [`layers`] reads the workspace's manifests and crate roots (no `cargo metadata`, no build):
//!   every member declares its layer, dependencies point to strictly lower layers, a UI toolkit is
//!   linked only by the UI layer and the apps built on it, the testkit is only a dev-dependency,
//!   `[lints] workspace = true`, the crate roots' headers, no committed `[patch]` of the
//!   Swarm Core. Configured by [`Layers`].
//! - [`scan`] reads the sources as text: patterns a layer may not use, in production code (outside
//!   the `#[cfg(test)]` modules, without comments, and without string literals where a pattern could match
//!   inside one), each file held to an allow-list whose every entry gives a reason and a count (a
//!   ratchet: more fails, fewer asks to lower it). Configured by [`Scan`]s, whose built-in ones
//!   the app turns on and points at its crates' files with globs.
//! - **No raw NUL bytes** in a tracked `.rs`, `.toml` or `.md` file: git takes one for binary and
//!   merges it by keeping one side whole. Nothing configures it.
//! - [`check`] runs them all, `layers` and `scan` as [`Hooks::architecture`] and [`Hooks::scans`]
//!   configure them: a project that sets neither has only its NUL bytes looked for.
//!
//! A consumer's `cargo xtask layers` is [`crate::layers::run`], which runs these beside the table
//! of [`Hooks::layers`] (a project describes its layers one way or the other), its crate map and
//! its code-health exemptions. A project that wants only these calls [`check`]:
//!
//! ```ignore
//! match swarm_core::checks::check(&Ours, &root()) {
//!     Ok(report) => println!("{report}"),
//!     Err(findings) => { eprintln!("{findings}"); return Err(findings.summary()); }
//! }
//! ```
//!
//! **The files read** are the repository's: what git tracks and the untracked files it doesn't
//! ignore, so a build's output or another worktree inside the root is never read; outside a
//! repository, the tree.
//!
//! Every finding names the file, the line (where the check knows it), the rule and the fix
//! ([`Finding`]). The files read are the repo's own, so they are hostile input: a manifest or a
//! source that can't be read, parsed or is over [`MAX_FILE`] is a finding, never a panic.

use std::fmt;
use std::path::Path;

use crate::Hooks;

mod glob;
mod headers;
mod layers;
mod manifest;
mod nul_bytes;
mod patterns;
mod scan;
mod source;

pub use layers::{Layers, LayersReport, UNSAFE_LINES, Unsafe, layers};
pub(crate) use manifest::dependency_tables;
pub use scan::{Allowed, Pattern, Region, Scan, ScanReport, Strings, TEST_FILES, scan};

/// The largest file a check reads: a larger one is a finding (nothing in a repo's sources or
/// manifests is anywhere near it).
pub const MAX_FILE: u64 = 4 * 1024 * 1024;

/// One thing a check found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The file, from the workspace root, with `/`.
    pub file: String,
    /// The line (1-based) when the check knows it.
    pub line: Option<usize>,
    /// The rule broken: `layer-order`, `value-reads`, …
    pub rule: &'static str,
    /// What is wrong.
    pub problem: String,
    /// What to do about it.
    pub fix: String,
}

impl Finding {
    pub(crate) fn new(file: &str, line: Option<usize>, rule: &'static str, problem: impl Into<String>, fix: impl Into<String>) -> Finding {
        Finding { file: file.to_owned(), line, rule, problem: problem.into(), fix: fix.into() }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(n) => write!(f, "{}:{n}", self.file)?,
            None => write!(f, "{}", self.file)?,
        }
        write!(f, ": [{}] {}. Fix: {}", self.rule, self.problem, self.fix)
    }
}

/// What a failed check found: at least one [`Finding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Findings(pub Vec<Finding>);

impl Findings {
    /// A one-line message for a command's `Err`, once the findings themselves are printed.
    pub fn summary(&self) -> String {
        format!("{} architecture problem(s)", self.0.len())
    }
}

impl fmt::Display for Findings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} architecture problem(s):", self.0.len())?;
        self.0.iter().try_for_each(|finding| write!(f, "\n  - {finding}"))
    }
}

impl std::error::Error for Findings {}

/// What [`check`] ran: each check's report, printed as two summary lines. `layers` is `None` for
/// a project that declares no layers in its manifests ([`Hooks::architecture`]).
#[derive(Debug, Clone)]
pub struct Report {
    pub layers: Option<LayersReport>,
    pub scan: ScanReport,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.layers {
            Some(layers) => write!(f, "{layers}\n{}", self.scan),
            None => write!(f, "{}", self.scan),
        }
    }
}

/// Every check, `layers` and `scan` as `hooks` configures them ([`Hooks::architecture`],
/// [`Hooks::scans`]), and the NUL bytes: every finding of all of them, or the report.
pub fn check(hooks: &dyn Hooks, root: &Path) -> Result<Report, Findings> {
    check_with(hooks, root, true)
}

/// Whether `hooks` configures a check of this module: [`crate::layers::run`] runs them then.
pub(crate) fn configured(hooks: &dyn Hooks) -> bool {
    hooks.architecture().is_some() || !hooks.scans().is_empty()
}

/// The layers `hooks` declares, with the packages it allows `unsafe`: those of its
/// [`Layers::unsafe_crates`], else those of [`Hooks::unsafe_crates`], so a project names them once.
fn declared(hooks: &dyn Hooks) -> Option<Layers> {
    let layers = hooks.architecture()?;
    let unsafe_crates = if layers.unsafe_crates.is_empty() { hooks.unsafe_crates() } else { layers.unsafe_crates };
    Some(Layers { unsafe_crates, ..layers })
}

/// [`check`], looking for NUL bytes only with `nul` ([`crate::layers::run`] has its own look).
pub(crate) fn check_with(hooks: &dyn Hooks, root: &Path, nul: bool) -> Result<Report, Findings> {
    let scans = hooks.scans();
    let (files, listing) = manifest::files(root);
    let layers = match declared(hooks) {
        Some(config) => layers::layers_files(root, &config, &files, listing).map(Some),
        None if listing.is_empty() => Ok(None),
        None => Err(Findings(listing)),
    };
    let scans = scan::scan_files(root, &scans, &files, Vec::new());
    let nul = if nul { nul_bytes::nul_bytes(root, &files) } else { Vec::new() };
    match (layers, scans) {
        (Ok(layers), Ok(scan)) if nul.is_empty() => Ok(Report { layers, scan }),
        (a, b) => Err(Findings(a.err().into_iter().chain(b.err()).flat_map(|f| f.0).chain(nul).collect())),
    }
}

/// A workspace member as its manifest declares it, for [`crate::layers::run`]'s crate map and
/// its count of code-health exemptions.
pub(crate) struct Declared {
    pub package: String,
    /// Its directory from the root (`""` for a root package).
    pub dir: String,
    /// `[package.metadata.swarm] layer`, when it says one.
    pub layer: String,
    /// `[package] description`, when it has one.
    pub description: String,
}

/// The members of the workspace at `root`, in package order. One whose manifest can't be read is
/// left out: [`check`] reports it.
pub(crate) fn declared_members(root: &Path) -> Vec<Declared> {
    let (files, _) = manifest::files(root);
    let (members, _, _) = manifest::members(root, &files);
    let text = |m: &manifest::Member, path: &[&str]| {
        let mut at = Some(&m.table);
        let (last, tables) = path.split_last().unwrap_or((&"", &[]));
        for key in tables {
            at = at.and_then(|t| t.get(*key)).and_then(toml::Value::as_table);
        }
        at.and_then(|t| t.get(*last)).and_then(toml::Value::as_str).unwrap_or_default().trim().to_owned()
    };
    let mut out: Vec<Declared> = members
        .iter()
        .map(|m| Declared {
            package: m.name.clone(),
            dir: m.dir.clone(),
            layer: text(m, &["package", "metadata", "swarm", "layer"]),
            description: text(m, &["package", "description"]),
        })
        .collect();
    out.sort_by(|a, b| a.package.cmp(&b.package));
    out
}

/// The crate map as AGENTS.md shows it, from the manifests: each member's layer, name and
/// description, in package order.
pub(crate) fn crate_map(members: &[Declared]) -> String {
    let layer = members.iter().map(|m| m.layer.len()).max().unwrap_or(0);
    let package = members.iter().map(|m| m.package.len()).max().unwrap_or(0);
    let mut s = String::from("```text\n");
    for m in members {
        s.push_str(format!(" {:<layer$} {:<package$}  {}", m.layer, m.package, m.description).trim_end());
        s.push('\n');
    }
    s.push_str("```\n");
    s
}

/// The committed overrides of Swarm Core's pin in the workspace at `root` (the part of
/// [`layers`] that [`crate::pin::check_committed`] runs alone), with the manifests that can't be read.
pub(crate) fn pin_findings(root: &Path) -> Vec<Finding> {
    let (files, mut found) = manifest::files(root);
    let (members, top, problems) = manifest::members(root, &files);
    found.extend(problems);
    found.extend(layers::swarm_core_pin(root, &top, &members));
    found
}

/// The file `path` as text, or the finding that says why not: unreadable, over [`MAX_FILE`].
/// Bytes that aren't UTF-8 are read lossily (a hostile file stays a file).
pub(crate) fn read(root: &Path, shown: &str) -> Result<String, Finding> {
    use std::io::Read;
    let path = root.join(shown);
    let read_err = |why: String| Finding::new(shown, None, "unreadable", why, "make the file readable text under 4 MiB, or exclude it from the check");
    // What is at the path is asked before it is opened: opening a FIFO waits for its writer, and
    // reading a device never ends. The read is capped too, so a file that grows after is no more.
    let meta = std::fs::metadata(&path).map_err(|e| read_err(format!("cannot read it: {e}")))?;
    if !meta.is_file() {
        return Err(read_err("it is not a regular file".to_owned()));
    }
    if meta.len() > MAX_FILE {
        return Err(read_err(format!("{} bytes is over the {MAX_FILE} the checks read", meta.len())));
    }
    let file = std::fs::File::open(&path).map_err(|e| read_err(format!("cannot read it: {e}")))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|e| read_err(format!("cannot read it: {e}")))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(read_err(format!("it is over the {MAX_FILE} bytes the checks read")));
    }
    Ok(String::from_utf8_lossy(&bytes).replace("\r\n", "\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finding_names_its_file_line_rule_and_fix() {
        let f = Finding::new("crates/a/Cargo.toml", Some(7), "layer-order", "a depends on b", "move it");
        assert_eq!(f.to_string(), "crates/a/Cargo.toml:7: [layer-order] a depends on b. Fix: move it");
        let g = Finding::new("Cargo.toml", None, "swarm-core-patch", "patched", "remove it");
        assert_eq!(g.to_string(), "Cargo.toml: [swarm-core-patch] patched. Fix: remove it");
        let all = Findings(vec![f, g]);
        assert!(all.to_string().starts_with("2 architecture problem(s):\n  - crates/a/Cargo.toml:7:"), "{all}");
        assert_eq!(all.summary(), "2 architecture problem(s)");
    }

    #[test]
    fn a_missing_or_oversized_file_is_a_finding_not_a_panic() {
        let dir = swarm_core_testkit::temp_dir("checks-read");
        let missing = read(&dir, "nope.rs").err().map(|f| f.problem);
        assert!(missing.is_some_and(|p| p.contains("cannot read it")));
        std::fs::write(dir.join("big.rs"), vec![b'a'; (MAX_FILE + 1) as usize]).unwrap();
        assert!(read(&dir, "big.rs").err().is_some_and(|f| f.problem.contains("over the")));
        std::fs::write(dir.join("crlf.rs"), b"a\r\nb\xff\n").unwrap();
        assert_eq!(read(&dir, "crlf.rs").ok().as_deref(), Some("a\nb\u{fffd}\n"));
    }

    /// What isn't a regular file is a finding before it is opened: a directory, and a FIFO, whose
    /// open would wait for a writer that never comes.
    #[test]
    fn what_is_not_a_regular_file_is_never_opened() {
        let dir = swarm_core_testkit::temp_dir("checks-read-kind");
        std::fs::create_dir_all(dir.join("Cargo.toml")).unwrap();
        assert!(read(&dir, "Cargo.toml").err().is_some_and(|f| f.problem.contains("not a regular file")));
        #[cfg(unix)]
        {
            let made = std::process::Command::new("mkfifo").arg(dir.join("pipe.rs")).status().is_ok_and(|s| s.success());
            if made {
                assert!(read(&dir, "pipe.rs").err().is_some_and(|f| f.problem.contains("not a regular file")));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The crate map from the manifests: each member's layer, name and description, by name.
    #[test]
    fn the_crate_map_is_rendered_from_the_manifests() {
        let dir = swarm_core_testkit::temp_dir("checks-crate-map");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"b\", \"a\"]\n").unwrap();
        for (name, layer, description) in [("a", "layer = \"engine\"", "description = \"the commands\""), ("b", "", "")] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("Cargo.toml"), format!("[package]\nname = \"my-{name}\"\n{description}\n[package.metadata.swarm]\n{layer}\n"))
                .unwrap();
        }
        let members = declared_members(&dir);
        assert_eq!(
            members.iter().map(|m| (m.package.as_str(), m.dir.as_str(), m.layer.as_str())).collect::<Vec<_>>(),
            [("my-a", "a", "engine"), ("my-b", "b", "")]
        );
        assert_eq!(crate_map(&members), "```text\n engine my-a  the commands\n        my-b\n```\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A one-member workspace; `lints` is its `[lints]` table.
    fn fixture(name: &str, lints: &str) -> std::path::PathBuf {
        let dir = swarm_core_testkit::temp_dir(name);
        let header = "#![forbid(unsafe_code)]\n#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]\n\
                      #![warn(clippy::cognitive_complexity, clippy::too_many_lines)]\n#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]\n";
        std::fs::create_dir_all(dir.join("a/src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"a\"]\n").unwrap();
        std::fs::write(dir.join("a/Cargo.toml"), format!("[package]\nname = \"a\"\n[package.metadata.swarm]\nlayer = \"ui\"\n{lints}")).unwrap();
        std::fs::write(dir.join("a/src/lib.rs"), format!("{header}fn f(v: Value) {{ v[\"k\"]; }}\n")).unwrap();
        dir
    }

    /// A project whose manifests declare the contract's layers.
    struct Standard;
    impl Hooks for Standard {
        fn architecture(&self) -> Option<Layers> {
            Some(Layers::STANDARD)
        }
    }

    struct Reads;
    impl Hooks for Reads {
        fn architecture(&self) -> Option<Layers> {
            Some(Layers::STANDARD)
        }
        fn scans(&self) -> Vec<Scan> {
            vec![Scan::value_reads().files(&["a/src/**"])]
        }
    }

    #[test]
    fn check_runs_both_checks_as_the_hooks_configure_them() {
        let ok = fixture("checks-hooks-ok", "[lints]\nworkspace = true\n");
        // The defaults: no layers read from the manifests, and no scan.
        let report = check(&crate::Defaults, &ok).unwrap();
        assert!(report.layers.is_none() && report.scan.files == 0 && !configured(&crate::Defaults));
        // The contract's layers, and no scan.
        let report = check(&Standard, &ok).unwrap();
        assert_eq!((report.layers.map(|l| l.members), report.scan.files), (Some(1), 0));
        // A manifest without `[lints]` is nothing to a project that declares no layers.
        assert!(check(&crate::Defaults, &fixture("checks-hooks-none", "")).is_ok());
        // A scan the hooks name finds the read; a manifest without `[lints]` is found beside it.
        let bad = fixture("checks-hooks-bad", "");
        let found = check(&Reads, &bad).err().unwrap();
        let rules: Vec<&str> = found.0.iter().map(|f| f.rule).collect();
        assert_eq!(rules, ["workspace-lints", "value-reads"], "{found}");
        assert!(check(&Reads, &ok).err().is_some_and(|f| f.0.len() == 1 && f.0[0].file == "a/src/lib.rs" && f.0[0].line == Some(5)));
        // A NUL byte fails a workspace both other checks pass, and is found beside their findings.
        std::fs::write(ok.join("a/README.md"), b"text\0\n").unwrap();
        assert!(check(&crate::Defaults, &ok).err().is_some_and(|f| f.0.len() == 1 && f.0[0].rule == "nul-byte"));
        assert!(check_with(&crate::Defaults, &ok, false).is_ok(), "layers::run looks for them itself");
        std::fs::write(bad.join("a/README.md"), b"\0").unwrap();
        let rules: Vec<&str> = check(&Reads, &bad).err().unwrap().0.iter().map(|f| f.rule).collect();
        assert_eq!(rules, ["workspace-lints", "value-reads", "nul-byte"]);
    }
}
