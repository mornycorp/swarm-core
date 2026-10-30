//! The reads outside a package that its sources make, found by
//! scanning them: the paths `include_str!` and `include_bytes!` name (relative to the file), and
//! the paths joined to the package's directory, `CARGO_MANIFEST_DIR` (relative to the package).
//! A helper function whose body is such a join (`fn docs() -> PathBuf { …join("../../docs") }`)
//! is followed to its calls: `docs().join("x.md")` reads `docs/x.md`, and a call joined to
//! something other than a literal reads the whole directory. `ci --changed` must test the reader
//! when one of those files changes: [`super::missed`] lists the reads a plan leaves out, for each
//! consumer's test (`every_read_outside_a_package_tests_its_reader`). The patterns are put together
//! at run time, so this file's text holds none of them.
//!
//! It is a scan, not a parser: it doesn't see a path built from a relative literal (the cwd of a
//! test or build script is its package: `"../../assets/icon/app.ico"`) or through `crate::root()`
//! in xtask. Those go in the project's reads ([`super::Rules::reads`]) and
//! [`crate::Hooks::xtask_reads`] by hand.

use std::path::Path;

/// A read outside a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The package that reads it.
    pub reader: String,
    /// What it reads, relative to the workspace root (`/` separators): a file or a directory; `""`
    /// is the root itself, and `None` a path outside it.
    pub target: Option<String>,
    /// Where: the source file, relative to the root.
    pub at: String,
    /// The package's code reads it (not only its tests): `src/` or the build script, before the
    /// file's first `#[cfg(test)]`.
    pub embeds: bool,
}

/// A path a source file names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// Where in the file (a byte offset).
    pub offset: usize,
    /// Relative to the file's directory (an embed), else to the package (a `CARGO_MANIFEST_DIR`
    /// join).
    pub from_file: bool,
    pub path: String,
}

/// The macros that embed a file named relative to the source file.
const EMBEDS: &[&str] = &["include_str", "include_bytes"];
/// The variable naming a package's directory.
const MANIFEST_DIR: &str = "CARGO_MANIFEST_DIR";
/// A package's directories that hold code, beside its build script.
const CODE_DIRS: &[&str] = &["src", "tests", "examples", "benches"];

/// The first string literal in `s`, if any.
fn first_literal(s: &str) -> Option<&str> {
    s.split_once('"').and_then(|(_, after)| after.split_once('"')).map(|(lit, _)| lit)
}

/// The paths `text` names, the embeds' first.
pub fn paths(text: &str) -> Vec<Named> {
    let mut out = Vec::new();
    for m in EMBEDS {
        let pat = format!("{m}!(");
        for (i, _) in text.match_indices(&pat) {
            let rest = text.get(i + pat.len()..).unwrap_or("").trim_start();
            if let Some((lit, _)) = rest.strip_prefix('"').and_then(|r| r.split_once('"')) {
                out.push(Named { offset: i, from_file: true, path: lit.to_owned() });
            }
        }
    }
    out.extend(manifest_dir_joins(text));
    out
}

/// The paths `text` joins to `CARGO_MANIFEST_DIR` (`env!` in code, `std::env::var` in a build
/// script), a helper's followed to its calls.
fn manifest_dir_joins(text: &str) -> Vec<Named> {
    let mut out = Vec::new();
    for pat in [format!("env!(\"{MANIFEST_DIR}\")"), format!("var(\"{MANIFEST_DIR}\")")] {
        for (i, _) in text.match_indices(&pat) {
            // The first literal of the rest of the expression: `.join("…")` or `concat!(…, "/…")`;
            // none (`.parent()`) is the package itself.
            let rest = text.get(i + pat.len()..).unwrap_or("");
            let expr = rest.split([';', '{', '}']).next().unwrap_or("");
            let path = first_literal(expr).unwrap_or("").trim_start_matches('/').to_owned();
            let ends_body = rest.get(expr.len()..).is_some_and(|r| r.starts_with('}'));
            let followed = helper(text, i).filter(|_| ends_body).map(|name| calls(text, name, &path)).filter(|c| !c.is_empty());
            out.extend(followed.unwrap_or_else(|| vec![Named { offset: i, from_file: false, path }]));
        }
    }
    out
}

