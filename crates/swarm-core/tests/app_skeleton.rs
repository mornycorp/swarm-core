//! The app skeleton, built for real (mornycorp/swarm-core#49). Three tests, all `#[ignore]`d
//! because they fetch crates from the registry and build for minutes: a normal `cargo test
//! --workspace` stays fast, and a change to the skeleton runs them before it merges (AGENTS.md):
//!
//! ```text
//! cargo test -p swarm-core --test app_skeleton -- --ignored --nocapture
//! ```
//!
//! - **The rendered skeleton** (`init --app` into an empty workspace) passes its own `cargo test`,
//!   `cargo fmt --check`, `cargo clippy -D warnings`, and the architecture's checks
//!   (`checks::check`); and the checks do catch a layer broken and an untyped read. So does the
//!   one rendered with the UI shell (`init --app --ui`), whose checks
//!   also catch a toolkit in a lower layer and a file write, a process start, an untyped read, a
//!   colour made in place and a word written in place in the UI crate.
//! - **A new repo that follows docs/new-swarm.md** gets a green `cargo xtask ci`, first with no app
//!   (steps 2, 4 and 6) and then as an app (step 5): the guide's manifest, pin and cargo config,
//!   `xtask/Cargo.toml`, the xtask copied from `examples/consumer_xtask.rs`, `swarm-core init`,
//!   `swarm-core init --app`, `sync-assets`. The guide's own code blocks are what the test writes, so
//!   a step the guide leaves out fails here first.
//!
//! One test that builds nothing runs every time: the example's checks on the guide's workspace with
//! no app (no findings) and with the skeleton (its scans).
//!
//! Swarm Core the new repo pins is **this checkout**, testkit included (the skeleton's tests run
//! Swarm Core's conformance run, a `swarm-core-testkit` dev-dependency pinned like Swarm Core: the
//! rendered skeleton's manifest pins it too): the test commits a snapshot of it into a
//! temporary git repository and has git resolve the pin's URL to it for the build's processes
//! (`GIT_CONFIG_COUNT`, appended to any inherited: git's `url.<base>.insteadOf`, which cargo's
//! `git-fetch-with-cli` honours), so no `[patch]` is written anywhere and no access to
//! Swarm Core's remote is needed. Only the crates registry is.
//!
//! **Isolated from the developer's cargo:** every cargo the tests start has its own `CARGO_HOME`
//! (`<target>/tmp/app-skeleton-cargo-home`, kept between runs so the registry's downloads are reused),
//! so a user-level `[patch]` of Swarm Core (AGENTS.md's way to iterate from a consumer) doesn't
//! apply and the snapshots never reach the real `$CARGO_HOME/git`. Cargo also reads a
//! `.cargo/config.toml` above the test's directory, so each test checks that Swarm Core it built
//! is the snapshot, and fails saying so when a `[patch]` replaced it.
//!
//! **Pinned versions:** each test lays down a lock from `tests/locks/` (`render.lock`,
//! `render-ui.lock`, `guide.lock`: the snapshot's commit is `{{rev}}` there) and builds `--locked`
//! against it, so a new release of serde, schemars or egui doesn't change what CI builds. A change
//! here that moves Swarm Core's version or dependencies, or the skeleton's, makes the locks
//! stale: the tests fail saying so. Refresh them (fresh versions, then the tests on them) with
//!
//! ```text
//! SWARM_CORE_REFRESH_LOCKS=1 cargo test -p swarm-core --test app_skeleton -- --ignored
//! ```
//!
//! and commit the locks it writes.
//!
//! Offline (the registry isn't reachable) or without a tool the e2e needs (`cargo nextest`), a test
//! **fails**, saying which: they run only when asked for, and there is no CI server to notice a
//! run that skipped itself and passed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use swarm_core::changed::Rules;
use swarm_core::checks::{self, Scan};
use swarm_core::{Hooks, Project, pin, process, skeleton};
use swarm_core_testkit::temp_dir;

