//! The theme's values, measured: contrast (WCAG 2), how far apart a range's entries stay
//! (CIEDE2000), and the pick's hash, pinned.

use super::*;

const LOOKS: [Look; 2] = [Look::Dark, Look::Light];

/// The pairs a token is read in, each with the contrast it must reach in both looks: text tokens
/// on the background, and strong text on the selection's fill.
const PAIRS: [(Token, Token); 8] = [
    (TEXT, BACKGROUND),
    (STRONG, BACKGROUND),
    (MUTED, BACKGROUND),
    (ACCENT, BACKGROUND),
    (SUCCESS, BACKGROUND),
    (WARNING, BACKGROUND),
    (ERROR, BACKGROUND),
    (STRONG, SELECTION),
];

#[test]
fn wcag_contrast_is_measured_as_the_standard_says() {
    let (white, black) = (Rgb::hex(0xFFFFFF), Rgb::hex(0x000000));
    assert!((white.luminance() - 1.0).abs() < 1e-12 && black.luminance().abs() < 1e-12);
    assert!((white.contrast(black) - 21.0).abs() < 1e-9 && (black.contrast(white) - 21.0).abs() < 1e-9);
    assert!((Rgb::hex(0x777777).contrast(white) - 4.48).abs() < 0.01, "#777 on white is the known 4.48:1");
    assert!((Rgb::hex(0xFF0000).luminance() - 0.2126).abs() < 1e-9);
}

/// **Every semantic token reaches 4.5:1** where it is read, in both looks; every token is checked.
#[test]
fn every_token_reaches_text_contrast_where_it_is_read() {
    let mut report = Vec::new();
    for (fg, bg) in PAIRS {
        for look in LOOKS {
            let ratio = fg.of(look).contrast(bg.of(look));
            report.push(format!("{} on {} ({look:?}): {ratio:.2}", fg.name, bg.name));
            assert!(ratio >= TEXT_CONTRAST, "{report:#?}");
        }
    }
    let checked: Vec<&str> = PAIRS.iter().flat_map(|(fg, bg)| [fg.name, bg.name]).collect();
    assert!(TOKENS.iter().all(|t| checked.contains(&t.name)), "each token is in a pair");
    let mut names: Vec<&str> = TOKENS.iter().map(|t| t.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), TOKENS.len(), "token names are unique");
}

/// **Every range entry reaches 3:1** against the background as a fill, in both looks.
#[test]
fn every_range_entry_reaches_fill_contrast() {
    for range in RANGES {
        for look in LOOKS {
            for (i, entry) in range.of(look).entries().iter().enumerate() {
                let ratio = entry.contrast(BACKGROUND.of(look));
                assert!(ratio >= FILL_CONTRAST, "{} {look:?} entry {i} {}: {ratio:.2}:1", range.name, entry.to_hex());
            }
        }
    }
}

