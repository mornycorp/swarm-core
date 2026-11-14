//! [`layers`]: the workspace's structure, from its manifests and crate roots (the contract's
//! Layers section, `docs/app-architecture.md`).
//!
//! - **Declared.** Every member's `[package.metadata.swarm] layer = "<name>"` is one of the
//!   [`Layers::stack`], or [`Layers::tooling`] (`xtask`) or [`Layers::testkit`], which sit outside
//!   the stack.
//! - **Downward.** A dependency (normal or build) of a stack crate is a crate in a strictly lower
//!   layer, so no one depends on an app, on tooling or on a crate of its own layer. A dev-dependency
//!   is free of the order; tooling and the testkit may use any layer.
//! - **`backends`**: the layer between `domain` and `engine`, the one that touches the outside
//!   (child processes, files, git, the network, a device). A workspace need not have a crate in it. A manifest that names it `sources`, its
//!   name until mornycorp/swarm-core#87, is read as `backends` ([`Layers::aliases`]). A workspace with no crate in it checks
//!   as it did before the layer was added: a manifest names its layer, never a position, so nothing
//!   else moves. `domain` sits below it, so a domain crate can't reach the outside through it.
//! - **Toolkits** ([`Layers::toolkits`]: egui, winit, …) are linked only by the UI layer and the
//!   apps built on it (an app that depends, through workspace crates, on a UI crate). The CLI and
//!   every layer below never link one.
//! - **The testkit** is only a `[dev-dependencies]` entry (tooling excepted).
//! - **Lints and headers.** `[lints] workspace = true` in every member, and every crate root
//!   carries the headers ([`headers`]).
//! - **No committed `[patch]`** of Swarm Core, nor a `[replace]`, nor a `path` or `branch` pin,
//!   in the root manifest, a member's or a cargo config (`.cargo/config.toml`, `.cargo/config`), and
//!   the testkit at Swarm Core's `rev` ([`crate::pin`]).

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use toml::Value;

use super::manifest::{self, Dep, Kind, Member, line_of_key};
use super::{Finding, Findings, headers, read};

/// What an app's layers are called and what each may do: [`Layers::STANDARD`] is the contract's,
/// and an app changes only what differs (`Layers { testkit: &["x-testkit"], ..Layers::STANDARD }`).
#[derive(Debug, Clone, Copy)]
pub struct Layers {
    /// The layers, lowest first. A crate depends only on a lower one.
    pub stack: &'static [&'static str],
    /// Earlier names of layers, each with the name in `stack` it is read as: a manifest that
    /// still declares the old one keeps passing. One that maps to no layer of `stack` does
    /// nothing.
    pub aliases: &'static [(&'static str, &'static str)],
    /// The UI layer, which holds toolkit code: one of `stack`.
    pub ui: &'static str,
    /// The apps' layer, the top of `stack`: an app links a toolkit only when it is built on a UI
    /// crate.
    pub apps: &'static str,
    /// The layer name of the repo's tooling (`xtask`), outside the stack.
    pub tooling: &'static str,
    /// The layer name of the testkit, outside the stack: only ever a dev-dependency.
    pub testkit: &'static str,
    /// The UI toolkits, by package name. [`Layers::STANDARD`]'s are the skeleton's (egui and what
    /// comes with it); the contract picks none, so an app that draws with another names its own
    /// (`Layers { toolkits: &["iced"], ..Layers::STANDARD }`).
    pub toolkits: &'static [&'static str],
    /// The lints every stack crate's root denies (`clippy::unwrap_used`, …).
    pub deny: &'static [&'static str],
    /// The lints the roots of tooling and the testkit deny: they may panic in a helper.
    pub outside_deny: &'static [&'static str],
    /// The library packages allowed `unsafe`, each with its reason: its library root carries
    /// [`UNSAFE_LINES`] in place of `#![forbid(unsafe_code)]`. The default: none.
    pub unsafe_crates: &'static [Unsafe],
}

/// A library package allowed `unsafe`, and what its library root carries in place of
/// `#![forbid(unsafe_code)]`: the table path's ([`crate::layers`]), one type for both.
pub use crate::layers::{UNSAFE_LINES, Unsafe};

impl Layers {
    /// The contract's layers (`model`, `domain`, `backends`, `engine`, `automation`,
    /// `ui`, `apps`), the UI toolkits both reference apps use, and the deny list of the
    /// Swarm Core's own crate root.
    pub const STANDARD: Layers = Layers {
        stack: &["model", "domain", "backends", "engine", "automation", "ui", "apps"],
        aliases: &[("sources", "backends")],
        ui: "ui",
        apps: "apps",
        tooling: "tooling",
        testkit: "testkit",
        toolkits: &["egui", "eframe", "egui_extras", "egui_kittest", "winit", "rfd", "muda"],
        deny: &["clippy::unwrap_used", "clippy::expect_used", "clippy::panic", "clippy::unimplemented", "clippy::todo", "clippy::unreachable"],
        outside_deny: &["clippy::unwrap_used", "clippy::expect_used"],
        unsafe_crates: &[],
    };
}

impl Default for Layers {
    fn default() -> Layers {
        Layers::STANDARD
    }
}

/// What [`layers`] checked.
#[derive(Debug, Clone)]
pub struct LayersReport {
    pub members: usize,
    pub took: Duration,
}

impl fmt::Display for LayersReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "layers: {} packages, all in order", self.members)
    }
}

/// Where the layers' own setup is: a finding about it names this, not a file of the workspace.
/// It is the project's `Hooks::architecture` and `Hooks::unsafe_crates`, in its xtask.
pub const CONFIGURED: &str = "xtask (Hooks::architecture)";

/// Where a member sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// An index into [`Layers::stack`].
    Stack(usize),
    Tooling,
    Testkit,
}

/// The workspace at `root` held to `config`: every finding, or what was checked.
pub fn layers(root: &Path, config: &Layers) -> Result<LayersReport, Findings> {
    let (files, listing) = manifest::files(root);
    layers_files(root, config, &files, listing)
}

