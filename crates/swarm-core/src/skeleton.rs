//! The app skeleton (mornycorp/swarm-core#49): a small workspace, from `templates/app/`, in
//! which one example command (`app.info`) already runs through every surface and the checks pass.
//! **`cargo xtask swarm-core init --app`** ([`init`]) writes it, and the starting points
//! ([`crate::templates`]) with it; **`--app --ui`** adds the UI shell and the windowed app
//! ([`UI_FILES`]). It is the app's own once written.
//!
//! - **What it is:** `crates/model` (the id rule, a hostile-input parser), `crates/engine` (the
//!   session, the typed registry, the one dispatch, `app.info`), `crates/automation` (the JSON-lines
//!   frame with the contract's caps, `serve`, the loopback control socket with its token and `auth`,
//!   an MCP server generated from the registry), `apps/<cli>` (`run`, `commands`, `serve [--socket]`, `mcp`), `crates/testkit` (temp
//!   dirs, hostile request lines; its tests run swarm-core's conformance run, a pinned `[dev-dependencies]` entry), plus `rustfmt.toml` (`clippy.toml` is the
//!   starting points' own). Every crate root carries
//!   the contract's lint headers, `[lints] workspace = true` and `[package.metadata.swarm] layer`
//!   (docs/app-architecture.md), which `checks::check` holds it to. It implements the contract's
//!   minimum interfaces as written there.
//! - **The UI shell** (`--ui`): `crates/ui` (layer `ui`: an egui shell
//!   that lists the commands, runs one on a worker thread and draws its typed result; the toolkit is
//!   the app's choice and the crate says so) and `apps/<app>` (layer `apps`: the window, with
//!   `--socket` the control socket on the same session). [`UI_SCANS`] holds the UI crate to the
//!   contract's The UI is a client (its colours from its theme, its words from its catalog among
//!   it); [`scans`] gives them with [`SCANS`].
//! - **Templates**, under `templates/app/`, are the files as they will be, with `{{name}}`
//!   placeholders; a manifest is `Cargo.toml.tmpl` there (cargo reads every `Cargo.toml` in a git
//!   dependency's checkout, and a template's is not valid TOML: it would print an error for each on
//!   every consumer's fetch).
//! - **Written only where missing**, as [`crate::templates::init`] does: a file the app has is
//!   never touched, so it is safe to run again.
//! - **Names.** `{{cli}}` (the binary: `apps/{{cli}}`) and the crates' prefix (`{{crate_prefix}}`,
//!   `{{crate_prefix_ident}}`: the project's [`crate::changed::Rules::package_prefix`], as `my-` and
//!   `my_`, or `crate_prefix` in its asset vars when that is empty) come from the project
//!   ([`crate::Hooks::asset_vars`], [`crate::Hooks::changed`]), with `{{product}}`. A project that
//!   gives none, or a binary's name that is a crate's package name (`cli = "my-engine"`), is refused
//!   before anything is written.
//! - **The root manifest is the app's.** The crates inherit `serde`, `serde_json`, `serde_path_to_error` and `schemars`, and the
//!   Swarm Core's testkit (`swarm-core-testkit`, pinned at the same `rev` as `swarm-core`: the
//!   conformance run is its), from `[workspace.dependencies]` and `[workspace.lints]`, and the workspace needs `members`
//!   covering `crates/*` and `apps/*`. `init --app` never edits the manifest, it reads it: when
//!   the dependencies or the lints are missing it writes nothing and says which lines to add
//!   ([`manifest_needs`]; docs/new-swarm.md's manifest has them from the start). The members can't
//!   be there first (cargo refuses a member glob that matches nothing, and would not run the xtask
//!   that writes the crates), so `init --app` writes the files and then fails until the manifest
//!   covers them ([`uncovered`]): `ci` would otherwise pass without building a crate.
//! - **Its checks** are the consumer's own `cargo xtask layers`, which calls
//!   [`crate::checks::check`]; `examples/consumer_xtask.rs` has the command, the scans for the
//!   skeleton's files and the `layers` step of `ci`. It runs the skeleton's scans and the full
//!   run's `conformance` step only in an app ([`is_app`], [`scans_at`]), so the same xtask serves a
//!   repo with no app.

