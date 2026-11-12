//! The theme as data (docs/app-architecture.md › The UI is a client, Colours from the theme): the
//! colours the UI draws with, in a dark and a light look, kept here, below the toolkit, so
//! `ui.theme` ([`crate::commands::UiThemeCommand`]) gives them to every surface and the UI crate's
//! theme module turns them into the toolkit's colours. Two kinds:
//!
//! - **Tokens**, named for what a colour means ([`TOKENS`]: `error`, `selection`, `muted`, …). Each
//!   is egui's own look, dark and light, or a nudge of it where egui's falls short of the contrast
//!   text needs (4.5:1 against the background, WCAG 2's minimum for text); the nudge is said where
//!   the value is.
//! - **Categorical ranges** for identifiers ([`RANGES`]: 8, 16 and 32 colours) such as the rows of
//!   a timeline, the bars of a scrubber, a legend's entries or badges. An identifier's colour comes
//!   from the range: [`Shades::pick`] hashes it to an entry (FNV-1a, [`fnv1a`]), so the same id gets
//!   the same colour on every run, surface and machine; [`Shades::at`] takes a number in order (a
//!   series, a marker). Never a colour made where it is drawn. Each entry reaches 3:1 against the
//!   background (WCAG 2's minimum for a part of the interface), and [`behind_text`] makes one a
//!   text background on which [`STRONG`] text reaches 4.5:1.
//!
//! The values are tested (`theme/tests.rs`): every token's contrast, every entry's, the text on
//! every entry, how far apart the entries of a range stay, and the hash's pins.

use std::fmt::{self, Display, Write as _};

use serde::{Deserialize, Serialize};

/// The contrast text needs against what it is drawn on (WCAG 2, 1.4.3: 4.5:1).
pub const TEXT_CONTRAST: f64 = 4.5;

/// The contrast a fill needs against the background, as a part of the interface (WCAG 2, 1.4.11: 3:1).
pub const FILL_CONTRAST: f64 = 3.0;

/// A colour: 8-bit sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// `0xRRGGBB` as a colour (the bits above the 24th are ignored).
    pub const fn hex(code: u32) -> Rgb {
        Rgb { r: (code >> 16) as u8, g: (code >> 8) as u8, b: code as u8 }
    }

    /// `#RRGGBB`, as `ui.theme` gives a colour.
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// A colour given as `#RRGGBB` (either case); `None` for anything else.
    pub fn parse(text: &str) -> Option<Rgb> {
        let digits = text.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(digits, 16).ok().map(Rgb::hex)
    }

    /// The WCAG 2 relative luminance: 0 for black, 1 for white (sRGB decoded to linear light,
    /// then weighted 0.2126, 0.7152, 0.0722).
    pub fn luminance(self) -> f64 {
        let linear = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    /// The WCAG 2 contrast ratio of this colour and `other`: from 1 (the same) to 21 (black and white).
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

/// The two looks a theme has: egui's dark and light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Look {
    Dark,
    Light,
}

/// A colour named for what it means, in each look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub name: &'static str,
    pub meaning: &'static str,
    pub dark: Rgb,
    pub light: Rgb,
}

impl Token {
    /// Its colour in `look`.
    pub const fn of(&self, look: Look) -> Rgb {
        match look {
            Look::Dark => self.dark,
            Look::Light => self.light,
        }
    }
}

// The tokens. Each is egui 0.37's `Visuals::dark()` and `Visuals::light()` (the UI crate's theme
// tests hold them to it), or that nudged where it is short of 4.5:1 against `background`: CIELAB
// lightness moved, its hue and chroma kept, to a little over 4.5:1. `selection` is a fill: the
// text on it is `strong`, and that is what reaches 4.5:1.

/// egui's `panel_fill`.
pub const BACKGROUND: Token =
    Token { name: "background", meaning: "What everything is drawn on: a panel's fill", dark: Rgb::hex(0x1B1B1B), light: Rgb::hex(0xF8F8F8) };
