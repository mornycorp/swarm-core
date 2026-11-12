//! [`scan`]: text scans of the sources for what a layer may not use (the contract's rules that name
//! `scan` as their check), each held to an allow-list that only ratchets down.
//!
//! A [`Scan`] says what to look for ([`Pattern`]s), where (globs of files, from the workspace
//! root, with `/`) and in which part of a file ([`Region`]): production code is the file less its
//! `#[cfg(test)]` modules (each to its own end). Comments are always taken out of the text, and string
//! literals blanked unless the scan reads them ([`Strings`]), so a pattern named in a comment or
//! a message never matches.
//!
//! Each file may hold as many matches as its [`Allowed`] entry says, and every entry gives a reason
//! and a count: **more matches fail** (a new one: fix it, or add an entry with a reason), **fewer
//! fail too** (one was fixed: lower the entry, so the allowance can't be spent again). An entry for
//! a file the scan doesn't read, with no reason or a count of 0, fails as well.
//!
//! The built-in scans are `const fn`s the app points at its files:
//!
//! ```ignore
//! const SCANS: &[Scan] = &[
//!     Scan::process_command().files(&["crates/**/*.rs", "apps/**/*.rs"]).except(&["crates/domain/src/process.rs"]),
//!     Scan::ui_file_writes().files(&["crates/ui-egui/src/**/*.rs"]),
//!     Scan::value_reads().files(&["crates/ui-egui/src/**/*.rs", "crates/automation/src/**/*.rs", "apps/*/src/**/*.rs"]),
//! ];
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use super::{Finding, Findings, glob, manifest, patterns, read, source};

/// What one scan looks for in a source.
#[derive(Debug, Clone, Copy)]
pub enum Pattern {
    /// Each of these texts, as a whole word where they begin or end with an identifier character
    /// (`File::create` is not `File::create_new`).
    Text(&'static [&'static str]),
    /// A mention of `std::process::Command`: its path, or its name in a `process::{…}` import.
    ProcessCommand,
    /// A read of a `serde_json::Value` by key: `v["k"]`, `.get("k")`, `.as_object()`, `.pointer(…)`.
    ValueReads,
    /// A literal of the struct with this name (`CommandSpec {`, or `CommandSpec::default()`), not its
    /// definition, an impl or a pattern.
    StructLiteral(&'static str),
    /// The app's own: the 1-based line of each match in a source without comments.
    Custom(fn(&str) -> Vec<usize>),
}

impl Pattern {
    /// The line of each match in `source`.
    fn lines(&self, source: &str) -> Vec<usize> {
        match self {
            Pattern::Text(needles) => patterns::text(source, needles),
            Pattern::ProcessCommand => patterns::process_command(source),
            Pattern::ValueReads => patterns::value_reads(source),
            Pattern::StructLiteral(name) => patterns::struct_literal(source, name),
            Pattern::Custom(f) => f(source),
        }
    }
}

/// Where a scan's own setup is: a finding about it names this, not a file of the workspace. The
/// scans are the project's `Hooks::scans`, in its xtask.
pub const CONFIGURED: &str = "xtask (Hooks::scans)";

/// The files a scan takes as wholly test code unless it says otherwise ([`Scan::test_files`]):
/// a `tests` directory (integration tests, a module's `tests/`), and a module's own `tests.rs`,
/// which its parent brings in under `#[cfg(test)]`.
pub const TEST_FILES: &[&str] = &["**/tests/**", "**/tests.rs"];

/// Which part of a file a scan reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// The file less its `#[cfg(test)]` modules (an inline one to its closing brace, a
    /// `mod tests;` its one item; `cfg(all(test, …))` too), and none of the files named by
    /// [`Scan::test_files`].
    Production,
    /// The file's `#[cfg(test)]` modules, and the whole of the files named by
    /// [`Scan::test_files`] (integration tests, a `tests.rs`).
    Tests,
}

/// What becomes of a string literal before a scan reads the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strings {
    /// Blanked: a pattern can't match inside one (a message that names `std::process::Command`).
    Blank,
    /// Kept: for a pattern that is itself a string (`"APPDATA"`).
    Keep,
}

/// A file a scan allows some matches in, and why.
#[derive(Debug, Clone, Copy)]
pub struct Allowed {
    /// The file, from the workspace root, with `/`.
    pub file: &'static str,
    /// How many matches it may hold: a ratchet, lowered as they are fixed.
    pub count: usize,
    /// Why they may stay: never empty.
    pub reason: &'static str,
}

/// One scan: a rule, what it looks for, where, and what it allows.
#[derive(Debug, Clone, Copy)]
pub struct Scan {
    /// The rule's name, in each finding (`value-reads`).
    pub rule: &'static str,
    pub patterns: &'static [Pattern],
    pub region: Region,
    pub strings: Strings,
    /// The files it reads: globs from the workspace root (`crates/ui-egui/src/**/*.rs`).
    pub files: &'static [&'static str],
    /// Files left out of `files`.
    pub except: &'static [&'static str],
    /// The files that are wholly test code (see [`Region`]): [`TEST_FILES`] unless the scan says.
    pub test_files: &'static [&'static str],
    pub allow: &'static [Allowed],
    /// What a match is, for the message (`untyped reply read(s)`).
    pub what: &'static str,
    /// What to do about one.
    pub advice: &'static str,
}

