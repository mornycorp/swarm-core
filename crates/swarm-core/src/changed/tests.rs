//! The plan, against both consumers' rules: the first project's reads and graph, and a second project's runtime edges and lockfile.

use super::*;

/// The first project's rules, as its xtask gives them.
struct App;
impl Hooks for App {
    fn changed(&self) -> Rules {
        Rules {
            package_prefix: "swarm-",
            reads: &[
                Read { path: "assets/", packages: &["swarm-ui-egui"], embeds: true },
                Read { path: "docs/code-health.toml", packages: &["swarm-source", "xtask"], embeds: false },
                Read { path: "apps/swarm-web/index.html", packages: &["swarm-ui-egui"], embeds: false },
            ],
            runtime: &[],
        }
    }
    fn xtask_reads(&self, path: &str) -> bool {
        ["docs/gaps.md", "docs/scorecard.md", "docs/development.md", ".github/workflows/ci.yml"].contains(&path) || path.starts_with("scorecard/")
    }
}

/// A second project's rules: runtime edges, a read and a file class of its own, with its prefix.
struct Gpu;
impl Hooks for Gpu {
    fn changed(&self) -> Rules {
        Rules {
            package_prefix: "gpu-",
            reads: &[Read { path: "docs/control-protocol.md", packages: &["gpu-engine"], embeds: false }],
            runtime: &[
                Runtime { producer: "gpu-replayer", artifact: "gpu-replayer.exe", consumer: "gpu-vk-layer" },
                Runtime { producer: "gpu-vk-layer", artifact: "the Vulkan layer", consumer: "gpu-replayer" },
                Runtime { producer: "gpu-vk-layer", artifact: "the Vulkan layer", consumer: "gpu-automation" },
                Runtime { producer: "gpu-vk-layer", artifact: "the Vulkan layer", consumer: "gpu-ui-egui" },
            ],
        }
    }
    fn file_class(&self, path: &str) -> Option<FileClass> {
        // Its corpus is read across the workspace.
        path.starts_with("corpus/").then(|| FileClass::Everything(format!("{path} is the corpus")))
    }
}

fn krate(name: &str, deps: &[(&str, DepKind)]) -> Crate {
    Crate { name: name.into(), deps: deps.iter().map(|(n, k)| Dep { name: (*n).into(), kind: *k }).collect() }
}

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter().map(|(n, d)| ((*n).to_owned(), (*d).to_owned())).collect()
}

/// The first project's graph as `cargo metadata` reported it (xtask's edges on 2026-10-22).
fn graph() -> Vec<Crate> {
    use DepKind::{Dev, Normal};
    vec![
        krate("swarm-model", &[]),
        krate("swarm-timeline", &[("swarm-model", Normal)]),
        krate("swarm-source", &[("swarm-model", Dev), ("swarm-testkit", Dev)]),
        krate("swarm-engine", &[("swarm-model", Normal), ("swarm-source", Normal), ("swarm-timeline", Normal), ("swarm-testkit", Dev)]),
        krate("swarm-automation", &[("swarm-engine", Normal)]),
        krate("swarm-ui-egui", &[("swarm-engine", Normal), ("swarm-model", Normal), ("swarm-timeline", Normal), ("swarm-testkit", Dev)]),
        krate("swarm-testkit", &[]),
        krate("swarm-cli", &[("swarm-automation", Normal), ("swarm-engine", Normal), ("swarm-testkit", Dev)]),
        krate("swarm-gui", &[("swarm-automation", Normal), ("swarm-engine", Normal), ("swarm-ui-egui", Normal), ("swarm-testkit", Dev)]),
        krate("swarm-web", &[("swarm-engine", Normal), ("swarm-ui-egui", Normal)]),
        krate("xtask", &[("swarm-source", Normal), ("swarm-testkit", Dev), ("swarm-model", Dev), ("swarm-timeline", Dev), (SWARM_CORE, Normal)]),
    ]
}

fn packages() -> Vec<(String, String)> {
    pairs(&[
        ("swarm-model", "crates/model"),
        ("swarm-timeline", "crates/timeline"),
        ("swarm-source", "crates/source"),
        ("swarm-engine", "crates/engine"),
        ("swarm-automation", "crates/automation"),
        ("swarm-ui-egui", "crates/ui-egui"),
        ("swarm-testkit", "crates/testkit"),
        ("swarm-cli", "apps/swarm-cli"),
        ("swarm-gui", "apps/swarm-gui"),
        ("swarm-web", "apps/swarm-web"),
        ("xtask", "xtask"),
    ])
}

