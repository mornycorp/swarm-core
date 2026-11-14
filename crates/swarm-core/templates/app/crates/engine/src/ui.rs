//! The UI's state as plain data (docs/app-architecture.md › The UI is a client): what a person has
//! open and selected, and how the window is laid out. It is the session's, not the UI crate's, so
//! `ui.inspect` and `ui.set` ([`crate::commands`]) read and set it from every surface (the CLI, the
//! socket, MCP, a test) and a session with no window has one too. The UI shell draws it each frame
//! and changes it through `ui.set`, as an agent does. It holds no toolkit type and nothing the
//! toolkit works out (a pixel, a hover): only what a person chose, which a restart brings back.
//!
//! It is versioned with the rest of the app's state ([`UiState::schema_version`]; [`UiState::parse`]
//! refuses a newer file), and the window saves it when it closes and restores it when it starts.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use {{crate_prefix_ident}}model::SCHEMA_VERSION;

use crate::error::{CommandError, clip};
use crate::lock::Lock;

/// The UI's state. An app adds what its views keep (the open panes, a scroll offset, a column's
/// width, a filter), each a plain value, and `ui.set` a param for each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiState {
    /// The version of this shape (`model::SCHEMA_VERSION`): a saved state from a newer app is refused.
    pub schema_version: u32,
    /// The command whose view is open, by id; none when nothing is selected.
    pub selected: Option<String>,
    /// Whether the command list is shown beside the open view.
    pub show_list: bool,
    /// Where the dock's panes are; none for the UI's default layout ([`DockNode::default_layout`]).
    #[serde(default)]
    pub dock: Option<DockNode>,
}

impl Default for UiState {
    fn default() -> UiState {
        UiState { schema_version: SCHEMA_VERSION, selected: None, show_list: true, dock: None }
    }
}

/// The dock's panes, by id (the App UI Substrate › The dock): the command list, and the open
/// command's view. An app adds an id for each pane it adds, and its UI a pane for each.
pub const PANES: [&str; 2] = [PANE_COMMANDS, PANE_VIEW];
/// The command list's pane.
pub const PANE_COMMANDS: &str = "commands";
/// The open command's view's pane.
pub const PANE_VIEW: &str = "view";

/// How deep a layout may nest: a split in a split, to this depth.
pub const DOCK_DEPTH: usize = 6;
/// The most tabs, splits and panes a layout may hold, all told.
pub const DOCK_ITEMS: usize = 32;
/// The largest share a split's part may have; a part's space is its share of its split's total.
pub const DOCK_SHARE: u16 = 1000;

/// Where the dock's panes are: tabs of panes, or a split into parts side by side or one above
/// another. The UI builds its dock from this whenever it changes, and writes back what a person
/// drags; the dock's own saved state is never loaded. Every layout [`DockNode::check`] accepts
/// builds a dock: a pane left out of it is added, and one named twice is shown once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DockNode {
    /// Panes as tabs, by id ([`PANES`]), and the index of the one shown.
    Tabs { panes: Vec<String>, active: u32 },
    /// Parts side by side (`across`) or one above another, in order.
    Split { across: bool, parts: Vec<DockPart> },
}

/// One part of a split: its share of the split's space, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DockPart {
    /// Its share: its space is this over the split's total (1 to [`DOCK_SHARE`]).
    pub share: u16,
    pub node: DockNode,
}

impl DockNode {
    /// The layout the UI starts with: the command list on the left, a third of the width, and the
    /// open command's view beside it.
    pub fn default_layout() -> DockNode {
        let tabs = |pane: &str| DockNode::Tabs { panes: vec![pane.to_owned()], active: 0 };
        DockNode::Split { across: true, parts: vec![DockPart { share: 300, node: tabs(PANE_COMMANDS) }, DockPart { share: 700, node: tabs(PANE_VIEW) }] }
    }

    /// Why this layout is refused, if it is: deeper than [`DOCK_DEPTH`], more than [`DOCK_ITEMS`]
    /// items, a split with no parts or a share out of range, tabs with no panes, or a pane that isn't
    /// one of [`PANES`].
    pub fn check(&self) -> Result<(), CommandError> {
        let mut items = 0;
        self.check_at(1, &mut items)
    }

