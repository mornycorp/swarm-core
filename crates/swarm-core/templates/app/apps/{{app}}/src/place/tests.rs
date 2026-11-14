use super::*;

/// A 4K primary monitor at 150%.
const PRIMARY: Monitor = Monitor { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(3840.0, 2160.0)), scale: 1.5, primary: true };
/// A 1080p second monitor at 100%, to the primary's right.
const SECOND: Monitor = Monitor { rect: Rect::from_min_max(pos2(3840.0, 0.0), pos2(5760.0, 1080.0)), scale: 1.0, primary: false };

fn saved(outer: [f32; 2], inner: [f32; 4]) -> Saved {
    Saved { version: VERSION, outer, inner, zoom: 1.0, maximized: false }
}

/// The corner this platform's winit places, of a window saved with its frame at `outer` and its
/// content at `inner`: the content's on macOS, the frame's elsewhere.
fn placed(outer: [f32; 2], inner: [f32; 2]) -> [f32; 2] {
    if cfg!(target_os = "macos") { inner } else { outer }
}

/// 1280×800 points (1920×1200 pixels at 150%) centred on the primary.
fn centred() -> Placement {
    Placement { position: Some(pos2(640.0, 320.0)), size: DEFAULT_SIZE, maximized: false }
}

#[test]
fn a_good_window_comes_back_where_it_was() {
    let s = saved([300.0, 150.0], [312.0, 200.0, 1800.0, 1200.0]);
    let p = placement(Some(&s), &[PRIMARY, SECOND]);
    let [x, y] = placed([300.0, 150.0], [312.0, 200.0]);
    assert_eq!(p, Placement { position: Some(pos2(x / 1.5, y / 1.5)), size: vec2(1200.0, 800.0), maximized: false });
    let on_second = saved([4000.0, 100.0], [4008.0, 140.0, 1000.0, 700.0]);
    assert_eq!(placement(Some(&on_second), &[PRIMARY, SECOND]).size, vec2(1000.0, 700.0), "at the second monitor's own scale");
}

#[test]
fn the_corner_is_the_frames_or_the_contents() {
    let s = saved([300.0, 150.0], [312.0, 200.0, 1800.0, 1200.0]);
    let inner = s.inner_rect().unwrap();
    assert_eq!(corner(&s, inner, false), pos2(300.0, 150.0), "the frame's (Windows, Linux)");
    assert_eq!(corner(&s, inner, true), pos2(312.0, 200.0), "the content's (macOS)");
}

#[test]
fn junk_falls_back_to_the_default_centred_on_the_primary() {
    let minimized = saved([-32000.0, -32000.0], [-32000.0, -32000.0, 0.0, 0.0]);
    let off_screen = saved([9000.0, 5000.0], [9008.0, 5040.0, 1800.0, 1200.0]);
    let too_small = saved([100.0, 100.0], [108.0, 140.0, 960.0, 600.0]);
    let nan = saved([f32::NAN, 0.0], [f32::NAN, 0.0, 1800.0, 1200.0]);
    let huge = saved([0.0, 0.0], [0.0, 0.0, f32::INFINITY, 1200.0]);
    for (why, s) in [("minimized", minimized), ("off screen", off_screen), ("too small", too_small), ("nan", nan), ("huge", huge)] {
        assert_eq!(placement(Some(&s), &[SECOND, PRIMARY]), centred(), "{why}");
    }
    assert_eq!(placement(None, &[PRIMARY, SECOND]), centred(), "never saved");
}

#[test]
fn a_window_on_an_unplugged_monitor_opens_on_the_primary() {
    let s = saved([4000.0, 100.0], [4008.0, 140.0, 1000.0, 700.0]);
    assert_eq!(placement(Some(&s), &[PRIMARY]), centred());
}

#[test]
fn half_on_a_monitor_is_kept_less_is_not() {
    let mostly = saved([2840.0, 100.0], [2840.0, 140.0, 1800.0, 900.0]);
    let [x, y] = placed([2840.0, 100.0], [2840.0, 140.0]);
    assert_eq!(placement(Some(&mostly), &[PRIMARY]).position, Some(pos2(x / 1.5, y / 1.5)), "1000 of 1800 pixels on it");
    let partly = saved([3040.0, 100.0], [3040.0, 140.0, 1800.0, 900.0]);
    assert_eq!(placement(Some(&partly), &[PRIMARY]), centred(), "800 of 1800");
}

#[test]
fn maximized_comes_back_maximized() {
    let good = Saved { maximized: true, ..saved([300.0, 150.0], [312.0, 200.0, 1800.0, 1200.0]) };
    let p = placement(Some(&good), &[PRIMARY]);
    assert!(p.maximized);
    assert_eq!(p.size, vec2(1200.0, 800.0), "the normal size, for when it is restored");
    let junk = Saved { maximized: true, ..saved([-32000.0, -32000.0], [-32000.0, -32000.0, 0.0, 0.0]) };
    assert_eq!(placement(Some(&junk), &[PRIMARY]), Placement { maximized: true, ..centred() });
}

