use serde_json::json;

use super::*;

const ENTRIES: &[Entry] = &[
    Entry { package: "app", layer: None, kind: "app", summary: "the app" },
    Entry { package: "my-ui", layer: Some(2), kind: "L2", summary: "the UI" },
    Entry { package: "my-engine", layer: Some(1), kind: "L1", summary: "commands" },
    Entry { package: "my-core", layer: Some(0), kind: "L0", summary: "the model" },
    Entry { package: "my-testkit", layer: None, kind: "dev", summary: "test helpers" },
];

const TABLE: Table = Table {
    entries: ENTRIES,
    ui_layer: 2,
    ui_crates: &["egui", "eframe"],
    dev_only: &["my-testkit"],
    reply_readers: &["crates/ui/src/", "apps/"],
    allowed_reads: &[],
};

fn member(name: &str, deps: &[(&str, bool)]) -> Member {
    Member { name: name.into(), dir: PathBuf::from("/nowhere"), deps: deps.iter().map(|(d, dev)| ((*d).to_owned(), *dev)).collect() }
}

fn of(name: &str) -> &'static Entry {
    ENTRIES.iter().find(|e| e.package == name).unwrap()
}

#[test]
fn a_crate_depends_only_on_strictly_lower_layers() {
    assert!(dependencies(&TABLE, of("my-engine"), &member("my-engine", &[("my-core", false), ("serde", false)])).is_empty());
    let up = dependencies(&TABLE, of("my-engine"), &member("my-engine", &[("my-ui", false)]));
    assert!(up.len() == 1 && up[0].contains("only on strictly lower layers"), "{up:?}");
    let app = dependencies(&TABLE, of("my-ui"), &member("my-ui", &[("app", false)]));
    assert!(app.len() == 1 && app[0].contains("an app or tool"), "{app:?}");
    // A dev-dependency may reach anywhere: tests drive the layers above.
    assert!(dependencies(&TABLE, of("my-core"), &member("my-core", &[("my-engine", true), ("egui", true)])).is_empty());
}

#[test]
fn ui_crates_stay_at_the_ui_layer_and_up_and_a_testkit_is_dev_only() {
    let low = dependencies(&TABLE, of("my-engine"), &member("my-engine", &[("egui", false)]));
    assert!(low.len() == 1 && low[0].contains("UI crates are for L2 and up"), "{low:?}");
    assert!(dependencies(&TABLE, of("my-ui"), &member("my-ui", &[("egui", false), ("my-engine", false)])).is_empty());
    let kit = dependencies(&TABLE, of("my-core"), &member("my-core", &[("my-testkit", false)]));
    assert!(kit.len() == 1 && kit[0].contains("only ever a dev-dependency"), "{kit:?}");
}

#[test]
fn every_member_is_registered_and_every_entry_a_member() {
    let members = [member("my-core", &[]), member("stray", &[])];
    let problems = registration(&TABLE, &members);
    assert!(problems.iter().any(|p| p.starts_with("stray is not in the layers table")), "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("lists my-engine, which is not a workspace member")), "{problems:?}");
}

#[test]
fn crate_roots_carry_the_lint_headers() {
    let dir = swarm_core_testkit::temp_dir("layers-headers");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n\n[lints]\nworkspace = true\n").unwrap();
    let good = format!("//! x\n#![forbid(unsafe_code)]\n{DENY_LINE}\n{LIMITS_LINE}\n{LIMITS_TEST_LINE}\n");
    std::fs::write(dir.join("src/lib.rs"), &good).unwrap();
    let m = Member { name: "x".into(), dir: dir.clone(), deps: Vec::new() };
    assert!(headers(&m, Safety::Forbidden).is_empty(), "{:?}", headers(&m, Safety::Forbidden));
    std::fs::write(dir.join("src/lib.rs"), "//! x\n").unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let problems = headers(&m, Safety::Forbidden);
    assert_eq!(problems.len(), 5, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("forbid(unsafe_code)")) && problems.iter().any(|p| p.contains("[lints] workspace = true")));
}

#[test]
fn a_root_allowed_unsafe_carries_its_own_lines_in_place_of_forbid() {
    let dir = swarm_core_testkit::temp_dir("layers-unsafe");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"gpu\"\n\n[lints]\nworkspace = true\n").unwrap();
    let base = format!("//! gpu\n{DENY_LINE}\n{LIMITS_LINE}\n{LIMITS_TEST_LINE}\n");
    std::fs::write(dir.join("src/lib.rs"), &base).unwrap();
    let m = Member { name: "gpu".into(), dir: dir.clone(), deps: Vec::new() };
    let problems = headers(&m, Safety::Allowed);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().all(|p| p.contains("allowed `unsafe`")), "{problems:?}");
    std::fs::write(dir.join("src/lib.rs"), format!("{base}{}\n{}\n", UNSAFE_LINES[0], UNSAFE_LINES[1])).unwrap();
    assert!(headers(&m, Safety::Allowed).is_empty(), "{:?}", headers(&m, Safety::Allowed));
    assert_eq!(headers(&m, Safety::Forbidden).len(), 1, "a package not allowed still forbids");
    assert!(headers(&m, Safety::Exempt).is_empty());
}

