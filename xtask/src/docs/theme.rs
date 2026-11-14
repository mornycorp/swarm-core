//! The skeleton theme's ranges, read from its source (`templates/app/crates/engine/src/theme.rs`):
//! the colour data a page's script draws. Read as text, as Swarm Core's own colour test reads
//! it, since the skeleton is a template, not a crate the xtask links. Anything not in the form the
//! theme writes is refused, naming what, so a change there fails the render rather than drawing
//! wrong colours.

/// One range, as the theme's `RANGES` gives it: its name, its entries in each look, and its colour
/// for none in each look, where it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub name: String,
    pub dark: Vec<String>,
    pub light: Vec<String>,
    pub none: Option<[String; 2]>,
}

/// Every range of the theme's `RANGES`, in its order.
pub fn ranges(theme: &str) -> Result<Vec<Range>, String> {
    let list = statement(theme, "pub const RANGES: ")?;
    let names = list.split_once("= [").and_then(|(_, r)| r.split_once(']')).map(|(names, _)| names).ok_or("theme.rs: `RANGES` isn't `[A, B, …]`")?;
    names.split(',').map(str::trim).filter(|n| !n.is_empty()).map(|n| range(theme, n)).collect()
}

/// The range the theme's `pub const <konst>: Range` defines.
fn range(theme: &str, konst: &str) -> Result<Range, String> {
    let s = statement(theme, &format!("pub const {konst}: Range = Range {{"))?;
    let name = s.split_once("name: \"").and_then(|(_, r)| r.split_once('"')).map(|(n, _)| n).ok_or_else(|| format!("theme.rs: {konst} has no name"))?;
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(format!("theme.rs: {konst}'s name `{name}` isn't a plain identifier"));
    }
    let look = |which: &str| -> Result<(Vec<String>, Option<String>), String> {
        let body = s.split_once(&format!("{which}: Shades {{")).and_then(|(_, r)| r.split_once('}')).map(|(b, _)| b);
        shades(theme, body.ok_or_else(|| format!("theme.rs: {konst} has no `{which}: Shades {{ … }}`"))?)
    };
    let (dark, dark_none) = look("dark")?;
    let (light, light_none) = look("light")?;
    let none = match (dark_none, light_none) {
        (Some(d), Some(l)) => Some([d, l]),
        (None, None) => None,
        _ => return Err(format!("theme.rs: {konst} has a colour for none in one look only")),
    };
    if dark.len() != light.len() || dark.is_empty() {
        return Err(format!("theme.rs: {konst} has {} dark entries and {} light", dark.len(), light.len()));
    }
    Ok(Range { name: name.to_owned(), dark, light, none })
}

/// A `Shades { entries: …, none: … }` body's entries and colour for none. The entries are a palette
/// (`&NAME`) or its first k (`NAME.split_at(k).0`); none is `None` or `Some(NAME)`.
fn shades(theme: &str, body: &str) -> Result<(Vec<String>, Option<String>), String> {
    let (entries, none) = body.split_once("entries:").and_then(|(_, r)| r.split_once(", none:")).ok_or_else(|| format!("theme.rs: `Shades {{{body}}}`"))?;
    let entries = entries.trim();
    let colours = if let Some(name) = entries.strip_prefix('&') {
        palette(theme, name)?
    } else if let Some((name, rest)) = entries.split_once(".split_at(") {
        let k: usize = rest.strip_suffix(").0").and_then(|k| k.parse().ok()).ok_or_else(|| format!("theme.rs: entries `{entries}`"))?;
        palette(theme, name)?.into_iter().take(k).collect()
    } else {
        return Err(format!("theme.rs: entries `{entries}` is neither `&NAME` nor `NAME.split_at(k).0`"));
    };
    let none = match none.trim() {
        "None" => None,
        some => {
            let name = some.strip_prefix("Some(").and_then(|r| r.strip_suffix(')')).ok_or_else(|| format!("theme.rs: none `{some}`"))?;
            let [one] = <[String; 1]>::try_from(palette(theme, name)?).map_err(|_| format!("theme.rs: {name} isn't one colour"))?;
            Some(one)
        }
    };
    Ok((colours, none))
}

/// The colours of the theme's `const <name>`, an array or one colour, as `#RRGGBB`.
fn palette(theme: &str, name: &str) -> Result<Vec<String>, String> {
    let s = statement(theme, &format!("const {name}: "))?;
    let hexes: Vec<String> = s.split("Rgb::hex(0x").skip(1).map(|r| r.chars().take(6).collect::<String>().to_uppercase()).collect();
    if hexes.is_empty() || hexes.iter().any(|h| h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(format!("theme.rs: {name} isn't `Rgb::hex(0xRRGGBB)` colours"));
    }
    Ok(hexes.into_iter().map(|h| format!("#{h}")).collect())
}

/// The statement of the theme's that starts with `start`: to the first line that ends in `;`, not
/// the `;` in a type (`[Rgb; 7]`). By lines, so a Windows checkout's CRLF ends a line too.
fn statement(theme: &str, start: &str) -> Result<String, String> {
    let at = theme.find(start).ok_or_else(|| format!("theme.rs: no `{}`", start.trim_end()))?;
    let mut out = String::new();
    for line in theme.get(at..).unwrap_or_default().lines() {
        out.push_str(line);
        out.push('\n');
        if line.trim_end().ends_with(';') {
            return Ok(out);
        }
    }
    Err(format!("theme.rs: `{}` never ends", start.trim_end()))
}