use std::path::Path;

use toml::Value;

use crate::Hooks;
use crate::Project;
use crate::assets::{Asset, fill, var};
use crate::checks::{Allowed, Scan, member_glob};

/// The files that are wholly test code: a scan reads only the production code of the others.
pub const TEST_FILES: &[&str] = crate::checks::TEST_FILES;

/// The source files of the app's crates and binaries, in the skeleton's layout.
pub const SOURCES: &[&str] = &["crates/**/*.rs", "apps/**/*.rs"];

/// The scans of docs/app-architecture.md ([`crate::checks::scan`]) pointed at the skeleton's files:
/// a consumer's [`Hooks::scans`] returns them (`SCANS.to_vec()`) and changes what its app does
/// differently: it names the process module in `process_command`'s `except` once it has one, adds
/// the UI crate's scans (`Scan::ui_file_writes`, `Scan::ui_process_starts`, `Scan::value_reads`)
/// and the state module's (`Scan::file_writes`). Every allowance gives its reason and a count.
pub const SCANS: &[Scan] = &[
    Scan::process_command().files(SOURCES).test_files(TEST_FILES),
    Scan::value_reads().files(&["crates/automation/**/*.rs", "apps/**/*.rs"]).test_files(TEST_FILES),
    Scan::command_spec().files(SOURCES).test_files(TEST_FILES).allowing(&[Allowed {
        file: "crates/engine/src/registry.rs",
        count: 1,
        reason: "the typed adapter: `Registry::register` is the one place a command's spec is built",
    }]),
    Scan::real_config_dir().files(SOURCES).test_files(TEST_FILES),
];

/// The UI shell's theme module: its colour tokens, the one place a colour is made.
pub const UI_THEME: &str = "crates/ui/src/theme.rs";

/// The UI shell's string catalog: every word it draws, the one place a text is written.
pub const UI_CATALOG: &str = "crates/ui/src/catalog.rs";

/// The UI crate's sources.
const UI_SOURCES: &[&str] = &["crates/ui/**/*.rs"];

/// The scans of the UI shell (`init --app --ui`), which [`scans`] adds to [`SCANS`]: no file write and
/// no process start in `crates/ui`, no `serde_json::Value` read there, and its colours from the theme
/// ([`UI_THEME`]) and its words from the catalog ([`UI_CATALOG`]) (the contract's The UI is a client).
/// They name `crates/ui`, so an app without the UI crate must not have them: a scan that reads no
/// files is a failure. The last two are [`UI_LOOK_RULES`], which [`scans_at`] leaves out of a UI crate
/// that has no catalog yet.
pub const UI_SCANS: &[Scan] = &[
    Scan::ui_file_writes().files(UI_SOURCES).test_files(TEST_FILES),
    Scan::ui_process_starts().files(UI_SOURCES).test_files(TEST_FILES),
    Scan::value_reads().files(UI_SOURCES).test_files(TEST_FILES),
    Scan::ui_colours().files(UI_SOURCES).except(&[UI_THEME]).test_files(TEST_FILES),
    Scan::ui_strings().files(UI_SOURCES).except(&[UI_CATALOG]).test_files(TEST_FILES).allowing(&[Allowed {
        file: "crates/ui/src/jobs.rs",
        count: 3,
        reason: "the worker thread's name and `Jobs`'s `Debug` names: what the OS and a debugger show, never the window",
    }]),
];

/// The rules of [`UI_SCANS`] that hold the UI crate to its theme and its catalog.
/// A UI crate written by an older skeleton has neither, so [`scans_at`] runs them only once
/// [`UI_CATALOG`] exists: the app opts in by adding the theme and the catalog, and a bump never fails
/// it for colours and words its old skeleton drew in place.
pub const UI_LOOK_RULES: &[&str] = &["ui-colours", "ui-strings"];

