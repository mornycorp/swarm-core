//! `cargo xtask ci --changed`'s affected set (phase 4): which of a
//! consumer's packages a change reaches, and why. The consumer's own `ci` runs the steps; this is
//! the plan it runs them for, the same in both swarms.
//!
//! - **The changed files** ([`changed_files`]) are those between the merge base with `--base` and
//!   the working tree, plus untracked files (or, with `--head`, that ref: a pushed branch). A
//!   manifest and lock diff that only moves Swarm Core's pin stands in as [`SWARM_CORE_PIN`]
//!   ([`crate::pin::only_moves_the_pin`]).
//! - **Each maps to a class** ([`classify`]): the package whose directory holds it, documentation,
//!   tooling no cargo step reads, data packages read from outside their directory, or a file that
//!   changes everything (the workspace manifest, the lint and format settings, the test runner's
//!   configuration, `.cargo/`, anything new outside a package). The project's own classes come
//!   first ([`crate::Hooks::file_class`]).
//! - **What a package reads from outside its directory** tests it too: the project's table
//!   ([`Rules::reads`]), and the files xtask's tests read through the root
//!   ([`crate::Hooks::xtask_reads`]). [`reads::scan`] finds such reads in the sources, and
//!   [`missed`] what a plan leaves out of them, for the consumer's test.
//! - **`Cargo.lock`** tests the packages whose resolved dependencies it changed ([`lock`]);
//!   it is the full run only when a lockfile can't be read.
//! - **The affected set** ([`affected`]) is closed over reverse dependencies: a package whose
//!   library changed reaches every package that depends on it, and so on; one that only
//!   dev-depends on it is tested, without reaching further.
//! - **Runtime edges** ([`Rules::runtime`]): a package whose tests launch another's
//!   artifact with no cargo edge to it is tested when the producer is built anew, and a producer
//!   whose consumers are tested without it is built first.

pub mod lock;
pub mod reads;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use crate::Hooks;

/// The lock file: planned by what its change reaches ([`lock_for`]), else the full run.
pub const LOCK: &str = "Cargo.lock";

/// Swarm Core's package: not a workspace member, but the graph keeps the edges to it, so a
/// bump of its pin reaches every package that depends on it.
pub const SWARM_CORE: &str = crate::pin::PACKAGE;

/// Swarm Core's testkit, which an app depends on (a dev-dependency) beside it, pinned to the same
/// commit: a bump of the pin moves it with Swarm Core.
pub const SWARM_CORE_TESTKIT: &str = crate::pin::TESTKIT;

/// What [`changed_files`] lists for `Cargo.toml` and `Cargo.lock` when their change only moves
/// Swarm Core's pin: Swarm Core itself, which [`plan`] follows to
/// the packages that depend on it.
pub const SWARM_CORE_PIN: &str = "swarm-core (its pin moved)";

/// What a consumer's change planning reads from it ([`crate::Hooks::changed`]).
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    /// The prefix its package names share, left out when the plan names them (`swarm-`).
    pub package_prefix: &'static str,
    /// Files, and directories (ending in `/`), that packages read from outside their own
    /// directory.
    pub reads: &'static [Read],
    /// Packages whose tests launch another package's artifact, with no cargo edge to it.
    pub runtime: &'static [Runtime],
}

impl Rules {
    /// No prefix, no reads, no runtime edges.
    pub const NONE: Rules = Rules { package_prefix: "", reads: &[], runtime: &[] };
}

/// A file or directory (`…/`) the `packages` read from outside their own directory, and whether
/// their code reads it (`embeds`: what depends on them is affected too) or only their tests do
/// (they alone are tested).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Read {
    pub path: &'static str,
    pub packages: &'static [&'static str],
    pub embeds: bool,
}

/// A runtime edge: `consumer`'s tests launch or load `producer`'s `artifact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    pub producer: &'static str,
    pub artifact: &'static str,
    pub consumer: &'static str,
}

