use super::*;
use crate::project::fixtures::{APP_SWARM, GPU_SWARM};

/// The first project's values for the assets' own placeholders.
pub(crate) struct App;
impl Hooks for App {
    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("product", "App Swarm"),
            ("app", "app-swarm"),
            ("cli", "swarm-cli"),
            ("ui_crate", "swarm-ui-egui"),
            ("task_links", "- Command: <!-- the command id it adds or changes, or \"none\" -->"),
            ("task_done", "- [ ] UI changes checked with the snapshot example"),
            ("mr_moves", "gap ids (G4), milestone"),
            ("mr_ci", ""),
            ("mr_ci_steps", ""),
            ("mr_docs", "Technical docs current; progress summaries left to the coordinator"),
        ]
    }
}

/// The second project's: a GPU gate, more generated docs and more for the PR to say.
pub(crate) struct Gpu;
impl Hooks for Gpu {
    fn asset_vars(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("product", "GPU Swarm"),
            ("app", "gpu-app"),
            ("cli", "gpu-cli"),
            ("ui_crate", "gpu-ui-egui"),
            (
                "task_links",
                "- Scorecard row: <!-- e.g. AUTO-5 in scorecard/automation.toml, or \"none\" -->\n- Menu item: <!-- catalog id from crates/ui-egui/src/menu_catalog.rs, or \"none\" -->",
            ),
            ("task_done", "- [ ] Scorecard row / parity checklist updated in the PR; gap movement in the PR's Progress section"),
            ("mr_moves", "gap ids (G4), scorecard rows (CAP-23), stage or milestone"),
            ("mr_ci", ", with the GPU"),
            ("mr_ci_steps", ", parity, scorecard"),
            (
                "mr_docs",
                "Scorecard rows this flips updated (`scorecard/*.toml`), generated docs regenerated, technical docs current; progress summaries left to the coordinator",
            ),
        ]
    }
}

fn asset(path: &str) -> &'static Asset {
    ASSETS.iter().find(|a| a.path == path).unwrap()
}

#[test]
fn every_file_under_assets_is_shipped() {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    let listed: Vec<&str> = ASSETS.iter().map(|a| a.path).collect();
    assert_eq!(found, listed, "src/assets/list.rs lists every file under assets/, sorted");
    for want in ["docs/swarm-protocol.md", "docs/lessons.md", "docs/progress-docs.md", ".github/pull_request_template.md", "scripts/hooks/pre-push"] {
        assert!(listed.contains(&want), "{want}");
    }
    assert!(listed.iter().any(|p| p.starts_with(".claude/skills/complexity-analyzer/")));
    // The refactoring skill is no longer shipped (owner, 2026-10-31).
    assert!(!listed.iter().any(|p| p.starts_with(".claude/skills/refactoring/")));
}

/// No asset or template holds a control character but a newline (a tab only in a `.tsv`, a carriage
/// return only before a newline): an escaped string written unescaped once put a carriage return and
/// a tab into the skills README's tools path (#4).
#[test]
fn no_shipped_file_holds_a_stray_control_character() {
    for a in ASSETS.iter().chain(crate::templates::TEMPLATES) {
        let tsv = a.path.ends_with(".tsv");
        for (line, text) in a.text.split('\n').enumerate() {
            let text = text.strip_suffix('\r').unwrap_or(text);
            let bad = text.chars().find(|&c| c.is_control() && !(tsv && c == '\t'));
            assert_eq!(bad, None, "{}:{}: a control character", a.path, line + 1);
        }
    }
}

/// What an asset or a template never names: a consumer, its issues or its old GitLab home. A
/// citation of one consumer's issues lands in every repo as theirs (it slipped back in with #8, #10
/// and #13).
pub(crate) const NAMES_NO_CONSUMER: [&str; 6] = ["nsight", "cmaughan/ns", "swarm inspector", "swarm-inspector", "si#", "gitlab-master"];

/// The assets name no consumer (#6).
#[test]
fn no_asset_names_a_consumer() {
    for a in ASSETS.iter().filter(|a| !a.path.starts_with(".claude/skills/complexity-analyzer/")) {
        let text = a.text.to_lowercase();
        for name in NAMES_NO_CONSUMER {
            assert!(!text.contains(name), "{}: names {name}", a.path);
        }
    }
}