/// The skeleton's scans for an app with or without the UI shell (`skeleton::scans(false)`, or `true`
/// once `init --app --ui` has been run). [`scans_at`] picks for the workspace as it is.
pub fn scans(ui: bool) -> Vec<Scan> {
    let mut all = SCANS.to_vec();
    if ui {
        all.extend_from_slice(UI_SCANS);
    }
    all
}

/// Whether the workspace at `root` is an app made from the skeleton: `init --app` has written its
/// engine (`crates/engine/Cargo.toml`). A consumer's xtask (`examples/consumer_xtask.rs`) asks this
/// before it runs the skeleton's scans ([`scans_at`]) and the full run's `conformance` step, so one
/// xtask serves a repo with no app (a library, a tooling repo) and the same repo once it has one.
pub fn is_app(root: &Path) -> bool {
    root.join("crates/engine/Cargo.toml").is_file()
}

/// Whether the app at `root` has the UI shell (`init --app --ui`): `crates/ui/Cargo.toml`.
pub fn has_ui(root: &Path) -> bool {
    is_app(root) && root.join("crates/ui/Cargo.toml").is_file()
}

/// The skeleton's scans for the workspace at `root` as it is: none in a repo with no app (each scan
/// would read no file, and its allow-list would name a file that isn't there: both findings),
/// [`scans`] in an app, with the UI crate's when it has one, its theme and catalog rules
/// ([`UI_LOOK_RULES`]) once it has a catalog. What a consumer's [`Hooks::scans`] returns
/// (`skeleton::scans_at(&root)`), plus the app's own as it grows.
pub fn scans_at(root: &Path) -> Vec<Scan> {
    if !is_app(root) {
        return Vec::new();
    }
    let mut all = scans(has_ui(root));
    if !root.join(UI_CATALOG).is_file() {
        all.retain(|scan| !UI_LOOK_RULES.contains(&scan.rule));
    }
    all
}

