//! The density the UI lays out with (the App UI Substrate › Density): the row pitch, spacing and
//! padding. Like the colours in [`crate::theme`], these are the one place the metrics live:
//! [`crate::theme::apply`] installs them as egui's [`Spacing`], and the menus and grids read them
//! from here rather than writing numbers where they draw. Values are logical points, scaled by the
//! system's display scale.

use egui::{Margin, Spacing, Style, Vec2, vec2};

/// Every row of a list, table, menu and tab bar, and the height of a button, combo box or
/// single-line field: the row for 12 pt Roboto.
pub const ROW_HEIGHT: f32 = 18.0;

/// Between neighbouring widgets: 6 across, 3 down (egui's is 8 × 3).
pub const ITEM_SPACING: Vec2 = vec2(6.0, 3.0);

/// Between a form's label column and its values.
pub const COLUMN_SPACING: f32 = 12.0;

/// Text to edge inside buttons and combo boxes: 6 across, and 2 down, which with a 12 pt body's
/// 14 pt line makes the 18 pt button (egui's is 4 × 1).
pub const BUTTON_PADDING: Vec2 = vec2(6.0, 2.0);

/// Text to edge of a menu entry. Entries touch (no item spacing down), so they sit
/// [`ROW_HEIGHT`] apart and their hover fills meet.
pub const MENU_PADDING: Vec2 = vec2(8.0, 2.0);

/// Text to edge of a menu-bar title: with [`ITEM_SPACING`] between titles, their text is 20 points
/// apart.
pub const MENU_BAR_PADDING: Vec2 = vec2(7.0, 2.0);

/// Inside a menu's frame, around its entries.
pub const MENU_MARGIN: Margin = Margin::symmetric(1, 3);

/// egui's spacing with these metrics.
pub fn spacing() -> Spacing {
    Spacing {
        item_spacing: ITEM_SPACING,
        button_padding: BUTTON_PADDING,
        interact_size: vec2(Spacing::default().interact_size.x, ROW_HEIGHT),
        menu_margin: MENU_MARGIN,
        ..Spacing::default()
    }
}

/// A form's two columns (a label, its value): [`COLUMN_SPACING`] across, the item spacing down.
pub fn form_spacing() -> Vec2 {
    vec2(COLUMN_SPACING, ITEM_SPACING.y)
}

/// The style of a menu's entries: egui's menu style (no frame on a resting entry) with
/// [`MENU_PADDING`], and no space between entries.
pub fn menu_style(style: &mut Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = MENU_PADDING;
    style.spacing.item_spacing.y = 0.0;
}

/// The style of the menu bar's titles.
pub fn menu_bar_style(style: &mut Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = MENU_BAR_PADDING;
}

#[cfg(test)]
mod tests;
