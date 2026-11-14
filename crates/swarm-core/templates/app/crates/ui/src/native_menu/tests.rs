//! The system menu bar as data: its layout, its items' keys and states, and the ids a click comes
//! back with. Plain data, so these run on every platform; the macOS wiring is the app's.

use {{crate_prefix_ident}}engine::CommandError;
use {{crate_prefix_ident}}engine::menu::{NAMED_KEYS, shortcut_problem};

use super::*;
use crate::shell::menu_bar;

/// A row with `menu` and `shortcut`, which can run now unless `why`.
fn row(id: &'static str, menu: Option<&'static str>, shortcut: Option<&'static str>, why: Option<&str>) -> Row {
    Row { id, title: id, summary: id, needs_consent: false, viewable: true, menu, shortcut, why: why.map(|w| CommandError::unavailable(w, "wait")) }
}

/// The skeleton's commands, and one in a submenu with a shortcut that can't run now.
fn rows() -> Vec<Row> {
    vec![
        row("app.info", Some("Help/App info"), None, None),
        row("test.deep", Some("Test/Deep/Locked"), Some("Mod+L"), Some("locked")),
        row("ui.theme", Some("View/Theme"), Some("Mod+Shift+T"), None),
        row("no.menu", None, Some("Mod+N"), None),
    ]
}

fn bar_of(rows: &[Row]) -> Vec<Menu> {
    bar("Demo", &menu_bar(rows), rows)
}

fn labels(menu: &Menu) -> Vec<&str> {
    menu.entries
        .iter()
        .map(|e| match e {
            Entry::Item(item) => item.label.as_str(),
            Entry::Menu(menu) => menu.label.as_str(),
            Entry::System { label, .. } => label.as_str(),
            Entry::Separator => "-",
        })
        .collect()
}

fn item(rows: &[Row], at: usize, label: &str, accelerator: Option<&str>, enabled: bool) -> Entry {
    let _ = rows.get(at).unwrap();
    Entry::Item(Item { label: label.to_owned(), action: Action::Row(at), accelerator: accelerator.map(str::to_owned), enabled })
}

/// macOS's layout: the application menu named for the product, the app's menus in the order the
/// commands first name them, the Window menu, and Help last, wherever the commands put it.
#[test]
fn the_bar_is_the_application_menu_the_apps_menus_window_then_help() {
    let rows = rows();
    let bar = bar_of(&rows);
    let names: Vec<(&str, Role)> = bar.iter().map(|m| (m.label.as_str(), m.role)).collect();
    assert_eq!(names, [("Demo", Role::App), ("Test", Role::Plain), ("View", Role::Plain), ("Window", Role::Window), ("Help", Role::Help)]);
    let app = bar.first().unwrap();
    assert_eq!(labels(app), ["About Demo", "-", "Services", "-", "Hide Demo", "Hide Others", "Show All", "-", "Quit Demo"]);
    let quit = Entry::Item(Item { label: "Quit Demo".into(), action: Action::Quit, accelerator: Some("Cmd+Q".into()), enabled: true });
    assert_eq!(app.entries.last(), Some(&quit), "Quit is the app's, on ⌘Q, so the window closes as its close button closes it");
    assert_eq!(labels(bar.get(3).unwrap()), ["Minimize", "Zoom", "-", "Bring All to Front"]);
}

/// Each command's item: its label, the row a click activates, its shortcut as the accelerator and
/// its state as the in-window bar shows it (greyed out when it can't run now); a middle part of a
/// path is a submenu. A command with no menu path has no item.
#[test]
fn an_item_is_its_commands_label_row_shortcut_and_state() {
    let rows = rows();
    let bar = bar_of(&rows);
    let menu = |label: &str| bar.iter().find(|m| m.label == label).unwrap().clone();
    let deep = Menu { label: "Deep".into(), role: Role::Plain, entries: vec![item(&rows, 1, "Locked", Some("Cmd+L"), false)] };
    assert_eq!(menu("Test").entries, [Entry::Menu(deep)]);
    assert_eq!(menu("View").entries, [item(&rows, 2, "Theme", Some("Cmd+Shift+T"), true)]);
    assert_eq!(menu("Help").entries, [item(&rows, 0, "App info", None, true)], "the app's own info stays in Help: About is the system's panel");
    let all = format!("{bar:?}");
    assert!(!all.contains("Row(3)"), "no.menu has no menu path, so no item: {all}");
}