/// The app's root manifest in the render test: what the skeleton's crates inherit, Swarm Core's
/// testkit among it, pinned (at `rev`, a commit of the snapshot) as a consumer pins Swarm Core.
fn manifest(rev: &str) -> String {
    let testkit = format!("swarm-core-testkit = {{ git = \"{}\", rev = \"{rev}\" }}", pin::URL);
    let deps = format!("serde = \"1\"\nserde_json = \"1\"\nserde_path_to_error = \"0.1\"\nschemars = \"1\"\n{testkit}");
    format!(
        "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\", \"apps/*\"]\n\n[workspace.dependencies]\n{deps}\n\n[workspace.lints.rust]\nunsafe_code = \"forbid\"\n"
    )
}

/// The values of the guide's example xtask (`examples/consumer_xtask.rs`), as the tests use it, for
/// the workspace at `root`: its scans are the example's, `skeleton::scans_at(root)` (none without an
/// app, the UI crate's with the UI shell).
struct App {
    root: PathBuf,
}

impl Hooks for App {
    fn changed(&self) -> Rules {
        Rules { package_prefix: "my-", ..Rules::NONE }
    }

    fn architecture(&self) -> Option<checks::Layers> {
        Some(checks::Layers::STANDARD)
    }

    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("product", "My Swarm"),
            ("app", "my-app"),
            ("cli", "my-cli"),
            ("ui_crate", "my-ui"),
            ("task_links", "- Gap: none"),
            ("task_done", "- [ ] Docs updated"),
            ("mr_moves", "gap ids"),
            ("mr_ci", ""),
            ("mr_ci_steps", ""),
            ("mr_docs", "Technical docs current"),
        ]
    }

    fn scans(&self) -> Vec<Scan> {
        skeleton::scans_at(&self.root)
    }
}

/// Whether this run refreshes the locks under `tests/locks/` instead of building against them.
fn refreshing() -> bool {
    std::env::var_os("SWARM_CORE_REFRESH_LOCKS").is_some_and(|v| v != "0" && !v.is_empty())
}

/// A test can't run here (`why`): a failure. These run only when asked for, and with no CI server
/// a run that skipped itself and passed would be read as a pass.
fn cannot(test: &str, why: &str) {
    panic!("{test}: {why}. Run it where the crates registry is reachable and `cargo nextest` is installed.");
}

/// Where a test's builds go: under the integration tests' scratch directory, so a rerun reuses
/// what the last one compiled.
fn target(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// The developer's (or CI's) own cargo home: `CARGO_HOME`, else `~/.cargo`.
fn real_cargo_home() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".cargo"));
    std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(home)
}

/// The cargo home every cargo of these tests runs with: its own, so the developer's config (a
/// `[patch]` of Swarm Core) doesn't apply and the snapshots' commits stay out of the real git
/// cache. Kept between runs, as the registry's downloads are large. Its config fetches git with git's
/// own client, as the guide's `.cargo/config.toml` does. Made once per run: the tests run in
/// parallel, and a cargo must never read a config another test is writing.
fn isolated_env() -> &'static [(String, String)] {
    static ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();
    ENV.get_or_init(|| {
        let home = target("app-skeleton-cargo-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.toml"), "[net]\ngit-fetch-with-cli = true\n").unwrap();
        let mut env = vec![("CARGO_HOME".to_owned(), home.to_string_lossy().into_owned())];
        // The tools installed in the real home (`cargo-nextest`) stay where cargo looks for them.
        if let Some(bin) = real_cargo_home().map(|h| h.join("bin")) {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let joined = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path))).unwrap();
            env.push(("PATH".to_owned(), joined.to_string_lossy().into_owned()));
        }
        env
    })
}