/// egui's `text_color()`.
pub const TEXT: Token = Token { name: "text", meaning: "Body text", dark: Rgb::hex(0x8C8C8C), light: Rgb::hex(0x505050) };
/// egui's `strong_text_color()`.
pub const STRONG: Token = Token {
    name: "strong",
    meaning: "Emphasised text, and text on a range colour made a text background",
    dark: Rgb::hex(0xFFFFFF),
    light: Rgb::hex(0x000000),
};
/// egui's `weak_text_color()` as it shows on the background (`#5F5F5F` dark, 2.70:1; `#939393` light,
/// 2.89:1), nudged to `#848484` (4.61:1) and `#707070` (4.66:1).
pub const MUTED: Token =
    Token { name: "muted", meaning: "Text that matters less: a command the UI can't run here, a hint", dark: Rgb::hex(0x848484), light: Rgb::hex(0x707070) };
/// egui's `hyperlink_color`; the light one (`#009BFF`, 2.77:1) nudged to `#0B74C0` (4.63:1).
pub const ACCENT: Token = Token { name: "accent", meaning: "A link, or what draws the eye to an action", dark: Rgb::hex(0x5AAAFF), light: Rgb::hex(0x0B74C0) };
/// egui's `selection.bg_fill`, a fill: `strong` text on it is 7.39:1 dark and 12.75:1 light.
pub const SELECTION: Token =
    Token { name: "selection", meaning: "What is selected: its fill, with strong text on it", dark: Rgb::hex(0x005C80), light: Rgb::hex(0x90D1FF) };
/// egui's look has no success colour: its named greens, `Color32::LIGHT_GREEN` and `DARK_GREEN`.
pub const SUCCESS: Token = Token { name: "success", meaning: "Something that worked", dark: Rgb::hex(0x90EE90), light: Rgb::hex(0x006400) };
/// egui's `warn_fg_color`; the light one (`#FF6400`, 2.79:1) nudged to `#C14B04` (4.62:1).
pub const WARNING: Token = Token { name: "warning", meaning: "Something to look at that hasn't failed", dark: Rgb::hex(0xFF8F00), light: Rgb::hex(0xC14B04) };
/// egui's `error_fg_color`, `#FF0000` in both looks (4.31:1 dark, 3.76:1 light), nudged to `#FE3017`
/// (4.63:1) and `#E20C03` (4.61:1).
pub const ERROR: Token = Token { name: "error", meaning: "A failure, or why a command can't run now", dark: Rgb::hex(0xFE3017), light: Rgb::hex(0xE20C03) };

/// Every token, in the order `ui.theme` gives them.
pub const TOKENS: [Token; 9] = [BACKGROUND, TEXT, STRONG, MUTED, ACCENT, SELECTION, SUCCESS, WARNING, ERROR];

/// A categorical range: colours for identifiers, in each look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub name: &'static str,
    /// What it is for.
    pub meaning: &'static str,
    /// Where its colours come from.
    pub source: &'static str,
    pub dark: Shades,
    pub light: Shades,
}

impl Range {
    /// Its colours in `look`.
    pub const fn of(&self, look: Look) -> Shades {
        match look {
            Look::Dark => self.dark,
            Look::Light => self.light,
        }
    }
}

/// One look's colours of a range: what [`Shades::pick`] and [`Shades::at`] choose from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shades {
    entries: &'static [Rgb],
}

/// What an empty range would give (none is): a mid grey, rather than a panic.
const NO_ENTRY: Rgb = Rgb::hex(0x808080);

impl Shades {
    pub const fn entries(&self) -> &'static [Rgb] {
        self.entries
    }

    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry for the number `n`, in order: `n` modulo the range's size.
    pub fn at(&self, n: u64) -> Rgb {
        n.checked_rem(self.len() as u64).and_then(|i| self.entries.get(usize::try_from(i).ok()?)).copied().unwrap_or(NO_ENTRY)
    }

    /// The entry an identifier always gets: its [`fnv1a`] hash modulo the range's size. `id` is
    /// hashed as it displays, so a number is its decimal digits: `pick(7)` is `pick("7")`.
    pub fn pick(&self, id: impl Display) -> Rgb {
        self.at(fnv1a(id))
    }

    /// The index [`Shades::pick`] chooses for `id`.
    pub fn index_of(&self, id: impl Display) -> usize {
        fnv1a(id).checked_rem(self.len() as u64).and_then(|i| usize::try_from(i).ok()).unwrap_or(0)
    }
}