/// A command placed in the `App` menu (preferences, say) goes in the application menu after About,
/// and the app's own `Window` items come before the system's.
#[test]
fn the_app_and_window_menus_take_the_apps_own_items() {
    let rows = vec![row("app.settings", Some("App/Settings…"), Some("Mod+Comma"), None), row("window.reset", Some("Window/Reset layout"), None, None)];
    let bar = bar_of(&rows);
    assert_eq!(bar.len(), 2, "no `App` or `Window` menu of their own: {bar:?}");
    let (app, window) = (bar.first().unwrap(), bar.get(1).unwrap());
    assert_eq!(labels(app), ["About Demo", "-", "Settings…", "-", "Services", "-", "Hide Demo", "Hide Others", "Show All", "-", "Quit Demo"]);
    assert_eq!(app.entries.get(2), Some(&item(&rows, 0, "Settings…", Some("Cmd+Comma"), true)));
    assert_eq!(labels(window), ["Reset layout", "-", "Minimize", "Zoom", "-", "Bring All to Front"]);
    assert_eq!(window.role, Role::Window);
}

/// A command's shortcut that the system's own items take (⌘Q, ⌘H, ⌥⌘H, ⌘M) keeps its item but not
/// the key, so a key never runs two things.
#[test]
fn a_shortcut_the_system_takes_is_left_off_the_item() {
    let rows: Vec<Row> = SYSTEM_KEYS.iter().map(|key| row("app.clash", Some("File/Clash"), Some(key), None)).collect();
    for (at, key) in SYSTEM_KEYS.iter().enumerate() {
        let one = vec![rows.get(at).unwrap().clone()];
        let file = bar_of(&one).into_iter().find(|m| m.label == "File").unwrap();
        assert_eq!(file.entries, [item(&one, 0, "Clash", None, true)], "{key}");
    }
}

/// Every shortcut the registry accepts has an accelerator, by these rules; the macOS wiring's own
/// test parses each with `muda`.
#[test]
fn every_portable_shortcut_has_an_accelerator() {
    for (shortcut, want) in [
        ("Mod+O", "Cmd+O"),
        ("Mod+Shift+P", "Cmd+Shift+P"),
        ("Ctrl+Alt+Delete", "Ctrl+Alt+Delete"),
        ("F5", "F5"),
        ("Shift+F24", "Shift+F24"),
        ("Mod+1", "Cmd+1"),
        ("Mod+Up", "Cmd+ArrowUp"),
        ("Mod+Plus", "Cmd+Equal"),
        ("Alt+Enter", "Alt+Enter"),
    ] {
        assert_eq!(accelerator(shortcut).as_deref(), Some(want), "{shortcut}");
    }
    for key in NAMED_KEYS.split(' ').chain(["A", "Z", "0", "9", "F1", "F24"]) {
        let shortcut = format!("Mod+Shift+{key}");
        assert_eq!(shortcut_problem(&shortcut), None, "{shortcut} is the registry's");
        assert!(accelerator(&shortcut).is_some_and(|a| a.starts_with("Cmd+Shift+")), "{shortcut}");
    }
    for bad in ["", "Mod+", "Cmd+O", "Mod+o", "Mod+F25", "Mod+é", "Hyper+O"] {
        assert_eq!(accelerator(bad), None, "{bad:?}");
    }
}

/// An item's id comes back with its click: `row.<n>` or `quit`, and nothing else is an action.
#[test]
fn an_items_id_comes_back_as_its_action() {
    for action in [Action::Row(0), Action::Row(41), Action::Quit] {
        assert_eq!(Action::from_id(&action.id()), Some(action));
    }
    for other in ["", "row.", "row.-1", "row.+1", "row.x", "quit ", "about", "1"] {
        assert_eq!(Action::from_id(other), None, "{other:?}");
    }
}
