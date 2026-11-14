//! `cargo xtask docs render [--check]` (mornycorp/swarm-core#108): the docs' pages made from
//! their markdown, so a page never drifts from the doc it shows. Each page in [`PAGES`] is its doc
//! rendered ([`page::render`]) inside a fixed shell, with the skeleton theme's ranges drawn where
//! the doc says ([`theme`]). The page is never edited by hand: edit the doc, then render.
//!
//! `--check` (and the test `the_pages_are_rendered_from_their_docs`) fails when a committed page
//! differs from a fresh render, naming the command and the first line that differs.
//!
//! This repo's own tooling: the markdown crate is the xtask's, never Swarm Core library's, so
//! neither consumer links it. A consumer gets the rendered page as a pinned asset, through
//! `sync-assets`, which copies it as it copies any other.

mod page;
mod theme;

use std::path::Path;

/// A doc that has a page.
pub struct Page {
    /// The doc, from the workspace root.
    pub doc: &'static str,
    /// Its page, from the workspace root.
    pub page: &'static str,
    /// The page's `<title>`.
    pub title: &'static str,
    /// The page's opening comment: where it comes from (no `--`).
    pub comment: &'static str,
    /// The line above its title (HTML).
    pub eyebrow: &'static str,
    /// The line under the doc's opening (HTML spans).
    pub meta: &'static str,
    /// Its footer (HTML).
    pub footer: &'static str,
}

/// The skeleton's theme, whose ranges a page draws.
const THEME: &str = "crates/swarm-core/templates/app/crates/engine/src/theme.rs";

/// The command that renders the pages: what a failed check says to run.
pub const RENDER: &str = "cargo xtask docs render";

/// Every doc that has a page. One that gets a page later is added here.
pub const PAGES: &[Page] = &[Page {
    doc: "crates/swarm-core/assets/docs/app-ui-substrate.md",
    page: "crates/swarm-core/assets/docs/app-ui-substrate.html",
    title: "App UI Substrate",
    comment: "Generated: never edit it by hand. A pinned copy from swarm-core\n     \
              (crates/swarm-core/assets/docs/app-ui-substrate.html), rendered from app-ui-substrate.md beside it,\n     \
              with the ranges drawn from the app skeleton's theme.rs, by `cargo xtask docs render`. Edit the doc in\n     \
              swarm-core and render; a consumer takes both with `cargo xtask swarm-core bump` and\n     \
              `cargo xtask swarm-core sync-assets`. Open it in a browser.",
    eyebrow: "swarm-core · the architectural substrate",
    meta: "<span>the colours: <code>templates/app/crates/engine/src/theme.rs</code></span><span>egui · eframe</span>\
           <span>generated from <code>app-ui-substrate.md</code></span>",
    footer: "  <span>Generated from <code>app-ui-substrate.md</code> by swarm-core's <code>cargo xtask docs render</code>, \
             the ranges drawn from the skeleton's <code>templates/app/crates/engine/src/theme.rs</code>: edit the doc, never this page.</span>\n  \
             <span>Its colours are the doc's tables, which <code>the_ui_substrate_has_the_skeletons_colours</code> holds to the skeleton's theme. \
             The rule: <code>app-architecture.md</code> › Colours from the theme, words from the catalog.</span>",
}];

/// `cargo xtask docs <args>`.
pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    match args {
        ["render"] => write(root),
        ["render", "--check"] => check(root),
        _ => Err(format!("usage: {USAGE}")),
    }
}

/// The command's usage, for `cargo xtask`'s list.
pub const USAGE: &str = "cargo xtask docs render [--check]";

/// Read `path` under `root`, with LF line endings.
fn read(root: &Path, path: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(path)).map(|t| t.replace("\r\n", "\n")).map_err(|e| format!("{path}: {e}"))
}

/// `page` as its doc renders it now.
fn render(root: &Path, page: &Page) -> Result<String, String> {
    let ranges = theme::ranges(&read(root, THEME)?)?;
    page::render(page, &read(root, page.doc)?, &ranges)
}

/// Render every page and write it where it is kept.
fn write(root: &Path) -> Result<(), String> {
    for page in PAGES {
        let text = render(root, page)?;
        let unchanged = read(root, page.page).is_ok_and(|have| have == text);
        if !unchanged {
            std::fs::write(root.join(page.page), &text).map_err(|e| format!("{}: {e}", page.page))?;
        }
        println!("docs: {} {} from {}", page.page, if unchanged { "is current" } else { "rendered" }, page.doc);
    }
    Ok(())
}

/// An error when a committed page differs from a fresh render of its doc.
pub fn check(root: &Path) -> Result<(), String> {
    let mut stale = Vec::new();
    for page in PAGES {
        let want = render(root, page)?;
        let have = read(root, page.page).unwrap_or_default();
        if let Some(why) = first_difference(&have, &want) {
            stale.push(format!("{} differs from its doc ({}): {why}", page.page, page.doc));
        }
    }
    if stale.is_empty() {
        println!("docs: {} page(s) match their docs", PAGES.len());
        return Ok(());
    }
    Err(format!("{}\nThe pages are generated, never edited by hand: edit the doc, then run `{RENDER}`.", stale.join("\n")))
}

/// Where `have` (the committed page) first differs from `want` (the render): its line, and the
/// line in each. `None` when they are the same.
fn first_difference(have: &str, want: &str) -> Option<String> {
    if have == want {
        return None;
    }
    let clip = |l: Option<&str>| l.map_or_else(|| "(none: the page ends)".to_owned(), |l| l.chars().take(160).collect());
    let (mut h, mut w) = (have.lines(), want.lines());
    for n in 1.. {
        let (a, b) = (h.next(), w.next());
        if a != b || (a.is_none() && b.is_none()) {
            return Some(format!("line {n}:\n  committed: {}\n  rendered:  {}", clip(a), clip(b)));
        }
    }
    None
}

#[cfg(test)]
mod tests;
