//! A doc's page: its markdown rendered by `pulldown-cmark` inside a fixed shell (the CSS, both
//! looks, the header and the footer), then small passes over the result for what the page shows
//! beyond plain markdown:
//!
//! - **Headings** get the ids the forge gives them, so the doc's own `#7-type` links work on the page;
//!   a section's number (`## 7. Type`) is set apart, and each `##` and its content is a `<section>`.
//! - **Statuses:** a paragraph that opens with `**In the skeleton.**` or `**To adopt.**` gives its
//!   heading the status's badge; the legend's `**In the skeleton:**` is the badge itself.
//! - **Rules:** a top-level paragraph that opens with a bold sentence is a rule box.
//! - **Tables** scroll in place; a cell that is one `` `#RRGGBB` `` gets its swatch, a number is
//!   set in figures, and a row of one bold cell is a group's heading. A range's table of entries
//!   (`| Entry | Dark | Light |`) is folded under a summary, as the page draws its colours.
//! - **The ranges drawn:** `<!-- page: ranges -->` in the doc is where the page draws the theme's
//!   ranges ([`super::theme`]), each entry's fill, border and text background in both looks, by a
//!   script whose colour data is the theme's.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, TextMergeStream};

use super::Page;
use super::theme::Range;

/// The fixed CSS every page wears.
const CSS: &str = include_str!("page.css");

/// The script that draws the ranges, after the data it is given (`const RANGES = [ … ];`).
const DRAW: &str = include_str!("ranges.js");

/// Where, in a doc, the page draws the theme's ranges.
pub const RANGES_MARK: &str = "<!-- page: ranges -->";

/// The statuses a section can have: the doc's word for it, and its badge's class.
const STATUSES: [(&str, &str); 2] = [("In the skeleton", "in"), ("To adopt", "todo")];

/// The page for `doc` (its markdown), with `ranges` drawn where the doc says.
pub fn render(page: &Page, doc: &str, ranges: &[Range]) -> Result<String, String> {
    let doc = doc.replace("\r\n", "\n");
    let html = markdown(without_leading_comment(&doc));
    let (h1, intro, body) = split(&html).ok_or_else(|| format!("{}: no `# title` before its first `##`", page.doc))?;
    let body = sections(&numbered(&statuses(&tables(body))));
    let drawn = body.contains(RANGES_MARK);
    let body = body.replace(RANGES_MARK, &drawing());
    let mut out = String::with_capacity(body.len() + CSS.len() + DRAW.len() + 4096);
    out.push_str(&head(page));
    out.push_str("<body>\n<main>\n<header>\n");
    out.push_str(&format!("  <span class=\"eyebrow\">{}</span>\n", page.eyebrow));
    out.push_str(h1);
    out.push_str(&legend(&tables(intro)));
    out.push_str(&format!("<div class=\"meta\">{}</div>\n</header>\n\n", page.meta));
    out.push_str(&body);
    out.push_str(&format!("<footer>\n{}\n</footer>\n</main>\n", page.footer));
    if drawn {
        out.push_str(&script(ranges)?);
    }
    out.push_str("</body>\n</html>\n");
    Ok(out.replace("\r\n", "\n"))
}

/// The doc without the comment it opens with (a pinned copy's note, which the page says its own way).
fn without_leading_comment(doc: &str) -> &str {
    let trimmed = doc.trim_start();
    match trimmed.strip_prefix("<!--").and_then(|r| r.split_once("-->")) {
        Some((_, rest)) => rest.trim_start(),
        None => trimmed,
    }
}

/// The page's head: the comment saying where it comes from, the title and the CSS.
fn head(page: &Page) -> String {
    format!(
        "<!doctype html>\n<!-- {comment} -->\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">\n\
         <title>{title}</title>\n<meta name=\"claude:viewport-fit-sides\" content=\"cover\">\n\
         <link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n\
         <link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\n\
         <link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Roboto:wght@300;400;500;700&family=Roboto+Mono:wght@400;500&display=swap\">\n\
         <style>\n{CSS}</style>\n</head>\n",
        comment = page.comment,
        title = page.title,
    )
}

/// `doc` as HTML: the forge's heading ids, and a top-level paragraph that opens with a bold sentence
/// as a rule box.
fn markdown(doc: &str) -> String {
    let mut events: Vec<Event<'_>> = TextMergeStream::new(Parser::new_ext(doc, Options::ENABLE_TABLES)).collect();
    heading_ids(&mut events);
    let mut html = String::with_capacity(doc.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut html, rules(events).into_iter());
    html
}