/// **Text on a range entry is readable**: made a text background, every entry takes strong text at
/// 4.5:1, with the lightness rule kept (at most 0.5 in dark, at least 0.5 in light) and its hue.
#[test]
fn strong_text_on_every_entry_made_a_text_background_reaches_text_contrast() {
    for range in RANGES {
        for look in LOOKS {
            for (i, &entry) in range.of(look).entries().iter().enumerate() {
                let behind = behind_text(entry, look);
                let ratio = behind.contrast(STRONG.of(look));
                assert!(ratio >= TEXT_CONTRAST, "{} {look:?} entry {i} {} behind text {}: {ratio:.2}:1", range.name, entry.to_hex(), behind.to_hex());
                let (hue, _, lightness) = to_hsl(behind);
                let in_rule = match look {
                    Look::Dark => lightness <= 0.5 + 1e-9,
                    Look::Light => lightness >= 0.5 - 1e-9,
                };
                assert!(in_rule, "{} {look:?} entry {i}: lightness {lightness}", range.name);
                let (was, _, _) = to_hsl(entry);
                let turned = (hue - was).abs().min(1.0 - (hue - was).abs());
                assert!(turned < 0.02, "{} {look:?} entry {i}: hue {was} became {hue}", range.name);
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
            for &entry in range.of(look).entries() {
                let (h, s, l) = to_hsl(entry);
                assert_eq!(from_hsl(h, s, l), entry);
            }
        }
    }
}

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
fn the_ranges_are_8_16_and_32_and_16_is_the_start_of_32() {
    for look in LOOKS {
        assert_eq!([RANGE_8.of(look).len(), RANGE_16.of(look).len(), RANGE_32.of(look).len()], [8, 16, 32]);
        assert_eq!(RANGE_16.of(look).entries(), &RANGE_32.of(look).entries()[..16]);
    }
    assert_eq!(RANGE_8.dark.entries().first(), Some(&Rgb::hex(0xE41A1C)), "Set1 as published");
}

/// **The pick is FNV-1a**, fixed: the known vectors, and known ids pinned to known indexes, so a
/// change to the hash (or to how an id is written) fails here, not on someone's screen.
#[test]
fn the_pick_is_fnv1a_and_its_indexes_are_pinned() {
    assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a("app.info"), 0xd2d7_7de3_5f3a_1e56);
    let pinned: [(&str, [usize; 3]); 8] = [
        ("", [5, 5, 5]),
        ("a", [4, 12, 12]),
        ("7", [6, 6, 22]),
        ("42", [3, 3, 3]),
        ("app.info", [6, 6, 22]),
        ("frame.render", [0, 8, 24]),
        ("Camera", [6, 14, 14]),
        ("node/17", [6, 6, 6]),
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
    assert_eq!(Shades { entries: &[] }.at(3), NO_ENTRY, "an empty range gives grey, not a panic");
    assert_eq!(Shades { entries: &[] }.index_of("x"), 0);
}

#[test]
fn a_colour_is_written_and_read_as_hex() {
    assert_eq!(Rgb::hex(0x0B74C0).to_hex(), "#0B74C0");
    assert_eq!(Rgb::parse("#0b74c0"), Some(Rgb::new(0x0B, 0x74, 0xC0)));
    for bad in ["", "#", "0B74C0", "#0B74C", "#0B74C00", "#GG0000", "#+1FFFF", " #0B74C0", "#é0000"] {
        assert_eq!(Rgb::parse(bad), None, "{bad:?}");
    }
}

/// `ui.theme`'s answer is the palette: each token and each range entry, as `#RRGGBB`, in order.
#[test]
fn the_data_is_the_palette() {
    let data = data();
    let tokens: Vec<(&str, String, String)> = data.tokens.iter().map(|t| (t.name.as_str(), t.dark.clone(), t.light.clone())).collect();
    let want: Vec<(&str, String, String)> = TOKENS.iter().map(|t| (t.name, t.dark.to_hex(), t.light.to_hex())).collect();
    assert_eq!(tokens, want);
    assert!(data.tokens.iter().all(|t| !t.meaning.is_empty()));
    assert_eq!(data.ranges.iter().map(|r| (r.name.as_str(), r.size)).collect::<Vec<_>>(), [("range8", 8), ("range16", 16), ("range32", 32)]);
    for (got, range) in data.ranges.iter().zip(RANGES) {
        assert!(!got.source.is_empty() && !got.meaning.is_empty());
        for (i, entry) in got.entries.iter().enumerate() {
            assert_eq!(entry.index as usize, i);
            assert_eq!(Rgb::parse(&entry.dark), range.dark.entries().get(i).copied());
            assert_eq!(Rgb::parse(&entry.light), range.light.entries().get(i).copied());
            assert_eq!(Rgb::parse(&entry.behind_text_light), range.light.entries().get(i).map(|&c| behind_text(c, Look::Light)));
        }
    }
}