/// FNV-1a, 64-bit (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`), over the UTF-8 bytes
/// `id` displays as. Fixed and documented, unlike std's `DefaultHasher`, whose output may change
/// between Rust releases: an identifier's colour must not.
pub fn fnv1a(id: impl Display) -> u64 {
    let mut hash = Fnv1a(0xcbf2_9ce4_8422_2325);
    // Writing to the hash can't fail; a `Display` that fails leaves what it wrote hashed.
    let _ = write!(hash, "{id}");
    hash.0
}

/// The hash's state, written to as text so nothing is allocated.
struct Fnv1a(u64);

impl fmt::Write for Fnv1a {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        Ok(())
    }
}

/// `colour` made a background for text in `look`: its HSL lightness at most 0.5 in the dark look
/// (light text on it) or at least 0.5 in the light (dark text), then moved further the same way
/// until [`STRONG`] text on it reaches [`TEXT_CONTRAST`]. Its hue and saturation are kept.
pub fn behind_text(colour: Rgb, look: Look) -> Rgb {
    let text = STRONG.of(look);
    let (hue, saturation, lightness) = to_hsl(colour);
    let (mut lightness, step) = match look {
        Look::Dark => (lightness.min(0.5), -0.01),
        Look::Light => (lightness.max(0.5), 0.01),
    };
    let mut out = from_hsl(hue, saturation, lightness);
    // At lightness 0 or 1 the colour is black or white, 21:1 with the other: the loop ends by then.
    for _ in 0..=100 {
        if out.contrast(text) >= TEXT_CONTRAST {
            break;
        }
        lightness = (lightness + step).clamp(0.0, 1.0);
        out = from_hsl(hue, saturation, lightness);
    }
    out
}

/// `colour`'s hue (in turns, 0 to 1), saturation and lightness.
fn to_hsl(colour: Rgb) -> (f64, f64, f64) {
    let [r, g, b] = [colour.r, colour.g, colour.b].map(|c| f64::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let chroma = max - min;
    if chroma <= f64::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = chroma / (1.0 - (2.0 * lightness - 1.0).abs());
    let sixths = if max <= r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max <= g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    };
    (sixths / 6.0, saturation, lightness)
}