/// Every template renders for both consumers, with no placeholder left; the skills are as they are.
#[test]
fn both_consumers_render_every_asset() {
    for (project, hooks) in [(&APP_SWARM, &App as &dyn Hooks), (&GPU_SWARM, &Gpu)] {
        for a in ASSETS {
            let text = render(project, hooks, a).unwrap_or_else(|e| panic!("{}: {e}", project.name));
            assert!(!text.contains("\r\n"), "{}: LF only", a.path);
            if a.is_template() {
                assert!(placeholders(&text).is_empty(), "{} for {}: {:?}", a.path, project.name, placeholders(&text));
            } else {
                assert_eq!(text, a.text.replace("\r\n", "\n"), "{}: a skill's file is copied as it is", a.path);
            }
        }
    }
    let hook = |p: &Project, h: &dyn Hooks| render(p, h, asset("scripts/hooks/pre-push")).unwrap();
    assert!(hook(&APP_SWARM, &App).contains("[ \"$APP_SWARM_CLAIM_CHECK\" = \"off\" ]"));
    assert!(hook(&GPU_SWARM, &Gpu).contains("[ \"$GPU_SWARM_CLAIM_STRICT\" = \"1\" ]"));
    let labels = render(&GPU_SWARM, &Gpu, asset("scripts/github-labels.sh")).unwrap();
    assert!(labels.contains("REPO=\"${1:-example/gpu-swarm}\"") && labels.contains("GH_HOST:-github.example.com}"), "{labels}");
    let mr = render(&APP_SWARM, &App, asset(".github/pull_request_template.md")).unwrap();
    assert!(mr.contains("passes locally (fmt,") && mr.contains("`cargo run -p swarm-ui-egui --example snapshot`"), "{mr}");
    let protocol = render(&GPU_SWARM, &Gpu, asset("docs/swarm-protocol.md")).unwrap();
    assert!(protocol.contains("gh pr create --repo example/gpu-swarm --draft") && protocol.contains("https://github.example.com/example/gpu-swarm/issues"));
    // The project's own settings, each consumer's value (mornycorp/swarm-core#5).
    assert!(
        protocol.contains("`gpu-stack.json`") && protocol.contains("3 hours, the project's `Limits::gate_lock_max_age_secs`"),
        "the second project's settings"
    );
    let si = render(&APP_SWARM, &App, asset("docs/swarm-protocol.md")).unwrap();
    assert!(
        si.contains("`swarm-stack.json`")
            && si.contains("2 hours, the project's `Limits::gate_lock_max_age_secs`")
            && si.contains("`.claude/worktrees/fullgate`"),
        "the first project's settings"
    );
}

/// GitHub refuses a label whose description is over 100 characters, and the script stops there.
#[test]
fn every_label_description_fits_github() {
    for line in asset("scripts/github-labels.sh").text.lines().filter(|l| l.starts_with("label ")) {
        let description = line.rsplit('"').nth(1).unwrap_or_default();
        assert!(description.chars().count() <= 100, "{} characters: {line}", description.chars().count());
    }
}

#[test]
fn a_duration_reads_in_hours_or_minutes() {
    assert_eq!(
        [duration(2 * 3600), duration(3600), duration(90 * 60), duration(60), duration(0)],
        ["2 hours", "1 hour", "90 minutes", "1 minute", "0 minutes"]
    );
}

#[test]
fn a_placeholder_the_project_doesnt_give_is_refused() {
    let err = render(&APP_SWARM, &crate::Defaults, asset(".github/ISSUE_TEMPLATE/bug.md")).unwrap_err();
    assert!(err.starts_with(".github/ISSUE_TEMPLATE/bug.md: the project gives no value for `{{app}}`"), "{err}");
    // The project's settings need no hook.
    let pre_push = render(&APP_SWARM, &crate::Defaults, asset("scripts/hooks/pre-push")).unwrap();
    assert!(pre_push.contains("APP_SWARM_CLAIM_CHECK"));
}

