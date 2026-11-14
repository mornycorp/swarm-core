//! macOS: the menus in the system menu bar, not inside the window (the App UI Substrate › Menus).
//! What the bar holds is the UI crate's ([`native_menu::bar`], plain data tested on every platform);
//! this module only hands it to AppKit, with `muda` (tauri-apps, MIT OR Apache-2.0), on the window's
//! own event loop. A click comes back as the item's [`Action`]: a command's item activates its row,
//! as a click in the in-window bar does ([`Shell::activate`]), so the dispatch's consent and
//! `unavailable` rules hold the same; Quit closes the window as its close button does, so the UI
//! state and the window's place are saved. The items' enabled state follows the shell's rows every
//! frame. A shortcut is the item's key equivalent alone: AppKit takes it before the window sees the
//! key, so egui never gets it too.

use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use muda::accelerator::Accelerator;
use muda::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use {{crate_prefix_ident}}ui::Shell;
use {{crate_prefix_ident}}ui::native_menu::{self, Action, Entry, Role, System};
use objc2::MainThreadMarker;
use objc2_app_kit::NSWindow;

/// Turns automatic window tabbing off, so AppKit adds no tab items (Show Tab Bar, Show All Tabs) to
/// the View menu: the app has one window. Call it on the main thread before the window and the menus
/// exist; off it, it does nothing and says so.
pub fn no_window_tabs() {
    match MainThreadMarker::new() {
        Some(mtm) => NSWindow::setAllowsAutomaticWindowTabbing(false, mtm),
        None => eprintln!("{}: not on the main thread, so window tabbing stays on", env!("CARGO_PKG_NAME")),
    }
}

/// The system menu bar, while the window runs.
pub struct NativeMenu {
    /// Kept alive: the items below are its.
    _menu: Menu,
    /// Each command's item, by its row, and whether it is enabled now.
    items: Vec<(usize, MenuItem, bool)>,
    clicks: Receiver<Action>,
}

impl NativeMenu {
    /// Builds the bar from `shell`'s menus for the product `product` at `version`, makes it the app's
    /// main menu and tells the shell, so the window draws no bar of its own. A click wakes `ctx`.
    ///
    /// # Errors
    ///
    /// What `muda` ran into; the window keeps its own bar then.
    pub fn install(ctx: &egui::Context, shell: &mut Shell, product: &str, version: &str) -> Result<NativeMenu, String> {
        let about = AboutMetadata { name: Some(product.to_owned()), version: Some(version.to_owned()), ..AboutMetadata::default() };
        let menu = Menu::new();
        let mut items = Vec::new();
        let mut roles = Vec::new();
        for bar_menu in native_menu::bar(product, shell.menus(), shell.rows()) {
            let submenu = submenu(&bar_menu, &about, &mut items).map_err(|e| e.to_string())?;
            menu.append(&submenu).map_err(|e| e.to_string())?;
            roles.push((bar_menu.role, submenu));
        }
        menu.init_for_nsapp();
        for (role, submenu) in &roles {
            match role {
                Role::Window => submenu.set_as_windows_menu_for_nsapp(),
                Role::Help => submenu.set_as_help_menu_for_nsapp(),
                Role::App | Role::Plain => {}
            }
        }
        let (send, clicks) = mpsc::channel();
        let waker = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = Action::from_id(event.id.as_ref()) {
                // The receiver goes only when the window does.
                let _ = send.send(action);
                waker.request_repaint();
            }
        }));
        shell.set_native_menu(true);
        Ok(NativeMenu { _menu: menu, items, clicks })
    }

    /// Runs the items clicked since the last frame: a command's activates its row, Quit closes the
    /// window.
    pub fn run_clicks(&self, shell: &mut Shell, ctx: &egui::Context) {
        while let Ok(action) = self.clicks.try_recv() {
            match action {
                Action::Row(at) => shell.activate(at),
                Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }

    /// Greys out the items whose commands can't run now, and enables the rest: the shell's rows, as
    /// its last refresh left them.
    pub fn show(&mut self, shell: &Shell) {
        for (at, item, shown) in &mut self.items {
            let enabled = shell.rows().get(*at).is_some_and(|row| row.why.is_none());
            if enabled != *shown {
                item.set_enabled(enabled);
                *shown = enabled;
            }
        }
    }
}

/// `bar_menu` as a submenu, its commands' items added to `items`.
fn submenu(bar_menu: &native_menu::Menu, about: &AboutMetadata, items: &mut Vec<(usize, MenuItem, bool)>) -> Result<Submenu, muda::Error> {
    let submenu = Submenu::new(&bar_menu.label, true);
    for entry in &bar_menu.entries {
        match entry {
            Entry::Separator => submenu.append(&PredefinedMenuItem::separator())?,
            Entry::Menu(inner) => submenu.append(&self::submenu(inner, about, items)?)?,
            Entry::System { kind, label } => submenu.append(&system(*kind, label, about))?,
            Entry::Item(item) => {
                let accelerator = item.accelerator.as_deref().and_then(|text| text.parse::<Accelerator>().ok());
                let native = MenuItem::with_id(item.action.id(), &item.label, item.enabled, accelerator);
                submenu.append(&native)?;
                if let Action::Row(at) = item.action {
                    items.push((at, native, item.enabled));
                }
            }
        }
    }
    Ok(submenu)
}

/// The system's own item `kind`, labelled `label`.
fn system(kind: System, label: &str, about: &AboutMetadata) -> PredefinedMenuItem {
    let label = Some(label);
    match kind {
        System::About => PredefinedMenuItem::about(label, Some(about.clone())),
        System::Services => PredefinedMenuItem::services(label),
        System::Hide => PredefinedMenuItem::hide(label),
        System::HideOthers => PredefinedMenuItem::hide_others(label),
        System::ShowAll => PredefinedMenuItem::show_all(label),
        System::Minimize => PredefinedMenuItem::minimize(label),
        System::Zoom => PredefinedMenuItem::zoom(label),
        System::BringAllToFront => PredefinedMenuItem::bring_all_to_front(label),
    }
}

#[cfg(test)]
mod tests {
    use {{crate_prefix_ident}}engine::menu::NAMED_KEYS;
    use {{crate_prefix_ident}}ui::native_menu::accelerator;

    use super::*;

    /// Every accelerator the UI crate makes from a portable shortcut is one `muda` reads, so no item
    /// loses its key silently (`submenu` drops one it can't parse).
    #[test]
    fn muda_reads_every_accelerator() {
        let keys = NAMED_KEYS.split(' ').map(str::to_owned).chain(('A'..='Z').chain('0'..='9').map(String::from)).chain((1..=24).map(|n| format!("F{n}")));
        for key in keys {
            for modifiers in ["", "Mod+", "Ctrl+Alt+", "Mod+Ctrl+Alt+Shift+"] {
                let shortcut = format!("{modifiers}{key}");
                let text = accelerator(&shortcut).unwrap_or_else(|| panic!("{shortcut} has no accelerator"));
                assert!(text.parse::<Accelerator>().is_ok(), "muda can't read {text} (from {shortcut})");
            }
        }
        assert_eq!(accelerator("Mod+Shift+P").and_then(|t| t.parse::<Accelerator>().ok()), "Cmd+Shift+P".parse().ok());
    }
}