/// [`layers`] over `files` (every file of the workspace, from [`manifest::files`]), with the
/// `found` so far.
pub(crate) fn layers_files(root: &Path, config: &Layers, files: &[String], mut found: Vec<Finding>) -> Result<LayersReport, Findings> {
    let start = Instant::now();
    let (members, top, problems) = manifest::members(root, files);
    found.extend(problems);
    found.extend(config_problems(config));
    found.extend(unknown_unsafe(config, &members, files));
    let workspace_deps = top.get("workspace").and_then(Value::as_table).and_then(|w| w.get("dependencies")).and_then(Value::as_table);
    let places: Vec<Option<Place>> = members.iter().map(|m| declared(m, config, &mut found)).collect();
    let all = Workspace { config, members: &members, places: &places, workspace_deps };
    for (k, member) in members.iter().enumerate() {
        all.dependencies(k, &mut found);
        found.extend(lints_and_headers(member, places.get(k).copied().flatten(), config, files, root));
    }
    found.extend(swarm_core_pin(root, &top, &members));
    if found.is_empty() { Ok(LayersReport { members: members.len(), took: start.elapsed() }) } else { Err(Findings(found)) }
}

/// A config whose `ui` or `apps` isn't one of its `stack` can't be applied.
fn config_problems(config: &Layers) -> Vec<Finding> {
    [("ui", config.ui), ("apps", config.apps)]
        .into_iter()
        .filter(|(_, name)| !config.stack.contains(name))
        .map(|(what, name)| {
            Finding::new(
                CONFIGURED,
                None,
                "layers-config",
                format!("the {what} layer `{name}` is not one of the stack {:?}", config.stack),
                "fix the app's `Layers`",
            )
        })
        .collect()
}

/// A package [`Layers::unsafe_crates`] lists that is no member, or has no library: its exception
/// would excuse nothing.
fn unknown_unsafe(config: &Layers, members: &[Member], files: &[String]) -> Vec<Finding> {
    let problem = |u: &Unsafe, what: &str| {
        let fix = "fix the app's `Layers::unsafe_crates`: it names a library member, by its package name";
        Finding::new(CONFIGURED, None, "unsafe-crates", format!("`unsafe_crates` lists {}, {what}", u.package), fix)
    };
    config
        .unsafe_crates
        .iter()
        .filter_map(|u| match members.iter().find(|m| m.name == u.package) {
            None => Some(problem(u, "which is not a workspace member")),
            Some(m) if headers::library(m, files).is_none() => Some(problem(u, "which has no library: `unsafe` is allowed in a library only")),
            Some(_) => None,
        })
        .collect()
}

/// The layer `member` declares, or the finding that it doesn't (or names an unknown one).
fn declared(member: &Member, config: &Layers, found: &mut Vec<Finding>) -> Option<Place> {
    let names = || format!("{}, {} or {}", config.stack.join(", "), config.tooling, config.testkit);
    let value = member.table.get("package").and_then(Value::as_table).and_then(|p| p.get("metadata")).and_then(Value::as_table);
    let layer = value.and_then(|m| m.get("swarm")).and_then(Value::as_table).and_then(|s| s.get("layer"));
    let line = line_of_key(&member.text, "package.metadata.swarm", "layer");
    let fix = |what: &str| format!("add `[package.metadata.swarm]` with `layer = \"{what}\"`, one of {}", names());
    let place = match layer {
        None => {
            found.push(Finding::new(&member.manifest, None, "layer-declared", format!("{} declares no layer", member.name), fix("engine")));
            return None;
        }
        Some(Value::String(name)) => place_of(config, name),
        Some(_) => None,
    };
    if place.is_none() {
        let problem = format!("{} declares a layer that isn't a name of {}", member.name, names());
        found.push(Finding::new(
            &member.manifest,
            line,
            "layer-declared",
            problem,
            "write the layer as a string naming one of them (a number was the old form)",
        ));
    }
    place
}

/// The place of layer `name`.
fn place_of(config: &Layers, name: &str) -> Option<Place> {
    if name == config.tooling {
        Some(Place::Tooling)
    } else if name == config.testkit {
        Some(Place::Testkit)
    } else {
        // A name in the stack is that layer, never an alias: an app's own stack may name `sources`.
        let position = |name: &str| config.stack.iter().position(|l| *l == name);
        position(name).or_else(|| config.aliases.iter().find(|(old, _)| *old == name).and_then(|(_, now)| position(now))).map(Place::Stack)
    }
}

/// The workspace as the dependency rules see it.
struct Workspace<'a> {
    config: &'a Layers,
    members: &'a [Member],
    places: &'a [Option<Place>],
    workspace_deps: Option<&'a toml::Table>,
}