/// How one package depends on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

/// A dependency on another workspace member (or on Swarm Core).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
}

/// A workspace member and its dependencies on other members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

/// `name` without the project's package prefix: `swarm-engine` → `engine`.
pub fn short_name<'a>(rules: &Rules, name: &'a str) -> &'a str {
    name.strip_prefix(rules.package_prefix).filter(|s| !s.is_empty()).unwrap_or(name)
}

/// The workspace's crate graph from `cargo metadata --no-deps`: each member and its dependencies
/// on other members and on Swarm Core (other external crates left out).
pub fn crates(meta: &serde_json::Value) -> Vec<Crate> {
    let packages = meta["packages"].as_array().cloned().unwrap_or_default();
    let members: BTreeSet<String> = packages.iter().filter_map(|p| p["name"].as_str().map(str::to_owned)).collect();
    let dep = |d: &serde_json::Value| {
        let name = d["name"].as_str()?;
        let kind = match d["kind"].as_str() {
            Some("dev") => DepKind::Dev,
            Some("build") => DepKind::Build,
            _ => DepKind::Normal,
        };
        (members.contains(name) || name == SWARM_CORE || name == SWARM_CORE_TESTKIT).then(|| Dep { name: name.to_owned(), kind })
    };
    packages
        .iter()
        .filter_map(|p| {
            Some(Crate { name: p["name"].as_str()?.to_owned(), deps: p["dependencies"].as_array().into_iter().flatten().filter_map(dep).collect() })
        })
        .collect()
}

/// The workspace members with a library target, from `cargo metadata`: the packages that can have
/// doctests (`cargo test --doc` refuses a package with none).
pub fn libraries(meta: &serde_json::Value) -> BTreeSet<String> {
    let is_lib = |t: &serde_json::Value| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| matches!(k.as_str(), Some("lib" | "rlib" | "proc-macro"))));
    meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["targets"].as_array().is_some_and(|ts| ts.iter().any(is_lib)))
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect()
}

/// The package directories relative to `root` (`/` separators), from `cargo metadata`.
pub fn package_dirs(meta: &serde_json::Value, root: &Path) -> Vec<(String, String)> {
    let root = root.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_owned();
            let manifest = p["manifest_path"].as_str()?.replace('\\', "/");
            let dir = manifest.strip_suffix("/Cargo.toml")?;
            // Windows drive letters may differ in case between git and cargo.
            let rel = dir.get(root.len()..).filter(|_| dir.get(..root.len()).is_some_and(|r| r.eq_ignore_ascii_case(root)))?;
            (rel.is_empty() || rel.starts_with('/')).then(|| (name, rel.trim_start_matches('/').to_owned()))
        })
        .collect()
}

/// A package that reads a file from outside its own directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reader {
    pub package: String,
    /// Its code (library, binary or build script) reads the file, so what depends on it is
    /// affected too; otherwise only its tests read it, and it alone is tested.
    pub embeds: bool,
}

/// What a changed file means for the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileClass {
    /// A file in package `name`'s directory, which the `readers` (other packages) read too.
    Package { name: String, readers: Vec<Reader> },
    /// Documentation (`docs/`, `scorecard/`, `*.md`): no build, but for the files a package reads,
    /// which are [`FileClass::Data`] for it.
    Docs,
    /// Repository tooling no cargo step reads (CI config, scripts, git and agent settings).
    NoBuild,
    /// Data these packages read from outside their own directory.
    Data(Vec<Reader>),
    /// Everything runs, for this reason.
    Everything(String),
}

