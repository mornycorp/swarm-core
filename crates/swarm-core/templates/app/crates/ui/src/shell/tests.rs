//! The shell without a window: its state, its workers, and that its calls go through the same
//! dispatch as `{{cli}} run`. Some tests draw frames headlessly (egui's context needs no window and
//! no GPU) and read the text they drew: what is drawn is what the commands answered.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use {{crate_prefix_ident}}engine::commands::{InfoCommand, InfoParams, UiThemeCommand, UiThemeParams};
use {{crate_prefix_ident}}engine::theme::{Look, Rgb};
use {{crate_prefix_ident}}engine::{AppInfo, CallContext, Effects, ErrorCode, Why};

use super::*;
use crate::catalog;
use crate::theme::Theme;
use crate::views::views;

fn app() -> AppInfo {
    AppInfo { name: "demo".into(), version: "0.3.0".into() }
}

/// What the test commands take and give.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Nothing {}

#[derive(Debug, PartialEq, Serialize, schemars::JsonSchema)]
struct Done {
    done: bool,
}

/// A command with an effect: the dispatch asks for the caller's consent.
struct Write;
impl Command for Write {
    type Params = Nothing;
    type Output = Done;
    const ID: &'static str = "test.write";
    const TITLE: &'static str = "Write";
    const SUMMARY: &'static str = "Pretends to write a file";
    const EFFECTS: Effects = Effects { writes: true, runs_code: false, network: false, device: false };
    fn run(&self, _: &Session, _: Nothing, _: &CallContext) -> Result<Done, CommandError> {
        Ok(Done { done: true })
    }
}

/// Set to let `Slow` finish.
static RELEASE: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicUsize = AtomicUsize::new(0);

/// A command that waits until it is released or cancelled.
struct Slow;
impl Command for Slow {
    type Params = Nothing;
    type Output = Done;
    const ID: &'static str = "test.slow";
    const TITLE: &'static str = "Slow";
    const SUMMARY: &'static str = "Waits for the test";
    fn run(&self, _: &Session, _: Nothing, cx: &CallContext) -> Result<Done, CommandError> {
        STARTED.fetch_add(1, Ordering::SeqCst);
        while !RELEASE.load(Ordering::SeqCst) {
            cx.check_cancelled()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(Done { done: true })
    }
}

/// A command that can run only once the test unlocks it: in a submenu, with a shortcut.
struct Locked(Arc<AtomicBool>);
impl Command for Locked {
    type Params = Nothing;
    type Output = Done;
    const ID: &'static str = "test.locked";
    const TITLE: &'static str = "Locked";
    const SUMMARY: &'static str = "Runs once unlocked";
    const MENU: Option<&'static str> = Some("Test/Deep/Locked");
    const SHORTCUT: Option<&'static str> = Some("Mod+L");
    fn enabled(&self, _: &Session) -> Result<(), Why> {
        if self.0.load(Ordering::SeqCst) { Ok(()) } else { Err(Why::new("the test has not unlocked it", "unlock it first")) }
    }
    fn run(&self, _: &Session, _: Nothing, _: &CallContext) -> Result<Done, CommandError> {
        Ok(Done { done: true })
    }
}

fn show_locked(session: &Session, opts: &CallOpts) -> Result<Shown, CommandError> {
    let done = session.call_typed::<Locked>(Nothing::default(), opts)?;
    Ok(Shown::default().field("done", done.done))
}

fn show_write(session: &Session, opts: &CallOpts) -> Result<Shown, CommandError> {
    let done = session.call_typed::<Write>(Nothing::default(), opts)?;
    Ok(Shown::default().field("done", done.done))
}

fn show_slow(session: &Session, opts: &CallOpts) -> Result<Shown, CommandError> {
    let done = session.call_typed::<Slow>(Nothing::default(), opts)?;
    Ok(Shown::default().field("done", done.done))
}

/// A session with the app's commands and the test's; `test.locked` stays disabled.
fn session() -> Arc<Session> {
    session_with(&Arc::new(AtomicBool::new(false)))
}

/// The same, with `test.locked` enabled while `unlocked` is set.
fn session_with(unlocked: &Arc<AtomicBool>) -> Arc<Session> {
    let mut registry = {{crate_prefix_ident}}engine::commands::registry().unwrap();
    registry.register(Write).unwrap();
    registry.register(Slow).unwrap();
    registry.register(Locked(Arc::clone(unlocked))).unwrap();
    Arc::new(Session::with_registry(app(), registry))
}

fn shell_of(session: &Arc<Session>, extra: &[View]) -> (Shell, Arc<AtomicUsize>) {
    let woken = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&woken);
    let wake: Wake = Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let mut all = views();
    all.extend_from_slice(extra);
    (Shell::new(Arc::clone(session), all, wake), woken)
}

