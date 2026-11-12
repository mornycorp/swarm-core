//! Which PR of a failed stack the failure points at ([`super`]), as pure functions over the gate's
//! log and what each PR changed.
//!
//! - **A build, clippy or fmt error names files** (rustc's `--> <file>:<line>:<col>` under an
//!   `error`, rustfmt's `Diff in <file>`): the PRs that changed one of them.
//! - **A failed test names its package and binary** (nextest's `FAIL […] <package>::<binary>
//!   <module>::<test>`; the golden binary's libtest `test <name> ... FAILED`): the PRs that changed
//!   the file the test lives in, read from its module path (`tests/<binary>/<module>.rs`,
//!   `src/<module>.rs`), the longest match first.
//! - **Else the packages** a failure names (`could not compile `<package>``, a failed test's):
//!   the PRs that changed a file in one of them.
//!
//! Each step that matches exactly one PR blames it; one that matches several stops there (the
//! daemon bisects); one that matches none hands over to the next.

use std::collections::BTreeSet;

/// What one PR of the stack changed, from where it was stacked to its stacked tip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touch {
    pub mr: u64,
    pub files: Vec<String>,
}

/// The workspace's packages: `(name, directory)`, the directory from the root (`""`: the root).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Packages(pub Vec<(String, String)>);

impl Packages {
    /// The directory of package `name`.
    pub fn dir(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, d)| d.as_str())
    }

    /// From each `(path of a Cargo.toml, its text)`: those with a `[package] name`.
    pub fn from_manifests(manifests: &[(String, String)]) -> Packages {
        let mut out = Vec::new();
        for (path, text) in manifests {
            let Ok(v) = text.parse::<toml::Table>() else { continue };
            let Some(name) = v.get("package").and_then(|p| p.get("name")).and_then(toml::Value::as_str) else { continue };
            let dir = path.strip_suffix("Cargo.toml").unwrap_or_default().trim_end_matches('/');
            out.push((name.to_owned(), dir.to_owned()));
        }
        Packages(out)
    }
}

/// A failed test: its binary as nextest names it (`<package>` for the library's unit tests,
/// `<package>::<binary>`), and its name (`module::tests::name`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedTest {
    pub binary: String,
    pub name: String,
}

impl FailedTest {
    fn package(&self) -> &str {
        self.binary.split("::").next().unwrap_or_default()
    }
}

/// What a failed gate's log names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Failure {
    /// Files an error names, from the root.
    pub files: Vec<String>,
    /// Packages that didn't compile.
    pub packages: Vec<String>,
    pub tests: Vec<FailedTest>,
}

/// What the gate's `log` names, its paths made relative to the gate's `root` (an absolute path
/// elsewhere, a dependency's in cargo's registry, is left out). `golden` is the project's golden
/// binary, whose libtest failures name no binary.
pub fn failure(log: &str, root: &str, golden: &str) -> Failure {
    let mut f = Failure::default();
    let mut level_error = false;
    for line in log.lines() {
        let t = line.trim_start();
        if t.starts_with("error") {
            level_error = true;
        } else if t.starts_with("warning") {
            level_error = false;
        }
        let file = match t.strip_prefix("--> ") {
            Some(at) if level_error => strip_position(at),
            _ => diff_in(t),
        };
        if let Some(p) = file.and_then(|p| relative(p, root)) {
            push_once(&mut f.files, p);
        }
        if let Some(pkg) = t.strip_prefix("error: could not compile `").and_then(|r| r.split_once('`')).map(|(p, _)| p.to_owned()) {
            push_once(&mut f.packages, pkg);
        }
    }
    f.tests = failed_tests(log, golden);
    f
}