/// The function whose body the join at `at` begins, if it is one: `fn name() -> … { <the join>`
/// (its caller checks that the join ends the body).
fn helper(text: &str, at: usize) -> Option<&str> {
    let before = text.get(..at)?;
    let f = before.rfind("fn ")?;
    let name: &str = before.get(f + 3..)?.split(|c: char| !(c.is_alphanumeric() || c == '_')).next()?;
    let (signature, body) = before.get(f..)?.split_once('{')?;
    // No statement before the join, no argument, and a result.
    let simple = !body.contains([';', '}']) && !body.contains("let ") && signature.contains(&format!("{name}()")) && signature.contains("->");
    (simple && !name.is_empty()).then_some(name)
}

/// The paths read through helper `name` (whose directory is `dir`, relative to the package): its
/// calls joined to a literal read that path, any other call reads `dir`.
fn calls(text: &str, name: &str, dir: &str) -> Vec<Named> {
    let call = format!("{name}()");
    let mut out = Vec::new();
    for (i, _) in text.match_indices(&call) {
        let prev = text.get(..i).and_then(|b| b.chars().last());
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') || text.get(..i).is_some_and(|b| b.ends_with("fn ")) {
            continue;
        }
        let rest = text.get(i + call.len()..).unwrap_or("");
        let joined = rest.strip_prefix(".join(\"").and_then(|r| r.split_once('"')).map(|(lit, _)| lit);
        let path = match joined {
            Some(lit) if dir.is_empty() => lit.to_owned(),
            Some(lit) => format!("{dir}/{lit}"),
            None => dir.to_owned(),
        };
        out.push(Named { offset: i, from_file: false, path });
    }
    out
}