/// Paths (relative to the root, `/` separators) whose change means the full run.
pub const EVERYTHING: &[&str] = &["Cargo.toml", LOCK, "clippy.toml", "rustfmt.toml", ".rustfmt.toml", ".config/nextest.toml"];
/// Directory prefixes whose change means the full run.
pub const EVERYTHING_DIRS: &[&str] = &[".cargo/"];
/// Tooling no cargo step reads.
pub const NO_BUILD: &[&str] = &[".gitignore", ".gitattributes", "LICENSE"];
/// `.github/`: the issue and pull request templates and the workflows, which no cargo step reads.
pub const NO_BUILD_DIRS: &[&str] = &[".github/", "scripts/", ".claude/", ".vscode/"];

/// Documentation (`docs/`, `scorecard/`, `*.md`): no build, but for the files a package reads.
pub fn is_docs(path: &str) -> bool {
    path.starts_with("docs/") || path.starts_with("scorecard/") || path.ends_with(".md")
}

/// The packages that read `path` from outside their own directory: the project's table
/// ([`Rules::reads`]) and xtask, when its tests read it ([`Hooks::xtask_reads`]).
pub fn readers(hooks: &dyn Hooks, path: &str) -> Vec<Reader> {
    let rules = hooks.changed();
    let mut r: BTreeSet<Reader> = rules
        .reads
        .iter()
        .filter(|read| if read.path.ends_with('/') { path.starts_with(read.path) } else { path == read.path })
        .flat_map(|read| read.packages.iter().map(|p| Reader { package: (*p).to_owned(), embeds: read.embeds }))
        .collect();
    if hooks.xtask_reads(path) {
        r.insert(Reader { package: "xtask".to_owned(), embeds: false });
    }
    r.into_iter().collect()
}

/// The deepest package directory holding `path`.
fn owner<'a>(path: &str, packages: &'a [(String, String)]) -> Option<&'a String> {
    packages
        .iter()
        .filter(|(_, dir)| !dir.is_empty() && path.strip_prefix(dir.as_str()).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|(_, dir)| dir.len())
        .map(|(name, _)| name)
}

/// Classify `path` (relative to the workspace root, `/` separators): the project's own class
/// first ([`Hooks::file_class`]), then the shared ones. `packages` maps each package to its
/// directory relative to the root (`""` for a root package).
pub fn classify(hooks: &dyn Hooks, path: &str, packages: &[(String, String)]) -> FileClass {
    if let Some(class) = hooks.file_class(path) {
        return class;
    }
    if path == SWARM_CORE_PIN {
        return FileClass::Package { name: SWARM_CORE.to_owned(), readers: Vec::new() };
    }
    if EVERYTHING.contains(&path) || path.starts_with("rust-toolchain") || EVERYTHING_DIRS.iter().any(|d| path.starts_with(d)) {
        return FileClass::Everything(format!("{path} changed"));
    }
    let readers = readers(hooks, path);
    let no_build = NO_BUILD.contains(&path) || NO_BUILD_DIRS.iter().any(|d| path.starts_with(d));
    if no_build || is_docs(path) {
        return if !readers.is_empty() {
            FileClass::Data(readers)
        } else if no_build {
            FileClass::NoBuild
        } else {
            FileClass::Docs
        };
    }
    match owner(path, packages) {
        Some(name) => FileClass::Package { name: name.clone(), readers },
        None if !readers.is_empty() => FileClass::Data(readers),
        // Anything new outside a package may be read by tests anywhere: be safe.
        None => FileClass::Everything(format!("{path} is outside any package")),
    }
}