fn files(f: &[&str]) -> Vec<String> {
    f.iter().map(|s| (*s).to_owned()).collect()
}

fn si(f: &[&str]) -> Plan {
    plan(&App, &files(f), &packages(), &graph(), None)
}

fn reached(f: &[&str]) -> BTreeMap<String, String> {
    match si(f) {
        Plan::Crates { crates, .. } => crates,
        p => panic!("{f:?}: {p:?}"),
    }
}

fn pkg(name: &str) -> FileClass {
    FileClass::Package { name: name.into(), readers: Vec::new() }
}

fn tests(package: &str) -> Reader {
    Reader { package: package.into(), embeds: false }
}

#[test]
fn files_map_to_the_deepest_package_or_a_class() {
    let p = packages();
    let c = |path: &str| classify(&App, path, &p);
    assert_eq!(c("crates/ui-egui/src/menus.rs"), pkg("swarm-ui-egui"));
    assert_eq!(c("apps/swarm-web/index.html"), FileClass::Package { name: "swarm-web".into(), readers: vec![tests("swarm-ui-egui")] });
    // `crates/model` is not a prefix of `crates/model-extra`.
    assert!(matches!(c("crates/model-extra/src/lib.rs"), FileClass::Everything(_)));
    assert_eq!(c("docs/roadmap.md"), FileClass::Docs);
    assert_eq!(c("crates/engine/README.md"), FileClass::Docs);
    // What xtask's tests read is data xtask reads.
    let xtask = FileClass::Data(vec![tests("xtask")]);
    assert_eq!((c("docs/gaps.md"), c("scorecard/ui.toml"), c(".github/workflows/ci.yml")), (xtask.clone(), xtask.clone(), xtask));
    assert_eq!(c(".github/pull_request_template.md"), FileClass::NoBuild);
    assert_eq!(c("scripts/github-labels.sh"), FileClass::NoBuild);
    assert_eq!(c(".claude/skills/refactoring/SKILL.md"), FileClass::NoBuild);
    assert_eq!(c("assets/fonts/CascadiaMono-Regular.ttf"), FileClass::Data(vec![Reader { package: "swarm-ui-egui".into(), embeds: true }]));
    assert_eq!(c("xtask/src/main.rs"), pkg("xtask"));
    assert_eq!(c(SWARM_CORE_PIN), pkg(SWARM_CORE));
    for everything in
        ["Cargo.toml", "Cargo.lock", "clippy.toml", "rustfmt.toml", "rust-toolchain.toml", ".cargo/config.toml", ".config/nextest.toml", "something-new/x.txt"]
    {
        assert!(matches!(c(everything), FileClass::Everything(_)), "{everything}");
    }
    // The project's own class comes first.
    assert_eq!(classify(&Gpu, "corpus/a.capture", &p), FileClass::Everything("corpus/a.capture is the corpus".into()));
    assert_eq!(classify(&crate::Defaults, "docs/gaps.md", &p), FileClass::Docs, "no rules: no reader");
}

#[test]
fn a_ui_change_reaches_only_the_apps_above_it() {
    let plan = si(&["crates/ui-egui/src/menus.rs"]);
    assert_eq!(plan.names(), ["swarm-gui", "swarm-ui-egui", "swarm-web"]);
    let Plan::Crates { crates, files, build } = plan else { panic!() };
    assert_eq!((files, build.len()), (1, 0));
    assert_eq!((crates["swarm-ui-egui"].as_str(), crates["swarm-gui"].as_str()), ("changed", "depends on ui-egui"));
}

#[test]
fn the_model_reaches_everything_built_on_it_but_not_dev_dependents_dependents() {
    let c = reached(&["crates/model/src/event.rs"]);
    let names: Vec<&str> = c.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        ["swarm-automation", "swarm-cli", "swarm-engine", "swarm-gui", "swarm-model", "swarm-source", "swarm-timeline", "swarm-ui-egui", "swarm-web", "xtask"]
    );
    assert_eq!((c["swarm-model"].as_str(), c["swarm-timeline"].as_str()), ("changed", "depends on model"));
    // source only dev-depends on model: tested, but its library did not change, so it reaches
    // nothing (engine is here for its own edge to model).
    assert_eq!((c["swarm-source"].as_str(), c["swarm-engine"].as_str()), ("dev-depends on model", "depends on model"));
    assert_eq!(c["xtask"], "dev-depends on model");
}

