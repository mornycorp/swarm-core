//! A workspace's manifests as the checks read them: the members (the root's `[workspace] members`
//! and `exclude` as globs, and every path dependency inside the workspace, which cargo makes a
//! member listed or not), each member's package name, dependencies and tables. Parsed with
//! `toml` and never built or resolved: nothing here runs `cargo`, so it is fast and runs on a
//! fixture in a test. Every manifest is hostile input: one that can't be read or parsed is a
//! finding.

use std::path::Path;

use toml::{Table, Value};

use super::{Finding, glob, read};

/// The most files [`files`] lists: a workspace is nowhere near it, a runaway directory is.
const MAX_FILES: usize = 200_000;
/// How deep [`files`] goes.
const MAX_DEPTH: usize = 24;
/// Directories no check reads: git's, cargo's output and package caches.
const SKIPPED_DIRS: &[&str] = &[".git", "target", "node_modules"];

/// Every file of the workspace at `root` (relative, `/`-separated, sorted): in a git repository,
/// the files git tracks and the untracked ones it doesn't ignore, so a build's output, another
/// worktree and a package cache are never read; outside one (a fixture, an unpacked archive), the
/// tree, by [`walk`]. Only regular files: a symlink is never followed. A listing git can't give,
/// a directory that can't be read and a tree over the limits are findings.
pub fn files(root: &Path) -> (Vec<String>, Vec<Finding>) {
    let failed = |why: String| (Vec::new(), vec![Finding::new("", None, "unreadable", why, "run the check where `git ls-files` works")]);
    match crate::process::git_code(root, &["rev-parse", "--is-inside-work-tree"], crate::process::GIT_DEADLINE) {
        Ok(0) => {}
        // No repository here (git says so with 128): the tree is what there is.
        Ok(_) => return walk(root),
        Err(e) => return failed(format!("cannot ask git whether this is a repository: {e}")),
    }
    let listed = match crate::process::git_bytes(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--"], None, &[]) {
        Ok(listed) => listed,
        Err(e) => return failed(format!("cannot list the repository's files: {e}")),
    };
    let mut found: Vec<String> = listed
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .filter(|name| std::fs::symlink_metadata(root.join(name)).is_ok_and(|m| m.is_file()))
        .collect();
    found.sort();
    found.dedup();
    if found.len() > MAX_FILES {
        return failed(format!("more than {MAX_FILES} files"));
    }
    (found, Vec::new())
}

/// Every file under `root`, skipping [`SKIPPED_DIRS`] and never following a symlink.
fn walk(root: &Path) -> (Vec<String>, Vec<Finding>) {
    let (mut found, mut problems) = (Vec::new(), Vec::new());
    let mut pending = vec![(String::new(), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        let path = if dir.is_empty() { root.to_path_buf() } else { root.join(&dir) };
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) => {
                problems.push(Finding::new(&dir, None, "unreadable", format!("cannot list the directory: {e}"), "make it readable"));
                continue;
            }
        };
        for entry in entries.flatten() {
            let (name, kind) = (entry.file_name().to_string_lossy().into_owned(), entry.file_type());
            let shown = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
            match kind {
                Ok(t) if t.is_dir() && depth < MAX_DEPTH && !SKIPPED_DIRS.contains(&name.as_str()) => pending.push((shown, depth + 1)),
                Ok(t) if t.is_file() => found.push(shown),
                _ => {}
            }
        }
        if found.len() > MAX_FILES {
            problems.push(Finding::new("", None, "unreadable", format!("more than {MAX_FILES} files"), "point the check at the workspace root"));
            break;
        }
    }
    found.sort();
    (found, problems)
}

/// One workspace member.
pub struct Member {
    pub name: String,
    /// Its directory from the root (`""` for a root package), with `/`.
    pub dir: String,
    /// Its manifest from the root.
    pub manifest: String,
    pub text: String,
    pub table: Table,
}

/// Which table a dependency is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Normal,
    Dev,
    Build,
}

impl Kind {
    /// The manifest table's name.
    pub fn section(self) -> &'static str {
        match self {
            Kind::Normal => "dependencies",
            Kind::Dev => "dev-dependencies",
            Kind::Build => "build-dependencies",
        }
    }

    /// Every name cargo reads the table under: the old editions' `dev_dependencies` and
    /// `build_dependencies` too.
    pub fn sections(self) -> &'static [&'static str] {
        match self {
            Kind::Normal => &["dependencies"],
            Kind::Dev => &["dev-dependencies", "dev_dependencies"],
            Kind::Build => &["build-dependencies", "build_dependencies"],
        }
    }
}