/// `cargo <args>` in `dir`, its target dir and extra environment given, with the tests' own cargo
/// home ([`isolated_env`]): its output, or a failure that carries the output's tail.
fn cargo(dir: &Path, target: &Path, args: &[&str], env: &[(String, String)]) -> Result<String, String> {
    let program = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(program);
    cmd.current_dir(dir).args(args).envs(isolated_env().iter().map(|(k, v)| (k, v))).envs(env.iter().map(|(k, v)| (k, v)));
    cmd.env("CARGO_TARGET_DIR", target).env("CARGO_TERM_COLOR", "never");
    // A test run under nextest must not hand its run's settings to the nextest it starts.
    for (key, _) in std::env::vars() {
        if key.starts_with("NEXTEST") || key == "CARGO_MANIFEST_DIR" {
            cmd.env_remove(key);
        }
    }
    let out = cmd.output().map_err(|e| format!("cargo {}: {e}", args.join(" ")))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        return Ok(text);
    }
    let tail: String = text.chars().rev().take(8000).collect::<Vec<_>>().into_iter().rev().collect();
    Err(format!("cargo {} in {} failed ({}):\n{tail}", args.join(" "), dir.display(), out.status))
}

/// Whether the crates registry is reachable: a `cargo fetch` of a tiny crate (as cargo is
/// configured here, proxy included).
fn registry_reachable(name: &str) -> Result<(), String> {
    let dir = temp_dir(&format!("app-skeleton-probe-{name}"));
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("src/lib.rs"), "").map_err(|e| e.to_string())?;
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nitoa = \"1\"\n")
        .map_err(|e| e.to_string())?;
    cargo(&dir, &target("probe"), &["fetch"], &[]).map(drop).map_err(|e| e.lines().find(|l| l.contains("error")).unwrap_or("cargo fetch failed").to_owned())
}

/// The first reason a tool of `tools` (`cargo <tool> --version`) isn't installed.
fn missing_tool(dir: &Path, tools: &[&str]) -> Option<String> {
    tools.iter().find(|tool| cargo(dir, &target("probe"), &[tool, "--version"], &[]).is_err()).map(|tool| format!("`cargo {tool}` isn't installed"))
}

/// The lock kept for the test `name` (`tests/locks/<name>.lock`).
fn lock_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/locks").join(format!("{name}.lock"))
}

/// The command that refreshes the locks, for a failure to name.
const REFRESH: &str = "SWARM_CORE_REFRESH_LOCKS=1 cargo test -p swarm-core --test app_skeleton -- --ignored";

/// Lay the kept lock `name` into `dir` as its `Cargo.lock`, the snapshot's commit `rev` in it; nothing
/// when [`refreshing`], where cargo resolves afresh.
fn lay_lock(dir: &Path, name: &str, rev: &str) {
    if refreshing() {
        return;
    }
    let kept = std::fs::read_to_string(lock_fixture(name)).unwrap_or_else(|e| panic!("{}: {e}; write it with `{REFRESH}`", lock_fixture(name).display()));
    std::fs::write(dir.join("Cargo.lock"), kept.replace("\r\n", "\n").replace("{{rev}}", rev)).unwrap();
}

/// When [`refreshing`], keep `dir`'s `Cargo.lock` (green by now) as the lock `name`, the snapshot's
/// commit `rev` written `{{rev}}`.
fn keep_lock(dir: &Path, name: &str, rev: &str) {
    if refreshing() {
        let lock = std::fs::read_to_string(dir.join("Cargo.lock")).unwrap();
        std::fs::write(lock_fixture(name), lock.replace("\r\n", "\n").replace(rev, "{{rev}}")).unwrap();
        println!("refreshed {}", lock_fixture(name).display());
    }
}

/// Resolve the workspace at `dir` (`--locked` to the lock laid down, unless [`refreshing`]) and check
/// that each of Swarm Core's packages in it is the snapshot at `rev`: a `[patch]` in a cargo config
/// the tests can't keep out (a `.cargo/config.toml` above `dir`) would build something else.
fn resolve_to_snapshot(dir: &Path, build: &Path, env: &[(String, String)], rev: &str) {
    let mut args = vec!["metadata", "--format-version", "1"];
    if !refreshing() {
        args.push("--locked");
    }
    let out = cargo(dir, build, &args, env).unwrap_or_else(|e| {
        panic!("{e}\n\nThe lock under tests/locks/ is stale (a change to Swarm Core's or the skeleton's dependencies or version): refresh it with `{REFRESH}`. Or a `[patch]` of Swarm Core applies (a .cargo/config.toml above {}): remove it while the e2e runs.", dir.display())
    });
    // Its JSON is one line of stdout; cargo's own lines (stderr) come after it.
    let json = out.lines().find(|l| l.starts_with('{')).unwrap_or_default();
    let meta: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|e| panic!("cargo metadata: {e}"));
    let want = format!("git+{}?rev={rev}#{rev}", pin::URL);
    let ours: Vec<&serde_json::Value> =
        meta["packages"].as_array().into_iter().flatten().filter(|p| [pin::PACKAGE, pin::TESTKIT].contains(&p["name"].as_str().unwrap_or_default())).collect();
    assert!(!ours.is_empty(), "the workspace at {} depends on Swarm Core", dir.display());
    for package in ours {
        let source = package["source"].as_str().unwrap_or("a path");
        assert_eq!(
            source,
            want,
            "{} comes from {source}, not the snapshot of this checkout: a `[patch]` of Swarm Core in a cargo config above {} applies. Remove it while the e2e runs.",
            package["name"],
            dir.display()
        );
    }
}