/// `rel` resolved against `base` (both `/`-separated, relative to the root); `None` when it
/// leaves the root.
pub fn resolve(base: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// The `.rs` files under `dir`, recursively.
fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// The reads outside package `name` (in `dir`) that the source file at `at` makes.
fn reads_in(name: &str, dir: &str, at: &str, text: &str) -> Vec<Read> {
    let file_dir = at.rsplit_once('/').map_or("", |(d, _)| d);
    // The package's code: `src/` or the build script, up to the file's tests.
    let code = at.strip_prefix(dir).is_some_and(|rest| rest.starts_with("/src/") || rest == "/build.rs");
    let tests_from = text.find("#[cfg(test)]").unwrap_or(text.len());
    paths(text)
        .into_iter()
        .filter_map(|n| {
            let target = resolve(if n.from_file { file_dir } else { dir }, &n.path);
            let inside = target.as_deref().is_some_and(|t| t == dir || t.starts_with(&format!("{dir}/")));
            (!inside).then(|| Read { reader: name.to_owned(), target, at: at.to_owned(), embeds: code && n.offset < tests_from })
        })
        .collect()
}

/// Every read outside its package in the sources of `packages` (name, directory relative to
/// `root`).
pub fn scan(root: &Path, packages: &[(String, String)]) -> Vec<Read> {
    let mut out = Vec::new();
    for (name, dir) in packages.iter().filter(|(_, d)| !d.is_empty()) {
        let mut files = vec![root.join(dir).join("build.rs")];
        for sub in CODE_DIRS {
            sources(&root.join(dir).join(sub), &mut files);
        }
        for file in files {
            if let Ok(text) = std::fs::read_to_string(&file) {
                let at = file.strip_prefix(root).unwrap_or(&file).to_string_lossy().replace('\\', "/");
                out.extend(reads_in(name, dir, &at, &text));
            }
        }
    }
    out
}

/// The paths to plan for a read of `target`: a file itself; a directory's tracked files (`tracked`,
/// the repository's, relative to the root) and a new file in it (a change to an untracked or
/// ignored file never reaches a plan, and a new one does).
pub fn probes(root: &Path, target: &str, tracked: &[String]) -> Vec<String> {
    if !root.join(target).is_dir() {
        return vec![target.to_owned()];
    }
    let prefix = format!("{target}/");
    let mut out: Vec<String> = tracked.iter().filter(|f| f.starts_with(&prefix)).cloned().collect();
    out.push(format!("{prefix}a-new-file"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(text: &str) -> Vec<(bool, String)> {
        paths(text).into_iter().map(|n| (n.from_file, n.path)).collect()
    }

    #[test]
    fn embeds_and_manifest_dir_joins_are_found() {
        let (inc, env) = ("include_str!", format!("env!(\"{MANIFEST_DIR}\")"));
        let text = format!(
            "let a = {inc}( \"../../docs/a.md\");\nlet b = PathBuf::from({env})\n    .join(\"../../docs/b.md\");\nlet c = concat!({env}, \"/build.rs\");\nlet d = Path::new({env}).parent();\n}}\nfn f() {{ g(\"not a read\"); }}"
        );
        let want = [(true, "../../docs/a.md"), (false, "../../docs/b.md"), (false, "build.rs"), (false, "")];
        assert_eq!(named(&text), want.map(|(f, p)| (f, p.to_owned())));
        // A build script's `std::env::var`.
        let build = format!("let src = PathBuf::from(std::env::var(\"{MANIFEST_DIR}\")?).join(\"exports.def\");");
        assert_eq!(named(&build), [(false, "exports.def".to_owned())]);
    }

    #[test]
    fn a_helper_is_followed_to_its_calls() {
        let env = format!("env!(\"{MANIFEST_DIR}\")");
        let text = format!(
            "fn docs() -> PathBuf {{\n    PathBuf::from({env}).join(\"../../docs\")\n}}\nfn a() {{ read(docs().join(\"x.md\")); }}\nfn b() {{ for n in N {{ read(docs().join(n)); }} }}\nfn c() {{ my_docs(); }}"
        );
        assert_eq!(named(&text), [(false, "../../docs/x.md".to_owned()), (false, "../../docs".to_owned())]);
        // The workspace root, joined: what it joins.
        let text = format!("fn workspace() -> PathBuf {{ Path::new({env}).join(\"../..\") }}\nlet d = workspace().join(\"corpus\");");
        assert_eq!(named(&text), [(false, "../../corpus".to_owned())]);
        // A function with statements before the join, or arguments, is no helper.
        let text = format!("fn f(name: &str) -> PathBuf {{ PathBuf::from({env}).join(\"../../docs\").join(name) }}\nf(\"x\");");
        assert_eq!(named(&text), [(false, "../../docs".to_owned())]);
    }

    #[test]
    fn paths_resolve_within_the_root_or_not_at_all() {
        assert_eq!(resolve("crates/model/tests", "../../../docs/x.md").as_deref(), Some("docs/x.md"));
        assert_eq!(resolve("crates/model", "./tests/../src"), Some("crates/model/src".to_owned()));
        assert_eq!(resolve("crates/model", "../.."), Some(String::new()));
        assert_eq!(resolve("crates/model", "../../../etc"), None);
    }

    #[test]
    fn a_scan_keeps_only_the_reads_that_leave_the_package_and_tells_code_from_tests() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-reads-scan");
        std::fs::create_dir_all(dir.join("crates/a/tests")).unwrap();
        std::fs::create_dir_all(dir.join("crates/a/src")).unwrap();
        let (inc, env) = ("include_bytes!", format!("env!(\"{MANIFEST_DIR}\")"));
        let test = format!("{inc}(\"../../../assets/x.bin\"); {inc}(\"data.bin\");\nlet p = Path::new({env}).join(\"tests/golden\");");
        std::fs::write(dir.join("crates/a/tests/t.rs"), test).unwrap();
        let lib = format!("const F: &[u8] = {inc}(\"../../../assets/f.ttf\");\n#[cfg(test)]\nmod tests {{ const D: &str = {inc}(\"../../../docs/d.md\"); }}");
        std::fs::write(dir.join("crates/a/src/lib.rs"), lib).unwrap();
        std::fs::write(dir.join("crates/a/build.rs"), format!("let p = Path::new({env}).join(\"../b/index.html\");")).unwrap();
        let mut found = scan(&dir, &[("a".to_owned(), "crates/a".to_owned())]);
        found.sort_by(|x, y| x.target.cmp(&y.target));
        let got: Vec<(&str, &str, bool)> = found.iter().map(|r| (r.target.as_deref().unwrap_or("?"), r.at.as_str(), r.embeds)).collect();
        assert_eq!(
            got,
            [
                ("assets/f.ttf", "crates/a/src/lib.rs", true),
                ("assets/x.bin", "crates/a/tests/t.rs", false),
                ("crates/b/index.html", "crates/a/build.rs", true),
                ("docs/d.md", "crates/a/src/lib.rs", false),
            ]
        );
        assert!(found.iter().all(|r| r.reader == "a"));
        std::fs::create_dir_all(dir.join("assets/icons")).unwrap();
        let tracked = ["assets/icons/i.png".to_owned(), "docs/d.md".to_owned()];
        assert_eq!(probes(&dir, "assets", &tracked), ["assets/icons/i.png", "assets/a-new-file"], "a directory: its tracked files and a new one");
        assert_eq!(probes(&dir, "assets/none.png", &tracked), ["assets/none.png"]);
    }
}