/// The colour of `hue` (in turns), `saturation` and `lightness`.
fn from_hsl(hue: f64, saturation: f64, lightness: f64) -> Rgb {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let channel = |n: f64| {
        let k = (n + hue * 12.0).rem_euclid(12.0);
        let c = lightness - chroma / 2.0 * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        // In 0..=255 once clamped: the cast neither wraps nor truncates a whole part.
        (c.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Rgb::new(channel(0.0), channel(8.0), channel(4.0))
}

// The ranges. Each look's entries reach 3:1 against its background; the tests measure how far
// apart they stay (CIEDE2000, at least 10 between any two entries and 15 between neighbours).

/// ColorBrewer Set1's first 8 (Cynthia A. Brewer, colorbrewer2.org, Apache-2.0), as published.
const SET1_DARK: [Rgb; 8] = [
    Rgb::hex(0xE41A1C),
    Rgb::hex(0x377EB8),
    Rgb::hex(0x4DAF4A),
    Rgb::hex(0x984EA3),
    Rgb::hex(0xFF7F00),
    Rgb::hex(0xFFFF33),
    Rgb::hex(0xA65628),
    Rgb::hex(0xF781BF),
];

/// Set1 on the light background: four entries are under 3:1 there, and are nudged darker (CIELAB
/// lightness, hue and chroma kept) to just over it.
const SET1_LIGHT: [Rgb; 8] = [
    Rgb::hex(0xE41A1C),
    Rgb::hex(0x377EB8),
    Rgb::hex(0x3EA13D), // nudged from #4DAF4A (2.62:1)
    Rgb::hex(0x984EA3),
    Rgb::hex(0xDF6E01), // nudged from #FF7F00 (2.39:1)
    Rgb::hex(0x919207), // nudged from #FFFF33 (1.01:1)
    Rgb::hex(0xA65628),
    Rgb::hex(0xD965A3), // nudged from #F781BF (2.24:1)
];

/// colorcet's `glasbey_light` (`glasbey_bw_minc_20_minl_30`, colorcet 3.2.1: Peter Kovesi,
/// holoviz/colorcet, CC-BY 4.0), made for dark backgrounds by Glasbey et al.'s method ("Colour
/// displays for categorical images", Color Research & Application 32(4), 2007), which orders a set
/// so that each colour is as far as it can be from all before it. Its first 32 that reach 3:1 on
/// the dark background, in its order: entries 7, 12, 25, 26, 27 and 32 don't, and are left out.
/// The 16 range is the first 16.
const GLASBEY_DARK: [Rgb; 32] = [
    Rgb::hex(0xD70000),
    Rgb::hex(0x028800),
    Rgb::hex(0xB600FF),
    Rgb::hex(0x06ACC6),
    Rgb::hex(0x98FF00),
    Rgb::hex(0xFFA530),
    Rgb::hex(0xFF8FC8),
    Rgb::hex(0x00FECF),
    Rgb::hex(0xB0A5FF),
    Rgb::hex(0x94AD84),
    Rgb::hex(0x9A6900),
    Rgb::hex(0xD3008C),
    Rgb::hex(0xFEF590),
    Rgb::hex(0xC86F66),
    Rgb::hex(0x9EE3FF),
    Rgb::hex(0x00C946),
    Rgb::hex(0xA977AD),
    Rgb::hex(0xB8BB02),
    Rgb::hex(0xF4C0B1),
    Rgb::hex(0xFF28FD),
    Rgb::hex(0xF3CEFF),
    Rgb::hex(0x009F7D),
    Rgb::hex(0xFF6200),
    Rgb::hex(0xFF3465),
    Rgb::hex(0xA0E492),
    Rgb::hex(0x8D9BB2),
    Rgb::hex(0x829126),
    Rgb::hex(0x78C7BB),
    Rgb::hex(0xBC9258),
    Rgb::hex(0xE58FFF),
    Rgb::hex(0x72B9FF),
    Rgb::hex(0xC6A5C1),
];

/// The same identifiers on the light background, from colorcet's `glasbey_dark`
/// (`glasbey_bw_minc_20_maxl_70`, the same source, made for light backgrounds): for each entry of
/// [`GLASBEY_DARK`] in turn, the unused one of `glasbey_dark`'s first 120 that reaches 3:1 on the
/// light background and is nearest in hue and chroma (CIELAB a*b* distance plus 0.3 times the
/// lightness difference). An identifier keeps its colour family when the look changes, and the 32
/// stay further apart (CIEDE2000 10.3 at the closest) than `glasbey_dark`'s own first 32 that pass
/// (8.3). Its `glasbey_dark` index is noted beside each.
const GLASBEY_LIGHT: [Rgb; 32] = [
    Rgb::hex(0xD70000), // 0
    Rgb::hex(0x028800), // 2
    Rgb::hex(0xBB00FE), // 57
    Rgb::hex(0x0088B1), // 74
    Rgb::hex(0x6C9700), // 78
    Rgb::hex(0xCC7407), // 41
    Rgb::hex(0xC5668F), // 38
    Rgb::hex(0x019F70), // 96
    Rgb::hex(0x5A4AA3), // 68
    Rgb::hex(0x546745), // 29
    Rgb::hex(0x755A00), // 80
    Rgb::hex(0xC63189), // 90
    Rgb::hex(0x8F7B01), // 20
    Rgb::hex(0x9D5743), // 108
    Rgb::hex(0x086B84), // 92
    Rgb::hex(0x006D2E), // 66
    Rgb::hex(0xA67BB8), // 23
    Rgb::hex(0x596D00), // 63
    Rgb::hex(0xA2766A), // 11
    Rgb::hex(0xFF00CE), // 47
    Rgb::hex(0x836F8C), // 98
    Rgb::hex(0x00816A), // 39
    Rgb::hex(0xEB4E00), // 77
    Rgb::hex(0xCB0044), // 75
    Rgb::hex(0x004B00), // 19
    Rgb::hex(0x536177), // 117
    Rgb::hex(0x4C5001), // 104
    Rgb::hex(0x4B9894), // 83
    Rgb::hex(0x9F6E32), // 88
    Rgb::hex(0x7F2B8E), // 42
    Rgb::hex(0x698DBE), // 103
    Rgb::hex(0xAC8499), // 89
];

/// The 8 range: few identifiers, each worth a name (a series, a marker by number: [`Shades::at`]).
pub const RANGE_8: Range = Range {
    name: "range8",
    meaning: "Up to 8 identifiers, each told apart at a glance: a series or a marker by number (at), or a few ids (pick)",
    source: "ColorBrewer Set1, first 8 (Cynthia A. Brewer, colorbrewer2.org, Apache-2.0); in the light look 4 entries darkened to 3:1",
    dark: Shades { entries: &SET1_DARK },
    light: Shades { entries: &SET1_LIGHT },
};

/// The 16 range: more identifiers, by a stable pick of their id (an object's type, a thread).
pub const RANGE_16: Range = Range {
    name: "range16",
    meaning: "Up to 16 identifiers by a stable pick of their id: an object's type, a thread, a source",
    source: "The first 16 of range32",
    dark: Shades { entries: GLASBEY_DARK.split_at(16).0 },
    light: Shades { entries: GLASBEY_LIGHT.split_at(16).0 },
};

/// The 32 range: many identifiers side by side. Any two differ (CIEDE2000 10 or more), but 32
/// colours can't each be named or matched to a legend alone: it tells neighbours apart.
pub const RANGE_32: Range = Range {
    name: "range32",
    meaning: "Up to 32 identifiers side by side (the rows of a timeline, the bars of a scrubber): told apart beside each other, not matched to a legend alone",
    source: "Glasbey et al. 2007's sets in colorcet 3.2.1 (Peter Kovesi, CC-BY 4.0): glasbey_light in the dark look, glasbey_dark's nearest hue to each in the light",
    dark: Shades { entries: &GLASBEY_DARK },
    light: Shades { entries: &GLASBEY_LIGHT },
};

/// Every range, in the order `ui.theme` gives them.
pub const RANGES: [Range; 3] = [RANGE_8, RANGE_16, RANGE_32];

/// What `ui.theme` returns: the theme as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ThemeData {
    /// The colours named for what they mean.
    pub tokens: Vec<TokenData>,
    /// The categorical ranges, for identifiers.
    pub ranges: Vec<RangeData>,
}

/// A token, as `ui.theme` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TokenData {
    pub name: String,
    /// What it is for.
    pub meaning: String,
    /// Its colour in the dark look, `#RRGGBB`.
    pub dark: String,
    /// Its colour in the light look, `#RRGGBB`.
    pub light: String,
}

