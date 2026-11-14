//! Icons on menus and buttons (the App UI Substrate › Icons): Font Awesome Free 6.7.2's solid face
//! ([`crate::fonts::icons`]; the font SIL OFL 1.1, the icons CC BY 4.0, whose attribution the
//! embedded font's own metadata carries).
//!
//! - **Codepoints** are the named constants here, each the icon's Font Awesome name, never a literal
//!   where it is drawn; a test checks every one ([`ALL`]) is in the embedded face.
//! - **Which command shows which icon** is [`BY_COMMAND`], by command id, so a new command needs no
//!   menu code to have one. A command without an icon draws none, in the same fixed-width column
//!   ([`cell`]), so the labels still line up.
//! - **An icon takes its widget's text colour**, so a disabled item's icon dims with it.
//! - **Accessible names stay text.** An icon is an atom built by a closure ([`atom`]), which egui
//!   leaves out of a widget's name: a widget is named by its text, as it would be with no icon.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Atom, AtomKind, Color32, FontFamily, FontId, IntoSizedResult, Response, RichText, SizedAtomKind, Ui, WidgetText, vec2};
use {{crate_prefix_ident}}engine::Command as _;
use {{crate_prefix_ident}}engine::commands::{InfoCommand, UiThemeCommand};

use crate::dock::Pane;
use crate::fonts;

/// An icon: a codepoint of Font Awesome's solid face.
pub type Icon = char;

// The codepoints, by Font Awesome name (fontawesome.com/icons/<name>?s=solid).
pub const CIRCLE_HALF_STROKE: Icon = '\u{f042}';
pub const CIRCLE_INFO: Icon = '\u{f05a}';
pub const CIRCLE_XMARK: Icon = '\u{f057}';
pub const EYE_SLASH: Icon = '\u{f070}';
pub const LIST: Icon = '\u{f03a}';
pub const PLAY: Icon = '\u{f04b}';
pub const RECTANGLE_LIST: Icon = '\u{f022}';

/// Every icon: what the test checks is in the embedded face.
pub const ALL: [Icon; 7] = [CIRCLE_HALF_STROKE, CIRCLE_INFO, CIRCLE_XMARK, EYE_SLASH, LIST, PLAY, RECTANGLE_LIST];

/// Each command's icon, by its id: its menu item and its button show it.
pub const BY_COMMAND: &[(&str, Icon)] = &[(InfoCommand::ID, CIRCLE_INFO), (UiThemeCommand::ID, CIRCLE_HALF_STROKE)];

/// The icons' size: the body's.
pub const SIZE: f32 = fonts::BODY_SIZE;

/// The width of a menu's icon column, in points: Font Awesome's widest glyphs are 1.25 em.
pub const COLUMN: f32 = 16.0;

/// The icon of the command `id`, if it has one.
pub fn for_command(id: &str) -> Option<Icon> {
    BY_COMMAND.iter().find(|(command, _)| *command == id).map(|&(_, icon)| icon)
}

/// Each dock pane's icon, for its tab (the App UI Substrate › Icons).
pub const fn for_pane(pane: Pane) -> Icon {
    match pane {
        Pane::Commands => LIST,
        Pane::View => RECTANGLE_LIST,
    }
}

/// `text` after `icon`, as one line in the colour it is drawn in: a dock tab's title, which is text
/// with its icon in it, so whatever draws it names it by its title.
pub fn titled(icon: Icon, text: &str) -> WidgetText {
    let format =
        |family: FontFamily| TextFormat { font_id: FontId::new(SIZE, family), color: Color32::PLACEHOLDER, valign: Align::Center, ..Default::default() };
    let mut job = LayoutJob::default();
    job.append(&icon.to_string(), 0.0, format(fonts::icons()));
    job.append(text, crate::density::spacing().icon_spacing, format(FontFamily::Proportional));
    job.into()
}

/// An icon as text in the icons' family, in the widget's own colour.
pub fn glyph(icon: Icon) -> RichText {
    RichText::new(icon.to_string()).family(fonts::icons()).size(SIZE)
}

/// An icon as a widget's atom: laid out and painted as the widget's text is (in its colour), but
/// built by a closure, which egui leaves out of the widget's accessible name.
pub fn atom(icon: Icon) -> Atom<'static> {
    Atom::from(AtomKind::closure(move |ui, args| {
        let galley = WidgetText::from(glyph(icon)).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, args.fallback_font);
        IntoSizedResult { intrinsic_size: galley.size(), sized: SizedAtomKind::Text(galley) }
    }))
}

/// A menu's icon column: `icon` in [`COLUMN`], or the column empty, so the labels line up.
pub fn cell(icon: Option<Icon>) -> Atom<'static> {
    let size = Some(vec2(COLUMN, SIZE));
    match icon {
        Some(icon) => Atom { size, ..atom(icon) },
        None => Atom { size, ..Atom::default() },
    }
}

/// A button with `icon` before `text`.
pub fn text_button(ui: &mut Ui, icon: Icon, text: &str) -> Response {
    ui.add(egui::Button::new((atom(icon), text)))
}

/// A small button (as `ui.small_button`) with `icon` before `text`.
pub fn small_button(ui: &mut Ui, icon: Icon, text: &str) -> Response {
    ui.add(egui::Button::new((atom(icon), text)).small())
}

#[cfg(test)]
mod tests;
