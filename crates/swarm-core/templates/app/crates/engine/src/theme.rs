//! The theme as data (docs/app-architecture.md › The UI is a client, Colours from the theme): the
//! colours the UI draws with, in a dark and a light look, kept here, below the toolkit, so
//! `ui.theme` ([`crate::commands::UiThemeCommand`]) gives them to every surface and the UI crate's
//! theme module turns them into the toolkit's colours.
//!
//! **The colours are the palette the swarm's tools share**, so an app built on Swarm Core looks
//! like the swarm's other tools. The shared palette has a dark look and a light one (Qt Fusion's
//! light palette), a series, markers and a `darker`. Three kinds:
//!
//! - **Tokens**, named for what a colour means ([`TOKENS`]: `background`, `text`, `selection`,
//!   `error`, …), each the shared palette's token of that meaning, its name beside it.
//! - **The shared palette's ranges**: the [`SERIES`] (ColorBrewer Dark2 in the dark look, Set2 in the
//!   light), whose `at(n)` is entry `n % 7` and whose [`Shades::none`] is the grey for something with
//!   no number; and the [`MARKERS`] (ColorBrewer Set1 with the shared orange,
//!   then grey, the same in both looks).
//! - **Swarm Core's own ranges**, beyond the shared palette's ([`RANGE_16`], [`RANGE_32`]), for
//!   many identifiers side by side.
//!
//! An identifier's colour comes from a range: [`Shades::pick`] hashes it to an entry (FNV-1a,
//! [`fnv1a`]), so the same id gets the same colour on every run, surface and machine; [`Shades::at`]
//! takes a number in order (a series, a marker). Never a colour made where it is drawn.
//! [`behind_text`] makes an entry a text background on which [`STRONG`] text reaches 4.5:1, and
//! [`darker`] (the shared palette's: each channel × 0.8) is the border a filled entry is outlined in.
//!
//! The values are tested (`theme/tests.rs`): that each is the shared palette's, every token's contrast
//! where it is read, every entry's against the background, the text on every entry, how far apart the
//! entries of a range stay, and the hash's pins. Where the shared palette's own value falls short of a
//! minimum it is kept, not nudged, and the tests record it with its measured ratio and why
//! (`KEPT_SHORT`); Swarm Core's own ranges are nudged where they fall short, and say so.

use std::fmt::{self, Display, Write as _};

use serde::{Deserialize, Serialize};

/// The contrast text needs against what it is drawn on (WCAG 2, 1.4.3: 4.5:1).
pub const TEXT_CONTRAST: f64 = 4.5;

/// The contrast a fill needs against the background, as a part of the interface (WCAG 2, 1.4.11: 3:1).
pub const FILL_CONTRAST: f64 = 3.0;

/// The corner radius of windows, menus and widgets, in points: near-square, the same in both looks
/// (the App UI Substrate › Corner radius). `ui.theme` gives it as `corner_radius`.
pub const CORNER_RADIUS: u8 = 2;

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

/// The two looks a theme has: dark (the shared palette's Dark) and light (Qt Fusion's light palette, its Light).
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

// The tokens: the shared palette's (`Tokens::DARK` and
// `Tokens::LIGHT`), each under the name of its meaning, with the shared palette's own name
// beside it. Two are Swarm Core's, where the shared palette has none: `strong` and `success`. The
// tests hold every value to the shared palette's and measure each where it is read; the one that falls
// short (`accent` in the dark look) is kept and recorded there.

/// The shared palette's `bg`: the window.
pub const BACKGROUND: Token =
    Token { name: "background", meaning: "What everything is drawn on: a panel's fill", dark: Rgb::hex(0x353535), light: Rgb::hex(0xEFEFEF) };
/// The shared palette's `chrome`.
pub const CHROME: Token =
    Token { name: "chrome", meaning: "The menu bar, the status bar and other chrome", dark: Rgb::hex(0x353535), light: Rgb::hex(0xEFEFEF) };