/// One dependency of a member.
pub struct Dep {
    /// The key it is written under.
    pub key: String,
    /// The package it names (the key, or its `package = "…"` rename).
    pub package: String,
    pub kind: Kind,
    /// The table it is in, as the manifest spells it (`build_dependencies`).
    pub section: &'static str,
    /// Its `path`, joined to the member's directory (`None`: not a path dependency, or one that
    /// leaves the workspace); a `workspace = true` one takes the workspace's.
    pub path: Option<String>,
}

/// The workspace's members, read from `root`'s manifest and `files`, and the root's own table.
/// Findings: no manifest, no `[workspace]`, a manifest that doesn't parse or has no package name.
pub fn members(root: &Path, files: &[String]) -> (Vec<Member>, Table, Vec<Finding>) {
    let mut problems = Vec::new();
    let Some(top) = load(root, "Cargo.toml", &mut problems) else { return (Vec::new(), Table::new(), problems) };
    let Some(workspace) = top.1.get("workspace").and_then(Value::as_table) else {
        let fix = "add a `[workspace]` with `members` to the root manifest";
        problems.push(Finding::new("Cargo.toml", None, "workspace", "the root manifest has no [workspace]", fix));
        return (Vec::new(), top.1, problems);
    };
    let globs = |key: &str| -> Vec<String> {
        workspace.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
    };
    let (include, exclude) = (globs("members"), globs("exclude"));
    let excluded = |dir: &str| exclude.iter().any(|g| glob::matches(g, dir) || dir.starts_with(&format!("{}/", g.trim_end_matches('/'))));
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for file in files.iter().filter(|f| f.as_str() == "Cargo.toml" || f.ends_with("/Cargo.toml")) {
        let dir = file.strip_suffix("Cargo.toml").unwrap_or("").trim_end_matches('/').to_owned();
        let root_package = dir.is_empty() && top.1.contains_key("package");
        let listed = include.iter().any(|g| glob::matches(g, &dir)) && !excluded(&dir);
        if !(root_package || (listed && !dir.is_empty())) {
            continue;
        }
        seen.insert(dir.clone());
        let loaded = if dir.is_empty() { Some(top.clone()) } else { load(root, file, &mut problems) };
        found.extend(loaded.and_then(|(text, table)| member(file, &dir, text, table, &mut problems)));
    }
    // Cargo makes a member of every path dependency inside the workspace, listed or not: follow
    // them from the members, transitively.
    let workspace_deps = workspace.get("dependencies").and_then(Value::as_table);
    let mut todo: Vec<String> = found.iter().flat_map(|m| m.deps(workspace_deps)).filter_map(|d| d.path).collect();
    while let Some(dir) = todo.pop() {
        let file = format!("{dir}/Cargo.toml");
        if dir.is_empty() || excluded(&dir) || files.binary_search(&file).is_err() || !seen.insert(dir.clone()) {
            continue;
        }
        if let Some(m) = load(root, &file, &mut problems).and_then(|(text, table)| member(&file, &dir, text, table, &mut problems)) {
            todo.extend(m.deps(workspace_deps).into_iter().filter_map(|d| d.path));
            found.push(m);
        }
    }
    (found, top.1, problems)
}

/// Read and parse the manifest at `shown`, or say why not.
fn load(root: &Path, shown: &str, problems: &mut Vec<Finding>) -> Option<(String, Table)> {
    let text = match read(root, shown) {
        Ok(text) => text,
        Err(finding) => {
            problems.push(finding);
            return None;
        }
    };
    match text.parse::<Table>() {
        Ok(table) => Some((text, table)),
        Err(e) => {
            let line = e.span().map(|s| text.get(..s.start).unwrap_or("").matches('\n').count() + 1);
            problems.push(Finding::new(shown, line, "manifest", format!("is not valid TOML: {}", e.message()), "fix the manifest"));
            None
        }
    }
}

/// The member whose manifest is `table` at `file`, or the finding that it has no package name.
fn member(file: &str, dir: &str, text: String, table: Table, problems: &mut Vec<Finding>) -> Option<Member> {
    let name = table.get("package").and_then(Value::as_table).and_then(|p| p.get("name")).and_then(Value::as_str);
    let Some(name) = name else {
        problems.push(Finding::new(file, None, "manifest", "has no `[package] name`", "name the package, or leave the directory out of `[workspace] members`"));
        return None;
    };
    Some(Member { name: name.to_owned(), dir: dir.to_owned(), manifest: file.to_owned(), text, table })
}

