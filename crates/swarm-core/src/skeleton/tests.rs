use super::*;
use crate::assets::placeholders;
use crate::assets::tests::{App, Gpu, NAMES_NO_CONSUMER};
use crate::changed::Rules;
use crate::project::fixtures::{APP_SWARM, GPU_SWARM};

const TODAY: &str = "2026-11-02";

/// The first project's values, with the package prefix its crates share (`Hooks::changed`).
struct FirstApp;
impl Hooks for FirstApp {
    fn changed(&self) -> Rules {
        Rules { package_prefix: "swarm-", ..Rules::NONE }
    }
    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        App.asset_vars()
    }
}

/// The second project's, with its own prefix.
struct GpuApp;
impl Hooks for GpuApp {
    fn changed(&self) -> Rules {
        Rules { package_prefix: "gpu-", ..Rules::NONE }
    }
    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        Gpu.asset_vars()
    }
}

/// A workspace manifest with everything the skeleton's crates inherit: Swarm Core and its testkit
/// pinned at one rev.
const MANIFEST: &str = "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\", \"apps/*\", \"xtask\"]\n\n[workspace.dependencies]\nserde = \"1\"\nserde_json = \"1\"\nserde_path_to_error = \"0.1\"\nschemars = \"1\"\nswarm-core = { git = \"https://github.com/mornycorp/swarm-core.git\", rev = \"0123456789abcdef0123456789abcdef01234567\" }\nswarm-core-testkit = { git = \"https://github.com/mornycorp/swarm-core.git\", rev = \"0123456789abcdef0123456789abcdef01234567\" }\n\n[workspace.lints.rust]\nunsafe_code = \"forbid\"\n";

fn root_with(name: &str, manifest: &str) -> std::path::PathBuf {
    let root = swarm_core_testkit::temp_dir(name);
    std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
    root
}

#[test]
fn every_file_under_app_templates_is_listed() {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                // A manifest is `Cargo.toml.tmpl` there: cargo reads every `Cargo.toml` in a git
                // dependency's checkout to find its package, and a template's isn't valid TOML.
                let path = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.push(path.strip_suffix(".tmpl").map_or(path.clone(), str::to_owned));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/app");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    let mut listed: Vec<&str> = FILES.iter().chain(UI_FILES).map(|f| f.path).chain(UI_BINARY_FILES.iter().map(|&(path, _)| path)).collect();
    listed.sort_unstable();
    assert_eq!(found, listed, "skeleton::FILES and UI_FILES list every file under templates/app/");
    let binary: Vec<&str> = UI_BINARY_FILES.iter().map(|&(path, _)| path).collect();
    assert!(binary.is_sorted(), "UI_BINARY_FILES is sorted");
    for list in [FILES, UI_FILES] {
        let paths: Vec<&str> = list.iter().map(|f| f.path).collect();
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted, "each list is sorted");
    }
    for f in FILES.iter().chain(UI_FILES) {
        assert!(!crate::assets::ASSETS.iter().any(|a| a.path == f.path), "{} is a skeleton file or a pinned asset, not both", f.path);
    }
}