impl Scan {
    /// A scan of production code for `patterns`, reading no file yet: name them with
    /// [`Scan::files`].
    pub const fn new(rule: &'static str, patterns: &'static [Pattern], what: &'static str, advice: &'static str) -> Scan {
        Scan { rule, patterns, region: Region::Production, strings: Strings::Blank, files: &[], except: &[], test_files: TEST_FILES, allow: &[], what, advice }
    }

    /// Read the files these globs name.
    pub const fn files(mut self, globs: &'static [&'static str]) -> Scan {
        self.files = globs;
        self
    }

    /// Leave out the files these globs name (the process module, the state module).
    pub const fn except(mut self, globs: &'static [&'static str]) -> Scan {
        self.except = globs;
        self
    }

    /// The files that are wholly test code, in place of [`TEST_FILES`].
    pub const fn test_files(mut self, globs: &'static [&'static str]) -> Scan {
        self.test_files = globs;
        self
    }

    /// Hold each file to its allowance.
    pub const fn allowing(mut self, allow: &'static [Allowed]) -> Scan {
        self.allow = allow;
        self
    }

    /// Another rule name, for the second use of a built-in (`state-file-writes`).
    pub const fn named(mut self, rule: &'static str) -> Scan {
        self.rule = rule;
        self
    }

    /// Another thing to do about a match.
    pub const fn advising(mut self, advice: &'static str) -> Scan {
        self.advice = advice;
        self
    }

    /// Other patterns than the built-in's (the app's own names for the real config dir, say).
    pub const fn matching(mut self, patterns: &'static [Pattern]) -> Scan {
        self.patterns = patterns;
        self
    }

    /// Read the text of string literals too.
    pub const fn reading_strings(mut self) -> Scan {
        self.strings = Strings::Keep;
        self
    }

    /// **`process-module`**: `std::process::Command` named anywhere but the process module (name it
    /// in [`Scan::except`]); the contract's Never hang on another program.
    pub const fn process_command() -> Scan {
        Scan::new(
            "process-module",
            &[Pattern::ProcessCommand],
            "mention(s) of `std::process::Command`",
            "start the program through the process module (a new function there): it closes stdin, sets the deadlines, caps the output and kills the process group",
        )
    }

    /// **`file-writes`**: a file write ([`patterns::FILE_WRITES`], or a file opened to write by its
    /// options, [`patterns::write_opens`]) outside the state module (name it in [`Scan::except`]);
    /// the contract's State on disk. A read-only open is no match.
    pub const fn file_writes() -> Scan {
        Scan::new(
            "file-writes",
            &[Pattern::Text(patterns::FILE_WRITES), Pattern::Custom(patterns::write_opens), Pattern::Custom(patterns::fs_imports)],
            "file write(s)",
            "persist state through the state module and its one atomic writer (a temporary file, flushed, renamed)",
        )
    }

    /// **`ui-file-writes`**: a file write in the UI crate; the contract's The UI is a client.
    pub const fn ui_file_writes() -> Scan {
        Scan::file_writes().named("ui-file-writes").advising("the UI never writes a file: call a command, on a worker, whose effects say it writes")
    }

    /// **`ui-process-starts`**: a process started in the UI crate (`std::process::Command`, or a
    /// system opener, [`patterns::OPENERS`]); the contract's The UI is a client.
    pub const fn ui_process_starts() -> Scan {
        Scan::new(
            "ui-process-starts",
            &[Pattern::ProcessCommand, Pattern::Text(patterns::OPENERS)],
            "process start(s)",
            "the UI never starts a program: call a command, on a worker, whose effects say it runs code",
        )
    }

    /// **`value-reads`**: a `serde_json::Value` read by key (`v["k"]`, `.get("k")`, `.as_object()`) in
    /// the UI, automation and the apps; the contract's The UI is a client. It reads text, not
    /// types: a map's `.get("k")`, a header's or a TOML table's matches too. Point it only at the
    /// code that consumes command replies, and give a file that reads another kind of map by key
    /// its count in the allow-list, with the reason.
    pub const fn value_reads() -> Scan {
        Scan::new(
            "value-reads",
            &[Pattern::ValueReads],
            "untyped reply read(s) (`v[\"k\"]`, `.get(\"k\")`, `.as_object()`, `.pointer(…)`)",
            "read the command's typed result (`call_typed`): a field the code needs is in a result type, so a rename is a compile error",
        )
    }

    /// **`ui-colours`**: a colour made in the UI crate's drawing code ([`patterns::COLOURS`]:
    /// `Color32::from_rgb(…)`, `Color32::RED`, `hex_color!`) rather than taken from the theme's tokens
    /// (name the theme module in [`Scan::except`]); the contract's The UI is a client.
    pub const fn ui_colours() -> Scan {
        Scan::new(
            "ui-colours",
            &[Pattern::Text(patterns::COLOURS)],
            "colour literal(s) (`Color32::…`, `Rgba::…`, `Hsva::…`, `hex_color!`)",
            "take the colour from the theme's tokens (a new token in the theme module, made there, when none fits), so a theme or a contrast fix changes one place",
        )
    }

    /// **`ui-strings`**: a string literal with text in the UI crate's drawing code
    /// ([`patterns::string_literals`]; not `""`, not one in an attribute) rather than the string
    /// catalog's (name the catalog module in [`Scan::except`]); the contract's The UI is a client.
    /// It counts every literal with text, a log line's and an assert's message among them: point it
    /// at the drawing code alone, and give what is never drawn (a thread's name, a `Debug` name)
    /// its count in the allow-list, with the reason.
    pub const fn ui_strings() -> Scan {
        Scan::new(
            "ui-strings",
            &[Pattern::Custom(patterns::string_literals)],
            "string literal(s) in drawing code",
            "take the text from the string catalog (an entry there; a function of the values for a text with values in it); an id the toolkit keeps state under is a typed salt (an enum), not a string",
        )
    }

    /// **`command-spec`**: a `CommandSpec { … }` literal (or a `CommandSpec::default()`) outside the
    /// typed adapter, which `allowing` holds to its one constructor; the contract's Every action is
    /// a command.
    pub const fn command_spec() -> Scan {
        Scan::new(
            "command-spec",
            &[Pattern::StructLiteral("CommandSpec")],
            "command spec literal(s) (`CommandSpec {`, `CommandSpec::default()`)",
            "register the command through the typed adapter (`Registry::register` builds every spec, with its schemas and effects)",
        )
    }

    /// **`real-config-dir`**: the real config, data or home directory named in test code
    /// ([`patterns::REAL_CONFIG_DIR`]; [`Scan::matching`] adds the app's own function); the
    /// contract's Tests touch nothing real.
    pub const fn real_config_dir() -> Scan {
        let mut scan = Scan::new(
            "real-config-dir",
            &[Pattern::Text(patterns::REAL_CONFIG_DIR), Pattern::Custom(patterns::dirs_imports)],
            "mention(s) of the real config dir in test code",
            "take the test's config and state dir from the testkit's `temp_dir`, and point the app's config-dir function at it",
        );
        scan.region = Region::Tests;
        scan.reading_strings()
    }

    /// Whether this scan reads `file`, and whether all of it is test code.
    fn selects(&self, file: &str) -> Option<bool> {
        let wholly_test = glob::any(self.test_files, file);
        let wanted = glob::any(self.files, file) && !glob::any(self.except, file) && (self.region == Region::Tests || !wholly_test);
        wanted.then_some(wholly_test && self.region == Region::Tests)
    }

    /// The malformed parts of this scan's own setup.
    fn problems(&self) -> Vec<Finding> {
        let here = CONFIGURED;
        let mut found = Vec::new();
        let mut bad = |what: String, fix: &str| found.push(Finding::new(here, None, self.rule, format!("the `{}` scan {what}", self.rule), fix));
        if self.files.is_empty() {
            bad("names no files: it would read nothing".to_owned(), "name the files with `.files(&[…])`");
        }
        if self.patterns.is_empty() {
            bad("looks for nothing".to_owned(), "give it patterns");
        }
        let mut seen = BTreeSet::new();
        for entry in self.allow {
            if entry.reason.trim().is_empty() {
                bad(format!("allows {} with no reason", entry.file), "give the entry its reason: why the matches may stay");
            }
            if entry.count == 0 {
                bad(format!("allows 0 matches in {}", entry.file), "remove the entry: a file with no match needs none");
            }
            if !seen.insert(entry.file) {
                bad(format!("allows {} twice", entry.file), "keep one entry per file");
            }
        }
        found
    }
}

