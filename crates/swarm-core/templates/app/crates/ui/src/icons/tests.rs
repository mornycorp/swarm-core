use super::*;
use egui::IntoAtoms as _;

/// Every named icon is in the embedded Font Awesome face, by its own character map (in a family of
/// one face every character resolves to that face, so egui's `has_glyph` can't say).
#[test]
fn every_icon_is_in_the_embedded_face() {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    let has = |c: char| ctx.fonts_mut(|f| f.characters(&fonts::icons()).contains_key(&c));
    let missing: Vec<String> = ALL.iter().filter(|&&icon| !has(icon)).map(|icon| format!("U+{:04X}", u32::from(*icon))).collect();
    assert!(missing.is_empty(), "not in fa-solid-900.ttf: {missing:?}");
    assert!(!has('\u{1e9e}'), "a character it lacks is not found");
}

/// A command's icon is chosen by its id, and one with none has none.
#[test]
fn icons_are_chosen_by_command_id() {
    assert_eq!(for_command(InfoCommand::ID), Some(CIRCLE_INFO));
    assert_eq!(for_command(UiThemeCommand::ID), Some(CIRCLE_HALF_STROKE));
    assert_eq!(for_command("app.nothing"), None);
    for (id, icon) in BY_COMMAND {
        assert!(ALL.contains(icon), "{id}'s icon is named");
    }
}

/// A widget with an icon is named by its text alone: the icon's atom is left out of the name.
#[test]
fn an_icon_is_left_out_of_a_widgets_name() {
    assert_eq!((atom(PLAY), "Run").into_atoms().text().as_deref(), Some("Run"));
    assert_eq!((cell(Some(CIRCLE_INFO)), "App info").into_atoms().text().as_deref(), Some("App info"));
    assert_eq!((cell(None), "App info").into_atoms().text().as_deref(), Some("App info"));
}