fn push_once(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// `<file>:<line>:<col>` without the position.
fn strip_position(at: &str) -> Option<&str> {
    let at = at.trim();
    let mut parts = at.rsplitn(3, ':');
    let (col, line) = (parts.next()?, parts.next()?);
    let file = parts.next()?;
    (col.chars().all(|c| c.is_ascii_digit()) && line.chars().all(|c| c.is_ascii_digit())).then_some(file)
}

/// rustfmt's `Diff in <file> at line <n>:` or `Diff in <file>:<n>:`.
fn diff_in(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("Diff in ")?;
    if let Some((file, _)) = rest.split_once(" at line ") {
        return Some(file);
    }
    let rest = rest.strip_suffix(':')?;
    let (file, n) = rest.rsplit_once(':')?;
    n.chars().all(|c| c.is_ascii_digit()).then_some(file)
}

/// `path` from `root`, `/`-separated; `None` for a path outside it.
pub fn relative(path: &str, root: &str) -> Option<String> {
    let norm = |p: &str| p.trim().trim_start_matches(r"\\?\").replace('\\', "/");
    let (p, r) = (norm(path), norm(root).trim_end_matches('/').to_owned());
    let rel = if !r.is_empty() && p.len() > r.len() && p.get(..r.len()).is_some_and(|h| h.eq_ignore_ascii_case(&r)) {
        p.get(r.len()..)?.trim_start_matches('/').to_owned()
    } else {
        p.trim_start_matches("./").to_owned()
    };
    let absolute = rel.starts_with('/') || rel.get(1..2) == Some(":");
    (!rel.is_empty() && !absolute).then_some(rel)
}

/// nextest's failed tests (`FAIL [ 1.2s] (n/m) <binary> <name>`), then the golden binary's
/// libtest ones (`test <name> ... FAILED`), each once. A libtest line that names a test nextest
/// failed is that test's own output, which nextest prints under its `FAIL`: not the golden
/// binary's .
fn failed_tests(log: &str, golden: &str) -> Vec<FailedTest> {
    let mut out: Vec<FailedTest> = Vec::new();
    let nextest: Vec<(String, String)> =
        crate::gate::failed_tests(log).into_iter().filter_map(|t| t.split_once(' ').map(|(b, n)| (b.to_owned(), n.trim().to_owned()))).collect();
    let by_nextest = |n: &str| nextest.iter().any(|(_, name)| name == n);
    let libtest: Vec<(String, String)> = log
        .lines()
        .filter_map(|l| l.trim().strip_prefix("test ")?.strip_suffix(" ... FAILED").map(|n| (golden.to_owned(), n.trim().to_owned())))
        .filter(|(_, n)| !golden.is_empty() && !by_nextest(n))
        .collect();
    for (binary, name) in nextest.into_iter().chain(libtest) {
        let t = FailedTest { binary, name };
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Who a failure points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blame {
    /// Exactly this PR, and the evidence, said in words.
    One(u64, String),
    /// Several PRs match the strongest evidence.
    Several(Vec<u64>),
    /// Nothing the log names matches an PR's change.
    None,
}

/// Who `f` points at among `touched`, the workspace's `packages` placing the tests and the
/// packages' files: the files, then the tests' files, then the packages ([the module's
/// rules](self)).
pub fn blame(f: &Failure, touched: &[Touch], packages: &Packages) -> Blame {
    let by_files = matching(touched, |file| f.files.iter().any(|e| e == file));
    if let Some(b) = decide(by_files, || format!("it changed {}, where the build or a check failed", list(&f.files, 3))) {
        return b;
    }
    if let Some(b) = decide(by_test_files(f, touched, packages), || format!("it changed the file of a failed test ({})", test_list(&f.tests))) {
        return b;
    }
    let names: BTreeSet<&str> = f.packages.iter().map(String::as_str).chain(f.tests.iter().map(FailedTest::package)).collect();
    let dirs: Vec<&str> = names.iter().filter_map(|n| packages.dir(n)).filter(|d| !d.is_empty()).collect();
    let by_package = matching(touched, |file| dirs.iter().any(|d| in_dir(file, d)));
    decide(by_package, || format!("it changed the package {} that failed", names.iter().copied().collect::<Vec<_>>().join(", "))).unwrap_or(Blame::None)
}

/// One match, several, or none (`None`: try the next evidence).
fn decide(mrs: Vec<u64>, why: impl FnOnce() -> String) -> Option<Blame> {
    match mrs.as_slice() {
        [] => None,
        [one] => Some(Blame::One(*one, why())),
        _ => Some(Blame::Several(mrs)),
    }
}

/// The PRs one of whose files `hit` takes.
fn matching(touched: &[Touch], hit: impl Fn(&str) -> bool) -> Vec<u64> {
    touched.iter().filter(|t| t.files.iter().any(|f| hit(f))).map(|t| t.mr).collect()
}

fn in_dir(file: &str, dir: &str) -> bool {
    file.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

/// For each failed test, the PRs that changed the file it most likely lives in: the file whose
/// module path is the longest prefix of the test's.
fn by_test_files(f: &Failure, touched: &[Touch], packages: &Packages) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    for t in &f.tests {
        let Some(dir) = packages.dir(t.package()) else { continue };
        let path = test_path(t);
        let scored: Vec<(u64, usize)> =
            touched.iter().filter_map(|tc| tc.files.iter().filter_map(|file| file_score(file, dir, &path)).max().map(|s| (tc.mr, s))).collect();
        let Some(best) = scored.iter().map(|(_, s)| *s).max() else { continue };
        for (mr, _) in scored.into_iter().filter(|(_, s)| *s == best) {
            if !out.contains(&mr) {
                out.push(mr);
            }
        }
    }
    out
}

/// Where a test lives, as module segments: `tests` and its binary, then its modules, for an
/// integration test; `src` and its modules for a unit test. The test's own name is dropped.
fn test_path(t: &FailedTest) -> Vec<String> {
    let mut mods: Vec<String> = t.name.split("::").map(str::to_owned).collect();
    mods.pop();
    let (root, binary) = match t.binary.split_once("::") {
        Some((_, b)) if !b.starts_with("bin/") => ("tests", Some(b.to_owned())),
        _ => ("src", None),
    };
    std::iter::once(root.to_owned()).chain(binary).chain(mods).collect()
}

/// How much of the test's module `path` the file `file` (in package `dir`) covers: its own
/// module segments, when they are a prefix of it with more than the root.
fn file_score(file: &str, dir: &str, path: &[String]) -> Option<usize> {
    let rel = if dir.is_empty() { file } else { file.strip_prefix(dir)?.strip_prefix('/')? };
    let stem = rel.strip_suffix(".rs")?;
    let mut segs: Vec<&str> = stem.split('/').collect();
    if matches!(segs.last(), Some(&("mod" | "main" | "lib"))) {
        segs.pop();
    }
    let prefix = segs.len() > 1 && segs.len() <= path.len() && segs.iter().zip(path).all(|(a, b)| *a == b);
    prefix.then_some(segs.len())
}

fn list(items: &[String], max: usize) -> String {
    let shown: Vec<String> = items.iter().take(max).map(|i| format!("`{i}`")).collect();
    let more = items.len().saturating_sub(max);
    if more > 0 { format!("{} and {more} more", shown.join(", ")) } else { shown.join(", ") }
}

fn test_list(tests: &[FailedTest]) -> String {
    let names: Vec<String> = tests.iter().map(|t| format!("{} {}", t.binary, t.name)).collect();
    list(&names, 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = r"D:\dev\ns\.claude\worktrees\gate";

    fn packages() -> Packages {
        Packages::from_manifests(&[
            ("crates/engine/Cargo.toml".into(), "[package]\nname = \"my-engine\"\n".into()),
            ("crates/model/Cargo.toml".into(), "[package]\nname = \"my-model\"\n".into()),
            ("Cargo.toml".into(), "[workspace]\nmembers = []\n".into()),
            ("xtask/Cargo.toml".into(), "not toml [".into()),
        ])
    }

    fn touch(mr: u64, files: &[&str]) -> Touch {
        Touch { mr, files: files.iter().map(|f| (*f).to_owned()).collect() }
    }

    #[test]
    fn the_manifests_name_the_packages_and_their_directories() {
        let p = packages();
        assert_eq!(p.0.len(), 2);
        assert_eq!(p.dir("my-engine"), Some("crates/engine"));
        assert_eq!(p.dir("xtask"), None);
    }

    #[test]
    fn a_log_names_the_files_of_its_errors_and_its_failed_tests() {
        let log = format!(
            "warning: unused import\n  --> crates/model/src/warned.rs:1:5\nerror[E0308]: mismatched types\n  --> crates/engine/src/cmd.rs:12:9\n\
             error: this `if` has identical blocks\n   --> {ROOT}\\crates\\engine\\src\\lib.rs:3:1\n   --> C:\\Users\\x\\.cargo\\registry\\serde\\src\\de.rs:1:1\n\
             error: could not compile `my-engine` (lib) due to 2 previous errors\nDiff in {ROOT}\\crates\\model\\src\\fmt.rs at line 3:\n\
             Diff in /home/x/gate/crates/model/src/new.rs:7:\n        FAIL [   1.2s] (3/9) my-engine::it panic_hunt::every_command\n\
             test golden::renders ... FAILED\ntest golden::fine ... ok\n"
        );
        let f = failure(&log, ROOT, "my-engine::golden");
        assert_eq!(f.files, vec!["crates/engine/src/cmd.rs", "crates/engine/src/lib.rs", "crates/model/src/fmt.rs"]);
        assert_eq!(f.packages, vec!["my-engine"]);
        assert_eq!(
            f.tests,
            vec![
                FailedTest { binary: "my-engine::it".into(), name: "panic_hunt::every_command".into() },
                FailedTest { binary: "my-engine::golden".into(), name: "golden::renders".into() }
            ]
        );
        assert_eq!(relative("/home/x/gate/crates/a.rs", "/home/x/gate"), Some("crates/a.rs".into()));
        assert_eq!(relative("./crates/a.rs", "/r"), Some("crates/a.rs".into()));
        assert_eq!(relative("/elsewhere/a.rs", "/r"), None);
    }

    #[test]
    fn an_error_in_a_file_one_mr_changed_blames_it() {
        let f = Failure { files: vec!["crates/engine/src/cmd.rs".into()], packages: vec!["my-engine".into()], tests: vec![] };
        let touched = [touch(1, &["crates/engine/src/other.rs"]), touch(2, &["crates/engine/src/cmd.rs", "docs/a.md"])];
        assert!(matches!(blame(&f, &touched, &packages()), Blame::One(2, why) if why.contains("cmd.rs")));
        // Both changed it: several.
        let both = [touch(1, &["crates/engine/src/cmd.rs"]), touch(2, &["crates/engine/src/cmd.rs"])];
        assert_eq!(blame(&f, &both, &packages()), Blame::Several(vec![1, 2]));
    }

    #[test]
    fn a_failed_test_blames_the_mr_that_changed_its_file_the_longest_match_first() {
        let t = |binary: &str, name: &str| FailedTest { binary: binary.into(), name: name.into() };
        let f = Failure { tests: vec![t("my-engine::it", "panic_hunt::every_command")], ..Failure::default() };
        // #1 added a test file's `mod` line to main.rs; #2 changed the test's own file.
        let touched = [touch(1, &["crates/engine/tests/it/main.rs", "crates/engine/tests/it/new.rs"]), touch(2, &["crates/engine/tests/it/panic_hunt.rs"])];
        assert!(matches!(blame(&f, &touched, &packages()), Blame::One(2, _)));
        // A unit test: its module's file.
        let unit = Failure { tests: vec![t("my-model", "store::tests::reads_back")], ..Failure::default() };
        let touched = [touch(3, &["crates/model/src/store.rs"]), touch(4, &["crates/model/src/lib.rs"])];
        assert!(matches!(blame(&unit, &touched, &packages()), Blame::One(3, _)));
        // No file matches: the package does.
        let touched = [touch(5, &["crates/model/src/other.rs"]), touch(6, &["crates/engine/src/x.rs"])];
        assert!(matches!(blame(&unit, &touched, &packages()), Blame::One(5, why) if why.contains("my-model")));
    }

    #[test]
    fn a_nextest_failures_own_libtest_line_is_not_the_golden_binarys() {
        // a swarm's gate-500 : nextest prints the failed test's
        // stdout, libtest's `test <name> ... FAILED` among it. Read as the golden binary's too, it
        // also blamed the PR that changed the golden test's file: two PRs, and a bisection.
        let log = "        FAIL [   4.012s] (1/1) my-model::ray_tracing ray_tracing_is_captured\n  stdout ───\n\n    running 1 test\n    \
                   test ray_tracing_is_captured ... FAILED\n";
        let f = failure(log, ROOT, "my-engine::golden");
        assert_eq!(f.tests, vec![FailedTest { binary: "my-model::ray_tracing".into(), name: "ray_tracing_is_captured".into() }]);
        let touched = [touch(1, &["crates/engine/tests/golden.rs"]), touch(2, &["crates/model/tests/ray_tracing.rs"])];
        assert!(matches!(blame(&f, &touched, &packages()), Blame::One(2, _)));
    }

    #[test]
    fn nothing_that_matches_blames_no_one() {
        let f = Failure { packages: vec!["my-engine".into()], ..Failure::default() };
        assert_eq!(blame(&f, &[touch(1, &["docs/a.md"])], &packages()), Blame::None);
        assert_eq!(blame(&Failure::default(), &[touch(1, &["crates/engine/src/a.rs"])], &packages()), Blame::None);
        let two = [touch(1, &["crates/engine/src/a.rs"]), touch(2, &["crates/engine/src/b.rs"])];
        assert_eq!(blame(&f, &two, &packages()), Blame::Several(vec![1, 2]));
    }
}
