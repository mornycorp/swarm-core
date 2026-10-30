//! What a `Cargo.lock` change reaches: the workspace packages whose resolved
//! dependencies include a package whose lock entry changed, so `ci --changed` tests those rather
//! than everything.
//!
//! A package's entry is its version, source, checksum and dependency list. One added, removed or
//! changed is followed through the lockfile's graph, the new lockfile's for what it has now and
//! the old one's for what it had: a package reaches it if it is among its dependencies at any
//! depth (normal, build or dev: the lockfile doesn't tell them apart), or is the package itself.
//!
//! The lockfile is read by hand, not by a TOML parser: cargo writes it in one fixed shape
//! (`[[package]]` tables of `key = "value"` lines and a `dependencies = [ … ]` list), and
//! Swarm Core keeps to `serde_json` alone.

use std::collections::{BTreeMap, BTreeSet};

/// A lockfile's packages by `name version`: each one's entry as written.
type Packages = BTreeMap<String, Entry>;

#[derive(Debug, Default, PartialEq, Eq)]
struct Entry {
    source: Option<String>,
    checksum: Option<String>,
    deps: Vec<String>,
}

/// The quoted strings in `text`, in order.
fn quoted(text: &str) -> Vec<String> {
    text.split('"').skip(1).step_by(2).map(str::to_owned).collect()
}

/// One `[[package]]` table's text: its `name version` and entry.
fn package(table: &str, which: &str) -> Result<(String, Entry), String> {
    let (mut entry, mut name, mut version) = (Entry::default(), None, String::new());
    // The dependency list may span lines: everything between its `[` and the next `]`.
    let (fields, deps) = match table.split_once("dependencies = [") {
        Some((before, rest)) => (format!("{before}{}", rest.split_once(']').map_or("", |(_, after)| after)), rest.split(']').next().unwrap_or("")),
        None => (table.to_owned(), ""),
    };
    entry.deps = quoted(deps);
    for line in fields.lines() {
        let Some((key, value)) = line.split_once(" = ") else { continue };
        let value = quoted(value).into_iter().next();
        match key.trim() {
            "name" => name = value,
            "version" => version = value.unwrap_or_default(),
            "source" => entry.source = value,
            "checksum" => entry.checksum = value,
            _ => {}
        }
    }
    let name = name.ok_or_else(|| format!("the {which} Cargo.lock has a package without a name"))?;
    Ok((format!("{name} {version}"), entry))
}

fn packages(text: &str, which: &str) -> Result<Packages, String> {
    let tables: Vec<&str> = text.split("[[package]]").skip(1).collect();
    if tables.is_empty() {
        return Err(format!("the {which} Cargo.lock has no [[package]]"));
    }
    // A table ends where the next section (`[metadata]`, `[[patch.unused]]`) starts.
    tables.iter().map(|t| package(t.split("\n[").next().unwrap_or(t), which)).collect()
}

/// The `name version` a dependency names: `name version (source)` and `name version` name it
/// whole, a bare `name` the one package of that name.
fn resolve(dep: &str, all: &Packages) -> Option<String> {
    let mut words = dep.split(' ');
    let name = words.next()?;
    match words.next() {
        Some(version) => Some(format!("{name} {version}")),
        None => {
            let prefix = format!("{name} ");
            let mut found = all.keys().filter(|k| k.starts_with(&prefix));
            let only = found.next()?;
            found.next().is_none().then(|| only.clone())
        }
    }
}

/// The packages `start` depends on at any depth in `all`, `start` among them.
fn closure(start: &str, all: &Packages) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start.to_owned()];
    while let Some(key) = stack.pop() {
        if let Some(entry) = all.get(&key).filter(|_| seen.insert(key.clone())) {
            stack.extend(entry.deps.iter().filter_map(|d| resolve(d, all)));
        }
    }
    seen
}