    fn check_at(&self, depth: usize, items: &mut usize) -> Result<(), CommandError> {
        *items = items.saturating_add(1);
        if depth > DOCK_DEPTH || *items > DOCK_ITEMS {
            return Err(CommandError::too_large(
                format!("the dock's layout nests deeper than {DOCK_DEPTH} or holds more than {DOCK_ITEMS} tabs, splits and panes"),
                "give a smaller layout",
            ));
        }
        match self {
            DockNode::Tabs { panes, .. } => Self::check_panes(panes, items),
            DockNode::Split { parts, .. } => {
                if parts.is_empty() || parts.iter().any(|p| p.share == 0 || p.share > DOCK_SHARE) {
                    return Err(CommandError::bad_params(
                        "a split in the dock's layout has no parts, or a part's share is 0 or over 1000",
                        "give each split one part or more, each with a share from 1 to 1000",
                    ));
                }
                parts.iter().try_for_each(|part| part.node.check_at(depth + 1, items))
            }
        }
    }

    fn check_panes(panes: &[String], items: &mut usize) -> Result<(), CommandError> {
        if panes.is_empty() {
            return Err(CommandError::bad_params("tabs in the dock's layout have no panes", "give tabs one pane or more"));
        }
        *items = items.saturating_add(panes.len());
        if *items > DOCK_ITEMS {
            return Err(CommandError::too_large(format!("the dock's layout holds more than {DOCK_ITEMS} tabs, splits and panes"), "give a smaller layout"));
        }
        match panes.iter().find(|pane| !PANES.contains(&pane.as_str())) {
            Some(pane) => {
                Err(CommandError::not_found(format!("the dock has no pane `{}`", clip(pane)), format!("name the panes the dock has: {}", PANES.join(", "))))
            }
            None => Ok(()),
        }
    }
}

/// Why a saved UI state wasn't read (the window then starts from the default, and leaves the file
/// alone), or couldn't be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiStateError {
    /// It isn't the shape this version reads or writes: the parser's message, with its line and column.
    Malformed(String),
    /// A newer version of the app wrote it: this one can't read it, and won't write over it.
    Newer { found: u32, supported: u32 },
}

impl fmt::Display for UiStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiStateError::Malformed(why) => write!(f, "the UI state is not the shape this version reads: {why}"),
            UiStateError::Newer { found, supported } => {
                write!(f, "the UI state is version {found}, from a newer app; this one reads up to version {supported}")
            }
        }
    }
}

impl std::error::Error for UiStateError {}

impl UiState {
    /// A saved state (`text`, JSON), as this version keeps it: one from a newer version is refused,
    /// one from an older version is migrated (every version so far has this shape; a change to it
    /// bumps `SCHEMA_VERSION` and adds its migration here).
    pub fn parse(text: &str) -> Result<UiState, UiStateError> {
        /// Only the version, read first: a newer file may have fields this version doesn't know.
        #[derive(Deserialize)]
        struct Versioned {
            schema_version: u32,
        }
        let malformed = |e: serde_json::Error| UiStateError::Malformed(e.to_string());
        let found = serde_json::from_str::<Versioned>(text).map_err(malformed)?.schema_version;
        if found > SCHEMA_VERSION {
            return Err(UiStateError::Newer { found, supported: SCHEMA_VERSION });
        }
        // Version 1 had no dock: its file reads as one with the default layout.
        let state: UiState = serde_json::from_str(text).map_err(malformed)?;
        if let Some(dock) = &state.dock {
            dock.check().map_err(|e| UiStateError::Malformed(format!("the dock's layout: {}", e.message)))?;
        }
        Ok(UiState { schema_version: SCHEMA_VERSION, ..state })
    }

    /// The state as the window saves it: JSON, one field a line.
    pub fn to_json(&self) -> Result<String, UiStateError> {
        serde_json::to_string_pretty(self).map_err(|e| UiStateError::Malformed(e.to_string()))
    }
}

/// What runs when the UI state changes: the window repaints, so a change made over the socket shows.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

/// The state, and what to call when it changes.
struct Held {
    state: UiState,
    on_change: Option<OnChange>,
}

/// The session's UI state behind the app's lock ([`Lock`]; docs/app-architecture.md › Never hang on
/// a lock): it is held only to copy or change the state, never across a call, and a caller that
/// can't take it within the lock's deadline gets an `internal` error that names it, never a wait
/// for ever.
pub(crate) struct UiCell {
    held: Lock<Held>,
}