#[test]
fn testkit_reaches_its_dev_dependents_and_no_further() {
    assert_eq!(
        si(&["crates/testkit/src/git.rs"]).names(),
        ["swarm-cli", "swarm-engine", "swarm-gui", "swarm-source", "swarm-testkit", "swarm-ui-egui", "xtask"]
    );
}

#[test]
fn a_normal_edge_beats_a_dev_edge_to_the_same_package() {
    use DepKind::{Dev, Normal};
    let g = vec![krate("a", &[]), krate("b", &[("a", Dev), ("a", Normal)]), krate("c", &[("b", Normal)])];
    let changed = BTreeMap::from([("a".to_owned(), "changed".to_owned())]);
    let (why, built) = affected(&Rules::NONE, &g, &changed, &BTreeMap::new());
    assert_eq!(why.keys().collect::<Vec<_>>(), ["a", "b", "c"]);
    assert_eq!(built.into_iter().collect::<Vec<_>>(), ["a", "b", "c"]);
}

#[test]
fn reads_from_outside_a_package_test_it() {
    let c = reached(&["assets/fonts/Roboto-Regular.ttf"]);
    assert_eq!(c["swarm-ui-egui"], "reads assets/fonts/Roboto-Regular.ttf");
    assert_eq!(c.keys().collect::<Vec<_>>(), ["swarm-gui", "swarm-ui-egui", "swarm-web"], "an embedded read reaches the dependents");
    let c = reached(&["docs/code-health.toml"]);
    assert_eq!(c.keys().collect::<Vec<_>>(), ["swarm-source", "xtask"]);
    assert_eq!(c["xtask"], "its tests read docs/code-health.toml");
    // A package whose library changed keeps that reason, and reaches its dependents.
    let c = reached(&["docs/code-health.toml", "crates/source/src/lib.rs"]);
    assert_eq!((c["swarm-source"].as_str(), c["swarm-engine"].as_str()), ("changed", "depends on source"));
}

#[test]
fn docs_only_builds_nothing() {
    let plan = si(&["docs/roadmap.md", "ROADMAP.md", "crates/engine/README.md", "scripts/hooks/pre-push"]);
    assert_eq!(plan, Plan::Crates { crates: BTreeMap::new(), build: BTreeMap::new(), files: 4 });
    assert!(describe(&App, &plan, "origin/main", 11).contains("0 of 11 packages affected"));
}

/// A progress fold wrapped a line of docs/gaps.md, the docs gate ran no xtask
/// test, and main went red. The files xtask's tests read test it.
#[test]
fn what_xtasks_tests_read_tests_xtask() {
    for f in ["docs/gaps.md", "docs/scorecard.md", "scorecard/ui.toml", ".github/workflows/ci.yml"] {
        let c = reached(&[f, "ROADMAP.md"]);
        assert_eq!(c.keys().collect::<Vec<_>>(), ["xtask"], "{f}");
        assert_eq!(c["xtask"], format!("its tests read {f}"));
    }
}

#[test]
fn the_workspace_manifests_and_the_test_config_mean_everything() {
    for f in ["Cargo.toml", "Cargo.lock", ".config/nextest.toml"] {
        let plan = si(&["crates/ui-egui/src/menus.rs", f]);
        assert_eq!(plan, Plan::Everything(format!("{f} changed")));
        assert!(describe(&App, &plan, "origin/main", 11).contains("the full run"));
        assert!(plan.names().is_empty());
    }
}

/// A bump of Swarm Core's pin builds what builds on it (xtask) and tests
/// what tests against it (the model's and timeline's round-trip tests), and nothing past a dev
/// edge.
#[test]
fn a_pin_bump_reaches_what_depends_on_swarm_core() {
    let mut g = graph();
    for c in g.iter_mut().filter(|c| matches!(c.name.as_str(), "swarm-model" | "swarm-timeline")) {
        c.deps.push(Dep { name: SWARM_CORE.into(), kind: DepKind::Dev });
    }
    let plan = plan(&App, &files(&[SWARM_CORE_PIN]), &packages(), &g, None);
    assert_eq!(plan.names(), ["swarm-model", "swarm-timeline", "xtask"]);
    let Plan::Crates { crates, .. } = &plan else { panic!() };
    assert_eq!((crates["xtask"].as_str(), crates["swarm-model"].as_str()), ("depends on core", "dev-depends on core"));
    assert!(describe(&App, &plan, "origin/main", 11).contains("3 of 11 packages affected"));
}