/// The shared palette's `panel`.
pub const PANEL: Token = Token { name: "panel", meaning: "Floating windows, dialogs and popups", dark: Rgb::hex(0x353535), light: Rgb::hex(0xEFEFEF) };
/// The shared palette's `base`: where content sits.
pub const BASE: Token = Token { name: "base", meaning: "Views: lists, tables and text fields", dark: Rgb::hex(0x2A2A2A), light: Rgb::hex(0xFFFFFF) };
/// The shared palette's `alt_base`: alternate rows.
pub const ALT_BASE: Token = Token { name: "alt_base", meaning: "Every other row of a list drawn on base", dark: Rgb::hex(0x303030), light: Rgb::hex(0xF7F7F7) };
/// The shared palette's `header`: a header's fill.
pub const HEADER: Token = Token { name: "header", meaning: "Table header rows and a ruler", dark: Rgb::hex(0x3F3F3F), light: Rgb::hex(0xCACACA) };
/// The shared palette's `frame`: a frame's border. Decorative: no minimum.
pub const FRAME: Token = Token { name: "frame", meaning: "Lines that frame views and panels", dark: Rgb::hex(0x424242), light: Rgb::hex(0xABABAB) };
/// The shared palette's `border`. Decorative: no minimum.
pub const BORDER: Token =
    Token { name: "border", meaning: "Decorative lines: window frames and separators", dark: Rgb::hex(0x424242), light: Rgb::hex(0xABABAB) };
/// The shared palette's `text`.
pub const TEXT: Token = Token { name: "text", meaning: "Body text", dark: Rgb::hex(0xFCFCFC), light: Rgb::hex(0x000000) };
/// Swarm Core's: white or black, what the shared palette's `text_on` puts on a data fill (it has no
/// emphasised text of its own).
pub const STRONG: Token = Token {
    name: "strong",
    meaning: "Emphasised text, and text on a range colour made a text background",
    dark: Rgb::hex(0xFFFFFF),
    light: Rgb::hex(0x000000),
};
/// The shared palette's `text_dim`.
pub const MUTED: Token =
    Token { name: "muted", meaning: "Text that matters less: a command the UI can't run here, a hint", dark: Rgb::hex(0xA6A6A6), light: Rgb::hex(0x666666) };
/// The shared palette's `selected_text`.
pub const SELECTED_TEXT: Token = Token { name: "selected_text", meaning: "Text on the selection", dark: Rgb::hex(0xFCFCFC), light: Rgb::hex(0x000000) };
/// The shared palette's `link`.
pub const LINK: Token = Token { name: "link", meaning: "A link", dark: Rgb::hex(0x4DA6FF), light: Rgb::hex(0x1F6FA8) };
/// The shared palette's `control`.
pub const CONTROL: Token =
    Token { name: "control", meaning: "The fill of a widget at rest: a button, a field", dark: Rgb::hex(0x454545), light: Rgb::hex(0xF7F7F7) };
/// The shared palette's `control_border`.
pub const CONTROL_BORDER: Token =
    Token { name: "control_border", meaning: "The edge of a widget at rest: 3:1 on the background", dark: Rgb::hex(0x858585), light: Rgb::hex(0x7A7A7A) };
/// The shared palette's `accent`: its accent blue. In the dark look it is 2.72:1 on the
/// background, kept (the tests' `KEPT_SHORT` says why).
pub const ACCENT: Token =
    Token { name: "accent", meaning: "An accent that carries no text: what has focus, an outline", dark: Rgb::hex(0x007ACC), light: Rgb::hex(0x2B78AE) };
/// The shared palette's `selection`, a fill: `selected_text` on it is 5.07:1 dark and 14.74:1 light.
pub const SELECTION: Token =
    Token { name: "selection", meaning: "What is selected: its fill, with selected_text on it", dark: Rgb::hex(0x0070BB), light: Rgb::hex(0xC1DCEE) };
/// Swarm Core's: the shared palette has no success colour. egui's named greens,
/// `Color32::LIGHT_GREEN` and `DARK_GREEN`, as before.
pub const SUCCESS: Token = Token { name: "success", meaning: "Something that worked", dark: Rgb::hex(0x90EE90), light: Rgb::hex(0x006400) };
/// The shared palette's `warn`: the shared orange in the dark look.
pub const WARNING: Token = Token { name: "warning", meaning: "Something to look at that hasn't failed", dark: Rgb::hex(0xFF7F1C), light: Rgb::hex(0x965C00) };
/// The shared palette's `error`.
pub const ERROR: Token = Token { name: "error", meaning: "A failure, or why a command can't run now", dark: Rgb::hex(0xFF8A80), light: Rgb::hex(0xB42318) };

