//! The system menu bar (the App UI Substrate › Menus): what a native menu bar holds, made from the
//! same menu paths and shortcuts the in-window bar is ([`Shell::menus`](crate::Shell::menus)), as
//! plain data. It names no toolkit and no platform API, so it is tested on every platform; the
//! windowed app turns it into the platform's menus (on macOS, `apps/{{app}}/src/macos_menu.rs`,
//! with `muda`), and a click comes back as an [`Action`] it hands to the shell, exactly as a click
//! in the in-window bar does ([`Shell::activate`](crate::Shell::activate)), so the dispatch's
//! consent and `unavailable` rules hold the same.
//!
//! The bar ([`bar`]) is macOS's layout:
//!
//! - **The application menu**, named for the product: About, the app's own `App` menu's items (a
//!   command whose menu path starts with [`APP_MENU`], such as `App/Settings…`), Services, Hide,
//!   Hide Others, Show All and Quit ([`Action::Quit`], which closes the window as its close button
//!   does, so the UI state and the window's place are saved).
//! - **The app's menus**, in the order the commands first name them, as the in-window bar has them.
//! - **The Window menu**: the app's own `Window` items, if any, then Minimize, Zoom and Bring All to
//!   Front. **The Help menu** last, when the app has one (the skeleton's `Help/App info`).
//!
//! A command's shortcut becomes its item's accelerator ([`accelerator`]), unless it is one of the
//! keys the system's own items take ([`SYSTEM_KEYS`]). Each item is enabled when its command can
//! run now (`Row::why`), as the in-window bar greys it out; the app sets that again as the rows
//! change.

use {{crate_prefix_ident}}engine::menu;

use crate::catalog;
use crate::shell::{MenuNode, Row, item_row};

/// The first part of a menu path that puts an item in the application menu (`App/Settings…`). On a
/// platform without one, it is a menu like any other, named `App`.
pub const APP_MENU: &str = "App";
/// The menu the system's window items (Minimize, Zoom, Bring All to Front) join; made, named
/// [`catalog::WINDOW`], when no command names it.
pub const WINDOW_MENU: &str = "Window";
/// The menu that goes last, as the platform's help menu.
pub const HELP_MENU: &str = "Help";

/// The shortcuts the system's own items take on macOS (Quit, Hide, Hide Others, Minimize): a
/// command's shortcut that is one of them keeps its item but not the key.
pub const SYSTEM_KEYS: [&str; 4] = ["Mod+Q", "Mod+H", "Mod+Alt+H", "Mod+M"];

/// A menu of the bar, or a submenu in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub label: String,
    pub role: Role,
    pub entries: Vec<Entry>,
}

/// What the platform makes of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The application menu, named for the product.
    App,
    /// One of the app's menus, or a submenu.
    Plain,
    /// The platform's window menu, which lists the app's windows.
    Window,
    /// The platform's help menu.
    Help,
}

/// One line of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// An item that runs a command, or quits.
    Item(Item),
    /// A submenu.
    Menu(Menu),
    /// One of the system's own items, which the platform runs itself.
    System {
        kind: System,
        label: String,
    },
    Separator,
}

/// An item the app runs when it is clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub action: Action,
    /// The key that runs it, in the platform menu's form ([`accelerator`]).
    pub accelerator: Option<String>,
    /// Whether it can run now: its command's state, as of the rows it was made from.
    pub enabled: bool,
}

/// What a click on an [`Item`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Activates the command of the shell's row, as a click in the in-window bar does.
    Row(usize),
    /// Closes the window, as its close button does.
    Quit,
}

impl Action {
    /// The item's id in the platform's menu: `row.<n>` or `quit`.
    pub fn id(self) -> String {
        match self {
            Action::Row(at) => format!("row.{at}"),
            Action::Quit => "quit".to_owned(),
        }
    }

    /// The action of the item with `id`, if it is one of [`Action::id`]'s.
    pub fn from_id(id: &str) -> Option<Action> {
        match id {
            "quit" => Some(Action::Quit),
            _ => id.strip_prefix("row.").filter(|n| !n.starts_with('+')).and_then(|n| n.parse().ok()).map(Action::Row),
        }
    }
}

/// The system's own items, which the platform runs: the app only places them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    BringAllToFront,
}

/// The system menu bar of `menus` (the shell's, made from `rows`' menu paths), for the product named
/// `product`: the application menu, the app's menus, the Window menu, and Help last.
pub fn bar(product: &str, menus: &[MenuNode], rows: &[Row]) -> Vec<Menu> {
    let mut app = vec![system(System::About, catalog::about(product)), Entry::Separator];
    let mut own: Vec<Menu> = Vec::new();
    let mut window: Vec<Entry> = Vec::new();
    let mut help = None;
    // A menu path has a menu and an item, so the bar's top level holds menus only.
    for node in menus.iter().filter(|node| node.row.is_none()) {
        let entries = entries(&node.children, rows);
        match node.label {
            APP_MENU => {
                app.extend(entries);
                app.push(Entry::Separator);
            }
            WINDOW_MENU => window = entries,
            HELP_MENU => help = Some(Menu { label: node.label.to_owned(), role: Role::Help, entries }),
            label => own.push(Menu { label: label.to_owned(), role: Role::Plain, entries }),
        }
    }
    app.extend([
        system(System::Services, catalog::SERVICES.to_owned()),
        Entry::Separator,
        system(System::Hide, catalog::hide(product)),
        system(System::HideOthers, catalog::HIDE_OTHERS.to_owned()),
        system(System::ShowAll, catalog::SHOW_ALL.to_owned()),
        Entry::Separator,
        Entry::Item(Item { label: catalog::quit(product), action: Action::Quit, accelerator: accelerator("Mod+Q"), enabled: true }),
    ]);
    if !window.is_empty() {
        window.push(Entry::Separator);
    }
    window.extend([
        system(System::Minimize, catalog::MINIMIZE.to_owned()),
        system(System::Zoom, catalog::ZOOM.to_owned()),
        Entry::Separator,
        system(System::BringAllToFront, catalog::BRING_ALL_TO_FRONT.to_owned()),
    ]);
    let mut bar = vec![Menu { label: product.to_owned(), role: Role::App, entries: app }];
    bar.extend(own);
    bar.push(Menu { label: catalog::WINDOW.to_owned(), role: Role::Window, entries: window });
    bar.extend(help);
    bar
}

fn system(kind: System, label: String) -> Entry {
    Entry::System { kind, label }
}

/// A menu's entries: an item per command, a submenu per menu. The depth is a menu path's, which the
/// registry caps.
fn entries(nodes: &[MenuNode], rows: &[Row]) -> Vec<Entry> {
    nodes
        .iter()
        .map(|node| match item_row(node, rows) {
            Some((at, row)) => Entry::Item(Item {
                label: node.label.to_owned(),
                action: Action::Row(at),
                accelerator: row.shortcut.filter(|s| !SYSTEM_KEYS.contains(s)).and_then(accelerator),
                enabled: row.why.is_none(),
            }),
            None => Entry::Menu(Menu { label: node.label.to_owned(), role: Role::Plain, entries: entries(&node.children, rows) }),
        })
        .collect()
}

/// A portable shortcut (`Mod+Shift+P`, the engine's form) as macOS's menus take it: the engine's
/// [`menu::macos_accelerator`], `muda`'s accelerator text (`Cmd+Shift+P`). `None` for text that
/// isn't a shortcut, which the registry never lets through.
pub fn accelerator(shortcut: &str) -> Option<String> {
    menu::macos_accelerator(shortcut)
}

#[cfg(test)]
mod tests;