fn select(shell: &mut Shell, id: &str) {
    let at = shell.rows().iter().position(|r| r.id == id).unwrap();
    shell.select(at);
}

/// Polls until the call is no longer running (or fails the test after a while).
fn finish(shell: &mut Shell) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while shell.is_running() {
        shell.poll();
        assert!(Instant::now() < deadline, "the call never finished");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_list_is_the_registrys_and_a_command_without_a_view_says_so() {
    let session = Arc::new(Session::new(app()).unwrap());
    let (shell, _) = shell_of(&session, &[]);
    let listed: Vec<&str> = shell.rows().iter().map(|r| r.id).collect();
    let registered: Vec<&str> = session.commands().iter().map(|c| c.id).collect();
    assert_eq!(listed, registered, "the shell lists what the registry has, and names no command to do it");
    assert!(shell.rows().iter().any(|r| r.id == "app.info" && r.viewable && !r.needs_consent));
    let session = self::session();
    let (shell, _) = shell_of(&session, &[]);
    assert!(shell.rows().iter().any(|r| r.id == "test.write" && !r.viewable && r.needs_consent), "{:?}", shell.rows());
}

fn row_of(shell: &Shell, id: &str) -> usize {
    shell.rows().iter().position(|r| r.id == id).unwrap()
}

/// The menu bar is the commands' menu paths: a menu per first part, a submenu per middle one, an
/// item per command with its shortcut. The shell names no command to build it.
#[test]
fn the_menu_bar_is_made_from_the_registrys_menu_paths() {
    let session = session();
    let (shell, _) = shell_of(&session, &[]);
    let menu = |label: &str| shell.menus().iter().find(|m| m.label == label && m.row.is_none()).unwrap().clone();
    let item = |label, id| MenuNode { label, row: Some(row_of(&shell, id)), children: Vec::new() };
    assert_eq!(menu("Help").children, [item("App info", "app.info")], "app.info is in the Help menu");
    let deep = MenuNode { label: "Deep", row: None, children: vec![item("Locked", "test.locked")] };
    assert_eq!(menu("Test").children, [deep], "a middle part is a submenu");
    assert_eq!(menu("View").children, [item("Theme", "ui.theme")], "the swatch view is in the View menu");
    assert_eq!(shell.rows().get(row_of(&shell, "test.locked")).and_then(|r| r.shortcut), Some("Mod+L"));
    let in_a_menu = shell.rows().iter().filter(|r| r.menu.is_some()).count();
    assert_eq!(in_a_menu, 3, "only the commands with a menu path are in the menus: {:?}", shell.menus());
}

/// With the menus in the system menu bar (macOS), the window draws no bar of its own, and the rest
/// as it was; the menus to draw are the same either way.
#[test]
fn a_native_menu_leaves_the_window_without_a_bar() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[]);
    let menus = shell.menus().to_vec();
    let own = drawn(&mut shell);
    for menu in ["Help", "Test", "View"] {
        assert!(own.iter().any(|t| t == menu), "the window's bar has {menu}: {own:?}");
    }
    shell.set_native_menu(true);
    assert!(shell.native_menu());
    let native = drawn(&mut shell);
    let rest: Vec<&String> = own.iter().filter(|t| !["Help", "Test", "View"].contains(&t.as_str())).collect();
    assert_eq!(native.iter().collect::<Vec<_>>(), rest, "only the bar is gone");
    assert_eq!(shell.menus(), menus, "the menus are the system bar's to draw now");
}

