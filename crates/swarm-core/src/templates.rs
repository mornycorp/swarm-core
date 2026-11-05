//! The starting points a new consumer copies once and then owns (swarm-core#7): the files
//! the swarm protocol and the coordinator's role assume a consumer has (`AGENTS.md`, its
//! `docs/architecture.md`, `docs/swarm.md`, `docs/development.md`, the progress docs, the owner's directions, the
//! scheduled merge check's prompt, nextest's `ci` profile). No GitHub Actions workflow: CI is
//! local, the gates of the coordinator and every worker. They live in
//! `crates/swarm-core/templates/`, at the path each takes in a consumer, built into the crate
//! ([`TEMPLATES`]).
//!
//! - **`cargo xtask swarm-core init`** ([`init`]) writes each one the consumer doesn't have yet,
//!   with the same `{{name}}` placeholders as the assets ([`crate::assets::fill`]) and
//!   `{{today}}`, the date. A file that exists is never touched, so it is safe to run again.
//! - **Not pinned.** Unlike [`crate::assets`], nothing lists or checks them after: the consumer
//!   edits them freely, and a later revision's templates reach it only if it copies them by hand.
//!
//! Why a command and not a documented copy: the templates take the consumer's values (its name,
//! its GitHub repository, its environment prefix), and a hand copy would fill in a dozen placeholders
//! by hand. The command reuses the assets' rendering, and refusing to overwrite keeps it small.

use std::path::Path;

use crate::assets::{Asset, fill};
use crate::{Hooks, Project};

/// The templates this revision ships, by path in a consumer.
pub const TEMPLATES: &[Asset] = &[
    Asset { path: ".config/nextest.toml", text: include_str!("../templates/.config/nextest.toml") },
    Asset { path: "AGENTS.md", text: include_str!("../templates/AGENTS.md") },
    Asset { path: "ROADMAP.md", text: include_str!("../templates/ROADMAP.md") },
    Asset { path: "docs/architecture.md", text: include_str!("../templates/docs/architecture.md") },
    Asset { path: "docs/development.md", text: include_str!("../templates/docs/development.md") },
    Asset { path: "docs/gaps.md", text: include_str!("../templates/docs/gaps.md") },
    Asset { path: "docs/merge-check.md", text: include_str!("../templates/docs/merge-check.md") },
    Asset { path: "docs/owner-directions.md", text: include_str!("../templates/docs/owner-directions.md") },
    Asset { path: "docs/roadmap.md", text: include_str!("../templates/docs/roadmap.md") },
    Asset { path: "docs/scorecard.md", text: include_str!("../templates/docs/scorecard.md") },
    Asset { path: "docs/swarm.md", text: include_str!("../templates/docs/swarm.md") },
];

/// `template` as `project` starts it on `today` (`YYYY-MM-DD`): LF line endings, its placeholders
/// filled in.
pub fn render_template(project: &Project, hooks: &dyn Hooks, template: &Asset, today: &str) -> Result<String, String> {
    fill(project, hooks, template.path, template.text.replace("\r\n", "\n").replace("{{today}}", today))
}

/// `cargo xtask swarm-core init`: write at `root` each template that isn't there yet, dated
/// `today`; the paths written, in order. Every template renders before anything is written, so a
/// placeholder the project doesn't give writes nothing.
pub fn init(project: &Project, hooks: &dyn Hooks, root: &Path, today: &str) -> Result<Vec<&'static str>, String> {
    let rendered: Vec<(&'static str, String)> =
        TEMPLATES.iter().map(|t| render_template(project, hooks, t, today).map(|text| (t.path, text))).collect::<Result<_, _>>()?;
    let mut written = Vec::new();
    for (path, text) in rendered {
        let at = root.join(path);
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
    println!(
        "swarm-core: {} of {} templates written from swarm-core {}; they are this repo's now (docs/new-swarm.md in Swarm Core)",
        written.len(),
        TEMPLATES.len(),
        crate::VERSION
    );
    Ok(written)
}

#[cfg(test)]
mod tests;