/// A categorical range, as `ui.theme` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RangeData {
    pub name: String,
    /// How many colours it has.
    pub size: u32,
    /// What it is for.
    pub meaning: String,
    /// Where its colours come from.
    pub source: String,
    /// Its colours, by index.
    pub entries: Vec<EntryData>,
}

/// One colour of a range, in each look, and made a background for text in each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EntryData {
    pub index: u32,
    /// `#RRGGBB` in the dark look.
    pub dark: String,
    /// `#RRGGBB` in the light look.
    pub light: String,
    /// The dark look's colour made a text background ([`behind_text`]), `#RRGGBB`.
    pub behind_text_dark: String,
    /// The light look's colour made a text background, `#RRGGBB`.
    pub behind_text_light: String,
}

/// The theme as `ui.theme` gives it.
pub fn data() -> ThemeData {
    let tokens = TOKENS.iter().map(|t| TokenData { name: t.name.into(), meaning: t.meaning.into(), dark: t.dark.to_hex(), light: t.light.to_hex() }).collect();
    ThemeData { tokens, ranges: RANGES.iter().map(range_data).collect() }
}

fn range_data(range: &Range) -> RangeData {
    let entries = range.dark.entries().iter().zip(range.light.entries()).zip(0u32..).map(|((&dark, &light), index)| EntryData {
        index,
        dark: dark.to_hex(),
        light: light.to_hex(),
        behind_text_dark: behind_text(dark, Look::Dark).to_hex(),
        behind_text_light: behind_text(light, Look::Light).to_hex(),
    });
    let entries: Vec<EntryData> = entries.collect();
    let size = u32::try_from(entries.len()).unwrap_or(u32::MAX);
    RangeData { name: range.name.into(), size, meaning: range.meaning.into(), source: range.source.into(), entries }
}

#[cfg(test)]
mod tests;