/// A command that can't run now is greyed out with the reason the dispatch gives: its menu item, run,
/// fails `unavailable` with that very reason. Unlocked, the next refresh enables it, and it runs.
#[test]
fn a_command_that_cannot_run_is_greyed_out_with_the_dispatchs_reason() {
    let unlocked = Arc::new(AtomicBool::new(false));
    let session = session_with(&unlocked);
    let (mut shell, _) = shell_of(&session, &[View { id: Locked::ID, run: show_locked }]);
    let at = row_of(&shell, "test.locked");
    let why = shell.rows().get(at).and_then(|r| r.why.clone()).unwrap();
    assert_eq!((why.code, why.remediation.as_str()), (ErrorCode::Unavailable, "unlock it first"));
    assert!(shell.rows().iter().filter(|r| r.id != "test.locked").all(|r| r.why.is_none()), "the others can run");
    shell.activate(at);
    finish(&mut shell);
    assert_eq!(*shell.call(), Call::Failed { id: "test.locked", error: why }, "the call is refused for the reason the menu shows");
    unlocked.store(true, Ordering::SeqCst);
    shell.refresh();
    assert!(shell.rows().get(at).is_some_and(|r| r.why.is_none()), "the state follows the session's");
    shell.activate(at);
    finish(&mut shell);
    assert_eq!(*shell.call(), Call::Done { id: "test.locked", shown: Shown::default().field("done", true) });
}

/// A menu item runs its command when the UI has a view for it and it needs no consent; one that needs
/// consent is only selected, for the user to give it.
#[test]
fn a_menu_item_runs_its_command_or_selects_one_that_needs_consent() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[View { id: Write::ID, run: show_write }]);
    shell.activate(row_of(&shell, "app.info"));
    finish(&mut shell);
    assert!(matches!(shell.call(), Call::Done { id: "app.info", .. }), "{:?}", shell.call());
    shell.activate(row_of(&shell, "test.write"));
    assert_eq!(shell.selected().map(|r| r.id), Some("test.write"));
    assert!(matches!(shell.call(), Call::Done { id: "app.info", .. }), "not run: it needs consent first");
    shell.activate(1000);
    assert_eq!(shell.selected().map(|r| r.id), Some("test.write"), "a row that isn't there runs nothing");
}

#[test]
fn a_call_through_the_ui_is_the_call_run_makes() {
    let session = Arc::new(Session::new(app()).unwrap());
    let (mut shell, woken) = shell_of(&session, &[]);
    select(&mut shell, "app.info");
    shell.run_selected();
    assert!(shell.is_running() || matches!(shell.call(), Call::Idle), "the frame that starts a call returns at once");
    finish(&mut shell);
    // The wire form, which `run` prints: the same lookup and checks, the same result, each field shown.
    let wire = session.call("app.info", json!({}), &CallOpts::new(Caller::Cli)).unwrap();
    let Call::Done { id, shown } = shell.call().clone() else { panic!("{:?}", shell.call()) };
    assert_eq!(id, "app.info");
    assert_eq!(shown, info_shown(&session));
    assert_eq!(shown.fields.len(), wire.as_object().map_or(0, serde_json::Map::len), "every field of the result is shown");
    assert!(woken.load(Ordering::SeqCst) >= 1, "the worker woke the UI when it was done");
}

/// `app.info`'s typed answer, as its view must show it: each field under its catalog label.
fn info_shown(session: &Session) -> Shown {
    let info = session.call_typed::<InfoCommand>(InfoParams::default(), &CallOpts::new(Caller::Test)).unwrap();
    Shown::default().field(catalog::INFO_NAME, info.name).field(catalog::INFO_VERSION, info.version).field(catalog::INFO_SCHEMA_VERSION, info.schema_version)
}

/// Every text a frame of `shell` draws, in the order it is painted. Two frames: egui sizes a grid on
/// its first, unseen.
fn drawn(shell: &mut Shell) -> Vec<String> {
    drawn_in(shell, egui::Theme::Dark).texts
}

/// What a frame drew: its texts, and the fills and edges of its rectangles, in the order they are
/// painted.
#[derive(Debug, Default)]
struct Drawn {
    texts: Vec<String>,
    fills: Vec<egui::Color32>,
    edges: Vec<egui::Color32>,
}

impl Drawn {
    fn walk(&mut self, shape: &egui::Shape) {
        match shape {
            egui::Shape::Text(text) => self.texts.push(text.galley.text().to_owned()),
            egui::Shape::Rect(rect) => {
                self.fills.push(rect.fill);
                self.edges.push(rect.stroke.color);
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| self.walk(s)),
            _ => {}
        }
    }
}