/// a lockfile change tests the packages it reaches and their dependents; without
/// what it reaches, everything.
#[test]
fn a_lockfile_change_tests_what_it_reaches() {
    let reached = |c: &[&str]| c.iter().map(|c| ((*c).to_owned(), "Cargo.lock: x 1 added".to_owned())).collect::<BTreeMap<_, _>>();
    let lock = files(&["Cargo.lock", "xtask/Cargo.toml"]);
    assert_eq!(plan(&App, &lock, &packages(), &graph(), None), Plan::Everything("Cargo.lock changed".into()));
    let p = plan(&App, &lock, &packages(), &graph(), Some(&reached(&["xtask"])));
    let Plan::Crates { crates, .. } = &p else { panic!() };
    assert_eq!(crates, &BTreeMap::from([("xtask".to_owned(), "changed".to_owned())]), "its own manifest's change says why first");
    let model = plan(&App, &files(&["Cargo.lock"]), &packages(), &graph(), Some(&reached(&["swarm-model"])));
    assert!(model.names().contains(&"swarm-engine") && model.names().contains(&"swarm-model"), "and what depends on it");
    assert!(plan(&App, &files(&["Cargo.lock"]), &packages(), &graph(), Some(&BTreeMap::new())).names().is_empty(), "reaching nothing");
}

/// The second project's graph, with the packages its runtime edges name.
fn gpu_graph() -> (Vec<Crate>, Vec<(String, String)>) {
    use DepKind::{Dev, Normal};
    let g = vec![
        krate("gpu-capture-model", &[]),
        krate("gpu-engine", &[("gpu-capture-model", Normal), ("gpu-testkit", Dev)]),
        krate("gpu-testkit", &[("gpu-engine", Normal)]),
        krate("gpu-ui-egui", &[("gpu-engine", Normal), ("gpu-testkit", Dev)]),
        krate("gpu", &[("gpu-ui-egui", Normal)]),
        krate("gpu-automation", &[("gpu-engine", Normal), ("gpu-testkit", Dev)]),
        krate("gpu-replayer", &[("gpu-capture-model", Normal), ("gpu-testkit", Dev)]),
        krate("gpu-capture-writer", &[("gpu-capture-model", Normal)]),
        krate("gpu-vk-layer", &[("gpu-capture-model", Normal), ("gpu-capture-writer", Dev), ("gpu-engine", Dev), ("gpu-testkit", Dev)]),
    ];
    let p = pairs(&[
        ("gpu-capture-model", "crates/capture-model"),
        ("gpu-engine", "crates/engine"),
        ("gpu-testkit", "crates/testkit"),
        ("gpu-ui-egui", "crates/ui-egui"),
        ("gpu", "apps/gpu"),
        ("gpu-automation", "crates/automation"),
        ("gpu-capture-writer", "crates/capture-writer"),
        ("gpu-replayer", "apps/gpu-replayer"),
        ("gpu-vk-layer", "apps/gpu-vk-layer"),
    ]);
    (g, p)
}

#[test]
fn a_binary_crates_change_tests_the_crates_that_launch_it() {
    let (g, p) = gpu_graph();
    let p1 = plan(&Gpu, &files(&["apps/gpu-replayer/src/main.rs"]), &p, &g, None);
    assert_eq!(p1.names(), ["gpu-replayer", "gpu-vk-layer"]);
    let Plan::Crates { crates, build, .. } = p1 else { panic!() };
    assert_eq!(crates["gpu-vk-layer"], "its tests launch gpu-replayer.exe (replayer affected)");
    assert!(build.is_empty(), "{build:?}");
    // The layer: the replayer's, automation's and the UI's tests capture under it; the UI's
    // dependents (the app) are not reached: the UI's code didn't change.
    let p1 = plan(&Gpu, &files(&["apps/gpu-vk-layer/src/lib.rs"]), &p, &g, None);
    assert_eq!(p1.names(), ["gpu-automation", "gpu-replayer", "gpu-ui-egui", "gpu-vk-layer"]);
}

