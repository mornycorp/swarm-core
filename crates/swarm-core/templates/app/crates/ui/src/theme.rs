//! The theme: every colour the shell draws with is a token here, named for what it means, or an
//! entry of a categorical range, never a colour made where it is drawn (docs/app-architecture.md ›
//! The UI is a client). A dark or light look, a contrast fix or a colour-blind palette then changes
//! the theme and nothing else, and a test can ask which colour a meaning has. `scan` holds the rest
//! of the crate to it (`ui-colours`: a `Color32::…` outside this module is a finding).
//!
//! The values are the engine's ([`palette`]: `ui.theme` gives the same to every surface), each in
//! egui's dark and light look; this module picks the look egui draws in ([`Theme::of`]) and makes
//! them egui's colours, the one place a colour is made. A colour of the app's own (a brand, a
//! status) is a token there too.
//!
//! **An identifier's colour comes from a range**: `theme.range16.pick(id)` (or `range8.at(n)` for a
//! number in order) gives the same colour for the same id on every run, surface and machine, and
//! [`Theme::behind_text`] makes it a background that text stays readable on.

use std::fmt::Display;

use egui::{Color32, Visuals};
use {{crate_prefix_ident}}engine::theme::{self as palette, Look, Rgb, Shades};

/// The colours the shell draws with, by what they mean, and the ranges for identifiers, in one look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// The look these are: egui's dark or light.
    pub look: Look,
    /// What everything is drawn on.
    pub background: Color32,
    /// Body text.
    pub text: Color32,
    /// Emphasised text, and text on a range colour made a text background.
    pub strong: Color32,
    /// Text that matters less: a command the UI can't run here.
    pub muted: Color32,
    /// A link, or what draws the eye to an action.
    pub accent: Color32,
    /// What is selected: its fill, with `strong` text on it.
    pub selection: Color32,
    /// Something that worked.
    pub success: Color32,
    /// A warning: something to look at that hasn't failed.
    pub warning: Color32,
    /// A failure, or why a command can't run now.
    pub error: Color32,
    /// Up to 8 identifiers (a series, a marker by number).
    pub range8: Colours,
    /// Up to 16 identifiers, by a stable pick of their id.
    pub range16: Colours,
    /// Up to 32 identifiers side by side.
    pub range32: Colours,
}

impl Theme {
    /// The theme for `visuals`, egui's dark or light look.
    pub fn of(visuals: &Visuals) -> Theme {
        Theme::in_look(if visuals.dark_mode { Look::Dark } else { Look::Light })
    }

    /// The theme in `look`.
    pub fn in_look(look: Look) -> Theme {
        let token = |t: palette::Token| colour(t.of(look));
        let range = |r: palette::Range| Colours { shades: r.of(look), look };
        Theme {
            look,
            background: token(palette::BACKGROUND),
            text: token(palette::TEXT),
            strong: token(palette::STRONG),
            muted: token(palette::MUTED),
            accent: token(palette::ACCENT),
            selection: token(palette::SELECTION),
            success: token(palette::SUCCESS),
            warning: token(palette::WARNING),
            error: token(palette::ERROR),
            range8: range(palette::RANGE_8),
            range16: range(palette::RANGE_16),
            range32: range(palette::RANGE_32),
        }
    }

    /// A colour a command gave as `#RRGGBB` (`ui.theme`'s), as egui's. One that isn't a colour is
    /// drawn in the `error` token, so it shows rather than vanishes.
    pub fn parse(&self, hex: &str) -> Color32 {
        Rgb::parse(hex).map_or(self.error, colour)
    }

    /// `colour` (a range's) made a background for text in this look: `strong` text on it is
    /// readable ([`palette::behind_text`]: the HSL lightness rule, then on until 4.5:1).
    pub fn behind_text(&self, colour: Color32) -> Color32 {
        self::colour(palette::behind_text(Rgb::new(colour.r(), colour.g(), colour.b()), self.look))
    }
}

/// One look's colours of a categorical range, as egui's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colours {
    shades: Shades,
    look: Look,
}

impl Colours {
    /// The colour an identifier always gets (`id` hashed as it displays: FNV-1a, so `7` and `"7"`
    /// are one id).
    pub fn pick(&self, id: impl Display) -> Color32 {
        colour(self.shades.pick(id))
    }