/// What a frame of `shell` draws in egui's `look` (dark or light), styled in the theme as the window
/// is. Two frames, as [`drawn`].
fn drawn_in(shell: &mut Shell, look: egui::Theme) -> Drawn {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    ctx.set_theme(look);
    let mut out = Drawn::default();
    for _ in 0..2 {
        out = Drawn::default();
        let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| shell.ui(ui));
        frame.shapes.iter().for_each(|clipped| out.walk(&clipped.shape));
        frame.textures_delta.clear();
    }
    out
}

/// A host that draws before it applies the theme gets a blank frame, not a panic: the shell installs
/// the theme's faces, and draws once they are bound, icons and all.
#[test]
fn a_frame_before_the_theme_is_applied_is_blank_not_a_panic() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[]);
    let ctx = egui::Context::default();
    let mut texts = Vec::new();
    for _ in 0..3 {
        let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| shell.ui(ui));
        let mut drawn = Drawn::default();
        frame.shapes.iter().for_each(|clipped| drawn.walk(&clipped.shape));
        frame.textures_delta.clear();
        texts.push(drawn.texts);
    }
    assert!(texts[0].is_empty(), "the first frame draws nothing: {:?}", texts[0]);
    assert!(crate::fonts::bound(&ctx), "the faces are bound by then");
    assert!(texts[2].iter().any(|t| t == catalog::COMMANDS), "then the shell draws: {:?}", texts[2]);
}

/// **What is drawn is what the command returns**: the view of `app.info`, run from the shell, draws
/// each field of the command's typed answer under its label, as the command gave it.
#[test]
fn the_view_draws_app_infos_result() {
    let session = Arc::new(Session::new(app()).unwrap());
    let (mut shell, _) = shell_of(&session, &[]);
    shell.activate(row_of(&shell, "app.info"));
    finish(&mut shell);
    let texts = drawn(&mut shell);
    let answer = info_shown(&session);
    assert!(answer.fields.iter().any(|(_, value)| value == "0.3.0"), "the answer is the session's: {answer:?}");
    for (label, value) in &answer.fields {
        let pair = texts.windows(2).any(|w| w[0] == *label && w[1] == *value);
        assert!(pair, "`{label}` drawn with `{value}`, the command's answer: {texts:?}");
    }
}

/// **The swatch view draws `ui.theme`'s answer**, in each look: each token's name, meaning and value
/// in that order, with a square of the value; each range's name, size, meaning and source, then its
/// entries' indexes in order (and `none` after them, where the range has one), each entry a square
/// of its colour edged in its border and its index on the entry made a text background. Every colour
/// is the answer's: the view names none.
#[test]
fn the_swatch_view_draws_ui_themes_answer() {
    let session = Arc::new(Session::new(app()).unwrap());
    let (mut shell, _) = shell_of(&session, &[]);
    shell.activate(row_of(&shell, "ui.theme"));
    finish(&mut shell);
    let answer = session.call_typed::<UiThemeCommand>(UiThemeParams::default(), &CallOpts::new(Caller::Test)).unwrap();
    for (egui_look, look) in [(egui::Theme::Dark, Look::Dark), (egui::Theme::Light, Look::Light)] {
        let frame = drawn_in(&mut shell, egui_look);
        let theme = Theme::in_look(look);
        let filled = |hex: &str| Rgb::parse(hex).is_some() && frame.fills.contains(&theme.parse(hex));
        let edged = |hex: &str| Rgb::parse(hex).is_some() && frame.edges.contains(&theme.parse(hex));
        let from = |first: &str| frame.texts.iter().position(|t| t == first).map(|at| frame.texts[at..].to_vec()).unwrap_or_default();
        for token in &answer.tokens {
            let value = if look == Look::Dark { &token.dark } else { &token.light };
            assert!(from(&token.name).starts_with(&[token.name.clone(), token.meaning.clone(), value.clone()]), "{look:?} {}: {:?}", token.name, frame.texts);
            assert!(filled(value), "{look:?}: a square of {} ({value})", token.name);
        }
        for range in &answer.ranges {
            let mut want = vec![range.name.clone(), catalog::range_size(range.size), range.meaning.clone(), catalog::source(&range.source)];
            want.extend(range.entries.iter().map(|e| e.index.to_string()));
            want.extend(range.none.iter().map(|_| catalog::NONE.to_owned()));
            assert!(from(&range.name).starts_with(&want), "{look:?} {}: {:?}", range.name, frame.texts);
            for entry in range.entries.iter().chain(&range.none) {
                let (colour, behind, border) = match look {
                    Look::Dark => (&entry.dark, &entry.behind_text_dark, &entry.border_dark),
                    Look::Light => (&entry.light, &entry.behind_text_light, &entry.border_light),
                };
                assert!(filled(colour) && filled(behind), "{look:?} {} entry {}: {colour} and {behind}", range.name, entry.index);
                assert!(edged(border), "{look:?} {} entry {}: edged in {border}", range.name, entry.index);
            }
        }
    }
}

