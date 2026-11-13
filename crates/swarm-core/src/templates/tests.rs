use super::*;
use crate::assets::placeholders;
use crate::assets::tests::{App, Gpu};
use crate::project::fixtures::{APP_SWARM, GPU_SWARM};

const TODAY: &str = "2026-10-27";

#[test]
fn every_file_under_templates_is_listed() {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                // `templates/app/` is the app skeleton (`skeleton::FILES`), not a starting point.
                if dir != root || p.file_name().is_none_or(|n| n != "app") {
                    walk(&p, root, out);
                }
            } else {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    let listed: Vec<&str> = TEMPLATES.iter().map(|t| t.path).collect();
    assert_eq!(found, listed, "templates::TEMPLATES lists every file under templates/, sorted");
    for t in TEMPLATES {
        assert!(!crate::assets::ASSETS.iter().any(|a| a.path == t.path), "{} is a template or a pinned asset, not both", t.path);
    }
}

/// Each template renders for both consumers with no placeholder left, dated, and says it is the
/// consumer's own once copied.
#[test]
fn both_consumers_render_every_template() {
    for (project, hooks) in [(&APP_SWARM, &App as &dyn Hooks), (&GPU_SWARM, &Gpu)] {
        for t in TEMPLATES {
            let text = render_template(project, hooks, t, TODAY).unwrap_or_else(|e| panic!("{}: {e}", project.name));
            assert!(placeholders(&text).is_empty(), "{} for {}: {:?}", t.path, project.name, placeholders(&text));
            assert!(!text.contains("\r\n"), "{}: LF only", t.path);
            assert!(text.contains("swarm-core") && text.contains("swarm-core init"), "{}: says where it came from", t.path);
            for name in crate::assets::tests::NAMES_NO_CONSUMER {
                assert!(!text.to_lowercase().contains(name), "{}: names {name}", t.path);
            }
        }
    }
    let agents = render_template(&GPU_SWARM, &Gpu, TEMPLATES.iter().find(|t| t.path == "AGENTS.md").unwrap(), TODAY).unwrap();
    assert!(agents.contains("GPU Swarm") && agents.contains("--repo example/gpu-swarm") && agents.contains("GPU_SWARM_AGENT"), "{agents}");
    let gaps = render_template(&APP_SWARM, &App, TEMPLATES.iter().find(|t| t.path == "docs/gaps.md").unwrap(), TODAY).unwrap();
    assert!(gaps.contains(&format!("**Last reviewed:** {TODAY}")), "{gaps}");
}

/// The docs the shared protocol links to are among the templates, with the sections it links.
#[test]
fn the_templates_hold_what_the_protocol_links_to() {
    let text = |p: &str| TEMPLATES.iter().find(|t| t.path == p).map(|t| t.text.replace("\r\n", "\n")).unwrap_or_else(|| panic!("{p}"));
    let swarm = text("docs/swarm.md");
    for heading in ["## Capabilities", "## The gate's kinds here", "## Each batch"] {
        assert!(swarm.contains(&format!("\n{heading}\n")), "docs/swarm.md: {heading}");
    }
    assert!(text("docs/development.md").contains("\n## How many builds at once\n"));
    assert!(text("docs/roadmap.md").contains("\n## Current focus\n"));
    let agents = text("AGENTS.md");
    assert!(agents.contains("\n## 4. Picking work\n") && agents.contains("\n## 5. Before you finish a task\n"), "AGENTS.md §4 and §5");
    assert!(text(".config/nextest.toml").contains("[profile.ci.junit]") && text(".config/nextest.toml").contains("path = \"junit.xml\""));
}