/// The skeleton renders for both consumers with no placeholder left, each crate declaring its layer
/// and inheriting the workspace's lints, each root carrying the headers.
#[test]
fn both_consumers_render_the_skeleton() {
    let layers = [("model", "model"), ("engine", "engine"), ("automation", "automation"), ("testkit", "testkit")];
    for (project, hooks, prefix, cli) in [(&APP_SWARM, &FirstApp as &dyn Hooks, "swarm", "swarm-cli"), (&GPU_SWARM, &GpuApp, "gpu", "gpu-cli")] {
        let files = render_all(project, hooks, false).unwrap_or_else(|e| panic!("{}: {e}", project.name));
        let text = |path: &str| files.iter().find(|(p, _)| p == path).map(|(_, t)| t.clone()).unwrap_or_else(|| panic!("{path} in {files:?}"));
        for (path, body) in &files {
            assert!(placeholders(body).is_empty() && placeholders(path).is_empty(), "{path}: {:?}", placeholders(body));
            assert!(!body.contains("\r\n") && !path.contains("{{"), "{path}");
            assert!(!NAMES_NO_CONSUMER.iter().any(|name| body.to_lowercase().contains(name)), "{path}: names no consumer");
        }
        for (dir, layer) in layers {
            let manifest: toml::Table = text(&format!("crates/{dir}/Cargo.toml")).parse().unwrap();
            let package = manifest["package"].as_table().unwrap();
            assert_eq!(package["name"].as_str(), Some(format!("{prefix}-{dir}").as_str()));
            assert_eq!(package["metadata"]["swarm"]["layer"].as_str(), Some(layer), "{dir}");
            assert_eq!(manifest["lints"]["workspace"].as_bool(), Some(true), "{dir}");
        }
        let app: toml::Table = text(&format!("apps/{cli}/Cargo.toml")).parse().unwrap();
        assert_eq!((app["package"]["name"].as_str(), app["package"]["metadata"]["swarm"]["layer"].as_str()), (Some(cli), Some("apps")));
        assert!(text(&format!("apps/{cli}/tests/surfaces.rs")).contains(&format!("CARGO_BIN_EXE_{cli}")));
        assert!(text("crates/automation/src/mcp.rs").contains(&format!("use {prefix}_engine::")), "the crates' prefix as a Rust path");
        for root in files.iter().filter(|(p, _)| p.ends_with("/lib.rs") || p.ends_with("/main.rs")).map(|(p, _)| p) {
            let body = text(root);
            for header in ["#![forbid(unsafe_code)]", "clippy::unwrap_used", "#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]"] {
                assert!(body.contains(header), "{root}: {header}");
            }
        }
        assert_eq!(files.len(), FILES.len());
        assert!(files.iter().all(|(p, _)| !p.starts_with("crates/ui/")), "no UI without --ui: {files:?}");
    }
}

/// With `--ui` the UI shell (layer `ui`) and the windowed app (layer `apps`, named by `app`) come too,
/// and nothing else changes: the headless files render the same.
#[test]
fn both_consumers_render_the_ui_shell_and_the_window() {
    for (project, hooks, prefix, app) in [(&APP_SWARM, &FirstApp as &dyn Hooks, "swarm", "app-swarm"), (&GPU_SWARM, &GpuApp, "gpu", "gpu-app")] {
        let headless = render_all(project, hooks, false).unwrap();
        let files = render_all(project, hooks, true).unwrap_or_else(|e| panic!("{}: {e}", project.name));
        assert_eq!(files.len(), FILES.len() + UI_FILES.len());
        assert!(headless.iter().all(|f| files.contains(f)), "--ui adds files, it changes none");
        let text = |path: &str| files.iter().find(|(p, _)| p == path).map(|(_, t)| t.clone()).unwrap_or_else(|| panic!("{path} in {files:?}"));
        for (path, body) in &files {
            assert!(placeholders(body).is_empty() && placeholders(path).is_empty(), "{path}: {:?}", placeholders(body));
        }
        let ui: toml::Table = text("crates/ui/Cargo.toml").parse().unwrap();
        let deps = ui["dependencies"].as_table().unwrap();
        assert_eq!(ui["package"]["name"].as_str(), Some(format!("{prefix}-ui").as_str()));
        assert_eq!(ui["package"]["metadata"]["swarm"]["layer"].as_str(), Some("ui"));
        assert!(deps.contains_key("egui"), "the UI crate links the toolkit");
        assert!(deps.contains_key(&format!("{prefix}-engine")), "and the engine");
        assert!(!deps.contains_key(&format!("{prefix}-automation")), "not a transport");
        let window: toml::Table = text(&format!("apps/{app}/Cargo.toml")).parse().unwrap();
        assert_eq!((window["package"]["name"].as_str(), window["package"]["metadata"]["swarm"]["layer"].as_str()), (Some(app), Some("apps")));
        assert!(window["dependencies"].as_table().unwrap().contains_key(&format!("{prefix}-ui")));
        for (path, manifest) in files.iter().filter(|(p, _)| p.ends_with("Cargo.toml")) {
            let toolkit = path == "crates/ui/Cargo.toml" || path == &format!("apps/{app}/Cargo.toml");
            assert_eq!(manifest.contains("egui") || manifest.contains("eframe"), toolkit, "a toolkit only in the UI layer and the window: {path}");
        }
        for root in [format!("apps/{app}/src/main.rs"), "crates/ui/src/lib.rs".to_owned()] {
            let body = text(&root);
            for header in ["#![forbid(unsafe_code)]", "clippy::unwrap_used", "#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]"] {
                assert!(body.contains(header), "{root}: {header}");
            }
        }
        assert!(text("crates/ui/src/lib.rs").contains("The toolkit is the app's choice"), "the crate says the toolkit is the app's choice");
        assert!(text("crates/ui/src/views.rs").contains(&format!("use {prefix}_engine::")), "the crates' prefix as a Rust path");
    }
    assert_eq!(scans(false).len(), SCANS.len());
    assert_eq!(scans(true).len(), SCANS.len() + UI_SCANS.len());
}