impl fmt::Debug for UiCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiCell").finish_non_exhaustive()
    }
}

impl UiCell {
    pub(crate) fn new() -> UiCell {
        UiCell { held: Lock::new("the UI state", Held { state: UiState::default(), on_change: None }) }
    }

    /// A copy of the state now.
    pub(crate) fn get(&self) -> Result<UiState, CommandError> {
        self.held.with(|held| held.state.clone())
    }

    /// Changes the state with `change`, and calls what watches it once the lock is let go: the new
    /// state.
    pub(crate) fn update(&self, change: impl FnOnce(&mut UiState)) -> Result<UiState, CommandError> {
        let (state, on_change) = self.held.with(|held| {
            change(&mut held.state);
            (held.state.clone(), held.on_change.clone())
        })?;
        if let Some(on_change) = on_change {
            on_change();
        }
        Ok(state)
    }

    /// Calls `on_change` after each change from now on (the window's repaint).
    pub(crate) fn watch(&self, on_change: OnChange) -> Result<(), CommandError> {
        self.held.with(|held| held.on_change = Some(on_change))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_saved_state_reads_back_and_a_newer_or_malformed_one_is_refused() {
        let state = UiState { selected: Some("app.info".into()), show_list: false, ..UiState::default() };
        assert_eq!(UiState::parse(&state.to_json().unwrap()), Ok(state));
        let newer = format!("{{\"schema_version\": {}, \"selected\": null, \"show_list\": true, \"panes\": 3}}", SCHEMA_VERSION + 1);
        assert_eq!(UiState::parse(&newer), Err(UiStateError::Newer { found: SCHEMA_VERSION + 1, supported: SCHEMA_VERSION }));
        for bad in ["", "{", "[]", "{\"schema_version\": \"1\"}", "{\"schema_version\": 1}", "{\"schema_version\": 1, \"selected\": 5, \"show_list\": true}"] {
            assert!(matches!(UiState::parse(bad), Err(UiStateError::Malformed(_))), "{bad}");
        }
        let unknown = format!("{{\"schema_version\": {SCHEMA_VERSION}, \"selected\": null, \"show_list\": true, \"extra\": 1}}");
        assert!(matches!(UiState::parse(&unknown), Err(UiStateError::Malformed(why)) if why.contains("extra")));
        assert!(UiStateError::Newer { found: 9, supported: 1 }.to_string().contains("newer app"));
    }

    /// A version-1 file (no dock) reads with the default layout, and a file whose dock the UI couldn't
    /// build is refused as malformed, like any other it can't read.
    #[test]
    fn an_older_file_reads_and_a_bad_dock_is_refused() {
        let v1 = "{\"schema_version\": 1, \"selected\": \"app.info\", \"show_list\": false}";
        assert_eq!(UiState::parse(v1), Ok(UiState { selected: Some("app.info".into()), show_list: false, ..UiState::default() }));
        let kept = UiState { dock: Some(DockNode::default_layout()), ..UiState::default() };
        assert_eq!(UiState::parse(&kept.to_json().unwrap()), Ok(kept));
        let bad = format!(
            "{{\"schema_version\": {SCHEMA_VERSION}, \"selected\": null, \"show_list\": true, \"dock\": {{\"kind\": \"tabs\", \"panes\": [\"nope\"], \"active\": 0}}}}"
        );
        assert!(matches!(UiState::parse(&bad), Err(UiStateError::Malformed(why)) if why.contains("nope")));
    }

    #[test]
    fn a_change_is_seen_and_what_watches_it_is_called_after() {
        let cell = UiCell::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        cell.watch(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        let changed = cell.update(|s| s.show_list = false).unwrap();
        assert_eq!((changed.show_list, cell.get().unwrap(), calls.load(Ordering::SeqCst)), (false, changed.clone(), 1));
    }

    #[test]
    fn a_lock_held_too_long_is_an_internal_error_not_a_hang() {
        let cell = Arc::new(UiCell::new());
        let other = Arc::clone(&cell);
        // Asked for while this thread holds it: the other gives up at the lock's deadline.
        let error = cell.held.with(|_| std::thread::spawn(move || other.get().unwrap_err()).join().unwrap()).unwrap();
        assert!(error.code == crate::ErrorCode::Internal && error.message.contains("UI state's lock"), "{error}");
        assert!(cell.get().is_ok(), "free again");
    }
}