/// Add the architecture's own failures to the skeleton at `root`: a layer broken in a manifest and an
/// untyped read in the automation crate. Each must be a finding of the right rule.
fn the_checks_catch_what_they_should(app: &App, root: &Path) {
    let manifest = root.join("crates/model/Cargo.toml");
    let original = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, format!("{original}\n[dependencies]\nmy-engine = {{ path = \"../engine\" }}\n")).unwrap();
    let found = checks::check(app, root).err().map_or_else(String::new, |f| f.to_string());
    assert!(found.contains("[layer-order]") && found.contains("crates/model/Cargo.toml"), "a layer broken is found: {found}");
    std::fs::write(&manifest, original).unwrap();
    let serve = root.join("crates/automation/src/serve.rs");
    let original = std::fs::read_to_string(&serve).unwrap();
    std::fs::write(&serve, format!("fn untyped(v: serde_json::Value) {{ let _ = v[\"k\"]; }}\n{original}")).unwrap();
    let found = checks::check(app, root).err().map_or_else(String::new, |f| f.to_string());
    assert!(found.contains("[value-reads]") && found.contains("crates/automation/src/serve.rs"), "an untyped read is found: {found}");
    std::fs::write(&serve, original).unwrap();
}

/// What the checks say of `root` once `change` has been applied to the file `path` (put back after).
fn found_after(app: &App, root: &Path, path: &str, change: impl FnOnce(&str) -> String) -> String {
    let file = root.join(path);
    let original = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, change(&original)).unwrap();
    let found = checks::check(app, root).err().map_or_else(String::new, |f| f.to_string());
    std::fs::write(&file, original).unwrap();
    found
}

/// The UI crate's own rules: a toolkit below the UI layer, and a file write, a process start, an
/// untyped read, a colour made in place and a word written in place in the UI crate, are each a
/// finding of the right rule in the right file.
fn the_checks_catch_the_ui_crates_breaks(app: &App, root: &Path) {
    let found = found_after(app, root, "crates/automation/Cargo.toml", |m| m.replace("[dependencies]\n", "[dependencies]\negui = \"0.37\"\n"));
    assert!(found.contains("[ui-toolkit]") && found.contains("crates/automation/Cargo.toml"), "a toolkit in the automation layer: {found}");
    let found = found_after(app, root, "apps/my-cli/Cargo.toml", |m| m.replace("[dependencies]\n", "[dependencies]\neframe = \"0.37\"\n"));
    assert!(found.contains("[ui-toolkit]") && found.contains("apps/my-cli/Cargo.toml"), "a toolkit in the CLI: {found}");
    let views = "crates/ui/src/views.rs";
    for (rule, line) in [
        ("ui-file-writes", "fn writes() { let _ = std::fs::write(\"x\", \"\"); }"),
        ("ui-process-starts", "fn starts() { let _ = std::process::Command::new(\"x\"); }"),
        ("value-reads", "fn reads(v: serde_json::Value) { let _ = v[\"k\"]; }"),
        ("ui-colours", "fn colour() -> egui::Color32 { egui::Color32::RED }"),
        ("ui-strings", "fn word(ui: &mut egui::Ui) { ui.label(\"Run\"); }"),
    ] {
        let found = found_after(app, root, views, |text| format!("{line}\n{text}"));
        assert!(found.contains(&format!("[{rule}]")) && found.contains(views), "{rule} in the UI crate: {found}");
    }
}