/// The scans follow the workspace: none without an app, the UI crate's with it, and its theme and
/// catalog rules only once it has a catalog, so a UI crate from an older skeleton isn't failed by a
/// bump for the colours and words it draws in place.
#[test]
fn the_ui_crates_theme_and_catalog_rules_start_with_its_catalog() {
    let root = root_with("core-skeleton-scans-at", MANIFEST);
    assert!(scans_at(&root).is_empty(), "no app, no scans");
    init(&APP_SWARM, &FirstApp, &root, TODAY, true).unwrap();
    let rules = |root: &Path| scans_at(root).iter().map(|s| s.rule).collect::<Vec<_>>();
    let all = rules(&root);
    assert_eq!(all.len(), SCANS.len() + UI_SCANS.len());
    assert!(UI_LOOK_RULES.iter().all(|r| all.contains(r)), "{all:?}");
    assert!(root.join(UI_THEME).is_file() && root.join(UI_CATALOG).is_file(), "the skeleton writes both");
    std::fs::remove_file(root.join(UI_CATALOG)).unwrap();
    let older = rules(&root);
    assert_eq!(older.len(), SCANS.len() + UI_SCANS.len() - UI_LOOK_RULES.len(), "{older:?}");
    assert!(older.contains(&"ui-file-writes") && !older.iter().any(|r| UI_LOOK_RULES.contains(r)), "{older:?}");
}

/// `init --app` writes the starting points and the skeleton, only what is missing; a second run
/// writes nothing and an edited file stays as it is.
#[test]
fn init_writes_only_what_is_missing() {
    let root = root_with("core-skeleton-init", MANIFEST);
    std::fs::create_dir_all(root.join("crates/engine/src")).unwrap();
    std::fs::write(root.join("crates/engine/src/lib.rs"), "// ours\n").unwrap();
    let written = init(&APP_SWARM, &FirstApp, &root, TODAY, false).unwrap();
    assert_eq!(written.len(), FILES.len() - 1);
    assert!(!written.iter().any(|p| p == "crates/engine/src/lib.rs"));
    assert_eq!(std::fs::read_to_string(root.join("crates/engine/src/lib.rs")).unwrap(), "// ours\n");
    for path in ["apps/swarm-cli/src/main.rs", "crates/model/src/ids.rs", "rustfmt.toml", "clippy.toml", "docs/swarm.md", ".config/nextest.toml"] {
        assert!(root.join(path).is_file(), "{path}");
    }
    assert_eq!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap(), MANIFEST, "the root manifest is the app's: never rewritten");
    std::fs::write(root.join("apps/swarm-cli/src/cli.rs"), "// edited\n").unwrap();
    assert!(init(&APP_SWARM, &FirstApp, &root, TODAY, false).unwrap().is_empty(), "a second run writes nothing");
    assert_eq!(std::fs::read_to_string(root.join("apps/swarm-cli/src/cli.rs")).unwrap(), "// edited\n");
}