#[test]
fn a_crate_whose_tests_launch_a_binary_builds_it_first() {
    let (g, p) = gpu_graph();
    let p1 = plan(&Gpu, &files(&["crates/ui-egui/src/menus.rs"]), &p, &g, None);
    let Plan::Crates { crates, build, .. } = &p1 else { panic!() };
    assert_eq!(crates.keys().collect::<Vec<_>>(), ["gpu", "gpu-ui-egui"]);
    assert_eq!(build.iter().collect::<Vec<_>>(), [(&"gpu-vk-layer".to_owned(), &"the Vulkan layer, for the tests of ui-egui".to_owned())]);
    assert!(describe(&Gpu, &p1, "origin/main", 9).contains("  vk-layer                 built first, not tested: the Vulkan layer, for the tests of ui-egui\n"));
    // A graph without the producer (another workspace) builds nothing it doesn't have.
    let Plan::Crates { build, .. } = plan(&Gpu, &files(&["crates/ui-egui/src/menus.rs"]), &packages(), &graph(), None) else { panic!() };
    assert!(build.is_empty());
}

#[test]
fn the_graph_and_the_libraries_come_from_metadata() {
    let meta = serde_json::json!({"packages": [
        {"name": "swarm-model", "dependencies": [{"name": "serde", "kind": null}], "targets": [{"kind": ["lib"]}]},
        {"name": "swarm-engine", "dependencies": [
            {"name": "swarm-model", "kind": null}, {"name": "swarm-testkit", "kind": "dev"}, {"name": "cc", "kind": "build"},
            {"name": "swarm-core", "kind": "dev"}
        ], "targets": [{"kind": ["bin"]}]},
        {"name": "swarm-testkit", "dependencies": []},
        {"dependencies": "not a list"},
    ]});
    let g = crates(&meta);
    assert_eq!(g.len(), 3);
    assert_eq!(g[0].deps, []);
    // The edge to Swarm Core stays, though it isn't a member.
    assert_eq!(
        g[1].deps,
        [
            Dep { name: "swarm-model".into(), kind: DepKind::Normal },
            Dep { name: "swarm-testkit".into(), kind: DepKind::Dev },
            Dep { name: SWARM_CORE.into(), kind: DepKind::Dev }
        ]
    );
    assert_eq!(libraries(&meta).into_iter().collect::<Vec<_>>(), ["swarm-model"]);
}

#[test]
fn package_dirs_are_relative_to_the_root() {
    let meta = serde_json::json!({"packages": [
        {"name": "swarm-engine", "manifest_path": "D:\\dev\\app-swarm\\crates\\engine\\Cargo.toml"},
        {"name": "xtask", "manifest_path": "d:/dev/app-swarm/xtask/Cargo.toml"},
        {"name": "elsewhere", "manifest_path": "C:/other/Cargo.toml"},
        {"name": "sibling", "manifest_path": "D:/dev/app-swarm-willow/Cargo.toml"},
        {"name": "no-manifest"},
    ]});
    assert_eq!(package_dirs(&meta, Path::new("D:\\dev\\app-swarm")), pairs(&[("swarm-engine", "crates/engine"), ("xtask", "xtask")]));
}

#[test]
fn short_names_leave_out_the_prefix() {
    let r = App.changed();
    assert_eq!((short_name(&r, "swarm-ui-egui"), short_name(&r, "xtask"), short_name(&r, "swarm-")), ("ui-egui", "xtask", "swarm-"));
    assert_eq!(short_name(&Rules::NONE, "swarm-ui-egui"), "swarm-ui-egui");
}

/// `--base` and `--head` are refs, never options for git.
#[test]
fn base_and_head_are_refs_not_options() {
    for ok in ["origin/main", "HEAD~2", "348-sync-xtask-claim-never-adopts", "a1b2c3d"] {
        assert_eq!(checked_ref("--base", ok), Ok(ok));
    }
    let root = swarm_core_testkit::temp_dir("swarm-core-changed-refs");
    for bad in ["", "--output=x", "-p", "main two", "main\n", "a\tb", "x\u{0}"] {
        assert!(checked_ref("--head", bad).unwrap_err().contains("not a valid --head ref"), "{bad:?}");
        assert!(changed_files(&root, bad, None).unwrap_err().contains("--base"), "{bad:?}");
        assert!(changed_files(&root, "origin/main", Some(bad)).unwrap_err().contains("--head"), "{bad:?}");
    }
}