#[test]
#[ignore = "needs the crates registry and builds for minutes: cargo test -p swarm-core --test app_skeleton -- --ignored"]
fn the_rendered_skeleton_passes_its_own_tests_lints_and_architecture_checks() {
    rendered_skeleton_passes("the_rendered_skeleton_passes_its_own_tests_lints_and_architecture_checks", false);
}

#[test]
#[ignore = "needs the crates registry and builds egui for minutes: cargo test -p swarm-core --test app_skeleton -- --ignored"]
fn the_rendered_skeleton_with_the_ui_passes_its_own_tests_lints_and_architecture_checks() {
    rendered_skeleton_passes("the_rendered_skeleton_with_the_ui_passes_its_own_tests_lints_and_architecture_checks", true);
}

/// `init --app` (with `--ui` when `ui`) into an empty workspace, then the skeleton's own tests, lints
/// and the architecture's checks, and the checks' catches.
fn rendered_skeleton_passes(test: &str, ui: bool) {
    if let Err(why) = registry_reachable(if ui { "render-ui" } else { "render" }) {
        return cannot(test, &format!("the crates registry isn't reachable ({why})"));
    }
    let root = temp_dir(if ui { "app-skeleton-render-ui" } else { "app-skeleton-render" });
    let app = App { root: root.clone() };
    // The testkit is pinned as a consumer pins it; git resolves the pin's URL to a snapshot of this checkout.
    let name = if ui { "render-ui" } else { "render" };
    let (core, rev) = snapshot(name);
    std::fs::write(root.join("Cargo.toml"), manifest(&rev)).unwrap();
    let written = skeleton::init(&Project::EXAMPLE, &app, &root, "2026-11-02", ui).unwrap();
    assert_eq!(written.len(), skeleton::FILES.len() + if ui { skeleton::UI_FILES.len() } else { 0 });
    assert_eq!(root.join("crates/ui").exists(), ui, "the UI crate is written with --ui and only then");
    let (build, env) = (target(name), resolve_pin_to(&core));
    lay_lock(&root, name, &rev);
    resolve_to_snapshot(&root, &build, &env, &rev);
    let locked: &[&str] = if refreshing() { &["test", "--workspace"] } else { &["test", "--workspace", "--locked"] };
    for args in [locked, &["fmt", "--all", "--", "--check"], &["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]] {
        cargo(&root, &build, args, &env).unwrap_or_else(|e| panic!("{e}"));
    }
    let report = checks::check(&app, &root).unwrap_or_else(|f| panic!("{f}"));
    println!("{report}");
    let (members, which) = if ui {
        (8, "model, backends, engine, automation, testkit, the CLI, the UI crate and the window")
    } else {
        (6, "model, backends, engine, automation, testkit and the CLI")
    };
    assert_eq!(report.layers.as_ref().map(|l| l.members), Some(members), "{which}");
    the_checks_catch_what_they_should(&app, &root);
    if ui {
        the_checks_catch_the_ui_crates_breaks(&app, &root);
    }
    keep_lock(&root, name, &rev);
    // Green: the snapshot of Swarm Core is no longer needed.
    let _ = std::fs::remove_dir_all(&core);
}

/// The fenced blocks of language `lang` in the guide's section whose heading starts with `heading`.
fn guide_blocks(guide: &str, heading: &str, lang: &str) -> Vec<String> {
    let mut in_section = false;
    let (mut blocks, mut open): (Vec<String>, Option<(usize, String)>) = (Vec::new(), None);
    for line in guide.lines() {
        if line.starts_with("## ") {
            in_section = line.starts_with(heading);
        }
        let fence = line.trim_start().strip_prefix("```");
        match (&mut open, fence) {
            (None, Some(l)) if in_section && l.trim() == lang => open = Some((line.len() - line.trim_start().len(), String::new())),
            (Some((_, body)), Some(_)) => {
                blocks.push(std::mem::take(body));
                open = None;
            }
            (Some((indent, body)), None) => {
                body.push_str(line.get(*indent..).unwrap_or(line.trim_start()));
                body.push('\n');
            }
            _ => {}
        }
    }
    blocks
}

/// docs/new-swarm.md, as the tests follow it.
fn guide() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/new-swarm.md")).unwrap().replace("\r\n", "\n")
}