/// `init --app --ui` writes the same files and the UI's; the window takes the project's `app` name.
#[test]
fn init_with_ui_writes_the_ui_crate_and_the_window_and_headless_writes_neither() {
    let headless = root_with("core-skeleton-init-headless", MANIFEST);
    init(&APP_SWARM, &FirstApp, &headless, TODAY, false).unwrap();
    assert!(!headless.join("crates/ui").exists() && !headless.join("apps/app-swarm").exists());
    let root = root_with("core-skeleton-init-ui", MANIFEST);
    let written = init(&APP_SWARM, &FirstApp, &root, TODAY, true).unwrap();
    assert_eq!(written.len(), FILES.len() + UI_FILES.len() + UI_BINARY_FILES.len());
    for path in ["crates/ui/src/shell.rs", "crates/ui/Cargo.toml", "apps/app-swarm/src/main.rs", "apps/swarm-cli/src/main.rs"] {
        assert!(root.join(path).is_file(), "{path}");
    }
    for &(path, bytes) in UI_BINARY_FILES {
        assert_eq!(std::fs::read(root.join(path)).unwrap(), bytes, "{path} is written as it is");
    }
    assert!(init(&APP_SWARM, &FirstApp, &root, TODAY, true).unwrap().is_empty(), "a second run writes nothing");
    let later = init(&APP_SWARM, &FirstApp, &headless, TODAY, true).unwrap();
    assert_eq!(later.len(), UI_FILES.len() + UI_BINARY_FILES.len(), "`--ui` on a headless app writes only the UI's files");
}