/// What [`scan`] read and found allowed.
#[derive(Debug, Clone)]
pub struct ScanReport {
    /// How many files it read.
    pub files: usize,
    pub took: Duration,
    /// Per scan: its rule, the matches it found and the matches its allow-list allows.
    pub rules: Vec<(&'static str, usize, usize)>,
}

impl fmt::Display for ScanReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rules: Vec<String> = self.rules.iter().map(|(rule, found, allowed)| format!("{rule} {found} (allowed {allowed})")).collect();
        write!(f, "scan: {} files, {} rule(s): {}", self.files, self.rules.len(), rules.join(", "))
    }
}

/// A file read and stripped: its production code and its test modules, each with the other's text
/// blanked and every line where it was.
struct Loaded {
    stripped: String,
    production: String,
    tests: String,
}

impl Loaded {
    fn new(text: &str, keep_strings: bool) -> Loaded {
        // The test modules are found in the text with its strings blanked (a `{` in a string is no
        // brace); kept or blanked, the strings take the same chars.
        let blank = source::strip(text, false);
        let modules = source::test_modules(&blank);
        let stripped = if keep_strings { source::strip(text, true) } else { blank };
        let (production, tests) = source::regions(&stripped, &modules);
        Loaded { stripped, production, tests }
    }

    /// The text a scan reads of this file.
    fn region(&self, region: Region, wholly_test: bool) -> &str {
        match (region, wholly_test) {
            (Region::Production, _) => &self.production,
            (Region::Tests, true) => &self.stripped,
            (Region::Tests, false) => &self.tests,
        }
    }
}

/// Every file read once per way of stripping it, however many scans read it.
struct Cache<'a> {
    root: &'a Path,
    loaded: BTreeMap<(String, bool), Option<Loaded>>,
}

impl Cache<'_> {
    fn get(&mut self, file: &str, keep_strings: bool, found: &mut Vec<Finding>) -> Option<&Loaded> {
        let root = self.root;
        let load = || match read(root, file) {
            Ok(text) => Some(Loaded::new(&text, keep_strings)),
            Err(finding) => {
                found.push(finding);
                None
            }
        };
        self.loaded.entry((file.to_owned(), keep_strings)).or_insert_with(load).as_ref()
    }
}