    /// The colour of the number `n`, in order, wrapping at the range's size.
    pub fn at(&self, n: u64) -> Color32 {
        colour(self.shades.at(n))
    }

    /// How many colours the range has.
    pub fn len(&self) -> usize {
        self.shades.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shades.is_empty()
    }

    /// The look these are in.
    pub fn look(&self) -> Look {
        self.look
    }
}

/// `rgb` as egui's colour: the one place the shell makes one.
fn colour(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: Color32) -> Rgb {
        Rgb::new(c.r(), c.g(), c.b())
    }

    /// A colour with alpha (egui's weak text is its text colour at 60%) as it shows on `bg`: egui
    /// keeps colours premultiplied and blends in gamma space.
    fn over(c: Color32, bg: Color32) -> Rgb {
        let keep = 1.0 - f32::from(c.a()) / 255.0;
        let channel = |fg: u8, bg: u8| (f32::from(fg) + f32::from(bg) * keep).round() as u8;
        Rgb::new(channel(c.r(), bg.r()), channel(c.g(), bg.g()), channel(c.b(), bg.b()))
    }

    /// The tokens are the engine's, in the look egui draws in.
    #[test]
    fn the_tokens_follow_the_look() {
        for (visuals, look) in [(Visuals::dark(), Look::Dark), (Visuals::light(), Look::Light)] {
            let theme = Theme::of(&visuals);
            assert_eq!(theme.look, look);
            assert_eq!(rgb(theme.error), palette::ERROR.of(look));
            assert_eq!(rgb(theme.selection), palette::SELECTION.of(look));
            assert_eq!(theme.range32.len(), 32);
            assert_eq!(rgb(theme.range16.pick("frame.render")), palette::RANGE_16.of(look).pick("frame.render"));
            assert_eq!(rgb(theme.range8.at(9)), palette::RANGE_8.of(look).at(1));
        }
    }

    /// **The tokens are egui's look**, or a nudge of it that egui's own value needed: each nudged one
    /// is listed, and egui's value falls short of 4.5:1 against the background, which is why.
    #[test]
    fn the_tokens_are_egui_s_look_or_a_nudge_it_needed() {
        const NUDGED: [&str; 4] = ["muted", "accent", "warning", "error"];
        for visuals in [Visuals::dark(), Visuals::light()] {
            let theme = Theme::of(&visuals);
            let bg = visuals.panel_fill;
            let success = if visuals.dark_mode { Color32::LIGHT_GREEN } else { Color32::DARK_GREEN };
            let egui = [
                ("background", rgb(bg), theme.background),
                ("text", rgb(visuals.text_color()), theme.text),
                ("strong", rgb(visuals.strong_text_color()), theme.strong),
                ("muted", over(visuals.weak_text_color(), bg), theme.muted),
                ("accent", rgb(visuals.hyperlink_color), theme.accent),
                ("selection", rgb(visuals.selection.bg_fill), theme.selection),
                ("success", rgb(success), theme.success),
                ("warning", rgb(visuals.warn_fg_color), theme.warning),
                ("error", rgb(visuals.error_fg_color), theme.error),
            ];
            assert_eq!(egui.len(), palette::TOKENS.len(), "every token is checked");
            for (name, theirs, ours) in egui {
                let nudged = NUDGED.contains(&name) && theirs.contrast(rgb(bg)) < palette::TEXT_CONTRAST;
                assert!(rgb(ours) == theirs || nudged, "{name} (dark: {}): egui's {} is ours {}", visuals.dark_mode, theirs.to_hex(), rgb(ours).to_hex());
            }
        }
    }

    #[test]
    fn a_colour_a_command_gave_is_parsed_and_a_bad_one_shows_as_an_error() {
        let theme = Theme::in_look(Look::Light);
        assert_eq!(theme.parse("#0B74C0"), theme.accent);
        assert_eq!(theme.parse("blue"), theme.error);
        let behind = theme.behind_text(theme.range8.at(5));
        assert_eq!(rgb(behind), palette::behind_text(palette::RANGE_8.light.at(5), Look::Light));
        assert!(rgb(behind).contrast(rgb(theme.strong)) >= palette::TEXT_CONTRAST);
    }
}