/// A real repository: the working tree's changes, untracked files included; a pushed branch's;
/// a manifest and lock diff that only moves Swarm Core's pin as [`SWARM_CORE_PIN`]; and what
/// a lockfile change reaches.
#[test]
fn changed_files_read_a_real_repository() {
    use swarm_core_testkit::git::git;
    let root = swarm_core_testkit::temp_dir("swarm-core-changed-files");
    git(&root, &["init", "-q", "-b", "main"]);
    let pin = |sha: &str| format!("[workspace.dependencies]\n{SWARM_CORE} = {{ git = \"{}\", rev = \"{sha}\" }}\n", crate::pin::URL);
    let lock = |sha: &str, log: &str| {
        format!(
            "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"log\",\n]\n\n[[package]]\nname = \"log\"\nversion = \"{log}\"\nsource = \"registry+x\"\nchecksum = \"{log}\"\n\n[[package]]\nname = \"{SWARM_CORE}\"\nversion = \"0.1.0\"\nsource = \"git+{}?rev={sha}#{sha}\"\n\n[[package]]\nname = \"xtask\"\nversion = \"0.1.0\"\ndependencies = [\n \"{SWARM_CORE}\",\n]\n",
            crate::pin::URL
        )
    };
    let (a, b) = ("a28c7554ddbfc7826a2c2db42628c2ac7133ead6", "0123456789abcdef0123456789abcdef01234567");
    std::fs::write(root.join("Cargo.toml"), pin(a)).unwrap();
    std::fs::write(root.join(LOCK), lock(a, "0.4.0")).unwrap();
    std::fs::create_dir_all(root.join("crates/app")).unwrap();
    std::fs::write(root.join("crates/app/lib.rs"), "").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "base"]);
    // The working tree: a change, an untracked file, and a bump of the pin alone.
    std::fs::write(root.join("crates/app/lib.rs"), "// changed").unwrap();
    std::fs::write(root.join("new.md"), "x").unwrap();
    std::fs::write(root.join("Cargo.toml"), pin(b)).unwrap();
    std::fs::write(root.join(LOCK), lock(b, "0.4.0")).unwrap();
    assert_eq!(changed_files(&root, "main", None).unwrap(), ["crates/app/lib.rs", "new.md", SWARM_CORE_PIN]);
    // A lock that moves another package too is the lock's own change: it reaches its user.
    std::fs::write(root.join(LOCK), lock(b, "0.5.0")).unwrap();
    let f = changed_files(&root, "main", None).unwrap();
    assert_eq!(f, ["Cargo.lock", "crates/app/lib.rs", "new.md", SWARM_CORE_PIN]);
    let members = vec![krate("app", &[]), krate("xtask", &[])];
    let reached = lock_for(&root, &f, "main", None, &members).unwrap();
    assert_eq!(reached.keys().collect::<Vec<_>>(), ["app", "xtask"], "app's log moved, xtask's swarm-core did");
    assert_eq!(lock_for(&root, &files(&["Cargo.toml"]), "main", None, &members), None, "no lock change");
    // A pushed branch: its commits' files, not the working tree's.
    git(&root, &["checkout", "-qb", "topic"]);
    git(&root, &["add", "crates/app/lib.rs"]);
    git(&root, &["commit", "-qm", "topic"]);
    assert_eq!(changed_files(&root, "main", Some("topic")).unwrap(), ["crates/app/lib.rs"]);
    assert!(changed_files(&root, "no-such-ref-anywhere", None).unwrap_err().contains("pass --base"));
}

/// [`missed`]: a read the plan leaves out is named, with where it is made; one the rules map is
/// not; a read of the whole workspace can't be planned.
#[test]
fn a_read_the_rules_leave_out_is_missed() {
    let read = |reader: &str, target: Option<&str>, embeds: bool| reads::Read {
        reader: reader.into(),
        target: target.map(str::to_owned),
        at: "crates/x/tests/t.rs".into(),
        embeds,
    };
    let found = [
        read("swarm-ui-egui", Some("assets/fonts/a.ttf"), true),
        read("swarm-source", Some("docs/code-health.toml"), false),
        read("swarm-engine", Some("docs/control-protocol.md"), false),
        read("swarm-engine", Some(""), false),
    ];
    let probes = |_: &reads::Read, target: &str| vec![target.to_owned()];
    let missed = missed(&App, &found, &packages(), &graph(), &probes);
    assert_eq!(missed.len(), 2, "{missed:#?}");
    assert!(
        missed[0].starts_with("swarm-engine reads docs/control-protocol.md (crates/x/tests/t.rs), and the plan leaves out [\"swarm-engine\"]"),
        "{}",
        missed[0]
    );
    assert!(missed[1].contains("the whole workspace or outside it"), "{}", missed[1]);
}
