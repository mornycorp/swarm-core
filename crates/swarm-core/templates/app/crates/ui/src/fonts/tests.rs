use super::*;

/// Each family draws from its face first: Roboto's weights for text, Cascadia Mono for code, and
/// Font Awesome alone for the icons.
#[test]
fn each_family_draws_from_its_face_first() {
    let fonts = definitions();
    for (name, _) in FACES {
        assert!(fonts.font_data.contains_key(name), "{name} is embedded");
    }
    let first = |family: FontFamily| fonts.families.get(&family).and_then(|faces| faces.first()).cloned();
    assert_eq!(first(FontFamily::Proportional).as_deref(), Some(ROBOTO_REGULAR));
    assert_eq!(first(light()).as_deref(), Some(ROBOTO_LIGHT));
    assert_eq!(first(medium()).as_deref(), Some(ROBOTO_MEDIUM));
    assert_eq!(first(bold()).as_deref(), Some(ROBOTO_BOLD));
    assert_eq!(first(FontFamily::Monospace).as_deref(), Some(CASCADIA_MONO));
    assert_eq!(fonts.families.get(&icons()), Some(&vec![FONT_AWESOME.to_owned()]), "icons come from Font Awesome alone");
}

/// The text styles are the App UI Substrate's: a 12 pt body, and the scale on it.
#[test]
fn the_text_styles_are_the_scale_on_a_12_pt_body() {
    let styles = text_styles();
    let want = [
        (TextStyle::Small, FontId::new(11.0, light())),
        (TextStyle::Body, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        (h2(), FontId::new(18.0, medium())),
        (TextStyle::Heading, FontId::new(21.0, medium())),
        (callout(), FontId::new(36.0, medium())),
    ];
    assert_eq!(styles.len(), want.len());
    for (style, font) in want {
        assert_eq!(styles.get(&style), Some(&font), "{style:?}");
    }
}

/// Once the theme is applied, the named families bind on the next pass, both looks have the text
/// styles, and a frame draws in every named family without a panic.
#[test]
fn the_fonts_bind_on_the_next_pass_and_both_looks_have_the_styles() {
    // A frame's textures go to the GPU; here they are dropped, which egui asks to be said.
    let pass = |ctx: &egui::Context, draw: &mut dyn FnMut(&mut egui::Ui)| ctx.run_ui(egui::RawInput::default(), draw).textures_delta.clear();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    pass(&ctx, &mut |_| {});
    assert!(bound(&ctx));
    for look in [egui::Theme::Dark, egui::Theme::Light] {
        assert_eq!(ctx.style_of(look).text_styles, text_styles(), "{look:?}");
    }
    pass(&ctx, &mut |ui| {
        for family in named_families() {
            ui.label(egui::RichText::new("Aa").family(family));
        }
    });
}