/// The example xtask, `examples/consumer_xtask.rs`, as a new repo copies it.
fn example() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/consumer_xtask.rs")).unwrap()
}

/// The guide's step 2 blocks (the manifest pinned at `rev`, `.cargo/config.toml`, `xtask/Cargo.toml`,
/// `rustfmt.toml`) and the example xtask, written into `repo`. The manifest, for step 5 to change.
fn write_step_2(guide: &str, repo: &Path, rev: &str) -> String {
    let blocks = guide_blocks(guide, "## 2. The xtask", "toml");
    assert_eq!(
        blocks.len(),
        4,
        "docs/new-swarm.md step 2 shows four toml blocks: the manifest, .cargo/config.toml, xtask/Cargo.toml and rustfmt.toml: {blocks:?}"
    );
    let (manifest, _) = pin::set_rev(&blocks[0], rev).unwrap_or_else(|e| panic!("the guide's manifest: {e}"));
    write(repo, "Cargo.toml", &manifest);
    write(repo, ".cargo/config.toml", &blocks[1]);
    write(repo, "xtask/Cargo.toml", &blocks[2]);
    write(repo, "rustfmt.toml", &blocks[3]);
    write(repo, "xtask/src/main.rs", &example());
    manifest
}

/// The guide's manifest with step 5's `members` line (the skeleton's crates added).
fn with_the_crates(guide: &str, manifest: &str) -> String {
    let members = guide_blocks(guide, "## 5. The app's skeleton", "toml");
    let members = members.first().expect("docs/new-swarm.md step 5 shows the members line to add").trim_end().to_owned();
    manifest.lines().map(|l| if l.starts_with("members = ") { members.as_str() } else { l }).collect::<Vec<_>>().join("\n") + "\n"
}

/// Write `text` to `path` under `repo`, making its directory.
fn write(repo: &Path, path: &str, text: &str) {
    let at = repo.join(path);
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::fs::write(at, text).unwrap();
}