/// Run `scans` over the workspace at `root`: every finding, or what was read.
pub fn scan(root: &Path, scans: &[Scan]) -> Result<ScanReport, Findings> {
    if scans.is_empty() {
        return Ok(ScanReport { files: 0, took: Duration::ZERO, rules: Vec::new() });
    }
    let (files, listing) = manifest::files(root);
    scan_files(root, scans, &files, listing)
}

/// [`scan`] over `files` (every file of the workspace, from [`manifest::files`]), with the
/// `found` so far.
pub(crate) fn scan_files(root: &Path, scans: &[Scan], files: &[String], mut found: Vec<Finding>) -> Result<ScanReport, Findings> {
    let start = Instant::now();
    let mut cache = Cache { root, loaded: BTreeMap::new() };
    let mut rules = Vec::new();
    for one in scans {
        found.extend(one.problems());
        rules.push(run_one(one, files, &mut cache, &mut found));
    }
    let read_files: BTreeSet<&str> = cache.loaded.keys().map(|(f, _)| f.as_str()).collect();
    if found.is_empty() { Ok(ScanReport { files: read_files.len(), took: start.elapsed(), rules }) } else { Err(Findings(found)) }
}

/// One scan's matches, judged against its allow-list: its rule, the matches and the allowance.
fn run_one(scan: &Scan, files: &[String], cache: &mut Cache<'_>, found: &mut Vec<Finding>) -> (&'static str, usize, usize) {
    let selected: Vec<(&String, bool)> = files.iter().filter_map(|f| scan.selects(f).map(|w| (f, w))).collect();
    // Each glob reads something: one that matches nothing (a crate renamed, a typo) is a silent hole.
    for stale in scan.files.iter().filter(|g| !selected.iter().any(|(f, _)| glob::matches(g, f))) {
        let problem = format!("its files glob {stale:?} (of {:?}) matches nothing (less the excepted ones)", scan.files);
        found.push(Finding::new(
            CONFIGURED,
            None,
            scan.rule,
            format!("the `{}` scan: {problem}", scan.rule),
            "fix the glob: a glob that reads nothing finds nothing",
        ));
    }
    let mut matches: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (file, wholly_test) in selected {
        let Some(loaded) = cache.get(file, scan.strings == Strings::Keep, found) else { continue };
        let text = loaded.region(scan.region, wholly_test);
        let mut lines: Vec<usize> = scan.patterns.iter().flat_map(|p| p.lines(text)).collect();
        lines.sort_unstable();
        matches.insert(file, lines);
    }
    found.extend(ratchet(scan, &matches));
    let total = matches.values().map(Vec::len).sum();
    (scan.rule, total, scan.allow.iter().map(|a| a.count).sum())
}

/// `matches` (the line of each match, by file read) against `scan`'s allow-list.
fn ratchet(scan: &Scan, matches: &BTreeMap<&str, Vec<usize>>) -> Vec<Finding> {
    let mut found = Vec::new();
    let allowance = |file: &str| scan.allow.iter().find(|a| a.file == file);
    for (file, lines) in matches {
        let max = allowance(file).map_or(0, |a| a.count);
        match lines.len().cmp(&max) {
            std::cmp::Ordering::Greater => found.push(over(scan, file, lines, allowance(file))),
            std::cmp::Ordering::Less => {
                let fix = if lines.is_empty() { "remove its allow-list entry".to_owned() } else { format!("lower its allow-list entry to {}", lines.len()) };
                let problem = format!("{} {}, fewer than the {max} the allow-list holds (one was fixed)", lines.len(), scan.what);
                found.push(Finding::new(file, None, scan.rule, problem, fix));
            }
            std::cmp::Ordering::Equal => {}
        }
    }
    for entry in scan.allow.iter().filter(|a| !matches.contains_key(a.file)) {
        let problem = format!("the allow-list has an entry for {}, which the scan doesn't read", entry.file);
        found.push(Finding::new(entry.file, None, scan.rule, problem, "remove the entry (the file moved, was deleted, or is excepted)"));
    }
    found
}

