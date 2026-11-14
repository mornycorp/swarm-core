use super::*;

/// The metrics are the App UI Substrate's, and egui's own where it doesn't say.
#[test]
fn the_spacing_is_the_substrates() {
    let s = spacing();
    assert_eq!((s.item_spacing, s.button_padding), (vec2(6.0, 3.0), vec2(6.0, 2.0)));
    assert_eq!(s.interact_size, vec2(Spacing::default().interact_size.x, 18.0), "a row and a button are 18 high");
    assert_eq!(s.menu_margin, Margin::symmetric(1, 3));
    assert_eq!(s.indent, Spacing::default().indent, "the rest is egui's");
    assert_eq!(form_spacing(), vec2(12.0, 3.0));
}

/// Menu entries touch with their padding; menu-bar titles have theirs.
#[test]
fn menus_have_their_padding_and_entries_touch() {
    let mut entry = Style::default();
    menu_style(&mut entry);
    assert_eq!((entry.spacing.button_padding, entry.spacing.item_spacing.y), (vec2(8.0, 2.0), 0.0));
    let mut title = Style::default();
    menu_bar_style(&mut title);
    assert_eq!(title.spacing.button_padding, vec2(7.0, 2.0));
}

/// Once the theme is applied, both looks lay out with the density.
#[test]
fn both_looks_lay_out_with_the_density_once_applied() {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    for look in [egui::Theme::Dark, egui::Theme::Light] {
        assert_eq!(ctx.style_of(look).spacing, spacing(), "{look:?}");
    }
}
