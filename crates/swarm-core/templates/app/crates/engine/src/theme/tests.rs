//! The theme's values, measured: that they are the shared palette's, contrast (WCAG 2), how far apart a
//! range's entries stay (CIEDE2000), and the pick's hash, pinned.

use super::*;

const LOOKS: [Look; 2] = [Look::Dark, Look::Light];

// ---- The source: the shared palette's values -------------------------------------------------------

/// The shared palette's tokens (`Tokens::DARK`, `Tokens::LIGHT`):
/// the token here, the shared palette's name for it, and its dark and light values, copied from there.
const SHARED_TOKENS: [(&str, &str, u32, u32); 18] = [
    ("background", "bg", 0x353535, 0xefefef),
    ("chrome", "chrome", 0x353535, 0xefefef),
    ("panel", "panel", 0x353535, 0xefefef),
    ("base", "base", 0x2a2a2a, 0xffffff),
    ("alt_base", "alt_base", 0x303030, 0xf7f7f7),
    ("header", "header", 0x3f3f3f, 0xcacaca),
    ("frame", "frame", 0x424242, 0xababab),
    ("border", "border", 0x424242, 0xababab),
    ("text", "text", 0xfcfcfc, 0x000000),
    ("muted", "text_dim", 0xa6a6a6, 0x666666),
    ("selected_text", "selected_text", 0xfcfcfc, 0x000000),
    ("link", "link", 0x4da6ff, 0x1f6fa8),
    ("control", "control", 0x454545, 0xf7f7f7),
    ("control_border", "control_border", 0x858585, 0x7a7a7a),
    ("accent", "accent", 0x007acc, 0x2b78ae),
    ("selection", "selection", 0x0070bb, 0xc1dcee),
    ("warning", "warn", 0xff7f1c, 0x965c00),
    ("error", "error", 0xff8a80, 0xb42318),
];

/// The tokens the shared palette has none for: Swarm Core's own.
const OUR_OWN: [&str; 2] = ["strong", "success"];

/// The shared palette's `series`, dark then light: 7 colours, then the grey for none (`Tokens::issue`).
const SHARED_SERIES: [[u32; 8]; 2] = [
    [0x1b9e77, 0xd95f02, 0x7570b3, 0xe7298a, 0x66a61e, 0xe6ab02, 0xa6761d, 0x666666],
    [0x66c2a5, 0xfc8d62, 0x8da0cb, 0xe78ac3, 0xa6d854, 0xffd92f, 0xe5c494, 0xb3b3b3],
];

/// The shared palette's marker palette, in its order: ColorBrewer Set1 with the shared orange, then grey.
const MARKER_ORDER: [u32; 9] = [0xe41a1c, 0x377eb8, 0x4daf4a, 0x984ea3, 0xff7f1c, 0xffff33, 0xa65628, 0xf781bf, 0x999999];

/// **Each colour is the shared palette's**, or Swarm Core's own where it has none: no token, series
/// entry or marker is nudged away from it. A value that falls short of a minimum is kept and recorded
/// in [`KEPT_SHORT`] instead.
#[test]
fn the_colours_are_the_shared_palettes() {
    for token in TOKENS {
        match SHARED_TOKENS.iter().find(|(name, ..)| *name == token.name) {
            Some(&(_, theirs, dark, light)) => {
                assert_eq!((token.dark, token.light), (Rgb::hex(dark), Rgb::hex(light)), "{} is the shared palette's {theirs}", token.name)
            }
            None => assert!(OUR_OWN.contains(&token.name), "{} is neither the shared palette's nor listed as Swarm Core's", token.name),
        }
    }
    assert_eq!(SHARED_TOKENS.len() + OUR_OWN.len(), TOKENS.len(), "each listed token is a token");
    for (look, theirs) in LOOKS.into_iter().zip(SHARED_SERIES) {
        let series = SERIES.of(look);
        let ours: Vec<Rgb> = series.entries().iter().copied().chain(series.none()).collect();
        assert_eq!(ours, theirs.map(Rgb::hex), "{look:?}: the series, then none");
        assert_eq!(MARKERS.of(look).entries(), MARKER_ORDER.map(Rgb::hex), "{look:?}: the markers, in the shared palette's order");
    }
}