/// The skeleton's files, by path in the app (`{{cli}}` in a path is the CLI's name), sorted.
pub const FILES: &[Asset] = &[
    Asset { path: "apps/{{cli}}/Cargo.toml", text: include_str!("../templates/app/apps/{{cli}}/Cargo.toml.tmpl") },
    Asset { path: "apps/{{cli}}/src/cli.rs", text: include_str!("../templates/app/apps/{{cli}}/src/cli.rs") },
    Asset { path: "apps/{{cli}}/src/main.rs", text: include_str!("../templates/app/apps/{{cli}}/src/main.rs") },
    Asset { path: "apps/{{cli}}/tests/surfaces.rs", text: include_str!("../templates/app/apps/{{cli}}/tests/surfaces.rs") },
    Asset { path: "crates/automation/Cargo.toml", text: include_str!("../templates/app/crates/automation/Cargo.toml.tmpl") },
    Asset { path: "crates/automation/src/control.rs", text: include_str!("../templates/app/crates/automation/src/control.rs") },
    Asset { path: "crates/automation/src/frame.rs", text: include_str!("../templates/app/crates/automation/src/frame.rs") },
    Asset { path: "crates/automation/src/lib.rs", text: include_str!("../templates/app/crates/automation/src/lib.rs") },
    Asset { path: "crates/automation/src/limits.rs", text: include_str!("../templates/app/crates/automation/src/limits.rs") },
    Asset { path: "crates/automation/src/lines.rs", text: include_str!("../templates/app/crates/automation/src/lines.rs") },
    Asset { path: "crates/automation/src/mcp.rs", text: include_str!("../templates/app/crates/automation/src/mcp.rs") },
    Asset { path: "crates/automation/src/serve.rs", text: include_str!("../templates/app/crates/automation/src/serve.rs") },
    Asset { path: "crates/automation/src/socket.rs", text: include_str!("../templates/app/crates/automation/src/socket.rs") },
    Asset { path: "crates/automation/src/socket/conn.rs", text: include_str!("../templates/app/crates/automation/src/socket/conn.rs") },
    Asset { path: "crates/automation/src/socket/tests.rs", text: include_str!("../templates/app/crates/automation/src/socket/tests.rs") },
    Asset { path: "crates/automation/src/token.rs", text: include_str!("../templates/app/crates/automation/src/token.rs") },
    Asset { path: "crates/engine/Cargo.toml", text: include_str!("../templates/app/crates/engine/Cargo.toml.tmpl") },
    Asset { path: "crates/engine/src/call.rs", text: include_str!("../templates/app/crates/engine/src/call.rs") },
    Asset { path: "crates/engine/src/commands.rs", text: include_str!("../templates/app/crates/engine/src/commands.rs") },
    Asset { path: "crates/engine/src/error.rs", text: include_str!("../templates/app/crates/engine/src/error.rs") },
    Asset { path: "crates/engine/src/lib.rs", text: include_str!("../templates/app/crates/engine/src/lib.rs") },
    Asset { path: "crates/engine/src/menu.rs", text: include_str!("../templates/app/crates/engine/src/menu.rs") },
    Asset { path: "crates/engine/src/registry.rs", text: include_str!("../templates/app/crates/engine/src/registry.rs") },
    Asset { path: "crates/engine/src/session.rs", text: include_str!("../templates/app/crates/engine/src/session.rs") },
    Asset { path: "crates/engine/src/spec.rs", text: include_str!("../templates/app/crates/engine/src/spec.rs") },
    Asset { path: "crates/engine/src/theme.rs", text: include_str!("../templates/app/crates/engine/src/theme.rs") },
    Asset { path: "crates/engine/src/theme/tests.rs", text: include_str!("../templates/app/crates/engine/src/theme/tests.rs") },
    Asset { path: "crates/engine/src/ui.rs", text: include_str!("../templates/app/crates/engine/src/ui.rs") },
    Asset { path: "crates/model/Cargo.toml", text: include_str!("../templates/app/crates/model/Cargo.toml.tmpl") },
    Asset { path: "crates/model/src/ids.rs", text: include_str!("../templates/app/crates/model/src/ids.rs") },
    Asset { path: "crates/model/src/lib.rs", text: include_str!("../templates/app/crates/model/src/lib.rs") },
    Asset { path: "crates/testkit/Cargo.toml", text: include_str!("../templates/app/crates/testkit/Cargo.toml.tmpl") },
    Asset { path: "crates/testkit/src/lib.rs", text: include_str!("../templates/app/crates/testkit/src/lib.rs") },
    Asset { path: "rustfmt.toml", text: include_str!("../templates/app/rustfmt.toml") },
];

/// The UI's files, written by `init --app --ui` only (sorted): a UI shell crate (layer `ui`, egui) and
/// the windowed app that hosts it (`apps/{{app}}`, layer `apps`). `{{app}}` is the project's own
/// `app` value ([`crate::Hooks::asset_vars`]), the name of the windowed binary.
pub const UI_FILES: &[Asset] = &[
    Asset { path: "apps/{{app}}/Cargo.toml", text: include_str!("../templates/app/apps/{{app}}/Cargo.toml.tmpl") },
    Asset { path: "apps/{{app}}/src/main.rs", text: include_str!("../templates/app/apps/{{app}}/src/main.rs") },
    Asset { path: "apps/{{app}}/src/ui_state.rs", text: include_str!("../templates/app/apps/{{app}}/src/ui_state.rs") },
    Asset { path: "apps/{{app}}/src/window.rs", text: include_str!("../templates/app/apps/{{app}}/src/window.rs") },
    Asset { path: "crates/ui/Cargo.toml", text: include_str!("../templates/app/crates/ui/Cargo.toml.tmpl") },
    Asset { path: "crates/ui/src/catalog.rs", text: include_str!("../templates/app/crates/ui/src/catalog.rs") },
    Asset { path: "crates/ui/src/draw.rs", text: include_str!("../templates/app/crates/ui/src/draw.rs") },
    Asset { path: "crates/ui/src/jobs.rs", text: include_str!("../templates/app/crates/ui/src/jobs.rs") },
    Asset { path: "crates/ui/src/lib.rs", text: include_str!("../templates/app/crates/ui/src/lib.rs") },
    Asset { path: "crates/ui/src/shell.rs", text: include_str!("../templates/app/crates/ui/src/shell.rs") },
    Asset { path: "crates/ui/src/shell/tests.rs", text: include_str!("../templates/app/crates/ui/src/shell/tests.rs") },
    Asset { path: "crates/ui/src/theme.rs", text: include_str!("../templates/app/crates/ui/src/theme.rs") },
    Asset { path: "crates/ui/src/views.rs", text: include_str!("../templates/app/crates/ui/src/views.rs") },
];

