//! The shell's state: the commands it lists, the menu bar made from their menu paths, whether each
//! can run now, the UI's state (the session's: which command's view is open, whether the list is
//! shown), the consent given, the call in flight or its result. It keeps no toolkit type, so it is
//! tested without a window; `draw.rs` only draws it.
//!
//! **The UI's state is data the session holds** (docs/app-architecture.md › The UI is a client):
//! the shell reads it with `ui.inspect` each frame and changes it with `ui.set` when the person
//! clicks, the very calls an agent makes over the socket, so a test or an agent sees and drives what
//! the window shows. Both read and change memory only, as the enabled query does, so the shell makes
//! them on its own thread; a command's own call runs on a worker.

use std::sync::Arc;

use {{crate_prefix_ident}}engine::commands::{UiInspectCommand, UiInspectParams, UiSetCommand, UiSetParams};
use {{crate_prefix_ident}}engine::{CallOpts, Caller, Command, CommandError, Session, UiState};

use crate::draw;
use crate::jobs::{Finished, Jobs, Started, Wake};
use crate::views::{Shown, View};

/// One line of the command list, copied from the registry's spec so drawing reads memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    /// The command's effects need the user's consent before it runs.
    pub needs_consent: bool,
    /// The UI has a view for it ([`crate::views`]); a row without one is listed but can't be run here.
    pub viewable: bool,
    /// Its menu path (`Help/App info`) and shortcut (`Mod+O`), from its spec.
    pub menu: Option<&'static str>,
    pub shortcut: Option<&'static str>,
    /// Why it can't run now (`unavailable`), as of the last [`Shell::refresh`]; `None`: it can.
    pub why: Option<CommandError>,
}

/// A menu of the menu bar, or an item in one: built from the commands' menu paths, so the shell
/// names no command to make its menus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuNode {
    /// The menu's name, or the item's label (a menu path's last part).
    pub label: &'static str,
    /// An item: the row of the command it runs. `None`: a menu, whose entries are `children`.
    pub row: Option<usize>,
    pub children: Vec<MenuNode>,
}

/// The menu bar of `rows`: a menu per first part of their menu paths, in the order the commands
/// first name it, a submenu per middle part, and an item per command.
fn menu_bar(rows: &[Row]) -> Vec<MenuNode> {
    let mut bar: Vec<MenuNode> = Vec::new();
    'rows: for (at, row) in rows.iter().enumerate() {
        let Some(path) = row.menu else { continue };
        let parts: Vec<&'static str> = path.split('/').collect();
        let Some((&label, menus)) = parts.split_last() else { continue };
        let mut level = &mut bar;
        for name in menus.iter().copied() {
            let Some(menu) = submenu(level, name) else { continue 'rows };
            level = &mut menu.children;
        }
        level.push(MenuNode { label, row: Some(at), children: Vec::new() });
    }
    bar
}

/// The menu `name` in `level`, made when it isn't there yet.
fn submenu<'a>(level: &'a mut Vec<MenuNode>, name: &'static str) -> Option<&'a mut MenuNode> {
    if !level.iter().any(|n| n.row.is_none() && n.label == name) {
        level.push(MenuNode { label: name, row: None, children: Vec::new() });
    }
    level.iter_mut().find(|n| n.row.is_none() && n.label == name)
}

/// `rows`' row `at`, when `node` is an item.
pub(crate) fn item_row<'a>(node: &MenuNode, rows: &'a [Row]) -> Option<(usize, &'a Row)> {
    node.row.and_then(|at| rows.get(at).map(|row| (at, row)))
}

