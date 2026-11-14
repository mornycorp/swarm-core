use super::*;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

/// **The pages are rendered from their docs**: each committed page is what `cargo xtask docs
/// render` makes of its doc now, so an edit to the doc that isn't rendered fails here, naming the
/// command. This holds what `the_ui_substrate_page_and_doc_agree` and the page's half of the colour
/// test held: the page's statuses and colours are the doc's, and its ranges the skeleton theme's.
#[test]
fn the_pages_are_rendered_from_their_docs() {
    if let Err(why) = check(root()) {
        panic!("{why}");
    }
}

/// A difference is named by its line, with both versions of it, and the check names the command.
#[test]
fn a_difference_names_its_line_and_the_command() {
    assert_eq!(first_difference("a\nb\n", "a\nb\n"), None);
    let why = first_difference("a\nb\nc\n", "a\nB\nc\n").unwrap();
    assert!(why.starts_with("line 2:") && why.contains("committed: b") && why.contains("rendered:  B"), "{why}");
    assert!(first_difference("a\n", "a\nb\n").unwrap().contains("(none: the page ends)"));
    let dir = swarm_core_testkit::temp_dir("xtask-docs-check");
    for path in [THEME, PAGES[0].doc] {
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::copy(root().join(path), dir.join(path)).unwrap();
    }
    let err = check(&dir).unwrap_err();
    assert!(err.contains(RENDER) && err.contains("line 1:"), "{err}");
    write(&dir).unwrap();
    check(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The passes over a rendered doc: the forge's heading ids, the section number set apart, a status as
/// its heading's badge, a bold opening sentence as a rule box (but not in a list), a colour cell's
/// swatch, figures, a group row, the legend, and the ranges drawn from the theme's data.
#[test]
fn the_page_shows_what_plain_markdown_does_not() {
    let doc = "<!-- a pinned copy -->\n# Title\n\nHow {{product}} looks.\n\n- **In the skeleton:** done.\n- **To adopt:** not yet.\n\n\
               ## 2. Looks, and how\n\n**In the skeleton.** The theme.\n\n### `range32` (and more)\n\n**To adopt.** Later.\n\n\
               **Wait for it.** egui binds.\n\n- **Not a rule.** In a list.\n\n\
               | Token | Ratio | Dark | Light |\n|---|---|---|---|\n| **Surfaces** | | | |\n| `background` | 2.14–2.97:1 | `#353535` | `#efefef` |\n\n\
               <!-- page: ranges -->\n\n| Entry | Dark | Light |\n|---|---|---|\n| series 0 | `#1B9E77` | `#66C2A5` |\n| series 1 | `#D95F02` | `#FC8D62` |\n\n\
               ## Revision history\n\nSee [Looks](#2-looks-and-how).\n";
    let ranges = [theme::Range { name: "series".into(), dark: vec!["#1B9E77".into()], light: vec!["#66C2A5".into()], none: None }];
    let html = page::render(&PAGES[0], doc, &ranges).unwrap();
    for want in [
        "<h1 id=\"title\">Title</h1>",
        "How {{product}} looks.",
        "<li><span class=\"status in\">In the skeleton</span> done.</li>",
        "<li><span class=\"status todo\">To adopt</span> not yet.</li>",
        "<section>\n<h2 id=\"2-looks-and-how\"><span class=\"n\">2</span>Looks, and how <span class=\"status in\">In the skeleton</span></h2>\n<p>The theme.</p>",
        "<h3 id=\"range32-and-more\"><code>range32</code> (and more) <span class=\"status todo\">To adopt</span></h3>\n<p>Later.</p>",
        "<div class=\"rule\"><strong>Wait for it</strong><span> egui binds.</span></div>",
        "<li><strong>Not a rule.</strong> In a list.</li>",
        "<div class=\"scroll\"><table>",
        "<tr><td class=\"group\" colspan=\"4\">Surfaces</td></tr>",
        "<td class=\"num\">2.14–2.97:1</td>",
        "<td class=\"hex\"><span class=\"sw\" style=\"background:#353535\"></span>#353535</td>",
        "<div id=\"ranges-out\" class=\"stack\"></div>",
        "<details class=\"entries\"><summary>Its 2 entries as <code>#RRGGBB</code></summary><div class=\"scroll\"><table><thead><tr><th>Entry</th>",
        "{ name: \"series\", dark: [\"#1B9E77\"], light: [\"#66C2A5\"] },",
        "<h2 id=\"revision-history\">Revision history</h2>",
        "<a href=\"#2-looks-and-how\">",
    ] {
        assert!(html.contains(want), "the page has {want}:\n{html}");
    }
    assert!(!html.contains("a pinned copy") && !html.contains('\r'));
    assert_eq!(page::slug("10. The dock"), "10-the-dock");
    assert_eq!(page::slug("Colours from the theme, words from the catalog"), "colours-from-the-theme-words-from-the-catalog");
}

/// The skeleton theme's ranges, read from its source: the four, in its order, with range16 the
/// first 16 of range32 and only the series with a colour for none.
#[test]
fn the_ranges_are_the_skeleton_themes() {
    let ranges = theme::ranges(&read(root(), THEME).unwrap()).unwrap();
    let names: Vec<&str> = ranges.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["series", "markers", "range16", "range32"]);
    let sizes: Vec<usize> = ranges.iter().map(|r| r.dark.len()).collect();
    assert_eq!(sizes, [7, 9, 16, 32]);
    assert_eq!(ranges[2].dark, ranges[3].dark[..16]);
    assert_eq!(ranges[0].none, Some(["#666666".to_owned(), "#B3B3B3".to_owned()]));
    assert!(ranges[1..].iter().all(|r| r.none.is_none()));
    assert!(theme::ranges("pub const RANGES: [Range; 1] = [NOPE];\n").unwrap_err().contains("NOPE"));
}