impl Member {
    /// Every dependency this member's manifest declares, in all three tables and each
    /// `[target.….dependencies]`. A `workspace = true` one takes its rename (if any) from
    /// `workspace_deps`, the root's `[workspace.dependencies]`.
    pub fn deps(&self, workspace_deps: Option<&Table>) -> Vec<Dep> {
        let mut out = Vec::new();
        for (kind, section, deps) in dependency_tables(&self.table) {
            for (key, value) in deps {
                let package = package_of(key, value, workspace_deps);
                let inherited = value.as_table().and_then(|t| t.get("workspace")).and_then(Value::as_bool) == Some(true);
                let path = if inherited {
                    workspace_deps.and_then(|w| w.get(key)).and_then(path_of).and_then(|p| join("", p))
                } else {
                    path_of(value).and_then(|p| join(&self.dir, p))
                };
                out.push(Dep { key: key.clone(), package, kind, section, path });
            }
        }
        out
    }

    /// The line of dependency `dep` in the manifest, when it can be told.
    pub fn line_of_dep(&self, dep: &Dep) -> Option<usize> {
        line_of_key(&self.text, dep.section, &dep.key)
    }

    /// The `lints.workspace` flag.
    pub fn lints_workspace(&self) -> bool {
        self.table.get("lints").and_then(Value::as_table).and_then(|l| l.get("workspace")).and_then(Value::as_bool) == Some(true)
    }
}

/// Every dependency table of `manifest` (its own and each `[target.….dependencies]`), with its
/// kind and its name as spelt there.
pub fn dependency_tables(manifest: &Table) -> Vec<(Kind, &'static str, &Table)> {
    let mut tables: Vec<&Table> = vec![manifest];
    tables.extend(manifest.get("target").and_then(Value::as_table).into_iter().flat_map(Table::values).filter_map(Value::as_table));
    let mut out = Vec::new();
    for table in tables {
        for kind in [Kind::Normal, Kind::Dev, Kind::Build] {
            out.extend(kind.sections().iter().filter_map(|s| table.get(*s).and_then(Value::as_table).map(|deps| (kind, *s, deps))));
        }
    }
    out
}

/// A dependency's `path`, if it has one.
fn path_of(value: &Value) -> Option<&str> {
    value.as_table().and_then(|t| t.get("path")).and_then(Value::as_str)
}