/// The longest name (`cli`, the prefix without its dash) the skeleton takes.
const MAX_NAME: usize = 40;

/// `name` as a package name's word: lowercase letters and digits in runs, joined by single dashes,
/// starting with a letter.
fn is_word(name: &str) -> bool {
    let runs_ok = name.split('-').all(|run| !run.is_empty() && run.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
    runs_ok && name.len() <= MAX_NAME && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
}

/// The project's values for the skeleton's own placeholders, each checked: they land in file and
/// directory names, in manifests and in Rust paths, so a value that isn't a plain package name is
/// refused before anything is written.
fn check_names(project: &Project, hooks: &dyn Hooks, ui: bool) -> Result<(), String> {
    let get = |name: &str, hint: &str| var(project, hooks, name).ok_or_else(|| format!("the project gives no value for `{{{{{name}}}}}` ({hint})"));
    let cli = get("cli", "Hooks::asset_vars: the CLI binary's name, such as `my-cli`")?;
    let prefix = get("crate_prefix", "Hooks::changed's `package_prefix`, such as `my-`, or `crate_prefix` in Hooks::asset_vars")?;
    let product = get("product", "Hooks::asset_vars: what the app is called")?;
    if !is_word(&cli) {
        return Err(format!(
            "`cli` is `{cli}`: the skeleton names a directory and a package after it, so it must be lowercase letters, digits and single dashes (`my-cli`)"
        ));
    }
    if !prefix.strip_suffix('-').is_some_and(is_word) {
        return Err(format!("the crates' prefix is `{prefix}`: it must be a lowercase word ending in a dash (`my-`), as `{prefix}engine` is a package name"));
    }
    if product.trim().is_empty() || product.chars().any(|c| c.is_control() || c == '"' || c == '\\') {
        return Err(format!("`product` is `{product}`: it lands in manifests' descriptions, so it must be one line without quotes or backslashes"));
    }
    let taken = taken_names(&prefix);
    not_taken("cli", &cli, &taken)?;
    if ui {
        let app = get("app", "Hooks::asset_vars: the windowed app's binary name, such as `my-app`")?;
        check_app_name(&app, &cli)?;
        not_taken("app", &app, &taken)?;
    }
    Ok(())
}

/// The package names a binary's name must not take in the app's workspace: each crate's of the
/// skeleton (`<prefix><dir>` for `crates/<dir>`, the UI crate's too, as `--ui` may come later), and
/// the xtask's. Each with where it is.
fn taken_names(prefix: &str) -> Vec<(String, String)> {
    let crates = FILES.iter().chain(UI_FILES).filter_map(|f| f.path.strip_prefix("crates/")?.strip_suffix("/Cargo.toml"));
    let mut taken: Vec<(String, String)> = crates.map(|dir| (format!("{prefix}{dir}"), format!("the skeleton's `crates/{dir}`"))).collect();
    taken.push(("xtask".to_owned(), "the workspace's `xtask/`".to_owned()));
    taken
}

/// `value` (`cli` or `app`), a binary's package name, is none of `taken`'s: two packages of one
/// workspace can't share a name.
fn not_taken(key: &str, value: &str, taken: &[(String, String)]) -> Result<(), String> {
    match taken.iter().find(|(name, _)| name == value) {
        Some((_, whose)) => Err(format!(
            "`{key}` is `{value}`, the package name of {whose}: two packages of one workspace can't share a name, so give the binary its own (`my-{key}`)"
        )),
        None => Ok(()),
    }
}

/// The windowed app's name (`app`, with `--ui`): a package and a directory name, and not the CLI's.
fn check_app_name(app: &str, cli: &str) -> Result<(), String> {
    if !is_word(app) {
        return Err(format!(
            "`app` is `{app}`: `init --ui` names a directory and a package after it, so it must be lowercase letters, digits and single dashes (`my-app`)"
        ));
    }
    if app == cli {
        return Err(format!(
            "`app` and `cli` are both `{app}`: the window and the CLI are two binaries, so they need two names (the CLI must never link a toolkit)"
        ));
    }
    Ok(())
}

/// Every file of the skeleton rendered for the project: its path in the app, and its text (LF
/// line endings, no placeholder left). With `ui`, the UI's files ([`UI_FILES`]) too.
pub fn render_all(project: &Project, hooks: &dyn Hooks, ui: bool) -> Result<Vec<(String, String)>, String> {
    check_names(project, hooks, ui)?;
    FILES
        .iter()
        .chain(if ui { UI_FILES } else { &[] })
        .map(|file| {
            let path = fill(project, hooks, file.path, file.path.to_owned())?;
            let text = fill(project, hooks, file.path, file.text.replace("\r\n", "\n"))?;
            Ok((path, text))
        })
        .collect()
}

/// The `[workspace]` table of the root manifest `manifest` (TOML), if it has one.
fn workspace_of(manifest: &str) -> Result<Option<toml::Table>, String> {
    let table: toml::Table = manifest.parse().map_err(|e| format!("Cargo.toml is not valid TOML: {e}"))?;
    Ok(table.get("workspace").and_then(Value::as_table).cloned())
}

/// What the app's root manifest (`manifest`, TOML) lacks for the skeleton's crates to build, each as
/// the line to add: the four workspace dependencies they inherit, Swarm Core's testkit (pinned at
/// the same `rev` as `swarm-core`: the conformance run is its) and `[workspace.lints]`. [`init`]
/// asks before it writes anything. (The members are asked after: [`uncovered`].)
pub fn manifest_needs(manifest: &str) -> Result<Vec<String>, String> {
    let Some(workspace) = workspace_of(manifest)? else {
        return Ok(vec!["a `[workspace]` table (docs/new-swarm.md step 2 has the whole manifest)".to_owned()]);
    };
    let deps = workspace.get("dependencies").and_then(Value::as_table);
    let mut needs: Vec<String> = [
        ("serde", "serde = \"1\""),
        ("serde_json", "serde_json = \"1\""),
        ("serde_path_to_error", "serde_path_to_error = \"0.1\""),
        ("schemars", "schemars = \"1\""),
    ]
    .into_iter()
    .filter(|(name, _)| !deps.is_some_and(|d| d.contains_key(*name)))
    .map(|(_, line)| format!("`[workspace.dependencies]`: {line}"))
    .collect();
    needs.extend(testkit_need(deps));
    if !workspace.contains_key("lints") {
        needs.push("`[workspace.lints.rust]`: unsafe_code = \"forbid\"".to_owned());
    }
    Ok(needs)
}

/// Swarm Core's testkit as a workspace dependency, pinned like Swarm Core: the same `git` and
/// the same `rev` (`bump` moves both). What to add or change when the manifest doesn't.
fn testkit_need(deps: Option<&toml::Table>) -> Option<String> {
    let rev_of = |name: &str| deps?.get(name)?.as_table()?.get("rev")?.as_str();
    let line = |rev: &str| format!("{} = {{ git = \"{}\", rev = \"{rev}\" }}", crate::pin::TESTKIT, crate::pin::URL);
    let core = rev_of(crate::pin::PACKAGE);
    match (rev_of(crate::pin::TESTKIT), core) {
        (None, rev) => Some(format!(
            "`[workspace.dependencies]`: {} (the conformance run; the same `rev` as `{}`, which `cargo xtask swarm-core bump` moves with it)",
            line(rev.unwrap_or("<the same rev as swarm-core>")),
            crate::pin::PACKAGE
        )),
        (Some(testkit), Some(core)) if testkit != core => Some(format!(
            "`[workspace.dependencies]`: `{}`'s `rev` is {testkit} but `{}`'s is {core}: they are one pin, set both to {core}",
            crate::pin::TESTKIT,
            crate::pin::PACKAGE
        )),
        _ => None,
    }
}

/// The directories of `dirs` (the skeleton's crates) that the manifest's `[workspace] members` don't
/// cover. They can only be added once the crates exist: cargo refuses a member glob that matches
/// nothing, so a manifest that named them first couldn't run the xtask that writes them.
pub fn uncovered(manifest: &str, dirs: &[String]) -> Result<Vec<String>, String> {
    let workspace = workspace_of(manifest)?.unwrap_or_default();
    let globs = |key: &str| -> Vec<&str> { workspace.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect() };
    let (members, exclude) = (globs("members"), globs("exclude"));
    let covered = |dir: &str| members.iter().any(|g| member_glob(g, dir)) && !exclude.iter().any(|g| member_glob(g, dir));
    Ok(dirs.iter().filter(|dir| !covered(dir)).cloned().collect())
}

/// `cargo xtask swarm-core init --app`: the starting points ([`crate::templates::init`]) and the
/// skeleton's files, each written only if it isn't there; the skeleton's paths written. Nothing is
/// written when a name the skeleton needs is missing or malformed, or the root manifest lacks what
/// the skeleton's crates inherit ([`manifest_needs`]). When the files are written but the
/// manifest's `members` don't cover the crates yet, it says so as an error ([`uncovered`]): `ci`
/// would otherwise pass without building them.
pub fn init(project: &Project, hooks: &dyn Hooks, root: &Path, today: &str, ui: bool) -> Result<Vec<String>, String> {
    let files = render_all(project, hooks, ui)?;
    let total = files.len();
    let dirs: Vec<String> = files.iter().filter_map(|(path, _)| path.strip_suffix("/Cargo.toml")).map(str::to_owned).collect();
    let manifest = crate::checks::read(root, "Cargo.toml").map_err(|f| format!("init --app: {f}"))?;
    let needs = manifest_needs(&manifest)?;
    if !needs.is_empty() {
        return Err(format!(
            "init --app: the workspace manifest (Cargo.toml) lacks what the skeleton's crates inherit, so nothing was written. Add to it:
  - {}
then run `cargo xtask swarm-core init --app` again (docs/new-swarm.md › The app's skeleton)",
            needs.join(
                "
  - "
            )
        ));
    }
    crate::templates::init(project, hooks, root, today)?;
    let written = write_missing(root, files)?;
    println!(
        "swarm-core: the app skeleton: {written} of {total} files written from swarm-core {version}; it is this app's now (docs/new-swarm.md › The app's skeleton)",
        written = written.len(),
        total = total,
        version = crate::VERSION,
    );
    let loose = uncovered(&manifest, &dirs)?;
    if !loose.is_empty() {
        return Err(format!(
            "init --app: the files are written, but the workspace manifest's `members` don't cover {} yet, so cargo and `ci` don't see them. Add `\"crates/*\"` and `\"apps/*\"` to `members` in Cargo.toml (cargo refuses a glob that matches nothing, so it can only be added now), then run `cargo xtask swarm-core sync-assets` and `cargo xtask ci` (docs/new-swarm.md › The app's skeleton)",
            loose.join(", ")
        ));
    }
    Ok(written)
}

/// Write each of `files` that isn't at `root` yet; the paths written.
fn write_missing(root: &Path, files: Vec<(String, String)>) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    for (path, text) in files {
        let at = root.join(&path);
        if at.exists() {
            println!("  kept     {path} (yours)");
            continue;
        }
        if let Some(dir) = at.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&at, text).map_err(|e| format!("{path}: {e}"))?;
        println!("  written  {path}");
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests;