/// Give each heading the id the forge gives it: [`slug`], made unique with `-1`, `-2`, ….
fn heading_ids(events: &mut [Event<'_>]) {
    let mut seen = std::collections::HashMap::new();
    for i in 0..events.len() {
        if !matches!(events.get(i), Some(Event::Start(Tag::Heading { .. }))) {
            continue;
        }
        let text: String = events
            .get(i + 1..)
            .unwrap_or_default()
            .iter()
            .take_while(|e| !matches!(e, Event::End(TagEnd::Heading(_))))
            .filter_map(|e| match e {
                Event::Text(t) | Event::Code(t) => Some(t.as_ref()),
                _ => None,
            })
            .collect();
        let base = slug(&text);
        let n: &mut usize = seen.entry(base.clone()).or_default();
        let unique = if *n == 0 { base } else { format!("{base}-{n}") };
        *n += 1;
        if let Some(Event::Start(Tag::Heading { id, .. })) = events.get_mut(i) {
            *id = Some(unique.into());
        }
    }
}

/// A heading's id as the forge makes it: lowercase, everything but letters, digits, `_`, `-` and
/// spaces dropped, spaces as `-`, runs of `-` as one.
pub fn slug(text: &str) -> String {
    let kept: String =
        text.to_lowercase().chars().filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' ')).map(|c| if c == ' ' { '-' } else { c }).collect();
    let mut out = String::with_capacity(kept.len());
    for c in kept.chars() {
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    out
}