#[test]
fn code_health_exemptions_are_reasoned_expects_and_counted() {
    let good = [
        "#[expect(clippy::too_many_lines, reason = \"egui layout\")]",
        "fn a() {}",
        "    #[expect(clippy::cognitive_complexity, clippy::too_many_lines, reason = \"debt\")]",
        "#[expect(clippy::too_many_arguments, reason = \"a drawing helper\")]",
        "/// #[allow(clippy::too_many_lines)] in a doc is prose",
        LIMITS_LINE,
        LIMITS_TEST_LINE,
        "#[allow(clippy::needless_range_loop)]",
    ]
    .join("\n");
    let (counts, problems) = limit_exemptions(&good, "src/a.rs");
    assert_eq!(counts, [1, 2, 1]);
    assert!(problems.is_empty(), "{problems:?}");
    for (bad, why) in [
        ("#[allow(clippy::too_many_lines)]", "not #[allow]"),
        ("#[expect(clippy::cognitive_complexity)]", "needs a `reason`"),
        ("#![allow(clippy::too_many_arguments)]", "no crate-wide"),
    ] {
        let (_, problems) = limit_exemptions(&format!("fn x() {{}}\n{bad}"), "src/b.rs");
        assert!(problems.len() == 1 && problems[0].starts_with("src/b.rs:2: ") && problems[0].contains(why), "{bad}: {problems:?}");
    }
    let mut by = BTreeMap::new();
    by.insert("engine".to_owned(), [1, 2, 0]);
    by.insert("core".to_owned(), [0, 0, 0]);
    assert_eq!(exemptions_line(&by), "layers: code-health exemptions: 3 (cognitive_complexity 1, too_many_lines 2, too_many_arguments 0; engine 3)");
}