/// A root manifest without what the crates inherit writes nothing and says which lines to add.
#[test]
fn a_manifest_that_lacks_what_the_crates_inherit_writes_nothing() {
    let manifest = "[workspace]
members = [\"xtask\"]

[workspace.dependencies]
serde_json = \"1\"
";
    let root = root_with("core-skeleton-manifest", manifest);
    let err = init(&APP_SWARM, &FirstApp, &root, TODAY, false).unwrap_err();
    for want in [
        "nothing was written",
        "serde = \"1\"",
        "serde_path_to_error = \"0.1\"",
        "schemars = \"1\"",
        "swarm-core-testkit = { git = ",
        "<the same rev as swarm-core>",
        "[workspace.lints.rust]",
    ] {
        assert!(err.contains(want), "{want}: {err}");
    }
    assert!(!err.contains("serde_json = \"1\""), "what it has isn't asked for again: {err}");
    let names: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Cargo.toml"], "nothing written, not even the starting points");
    assert!(init(&APP_SWARM, &FirstApp, &swarm_core_testkit::temp_dir("core-skeleton-no-manifest"), TODAY, false).unwrap_err().contains("Cargo.toml"));
}

/// The members can only come after the crates (cargo refuses a glob that matches nothing): `init
/// --app` writes the files, then fails until the manifest covers them, and passes once it does.
#[test]
fn the_files_are_written_and_the_members_are_asked_for_after() {
    let manifest = MANIFEST.replace("\"crates/*\", \"apps/*\", ", "");
    let root = root_with("core-skeleton-members", &manifest);
    let err = init(&APP_SWARM, &FirstApp, &root, TODAY, false).unwrap_err();
    assert!(err.contains("the files are written") && err.contains("\"crates/*\"") && err.contains("crates/model") && err.contains("apps/swarm-cli"), "{err}");
    assert!(root.join("crates/model/src/ids.rs").is_file() && root.join("AGENTS.md").is_file());
    assert!(init(&APP_SWARM, &FirstApp, &root, TODAY, false).is_err(), "still asks, and writes nothing more");
    std::fs::write(root.join("Cargo.toml"), MANIFEST).unwrap();
    assert!(init(&APP_SWARM, &FirstApp, &root, TODAY, false).unwrap().is_empty(), "once the members cover them it passes, with nothing left to write");
}

/// The conformance run is Swarm Core's testkit (#26): the manifest pins it beside Swarm Core, at the
/// same rev. A manifest that lacks it is told the line to add, with Swarm Core's own rev in it; one
/// that pins the two at different revs is told to set them together.
#[test]
fn the_manifest_pins_swarm_cores_testkit_at_swarm_cores_rev() {
    let swarm_core_only = MANIFEST.lines().filter(|l| !l.starts_with("swarm-core-testkit")).collect::<Vec<_>>().join("\n");
    let needs = manifest_needs(&swarm_core_only).unwrap();
    assert_eq!(needs.len(), 1, "{needs:?}");
    let want = "swarm-core-testkit = { git = \"https://github.com/mornycorp/swarm-core.git\", rev = \"0123456789abcdef0123456789abcdef01234567\" }";
    assert!(needs[0].contains(want), "the line to add carries Swarm Core's rev: {needs:?}");
    let apart = MANIFEST.replacen(
        "rev = \"0123456789abcdef0123456789abcdef01234567\" }\nswarm-core-testkit",
        "rev = \"89abcdef0123456789abcdef0123456789abcdef\" }\nswarm-core-testkit",
        1,
    );
    let needs = manifest_needs(&apart).unwrap();
    assert!(needs.len() == 1 && needs[0].contains("one pin") && needs[0].contains("0123456789abcdef0123456789abcdef01234567"), "{needs:?}");
    assert!(manifest_needs(MANIFEST).unwrap().is_empty());
}

#[test]
fn the_manifests_needs_are_read_from_its_tables() {
    assert!(manifest_needs(MANIFEST).unwrap().is_empty());
    assert_eq!(
        manifest_needs(
            "[workspace]
"
        )
        .unwrap()
        .len(),
        6,
        "four dependencies, the testkit and the lints"
    );
    assert!(
        manifest_needs(
            "[package]
name = \"solo\"
"
        )
        .unwrap()[0]
            .contains("[workspace]")
    );
    assert!(
        manifest_needs(
            "[workspace
"
        )
        .unwrap_err()
        .contains("not valid TOML")
    );
    let dirs: Vec<String> = ["crates/model", "apps/x-cli"].map(str::to_owned).to_vec();
    assert!(uncovered(MANIFEST, &dirs).unwrap().is_empty());
    let explicit = MANIFEST.replace("\"crates/*\", \"apps/*\"", "\"crates/model\", \"apps/x-cli\"");
    assert!(uncovered(&explicit, &dirs).unwrap().is_empty());
    let excluded = MANIFEST.replace(
        "\"xtask\"]
",
        "\"xtask\"]
exclude = [\"apps/x-cli\"]
",
    );
    assert_eq!(uncovered(&excluded, &dirs).unwrap(), ["apps/x-cli"], "an excluded crate isn't a member");
    assert_eq!(
        uncovered(
            "[workspace]
members = [\"xtask\"]
",
            &dirs
        )
        .unwrap()
        .len(),
        2
    );
}

/// A project that gives no CLI name, no prefix or a malformed one is refused with what to give.
#[test]
fn the_names_the_skeleton_takes_are_checked_before_anything_is_written() {
    struct Bad(&'static [(&'static str, &'static str)], &'static str);
    impl Hooks for Bad {
        fn changed(&self) -> Rules {
            Rules { package_prefix: self.1, ..Rules::NONE }
        }
        fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
            self.0
        }
    }
    let good: &'static [(&str, &str)] = &[("cli", "x-cli"), ("product", "X")];
    let err = |hooks: &dyn Hooks| render_all(&APP_SWARM, hooks, false).unwrap_err();
    assert!(err(&crate::Defaults).contains("`{{cli}}`"), "no cli");
    assert!(err(&Bad(good, "")).contains("`{{crate_prefix}}`") && err(&Bad(good, "")).contains("package_prefix"), "no prefix");
    assert!(err(&Bad(good, "x")).contains("ending in a dash"), "a prefix without its dash");
    assert!(err(&Bad(good, "X-")).contains("ending in a dash") && err(&Bad(good, "../")).contains("ending in a dash"));
    assert!(err(&Bad(&[("cli", "My_CLI"), ("product", "X")], "x-")).contains("`cli` is `My_CLI`"));
    assert!(err(&Bad(&[("cli", "../evil"), ("product", "X")], "x-")).contains("`cli` is"));
    assert!(err(&Bad(&[("cli", "x-cli"), ("product", "My \"App\"")], "x-")).contains("`product` is"));
    assert!(err(&Bad(&[("cli", "x-cli")], "x-")).contains("`{{product}}`"));
    assert!(render_all(&APP_SWARM, &Bad(good, "x-"), false).is_ok());
    // `app` is only needed, and checked, with `--ui`: it names the window's directory and package.
    let err_ui = |vars: &'static [(&str, &str)]| render_all(&APP_SWARM, &Bad(vars, "x-"), true).unwrap_err();
    assert!(err_ui(good).contains("`{{app}}`"), "no app");
    assert!(err_ui(&[("cli", "x-cli"), ("product", "X"), ("app", "My_App")]).contains("`app` is `My_App`"));
    assert!(err_ui(&[("cli", "x-cli"), ("product", "X"), ("app", "../x")]).contains("`app` is"));
    assert!(err_ui(&[("cli", "x-cli"), ("product", "X"), ("app", "x-cli")]).contains("two names"));
    assert!(render_all(&APP_SWARM, &Bad(&[("cli", "x-cli"), ("product", "X"), ("app", "x-app")], "x-"), true).is_ok());
}

/// The starting points' maps (AGENTS.md §2, docs/architecture.md's crates) name the paths the
/// skeleton writes: each `crates/…` or `apps/…` path they give (but a `<placeholder>` one) is a
/// directory of the rendered skeleton.
#[test]
fn the_starting_points_name_the_paths_the_skeleton_writes() {
    let files = render_all(&APP_SWARM, &FirstApp, true).unwrap();
    for name in ["AGENTS.md", "docs/architecture.md"] {
        let template = crate::templates::TEMPLATES.iter().find(|t| t.path == name).unwrap();
        let text = crate::templates::render_template(&APP_SWARM, &FirstApp, template, TODAY).unwrap();
        let paths: Vec<&str> = text.split('`').skip(1).step_by(2).filter(|p| p.starts_with("crates/") || p.starts_with("apps/")).collect();
        assert!(paths.iter().any(|p| p.starts_with("apps/swarm-cli")) && paths.iter().any(|p| p.starts_with("crates/testkit")), "{name}: {paths:?}");
        for path in paths.into_iter().filter(|p| !p.contains('<') && !p.contains('*')) {
            let dir = format!("{}/", path.trim_end_matches('/'));
            assert!(files.iter().any(|(p, _)| p.starts_with(&dir)), "{name} names `{path}`, which the skeleton doesn't write");
        }
    }
}

/// A project with no package prefix gives the crates' prefix as `crate_prefix` in its asset vars, as
/// the refusal's hint says it can; a package prefix, when there is one, is the prefix.
#[test]
fn the_crates_prefix_comes_from_the_asset_vars_when_the_package_prefix_is_empty() {
    struct Vars(&'static str);
    impl Hooks for Vars {
        fn changed(&self) -> Rules {
            Rules { package_prefix: self.0, ..Rules::NONE }
        }
        fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
            &[("product", "My Swarm"), ("cli", "my-cli"), ("crate_prefix", "my-")]
        }
    }
    let files = render_all(&APP_SWARM, &Vars(""), false).unwrap_or_else(|e| panic!("crate_prefix in asset_vars: {e}"));
    let text = |path: &str| files.iter().find(|(p, _)| p == path).map(|(_, t)| t.clone()).unwrap();
    assert!(text("crates/engine/Cargo.toml").contains("name = \"my-engine\""));
    assert!(text("crates/automation/src/mcp.rs").contains("use my_engine::"), "crate_prefix_ident follows it");
    let files = render_all(&APP_SWARM, &Vars("our-"), false).unwrap();
    assert!(files.iter().any(|(p, t)| p == "crates/engine/Cargo.toml" && t.contains("name = \"our-engine\"")), "the package prefix comes first");
}

/// A binary's name that is a crate's package name (`cli = "my-engine"` with the prefix `my-`) would
/// make two packages of one name: refused before anything is written, for the CLI and the window.
#[test]
fn a_binary_named_like_a_crate_is_refused_before_anything_is_written() {
    struct Named(&'static [(&'static str, &'static str)]);
    impl Hooks for Named {
        fn changed(&self) -> Rules {
            Rules { package_prefix: "my-", ..Rules::NONE }
        }
        fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
            self.0
        }
    }
    for (vars, ui, key, value) in [
        (&[("product", "X"), ("cli", "my-engine")][..], false, "cli", "my-engine"),
        (&[("product", "X"), ("cli", "my-testkit")], false, "cli", "my-testkit"),
        (&[("product", "X"), ("cli", "my-ui")], false, "cli", "my-ui"),
        (&[("product", "X"), ("cli", "xtask")], false, "cli", "xtask"),
        (&[("product", "X"), ("cli", "my-cli"), ("app", "my-model")], true, "app", "my-model"),
    ] {
        let err = render_all(&APP_SWARM, &Named(vars), ui).unwrap_err();
        assert!(err.contains(&format!("`{key}` is `{value}`, the package name of")) && err.contains("can't share a name"), "{err}");
        let root = root_with("core-skeleton-taken-name", MANIFEST);
        assert!(init(&APP_SWARM, &Named(vars), &root, TODAY, ui).is_err());
        assert!(!root.join("crates").exists() && !root.join("apps").exists(), "{key} = {value}: nothing written");
    }
    assert!(render_all(&APP_SWARM, &Named(&[("product", "X"), ("cli", "my-cli"), ("app", "my-app")]), true).is_ok());
}

#[test]
fn swarm_core_command_runs_init_app_with_or_without_ui_and_refuses_ui_alone() {
    let root = root_with("core-skeleton-command", MANIFEST);
    let err = crate::pin::run(&APP_SWARM, &FirstApp, &root, &["init", "--ui"]).unwrap_err();
    assert!(err.contains("--app --ui") && err.contains("goes with `--app`"), "{err}");
    assert!(!root.join("crates").exists(), "a refused flag writes nothing");
    crate::pin::run(&APP_SWARM, &FirstApp, &root, &["init", "--app"]).unwrap();
    assert!(root.join("apps/swarm-cli/Cargo.toml").is_file() && root.join("docs/merge-check.md").is_file());
    assert!(!root.join("crates/ui").exists(), "`init --app` without `--ui` writes no UI");
    let with_ui = root_with("core-skeleton-command-ui", MANIFEST);
    crate::pin::run(&APP_SWARM, &FirstApp, &with_ui, &["init", "--app", "--ui"]).unwrap();
    assert!(with_ui.join("crates/ui/src/shell.rs").is_file() && with_ui.join("apps/app-swarm/src/window.rs").is_file());
    let either_order = root_with("core-skeleton-command-ui-order", MANIFEST);
    crate::pin::run(&APP_SWARM, &FirstApp, &either_order, &["init", "--ui", "--app"]).unwrap();
    assert!(either_order.join("crates/ui/Cargo.toml").is_file());
    assert!(crate::pin::USAGE.contains("init [--app [--ui]]"));
}

/// mornycorp/swarm-core#91: an app from an earlier skeleton keeps its process module and its
/// writer in the engine, and is read with that layout's scans; one with the backends crate is
/// held to the layer.
#[test]
fn an_app_without_the_backends_crate_is_scanned_at_its_engines_modules() {
    let root = swarm_core_testkit::temp_dir("core-skeleton-earlier-layout");
    std::fs::create_dir_all(root.join("crates/engine")).unwrap();
    std::fs::write(root.join("crates/engine/Cargo.toml"), "").unwrap();
    assert!(is_app(&root) && !has_backends(&root));
    let excepts = |scans: &[Scan], rule: &str| scans.iter().find(|s| s.rule == rule).map(|s| s.except.to_vec()).unwrap_or_default();
    let earlier = scans_at(&root);
    assert_eq!(excepts(&earlier, "process-module"), [ENGINE_PROCESS_MODULE]);
    assert_eq!(excepts(&earlier, "file-writes"), [STATE_MODULE]);
    std::fs::create_dir_all(root.join("crates/backends")).unwrap();
    std::fs::write(root.join("crates/backends/Cargo.toml"), "").unwrap();
    let now = scans_at(&root);
    assert_eq!(excepts(&now, "process-module"), [PROCESS_MODULE]);
    assert_eq!(excepts(&now, "file-writes"), FILE_WRITERS);
}