/// The title of a rule box at `events[i..]`: a paragraph that opens with a bold sentence, `**A
/// rule.**`, that is no status. The title is without its full stop.
fn rule_title(events: &[Event<'_>], i: usize) -> Option<String> {
    let [Event::Start(Tag::Paragraph), Event::Start(Tag::Strong), Event::Text(t), Event::End(TagEnd::Strong)] = events.get(i..i + 4)? else {
        return None;
    };
    let title = t.strip_suffix('.')?;
    (!STATUSES.iter().any(|(s, _)| *s == title)).then(|| title.to_owned())
}

/// `events` with each top-level paragraph that opens with a bold sentence made a rule box: the
/// sentence its title, the rest its text.
fn rules(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut out = Vec::with_capacity(events.len());
    let mut depth = 0usize;
    let mut in_rule = false;
    let mut i = 0;
    while let Some(e) = events.get(i) {
        match e {
            Event::Start(Tag::List(_) | Tag::Item | Tag::BlockQuote(_) | Tag::Table(_)) => depth += 1,
            Event::End(TagEnd::List(_) | TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::Table) => depth = depth.saturating_sub(1),
            _ => {}
        }
        let title = if depth == 0 { rule_title(&events, i) } else { None };
        if let Some(title) = title {
            out.extend([Event::Html("<div class=\"rule\"><strong>".into()), Event::Text(title.into()), Event::Html("</strong><span>".into())]);
            in_rule = true;
            i += 4;
            continue;
        }
        if in_rule && matches!(e, Event::End(TagEnd::Paragraph)) {
            out.push(Event::Html("</span></div>\n".into()));
            in_rule = false;
        } else {
            out.push(e.clone());
        }
        i += 1;
    }
    out
}

/// The HTML's `<h1>`, what comes between it and the first `<h2`, and the rest.
fn split(html: &str) -> Option<(&str, &str, &str)> {
    let h1_at = html.find("<h1")?;
    let h1_end = h1_at + html.get(h1_at..)?.find("</h1>")? + "</h1>\n".len();
    let h2_at = h1_end + html.get(h1_end..)?.find("<h2").unwrap_or(html.len() - h1_end);
    Some((html.get(h1_at..h1_end)?, html.get(h1_end..h2_at)?, html.get(h2_at..)?))
}

/// Each span of `html` from `open` to the next `close` (both kept) replaced by what `f` makes of
/// what is between them, or left as it is when `f` makes nothing.
fn each(html: &str, open: &str, close: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(html.len() + html.len() / 8);
    let mut rest = html;
    while let Some(at) = rest.find(open) {
        let after = rest.get(at + open.len()..).unwrap_or_default();
        let Some(end) = after.find(close) else { break };
        let inner = after.get(..end).unwrap_or_default();
        out.push_str(rest.get(..at).unwrap_or_default());
        match f(inner) {
            Some(made) => out.push_str(&made),
            None => {
                out.push_str(open);
                out.push_str(inner);
                out.push_str(close);
            }
        }
        rest = after.get(end + close.len()..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// Tables that scroll in place, a swatch beside each colour cell, numbers in figures, and a row of
/// one bold cell as a group's heading.
fn tables(html: &str) -> String {
    let html = html.replace("<table>", "<div class=\"scroll\"><table>").replace("</table>", "</table></div>");
    let html = each(&html, "<div class=\"scroll\"><table>", "</table></div>", folded);
    let html = each(&html, "<tr>", "</tr>", group_row);
    each(&html, "<td>", "</td>", |cell| {
        if let Some(hex) = cell.strip_prefix("<code>").and_then(|c| c.strip_suffix("</code>")).filter(|h| is_hex(h)) {
            return Some(format!("<td class=\"hex\"><span class=\"sw\" style=\"background:{hex}\"></span>{hex}</td>"));
        }
        let figures = cell.chars().any(|c| c.is_ascii_digit()) && cell.chars().all(|c| c.is_ascii_digit() || ".,:×–- %".contains(c));
        figures.then(|| format!("<td class=\"num\">{cell}</td>"))
    })
}

/// A range's table of entries (`| Entry | Dark | Light |`), folded under a summary: the page draws
/// its colours, so the hex values are there to open rather than a long table to scroll past.
fn folded(table: &str) -> Option<String> {
    let body = table.strip_prefix("<thead><tr><th>Entry</th><th>Dark</th><th>Light</th></tr></thead>")?;
    let rows = body.matches("<tr>").count();
    Some(format!(
        "<details class=\"entries\"><summary>Its {rows} entries as <code>#RRGGBB</code></summary><div class=\"scroll\"><table>{table}</table></div></details>"
    ))
}

/// A row whose first cell is one bold run and whose others are empty, as one cell across the row.
fn group_row(row: &str) -> Option<String> {
    let rest = row.strip_prefix("<td><strong>")?;
    let (name, rest) = rest.split_once("</strong></td>")?;
    let empty = rest.matches("<td></td>").count();
    (rest.replace("<td></td>", "").is_empty() && !name.contains('<')).then(|| format!("<tr><td class=\"group\" colspan=\"{}\">{name}</td></tr>", empty + 1))
}

/// Whether `s` is `#RRGGBB`.
fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s.bytes().skip(1).all(|b| b.is_ascii_hexdigit())
}

/// A heading followed by a paragraph that opens with a status gives the heading its badge, and the
/// paragraph loses the word.
fn statuses(html: &str) -> String {
    let mut html = html.to_owned();
    for level in ["h2", "h3"] {
        for (word, class) in STATUSES {
            let from = format!("</{level}>\n<p><strong>{word}.</strong> ");
            let to = format!(" <span class=\"status {class}\">{word}</span></{level}>\n<p>");
            html = html.replace(&from, &to);
        }
    }
    html
}

/// The legend: each status named at the start of a list item is its badge.
fn legend(html: &str) -> String {
    let mut html = html.to_owned();
    for (word, class) in STATUSES {
        html = html.replace(&format!("<li><strong>{word}:</strong>"), &format!("<li><span class=\"status {class}\">{word}</span>"));
    }
    html
}

/// Each `<h2>` whose text opens with a section's number (`7. Type`) has the number set apart.
fn numbered(html: &str) -> String {
    each(html, "<h2 id=\"", "</h2>", |inner| {
        let (id, text) = inner.split_once("\">")?;
        let (n, title) = text.split_once(". ")?;
        (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| format!("<h2 id=\"{id}\"><span class=\"n\">{n}</span>{title}</h2>"))
    })
}

/// The body, each `<h2>` and what follows it up to the next one in a `<section>`.
fn sections(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 512);
    for part in body.split("<h2").filter(|p| !p.trim().is_empty()) {
        out.push_str("<section>\n<h2");
        out.push_str(part.trim_end());
        out.push_str("\n</section>\n\n");
    }
    out
}

/// What stands where the doc says to draw the ranges: a caption, and the box the script fills.
fn drawing() -> String {
    "<div class=\"drawn\"><p class=\"source\">Drawn from the skeleton's theme (<code>templates/app/crates/engine/src/theme.rs</code>): \
     each range's entries, each as its fill in its border, then <code>strong</code> text on its text background, on each look's \
     <code>background</code>.</p><div id=\"ranges-out\" class=\"stack\"></div></div>"
        .to_owned()
}

/// The script that draws the ranges: the theme's colours as data, then [`DRAW`].
fn script(ranges: &[Range]) -> Result<String, String> {
    if ranges.is_empty() {
        return Err("the doc draws the ranges, and the theme has none".to_owned());
    }
    let list = |c: &[String]| c.iter().map(|h| format!("\"{h}\"")).collect::<Vec<_>>().join(", ");
    let mut s =
        String::from("\n<script>\n// The colours: the skeleton theme's RANGES (templates/app/crates/engine/src/theme.rs), in its order.\nconst RANGES = [\n");
    for r in ranges {
        let none = r.none.as_ref().map(|n| format!(", none: [{}]", list(n))).unwrap_or_default();
        s.push_str(&format!("  {{ name: \"{}\", dark: [{}], light: [{}]{none} }},\n", r.name, list(&r.dark), list(&r.light)));
    }
    s.push_str("];\n\n");
    s.push_str(DRAW);
    s.push_str("</script>\n");
    Ok(s)
}