#[test]
fn untyped_reads_are_indexes_gets_and_objects_held_to_their_allowance() {
    assert_eq!(count_reads(r#"let a = v["field"].as_str();"#), 1);
    assert_eq!(count_reads(r#"x.get("activity").and_then(|a| a.get("kind"))"#), 2);
    assert_eq!(count_reads("let Some(o) = v.as_object() else { return };"), 1);
    assert_eq!(count_reads(r#"const A: &[&str] = &["egui"]; f(["a", "b"]); m.get(key); v.as_object_mut()"#), 0);
    let text = "fn a() { v[\"x\"]; }\n// v[\"y\"]\n#[cfg(test)]\nmod tests {\n    v[\"t\"];\n}\n";
    assert_eq!(production(text).map(count_reads).sum::<usize>(), 1);
    let found = [("apps/a.rs".to_owned(), 2), ("apps/b.rs".to_owned(), 0)];
    let allowed = [Allowed { file: "apps/a.rs", count: 1, reason: "older" }, Allowed { file: "apps/gone.rs", count: 1, reason: "x" }];
    let problems = ratchet(&found, &allowed);
    assert!(problems.iter().any(|p| p.starts_with("apps/a.rs: 2 untyped") && p.contains("allows 1 (older)")), "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("apps/gone.rs, which isn't")), "{problems:?}");
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(ratchet(&[("apps/a.rs".to_owned(), 0)], &[Allowed { file: "apps/a.rs", count: 1, reason: "" }])[0].ends_with("lower its entry to 0"));
}

#[test]
fn check_reads_members_from_cargo_metadata() {
    let root = swarm_core_testkit::temp_dir("layers-check");
    for (dir, lib) in [("crates/core", true), ("crates/ui", true)] {
        std::fs::create_dir_all(root.join(dir).join("src")).unwrap();
        std::fs::write(root.join(dir).join("Cargo.toml"), "[package]\n\n[lints]\nworkspace = true\n").unwrap();
        let header = format!("#![forbid(unsafe_code)]\n{DENY_LINE}\n{LIMITS_LINE}\n{LIMITS_TEST_LINE}\n");
        let body = if dir == "crates/ui" { "fn f(v: &Value) { v[\"field\"]; }\n" } else { "" };
        std::fs::write(root.join(dir).join(if lib { "src/lib.rs" } else { "src/main.rs" }), format!("{header}{body}")).unwrap();
    }
    let at = |d: &str| root.join(d).join("Cargo.toml").display().to_string();
    let meta = json!({"packages": [
        {"name": "my-core", "manifest_path": at("crates/core"), "dependencies": []},
        {"name": "my-ui", "manifest_path": at("crates/ui"), "dependencies": [{"name": "my-core", "kind": null}, {"name": "egui", "kind": null}]},
    ]});
    const CHECKED: &[Entry] = &[ENTRIES[1], ENTRIES[3]];
    let problems = check(&Table { entries: CHECKED, reply_readers: &["crates/ui/src/"], ..TABLE }, &meta, &root);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].starts_with("crates/ui/src/lib.rs: 1 untyped reply read"), "{problems:?}");
}

#[test]
fn the_crate_map_is_rendered_between_its_markers() {
    let map = crate_map(&TABLE);
    assert!(map.contains(" L1   my-engine   commands\n") && map.starts_with("```text\n"), "{map}");
    let text = format!("# A\n\n{MAP_START}\nold\n{MAP_END}\n\nafter\n");
    let new = with_map(&text, &map).unwrap();
    assert!(new.contains(&format!("{MAP_START}\n{map}{MAP_END}")) && new.ends_with("after\n") && !new.contains("old"), "{new}");
    assert_eq!(with_map(&new, &map).unwrap(), new, "rendering again changes nothing");
    assert!(with_map("# no markers", &map).is_none());
}

#[test]
fn raw_nul_bytes_in_tracked_source_are_refused() {
    use swarm_core_testkit::git::git as tgit;
    let dir = swarm_core_testkit::temp_dir("layers-nul");
    tgit(&dir, &["init", "--quiet"]);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/bad.rs"), b"fn a() {}\nlet s = \"x\0y\";\n").unwrap();
    std::fs::write(dir.join("src/good.rs"), "let s = \"x\\u{0}y\";\n").unwrap();
    tgit(&dir, &["add", "src"]);
    std::fs::write(dir.join("untracked.rs"), b"\0").unwrap();
    let found = raw_nuls(&dir);
    assert!(found.len() == 1 && found[0].starts_with("src/bad.rs:2: a raw NUL"), "{found:?}");
    assert_eq!(nul_lines(b"\0\n\n\0\0\na"), [1, 3]);
}

/// A project that declares its layers in its manifests, not in a table.
struct Manifests;
impl crate::Hooks for Manifests {
    fn architecture(&self) -> Option<crate::checks::Layers> {
        Some(crate::checks::Layers::STANDARD)
    }
}

/// A one-member repository whose manifest declares its layer; `agents` is its AGENTS.md.
fn declared_workspace(name: &str, agents: &str) -> std::path::PathBuf {
    let dir = swarm_core_testkit::temp_dir(name);
    swarm_core_testkit::git::git(&dir, &["init", "--quiet"]);
    std::fs::create_dir_all(dir.join("crates/core/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/core\"]\n").unwrap();
    let manifest = "[package]\nname = \"my-core\"\ndescription = \"the model\"\n[package.metadata.swarm]\nlayer = \"model\"\n[lints]\nworkspace = true\n";
    std::fs::write(dir.join("crates/core/Cargo.toml"), manifest).unwrap();
    let root = format!(
        "#![forbid(unsafe_code)]\n{DENY_LINE}\n{LIMITS_LINE}\n{LIMITS_TEST_LINE}\n#[expect(clippy::too_many_lines, reason = \"a table\")]\nfn f() {{}}\n"
    );
    std::fs::write(dir.join("crates/core/src/lib.rs"), root).unwrap();
    std::fs::write(dir.join("AGENTS.md"), agents).unwrap();
    dir
}

/// `layers` on a project whose manifests declare its layers: the manifests' checks run, and the
/// two rules the table path holds are held too: the exemptions, and the crate map where
/// AGENTS.md has the markers.
#[test]
fn a_project_that_declares_its_layers_in_its_manifests_is_checked_from_them() {
    let dir = declared_workspace("layers-manifests", &format!("# Agents\n{MAP_START}\n{MAP_END}\n"));
    let stale = run(&Manifests, &dir, &[]).unwrap_err();
    assert!(stale.contains("1 layering problem(s)") && stale.contains("crate map is stale"), "{stale}");
    run(&Manifests, &dir, &["--write"]).unwrap();
    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(agents.contains(" model my-core  the model\n"), "{agents}");
    run(&Manifests, &dir, &[]).unwrap();
    // A member with no layer is a finding of the manifests' check, named as it names it.
    std::fs::write(dir.join("crates/core/Cargo.toml"), "[package]\nname = \"my-core\"\n[lints]\nworkspace = true\n").unwrap();
    let found = run(&Manifests, &dir, &[]).unwrap_err();
    assert!(found.contains("crates/core/Cargo.toml") && found.contains("[layer-declared]"), "{found}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An AGENTS.md with no markers keeps no generated map: nothing to hold on the manifests' path.
/// A project that configures nothing has nothing checked, as before.
#[test]
fn no_markers_is_no_map_and_no_configuration_is_nothing_to_check() {
    let dir = declared_workspace("layers-manifests-no-map", "# Agents\n");
    run(&Manifests, &dir, &[]).unwrap();
    // A code-health exemption without a reason is a problem on this path too.
    let lib = dir.join("crates/core/src/lib.rs");
    std::fs::write(&lib, std::fs::read_to_string(&lib).unwrap().replace(", reason = \"a table\"", "")).unwrap();
    assert!(run(&Manifests, &dir, &[]).unwrap_err().contains("needs a `reason`"));
    run(&crate::Defaults, &dir, &[]).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