#[test]
fn placeholders_are_names_in_double_braces() {
    assert_eq!(placeholders("[ \"${{env}}_X\" ] ${1:-{{github.repo}}} {{ not one }} {{a_b.c2}} {{"), ["env", "github.repo", "a_b.c2"]);
    assert!(placeholders("${HOME} {} {{}} {{Upper}}").is_empty());
    let skill = Asset { path: ".claude/skills/x/SKILL.md", text: "{{kept}}" };
    assert!(!skill.is_template() && asset(".claude/skills/README.md").is_template());
    assert_eq!(render(&APP_SWARM, &crate::Defaults, &skill).unwrap(), "{{kept}}");
}

/// What the `add-command` skill names is in the skeleton: a path it points at is one of the
/// skeleton's files, and a name it uses (a function, a type, an argument) is in their text, so a
/// rename in the skeleton fails here instead of leaving the skill wrong.
#[test]
fn the_add_command_skill_names_only_what_the_skeleton_has() {
    let skill = asset(".claude/skills/add-command/SKILL.md").text;
    let files = crate::skeleton::FILES;
    let paths: Vec<&str> = skill
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| (t.starts_with("crates/") || t.starts_with("apps/")) && t.ends_with(".rs") && !t.contains('<'))
        .collect();
    assert!(paths.len() >= 2, "the skill points at the skeleton's registry and its conformance test: {paths:?}");
    for path in paths {
        assert!(files.iter().any(|f| f.path == path), "the skill names `{path}`, which the skeleton has no file for");
    }
    let engine = |name: &str| files.iter().filter(|f| f.path.starts_with("crates/engine/")).any(|f| f.text.contains(name));
    for name in [
        "pub fn registry()",
        "registry.register(",
        "fn check_cancelled",
        "fn with_key",
        "fn bad_params",
        "fn not_found",
        "fn too_large",
        "fn unavailable",
        "fn internal",
        "fn consent_required",
        "fn consenting",
        "fn call_typed",
        "const EFFECTS",
        "const REQUIRES",
        "const MENU",
        "const SHORTCUT",
        "fn enabled(&self, session: &Session) -> Result<(), Why>",
        "Why::new",
        "pub fn states(&self)",
        "deny_unknown_fields",
        "BadParams",
        "Caller::Test",
        "ConsentRequired",
        "pub fn check(self",
    ] {
        assert!(engine(name), "the skill names `{name}`, which the skeleton's engine doesn't have");
    }
    let everywhere = |name: &str| files.iter().any(|f| f.text.contains(name));
    for name in [
        "adversarial_params",
        "pub fn temp_dir",
        "every_command_gives_the_same_answer_on_every_surface",
        "\"commands\"",
        "\"--json\"",
        "\"--yes\"",
        "param: ",
        "fix: ",
        "tools/list",
        "error [",
    ] {
        assert!(everywhere(name), "the skill names `{name}`, which the skeleton doesn't have");
    }
}

/// The skills are copied as they are but for the README and `add-command`, whose commands name the
/// app's CLI and its environment prefix: each consumer's copy carries its own.
#[test]
fn only_the_readme_and_add_command_are_templated_skills() {
    let templated: Vec<&str> = ASSETS.iter().filter(|a| a.path.starts_with(VERBATIM) && a.is_template()).map(|a| a.path).collect();
    assert_eq!(templated, [".claude/skills/README.md", ".claude/skills/add-command/SKILL.md", ".claude/skills/add-command/agents/openai.yaml"]);
    let skill = asset(".claude/skills/add-command/SKILL.md");
    let mut used = placeholders(skill.text);
    used.sort_unstable();
    used.dedup();
    assert_eq!(used, ["cli", "env"], "the skill's placeholders: the app's CLI and its environment prefix");
    for (project, hooks, cli, env, other) in
        [(&APP_SWARM, &App as &dyn Hooks, "swarm-cli", "APP_SWARM", "gpu-cli"), (&GPU_SWARM, &Gpu, "gpu-cli", "GPU_SWARM", "swarm-cli")]
    {
        let text = render(project, hooks, skill).unwrap();
        assert!(text.contains(&format!("`{cli} run <id> '<json params>'`")) && text.contains(&format!("`{env}_CONTROL_TOKEN`")), "{}", project.name);
        assert!(placeholders(&text).is_empty() && !text.contains(other), "{}", project.name);
        let yaml = render(project, hooks, asset(".claude/skills/add-command/agents/openai.yaml")).unwrap();
        assert!(yaml.contains(&format!("add a {cli} command")), "{yaml}");
    }
    // A consumer that gives no `cli` is told which placeholder, as for any template.
    let err = render(&APP_SWARM, &crate::Defaults, skill).unwrap_err();
    assert!(err.starts_with(".claude/skills/add-command/SKILL.md: the project gives no value for `{{cli}}`"), "{err}");
}