/// The finding for `file`, which has more matches (`lines`) than `allowed` holds.
fn over(scan: &Scan, file: &str, lines: &[usize], allowed: Option<&Allowed>) -> Finding {
    let max = allowed.map_or(0, |a| a.count);
    let reason = allowed.map_or(String::new(), |a| format!(" ({})", a.reason));
    let at: Vec<String> = lines.iter().take(10).map(usize::to_string).collect();
    let more = if lines.len() > 10 { ", …" } else { "" };
    let problem = format!("{} {}, {max} allowed{reason}: lines {}{more}", lines.len(), scan.what, at.join(", "));
    let fix = format!("{}; if one must stay, raise the file's allow-list entry (a reason and the new count)", scan.advice);
    Finding::new(file, lines.get(max).copied(), scan.rule, problem, fix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use swarm_core_testkit::temp_dir;

    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = temp_dir(name);
        for (path, text) in files {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        }
        dir
    }

    fn failures(root: &Path, scans: &[Scan]) -> Vec<String> {
        scan(root, scans).err().map(|f| f.0.iter().map(Finding::to_string).collect()).unwrap_or_default()
    }

    const SRC: &[&str] = &["crates/**/src/**/*.rs"];

    #[test]
    fn production_code_stops_at_the_test_module_and_loses_its_comments() {
        let dir = tree(
            "scan-region",
            &[(
                "crates/a/src/lib.rs",
                "fn a() { v[\"x\"]; }\n// v[\"y\"]\n/// v[\"z\"]\n/* v[\"w\"] */\nfn b() { let s = \"v[\\\"s\\\"]\"; }\n#[cfg(test)]\nmod tests {\n    fn t() { v[\"t\"]; }\n}\n",
            )],
        );
        let one = [Scan::value_reads().files(SRC)];
        let found = failures(&dir, &one);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("crates/a/src/lib.rs:1: [value-reads] 1 untyped reply read(s)"), "{found:?}");
        assert!(found[0].contains("1 untyped") && found[0].contains("0 allowed: lines 1"), "{found:?}");
    }

    #[test]
    fn the_process_module_is_the_only_place_for_std_process_command() {
        let bad = "use std::process::Command;\nfn run() { Command::new(\"git\"); }\nconst M: &str = \"use std::process::Command\";\n// std::process::Command in a comment\n";
        let dir = tree(
            "scan-process",
            &[("crates/a/src/lib.rs", bad), ("crates/a/src/process.rs", "use std::process::Command;\n"), ("crates/a/src/clap.rs", "use clap::Command;\n")],
        );
        let scans = [Scan::process_command().files(SRC).except(&["crates/a/src/process.rs"])];
        let found = failures(&dir, &scans);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("crates/a/src/lib.rs:1: [process-module] 1 mention(s) of `std::process::Command`, 0 allowed: lines 1. Fix: start the program through the process module"), "{found:?}");
        let clean = tree("scan-process-ok", &[("crates/a/src/process.rs", "use std::process::{Command, Stdio};\n"), ("crates/a/src/lib.rs", "fn f() {}\n")]);
        assert!(scan(&clean, &scans).is_ok());
    }

    #[test]
    fn the_ui_crate_writes_no_file_and_starts_no_program() {
        let ui =
            "fn save() { std::fs::write(\"a\", b\"\"); let _ = File::create(\"b\"); }\nfn go() { open::that(\"x\"); std::process::Command::new(\"y\"); }\n";
        let dir = tree("scan-ui", &[("crates/ui/src/app.rs", ui), ("crates/domain/src/state.rs", "fn s() { std::fs::write(\"a\", b\"\"); }\n")]);
        let scans = [Scan::ui_file_writes().files(&["crates/ui/src/**"]), Scan::ui_process_starts().files(&["crates/ui/src/**"])];
        let found = failures(&dir, &scans);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[0].starts_with("crates/ui/src/app.rs:1: [ui-file-writes] 2 file write(s), 0 allowed: lines 1, 1. Fix: the UI never writes a file"),
            "{found:?}"
        );
        assert!(
            found[1].starts_with("crates/ui/src/app.rs:2: [ui-process-starts] 2 process start(s), 0 allowed: lines 2, 2. Fix: the UI never starts a program"),
            "{found:?}"
        );
        // Outside the state module and its writer, any write is one; inside, none is looked for.
        let state = [Scan::file_writes().files(&["crates/**/src/**"]).except(&["crates/domain/src/state.rs"])];
        let found = failures(&dir, &state);
        assert!(found.len() == 1 && found[0].starts_with("crates/ui/src/app.rs:1: [file-writes] 2 file write(s)"), "{found:?}");
    }

    /// The drawing code takes its colours from the theme and its words from the catalog: a colour or
    /// a word made anywhere else in the UI crate is a finding, a reasoned one held to its count.
    #[test]
    fn the_ui_crate_takes_colours_from_the_theme_and_words_from_the_catalog() {
        let draw = "#[expect(clippy::x, reason = \"a reason is no word drawn\")]\nfn d(ui: &mut Ui) {\n    ui.colored_label(Color32::RED, \"Failed\");\n    ui.label(catalog::RUN);\n    ui.label(\"\");\n}\n";
        let dir = tree(
            "scan-ui-look",
            &[
                ("crates/ui/src/draw.rs", draw),
                ("crates/ui/src/theme.rs", "pub const OK: Color32 = Color32::from_rgb(0, 160, 0);\n"),
                ("crates/ui/src/catalog.rs", "pub const RUN: &str = \"Run\";\n"),
                ("crates/ui/src/jobs.rs", "fn w() { thread::Builder::new().name(\"ui-call\".to_owned()); }\n"),
                ("crates/ui/src/lib.rs", "#[cfg(test)]\nmod tests { fn t() { assert_eq!(Color32::RED, x, \"in a test\"); } }\n"),
            ],
        );
        const UI: &[&str] = &["crates/ui/src/**/*.rs"];
        static NAMED: &[Allowed] = &[Allowed { file: "crates/ui/src/jobs.rs", count: 1, reason: "the worker thread's name, never drawn" }];
        let scans = [
            Scan::ui_colours().files(UI).except(&["crates/ui/src/theme.rs"]),
            Scan::ui_strings().files(UI).except(&["crates/ui/src/catalog.rs"]).allowing(NAMED),
        ];
        let found = failures(&dir, &scans);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[0].starts_with("crates/ui/src/draw.rs:3: [ui-colours] 1 colour literal(s)") && found[0].contains("Fix: take the colour from the theme"),
            "{found:?}"
        );
        assert!(found[1].starts_with("crates/ui/src/draw.rs:3: [ui-strings] 1 string literal(s) in drawing code, 0 allowed: lines 3."), "{found:?}");
        assert!(found[1].contains("Fix: take the text from the string catalog"), "{found:?}");
        // The theme and the catalog themselves are the places for them; without the exceptions they count.
        let unexcepted = failures(&dir, &[Scan::ui_colours().files(UI), Scan::ui_strings().files(UI).allowing(NAMED)]);
        assert!(unexcepted.iter().any(|f| f.starts_with("crates/ui/src/theme.rs:1: [ui-colours] 1 ")), "{unexcepted:?}");
        assert!(unexcepted.iter().any(|f| f.starts_with("crates/ui/src/catalog.rs:1: [ui-strings] 1 ")), "{unexcepted:?}");
    }

    #[test]
    fn value_reads_are_held_in_the_ui_automation_and_apps() {
        let dir = tree(
            "scan-reads",
            &[
                ("crates/ui/src/pane.rs", "fn a(r: Value) { r[\"field\"]; r.get(\"k\"); }\n"),
                ("crates/automation/src/serve.rs", "fn b(r: Value) { r.as_object(); }\n"),
                ("apps/cli/src/main.rs", "fn c() { let a = [\"x\", \"y\"]; }\n"),
                ("crates/engine/src/lib.rs", "fn d(r: Value) { r[\"free\"]; }\n"),
            ],
        );
        let scans = [Scan::value_reads().files(&["crates/ui/src/**", "crates/automation/src/**", "apps/*/src/**"])];
        let found = failures(&dir, &scans);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|f| f.starts_with("crates/ui/src/pane.rs:1:") && f.contains("2 untyped reply read(s)")), "{found:?}");
        assert!(found.iter().any(|f| f.starts_with("crates/automation/src/serve.rs:1:")), "{found:?}");
    }

    #[test]
    fn a_command_spec_is_built_only_in_the_typed_adapter() {
        let adapter = "pub struct CommandSpec { pub id: &'static str }\nimpl Registry {\n    pub fn register() { let s = CommandSpec { id: \"a.b\" }; }\n}\n";
        let dir = tree(
            "scan-spec",
            &[("crates/engine/src/typed.rs", adapter), ("crates/engine/src/cmds.rs", "fn x() -> CommandSpec {\n    CommandSpec { id: \"x.y\" }\n}\n")],
        );
        static ALLOWED: &[Allowed] = &[Allowed { file: "crates/engine/src/typed.rs", count: 1, reason: "Registry::register builds every spec" }];
        let scans = [Scan::command_spec().files(SRC).allowing(ALLOWED)];
        let found = failures(&dir, &scans);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0]
                .starts_with("crates/engine/src/cmds.rs:2: [command-spec] 1 command spec literal(s) (`CommandSpec {`, `CommandSpec::default()`), 0 allowed"),
            "{found:?}"
        );
        // A second constructor in the adapter is over its one.
        let two = format!("{adapter}fn again() {{ CommandSpec {{ id: \"c.d\" }}; }}\n");
        let dir = tree("scan-spec-two", &[("crates/engine/src/typed.rs", &two)]);
        let found = failures(&dir, &scans);
        assert!(
            found.len() == 1
                && found[0].starts_with("crates/engine/src/typed.rs:5: [command-spec] 2 ")
                && found[0].contains("1 allowed (Registry::register builds every spec)"),
            "{found:?}"
        );
    }

    #[test]
    fn the_real_config_dir_is_looked_for_in_test_code_only() {
        let lib = "fn config() -> PathBuf { dirs::config_dir().unwrap() }\n#[cfg(test)]\nmod tests {\n    fn t() { let p = std::env::var(\"APPDATA\"); }\n}\n";
        let dir = tree(
            "scan-config",
            &[
                ("crates/a/src/lib.rs", lib),
                ("crates/a/tests/it.rs", "fn t() { let h = dirs::home_dir(); }\n"),
                ("crates/a/tests/clean.rs", "fn t() { let d = temp_dir(\"x\"); }\n"),
            ],
        );
        let scans = [Scan::real_config_dir().files(&["crates/**/*.rs"]).test_files(&["**/tests/**"])];
        let found = failures(&dir, &scans);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|f| f.starts_with("crates/a/src/lib.rs:4: [real-config-dir] 1 mention(s) of the real config dir in test code")), "{found:?}");
        assert!(found.iter().any(|f| f.starts_with("crates/a/tests/it.rs:1:")), "{found:?}");
        // The app names its own config-dir function.
        const OWN: &[Pattern] = &[Pattern::Text(&["app_config_dir"])];
        let own = tree("scan-config-own", &[("crates/a/tests/it.rs", "fn t() { app_config_dir(); }\n")]);
        assert_eq!(failures(&own, &[Scan::real_config_dir().files(&["crates/**/*.rs"]).test_files(&["**/tests/**"]).matching(OWN)]).len(), 1);
    }

    #[test]
    fn the_ratchet_fails_on_more_and_on_fewer_and_on_a_stale_entry() {
        let dir = tree("scan-ratchet", &[("crates/a/src/x.rs", "fn f() { std::fs::write(1); fs::copy(1); }\n"), ("crates/a/src/y.rs", "fn g() {}\n")]);
        let allow = |count: usize| -> &'static [Allowed] { Box::leak(Box::new([Allowed { file: "crates/a/src/x.rs", count, reason: "the writer" }])) };
        let scan_with = |a: &'static [Allowed]| [Scan::file_writes().files(SRC).allowing(a)];
        assert!(scan(&dir, &scan_with(allow(2))).is_ok(), "exactly the allowance passes");
        let more = failures(&dir, &scan_with(allow(1)));
        assert!(more.len() == 1 && more[0].starts_with("crates/a/src/x.rs:1: [file-writes] 2 file write(s), 1 allowed (the writer)"), "{more:?}");
        let fewer = failures(&dir, &scan_with(allow(3)));
        assert!(
            fewer.len() == 1
                && fewer[0].starts_with("crates/a/src/x.rs: [file-writes] 2 file write(s), fewer than the 3")
                && fewer[0].ends_with("Fix: lower its allow-list entry to 2"),
            "{fewer:?}"
        );
        // An entry for a file that has none, one that isn't read, no reason, a count of 0, and a twin.
        static STALE: &[Allowed] = &[
            Allowed { file: "crates/a/src/y.rs", count: 1, reason: "gone" },
            Allowed { file: "crates/a/src/missing.rs", count: 1, reason: "moved" },
            Allowed { file: "crates/a/src/x.rs", count: 2, reason: " " },
            Allowed { file: "crates/a/src/z.rs", count: 0, reason: "none" },
            Allowed { file: "crates/a/src/x.rs", count: 2, reason: "twice" },
        ];
        let found = failures(&dir, &scan_with(STALE));
        for want in [
            "crates/a/src/y.rs: [file-writes] 0 file write(s), fewer than the 1 the allow-list holds (one was fixed). Fix: remove its allow-list entry",
            "crates/a/src/missing.rs: [file-writes] the allow-list has an entry for crates/a/src/missing.rs, which the scan doesn't read",
            "allows crates/a/src/x.rs with no reason",
            "allows 0 matches in crates/a/src/z.rs",
            "allows crates/a/src/x.rs twice",
        ] {
            assert!(found.iter().any(|f| f.contains(want)), "{want} in {found:#?}");
        }
    }

    #[test]
    fn a_scan_that_reads_nothing_or_a_file_that_cannot_be_read_is_a_finding() {
        let dir = tree("scan-misc", &[("crates/a/src/x.rs", "fn f() {}\n")]);
        assert!(failures(&dir, &[Scan::value_reads()]).iter().any(|f| f.contains("names no files")));
        let nothing = failures(&dir, &[Scan::value_reads().files(&["nowhere/**"])]);
        assert!(nothing.iter().any(|f| f.contains("matches nothing") && f.contains("fix the glob")), "{nothing:?}");
        std::fs::write(dir.join("crates/a/src/big.rs"), vec![b'x'; (super::super::MAX_FILE + 1) as usize]).unwrap();
        assert!(failures(&dir, &[Scan::value_reads().files(SRC)]).iter().any(|f| f.starts_with("crates/a/src/big.rs: [unreadable]")));
        assert!(scan(&dir, &[]).is_ok(), "no scans, nothing to do");
    }

    #[test]
    fn each_glob_that_matches_nothing_is_a_finding() {
        let dir = tree("scan-stale-glob", &[("crates/automation/src/lib.rs", "fn f() {}\n"), ("apps/cli/src/main.rs", "fn r(v: Value) { v[\"k\"]; }\n")]);
        let found = failures(&dir, &[Scan::value_reads().files(&["crates/automation/**/*.rs", "app/**/*.rs"])]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("xtask (Hooks::scans): [value-reads] the `value-reads` scan: its files glob \"app/**/*.rs\" (of "), "{found:?}");
        // A glob whose every file is excepted, or wholly test code, reads nothing either.
        let excepted = failures(&dir, &[Scan::value_reads().files(&["crates/**/*.rs", "apps/**/*.rs"]).except(&["apps/cli/src/main.rs"])]);
        assert!(excepted.len() == 1 && excepted[0].contains("glob \"apps/**/*.rs\""), "{excepted:?}");
    }

    #[test]
    fn production_code_after_a_test_module_is_still_scanned() {
        let dir = tree(
            "scan-after-tests",
            &[
                ("crates/a/src/top.rs", "#[cfg(test)]\nmod tests;\n\npub fn run() { std::process::Command::new(\"x\"); }\n"),
                ("crates/a/src/mid.rs", "#[cfg(test)]\nmod tests { }\npub fn run() { std::process::Command::new(\"x\"); }\n"),
                ("crates/a/src/all.rs", "#[cfg(all(test, unix))]\nmod tests { fn t() { std::process::Command::new(\"x\"); } }\n"),
                ("crates/a/src/raw.rs", "pub fn run() { let s = cr#\"a \" b\"#; std::process::Command::new(\"x\"); }\n"),
            ],
        );
        let found = failures(&dir, &[Scan::process_command().files(SRC)]);
        assert_eq!(found.len(), 3, "{found:?}");
        for want in ["crates/a/src/top.rs:4: [process-module]", "crates/a/src/mid.rs:3: [process-module]", "crates/a/src/raw.rs:1: [process-module]"] {
            assert!(found.iter().any(|f| f.starts_with(want)), "{want} in {found:?}");
        }
        // And the test region is the test modules only, not the production code after one.
        let tests = tree(
            "scan-tests-region",
            &[("crates/a/src/lib.rs", "#[cfg(test)]\nmod tests { fn t() { let d = dirs::home_dir(); } }\nfn real() { dirs::config_dir(); }\n")],
        );
        let found = failures(&tests, &[Scan::real_config_dir().files(SRC)]);
        assert!(found.len() == 1 && found[0].starts_with("crates/a/src/lib.rs:2: [real-config-dir] 2 mention(s)"), "{found:?}");
    }

    #[test]
    fn the_scans_see_through_imports_aliases_and_defaults() {
        let dir = tree(
            "scan-holes",
            &[
                ("crates/a/src/glob.rs", "use std::process::*;\npub fn run() { Command::new(\"x\"); }\n"),
                ("crates/a/src/alias.rs", "use std::process as p;\npub fn run() { p::Command::new(\"x\"); }\n"),
                ("crates/a/src/write.rs", "use std::fs::{self, write};\npub fn w() { write(\"p\", b\"x\"); }\n"),
                ("crates/a/src/options.rs", "pub fn w() { std::fs::File::options().write(true).open(\"p\"); }\n"),
                ("crates/a/src/reads.rs", "pub fn r(v: Value) -> Option<()> { let id = call()?[\"id\"].as_str(); v.pointer(\"/a/b\"); None }\n"),
                ("crates/a/src/spec.rs", "pub fn f(s: CommandSpec) { let CommandSpec { id, .. } = s; match s { CommandSpec { .. } => {} } }\n"),
                ("crates/a/src/default.rs", "pub fn f() { let mut s = CommandSpec::default(); s.id = \"x\"; }\n"),
                ("crates/a/src/dirs.rs", "#[cfg(test)]\nmod tests {\n use dirs::{config_dir};\n fn t() { config_dir(); }\n}\n"),
            ],
        );
        let scans = [
            Scan::process_command().files(SRC),
            Scan::file_writes().files(SRC),
            Scan::value_reads().files(SRC),
            Scan::command_spec().files(SRC),
            Scan::real_config_dir().files(SRC),
        ];
        let found = failures(&dir, &scans);
        for want in [
            "crates/a/src/glob.rs:1: [process-module] 1 ",
            "crates/a/src/alias.rs:2: [process-module] 1 ",
            "crates/a/src/write.rs:1: [file-writes] 1 ",
            "crates/a/src/options.rs:1: [file-writes] 1 ",
            "crates/a/src/reads.rs:1: [value-reads] 2 ",
            "crates/a/src/default.rs:1: [command-spec] 1 ",
            "crates/a/src/dirs.rs:3: [real-config-dir] 1 ",
        ] {
            assert!(found.iter().any(|f| f.starts_with(want)), "{want} in {found:#?}");
        }
        assert_eq!(found.len(), 7, "a struct pattern is no literal: {found:#?}");
    }

    #[test]
    fn a_hostile_file_near_the_cap_scans_in_linear_time() {
        for unit in ["#[cfg(test)]\n", "process::{\n", "#[cfg(test)] fn f() {}\n"] {
            let dir = tree("scan-hostile", &[("crates/a/src/lib.rs", unit.repeat((super::super::MAX_FILE as usize - 64) / unit.len()).as_str())]);
            let start = Instant::now();
            let _ = scan(&dir, &[Scan::process_command().files(SRC), Scan::real_config_dir().files(SRC)]);
            // Linear is about a second in a debug build; the quadratic scan took minutes.
            assert!(start.elapsed() < Duration::from_secs(20), "{unit:?}: {:?}", start.elapsed());
        }
    }

    /// A module's own `tests.rs` and a `tests` directory are test code unless the scan says
    /// otherwise: no `#[cfg(test)]` in the file says so, its parent's `mod tests;` does.
    #[test]
    fn a_tests_file_is_test_code_by_default() {
        let spawn = "fn f() { std::process::Command::new(\"x\"); }\n";
        let dir =
            tree("scan-tests-default", &[("crates/a/src/x/tests.rs", spawn), ("crates/a/src/tests/helper.rs", spawn), ("crates/a/src/x.rs", "mod tests;\n")]);
        assert!(failures(&dir, &[Scan::process_command().files(SRC)]).is_empty());
        let none = failures(&dir, &[Scan::process_command().files(SRC).test_files(&[])]);
        assert_eq!(none.len(), 2, "with no test files named, both are production: {none:?}");
    }

    #[test]
    fn the_report_says_what_was_read_and_allowed() {
        let dir = tree("scan-report", &[("crates/a/src/x.rs", "fn f() { fs::copy(1); }\n"), ("crates/a/src/y.rs", "fn g() {}\n")]);
        static ALLOWED: &[Allowed] = &[Allowed { file: "crates/a/src/x.rs", count: 1, reason: "the writer" }];
        let report = scan(&dir, &[Scan::file_writes().files(SRC).allowing(ALLOWED), Scan::value_reads().files(SRC)]).unwrap();
        assert_eq!((report.files, report.rules.clone()), (2, vec![("file-writes", 1, 1), ("value-reads", 0, 0)]));
        // The same text every run: no timing in it, so a consumer can compare a gate's output.
        assert_eq!(report.to_string(), "scan: 2 files, 2 rule(s): file-writes 1 (allowed 1), value-reads 0 (allowed 0)");
    }
}