/// `path` (relative, `/` or `\`) from directory `dir` (from the root, `""` for the root), as a
/// directory from the root with `/`; `None` when it is absolute or leaves the root.
pub fn join(dir: &str, path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    if path.starts_with('/') || path.contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in dir.split('/').chain(path.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The package a dependency `key = value` names.
fn package_of(key: &str, value: &Value, workspace_deps: Option<&Table>) -> String {
    let renamed = |v: &Value| v.as_table().and_then(|t| t.get("package")).and_then(Value::as_str).map(str::to_owned);
    if let Some(name) = renamed(value) {
        return name;
    }
    let inherited = value.as_table().and_then(|t| t.get("workspace")).and_then(Value::as_bool) == Some(true);
    inherited.then(|| workspace_deps.and_then(|w| w.get(key)).and_then(renamed)).flatten().unwrap_or_else(|| key.to_owned())
}

/// The 1-based line of `key` in `text`'s table `section` (a header ending in `section]`, or
/// `section.key]` for the table form), or of a table header for it.
pub fn line_of_key(text: &str, section: &str, key: &str) -> Option<usize> {
    let (mut header, dotted) = (String::new(), format!("{section}.{key}]"));
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('[') {
            line.clone_into(&mut header);
            if line.ends_with(&dotted) || line.ends_with(&format!("{section}.\"{key}\"]")) {
                return Some(i + 1);
            }
        } else if header.ends_with(&format!("{section}]")) && starts_with_key(line, key) {
            return Some(i + 1);
        }
    }
    None
}

/// Whether `line` is a `key = …` or `key.sub = …` line, the key bare or quoted.
fn starts_with_key(line: &str, key: &str) -> bool {
    let bare = line.strip_prefix(key).or_else(|| line.strip_prefix('"').and_then(|l| l.strip_prefix(key)).and_then(|l| l.strip_prefix('"')));
    bare.is_some_and(|rest| rest.trim_start().starts_with(['=', '.']))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In a repository the files are git's: what it tracks and what it would (untracked, not
    /// ignored), so a build's output inside the root is never read; outside one, the tree.
    #[test]
    fn in_a_repository_the_files_are_gits_and_outside_one_the_tree() {
        let dir = swarm_core_testkit::temp_dir("checks-files-git");
        for file in ["a/src/lib.rs", "out/built.rs", "worktrees/other/src/lib.rs"] {
            std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
            std::fs::write(dir.join(file), "").unwrap();
        }
        let (walked, problems) = files(&dir);
        assert_eq!((walked, problems.len()), (vec!["a/src/lib.rs".to_owned(), "out/built.rs".into(), "worktrees/other/src/lib.rs".into()], 0));
        swarm_core_testkit::git::git(&dir, &["init", "--quiet"]);
        std::fs::write(dir.join(".gitignore"), "out/\nworktrees/\n").unwrap();
        // Untracked and not ignored is read, as a new file is before its first commit.
        let (listed, problems) = files(&dir);
        assert_eq!((listed, problems.len()), (vec![".gitignore".to_owned(), "a/src/lib.rs".into()], 0));
        // Tracked and deleted is no file to read.
        swarm_core_testkit::git::git(&dir, &["add", "a/src/lib.rs"]);
        std::fs::remove_file(dir.join("a/src/lib.rs")).unwrap();
        assert_eq!(files(&dir).0, [".gitignore"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn table(text: &str) -> Table {
        text.parse().unwrap()
    }

    fn member_of(text: &str) -> Member {
        Member { name: "a".into(), dir: "a".into(), manifest: "a/Cargo.toml".into(), text: text.into(), table: table(text) }
    }

    #[test]
    fn every_dependency_table_and_form_is_read() {
        let m = member_of(
            "[package]\nname = \"a\"\n[dependencies]\nb = \"1\"\nc = { path = \"../c\" }\nd = { package = \"real-d\", version = \"1\" }\ne = { workspace = true }\n\
             [dev-dependencies]\nf = \"1\"\n[build-dependencies.g]\nversion = \"1\"\n[target.'cfg(unix)'.dependencies]\nh = \"1\"\n[target.'cfg(unix)'.dev-dependencies]\ni = \"1\"\n",
        );
        let ws = table("[x]\ne = { package = \"real-e\", version = \"1\" }\n");
        let deps = m.deps(ws.get("x").and_then(Value::as_table));
        let seen: Vec<(String, Kind)> = deps.iter().map(|d| (d.package.clone(), d.kind)).collect();
        for want in [
            ("b", Kind::Normal),
            ("c", Kind::Normal),
            ("real-d", Kind::Normal),
            ("real-e", Kind::Normal),
            ("f", Kind::Dev),
            ("g", Kind::Build),
            ("h", Kind::Normal),
            ("i", Kind::Dev),
        ] {
            assert!(seen.contains(&(want.0.to_owned(), want.1)), "{want:?} in {seen:?}");
        }
        assert_eq!(seen.len(), 8);
        assert!(member_of("[lints]\nworkspace = true\n").lints_workspace());
        assert!(!member_of("[lints.clippy]\nx = 1\n").lints_workspace());
    }

    #[test]
    fn the_old_underscore_tables_are_read_too() {
        let m = member_of("[package]\nname = \"a\"\n[build_dependencies]\nb = { path = \"../b\" }\n[dev_dependencies]\nc = \"1\"\n");
        let deps = m.deps(None);
        let seen: Vec<(&str, Kind, &str)> = deps.iter().map(|d| (d.package.as_str(), d.kind, d.section)).collect();
        assert_eq!(seen, [("c", Kind::Dev, "dev_dependencies"), ("b", Kind::Build, "build_dependencies")]);
        assert_eq!(deps.iter().map(|d| m.line_of_dep(d)).collect::<Vec<_>>(), [Some(6), Some(4)]);
        assert_eq!(deps.get(1).and_then(|d| d.path.as_deref()), Some("b"));
    }

    #[test]
    fn a_path_is_joined_inside_the_root_or_not_at_all() {
        assert_eq!(join("apps/cli", "../../crates/model").as_deref(), Some("crates/model"));
        assert_eq!(join("a", ".\\b\\.\\c/").as_deref(), Some("a/b/c"));
        assert_eq!(join("", "./src/core.rs").as_deref(), Some("src/core.rs"));
        assert_eq!(join("a", "../../outside"), None);
        assert_eq!(join("a", "/abs"), None);
        assert_eq!(join("a", "C:/abs"), None);
    }

    #[test]
    fn a_path_dependency_inside_the_workspace_is_a_member_listed_or_not() {
        let dir = swarm_core_testkit::temp_dir("checks-members-implicit");
        let write = |p: &str, t: &str| {
            let path = dir.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, t).unwrap();
        };
        write("Cargo.toml", "[workspace]\nmembers = [\"apps/*\"]\nexclude = [\"vendor\"]\n[workspace.dependencies]\nshared = { path = \"crates/shared\" }\n");
        write(
            "apps/cli/Cargo.toml",
            "[package]\nname = \"cli\"\n[dependencies]\nmodel = { path = \"../../crates/model\" }\nshared = { workspace = true }\nv = { path = \"../../vendor/v\" }\nout = { path = \"../../../outside\" }\n",
        );
        write("crates/model/Cargo.toml", "[package]\nname = \"model\"\n[dev-dependencies]\ndeep = { path = \"../deep\" }\n");
        write("crates/deep/Cargo.toml", "[package]\nname = \"deep\"\n[dependencies]\nmodel = { path = \"../model\" }\n");
        write("crates/shared/Cargo.toml", "[package]\nname = \"shared\"\n");
        write("vendor/v/Cargo.toml", "[package]\nname = \"v\"\n");
        let (files, _) = files(&dir);
        let (found, _, problems) = members(&dir, &files);
        let mut names: Vec<&str> = found.iter().map(|m| m.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["cli", "deep", "model", "shared"], "transitively, the workspace's own too; not an excluded or outside one");
        assert!(problems.is_empty(), "{problems:?}");
        assert!(found.iter().any(|m| m.name == "deep" && m.dir == "crates/deep" && m.manifest == "crates/deep/Cargo.toml"));
    }

    #[test]
    fn a_dependencys_line_is_found_in_its_table() {
        let text = "[package]\nname = \"a\"\n\n[dependencies]\nserde = \"1\"\negui = { version = \"1\" }\n\n[dev-dependencies]\negui = \"1\"\n\n[dependencies.winit]\nversion = \"1\"\n";
        let m = member_of(text);
        let dep = |key: &str, kind: Kind| Dep { key: key.into(), package: key.into(), kind, section: kind.section(), path: None };
        assert_eq!(m.line_of_dep(&dep("egui", Kind::Normal)), Some(6));
        assert_eq!(m.line_of_dep(&dep("egui", Kind::Dev)), Some(9));
        assert_eq!(m.line_of_dep(&dep("winit", Kind::Normal)), Some(11));
        assert_eq!(m.line_of_dep(&dep("nope", Kind::Normal)), None);
        assert_eq!(line_of_key("[package.metadata.swarm]\nlayer = \"ui\"\n", "package.metadata.swarm", "layer"), Some(2));
    }

    #[test]
    fn members_come_from_the_workspace_globs() {
        let dir = swarm_core_testkit::temp_dir("checks-members");
        let write = |p: &str, t: &str| {
            let path = dir.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, t).unwrap();
        };
        write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\", \"xtask\"]\nexclude = [\"crates/old\"]\n");
        write("crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
        write("crates/old/Cargo.toml", "[package]\nname = \"old\"\n");
        write("crates/nameless/Cargo.toml", "[lib]\n");
        write("crates/broken/Cargo.toml", "[package\n");
        write("xtask/Cargo.toml", "[package]\nname = \"xtask\"\n");
        write("other/Cargo.toml", "[package]\nname = \"not-a-member\"\n");
        write("target/x/Cargo.toml", "[package]\nname = \"built\"\n");
        let (files, listing) = files(&dir);
        assert!(listing.is_empty() && !files.iter().any(|f| f.starts_with("target/")), "{files:?}");
        let (found, _, problems) = members(&dir, &files);
        let mut names: Vec<&str> = found.iter().map(|m| m.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "xtask"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.file == "crates/nameless/Cargo.toml" && p.problem.contains("no `[package] name`")));
        assert!(problems.iter().any(|p| p.file == "crates/broken/Cargo.toml" && p.line.is_some() && p.problem.contains("not valid TOML")));
        let (none, _, problems) = members(&dir.join("nowhere"), &[]);
        assert!(none.is_empty() && problems.iter().any(|p| p.problem.contains("cannot read it")));
    }

    #[test]
    fn a_root_without_a_workspace_is_a_finding_and_a_root_package_is_a_member() {
        let dir = swarm_core_testkit::temp_dir("checks-members-root");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"solo\"\n").unwrap();
        let (_, _, problems) = members(&dir, &["Cargo.toml".to_owned()]);
        assert!(problems.iter().any(|p| p.rule == "workspace"), "{problems:?}");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"root\"\n[workspace]\nmembers = []\n").unwrap();
        let (found, _, problems) = members(&dir, &["Cargo.toml".to_owned()]);
        assert!(problems.is_empty() && found.len() == 1 && found.first().is_some_and(|m| m.name == "root" && m.dir.is_empty()));
    }
}