/// A rectangle or a text a frame drew: what it says (a text), where it is, and the rectangle it is
/// clipped to.
#[derive(Debug)]
struct Placed {
    text: Option<String>,
    at: egui::Rect,
    clip: egui::Rect,
}

fn place(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<Placed>) {
    let at = shape.visual_bounding_rect();
    match shape {
        egui::Shape::Text(text) => out.push(Placed { text: Some(text.galley.text().to_owned()), at, clip }),
        egui::Shape::Rect(_) => out.push(Placed { text: None, at, clip }),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| place(s, clip, out)),
        _ => {}
    }
}

/// **A range's row wraps** (mornycorp/swarm-core#102): in a window 400 px wide, every entry of `range32` (its square, its
/// index's background and its index, all drawn after the range's name) is inside the rectangle it
/// is clipped to, the view's, rather than past its right edge. The window is tall enough for the
/// whole view, so only the width can hide an entry.
#[test]
fn a_ranges_row_wraps_so_range32_shows_whole_in_a_narrow_window() {
    let session = Arc::new(Session::new(app()).unwrap());
    let (mut shell, _) = shell_of(&session, &[]);
    shell.set_show_list(false);
    shell.activate(row_of(&shell, "ui.theme"));
    finish(&mut shell);
    let answer = session.call_typed::<UiThemeCommand>(UiThemeParams::default(), &CallOpts::new(Caller::Test)).unwrap();
    let range = answer.ranges.iter().find(|r| r.name == "range32").unwrap();
    assert_eq!(answer.ranges.last().map(|r| &r.name), Some(&range.name), "range32 is drawn last, so all after its name is its");
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 8000.0));
    let mut placed = Vec::new();
    for _ in 0..2 {
        placed.clear();
        let mut frame = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| shell.ui(ui));
        frame.shapes.iter().for_each(|clipped| place(&clipped.shape, clipped.clip_rect, &mut placed));
        frame.textures_delta.clear();
    }
    let name = placed.iter().position(|p| p.text.as_deref() == Some(range.name.as_str())).unwrap();
    let entries = &placed[name..];
    let want: Vec<String> = range.entries.iter().map(|e| e.index.to_string()).collect();
    let indexes: Vec<&Placed> = entries.iter().filter(|p| p.text.as_ref().is_some_and(|t| want.contains(t))).collect();
    assert_eq!(indexes.iter().filter_map(|p| p.text.clone()).collect::<Vec<_>>(), want, "every index of range32 is drawn, in order");
    assert!(indexes.windows(2).any(|w| w[1].at.min.y > w[0].at.max.y), "range32 takes more than one line at 400 px: {indexes:?}");
    for p in entries {
        assert!(p.clip.max.x <= screen.max.x, "the view is clipped to the window: {p:?}");
        assert!(p.clip.contains_rect(p.at), "range32 shows whole at 400 px wide: {p:?} is outside the view");
    }
}

