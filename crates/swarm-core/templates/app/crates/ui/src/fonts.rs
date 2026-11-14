//! The faces and text styles (the App UI Substrate › Type): Roboto for text, Cascadia Mono for code
//! and Font Awesome Free's solid face for icons, on a 12 pt body. The files are embedded from
//! `assets/fonts/` (each with its licence and upstream release in that folder's README), so every
//! build draws the same with no network. egui's own fonts, when the app has them, stay behind ours
//! as fallbacks for the symbols and emoji Roboto lacks.
//!
//! **Wait for the fonts to bind.** egui binds the definitions [`crate::theme::apply`] installs at the
//! start of the next pass, and panics on a named family it doesn't know: nothing draws in
//! [`light`], [`medium`], [`bold`] or [`icons`] until [`bound`] is true.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontId, TextStyle};

/// The faces' names, as keys of [`FontDefinitions::font_data`].
pub const ROBOTO_REGULAR: &str = "Roboto-Regular";
pub const ROBOTO_LIGHT: &str = "Roboto-Light";
pub const ROBOTO_MEDIUM: &str = "Roboto-Medium";
pub const ROBOTO_BOLD: &str = "Roboto-Bold";
pub const CASCADIA_MONO: &str = "CascadiaMono-Regular";
/// Font Awesome Free 6.7.2, solid (the font SIL OFL 1.1, the icons CC BY 4.0).
pub const FONT_AWESOME: &str = "FontAwesome6Free-Solid";

/// Every embedded face: its name and its file.
pub const FACES: [(&str, &[u8]); 6] = [
    (ROBOTO_REGULAR, include_bytes!("../../../assets/fonts/Roboto-Regular.ttf")),
    (ROBOTO_LIGHT, include_bytes!("../../../assets/fonts/Roboto-Light.ttf")),
    (ROBOTO_MEDIUM, include_bytes!("../../../assets/fonts/Roboto-Medium.ttf")),
    (ROBOTO_BOLD, include_bytes!("../../../assets/fonts/Roboto-Bold.ttf")),
    (CASCADIA_MONO, include_bytes!("../../../assets/fonts/CascadiaMono-Regular.ttf")),
    (FONT_AWESOME, include_bytes!("../../../assets/fonts/fa-solid-900.ttf")),
];

/// The named families, for the weights egui can't make itself (it has no synthetic bold), and the
/// icons' family: Font Awesome alone, so an icon is never drawn from another face, nor text from it.
const LIGHT: &str = "light";
const MEDIUM: &str = "medium";
const BOLD: &str = "bold";
const ICONS: &str = "icons";

/// The body's size, and a button's and code's, in points.
pub const BODY_SIZE: f32 = 12.0;
/// Small and subtle text: × 0.9, in Roboto Light.
pub const SMALL_SIZE: f32 = 11.0;
/// Section titles ([`h2`]): × 1.5, in Roboto Medium.
pub const H2_SIZE: f32 = 18.0;
/// Headings: × 1.75, in Roboto Medium.
pub const HEADING_SIZE: f32 = 21.0;
/// A start page's title ([`callout`]): × 3, in Roboto Medium.
pub const CALLOUT_SIZE: f32 = 36.0;

/// The [`TextStyle::Name`]s of section titles and a start page's title.
const H2: &str = "h2";
const CALLOUT: &str = "callout";

/// Roboto Light: small and subtle text.
pub fn light() -> FontFamily {
    FontFamily::Name(LIGHT.into())
}

/// Roboto Medium: headings.
pub fn medium() -> FontFamily {
    FontFamily::Name(MEDIUM.into())
}

/// Roboto Bold: emphasis.
pub fn bold() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

/// Font Awesome's solid face, alone: icons only.
pub fn icons() -> FontFamily {
    FontFamily::Name(ICONS.into())
}

/// Section titles.
pub fn h2() -> TextStyle {
    TextStyle::Name(H2.into())
}

/// A start page's title.
pub fn callout() -> TextStyle {
    TextStyle::Name(CALLOUT.into())
}

/// Every named family [`definitions`] adds.
pub fn named_families() -> [FontFamily; 4] {
    [light(), medium(), bold(), icons()]
}

/// Whether `ctx` has bound every named family yet: draw in one only once this is true.
pub fn bound(ctx: &egui::Context) -> bool {
    ctx.fonts(|f| named_families().iter().all(|family| f.definitions().families.contains_key(family)))
}

/// egui's fonts with ours in front: Roboto for the proportional families, Cascadia Mono for
/// monospace, and Font Awesome alone for the icons.
pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (name, data) in FACES {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(data)));
    }
    let text = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let code = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let first = |face: &str, behind: &[String]| std::iter::once(face.to_owned()).chain(behind.iter().cloned()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, first(ROBOTO_REGULAR, &text));
    fonts.families.insert(light(), first(ROBOTO_LIGHT, &text));
    fonts.families.insert(medium(), first(ROBOTO_MEDIUM, &text));
    fonts.families.insert(bold(), first(ROBOTO_BOLD, &text));
    fonts.families.insert(FontFamily::Monospace, first(CASCADIA_MONO, &code));
    fonts.families.insert(icons(), vec![FONT_AWESOME.to_owned()]);
    fonts
}

/// The text styles, on a 12 pt body.
pub fn text_styles() -> BTreeMap<TextStyle, FontId> {
    [
        (TextStyle::Small, FontId::new(SMALL_SIZE, light())),
        (TextStyle::Body, FontId::new(BODY_SIZE, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(BODY_SIZE, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(BODY_SIZE, FontFamily::Monospace)),
        (h2(), FontId::new(H2_SIZE, medium())),
        (TextStyle::Heading, FontId::new(HEADING_SIZE, medium())),
        (callout(), FontId::new(CALLOUT_SIZE, medium())),
    ]
    .into()
}

#[cfg(test)]
mod tests;