/// Every token, in the order `ui.theme` gives them: surfaces, text, controls and highlights, states.
pub const TOKENS: [Token; 20] = [
    BACKGROUND,
    CHROME,
    PANEL,
    BASE,
    ALT_BASE,
    HEADER,
    FRAME,
    BORDER,
    TEXT,
    STRONG,
    MUTED,
    SELECTED_TEXT,
    LINK,
    CONTROL,
    CONTROL_BORDER,
    ACCENT,
    SELECTION,
    SUCCESS,
    WARNING,
    ERROR,
];

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

/// One look's colours of a range: what [`Shades::pick`] and [`Shades::at`] choose from, and the
/// colour for none, where the range has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shades {
    entries: &'static [Rgb],
    none: Option<Rgb>,
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

    /// The colour of something with no number or id (the series' grey: the shared palette's work with
    /// no issue). Never one of the entries, so [`Shades::at`] and [`Shades::pick`] never give it.
    pub const fn none(&self) -> Option<Rgb> {
        self.none
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

/// A range entry's border, how a filled entry (a bar, a mark) is outlined: each channel × 0.8,
/// rounded down (the shared palette's `darker`: the darker edge on its bars).
pub fn darker(colour: Rgb) -> Rgb {
    let channel = |c: u8| u8::try_from(u16::from(c) * 4 / 5).unwrap_or(c);
    Rgb::new(channel(colour.r), channel(colour.g), channel(colour.b))
}

// The ranges. The shared palette's two (the series and the markers) are its values as they are, the
// published palettes they come from; where one falls short of 3:1 on the background it is kept,
// and the tests record it. Swarm Core's two (16 and 32) reach 3:1 against the background in
// each look, nudged where they didn't. The tests measure how far apart each range's entries stay
// (CIEDE2000, at least 10 between any two entries and 15 between neighbours).

/// ColorBrewer Dark2's first 7 (Cynthia A. Brewer, colorbrewer2.org, Apache-2.0), as published:
/// the shared palette's dark series (teal, rust, violet, magenta, lime, gold, copper).
const DARK2: [Rgb; 7] =
    [Rgb::hex(0x1B9E77), Rgb::hex(0xD95F02), Rgb::hex(0x7570B3), Rgb::hex(0xE7298A), Rgb::hex(0x66A61E), Rgb::hex(0xE6AB02), Rgb::hex(0xA6761D)];

/// Dark2's eighth, its grey: the shared palette's dark series' colour for none (work with no issue).
const DARK2_NONE: Rgb = Rgb::hex(0x666666);

/// ColorBrewer Set2's first 7 (the same source), as published: the shared palette's light series.
const SET2: [Rgb; 7] =
    [Rgb::hex(0x66C2A5), Rgb::hex(0xFC8D62), Rgb::hex(0x8DA0CB), Rgb::hex(0xE78AC3), Rgb::hex(0xA6D854), Rgb::hex(0xFFD92F), Rgb::hex(0xE5C494)];

/// Set2's eighth, its grey: the light series' colour for none.
const SET2_NONE: Rgb = Rgb::hex(0xB3B3B3);

/// The shared palette's 9-colour marker palette, in its order (markers by number): ColorBrewer Set1's 8 (the
/// same source) with the shared orange `#FF7F1C` for Set1's `#FF7F00`, then grey. The same in both
/// looks, all nine in its numbered order.
const MARKER_PALETTE: [Rgb; 9] = [
    Rgb::hex(0xE41A1C),
    Rgb::hex(0x377EB8),
    Rgb::hex(0x4DAF4A),
    Rgb::hex(0x984EA3),
    Rgb::hex(0xFF7F1C),
    Rgb::hex(0xFFFF33),
    Rgb::hex(0xA65628),
    Rgb::hex(0xF781BF),
    Rgb::hex(0x999999),
];

/// Swarm Core's own, beyond the shared palette's. colorcet's `glasbey_light`
/// (`glasbey_bw_minc_20_minl_30`, colorcet 3.2.1: Peter Kovesi, holoviz/colorcet, CC-BY 4.0), made
/// for dark backgrounds by Glasbey et al.'s method ("Colour displays for categorical images", Color
/// Research & Application 32(4), 2007), which orders a set so that each colour is as far as it can
/// be from all before it. Its first 32 that reached 3:1 on egui's dark background (`#1B1B1B`),
/// in its order: entries 7, 12, 25, 26, 27 and 32 didn't, and are left out. On the shared palette's
/// (`#353535`) five fall short, and are nudged lighter (CIELAB lightness, a* and b* kept as far as
/// sRGB allows) to just over 3:1, each noted. The 16 range is the first 16.
const GLASBEY_DARK: [Rgb; 32] = [
    Rgb::hex(0xF12F17), // nudged from #D70000 (2.27:1)
    Rgb::hex(0x1B9211), // nudged from #028800 (2.64:1)
    Rgb::hex(0xC727FF), // nudged from #B600FF (2.59:1)
    Rgb::hex(0x06ACC6),
    Rgb::hex(0x98FF00),
    Rgb::hex(0xFFA530),
    Rgb::hex(0xFF8FC8),
    Rgb::hex(0x00FECF),
    Rgb::hex(0xB0A5FF),
    Rgb::hex(0x94AD84),
    Rgb::hex(0xA77412), // nudged from #9A6900 (2.57:1)
    Rgb::hex(0xE6299C), // nudged from #D3008C (2.42:1)
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
/// [`GLASBEY_DARK`] in turn, the unused one of `glasbey_dark`'s first 120 that reached 3:1 on egui's
/// light background (`#F8F8F8`) and is nearest in hue and chroma (CIELAB a*b* distance plus 0.3
/// times the lightness difference). An identifier keeps its colour family when the look changes,
/// and the 32 stay further apart (CIEDE2000 10.0 at the closest) than `glasbey_dark`'s own first 32
/// that pass (8.3). Its `glasbey_dark` index is noted beside each. On the shared palette's light
/// background (`#EFEFEF`) six fall short, and are nudged darker, as the dark ones lighter.
const GLASBEY_LIGHT: [Rgb; 32] = [
    Rgb::hex(0xD70000), // 0
    Rgb::hex(0x028800), // 2
    Rgb::hex(0xBB00FE), // 57
    Rgb::hex(0x0088B1), // 74
    Rgb::hex(0x6C9700), // 78
    Rgb::hex(0xCC7407), // 41
    Rgb::hex(0xC5668F), // 38
    Rgb::hex(0x009D6F), // 96, nudged from #019F70 (2.95:1)
    Rgb::hex(0x5A4AA3), // 68
    Rgb::hex(0x546745), // 29
    Rgb::hex(0x755A00), // 80
    Rgb::hex(0xC63189), // 90
    Rgb::hex(0x8F7B01), // 20
    Rgb::hex(0x9D5743), // 108
    Rgb::hex(0x086B84), // 92
    Rgb::hex(0x006D2E), // 66
    Rgb::hex(0xA57AB7), // 23, nudged from #A67BB8 (2.97:1)
    Rgb::hex(0x596D00), // 63
    Rgb::hex(0xA2766A), // 11
    Rgb::hex(0xFD00CD), // 47, nudged from #FF00CE (2.97:1)
    Rgb::hex(0x836F8C), // 98
    Rgb::hex(0x00816A), // 39
    Rgb::hex(0xEB4E00), // 77
    Rgb::hex(0xCB0044), // 75
    Rgb::hex(0x004B00), // 19
    Rgb::hex(0x536177), // 117
    Rgb::hex(0x4C5001), // 104
    Rgb::hex(0x499692), // 83, nudged from #4B9894 (2.94:1)
    Rgb::hex(0x9F6E32), // 88
    Rgb::hex(0x7F2B8E), // 42
    Rgb::hex(0x688CBD), // 103, nudged from #698DBE (2.97:1)
    Rgb::hex(0xA67E93), // 89, nudged from #AC8499 (2.80:1)
];

/// The shared palette's series: numbered things in order, `at(n)` the entry `n % 7`, and the grey for
/// none (the shared palette's `Tokens::issue`: a work bar's colour by its issue number).
pub const SERIES: Range = Range {
    name: "series",
    meaning: "Numbered things in order (an issue, a lane, a frame): at(n) is entry n % 7, and none is the grey for something with no number",
    source: "The shared palette's series: ColorBrewer Dark2 in the dark look, Set2 in the light (Cynthia A. Brewer, colorbrewer2.org, Apache-2.0), as published",
    dark: Shades { entries: &DARK2, none: Some(DARK2_NONE) },
    light: Shades { entries: &SET2, none: Some(SET2_NONE) },
};

/// The shared palette's marker palette: up to 9 marks told apart at a glance, by number or a few ids.
pub const MARKERS: Range = Range {
    name: "markers",
    meaning: "Up to 9 marks told apart at a glance, the same in both looks: a marker by number (at), or a few ids (pick)",
    source: "The shared palette's markers: ColorBrewer Set1 (Cynthia A. Brewer, colorbrewer2.org, Apache-2.0) with the shared orange #FF7F1C, then grey",
    dark: Shades { entries: &MARKER_PALETTE, none: None },
    light: Shades { entries: &MARKER_PALETTE, none: None },
};

/// The 16 range, Swarm Core's own: more identifiers, by a stable pick of their id (an object's
/// type, a thread).
pub const RANGE_16: Range = Range {
    name: "range16",
    meaning: "Up to 16 identifiers by a stable pick of their id: an object's type, a thread, a source",
    source: "Swarm Core's own, beyond the shared palette's: the first 16 of range32",
    dark: Shades { entries: GLASBEY_DARK.split_at(16).0, none: None },
    light: Shades { entries: GLASBEY_LIGHT.split_at(16).0, none: None },
};

/// The 32 range, Swarm Core's own: many identifiers side by side. Any two differ (CIEDE2000 10
/// or more), but 32 colours can't each be named or matched to a legend alone: it tells neighbours
/// apart.
pub const RANGE_32: Range = Range {
    name: "range32",
    meaning: "Up to 32 identifiers side by side (the rows of a timeline, the bars of a scrubber): told apart beside each other, not matched to a legend alone",
    source: "Swarm Core's own, beyond the shared palette's: Glasbey et al. 2007's sets in colorcet 3.2.1 (Peter Kovesi, CC-BY 4.0), glasbey_light in the dark look and glasbey_dark's nearest hue to each in the light, 11 entries nudged to 3:1 on the shared palette's backgrounds",
    dark: Shades { entries: &GLASBEY_DARK, none: None },
    light: Shades { entries: &GLASBEY_LIGHT, none: None },
};

/// Every range, in the order `ui.theme` gives them: the shared palette's, then Swarm Core's.
pub const RANGES: [Range; 4] = [SERIES, MARKERS, RANGE_16, RANGE_32];

/// What `ui.theme` returns: the theme as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ThemeData {
    /// The colours named for what they mean.
    pub tokens: Vec<TokenData>,
    /// The categorical ranges, for identifiers.
    pub ranges: Vec<RangeData>,
    /// The corner radius of windows, menus and widgets, in points ([`CORNER_RADIUS`]).
    pub corner_radius: u8,
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
    /// How many colours it has (`at` and `pick` choose from these; none is apart).
    pub size: u32,
    /// What it is for.
    pub meaning: String,
    /// Where its colours come from.
    pub source: String,
    /// Its colours, by index.
    pub entries: Vec<EntryData>,
    /// The colour for none, where the range has one (the series' grey), indexed after the entries.
    pub none: Option<EntryData>,
}

/// One colour of a range, in each look, made a background for text in each, and its border in each.
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
    /// The dark look's colour's border, when it is drawn filled ([`darker`]), `#RRGGBB`.
    pub border_dark: String,
    /// The light look's colour's border, `#RRGGBB`.
    pub border_light: String,
}