/// The affected packages, each with why it is in the set: the changed ones (`changed`, with why),
/// every package that depends on one of them (normal or build) and so on, the packages that only
/// dev-depend on one (tested, not followed further), and the `tested` ones (only their tests read
/// a changed file: not followed either). Also the packages whose own build changed (the changed
/// ones and those that depend on one): their artifacts are new, a dev-dependent's are not.
/// Swarm Core, when it changed, is not built here: it reaches the packages that depend on it.
pub fn affected(
    rules: &Rules,
    crates: &[Crate],
    changed: &BTreeMap<String, String>,
    tested: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, BTreeSet<String>) {
    let known = |c: &str| crates.iter().any(|k| k.name == c);
    let mut why: BTreeMap<String, String> = tested.iter().filter(|(c, _)| known(c)).map(|(c, r)| (c.clone(), r.clone())).collect();
    let mut queue: VecDeque<String> = VecDeque::new();
    for (c, reason) in changed.iter().filter(|(c, _)| known(c) || *c == SWARM_CORE) {
        if c != SWARM_CORE {
            why.insert(c.clone(), reason.clone());
        }
        queue.push_back(c.clone());
    }
    // Swarm Core's pin moves its testkit with it: what depends on that is reached too.
    if changed.contains_key(SWARM_CORE) {
        queue.push_back(SWARM_CORE_TESTKIT.to_owned());
    }
    // Packages whose library changed: they reach their dependents.
    let mut lib: BTreeSet<String> = queue.iter().cloned().collect();
    while let Some(dep) = queue.pop_front() {
        for c in crates {
            let edges: Vec<DepKind> = c.deps.iter().filter(|d| d.name == dep).map(|d| d.kind).collect();
            // A package can name one dependency twice (normal and dev): the library edge wins.
            if edges.iter().any(|k| matches!(k, DepKind::Normal | DepKind::Build)) {
                if lib.insert(c.name.clone()) {
                    why.insert(c.name.clone(), format!("depends on {}", short_name(rules, &dep)));
                    queue.push_back(c.name.clone());
                }
            } else if edges.contains(&DepKind::Dev) {
                why.entry(c.name.clone()).or_insert_with(|| format!("dev-depends on {}", short_name(rules, &dep)));
            }
        }
    }
    lib.remove(SWARM_CORE);
    lib.remove(SWARM_CORE_TESTKIT);
    (why, lib)
}

/// Add to `crates` (the affected set) the consumers ([`Rules::runtime`]) of every producer in
/// `built`, the packages whose own build changed: tested, not followed further, as their own code
/// didn't change. Returns the producers to build first: those not in the set whose consumers are.
pub fn runtime(rules: &Rules, crates: &mut BTreeMap<String, String>, built: &BTreeSet<String>, all: &[Crate]) -> BTreeMap<String, String> {
    let known = |name: &str| all.iter().any(|c| c.name == name);
    for e in rules.runtime {
        if built.contains(e.producer) && known(e.consumer) {
            crates.entry(e.consumer.to_owned()).or_insert_with(|| format!("its tests launch {} ({} affected)", e.artifact, short_name(rules, e.producer)));
        }
    }
    let mut build: BTreeMap<String, (&str, Vec<&str>)> = BTreeMap::new();
    for e in rules.runtime {
        if crates.contains_key(e.consumer) && !crates.contains_key(e.producer) && known(e.producer) {
            build.entry(e.producer.to_owned()).or_insert_with(|| (e.artifact, Vec::new())).1.push(short_name(rules, e.consumer));
        }
    }
    build.into_iter().map(|(p, (artifact, users))| (p, format!("{artifact}, for the tests of {}", users.join(", ")))).collect()
}

/// What `ci --changed` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The full `cargo xtask ci`, for this reason.
    Everything(String),
    /// Only these packages (each with why); `build`: the producers of runtime artifacts whose
    /// consumers are tested without them, built first (each with why), so no test runs an artifact
    /// older than the tree; `files` changed.
    Crates { crates: BTreeMap<String, String>, build: BTreeMap<String, String>, files: usize },
}

impl Plan {
    /// The affected packages' names; none for the full run.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Plan::Crates { crates, .. } => crates.keys().map(String::as_str).collect(),
            Plan::Everything(_) => Vec::new(),
        }
    }
}

