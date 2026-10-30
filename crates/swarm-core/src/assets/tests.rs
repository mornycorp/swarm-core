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
    assert!(listed.iter().any(|p| p.starts_with(".claude/skills/refactoring/")) && listed.iter().any(|p| p.starts_with(".claude/skills/complexity-analyzer/")));
    // A repo's own file beside the skills is never shipped (a project's score targets).
    assert!(!listed.contains(&".claude/skills/refactoring/references/project-targets.md"));
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
    // The project's own settings, each consumer's value (cmaughan/swarm-core#5).
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
    std::fs::remove_file(root.join(".claude/skills/refactoring/SKILL.md")).unwrap();
    let err = check(&APP_SWARM, &App, &root).unwrap_err();
    assert!(err.starts_with("2 pinned asset(s) differ"), "{err}");
    assert_eq!(differences(&APP_SWARM, &App, &root).unwrap(), [".claude/skills/refactoring/SKILL.md: missing", "docs/lessons.md: differs"]);
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