/// `sync-assets` writes every copy and the manifest; `--check` passes on it, also with CRLF line
/// endings, and fails on an edit, a deletion or a stale manifest until the next sync.
#[test]
fn sync_writes_what_check_accepts_and_an_edit_fails_the_check() {
    let root = swarm_core_testkit::temp_dir("swarm-core-assets-sync");
    let diff = differences(&APP_SWARM, &App, &root).unwrap();
    assert_eq!(diff.len(), ASSETS.len() + 1, "every asset and the manifest are missing");
    sync(&APP_SWARM, &App, &root).unwrap();
    check(&APP_SWARM, &App, &root).unwrap();
    assert_eq!(std::fs::read_to_string(root.join(MANIFEST)).unwrap(), manifest());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(root.join("scripts/hooks/pre-push")).unwrap().permissions().mode() & 0o111, 0o111);
    }
    // A Windows checkout's CRLF is the same copy.
    let doc = root.join("docs/lessons.md");
    std::fs::write(&doc, std::fs::read_to_string(&doc).unwrap().replace('\n', "\r\n")).unwrap();
    check(&APP_SWARM, &App, &root).unwrap();
    // An edit, and a deletion, fail it, naming each.
    std::fs::write(&doc, "edited here\n").unwrap();
    std::fs::remove_file(root.join(".claude/skills/complexity-analyzer/SKILL.md")).unwrap();
    let err = check(&APP_SWARM, &App, &root).unwrap_err();
    assert!(err.starts_with("2 pinned asset(s) differ"), "{err}");
    assert_eq!(differences(&APP_SWARM, &App, &root).unwrap(), [".claude/skills/complexity-analyzer/SKILL.md: missing", "docs/lessons.md: differs"]);
    // Another project's copy isn't this one's.
    assert!(!differences(&GPU_SWARM, &Gpu, &root).unwrap().is_empty());
    sync(&APP_SWARM, &App, &root).unwrap();
    check(&APP_SWARM, &App, &root).unwrap();
}

/// A file an earlier revision shipped and this one doesn't is removed by the next sync, and the
/// check names it until then; a manifest line that leaves the root is never touched.
#[test]
fn a_file_no_longer_shipped_is_removed() {
    let root = swarm_core_testkit::temp_dir("swarm-core-assets-stale");
    sync(&APP_SWARM, &App, &root).unwrap();
    std::fs::create_dir_all(root.join("docs/old")).unwrap();
    std::fs::write(root.join("docs/old/gone.md"), "x").unwrap();
    let outside = root.parent().unwrap().join(format!("{}-outside.md", root.file_name().unwrap().to_string_lossy()));
    std::fs::write(&outside, "keep").unwrap();
    let old = format!("{}- `docs/old/gone.md`\n- `../{}`\n- `C:/x`\n", manifest(), outside.file_name().unwrap().to_string_lossy());
    std::fs::write(root.join(MANIFEST), old).unwrap();
    let diff = differences(&APP_SWARM, &App, &root).unwrap();
    assert_eq!(diff, [format!("{MANIFEST}: differs"), "docs/old/gone.md: no longer shipped".to_owned()]);
    sync(&APP_SWARM, &App, &root).unwrap();
    assert!(!root.join("docs/old/gone.md").exists() && outside.exists());
    check(&APP_SWARM, &App, &root).unwrap();
    let _ = std::fs::remove_file(outside);
}

/// Every `Rgb::hex(0x……)` in `text`, in order, as `#RRGGBB`.
fn rgb_hexes(text: &str) -> Vec<String> {
    text.split("Rgb::hex(0x").skip(1).map(|rest| format!("#{}", rest.get(..6).unwrap().to_uppercase())).collect()
}