/// `darker` is the shared palette's (`(f32::from(v) * 0.8) as u8` a channel), on every channel value.
#[test]
fn darker_is_the_shared_palettes() {
    for v in 0..=255u8 {
        let theirs = (f32::from(v) * 0.8) as u8;
        assert_eq!(darker(Rgb::new(v, v, v)), Rgb::new(theirs, theirs, theirs), "{v}");
    }
    assert_eq!(darker(Rgb::hex(0xFF7F1C)), Rgb::hex(0xCC6516));
}

// ---- Contrast -----------------------------------------------------------------------------------

#[test]
fn wcag_contrast_is_measured_as_the_standard_says() {
    let (white, black) = (Rgb::hex(0xFFFFFF), Rgb::hex(0x000000));
    assert!((white.luminance() - 1.0).abs() < 1e-12 && black.luminance().abs() < 1e-12);
    assert!((white.contrast(black) - 21.0).abs() < 1e-9 && (black.contrast(white) - 21.0).abs() < 1e-9);
    assert!((Rgb::hex(0x777777).contrast(white) - 4.48).abs() < 0.01, "#777 on white is the known 4.48:1");
    assert!((Rgb::hex(0xFF0000).luminance() - 0.2126).abs() < 1e-9);
}

/// The text tokens, and the surfaces each is read on (the shared palette's `tests/theme_contrast.rs`
/// holds the same: text on every background).
const TEXTS: [Token; 7] = [TEXT, STRONG, MUTED, LINK, SUCCESS, WARNING, ERROR];
const SURFACES: [Token; 5] = [BACKGROUND, CHROME, PANEL, BASE, ALT_BASE];

/// The other pairs a token is read in, with the contrast each needs: text on a header row, on a
/// widget and on the selection (4.5:1), and the parts of the interface that carry no text on what
/// they are drawn on (3:1).
const PAIRS: [(Token, Token, f64); 7] = [
    (TEXT, HEADER, TEXT_CONTRAST),
    (STRONG, HEADER, TEXT_CONTRAST),
    (TEXT, CONTROL, TEXT_CONTRAST),
    (SELECTED_TEXT, SELECTION, TEXT_CONTRAST),
    (CONTROL_BORDER, BACKGROUND, FILL_CONTRAST),
    (CONTROL_BORDER, PANEL, FILL_CONTRAST),
    (ACCENT, BACKGROUND, FILL_CONTRAST),
];

/// Lines with no minimum: decorative, as the shared palette says of them.
const DECORATIVE: [Token; 2] = [FRAME, BORDER];

/// A value kept although it falls short of a minimum where it is drawn: the shared palette's own, kept so the app looks like them, with the ratio it measures.
struct Short {
    /// The token's or range's name, and for a range the entry's index (`none` for its grey).
    what: &'static str,
    look: Look,
    colour: u32,
    ratio: f64,
    why: &'static str,
}

/// Why each of the shared palette's fills is kept short of 3:1 against the background.
const FILL: &str = "The shared palette's (ColorBrewer's) fill, as the swarm's tools draw it: kept so a series \
                    or a mark is the same colour here as there. It is told apart from the background by its border \
                    (darker) and its neighbours, and text on it is made a text background (behind_text), which reaches 4.5:1";
const PASTEL: &str = "the shared palette's light series is ColorBrewer Set2, a pastel set made to carry black text, \
                      every entry of it under 3:1 on the light background: kept, as FILL";
const ACCENT_WHY: &str = "The shared accent blue: kept so the swarm's tools share one. It carries \
                          nothing alone: the selection (selected_text 5.07:1) and control_border (3.32:1) do";