/// The plan for `files` (changed paths relative to the root). `lock`: the packages a change of
/// [`LOCK`] reaches, each with why ([`lock_for`]); without it, that change is the full run.
pub fn plan(hooks: &dyn Hooks, files: &[String], packages: &[(String, String)], crates: &[Crate], lock: Option<&BTreeMap<String, String>>) -> Plan {
    let rules = hooks.changed();
    let (mut changed, mut tested) = (BTreeMap::new(), BTreeMap::new());
    for f in files {
        if f == LOCK && hooks.file_class(f).is_none() {
            let Some(reached) = lock else { return Plan::Everything(format!("{LOCK} changed")) };
            for (c, why) in reached {
                changed.entry(c.clone()).or_insert_with(|| why.clone());
            }
            continue;
        }
        let readers = match classify(hooks, f, packages) {
            FileClass::Everything(why) => return Plan::Everything(why),
            FileClass::Package { name, readers } => {
                changed.insert(name, "changed".to_owned());
                readers
            }
            FileClass::Data(readers) => readers,
            FileClass::Docs | FileClass::NoBuild => Vec::new(),
        };
        for r in readers {
            if r.embeds {
                changed.entry(r.package).or_insert_with(|| format!("reads {f}"));
            } else {
                tested.entry(r.package).or_insert_with(|| format!("its tests read {f}"));
            }
        }
    }
    let (mut affected, built) = affected(&rules, crates, &changed, &tested);
    let build = runtime(&rules, &mut affected, &built, crates);
    Plan::Crates { crates: affected, build, files: files.len() }
}

/// The plan as text: the affected packages and why, and what is built first.
pub fn describe(hooks: &dyn Hooks, plan: &Plan, base: &str, all: usize) -> String {
    let rules = hooks.changed();
    match plan {
        Plan::Everything(why) => format!("ci --changed (base {base}): the full run: {why}\n"),
        Plan::Crates { crates, build, files } => {
            let mut s = format!("ci --changed (base {base}): {files} changed file(s), {} of {all} packages affected\n", crates.len());
            for (c, why) in crates {
                s.push_str(&format!("  {:<24} {why}\n", short_name(&rules, c)));
            }
            for (c, why) in build {
                s.push_str(&format!("  {:<24} built first, not tested: {why}\n", short_name(&rules, c)));
            }
            if crates.is_empty() {
                s.push_str("  (none: no build, clippy, test or wasm)\n");
            }
            s
        }
    }
}

/// A ref from the command line, checked before git sees it: never an option.
pub fn checked_ref<'a>(what: &str, r: &'a str) -> Result<&'a str, String> {
    if r.is_empty() || r.starts_with('-') || r.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("ci: `{r}` is not a valid {what} ref"));
    }
    Ok(r)
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut all = vec!["-c", "core.quotepath=off"];
    all.extend_from_slice(args);
    crate::process::git(root, &all, None, &[])
}

/// The merge base of `base` and `head` (else `HEAD`), the refs checked.
fn merge_base(root: &Path, base: &str, head: Option<&str>) -> Result<String, String> {
    let base = checked_ref("--base", base)?;
    let tip = head.map(|h| checked_ref("--head", h)).transpose()?.unwrap_or("HEAD");
    let mb = git(root, &["merge-base", base, tip]).map_err(|e| format!("{e} (is `{base}` fetched? pass --base <ref>)"))?;
    Ok(mb.trim().to_owned())
}

/// `file` at the merge base `mb`, and at `head` (or in the working tree).
fn both_sides(root: &Path, file: &str, mb: &str, head: Option<&str>) -> Result<(String, String), String> {
    let old = git(root, &["show", &format!("{mb}:{file}")])?;
    let new = match head {
        Some(h) => git(root, &["show", &format!("{}:{file}", checked_ref("--head", h)?)])?,
        None => std::fs::read_to_string(root.join(file)).map_err(|e| format!("{file}: {e}"))?,
    };
    Ok((old, new))
}