/// The UI's state is the session's: a `ui.set` made as an agent makes it (the wire form) changes
/// what the next frame draws, and a click is a `ui.set` that `ui.inspect` then reports.
#[test]
fn the_ui_state_is_the_sessions_and_ui_set_drives_what_is_drawn() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[]);
    let before = drawn(&mut shell);
    assert!(before.iter().any(|t| t == catalog::COMMANDS) && before.iter().any(|t| t == catalog::SELECT_A_COMMAND), "{before:?}");
    let agent = CallOpts::new(Caller::Socket);
    session.call("ui.set", json!({"selected": "app.info", "show_list": false}), &agent).unwrap();
    let after = drawn(&mut shell);
    assert_eq!(shell.selected().map(|r| r.id), Some("app.info"));
    assert!(after.iter().any(|t| t == "App info") && after.iter().any(|t| t == catalog::SHOW_LIST), "the view is open: {after:?}");
    assert!(!after.iter().any(|t| t == catalog::COMMANDS), "the list is hidden: {after:?}");
    shell.select(row_of(&shell, "test.write"));
    shell.set_show_list(true);
    let inspected = session.call("ui.inspect", json!({}), &agent).unwrap();
    assert_eq!((inspected["selected"].clone(), inspected["show_list"].clone()), (json!("test.write"), json!(true)), "the clicks were `ui.set`s");
}

/// The dock is laid out by the UI state: a layout set over the socket is what the next frame builds,
/// and a person's resize goes back to the state through `ui.set`, as the agent's change came.
#[test]
fn the_dock_is_laid_out_by_the_ui_state_and_a_resize_goes_back_to_it() {
    use egui_tiles::{Container, Tile};
    use {{crate_prefix_ident}}engine::DockNode;
    let session = session();
    let (mut shell, _) = shell_of(&session, &[]);
    drawn(&mut shell);
    assert_eq!(shell.dock().and_then(crate::dock::Dock::layout), Some(DockNode::default_layout()));
    let stacked = json!({"kind": "split", "across": false, "parts": [
        {"share": 400, "node": {"kind": "tabs", "panes": ["view"], "active": 0}},
        {"share": 600, "node": {"kind": "tabs", "panes": ["commands"], "active": 0}}]});
    let agent = CallOpts::new(Caller::Socket);
    session.call("ui.set", json!({"dock": stacked.clone()}), &agent).unwrap();
    drawn(&mut shell);
    assert_eq!(shell.dock().and_then(crate::dock::Dock::layout).map(|l| serde_json::to_value(l).unwrap()), Some(stacked));
    let tree = shell.dock_mut().unwrap().tree_mut();
    let root = tree.root().unwrap();
    let Some(Tile::Container(Container::Linear(split))) = tree.tiles.get_mut(root) else { panic!("the root is a split") };
    let view = split.children[0];
    split.shares.set_share(view, 900.0);
    drawn(&mut shell);
    let inspected = session.call("ui.inspect", json!({}), &agent).unwrap();
    assert_eq!(inspected["dock"]["parts"][0]["share"], json!(600), "900 of 1500 is 600 per mille: {inspected}");
}

/// A consent is for the command it was given for: a selection changed over the socket leaves it unused.
#[test]
fn a_selection_changed_elsewhere_withdraws_the_consent() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[View { id: Write::ID, run: show_write }]);
    select(&mut shell, "test.write");
    shell.set_consent(true);
    assert!(shell.consent());
    session.call("ui.set", json!({"selected": "app.info"}), &CallOpts::new(Caller::Socket)).unwrap();
    shell.refresh();
    assert!(!shell.consent(), "given for test.write, not app.info");
    session.call("ui.set", json!({"selected": "test.write"}), &CallOpts::new(Caller::Socket)).unwrap();
    shell.refresh();
    shell.run_selected();
    finish(&mut shell);
    let Call::Failed { error, .. } = shell.call().clone() else { panic!("{:?}", shell.call()) };
    assert_eq!(error.code, ErrorCode::ConsentRequired, "a consent outlives no change of selection it didn't see");
}

#[test]
fn a_failure_is_the_dispatchs_error_and_consent_is_the_users() {
    let session = session();
    let write = View { id: Write::ID, run: show_write };
    let (mut shell, _) = shell_of(&session, &[write]);
    select(&mut shell, "test.write");
    shell.run_selected();
    finish(&mut shell);
    let wire = session.call("test.write", json!({}), &CallOpts::new(Caller::Cli)).unwrap_err();
    let Call::Failed { error, .. } = shell.call().clone() else { panic!("{:?}", shell.call()) };
    assert_eq!(error, wire, "the UI shows the very error `run` gets without --yes");
    assert_eq!(error.code, ErrorCode::ConsentRequired);
    shell.set_consent(true);
    shell.run_selected();
    finish(&mut shell);
    assert_eq!(*shell.call(), Call::Done { id: "test.write", shown: Shown::default().field("done", true) });
}