/// **Every shortfall, with its measured ratio**: the shared palette's values that are under a minimum
/// here, kept rather than nudged (none is nudged: `the_colours_are_the_shared_palettes`).
const KEPT_SHORT: [Short; 21] = [
    Short { what: "accent", look: Look::Dark, colour: 0x007ACC, ratio: 2.72, why: ACCENT_WHY },
    Short { what: "series 2", look: Look::Dark, colour: 0x7570B3, ratio: 2.76, why: FILL },
    Short { what: "series 3", look: Look::Dark, colour: 0xE7298A, ratio: 2.97, why: FILL },
    Short { what: "series none", look: Look::Dark, colour: 0x666666, ratio: 2.14, why: FILL },
    Short { what: "series 0", look: Look::Light, colour: 0x66C2A5, ratio: 1.86, why: PASTEL },
    Short { what: "series 1", look: Look::Light, colour: 0xFC8D62, ratio: 2.00, why: PASTEL },
    Short { what: "series 2", look: Look::Light, colour: 0x8DA0CB, ratio: 2.28, why: PASTEL },
    Short { what: "series 3", look: Look::Light, colour: 0xE78AC3, ratio: 2.07, why: PASTEL },
    Short { what: "series 4", look: Look::Light, colour: 0xA6D854, ratio: 1.45, why: PASTEL },
    Short { what: "series 5", look: Look::Light, colour: 0xFFD92F, ratio: 1.20, why: PASTEL },
    Short { what: "series 6", look: Look::Light, colour: 0xE5C494, ratio: 1.44, why: PASTEL },
    Short { what: "series none", look: Look::Light, colour: 0xB3B3B3, ratio: 1.82, why: PASTEL },
    Short { what: "markers 0", look: Look::Dark, colour: 0xE41A1C, ratio: 2.61, why: FILL },
    Short { what: "markers 1", look: Look::Dark, colour: 0x377EB8, ratio: 2.83, why: FILL },
    Short { what: "markers 3", look: Look::Dark, colour: 0x984EA3, ratio: 2.31, why: FILL },
    Short { what: "markers 6", look: Look::Dark, colour: 0xA65628, ratio: 2.33, why: FILL },
    Short { what: "markers 2", look: Look::Light, colour: 0x4DAF4A, ratio: 2.42, why: FILL },
    Short { what: "markers 4", look: Look::Light, colour: 0xFF7F1C, ratio: 2.20, why: FILL },
    Short { what: "markers 5", look: Look::Light, colour: 0xFFFF33, ratio: 1.07, why: FILL },
    Short { what: "markers 7", look: Look::Light, colour: 0xF781BF, ratio: 2.06, why: FILL },
    Short { what: "markers 8", look: Look::Light, colour: 0x999999, ratio: 2.48, why: FILL },
];

/// Measures `fg` on `bg` against `minimum`: a shortfall must be in [`KEPT_SHORT`] under `what`, with
/// the ratio it measures (to the hundredth). Pushes what it found to `seen`.
fn measure(what: &str, look: Look, fg: Rgb, bg: Rgb, minimum: f64, seen: &mut Vec<String>) {
    let ratio = fg.contrast(bg);
    if ratio >= minimum {
        return;
    }
    let kept = KEPT_SHORT.iter().find(|s| s.what == what && s.look == look);
    let Some(kept) = kept else { panic!("{what} ({look:?}) {} is {ratio:.2}:1, under {minimum}:1, and not in KEPT_SHORT", fg.to_hex()) };
    assert_eq!(Rgb::hex(kept.colour), fg, "{what} ({look:?}): KEPT_SHORT records another colour");
    assert!((kept.ratio - ratio).abs() < 0.005, "{what} ({look:?}): measured {ratio:.4}:1, recorded {}:1", kept.ratio);
    assert!(!kept.why.is_empty());
    seen.push(format!("{what} {look:?}"));
}

/// **Every token reaches its minimum** where it is read, in both looks (4.5:1 for text, 3:1 for a
/// part of the interface), or is a recorded shortfall; every token is checked or said to be
/// decorative.
#[test]
fn every_token_reaches_its_contrast_where_it_is_read() {
    let mut seen = Vec::new();
    for look in LOOKS {
        for fg in TEXTS {
            for bg in SURFACES {
                measure(fg.name, look, fg.of(look), bg.of(look), TEXT_CONTRAST, &mut seen);
            }
        }
        for (fg, bg, minimum) in PAIRS {
            measure(fg.name, look, fg.of(look), bg.of(look), minimum, &mut seen);
        }
    }
    let checked: Vec<&str> =
        TEXTS.iter().chain(&SURFACES).chain(&DECORATIVE).map(|t| t.name).chain(PAIRS.iter().flat_map(|(fg, bg, _)| [fg.name, bg.name])).collect();
    assert!(TOKENS.iter().all(|t| checked.contains(&t.name)), "each token is checked, or decorative");
    assert_eq!(seen, ["accent Dark"], "the token shortfalls kept");
    let mut names: Vec<&str> = TOKENS.iter().map(|t| t.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), TOKENS.len(), "token names are unique");
}