/// The example's checks (`cargo xtask layers`: `checks::check` with the example's scans,
/// `skeleton::scans_at`) on the guide's workspace. With no app: no findings and no scan (each would
/// read no file). Once `init --app` has written the skeleton: the skeleton's scans; with `--ui`, the UI
/// crate's too. Nothing is built, so it runs every time.
#[test]
fn the_examples_checks_pass_with_no_app_and_hold_an_app_to_the_skeletons_scans() {
    let example = example();
    assert!(
        example.contains("skeleton::scans_at(&root())") && example.contains("steps(&c, &scope, skeleton::is_app(root))"),
        "the example asks the workspace whether it is an app, for its scans and its conformance step"
    );
    let guide = guide();
    let repo = temp_dir("app-example-checks");
    let app = App { root: repo.clone() };
    let manifest = write_step_2(&guide, &repo, &"0".repeat(40));
    let report = checks::check(&app, &repo).unwrap_or_else(|f| panic!("a repo with no app:\n{f}"));
    assert_eq!((report.layers.as_ref().map(|l| l.members), report.scan.rules.len()), (Some(1), 0), "the xtask, and no scan: {report}");
    assert!(!skeleton::is_app(&repo));

    let unfinished = skeleton::init(&Project::EXAMPLE, &app, &repo, "2026-11-03", false).unwrap_err();
    assert!(unfinished.contains("members"), "{unfinished}");
    write(&repo, "Cargo.toml", &with_the_crates(&guide, &manifest));
    assert!(skeleton::is_app(&repo) && !skeleton::has_ui(&repo));
    let report = checks::check(&app, &repo).unwrap_or_else(|f| panic!("the app:\n{f}"));
    assert_eq!((report.layers.as_ref().map(|l| l.members), report.scan.rules.len()), (Some(7), skeleton::SCANS.len()), "the skeleton's scans: {report}");
    let found = found_after(&app, &repo, "crates/automation/src/serve.rs", |text| format!("fn reads(v: serde_json::Value) {{ let _ = v[\"k\"]; }}\n{text}"));
    assert!(found.contains("[value-reads]"), "the app is held to them: {found}");

    skeleton::init(&Project::EXAMPLE, &app, &repo, "2026-11-03", true).unwrap();
    assert!(skeleton::has_ui(&repo));
    let report = checks::check(&app, &repo).unwrap_or_else(|f| panic!("the app with the UI shell:\n{f}"));
    assert_eq!(report.scan.rules.len(), skeleton::SCANS.len() + skeleton::UI_SCANS.len(), "the UI crate's scans too: {report}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// A commit of this checkout's files (tracked, or untracked and not ignored, as they are on disk) in
/// a temporary git repository: Swarm Core the new repo pins. Only what a build of Swarm Core
/// reads (`crates/`, `docs/`, the workspace manifest and lock, the style files, and `xtask/`, a member
/// the manifest names): CI keeps its cargo home inside the checkout, untracked, and it must not be
/// copied. Its path and whole commit id.
fn snapshot(name: &str) -> (PathBuf, String) {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let listed = process::git(here, &["rev-parse", "--show-toplevel"], None, &[]).unwrap();
    let top = PathBuf::from(listed.trim());
    let listing = [
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
        "--",
        "crates",
        "docs",
        "xtask",
        "Cargo.toml",
        "Cargo.lock",
        "rustfmt.toml",
        "clippy.toml",
    ];
    let files = process::git(&top, &listing, None, &[]).unwrap();
    let copy = temp_dir(&format!("app-e2e-core-{name}"));
    for file in files.split('\0').filter(|f| !f.is_empty() && top.join(f).is_file()) {
        let to = copy.join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(top.join(file), to).unwrap();
    }
    let git = |args: &[&str]| process::git(&copy, args, None, &[]).unwrap_or_else(|e| panic!("{e}"));
    git(&["init", "-q"]);
    git(&["add", "--all"]);
    git(&[
        "-c",
        "user.name=snapshot",
        "-c",
        "user.email=snapshot@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "the checkout under test",
    ]);
    let rev = git(&["rev-parse", "HEAD"]).trim().to_owned();
    (copy, rev)
}

/// What a cargo run in the new repo sees: git resolves Swarm Core's ssh URL to the snapshot, and cargo
/// fetches it with git's own client (as the guide's `.cargo/config.toml` says, which the render test has none of).
/// The rewrite is appended to the `GIT_CONFIG_*` entries the run inherits, which stay.
fn resolve_pin_to(snapshot: &Path) -> Vec<(String, String)> {
    let base = snapshot.to_string_lossy().replace('\\', "/");
    let inherited: usize = std::env::var("GIT_CONFIG_COUNT").ok().and_then(|n| n.trim().parse().ok()).unwrap_or(0);
    vec![
        ("GIT_CONFIG_COUNT".to_owned(), (inherited + 1).to_string()),
        (format!("GIT_CONFIG_KEY_{inherited}"), format!("url.{base}.insteadOf")),
        (format!("GIT_CONFIG_VALUE_{inherited}"), pin::URL.to_owned()),
        ("CARGO_NET_GIT_FETCH_WITH_CLI".to_owned(), "true".to_owned()),
    ]
}

#[test]
#[ignore = "needs the crates registry and cargo-nextest, builds for minutes: cargo test -p swarm-core --test app_skeleton -- --ignored"]
fn a_new_repo_that_follows_the_guide_gets_a_green_ci() {
    let test = "a_new_repo_that_follows_the_guide_gets_a_green_ci";
    if let Err(why) = registry_reachable("e2e") {
        return cannot(test, &format!("the crates registry isn't reachable ({why})"));
    }
    let probe = temp_dir("app-e2e-probe");
    if let Some(why) = missing_tool(&probe, &["nextest", "fmt", "clippy"]) {
        return cannot(test, &why);
    }
    let guide = guide();

    // Step 2: the manifest and its pin, the cargo config, the xtask.
    let (core, rev) = snapshot("guide");
    let repo = temp_dir("app-e2e-repo");
    // A new repo is a git repository: `layers` reads the files git knows of.
    swarm_core_testkit::git::git(&repo, &["init", "--quiet"]);
    std::fs::write(repo.join(".gitignore"), "/target\n").unwrap();
    let manifest = write_step_2(&guide, &repo, &rev);
    // The kept lock pins the versions; cargo drops what the workspace doesn't use yet (the app's).
    lay_lock(&repo, "guide", &rev);

    // Its own target dir: `ci` records the checkout that uses one, and each run is a new repo.
    let (env, build) = (resolve_pin_to(&core), repo.join("target"));
    let xtask = |args: &[&str]| {
        let mut all = vec!["xtask"];
        all.extend_from_slice(args);
        cargo(&repo, &build, &all, &env).unwrap_or_else(|e| panic!("{e}"))
    };
    let version = xtask(&["swarm-core", "--version"]);
    assert!(version.contains(&rev), "the lock pins the snapshot {rev}: {version}");
    resolve_to_snapshot(&repo, &build, &env, &rev);

    // Steps 4 and 6, a repo with no app: the pinned copies, the starting points, then the gate. The
    // example leaves out the skeleton's scans and the conformance step.
    xtask(&["swarm-core", "sync-assets"]);
    xtask(&["swarm-core", "init"]);
    // Step 6: `layers --write` fills in AGENTS.md's crate map, which `layers` then holds.
    xtask(&["layers", "--write"]);
    let layers = xtask(&["layers"]);
    assert!(layers.contains("layers: 1 packages") && layers.contains("0 rule(s)"), "no app, no scans: {layers}");
    let ci = xtask(&["ci"]);
    assert!(ci.contains("CI summary: all ") && ci.contains("steps passed") && !ci.contains("conformance"), "a repo with no app: {ci}");

    // Step 5.3: `init --app` writes the files, then fails until the members cover them; the guide
    // shows the line to put in the manifest.
    let unfinished = cargo(&repo, &build, &["xtask", "swarm-core", "init", "--app"], &env).unwrap_err();
    assert!(unfinished.contains("the files are written") && unfinished.contains("members"), "{unfinished}");
    assert!(repo.join("crates/engine/src/registry.rs").is_file() && repo.join("AGENTS.md").is_file(), "the files are there");
    write(&repo, "Cargo.toml", &with_the_crates(&guide, &manifest));
    lay_lock(&repo, "guide", &rev);
    resolve_to_snapshot(&repo, &build, &env, &rev);
    xtask(&["swarm-core", "init", "--app"]);
    xtask(&["swarm-core", "sync-assets"]);
    xtask(&["layers", "--write"]);
    let agents = std::fs::read_to_string(repo.join("AGENTS.md")).unwrap();
    assert!(agents.contains(" engine     my-engine") || agents.contains("engine"), "the crate map names the crates: {agents}");
    let ci = xtask(&["ci"]);
    assert!(ci.contains("CI summary: all ") && ci.contains("layers") && ci.contains("conformance") && ci.contains("steps passed"), "{ci}");
    println!("{}", ci.lines().rev().find(|l| l.starts_with("CI summary")).unwrap_or_default());
    assert!(ci.contains("Summary [") && ci.contains("tests run: ") && ci.contains("passed"), "nextest ran the skeleton's tests: {ci}");
    assert!(ci.contains("test result: ok. 1 passed;"), "the conformance step ran its one test: {ci}");
    for path in ["apps/my-cli/src/main.rs", "crates/engine/src/registry.rs", "docs/swarm-core-assets.md", "AGENTS.md", ".config/nextest.toml"] {
        assert!(repo.join(path).is_file(), "{path}");
    }
    keep_lock(&repo, "guide", &rev);
    // Green: what a failure would be inspected from is no longer needed (the build alone is large).
    for dir in [&repo, &core, &probe] {
        let _ = std::fs::remove_dir_all(dir);
    }
}