/// The colours of the skeleton theme's `const <name>`: an array, or one colour.
fn palette_const(theme: &str, name: &str) -> Vec<String> {
    let start = theme.find(&format!("const {name}: ")).unwrap_or_else(|| panic!("no const {name}"));
    // To the statement's end: the first line ending in `;`, not the `;` in its type (`[Rgb; 7]`).
    // By lines, so a Windows checkout's CRLF ends a line too.
    let mut statement = String::new();
    for line in theme.get(start..).unwrap().lines() {
        statement.push_str(line);
        if line.trim_end().ends_with(';') {
            break;
        }
    }
    rgb_hexes(&statement)
}

/// Every token of the skeleton's theme: its name, then its dark and light colours.
fn skeleton_tokens(theme: &str) -> Vec<(String, [String; 2])> {
    let literals = theme.split("Token {").skip(1).filter_map(|rest| {
        let body = rest.get(..rest.find('}')?)?;
        let name = body.split("name: \"").nth(1)?.split('"').next()?;
        let [dark, light] = <[String; 2]>::try_from(rgb_hexes(body)).ok()?;
        Some((name.to_owned(), [dark, light]))
    });
    literals.collect()
}

/// The colour rows of a document's tables: those whose last two cells are each one `#RRGGBB`, by
/// their first cell (its backticks dropped).
fn colour_rows(doc: &str) -> std::collections::BTreeMap<String, [String; 2]> {
    let is_hex = |c: &str| c.len() == 7 && c.starts_with('#') && c.get(1..).unwrap().bytes().all(|b| b.is_ascii_hexdigit());
    let mut rows = std::collections::BTreeMap::new();
    for line in doc.lines().filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(|c| c.trim().trim_matches('`')).collect();
        let [first, .., dark, light] = cells.as_slice() else { continue };
        if is_hex(dark) && is_hex(light) {
            let old = rows.insert((*first).to_owned(), [dark.to_uppercase(), light.to_uppercase()]);
            assert_eq!(old, None, "{first} is in the UI substrate twice");
        }
    }
    rows
}

/// **The App UI Substrate has the skeleton's colours** (mornycorp/swarm-core#104): its tables give every token and every
/// entry of the series, the markers and range32 (range16 is its first 16) as the skeleton's theme
/// has them, in each look, and nothing else, so the document and the theme can't drift apart. The
/// page is generated from the doc: `the_pages_are_rendered_from_their_docs`, in this repo's
/// xtask, holds it to the doc, its ranges drawn from the theme.
#[test]
fn the_ui_substrate_has_the_skeletons_colours() {
    let theme = crate::skeleton::FILES.iter().find(|f| f.path == "crates/engine/src/theme.rs").unwrap().text;
    let mut want: std::collections::BTreeMap<String, [String; 2]> = skeleton_tokens(theme).into_iter().collect();
    assert_eq!(want.len(), 20, "the skeleton's tokens");
    let range = |want: &mut std::collections::BTreeMap<_, _>, name: &str, dark: Vec<String>, light: Vec<String>| {
        assert_eq!(dark.len(), light.len(), "{name}");
        for (i, (d, l)) in dark.into_iter().zip(light).enumerate() {
            want.insert(format!("{name} {i}"), [d, l]);
        }
    };
    range(&mut want, "series", palette_const(theme, "DARK2"), palette_const(theme, "SET2"));
    let none = [palette_const(theme, "DARK2_NONE"), palette_const(theme, "SET2_NONE")].map(|c| c.concat());
    want.insert("series none".to_owned(), none);
    let markers = palette_const(theme, "MARKER_PALETTE");
    range(&mut want, "markers", markers.clone(), markers);
    range(&mut want, "range32", palette_const(theme, "GLASBEY_DARK"), palette_const(theme, "GLASBEY_LIGHT"));
    assert_eq!(want.len(), 20 + 8 + 9 + 32);
    assert_eq!(colour_rows(asset("docs/app-ui-substrate.md").text), want, "the doc");
}

/// What the App Substrate's page says it has caught up with: the date and the start of a row of
/// `app-architecture.md`'s revision history.
const CONTRACT_STAMP: &str = "<!-- contract revision: ";

/// How much of a row's text a stamp carries.
const STAMP_CHARS: usize = 60;

/// The stamp the App Substrate's page carries once it has caught up with `doc`'s newest revision:
/// that row's date and the start of its text.
fn contract_stamp(doc: &str) -> String {
    let history = doc.get(doc.find("## Revision history").unwrap()..).unwrap();
    // The newest row is the first: this history is newest first.
    let newest = history.lines().find(|l| l.starts_with("| 2")).unwrap();
    let (date, text) = newest.trim_start_matches("| ").split_once(" | ").unwrap();
    let start: String = text.chars().take(STAMP_CHARS).collect();
    format!("{CONTRACT_STAMP}{date} | {} -->", start.trim_end())
}