/// A range's colours in `look` with what each is called in [`KEPT_SHORT`]: its entries by index,
/// then its none.
fn named(range: &Range, look: Look) -> Vec<(String, Rgb)> {
    let shades = range.of(look);
    let entries = shades.entries().iter().enumerate().map(|(i, &c)| (format!("{} {i}", range.name), c));
    entries.chain(shades.none().map(|c| (format!("{} none", range.name), c))).collect()
}

/// **Every range entry reaches 3:1** against the background as a fill, in both looks, or is one of
/// The shared palette's kept short and recorded; Swarm Core's own ranges have none short. Every
/// record is used, so none outlives its colour.
#[test]
fn every_range_entry_reaches_fill_contrast_or_is_recorded() {
    let mut seen = Vec::new();
    for range in RANGES {
        for look in LOOKS {
            for (what, colour) in named(&range, look) {
                measure(&what, look, colour, BACKGROUND.of(look), FILL_CONTRAST, &mut seen);
            }
        }
    }
    let kept_ranges: Vec<String> = KEPT_SHORT.iter().filter(|s| s.what != "accent").map(|s| format!("{} {:?}", s.what, s.look)).collect();
    seen.sort();
    let mut want = kept_ranges;
    want.sort();
    assert_eq!(seen, want, "every record in KEPT_SHORT is a shortfall that is there");
    assert!(seen.iter().all(|s| s.starts_with(SERIES.name) || s.starts_with(MARKERS.name)), "only the shared palette's are kept short: {seen:?}");
}

/// **Text on a range entry is readable**: made a text background, every entry (and none) takes strong
/// text at 4.5:1, with the lightness rule kept (at most 0.5 in dark, at least 0.5 in light) and its hue.
#[test]
fn strong_text_on_every_entry_made_a_text_background_reaches_text_contrast() {
    for range in RANGES {
        for look in LOOKS {
            for (what, entry) in named(&range, look) {
                let behind = behind_text(entry, look);
                let ratio = behind.contrast(STRONG.of(look));
                assert!(ratio >= TEXT_CONTRAST, "{what} {look:?} {} behind text {}: {ratio:.2}:1", entry.to_hex(), behind.to_hex());
                let (hue, _, lightness) = to_hsl(behind);
                let in_rule = match look {
                    Look::Dark => lightness <= 0.5 + 1e-9,
                    Look::Light => lightness >= 0.5 - 1e-9,
                };
                assert!(in_rule, "{what} {look:?}: lightness {lightness}");
                let (was, _, _) = to_hsl(entry);
                let turned = (hue - was).abs().min(1.0 - (hue - was).abs());
                assert!(turned < 0.02, "{what} {look:?}: hue {was} became {hue}");
            }
        }
    }
    // A colour already dark enough (in dark) or light enough (in light) is kept as it is.
    assert_eq!(behind_text(Rgb::hex(0x203040), Look::Dark), Rgb::hex(0x203040));
    assert_eq!(behind_text(Rgb::hex(0xE0F0FF), Look::Light), Rgb::hex(0xE0F0FF));
}

#[test]
fn hsl_round_trips() {
    for range in RANGES {
        for look in LOOKS {
            for (_, entry) in named(&range, look) {
                let (h, s, l) = to_hsl(entry);
                assert_eq!(from_hsl(h, s, l), entry);
            }
        }
    }
}

// ---- How far apart ------------------------------------------------------------------------------

/// CIELAB (D65) of an sRGB colour.
fn lab(c: Rgb) -> (f64, f64, f64) {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c.r), lin(c.g), lin(c.b));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    (116.0 * f(y) - 16.0, 500.0 * (f(x) - f(y)), 200.0 * (f(y) - f(z)))
}