impl Workspace<'_> {
    /// The index and place of the member called `package`.
    fn find(&self, package: &str) -> Option<(usize, Place)> {
        let k = self.members.iter().position(|m| m.name == package)?;
        Some((k, self.places.get(k).copied().flatten()?))
    }

    fn name_of(&self, place: Place) -> String {
        match place {
            Place::Stack(i) => self.config.stack.get(i).map_or("?", |s| s).to_owned(),
            Place::Tooling => self.config.tooling.to_owned(),
            Place::Testkit => self.config.testkit.to_owned(),
        }
    }

    /// Whether member `k` depends, directly or through workspace crates, on a UI-layer crate.
    fn built_on_ui(&self, k: usize) -> bool {
        let ui = self.config.stack.iter().position(|l| *l == self.config.ui).map(Place::Stack);
        let (mut seen, mut todo) = (BTreeSet::new(), vec![k]);
        while let Some(next) = todo.pop() {
            let Some(member) = self.members.get(next) else { continue };
            for dep in member.deps(self.workspace_deps).iter().filter(|d| d.kind != Kind::Dev) {
                if let Some((at, place)) = self.find(&dep.package).filter(|(at, _)| seen.insert(*at)) {
                    if Some(place) == ui {
                        return true;
                    }
                    todo.push(at);
                }
            }
        }
        false
    }

    /// The findings on member `k`'s dependencies.
    fn dependencies(&self, k: usize, found: &mut Vec<Finding>) {
        let (Some(member), Some(place)) = (self.members.get(k), self.places.get(k).copied().flatten()) else { return };
        for dep in member.deps(self.workspace_deps) {
            if dep.kind != Kind::Dev && self.config.toolkits.contains(&dep.package.as_str()) {
                found.extend(self.toolkit(k, place, &dep));
            }
            if let Some((_, target_place)) = self.find(&dep.package).filter(|(t, _)| *t != k) {
                found.extend(self.order(member, place, &dep, target_place));
                found.extend(self.testkit(member, place, &dep, target_place));
            }
        }
    }

    /// A toolkit dependency outside the UI layer and the apps built on it.
    fn toolkit(&self, k: usize, place: Place, dep: &Dep) -> Option<Finding> {
        let member = self.members.get(k)?;
        let ui = self.config.stack.iter().position(|l| *l == self.config.ui);
        let apps = self.config.stack.iter().position(|l| *l == self.config.apps);
        let allowed = match place {
            Place::Stack(i) => Some(i) == ui || (Some(i) == apps && self.built_on_ui(k)),
            Place::Tooling | Place::Testkit => false,
        };
        let problem = format!("{} ({}) depends on {}, a UI toolkit", member.name, self.name_of(place), dep.package);
        let fix = format!("only the {} layer and the apps built on it link a toolkit: move the code there, or drop the dependency", self.config.ui);
        (!allowed).then(|| Finding::new(&member.manifest, member.line_of_dep(dep), "ui-toolkit", problem, fix))
    }

    /// A dependency that doesn't point to a strictly lower layer.
    fn order(&self, member: &Member, place: Place, dep: &Dep, target: Place) -> Option<Finding> {
        let up = |why: &str| {
            let problem = format!("{} ({}) depends on {} ({}): {why}", member.name, self.name_of(place), dep.package, self.name_of(target));
            Some(Finding::new(
                &member.manifest,
                member.line_of_dep(dep),
                "layer-order",
                problem,
                "move what it needs into a lower layer, or invert the dependency",
            ))
        };
        match (place, target) {
            _ if dep.kind == Kind::Dev => None,
            (_, Place::Testkit) => None,
            // A repo's tooling may be several crates (an xtask and a library it shares): they may
            // use each other. No crate of the stack, nor the testkit, uses tooling.
            (Place::Tooling, Place::Tooling) => None,
            (_, Place::Tooling) => up("tooling is nobody's dependency but tooling's"),
            (Place::Stack(mine), Place::Stack(theirs)) if theirs >= mine => up("only strictly lower layers"),
            _ => None,
        }
    }

    /// The testkit as a normal or build dependency.
    fn testkit(&self, member: &Member, place: Place, dep: &Dep, target_place: Place) -> Option<Finding> {
        if target_place != Place::Testkit || dep.kind == Kind::Dev || place == Place::Tooling {
            return None;
        }
        let problem = format!("{} depends on {}, the testkit, outside [dev-dependencies]", member.name, dep.package);
        let fix = format!("move it to `[dev-dependencies]`: the testkit is never a dependency of production code ({})", Kind::Dev.section());
        Some(Finding::new(&member.manifest, member.line_of_dep(dep), "testkit-dev-only", problem, fix))
    }
}

/// `[lints] workspace = true` in `member`'s manifest, and the headers of each of its crate roots.
fn lints_and_headers(member: &Member, place: Option<Place>, config: &Layers, files: &[String], root: &Path) -> Vec<Finding> {
    let mut found = Vec::new();
    if !member.lints_workspace() {
        let line = member.text.lines().position(|l| l.trim() == "[package]").map(|n| n + 1);
        found.push(Finding::new(
            &member.manifest,
            line,
            "workspace-lints",
            format!("{} has no `[lints] workspace = true`", member.name),
            "add `[lints]` with `workspace = true`",
        ));
    }
    let deny = if matches!(place, Some(Place::Tooling | Place::Testkit)) { config.outside_deny } else { config.deny };
    let unsafe_lib = config.unsafe_crates.iter().any(|u| u.package == member.name).then(|| headers::library(member, files)).flatten();
    for shown in headers::roots(member, files) {
        let safety = if unsafe_lib.as_deref() == Some(shown.as_str()) { headers::Safety::Allowed } else { headers::Safety::Forbidden };
        match read(root, &shown) {
            Ok(text) => found.extend(headers::check(&shown, &text, deny, safety)),
            Err(finding) => found.push(finding),
        }
    }
    found
}

/// Whether `line` names `word` whole: `swarm-core` is not named by `swarm-core-testkit`.
fn names(line: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    line.match_indices(word).any(|(at, m)| {
        let before = line.get(..at).and_then(|h| h.chars().next_back());
        let after = line.get(at + m.len()..).and_then(|r| r.chars().next());
        !before.is_some_and(part) && !after.is_some_and(part)
    })
}