/// **The App Substrate's page has caught up with the contract**: it explains
/// `app-architecture.md` in its own words, so it carries the newest revision it has been read
/// against, and fails here when the contract gains one, until someone re-reads the page against
/// what changed and moves the stamp.
#[test]
fn the_app_substrate_page_has_caught_up_with_the_contract() {
    let page = asset("docs/app-substrate.html").text;
    let want = contract_stamp(asset("docs/app-architecture.md").text);
    let have = page.lines().find(|l| l.starts_with(CONTRACT_STAMP)).unwrap_or("(no stamp)");
    assert_eq!(
        have.trim_end(),
        want,
        "app-substrate.html explains app-architecture.md as of the revision it names, and the contract has moved on: re-read the page \
         against the revision history's rows since then, change what they make wrong, then set the page's stamp to `right`"
    );
}

/// **The App Substrate's page says what Swarm Core does**: its layers, top to bottom, are
/// `Layers::STANDARD`'s; its swarm commands are the ones Swarm Core serves ([`crate::COMMANDS`])
/// and the app's own two (`ci`, `layers`); its transport limits are the skeleton's `limits.rs`.
#[test]
fn the_app_substrate_page_says_what_swarm_core_does() {
    let page = asset("docs/app-substrate.html").text;
    let layers_at = page.find("const LAYERS = [").unwrap();
    let layers = page.get(layers_at..).unwrap();
    let layers = layers.get(..layers.find("];").unwrap()).unwrap();
    let mut ids: Vec<&str> = layers.split("id: \"").skip(1).filter_map(|r| r.split('"').next()).collect();
    ids.reverse();
    assert_eq!(ids, crate::checks::Layers::STANDARD.stack, "the layers, bottom up");

    let cmds_at = page.find("id=\"cmds-worker\"").unwrap();
    let cmds = page.get(cmds_at..).unwrap();
    let cmds = cmds.get(..cmds.find("</section>").unwrap()).unwrap();
    let mut named: Vec<&str> = cmds.split("<dt>").skip(1).filter_map(|r| r.split("</dt>").next()).collect();
    named.sort_unstable();
    let mut served: Vec<&str> = crate::COMMANDS.iter().map(|&(name, _)| name).chain(["ci", "layers"]).collect();
    served.sort_unstable();
    assert_eq!(named, served, "the swarm's commands");

    let limits = crate::skeleton::FILES.iter().find(|f| f.path == "crates/automation/src/limits.rs").unwrap().text;
    let pairs = [
        ("pub const MAX_REQUEST: usize = 1024 * 1024;", "<dt>1 MiB</dt>"),
        ("pub const MAX_REPLY: usize = 8 * 1024 * 1024;", "<dt>8 MiB</dt>"),
        ("pub const MAX_CONNECTIONS: usize = 16;", "<dt>16</dt>"),
        ("pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);", "<dt>30 s</dt>"),
    ];
    for (code, shown) in pairs {
        assert!(limits.contains(code) && page.contains(shown), "the skeleton has `{code}` and the page shows {shown}");
    }
    let socket = crate::skeleton::FILES.iter().find(|f| f.path == "crates/automation/src/socket.rs").unwrap().text;
    assert!(socket.contains("It binds `127.0.0.1`") && page.contains("<dt>127.0.0.1</dt>"), "the socket's address");
}

/// `cargo xtask swarm-core sync-assets [--check]` is served, and refuses other arguments.
#[test]
fn the_swarm_core_command_serves_sync_assets() {
    let root = swarm_core_testkit::temp_dir("swarm-core-assets-command");
    let run = |args: &[&str]| crate::run(&APP_SWARM, &App, &root, "swarm-core", args);
    assert!(run(&["sync-assets", "--check"]).is_err());
    run(&["sync-assets"]).unwrap();
    run(&["sync-assets", "--check"]).unwrap();
    assert!(run(&["sync-assets", "--force"]).unwrap_err().starts_with("usage: cargo xtask swarm-core"));
}