/// Consent is for one run of the command it was given for: a run uses it up, and selecting a command
/// withdraws it, so a box ticked for one command never runs another's effects.
#[test]
fn consent_is_withdrawn_after_a_run_and_on_select() {
    let session = session();
    let write = View { id: Write::ID, run: show_write };
    let (mut shell, _) = shell_of(&session, &[write]);
    select(&mut shell, "test.write");
    shell.set_consent(true);
    shell.run_selected();
    assert!(!shell.consent(), "the run used the consent up");
    finish(&mut shell);
    assert!(matches!(shell.call(), Call::Done { .. }), "{:?}", shell.call());
    shell.run_selected();
    finish(&mut shell);
    let Call::Failed { error, .. } = shell.call().clone() else { panic!("{:?}", shell.call()) };
    assert_eq!(error.code, ErrorCode::ConsentRequired, "the second run asks again");
    shell.set_consent(true);
    select(&mut shell, "app.info");
    assert!(!shell.consent(), "a consent given for one command isn't another's");
    select(&mut shell, "test.write");
    assert!(!shell.consent());
}

fn show_panics(_: &Session, _: &CallOpts) -> Result<Shown, CommandError> {
    panic!("a view with a bug")
}

/// A view that panics doesn't leave the shell running for ever: the call fails `internal`.
#[test]
fn a_view_that_panics_is_an_internal_failure_not_a_call_that_never_ends() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[View { id: Write::ID, run: show_panics }]);
    select(&mut shell, "test.write");
    shell.run_selected();
    finish(&mut shell);
    let Call::Failed { id, error } = shell.call().clone() else { panic!("{:?}", shell.call()) };
    assert_eq!((id, error.code), ("test.write", ErrorCode::Internal));
    assert!(!error.remediation.is_empty(), "{error}");
}

#[test]
fn a_call_does_not_block_the_ui_thread_and_cancel_stops_it() {
    RELEASE.store(false, Ordering::SeqCst);
    let session = session();
    let (mut shell, _) = shell_of(&session, &[View { id: Slow::ID, run: show_slow }]);
    select(&mut shell, "test.slow");
    shell.run_selected();
    assert!(shell.is_running(), "the call started and the frame returned while it runs");
    shell.run_selected();
    shell.poll();
    assert!(shell.is_running(), "a second run while one is going is ignored");
    let deadline = Instant::now() + Duration::from_secs(20);
    while STARTED.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the worker never started the command");
        std::thread::sleep(Duration::from_millis(2));
    }
    shell.cancel();
    finish(&mut shell);
    let Call::Failed { error, .. } = shell.call().clone() else { panic!("{:?}", shell.call()) };
    assert_eq!(error.code, ErrorCode::Cancelled, "the command polled its token and stopped");
    // And released, it finishes with its typed result.
    RELEASE.store(true, Ordering::SeqCst);
    shell.run_selected();
    finish(&mut shell);
    assert!(matches!(shell.call(), Call::Done { .. }), "{:?}", shell.call());
}

#[test]
fn nothing_runs_without_a_viewable_selection() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[]);
    shell.run_selected();
    assert_eq!(*shell.call(), Call::Idle, "no selection");
    select(&mut shell, "test.write");
    shell.run_selected();
    assert_eq!(*shell.call(), Call::Idle, "no view");
    shell.select(1000);
    assert_eq!(shell.selected().map(|r| r.id), Some("test.write"), "a row that isn't there isn't selected");
}

#[test]
fn a_frame_draws_headlessly_in_every_state() {
    let session = session();
    let (mut shell, _) = shell_of(&session, &[View { id: Write::ID, run: show_write }]);
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let frame = |shell: &mut Shell| {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| shell.ui(ui));
        assert!(!output.shapes.is_empty(), "something was drawn");
        output.textures_delta.clear();
    };
    frame(&mut shell);
    select(&mut shell, "app.info");
    shell.run_selected();
    frame(&mut shell);
    finish(&mut shell);
    frame(&mut shell);
    select(&mut shell, "test.write");
    shell.run_selected();
    finish(&mut shell);
    frame(&mut shell);
    assert!(matches!(shell.call(), Call::Failed { .. }));
}