/// The state of the selected command's call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// Nothing has been run yet.
    Idle,
    /// Running on a worker: its number, for matching the result that comes back.
    Running { id: &'static str, number: u64 },
    /// Finished: the typed result, as lines.
    Done { id: &'static str, shown: Shown },
    /// Failed: the dispatch's own typed error (a code, a message and what to do about it).
    Failed { id: &'static str, error: CommandError },
}

/// The UI shell: a menu bar, a command list, the UI's state, a consent box and the call's state.
#[derive(Debug)]
pub struct Shell {
    session: Arc<Session>,
    rows: Vec<Row>,
    menus: Vec<MenuNode>,
    views: Vec<View>,
    /// The UI's state as `ui.inspect` last gave it (or `ui.set` returned it).
    state: UiState,
    /// The command whose effects the user agreed to, for its next run: a consent is for one run of
    /// one command, so a selection changed (here or over the socket) leaves it unused.
    consent: Option<&'static str>,
    jobs: Jobs,
    running: Option<Started>,
    call: Call,
}

impl Shell {
    /// A shell over `session`: it lists the session's commands, puts those with a menu path in its
    /// menu bar, and can run those `views` cover. `wake` is called from a worker when a result is
    /// ready (the app repaints the window).
    pub fn new(session: Arc<Session>, views: Vec<View>, wake: Wake) -> Shell {
        let rows: Vec<Row> = session
            .commands()
            .iter()
            .map(|spec| Row {
                id: spec.id,
                title: spec.title,
                summary: spec.summary,
                needs_consent: spec.effects.needs_consent(),
                viewable: views.iter().any(|v| v.id == spec.id),
                menu: spec.menu,
                shortcut: spec.shortcut,
                why: None,
            })
            .collect();
        let menus = menu_bar(&rows);
        let jobs = Jobs::new(Arc::clone(&session), wake);
        let state = UiState::default();
        let mut shell = Shell { session, rows, menus, views, state, consent: None, jobs, running: None, call: Call::Idle };
        shell.refresh();
        shell
    }

    /// The commands, in id order.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The menu bar, made from the commands' menu paths.
    pub fn menus(&self) -> &[MenuNode] {
        &self.menus
    }

    /// The UI's state, as of the last [`Shell::refresh`] or change.
    pub fn state(&self) -> &UiState {
        &self.state
    }

    /// Asks the session which commands can run now, and why not (one call: `Session::states`, the
    /// dispatch's own check, which reads memory only), and the UI's state (`ui.inspect`). Each frame
    /// does, so a menu greys out what a call would refuse and the window shows what an agent set.
    pub fn refresh(&mut self) {
        for (row, state) in self.rows.iter_mut().zip(self.session.states()) {
            if row.id == state.id {
                row.why = state.why;
            }
        }
        match self.session.call_typed::<UiInspectCommand>(UiInspectParams::default(), &CallOpts::new(Caller::Ui)) {
            Ok(state) => self.take(state),
            Err(error) => self.call = Call::Failed { id: UiInspectCommand::ID, error },
        }
    }

    /// `state` becomes the shell's: a consent given before the selection changed is withdrawn.
    fn take(&mut self, state: UiState) {
        if state.selected != self.state.selected {
            self.consent = None;
        }
        self.state = state;
    }

    /// Changes the UI's state with `ui.set`, as an agent would; a refusal is shown as the call's.
    fn set_state(&mut self, params: UiSetParams) {
        match self.session.call_typed::<UiSetCommand>(params, &CallOpts::new(Caller::Ui)) {
            Ok(state) => self.take(state),
            Err(error) => self.call = Call::Failed { id: UiSetCommand::ID, error },
        }
    }

    /// What a menu item does: selects row `index`'s command and runs it, when the UI has a view for
    /// it and it needs no consent; one that needs consent is only selected, for the user to give it.
    /// A disabled command's item is greyed out, and its run is the dispatch's to refuse.
    pub fn activate(&mut self, index: usize) {
        self.select(index);
        let wanted = self.rows.get(index).map(|row| row.id);
        if self.selected().is_some_and(|row| Some(row.id) == wanted && row.viewable && !row.needs_consent) {
            self.run_selected();
        }
    }

    /// The command whose view is open (the UI state's `selected`), when the session has it.
    pub fn selected(&self) -> Option<&Row> {
        let id = self.state.selected.as_deref()?;
        self.rows.iter().find(|row| row.id == id)
    }

    /// Opens row `index`'s view (nothing if there is none): `ui.set` with its id. A call in flight
    /// stays its own, and a consent given for another command is withdrawn.
    pub fn select(&mut self, index: usize) {
        if let Some(id) = self.rows.get(index).map(|row| row.id) {
            self.consent = None;
            self.set_state(UiSetParams { selected: Some(id.to_owned()), show_list: None });
        }
    }

    /// Shows or hides the command list: `ui.set`.
    pub fn set_show_list(&mut self, show: bool) {
        self.set_state(UiSetParams { selected: None, show_list: Some(show) });
    }

    pub fn call(&self) -> &Call {
        &self.call
    }

    /// Whether the user agrees to the effects of the selected command's next run.
    pub fn consent(&self) -> bool {
        self.consent.is_some() && self.consent == self.selected().map(|row| row.id)
    }

    /// Whether the user agrees to the effects of what they run next: one run, of the selected command.
    pub fn set_consent(&mut self, consent: bool) {
        self.consent = if consent { self.selected().map(|row| row.id) } else { None };
    }

    /// Whether a call is running now.
    pub fn is_running(&self) -> bool {
        matches!(self.call, Call::Running { .. })
    }

    /// Runs the selected command's view on a worker and returns at once. Nothing happens when no
    /// viewable command is selected or a call is already running. The consent given is used by this
    /// run and then withdrawn: the next run asks again.
    pub fn run_selected(&mut self) {
        let Some(row) = self.selected().cloned() else { return };
        let Some(view) = self.views.iter().find(|v| v.id == row.id).copied() else { return };
        if self.is_running() {
            return;
        }
        let opts = CallOpts::new(Caller::Ui);
        let opts = if self.consent() { opts.consenting() } else { opts };
        self.consent = None;
        self.call = match self.jobs.start(view, opts) {
            Ok(started) => {
                let number = started.call;
                self.running = Some(started);
                Call::Running { id: row.id, number }
            }
            Err(error) => Call::Failed { id: row.id, error },
        };
    }

    /// Asks the running call to stop (the command polls its token and answers `cancelled`).
    pub fn cancel(&self) {
        if let Some(started) = &self.running {
            started.cancel.cancel();
        }
    }

    /// Takes in the results that have arrived: the one that is the running call's becomes the
    /// state, anything else (a call this shell no longer waits for) is dropped. Called each frame.
    pub fn poll(&mut self) {
        for Finished { call, id, result } in self.jobs.poll() {
            if self.running.as_ref().is_some_and(|s| s.call == call) {
                self.running = None;
                self.call = match result {
                    Ok(shown) => Call::Done { id, shown },
                    Err(error) => Call::Failed { id, error },
                };
            }
        }
    }

    /// Draws the shell into `ui`: it takes in what workers have finished, which commands can run
    /// now and the UI's state, then draws them.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll();
        self.refresh();
        draw::shell(self, ui);
    }
}

#[cfg(test)]
mod tests;
