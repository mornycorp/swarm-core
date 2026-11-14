//! The theme: every colour the shell draws with is a token here, named for what it means, or an
//! entry of a categorical range, never a colour made where it is drawn (docs/app-architecture.md ›
//! The UI is a client). A dark or light look, a contrast fix or a colour-blind palette then changes
//! the theme and nothing else, and a test can ask which colour a meaning has. `scan` holds the rest
//! of the crate to it (`ui-colours`: a `Color32::…` outside this module is a finding).
//!
//! The values are the engine's ([`palette`]: `ui.theme` gives the same to every surface): the palette the swarm's tools share, in a dark and a light look. This module makes them egui's: the
//! look egui draws in ([`apply`] sets egui's visuals from the tokens, so the window is drawn in the
//! theme's colours), the theme of that look ([`Theme::of`]), and each colour, the one place a colour
//! is made. A colour of the app's own (a brand, a status) is a token there too.
//!
//! **An identifier's colour comes from a range**: `theme.range16.pick(id)` (or `series.at(n)` for a
//! number in order, `markers.at(n)` for a marker) gives the same colour for the same id on every
//! run, surface and machine, [`Theme::behind_text`] makes it a background that text stays readable
//! on, and [`Theme::border`] gives the border a filled entry is outlined in.

use std::fmt::Display;

use egui::{Color32, CornerRadius, Stroke, Visuals};
use {{crate_prefix_ident}}engine::theme::{self as palette, Look, Rgb, Shades};

/// The colours the shell draws with, by what they mean, and the ranges for identifiers, in one look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// The look these are: dark or light.
    pub look: Look,
    /// What everything is drawn on.
    pub background: Color32,
    /// The menu bar, the status bar and other chrome.
    pub chrome: Color32,
    /// Floating windows, dialogs and popups.
    pub panel: Color32,
    /// Views: lists, tables and text fields.
    pub base: Color32,
    /// Every other row of a list drawn on `base`.
    pub alt_base: Color32,
    /// Table header rows and a ruler.
    pub header: Color32,
    /// Lines that frame views and panels.
    pub frame: Color32,
    /// Decorative lines: window frames and separators.
    pub border: Color32,
    /// Body text.
    pub text: Color32,
    /// Emphasised text, and text on a range colour made a text background.
    pub strong: Color32,
    /// Text that matters less: a command the UI can't run here.
    pub muted: Color32,
    /// Text on the selection.
    pub selected_text: Color32,
    /// A link.
    pub link: Color32,
    /// The fill of a widget at rest.
    pub control: Color32,
    /// The edge of a widget at rest.
    pub control_border: Color32,
    /// An accent that carries no text: what has focus, an outline.
    pub accent: Color32,
    /// What is selected: its fill, with `selected_text` on it.
    pub selection: Color32,
    /// Something that worked.
    pub success: Color32,
    /// A warning: something to look at that hasn't failed.
    pub warning: Color32,
    /// A failure, or why a command can't run now.
    pub error: Color32,
    /// Numbered things in order (`at(n)`: entry `n % 7`), and the grey for none.
    pub series: Colours,
    /// Up to 9 marks, by number, the same in both looks.
    pub markers: Colours,
    /// Up to 16 identifiers, by a stable pick of their id (Swarm Core's own range).
    pub range16: Colours,
    /// Up to 32 identifiers side by side (Swarm Core's own range).
    pub range32: Colours,
    /// The corner radius of windows, menus and widgets, in points ([`palette::CORNER_RADIUS`]).
    pub radius: u8,
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
            chrome: token(palette::CHROME),
            panel: token(palette::PANEL),
            base: token(palette::BASE),
            alt_base: token(palette::ALT_BASE),
            header: token(palette::HEADER),
            frame: token(palette::FRAME),
            border: token(palette::BORDER),
            text: token(palette::TEXT),
            strong: token(palette::STRONG),
            muted: token(palette::MUTED),
            selected_text: token(palette::SELECTED_TEXT),
            link: token(palette::LINK),
            control: token(palette::CONTROL),
            control_border: token(palette::CONTROL_BORDER),
            accent: token(palette::ACCENT),
            selection: token(palette::SELECTION),
            success: token(palette::SUCCESS),
            warning: token(palette::WARNING),
            error: token(palette::ERROR),
            series: range(palette::SERIES),
            markers: range(palette::MARKERS),
            range16: range(palette::RANGE_16),
            range32: range(palette::RANGE_32),
            radius: palette::CORNER_RADIUS,
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
        self::colour(palette::behind_text(rgb(colour), self.look))
    }

    /// The border of `colour` (a range's) drawn filled: a bar's or a mark's edge ([`palette::darker`]).
    pub fn border(&self, colour: Color32) -> Color32 {
        self::colour(palette::darker(rgb(colour)))
    }

    /// egui's visuals in this theme: egui's own look for this one, its colours the tokens.
    pub fn visuals(&self) -> Visuals {
        let mut v = match self.look {
            Look::Dark => Visuals::dark(),
            Look::Light => Visuals::light(),
        };
        v.panel_fill = self.background;
        v.window_fill = self.panel;
        v.extreme_bg_color = self.base;
        v.code_bg_color = self.base;
        v.faint_bg_color = self.alt_base;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.hyperlink_color = self.link;
        v.selection.bg_fill = self.selection;
        v.selection.stroke = Stroke::new(1.0, self.selected_text);
        v.warn_fg_color = self.warning;
        v.error_fg_color = self.error;
        v.override_text_color = None;
        v.weak_text_color = Some(self.muted);
        let radius = CornerRadius::same(self.radius);
        v.window_corner_radius = radius;
        v.menu_corner_radius = radius;
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = radius;
        }
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.open] {
            w.fg_stroke.color = self.text;
        }
        // egui's strong text is its active widgets' text.
        v.widgets.active.fg_stroke.color = self.strong;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        // egui draws resting widgets with no edge and a fill barely off the panel.
        v.widgets.inactive.bg_fill = self.control;
        v.widgets.inactive.weak_bg_fill = self.control;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, self.control_border);
        v.widgets.open.bg_stroke = Stroke::new(1.0, self.control_border);
        v
    }
}