#[test]
fn the_default_fits_a_small_monitor_and_needs_none() {
    let small = Monitor { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0)), scale: 1.0, primary: true };
    let p = placement(None, &[small]);
    assert_eq!(p.size, vec2(1152.0, 648.0));
    assert_eq!(p.position, Some(pos2(64.0, 36.0)));
    assert_eq!(placement(None, &[]), Placement { position: None, size: DEFAULT_SIZE, maximized: false });
}

#[test]
fn only_a_normal_window_is_saved() {
    let info = |minimized, maximized| egui::ViewportInfo {
        minimized: Some(minimized),
        maximized: Some(maximized),
        outer_rect: Some(Rect::from_min_size(pos2(200.0, 100.0), vec2(1200.0, 832.0))),
        inner_rect: Some(Rect::from_min_size(pos2(208.0, 132.0), vec2(1184.0, 792.0))),
        ..Default::default()
    };
    let normal = observed(None, &info(false, false), 1.5, 1.0);
    assert_eq!(normal, Some(saved([300.0, 150.0], [312.0, 198.0, 1776.0, 1188.0])));
    assert_eq!(observed(normal, &info(true, false), 1.5, 1.0), normal, "minimized");
    let maximized = observed(normal, &info(false, true), 1.5, 1.0);
    assert_eq!(maximized, normal.map(|s| Saved { maximized: true, ..s }), "maximized keeps the normal rect");
    assert_eq!(observed(maximized, &info(false, false), 1.5, 1.0), normal, "restored again");
    let full = egui::ViewportInfo { fullscreen: Some(true), ..info(false, false) };
    assert_eq!(observed(None, &full, 1.5, 1.0), None, "full screen");
    let parked = Rect::from_min_size(pos2(-32000.0, -32000.0), vec2(160.0, 28.0));
    let lagging = egui::ViewportInfo { outer_rect: Some(parked), inner_rect: Some(parked), ..info(false, false) };
    assert_eq!(observed(normal, &lagging, 1.0, 1.0), normal, "Windows' minimized corner, even when the flag lags");
    let gone = egui::ViewportInfo { outer_rect: None, inner_rect: None, ..info(false, false) };
    assert_eq!(observed(normal, &gone, 1.0, 1.0), normal);
}

#[test]
fn the_file_round_trips_and_junk_is_ignored() {
    let dir = {{crate_prefix_ident}}testkit::temp_dir("window-file").unwrap();
    let path = file_beside(&dir.join("ui-state.json"));
    assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("ui-state.window.json"));
    assert_eq!(load(&path), None, "no file");
    let s = Saved { maximized: true, ..saved([300.0, 150.0], [312.0, 200.0, 1800.0, 1200.0]) };
    save(&path, &s).unwrap();
    assert_eq!(load(&path), Some(s));
    let newer = r#"{"version":2,"outer":[0,0],"inner":[0,0,1,1],"zoom":1,"maximized":false}"#;
    for junk in ["", "{", "[1,2]", newer, &"x".repeat(5000)] {
        files::write_atomic(&path, junk.as_bytes()).unwrap();
        assert_eq!(load(&path), None, "{junk:.40}");
    }
}

#[test]
fn the_tracker_writes_only_what_changed_and_never_a_minimized_window() {
    let dir = {{crate_prefix_ident}}testkit::temp_dir("window-tracker").unwrap();
    let path = dir.join("ui.window.json");
    let good = saved([300.0, 150.0], [312.0, 200.0, 1800.0, 1200.0]);
    let mut tracker = Tracker::new(Some(path.clone()), Some(good));
    tracker.save();
    assert!(!path.exists(), "nothing new to write");
    let ctx = egui::Context::default();
    let frame = |tracker: &mut Tracker, info: egui::ViewportInfo| {
        let mut raw = egui::RawInput::default();
        raw.viewports.insert(egui::ViewportId::ROOT, info);
        ctx.run_ui(raw, |_| {}).textures_delta.clear();
        tracker.observe(&ctx);
    };
    frame(&mut tracker, egui::ViewportInfo { minimized: Some(true), ..Default::default() });
    tracker.save();
    assert!(!path.exists(), "closed while minimized: the last good window stays");
    let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(1000.0, 700.0));
    let normal = egui::ViewportInfo { minimized: Some(false), maximized: Some(false), outer_rect: Some(rect), inner_rect: Some(rect), ..Default::default() };
    frame(&mut tracker, normal);
    tracker.save();
    assert_eq!(load(&path), Some(saved([10.0, 20.0], [10.0, 20.0, 1000.0, 700.0])));
}

/// The hook places the viewport eframe built by what the monitors say.
#[test]
fn the_hook_places_the_window_on_the_monitors_read() {
    let monitors = monitors();
    monitors.with(|m| m.push(PRIMARY)).unwrap();
    let built = hook(None, monitors)(egui::ViewportBuilder::default());
    assert_eq!((built.position, built.inner_size, built.maximized), (centred().position, Some(DEFAULT_SIZE), Some(false)));
}