/// The committed overrides of Swarm Core's pin ([`crate::pin`]): in the root manifest (`top`,
/// its dependencies, `[workspace.dependencies]`, `[patch]`, `[replace]`, the testkit's rev), each
/// member's dependencies, and the cargo configs (`.cargo/config.toml`, the legacy `.cargo/config`).
/// A config that exists but can't be read or parsed is a finding: what it overrides can't be told.
pub(crate) fn swarm_core_pin(root: &Path, top: &toml::Table, members: &[Member]) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut overrides = vec![(read(root, "Cargo.toml").unwrap_or_default(), crate::pin::manifest_overrides("Cargo.toml", top))];
    overrides.extend(members.iter().filter(|m| !m.dir.is_empty()).map(|m| (m.text.clone(), crate::pin::dependency_overrides(&m.manifest, &m.table))));
    for file in [".cargo/config.toml", ".cargo/config"] {
        if std::fs::symlink_metadata(root.join(file)).is_err() {
            continue;
        }
        match read(root, file).map(|text| (text.parse::<toml::Table>(), text)) {
            Err(finding) => found.push(finding),
            Ok((Ok(config), text)) => overrides.push((text, crate::pin::config_overrides(file, &config))),
            Ok((Err(e), text)) => {
                let line = e.span().map(|s| text.get(..s.start).unwrap_or("").matches('\n').count() + 1);
                let problem = format!("{file} is not valid TOML ({}), so whether it overrides Swarm Core's pin can't be told", e.message().trim());
                found.push(Finding::new(file, line, "swarm-core-patch", problem, "fix the config"));
            }
        }
    }
    for (text, list) in overrides {
        for o in list {
            let line = text.lines().position(|l| names(l, o.near)).map(|n| n + 1);
            found.push(Finding::new(&o.file, line, o.rule, format!("{} {}", o.file, o.problem), o.fix));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use swarm_core_testkit::temp_dir;

    const HEADER: &str = "#![forbid(unsafe_code)]\n#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]\n\
                          #![warn(clippy::cognitive_complexity, clippy::too_many_lines)]\n#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]\n";

    /// A manifest for crate `name` in layer `layer` with `deps` (raw TOML lines) under `[dependencies]` and `extra` after.
    fn manifest(name: &str, layer: &str, deps: &str, extra: &str) -> String {
        format!("[package]\nname = \"{name}\"\n\n[package.metadata.swarm]\nlayer = \"{layer}\"\n\n[lints]\nworkspace = true\n\n[dependencies]\n{deps}\n{extra}")
    }

    /// A fixture workspace: each `(dir, manifest)` a member with a headed `src/lib.rs`.
    fn workspace(name: &str, crates: &[(&str, String)]) -> PathBuf {
        let dir = temp_dir(name);
        let members: Vec<String> = crates.iter().map(|(d, _)| format!("\"{d}\"")).collect();
        std::fs::write(dir.join("Cargo.toml"), format!("[workspace]\nmembers = [{}]\n", members.join(", "))).unwrap();
        for (d, m) in crates {
            std::fs::create_dir_all(dir.join(d).join("src")).unwrap();
            std::fs::write(dir.join(d).join("Cargo.toml"), m).unwrap();
            std::fs::write(dir.join(d).join("src/lib.rs"), format!("{HEADER}pub fn f() {{}}\n")).unwrap();
        }
        dir
    }

    fn rules(root: &Path) -> Vec<(&'static str, String)> {
        layers(root, &Layers::STANDARD).err().map(|f| f.0.into_iter().map(|f| (f.rule, f.to_string())).collect()).unwrap_or_default()
    }

    /// #51: the toolkit is the app's choice, not the contract's. An app that draws with another
    /// names it in its `Layers`, and the rule holds that one: its toolkit below the UI layer is
    /// a finding, and the standard list's names are then ordinary crates.
    #[test]
    fn an_app_names_its_own_toolkit_and_the_rule_holds_that_one() {
        let theirs = Layers { toolkits: &["iced", "iced_wgpu"], ..Layers::STANDARD };
        let found = |name: &str, engine_dep: &str| {
            let mut crates = well_formed();
            crates.retain(|(d, _)| *d != "crates/engine" && *d != "crates/ui");
            crates.push((
                "crates/engine",
                manifest("app-engine", "engine", &format!("app-domain = {{ path = \"../domain\" }}\napp-model = {{ path = \"../model\" }}\n{engine_dep}"), ""),
            ));
            crates.push(("crates/ui", manifest("app-ui", "ui", "app-engine = { path = \"../engine\" }\niced = \"1\"", "")));
            let root = workspace(name, &crates);
            layers(&root, &theirs).err().map(|f| f.0.iter().map(|f| (f.rule, f.problem.clone())).collect::<Vec<_>>()).unwrap_or_default()
        };
        assert!(found("layers-own-toolkit", "").is_empty(), "the UI crate links the app's own toolkit: {:?}", found("layers-own-toolkit-again", ""));
        let low = found("layers-own-toolkit-low", "iced_wgpu = \"1\"");
        assert!(low.iter().any(|(r, p)| *r == "ui-toolkit" && p.contains("app-engine (engine) depends on iced_wgpu, a UI toolkit")), "{low:?}");
        assert!(found("layers-own-toolkit-egui", "egui = \"1\"").is_empty(), "egui is no toolkit of this app's");
    }

    /// #63: a repo's tooling may be several crates that use each other; no crate of the stack
    /// uses one. And a finding about the configuration names the xtask, not a manifest.
    #[test]
    fn tooling_may_use_tooling_and_the_stack_may_not() {
        let mut crates = well_formed();
        crates.push(("tools/shared", manifest("app-tools", "tooling", "", "")));
        crates.push(("tools/extra", manifest("app-extra", "tooling", "app-tools = { path = \"../shared\" }", "")));
        assert!(rules(&workspace("layers-tooling", &crates)).is_empty(), "{:?}", rules(&workspace("layers-tooling", &crates)));
        crates.push(("crates/low", manifest("app-low", "model", "app-tools = { path = \"../../tools/shared\" }", "")));
        let found = rules(&workspace("layers-tooling-up", &crates));
        assert!(found.iter().any(|(r, m)| *r == "layer-order" && m.contains("tooling is nobody's dependency but tooling's")), "{found:?}");
        assert!(names("swarm-core = { path = \"x\" }", "swarm-core") && !names("swarm-core-testkit = { path = \"x\" }", "swarm-core"));
    }

    fn well_formed() -> Vec<(&'static str, String)> {
        vec![
            ("crates/model", manifest("app-model", "model", "serde = \"1\"", "")),
            ("crates/domain", manifest("app-domain", "domain", "app-model = { path = \"../model\" }", "")),
            ("crates/engine", manifest("app-engine", "engine", "app-domain = { path = \"../domain\" }\napp-model = { path = \"../model\" }", "")),
            ("crates/automation", manifest("app-automation", "automation", "app-engine = { path = \"../engine\" }", "")),
            ("crates/ui", manifest("app-ui", "ui", "app-engine = { path = \"../engine\" }\negui = \"1\"", "")),
            (
                "apps/cli",
                manifest(
                    "app-cli",
                    "apps",
                    "app-automation = { path = \"../../crates/automation\" }",
                    "[dev-dependencies]\napp-testkit = { path = \"../../crates/testkit\" }\n",
                ),
            ),
            ("apps/gui", manifest("app-gui", "apps", "app-ui = { path = \"../../crates/ui\" }\neframe = \"1\"", "")),
            ("crates/testkit", manifest("app-testkit", "testkit", "app-model = { path = \"../model\" }", "")),
            ("xtask", manifest("xtask", "tooling", "app-engine = { path = \"../crates/engine\" }\napp-testkit = { path = \"../crates/testkit\" }", "")),
        ]
    }

    #[test]
    fn a_workspace_that_keeps_the_contract_passes() {
        let dir = workspace("layers-pass", &well_formed());
        let report = layers(&dir, &Layers::STANDARD).unwrap();
        assert_eq!(report.members, 9);
        assert!(report.to_string().starts_with("layers: 9 packages, all in order"), "{report}");
    }

    /// `well_formed` with crate `dir`'s manifest replaced.
    fn changed(dir: &str, manifest: String) -> Vec<(&'static str, String)> {
        well_formed().into_iter().map(|(d, m)| if d == dir { (d, manifest.clone()) } else { (d, m) }).collect()
    }

    #[test]
    fn a_member_must_declare_a_known_layer() {
        let bare = "[package]\nname = \"app-model\"\n[lints]\nworkspace = true\n".to_owned();
        let dir = workspace("layers-undeclared", &changed("crates/model", bare));
        let found = rules(&dir);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].1.starts_with("crates/model/Cargo.toml: [layer-declared] app-model declares no layer. Fix: add `[package.metadata.swarm]`"),
            "{found:?}"
        );
        for (bad, line) in [("layer = \"core\"", 5), ("layer = 3", 5)] {
            let m = manifest("app-model", "model", "", "").replace("layer = \"model\"", bad);
            let found = rules(&workspace("layers-unknown", &changed("crates/model", m)));
            assert_eq!(found.len(), 1, "{bad}: {found:?}");
            assert!(
                found[0]
                    .1
                    .starts_with(&format!("crates/model/Cargo.toml:{line}: [layer-declared] app-model declares a layer that isn't a name of model, domain")),
                "{found:?}"
            );
        }
    }

    #[test]
    fn dependencies_point_only_to_strictly_lower_layers() {
        let up = manifest("app-model", "model", "app-domain = { path = \"../domain\" }", "");
        let found = rules(&workspace("layers-up", &changed("crates/model", up)));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].1.starts_with("crates/model/Cargo.toml:11: [layer-order] app-model (model) depends on app-domain (domain): only strictly lower layers. Fix: move what it needs"), "{found:?}");
        let same = manifest("app-domain", "domain", "app-model = { path = \"../model\" }\napp-domain2 = { path = \"../domain2\" }", "");
        let mut crates = changed("crates/domain", same);
        crates.push(("crates/domain2", manifest("app-domain2", "domain", "app-domain = { path = \"../domain\" }", "")));
        let found = rules(&workspace("layers-same", &crates));
        assert!(found.iter().any(|(r, m)| *r == "layer-order" && m.contains("app-domain (domain) depends on app-domain2 (domain)")), "{found:?}");
        // An app is no one's dependency, and neither is tooling; apps may not use apps.
        let on_app = manifest("app-engine", "engine", "app-cli = { path = \"../../apps/cli\" }", "");
        assert!(
            rules(&workspace("layers-onapp", &changed("crates/engine", on_app)))
                .iter()
                .any(|(r, m)| *r == "layer-order" && m.contains("app-engine (engine) depends on app-cli (apps)"))
        );
        let on_tool = manifest("app-gui", "apps", "xtask = { path = \"../../xtask\" }", "");
        assert!(
            rules(&workspace("layers-ontool", &changed("apps/gui", on_tool)))
                .iter()
                .any(|(r, m)| *r == "layer-order" && m.contains("tooling is nobody's dependency"))
        );
    }

    /// `well_formed` with a `backends` crate, which the engine reads the outside through.
    fn with_backends() -> Vec<(&'static str, String)> {
        let engine = manifest("app-engine", "engine", "app-domain = { path = \"../domain\" }\napp-backends = { path = \"../backends\" }", "");
        let mut crates = changed("crates/engine", engine);
        crates
            .push(("crates/backends", manifest("app-backends", "backends", "app-domain = { path = \"../domain\" }\napp-model = { path = \"../model\" }", "")));
        crates
    }

    #[test]
    fn a_manifest_that_still_says_sources_is_read_as_backends() {
        // mornycorp/swarm-core#87: the layer was `sources`; a consumer's manifests pass unchanged,
        // and are held to the same order.
        let old: Vec<(&'static str, String)> =
            with_backends().into_iter().map(|(dir, text)| (dir, text.replace("layer = \"backends\"", "layer = \"sources\""))).collect();
        assert!(old.iter().any(|(_, text)| text.contains("layer = \"sources\"")));
        assert!(layers(&workspace("layers-sources-alias", &old), &Layers::STANDARD).is_ok());
        let up = manifest("app-backends", "sources", "app-domain = { path = \"../domain\" }\napp-engine = { path = \"../engine\" }", "");
        let mut crates = with_backends();
        crates.retain(|(d, _)| *d != "crates/backends");
        crates.push(("crates/backends", up));
        let found = rules(&workspace("layers-sources-alias-up", &crates));
        assert!(found.iter().any(|(rule, m)| *rule == "layer-order" && m.contains("app-backends (backends) depends on app-engine")), "{found:?}");
    }

    #[test]
    fn the_backends_layer_sits_between_the_domain_and_the_engine() {
        assert_eq!(Layers::STANDARD.stack, ["model", "domain", "backends", "engine", "automation", "ui", "apps"]);
        // Without it (`well_formed`, the pass above) and with it, the contract is kept.
        let report = layers(&workspace("layers-backends", &with_backends()), &Layers::STANDARD).unwrap();
        assert_eq!(report.members, 10);
        // The engine, the transports and the apps may use it; the apps may skip the engine for it.
        let cli =
            manifest("app-cli", "apps", "app-automation = { path = \"../../crates/automation\" }\napp-backends = { path = \"../../crates/backends\" }", "");
        let mut crates = with_backends();
        crates.retain(|(d, _)| *d != "apps/cli");
        crates.push(("apps/cli", cli));
        assert!(rules(&workspace("layers-backends-app", &crates)).is_empty());
    }

    #[test]
    fn the_domain_and_the_model_never_reach_the_outside_through_backends() {
        for (dir, name, layer, deps) in [
            ("crates/domain", "app-domain", "domain", "app-model = { path = \"../model\" }\napp-backends = { path = \"../backends\" }"),
            ("crates/model", "app-model", "model", "serde = \"1\"\napp-backends = { path = \"../backends\" }"),
        ] {
            let mut crates = with_backends();
            crates.retain(|(d, _)| *d != dir);
            crates.push((dir, manifest(name, layer, deps, "")));
            let found = rules(&workspace("layers-backends-down", &crates));
            // The domain's is a cycle too (backends uses the domain); the order finds it all the same.
            let want = format!("{dir}/Cargo.toml:12: [layer-order] {name} ({layer}) depends on app-backends (backends): only strictly lower layers");
            assert!(found.iter().any(|(r, m)| *r == "layer-order" && m.starts_with(&want)), "{want} in {found:?}");
        }
    }

    #[test]
    fn backends_uses_only_the_model_and_the_domain_and_no_toolkit() {
        let up = manifest("app-backends", "backends", "app-domain = { path = \"../domain\" }\napp-engine = { path = \"../engine\" }", "");
        let mut crates = with_backends();
        crates.retain(|(d, _)| *d != "crates/backends");
        crates.push(("crates/backends", up));
        let found = rules(&workspace("layers-backends-up", &crates));
        assert!(
            found.iter().any(|(r, m)| *r == "layer-order"
                && m.starts_with("crates/backends/Cargo.toml:12: [layer-order] app-backends (backends) depends on app-engine (engine)")),
            "{found:?}"
        );
        let toolkit = manifest("app-backends", "backends", "app-domain = { path = \"../domain\" }\nrfd = \"1\"", "");
        let mut crates = with_backends();
        crates.retain(|(d, _)| *d != "crates/backends");
        crates.push(("crates/backends", toolkit));
        let found = rules(&workspace("layers-backends-toolkit", &crates));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].1.starts_with("crates/backends/Cargo.toml:12: [ui-toolkit] app-backends (backends) depends on rfd, a UI toolkit"), "{found:?}");
    }

    #[test]
    fn an_app_that_names_its_own_stack_checks_as_before() {
        // An app that had a layer of its own there before the contract did (`source`) keeps its
        // `Layers`: its manifests stay valid, and the contract's name is not one of its layers.
        let own = Layers { stack: &["model", "domain", "source", "engine", "automation", "ui", "apps"], ..Layers::STANDARD };
        let source = manifest("app-source", "source", "app-domain = { path = \"../domain\" }", "");
        let engine = manifest("app-engine", "engine", "app-domain = { path = \"../domain\" }\napp-source = { path = \"../source\" }", "");
        let mut crates = changed("crates/engine", engine);
        crates.push(("crates/source", source));
        assert!(layers(&workspace("layers-own-stack", &crates), &own).is_ok());
        let dir = workspace("layers-own-stack-backends", &with_backends());
        let found = layers(&dir, &own).err().map(|f| f.0.into_iter().map(|f| f.to_string()).collect::<Vec<_>>()).unwrap_or_default();
        assert!(found.iter().any(|m| m.starts_with("crates/backends/Cargo.toml:5: [layer-declared] app-backends declares a layer that isn't")), "{found:?}");
        // The contract's alias maps to a layer this stack doesn't have, so it does nothing here;
        // and a stack that names `sources` itself keeps it: a name in the stack is never an alias.
        let said_sources: Vec<(&'static str, String)> =
            with_backends().into_iter().map(|(dir, text)| (dir, text.replace("layer = \"backends\"", "layer = \"sources\""))).collect();
        let found = layers(&workspace("layers-own-stack-alias", &said_sources), &own)
            .err()
            .map(|f| f.0.into_iter().map(|f| f.to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(found.iter().any(|m| m.starts_with("crates/backends/Cargo.toml:5: [layer-declared] app-backends declares a layer that isn't")), "{found:?}");
        let named = Layers { stack: &["model", "domain", "sources", "engine", "automation", "ui", "apps"], ..Layers::STANDARD };
        assert!(layers(&workspace("layers-own-stack-named", &said_sources), &named).is_ok());
    }

    #[test]
    fn a_dev_dependency_is_free_of_the_order_and_a_renamed_one_is_still_checked() {
        let dev = manifest("app-model", "model", "", "[dev-dependencies]\napp-engine = { path = \"../engine\" }\n");
        assert!(rules(&workspace("layers-dev", &changed("crates/model", dev))).is_empty());
        let renamed = manifest("app-model", "model", "up = { package = \"app-engine\", path = \"../engine\" }", "");
        let found = rules(&workspace("layers-renamed", &changed("crates/model", renamed)));
        assert!(found.iter().any(|(r, m)| *r == "layer-order" && m.contains("app-model (model) depends on app-engine (engine)")), "{found:?}");
        let target = manifest("app-model", "model", "", "[target.'cfg(unix)'.dependencies]\napp-engine = { path = \"../engine\" }\n");
        assert_eq!(rules(&workspace("layers-target", &changed("crates/model", target))).len(), 1);
    }

    #[test]
    fn a_member_cargo_adds_by_its_path_is_held_to_the_contract() {
        // `members` lists only the apps: cargo still makes the model a member, as a path dependency.
        let mut crates = well_formed();
        crates.retain(|(d, _)| d.starts_with("apps/"));
        let dir = workspace("layers-implicit", &crates);
        for (d, m) in well_formed().into_iter().filter(|(d, _)| !d.starts_with("apps/")) {
            std::fs::create_dir_all(dir.join(d).join("src")).unwrap();
            std::fs::write(dir.join(d).join("Cargo.toml"), m).unwrap();
            std::fs::write(dir.join(d).join("src/lib.rs"), format!("{HEADER}pub fn f() {{}}\n")).unwrap();
        }
        std::fs::write(dir.join("crates/model/Cargo.toml"), "[package]\nname = \"app-model\"\n[dependencies]\negui = \"1\"\n").unwrap();
        std::fs::write(dir.join("crates/model/src/lib.rs"), "pub fn f() {}\n").unwrap();
        let found = rules(&dir);
        for want in ["layer-declared", "workspace-lints", "crate-root-header"] {
            assert!(found.iter().any(|(r, m)| *r == want && m.starts_with("crates/model/")), "{want} in {found:#?}");
        }
        // Declared, it is in the order like any other: the model may not depend on the engine.
        let model = manifest("app-model", "model", "app-engine = { path = \"../engine\" }", "");
        std::fs::write(dir.join("crates/model/Cargo.toml"), model).unwrap();
        std::fs::write(dir.join("crates/model/src/lib.rs"), format!("{HEADER}pub fn f() {{}}\n")).unwrap();
        let found = rules(&dir);
        assert!(found.iter().any(|(r, m)| *r == "layer-order" && m.contains("app-model (model) depends on app-engine (engine)")), "{found:#?}");
    }

    #[test]
    fn an_upward_build_dependency_in_the_old_spelling_is_found() {
        let up = manifest("app-model", "model", "", "[build_dependencies]\napp-ui = { path = \"../ui\" }\n");
        let found = rules(&workspace("layers-build-underscore", &changed("crates/model", up)));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].1.starts_with("crates/model/Cargo.toml:13: [layer-order] app-model (model) depends on app-ui (ui)"), "{found:?}");
    }

    #[test]
    fn toolkits_are_for_the_ui_layer_and_the_apps_built_on_it() {
        // Passing: ui and gui (built on app-ui) link a toolkit, in `well_formed`. The CLI doesn't.
        let cli = manifest("app-cli", "apps", "app-automation = { path = \"../../crates/automation\" }\nwinit = \"1\"", "");
        let found = rules(&workspace("layers-cli-toolkit", &changed("apps/cli", cli)));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].1.starts_with(
                "apps/cli/Cargo.toml:12: [ui-toolkit] app-cli (apps) depends on winit, a UI toolkit. Fix: only the ui layer and the apps built on it"
            ),
            "{found:?}"
        );
        let engine = manifest("app-engine", "engine", "app-domain = { path = \"../domain\" }\negui = \"1\"", "");
        assert!(
            rules(&workspace("layers-engine-toolkit", &changed("crates/engine", engine)))
                .iter()
                .any(|(r, m)| *r == "ui-toolkit" && m.contains("app-engine (engine)"))
        );
        // A dev-dependency (egui_kittest in a lower layer's tests) is fine; tooling may not link one.
        let dev = manifest("app-engine", "engine", "app-domain = { path = \"../domain\" }", "[dev-dependencies]\negui_kittest = \"1\"\n");
        assert!(rules(&workspace("layers-dev-toolkit", &changed("crates/engine", dev))).is_empty());
        let tool = manifest("xtask", "tooling", "eframe = \"1\"", "");
        assert!(rules(&workspace("layers-tool-toolkit", &changed("xtask", tool))).iter().any(|(r, m)| *r == "ui-toolkit" && m.contains("xtask (tooling)")));
        // An app built on the UI crate through another app-level crate still counts: gui -> ui.
        let via = manifest("app-via", "apps", "app-ui = { path = \"../../crates/ui\" }\negui = \"1\"", "");
        let mut crates = well_formed();
        crates.push(("apps/via", via));
        assert!(rules(&workspace("layers-via", &crates)).is_empty());
    }

    #[test]
    fn the_testkit_is_only_a_dev_dependency() {
        let normal = manifest("app-engine", "engine", "app-domain = { path = \"../domain\" }\napp-testkit = { path = \"../testkit\" }", "");
        let found = rules(&workspace("layers-testkit", &changed("crates/engine", normal)));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].1.starts_with(
                "crates/engine/Cargo.toml:12: [testkit-dev-only] app-engine depends on app-testkit, the testkit, outside [dev-dependencies]. Fix: move it"
            ),
            "{found:?}"
        );
    }

    #[test]
    fn the_manifest_and_every_root_carry_the_lints() {
        let none = "[package]\nname = \"app-model\"\n\n[package.metadata.swarm]\nlayer = \"model\"\n".to_owned();
        let found = rules(&workspace("layers-lints", &changed("crates/model", none)));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].1.starts_with("crates/model/Cargo.toml:1: [workspace-lints] app-model has no `[lints] workspace = true`. Fix: add `[lints]`"),
            "{found:?}"
        );
        let dir = workspace("layers-headers", &well_formed());
        std::fs::write(dir.join("crates/engine/src/lib.rs"), "#![forbid(unsafe_code)]\npub fn f() {}\n").unwrap();
        std::fs::create_dir_all(dir.join("apps/cli/src")).unwrap();
        std::fs::write(dir.join("apps/cli/src/main.rs"), "fn main() {}\n").unwrap();
        let found = rules(&dir);
        assert_eq!(found.iter().filter(|(r, m)| *r == "crate-root-header" && m.starts_with("crates/engine/src/lib.rs:1:")).count(), 3, "{found:?}");
        assert_eq!(found.iter().filter(|(r, m)| *r == "crate-root-header" && m.starts_with("apps/cli/src/main.rs:1:")).count(), 4, "{found:?}");
        // The testkit and tooling need the short deny list only.
        let testkit_root = "#![forbid(unsafe_code)]\n#![deny(clippy::unwrap_used, clippy::expect_used)]\n#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]\n\
                            #![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]\n";
        std::fs::write(dir.join("crates/testkit/src/lib.rs"), testkit_root).unwrap();
        assert!(rules(&dir).iter().all(|(_, m)| !m.starts_with("crates/testkit")));
    }

    #[test]
    fn swarm_core_is_pinned_never_patched_in_a_committed_file() {
        let dir = workspace("layers-patch", &well_formed());
        let root = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        let url = crate::pin::URL;
        std::fs::write(dir.join("Cargo.toml"), format!("{root}\n[patch.\"{url}\"]\nswarm-core = {{ path = \"../swarm-core/crates/swarm-core\" }}\n")).unwrap();
        let found = rules(&dir);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].1.starts_with("Cargo.toml:4: [swarm-core-patch] Cargo.toml patches swarm-core"), "{found:?}");
        std::fs::write(dir.join("Cargo.toml"), root).unwrap();
        std::fs::create_dir_all(dir.join(".cargo")).unwrap();
        std::fs::write(dir.join(".cargo/config.toml"), format!("[patch.\"{url}\"]\nswarm-core = {{ path = \"x\" }}\n")).unwrap();
        assert!(rules(&dir).iter().any(|(r, m)| *r == "swarm-core-patch" && m.starts_with(".cargo/config.toml:1:")));
    }

    #[test]
    fn an_override_anywhere_in_the_workspace_is_found() {
        let url = crate::pin::URL;
        // A member's own path pin.
        let pinned = manifest("app-model", "model", "swarm-core = { path = \"../../../swarm-core/crates/swarm-core\" }", "");
        let dir = workspace("layers-member-pin", &changed("crates/model", pinned));
        let found = rules(&dir);
        assert!(
            found.len() == 1 && found[0].1.starts_with("crates/model/Cargo.toml:11: [swarm-core-patch] crates/model/Cargo.toml pins swarm-core by `path`"),
            "{found:?}"
        );
        assert!(crate::pin::check_committed(&dir).is_err());
        // The legacy `.cargo/config`, and the root's table form.
        let dir = workspace("layers-legacy-config", &well_formed());
        std::fs::create_dir_all(dir.join(".cargo")).unwrap();
        std::fs::write(dir.join(".cargo/config"), format!("[patch.\"{url}\"]\nswarm-core = {{ path = \"../sf\" }}\n")).unwrap();
        let root = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), format!("{root}[workspace.dependencies.swarm-core]\npath = \"../sf\"\n")).unwrap();
        let found = rules(&dir);
        assert!(found.iter().any(|(r, m)| *r == "swarm-core-patch" && m.starts_with(".cargo/config:1: ")), "{found:?}");
        assert!(
            found.iter().any(|(r, m)| *r == "swarm-core-patch" && m.starts_with("Cargo.toml:3: [swarm-core-patch] Cargo.toml pins swarm-core by `path`")),
            "{found:?}"
        );
        // A config that can't be read, or parsed, is no silent pass.
        let dir = workspace("layers-bad-config", &well_formed());
        std::fs::create_dir_all(dir.join(".cargo")).unwrap();
        std::fs::write(dir.join(".cargo/config.toml"), vec![b'#'; (super::super::MAX_FILE + 1) as usize]).unwrap();
        std::fs::write(dir.join(".cargo/config"), "[patch\n").unwrap();
        let found = rules(&dir);
        assert!(found.iter().any(|(r, m)| *r == "unreadable" && m.starts_with(".cargo/config.toml: ")), "{found:?}");
        assert!(found.iter().any(|(r, m)| *r == "swarm-core-patch" && m.starts_with(".cargo/config:1: ") && m.contains("not valid TOML")), "{found:?}");
    }

    #[test]
    fn a_testkit_at_another_rev_than_swarm_core_is_found() {
        let (url, a, c) = (crate::pin::URL, "a".repeat(40), "c".repeat(40));
        let dir = workspace("layers-testkit-rev", &well_formed());
        let root = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        let deps =
            format!("[workspace.dependencies]\nswarm-core = {{ git = \"{url}\", rev = \"{a}\" }}\nswarm-core-testkit = {{ git = \"{url}\", rev = \"{c}\" }}\n");
        std::fs::write(dir.join("Cargo.toml"), format!("{root}{deps}")).unwrap();
        let found = rules(&dir);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].1.starts_with(&format!("Cargo.toml:5: [swarm-core-testkit-rev] Cargo.toml pins swarm-core-testkit at {c}, not at swarm-core's {a}")),
            "{found:?}"
        );
    }

    #[test]
    fn a_workspace_the_check_cannot_read_is_findings_not_a_panic() {
        let dir = temp_dir("layers-empty");
        assert!(rules(&dir).iter().any(|(r, _)| *r == "unreadable"));
        std::fs::write(dir.join("Cargo.toml"), "[workspace\n").unwrap();
        assert!(rules(&dir).iter().any(|(r, m)| *r == "manifest" && m.contains("not valid TOML")));
        let odd = Layers { ui: "gui", ..Layers::STANDARD };
        assert!(layers(&workspace("layers-odd", &well_formed()), &odd).err().is_some_and(|f| f.0.iter().any(|f| f.rule == "layers-config")));
    }

    #[test]
    fn a_listed_library_may_use_unsafe_and_no_other_root_may() {
        const UNSAFE_HEADER: &str = "#![deny(unsafe_op_in_unsafe_fn)]\n#![warn(clippy::undocumented_unsafe_blocks)]\n#![allow(unsafe_code)]\n\
                                     #![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]\n\
                                     #![warn(clippy::cognitive_complexity, clippy::too_many_lines)]\n#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]\n";
        let root = workspace("layers-unsafe", &well_formed());
        std::fs::write(root.join("crates/domain/src/lib.rs"), format!("{UNSAFE_HEADER}pub fn f() {{}}\n")).unwrap();
        std::fs::write(root.join("crates/domain/src/main.rs"), format!("{UNSAFE_HEADER}fn main() {{}}\n")).unwrap();
        let ffi = Layers { unsafe_crates: &[Unsafe { package: "app-domain", reason: "binds the GPU driver's C API" }], ..Layers::STANDARD };
        let with = |config: &Layers| layers(&root, config).err().map(|f| f.0.into_iter().map(|f| (f.rule, f.file)).collect::<Vec<_>>()).unwrap_or_default();
        // Listed: the library passes; the package's binary still must forbid.
        // (It lacks the forbid, and allows `unsafe_code`.)
        let main = ("crate-root-header", "crates/domain/src/main.rs".to_owned());
        assert_eq!(with(&ffi), [main.clone(), main]);
        // Not listed: the library fails too, in the same two ways.
        assert_eq!(with(&Layers::STANDARD).len(), 4);
        // A listed package that is no member, or has no library, is a config finding.
        let ghost = Layers { unsafe_crates: &[Unsafe { package: "app-ghost", reason: "x" }], ..Layers::STANDARD };
        assert!(with(&ghost).contains(&("unsafe-crates", CONFIGURED.to_owned())));
        std::fs::remove_file(root.join("crates/domain/src/lib.rs")).unwrap();
        let found = layers(&root, &ffi).err().unwrap();
        assert!(found.0.iter().any(|f| f.rule == "unsafe-crates" && f.problem.contains("has no library")), "{found}");
    }
}