impl EntryData {
    fn of(index: u32, dark: Rgb, light: Rgb) -> EntryData {
        EntryData {
            index,
            dark: dark.to_hex(),
            light: light.to_hex(),
            behind_text_dark: behind_text(dark, Look::Dark).to_hex(),
            behind_text_light: behind_text(light, Look::Light).to_hex(),
            border_dark: darker(dark).to_hex(),
            border_light: darker(light).to_hex(),
        }
    }
}

/// The theme as `ui.theme` gives it.
pub fn data() -> ThemeData {
    let tokens = TOKENS.iter().map(|t| TokenData { name: t.name.into(), meaning: t.meaning.into(), dark: t.dark.to_hex(), light: t.light.to_hex() }).collect();
    ThemeData { tokens, ranges: RANGES.iter().map(range_data).collect(), corner_radius: CORNER_RADIUS }
}

fn range_data(range: &Range) -> RangeData {
    let entries = range.dark.entries().iter().zip(range.light.entries()).zip(0u32..).map(|((&dark, &light), index)| EntryData::of(index, dark, light));
    let entries: Vec<EntryData> = entries.collect();
    let size = u32::try_from(entries.len()).unwrap_or(u32::MAX);
    let none = range.dark.none().zip(range.light.none()).map(|(dark, light)| EntryData::of(size, dark, light));
    RangeData { name: range.name.into(), size, meaning: range.meaning.into(), source: range.source.into(), entries, none }
}

#[cfg(test)]
mod tests;