/// The workspace packages (`members`, by name) that `old` → `new` reaches, each with why: the
/// first package (by name) whose entry changed among its dependencies, or its own entry. An error
/// when a lockfile can't be read: the caller plans everything then.
pub fn reaches(old: &str, new: &str, members: &[String]) -> Result<BTreeMap<String, String>, String> {
    let (old, new) = (packages(old, "base's")?, packages(new, "head's")?);
    let mut changed: BTreeMap<&str, &str> = BTreeMap::new();
    for (key, entry) in &new {
        match old.get(key) {
            None => changed.insert(key, "added"),
            Some(was) if was != entry => changed.insert(key, "changed"),
            Some(_) => None,
        };
    }
    for key in old.keys().filter(|k| !new.contains_key(*k)) {
        changed.insert(key, "removed");
    }
    Ok(members.iter().filter_map(|m| why_reached(m, &old, &new, &changed).map(|why| (m.clone(), why))).collect())
}

/// Why workspace package `member` is reached: the first package (by name) whose entry changed
/// among its dependencies in either lockfile, else its own entry's change; `None` if neither.
fn why_reached(member: &str, old: &Packages, new: &Packages, changed: &BTreeMap<&str, &str>) -> Option<String> {
    let is_member = |k: &str| k.split(' ').next() == Some(member);
    let (mut reached, mut own_changed) = (BTreeSet::new(), false);
    for all in [new, old] {
        let Some(key) = all.iter().find(|(k, e)| e.source.is_none() && is_member(k)).map(|(k, _)| k) else { continue };
        own_changed |= changed.contains_key(key.as_str());
        reached.extend(closure(key, all));
    }
    let dep = reached.iter().filter(|k| !is_member(k)).find_map(|k| changed.get(k.as_str()).map(|how| format!("Cargo.lock: {k} {how}")));
    dep.or_else(|| own_changed.then(|| "Cargo.lock: its own entry changed".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lockfile from `name version [deps…]` lines; a workspace member's version starts with `w`
    /// (no source), and a package's checksum is its version.
    fn lock(lines: &[&str]) -> String {
        let mut out = String::from("# This file is automatically @generated by Cargo.\nversion = 4\n");
        for line in lines {
            let mut words = line.split(' ');
            let (name, version) = (words.next().unwrap(), words.next().unwrap());
            out.push_str(&format!("\n[[package]]\nname = \"{name}\"\nversion = \"{}\"\n", version.trim_start_matches('w')));
            if !version.starts_with('w') {
                out.push_str(&format!("source = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{version}\"\n"));
            }
            let deps: Vec<String> = words.map(|d| format!(" \"{}\",\n", d.replace('@', " "))).collect();
            if !deps.is_empty() {
                out.push_str(&format!("dependencies = [\n{}]\n", deps.concat()));
            }
        }
        out
    }

    fn members() -> Vec<String> {
        ["app", "engine", "xtask"].map(str::to_owned).to_vec()
    }

    const BASE: &[&str] = &["app w1 engine serde", "engine w1 serde log", "xtask w1 toml", "serde 1.0", "log 0.4", "toml 1.0 serde"];

    #[test]
    fn the_lockfile_reads_as_cargo_writes_it() {
        let all = packages(&lock(&["app w1 engine serde@1.0", "serde 1.0"]), "base's").unwrap();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["app 1", "serde 1.0"]);
        assert_eq!(all["app 1"].deps, ["engine", "serde 1.0"]);
        assert_eq!((all["app 1"].source.as_deref(), all["serde 1.0"].checksum.as_deref()), (None, Some("1.0")));
        // A trailing table that isn't a package ends the last one.
        let with_patch = format!("{}\n[[patch.unused]]\nname = \"other\"\nversion = \"9\"\n", lock(&["serde 1.0"]));
        assert_eq!(packages(&with_patch, "base's").unwrap().keys().collect::<Vec<_>>(), ["serde 1.0"]);
        assert!(packages("[[package]]\nversion = \"1\"\n", "head's").unwrap_err().contains("without a name"));
    }

    #[test]
    fn an_unchanged_lockfile_reaches_nothing() {
        assert_eq!(reaches(&lock(BASE), &lock(BASE), &members()).unwrap(), BTreeMap::new());
    }

    #[test]
    fn an_added_dependency_reaches_its_user_alone() {
        let mut head = BASE.to_vec();
        head[2] = "xtask w1 toml sha2";
        head.push("sha2 0.10 digest");
        head.push("digest 0.10");
        let got = reaches(&lock(BASE), &lock(&head), &members()).unwrap();
        assert_eq!(got, BTreeMap::from([("xtask".to_owned(), "Cargo.lock: digest 0.10 added".to_owned())]));
    }

    #[test]
    fn a_bumped_package_reaches_every_crate_that_uses_it_at_any_depth() {
        let head = ["app w1 engine serde", "engine w1 serde log", "xtask w1 toml", "serde 1.0", "log 0.5", "toml 1.0 serde"];
        let got = reaches(&lock(BASE), &lock(&head), &members()).unwrap();
        // log 0.4 -> 0.5: engine uses it, and app through engine; xtask doesn't.
        assert_eq!(got.keys().collect::<Vec<_>>(), ["app", "engine"]);
        assert_eq!(got["engine"], "Cargo.lock: log 0.4 removed");
        // A package deeper down: toml reaches xtask; serde reaches every crate, through toml too.
        let head = ["app w1 engine serde", "engine w1 serde log", "xtask w1 toml", "serde 1.0", "log 0.4", "toml 1.1 serde"];
        assert_eq!(reaches(&lock(BASE), &lock(&head), &members()).unwrap().keys().collect::<Vec<_>>(), ["xtask"]);
        let head = ["app w1 engine serde", "engine w1 serde log", "xtask w1 toml", "serde 1.1", "log 0.4", "toml 1.0 serde"];
        assert_eq!(reaches(&lock(BASE), &lock(&head), &members()).unwrap().keys().collect::<Vec<_>>(), ["app", "engine", "xtask"], "serde, through toml too");
    }

    #[test]
    fn a_removed_dependency_reaches_the_crate_that_dropped_it() {
        let head = ["app w1 engine serde", "engine w1 serde", "xtask w1 toml", "serde 1.0", "toml 1.0 serde"];
        let got = reaches(&lock(BASE), &lock(&head), &members()).unwrap();
        assert_eq!(got.keys().collect::<Vec<_>>(), ["app", "engine"]);
        assert_eq!(got["engine"], "Cargo.lock: log 0.4 removed");
    }

    #[test]
    fn a_member_whose_own_entry_changed_is_reached() {
        let head = ["app w1 engine serde", "engine w2 serde log", "xtask w1 toml", "serde 1.0", "log 0.4", "toml 1.0 serde"];
        let got = reaches(&lock(BASE), &lock(&head), &members()).unwrap();
        assert_eq!(got["engine"], "Cargo.lock: its own entry changed");
        assert_eq!(got["app"], "Cargo.lock: engine 1 removed", "its dependents, through the entry");
        assert!(!got.contains_key("xtask"));
    }

    #[test]
    fn two_versions_of_a_package_are_told_apart_and_a_bad_lockfile_is_an_error() {
        let base = ["app w1 serde@1.0", "engine w1 serde@2.0", "xtask w1", "serde 1.0", "serde 2.0"];
        let head = ["app w1 serde@1.0", "engine w1 serde@2.1", "xtask w1", "serde 1.0", "serde 2.1"];
        assert_eq!(reaches(&lock(&base), &lock(&head), &members()).unwrap().keys().collect::<Vec<_>>(), ["engine"]);
        assert!(reaches("not toml [", &lock(BASE), &members()).unwrap_err().contains("base's"));
        assert!(reaches(&lock(BASE), "version = 4\n", &members()).unwrap_err().contains("no [[package]]"));
    }

    /// A Swarm Core pin's move, as cargo writes it: the git source's commit changes, so what
    /// depends on it is reached (the plan takes such a bump as Swarm Core's, [`super::super::SWARM_CORE_PIN`]).
    #[test]
    fn a_git_dependencys_new_commit_reaches_its_users() {
        let entry = |sha: &str| format!("\n[[package]]\nname = \"swarm-core\"\nversion = \"0.1.0\"\nsource = \"git+ssh://x/y.git?rev={sha}#{sha}\"\n");
        let base = format!("{}{}", lock(&["xtask w1 swarm-core", "app w1"]), entry("aaaa"));
        let head = format!("{}{}", lock(&["xtask w1 swarm-core", "app w1"]), entry("bbbb"));
        let got = reaches(&base, &head, &members()).unwrap();
        assert_eq!(got, BTreeMap::from([("xtask".to_owned(), "Cargo.lock: swarm-core 0.1.0 changed".to_owned())]));
    }
}