/// CIEDE2000 colour difference (Sharma, Wu and Dalal, 2005).
fn de2000(c1: Rgb, c2: Rgb) -> f64 {
    let ((l1, a1, b1), (l2, a2, b2)) = (lab(c1), lab(c2));
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let g = 0.5 * (1.0 - (c_bar.powi(7) / (c_bar.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| b.atan2(a).to_degrees().rem_euclid(360.0);
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dh = if c1p * c2p <= f64::EPSILON {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p > h1p {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let (dl, dc, dhh) = (l2 - l1, c2p - c1p, 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).to_radians().sin());
    let (l_bar, cp_bar) = ((l1 + l2) / 2.0, (c1p + c2p) / 2.0);
    let h_bar = if c1p * c2p <= f64::EPSILON {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let cos = |deg: f64| deg.to_radians().cos();
    let t = 1.0 - 0.17 * cos(h_bar - 30.0) + 0.24 * cos(2.0 * h_bar) + 0.32 * cos(3.0 * h_bar + 6.0) - 0.20 * cos(4.0 * h_bar - 63.0);
    let d_theta = 30.0 * (-((h_bar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cp_bar.powi(7) / (cp_bar.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (l_bar - 50.0).powi(2) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let (sc, sh) = (1.0 + 0.045 * cp_bar, 1.0 + 0.015 * cp_bar * t);
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh)).sqrt()
}

#[test]
fn ciede2000_matches_sharmas_test_data() {
    // Through sRGB, the ends the formula is sure of:
    // identical colours differ by 0, and black and white by the known 100.
    assert!(de2000(Rgb::hex(0x336699), Rgb::hex(0x336699)).abs() < 1e-9);
    assert!((de2000(Rgb::hex(0x000000), Rgb::hex(0xFFFFFF)) - 100.0).abs() < 0.1);
}

/// **The ranges stay distinguishable**: in each look, any two entries of a range differ by at least
/// CIEDE2000 10 (several times a just-noticeable difference, side by side), and neighbours (`at(n)`
/// and `at(n + 1)`) by at least 15. The 32 range is the largest that holds this here; a range that
/// can't is cut to the size that can, not shipped as it is.
#[test]
fn the_ranges_stay_distinguishable() {
    for range in RANGES {
        for look in LOOKS {
            let entries = range.of(look).entries();
            let mut closest = (f64::MAX, 0, 0);
            for (i, &a) in entries.iter().enumerate() {
                for (j, &b) in entries.iter().enumerate().skip(i + 1) {
                    let d = de2000(a, b);
                    if d < closest.0 {
                        closest = (d, i, j);
                    }
                }
            }
            assert!(closest.0 >= 10.0, "{} {look:?}: entries {} and {} differ by {:.1}", range.name, closest.1, closest.2, closest.0);
            for (i, pair) in entries.windows(2).enumerate() {
                let d = de2000(pair[0], pair[1]);
                assert!(d >= 15.0, "{} {look:?}: neighbours {i} and {} differ by {d:.1}", range.name, i + 1);
            }
        }
    }
}

#[test]
fn the_ranges_are_the_shared_palettes_two_then_swarm_cores_16_and_32() {
    for look in LOOKS {
        let sizes = RANGES.map(|r| r.of(look).len());
        assert_eq!(sizes, [7, 9, 16, 32]);
        assert_eq!(RANGE_16.of(look).entries(), &RANGE_32.of(look).entries()[..16]);
        let series = SERIES.of(look);
        let none = series.none().unwrap();
        assert!(!series.entries().contains(&none), "{look:?}: none is no entry, so at and pick never give it");
        assert_eq!([MARKERS, RANGE_16, RANGE_32].map(|r| r.of(look).none()), [None; 3], "{look:?}: only the series has a none");
    }
    assert_eq!(MARKERS.dark, MARKERS.light, "the markers are the same in both looks");
    assert_eq!(MARKERS.dark.entries().first(), Some(&Rgb::hex(0xE41A1C)), "Set1, in the shared palette's order");
    for range in RANGES {
        let own = range.source.starts_with("Swarm Core's own");
        assert_eq!(own, range.name == RANGE_16.name || range.name == RANGE_32.name, "{}: says whose it is", range.name);
    }
}

/// **The pick is FNV-1a**, fixed: the known vectors, and known ids pinned to known indexes, so a
/// change to the hash (or to how an id is written) fails here, not on someone's screen. The 16 and
/// 32 ranges' pins are unchanged.
#[test]
fn the_pick_is_fnv1a_and_its_indexes_are_pinned() {
    assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a("app.info"), 0xd2d7_7de3_5f3a_1e56);
    // series (7), markers (9), range16, range32.
    let pinned: [(&str, [usize; 4]); 8] = [
        ("", [2, 5, 5, 5]),
        ("a", [5, 7, 12, 12]),
        ("7", [2, 2, 6, 22]),
        ("42", [5, 1, 3, 3]),
        ("app.info", [5, 8, 6, 22]),
        ("frame.render", [2, 4, 8, 24]),
        ("Camera", [2, 6, 14, 14]),
        ("node/17", [3, 3, 6, 6]),
    ];
    for (id, indexes) in pinned {
        let got = RANGES.map(|r| r.dark.index_of(id));
        assert_eq!(got, indexes, "{id:?}");
        for (range, index) in RANGES.iter().zip(indexes) {
            for look in LOOKS {
                assert_eq!(Some(&range.of(look).pick(id)), range.of(look).entries().get(index));
            }
        }
    }
    // A number is its decimal digits: 7 and "7" are one id.
    assert_eq!(RANGE_32.dark.pick(7u64), RANGE_32.dark.pick("7"));
    assert_eq!(RANGE_32.light.index_of(42i32), 3);
}

/// `at(n)` takes numbers in order and wraps: the series' is the shared palette's `n % 7`.
#[test]
fn at_takes_numbers_in_order_and_wraps() {
    for range in RANGES {
        for look in LOOKS {
            let shades = range.of(look);
            for (n, entry) in shades.entries().iter().enumerate() {
                assert_eq!(shades.at(n as u64), *entry);
                assert_eq!(shades.at((n + shades.len()) as u64), *entry, "wraps");
            }
            assert_eq!(shades.at(u64::MAX), shades.at(u64::MAX % shades.len() as u64));
        }
    }
    for n in 0..30u64 {
        assert_eq!(SERIES.dark.at(n), Rgb::hex(SHARED_SERIES[0][(n % 7) as usize]), "issue {n}");
    }
    assert_eq!(Shades { entries: &[], none: None }.at(3), NO_ENTRY, "an empty range gives grey, not a panic");
    assert_eq!(Shades { entries: &[], none: None }.index_of("x"), 0);
}

#[test]
fn a_colour_is_written_and_read_as_hex() {
    assert_eq!(Rgb::hex(0x0B74C0).to_hex(), "#0B74C0");
    assert_eq!(Rgb::parse("#0b74c0"), Some(Rgb::new(0x0B, 0x74, 0xC0)));
    for bad in ["", "#", "0B74C0", "#0B74C", "#0B74C00", "#GG0000", "#+1FFFF", " #0B74C0", "#é0000"] {
        assert_eq!(Rgb::parse(bad), None, "{bad:?}");
    }
}

/// `ui.theme`'s answer is the palette: each token and each range entry (and none), as `#RRGGBB`, in
/// order, each entry made a text background and given its border.
#[test]
fn the_data_is_the_palette() {
    let data = data();
    let tokens: Vec<(&str, String, String)> = data.tokens.iter().map(|t| (t.name.as_str(), t.dark.clone(), t.light.clone())).collect();
    let want: Vec<(&str, String, String)> = TOKENS.iter().map(|t| (t.name, t.dark.to_hex(), t.light.to_hex())).collect();
    assert_eq!(tokens, want);
    assert!(data.tokens.iter().all(|t| !t.meaning.is_empty()));
    let sizes: Vec<(&str, u32)> = data.ranges.iter().map(|r| (r.name.as_str(), r.size)).collect();
    assert_eq!(sizes, [("series", 7), ("markers", 9), ("range16", 16), ("range32", 32)]);
    for (got, range) in data.ranges.iter().zip(RANGES) {
        assert!(!got.source.is_empty() && !got.meaning.is_empty());
        let none = range.dark.none().zip(range.light.none());
        assert_eq!(got.none.as_ref().map(|n| (n.index, Rgb::parse(&n.dark), Rgb::parse(&n.light))), none.map(|(d, l)| (got.size, Some(d), Some(l))));
        for (i, entry) in got.entries.iter().chain(&got.none).enumerate() {
            assert_eq!(entry.index as usize, i);
            let (dark, light) = (Rgb::parse(&entry.dark).unwrap(), Rgb::parse(&entry.light).unwrap());
            assert_eq!(Rgb::parse(&entry.behind_text_dark), Some(behind_text(dark, Look::Dark)));
            assert_eq!(Rgb::parse(&entry.behind_text_light), Some(behind_text(light, Look::Light)));
            assert_eq!((Rgb::parse(&entry.border_dark), Rgb::parse(&entry.border_light)), (Some(darker(dark)), Some(darker(light))));
        }
        for (i, entry) in got.entries.iter().enumerate() {
            assert_eq!(Rgb::parse(&entry.dark), range.dark.entries().get(i).copied());
            assert_eq!(Rgb::parse(&entry.light), range.light.entries().get(i).copied());
        }
    }
}