/// `init` writes what is missing and nothing else: a second run writes nothing, and a file the
/// The app starting points follow the pinned contract: AGENTS.md's golden rules are its ten
/// headings, each linked to an anchor that exists, and the architecture doc has the recipe.
#[test]
fn the_app_templates_follow_the_contract() {
    let text = |p: &str| TEMPLATES.iter().find(|t| t.path == p).map(|t| t.text.replace("\r\n", "\n")).unwrap_or_else(|| panic!("{p}"));
    let contract = crate::assets::ASSETS.iter().find(|a| a.path == "docs/app-architecture.md").expect("the contract is pinned").text.replace("\r\n", "\n");
    let anchor = |heading: &str| heading.to_lowercase().replace(' ', "-");
    let headings: Vec<&str> = contract.lines().filter_map(|l| l.strip_prefix("## ")).filter(|h| *h != "Revision history").collect();
    assert_eq!(headings.len(), 10, "the contract's rules: {headings:?}");
    let agents = text("AGENTS.md");
    for h in &headings {
        assert!(agents.contains(&format!("**[{h}](docs/app-architecture.md#{})", anchor(h))), "AGENTS.md §3 links the rule {h}");
    }
    let contract_anchors: Vec<String> =
        contract.lines().filter(|l| l.starts_with('#') && !l.starts_with("# ")).map(|l| anchor(l.trim_start_matches('#').trim())).collect();
    for p in ["AGENTS.md", "docs/architecture.md", "clippy.toml"] {
        for link in text(p).split("app-architecture.md#").skip(1) {
            let id: String = link.chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect();
            assert!(contract_anchors.contains(&id), "{p}: no heading `{id}` in the contract");
        }
    }
    for layer in ["`model`", "`domain`", "`engine`", "`automation`", "`ui`", "`apps`", "`xtask/`", "`crates/testkit/`"] {
        assert!(agents.contains(layer), "AGENTS.md §2: the layer {layer}");
    }
    let arch = text("docs/architecture.md");
    // Every layer of the check's stack, numbered in its order, in the contract's table and both
    // maps; the optional one says so .
    let stack = crate::checks::Layers::STANDARD.stack;
    for (doc, text) in [("the contract", contract.as_str()), ("AGENTS.md §2", agents.as_str()), ("docs/architecture.md", arch.as_str())] {
        let at: Vec<Option<usize>> =
            stack.iter().enumerate().map(|(n, l)| text.find(&format!("| {n} | `{l}`")).or_else(|| text.find(&format!("| {n} `{l}`")))).collect();
        assert!(at.iter().all(Option::is_some) && at.is_sorted(), "{doc}: the layers {stack:?}, numbered in order: {at:?}");
        assert!(text.contains("`backends` (optional)"), "{doc}: `backends` is optional");
    }
    for heading in ["## Layers", "## How a command flows", "## Where state lives", "## Adding a command", "## Revision history"] {
        assert!(arch.contains(&format!("\n{heading}\n")), "docs/architecture.md: {heading}");
    }
    assert!(arch.contains("run <id> '<json params>'") && arch.contains("`add-command`"), "the recipe ends at the CLI and names the skill");
    let clippy = text("clippy.toml");
    assert!(clippy.contains("std::sync::Mutex") && clippy.contains("std::sync::MutexGuard") && clippy.contains("allow-unwrap-in-tests = true"));
}

/// consumer has, edited or not, is kept as it is.
#[test]
fn init_writes_only_what_is_missing() {
    let root = swarm_core_testkit::temp_dir("swarm-core-templates-init");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/gaps.md"), "ours\n").unwrap();
    let written = init(&APP_SWARM, &App, &root, TODAY).unwrap();
    assert_eq!(written.len(), TEMPLATES.len() - 1);
    assert!(!written.contains(&"docs/gaps.md"));
    assert_eq!(std::fs::read_to_string(root.join("docs/gaps.md")).unwrap(), "ours\n");
    assert!(std::fs::read_to_string(root.join("docs/swarm.md")).unwrap().contains("App Swarm"));
    std::fs::write(root.join("AGENTS.md"), "edited\n").unwrap();
    assert!(init(&APP_SWARM, &App, &root, TODAY).unwrap().is_empty(), "a second run writes nothing");
    assert_eq!(std::fs::read_to_string(root.join("AGENTS.md")).unwrap(), "edited\n");
    // Nothing is pinned: sync-assets neither lists nor checks them.
    assert!(!crate::assets::manifest().contains("docs/swarm.md"));
}

/// A placeholder the project doesn't give is refused before anything is written.
#[test]
fn a_placeholder_the_project_doesnt_give_writes_nothing() {
    let root = swarm_core_testkit::temp_dir("swarm-core-templates-refused");
    let err = init(&APP_SWARM, &crate::Defaults, &root, TODAY).unwrap_err();
    assert!(err.contains("the project gives no value for `{{"), "{err}");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn the_swarm_core_command_runs_init() {
    let root = swarm_core_testkit::temp_dir("swarm-core-templates-command");
    crate::pin::run(&APP_SWARM, &App, &root, &["init"]).unwrap();
    assert!(root.join("docs/merge-check.md").is_file() && root.join(".config/nextest.toml").is_file());
    assert!(crate::pin::USAGE.contains("init"));
}