/// Files changed in the repository at `root` since the merge base with `base`: with `head`, those
/// the commits up to `head` change (a branch as it is pushed); without it, the working tree's
/// committed, staged, unstaged and untracked changes. Renames count as both paths.
pub fn changed_files(root: &Path, base: &str, head: Option<&str>) -> Result<Vec<String>, String> {
    let head = head.map(|h| checked_ref("--head", h)).transpose()?;
    let mb = merge_base(root, base, head)?;
    let mut files: BTreeSet<String> = match head {
        Some(h) => git(root, &["diff", "--name-only", "--no-renames", &mb, h])?,
        None => git(root, &["diff", "--name-only", "--no-renames", &mb])?,
    }
    .lines()
    .map(str::to_owned)
    .collect();
    if head.is_none() {
        files.extend(git(root, &["ls-files", "--others", "--exclude-standard"])?.lines().map(str::to_owned));
    }
    pin_bumps(root, &mut files, &mb, head);
    Ok(files.into_iter().filter(|f| !f.is_empty()).collect())
}

/// A bump of Swarm Core's pin is the change of what
/// depends on it, not everything's: `Cargo.toml` and `Cargo.lock` stand in as [`SWARM_CORE_PIN`]
/// when their diff from `mb` only moves the pin.
fn pin_bumps(root: &Path, files: &mut BTreeSet<String>, mb: &str, head: Option<&str>) {
    for f in ["Cargo.toml", LOCK] {
        if !files.contains(f) {
            continue;
        }
        let (old, new) = both_sides(root, f, mb, head).unwrap_or_default();
        if crate::pin::only_moves_the_pin(f, &old, &new) {
            files.remove(f);
            files.insert(SWARM_CORE_PIN.to_owned());
        }
    }
}

/// The workspace packages (of `crates`) a change of [`LOCK`] since the merge base with `base`
/// reaches, each with why ([`lock::reaches`]), when `files` changes it; `None` when it doesn't, or
/// when a lockfile can't be read (said: the plan is the full run then).
pub fn lock_for(root: &Path, files: &[String], base: &str, head: Option<&str>, crates: &[Crate]) -> Option<BTreeMap<String, String>> {
    if !files.iter().any(|f| f == LOCK) {
        return None;
    }
    let reached = merge_base(root, base, head)
        .and_then(|mb| both_sides(root, LOCK, &mb, head))
        .and_then(|(old, new)| lock::reaches(&old, &new, &crates.iter().map(|c| c.name.clone()).collect::<Vec<_>>()));
    reached.map_err(|e| eprintln!("warning: what the {LOCK} change reaches: {e}; planning everything")).ok()
}

/// The reads `found` ([`reads::scan`]) that the plan leaves out, for a consumer's test: for each
/// file a read names (`probes`: a directory's files, and a new one; [`reads::probes`]), the plan
/// must test the reader, and what depends on it when its code (not only its tests) reads it.
pub fn missed(
    hooks: &dyn Hooks,
    found: &[reads::Read],
    dirs: &[(String, String)],
    crates: &[Crate],
    probes: &dyn Fn(&reads::Read, &str) -> Vec<String>,
) -> Vec<String> {
    let rules = hooks.changed();
    let mut out = Vec::new();
    for r in found {
        let Some(target) = r.target.as_deref().filter(|t| !t.is_empty()) else {
            out.push(format!("{} reads {:?} ({}): the whole workspace or outside it; join what it reads", r.reader, r.target, r.at));
            continue;
        };
        for probe in probes(r, target) {
            let Plan::Crates { crates: planned, .. } = plan(hooks, std::slice::from_ref(&probe), dirs, crates, None) else { continue };
            let reader = BTreeMap::from([(r.reader.clone(), String::new())]);
            let want = if r.embeds { affected(&rules, crates, &reader, &BTreeMap::new()).0 } else { reader };
            let left: Vec<&String> = want.keys().filter(|p| !planned.contains_key(*p)).collect();
            if !left.is_empty() {
                out.push(format!("{} reads {probe} ({}), and the plan leaves out {left:?}: add it to the project's reads (`Hooks::changed`)", r.reader, r.at));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