/// Styles `ctx` in the theme: it installs the faces ([`crate::fonts`]), and each of egui's looks gets
/// the theme's visuals for it, the text styles and the density ([`crate::density`]), and egui keeps
/// choosing between them (the
/// system's, or what the app asks for). Called once, when the window starts; the named families bind
/// on the next pass ([`crate::fonts::bound`]).
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(crate::fonts::definitions());
    for (appearance, look) in [(egui::Theme::Dark, Look::Dark), (egui::Theme::Light, Look::Light)] {
        let visuals = Theme::in_look(look).visuals();
        ctx.style_mut_of(appearance, |style| {
            style.visuals = visuals;
            style.text_styles = crate::fonts::text_styles();
            style.spacing = crate::density::spacing();
        });
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

    /// The colour of something with no number (the series' grey); `None` for a range without one.
    pub fn none(&self) -> Option<Color32> {
        self.shades.none().map(colour)
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

/// egui's colour as the engine's, its alpha dropped.
fn rgb(colour: Color32) -> Rgb {
    Rgb::new(colour.r(), colour.g(), colour.b())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(rgb(theme.series.at(9)), palette::SERIES.of(look).at(2), "the series wraps at 7");
            assert_eq!(theme.series.none().map(rgb), palette::SERIES.of(look).none());
            assert_eq!(rgb(theme.markers.at(13)), palette::MARKERS.of(look).at(4));
            assert_eq!(theme.markers.none(), None);
        }
    }

    /// **egui draws in the theme**: once applied, each of egui's looks has the tokens for its
    /// surfaces, text, selection, widgets and states, and the theme's corner radius on windows, menus
    /// and every widget state, so what the window shows is the theme's.
    #[test]
    fn egui_draws_with_the_tokens_once_applied() {
        let ctx = egui::Context::default();
        apply(&ctx);
        for (appearance, look) in [(egui::Theme::Dark, Look::Dark), (egui::Theme::Light, Look::Light)] {
            let v = ctx.style_of(appearance).visuals.clone();
            let theme = Theme::in_look(look);
            assert_eq!(Theme::of(&v), theme, "{look:?}");
            let drawn = [
                ("background", v.panel_fill, theme.background),
                ("panel", v.window_fill, theme.panel),
                ("base", v.extreme_bg_color, theme.base),
                ("alt_base", v.faint_bg_color, theme.alt_base),
                ("border", v.window_stroke.color, theme.border),
                ("text", v.text_color(), theme.text),
                ("strong", v.strong_text_color(), theme.strong),
                ("muted", v.weak_text_color(), theme.muted),
                ("link", v.hyperlink_color, theme.link),
                ("selection", v.selection.bg_fill, theme.selection),
                ("selected_text", v.selection.stroke.color, theme.selected_text),
                ("control", v.widgets.inactive.bg_fill, theme.control),
                ("control_border", v.widgets.inactive.bg_stroke.color, theme.control_border),
                ("warning", v.warn_fg_color, theme.warning),
                ("error", v.error_fg_color, theme.error),
            ];
            for (name, egui, ours) in drawn {
                assert_eq!(egui, ours, "{look:?} {name}");
            }
            let radius = CornerRadius::same(palette::CORNER_RADIUS);
            let widgets = [&v.widgets.noninteractive, &v.widgets.inactive, &v.widgets.hovered, &v.widgets.active, &v.widgets.open];
            assert_eq!((v.window_corner_radius, v.menu_corner_radius), (radius, radius), "{look:?} windows and menus");
            assert!(widgets.iter().all(|w| w.corner_radius == radius), "{look:?} every widget state");
        }
    }

    #[test]
    fn a_colour_a_command_gave_is_parsed_and_a_bad_one_shows_as_an_error() {
        let theme = Theme::in_look(Look::Light);
        assert_eq!(theme.parse("#2B78AE"), theme.accent);
        assert_eq!(theme.parse("blue"), theme.error);
        let behind = theme.behind_text(theme.series.at(5));
        assert_eq!(rgb(behind), palette::behind_text(palette::SERIES.light.at(5), Look::Light));
        assert!(rgb(behind).contrast(rgb(theme.strong)) >= palette::TEXT_CONTRAST);
        assert_eq!(rgb(theme.border(theme.markers.at(4))), Rgb::hex(0xCC6516), "the shared palette's darker #FF7F1C");
    }
}
