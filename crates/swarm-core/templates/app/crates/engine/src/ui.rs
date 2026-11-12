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
use std::sync::{Arc, RwLock, TryLockError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use {{crate_prefix_ident}}model::SCHEMA_VERSION;

use crate::error::CommandError;

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
}

impl Default for UiState {
    fn default() -> UiState {
        UiState { schema_version: SCHEMA_VERSION, selected: None, show_list: true }
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
        let state: UiState = serde_json::from_str(text).map_err(malformed)?;
        Ok(UiState { schema_version: SCHEMA_VERSION, ..state })
    }

    /// The state as the window saves it: JSON, one field a line.
    pub fn to_json(&self) -> Result<String, UiStateError> {
        serde_json::to_string_pretty(self).map_err(|e| UiStateError::Malformed(e.to_string()))
    }
}

/// What runs when the UI state changes: the window repaints, so a change made over the socket shows.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

/// How long a caller waits for the UI state's lock before it calls it a deadlock.
const LOCK_DEADLINE: Duration = Duration::from_secs(2);

/// The state, and what to call when it changes.
struct Held {
    state: UiState,
    on_change: Option<OnChange>,
}

/// The session's UI state behind its lock (docs/app-architecture.md › Never hang on a lock): it is
/// held only to copy or change the state, never across a call, and a caller that can't take it
/// within [`LOCK_DEADLINE`] gets an `internal` error that names it, never a wait for ever.
pub(crate) struct UiCell {
    held: RwLock<Held>,
}

impl fmt::Debug for UiCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiCell").finish_non_exhaustive()
    }
}

impl UiCell {
    pub(crate) fn new() -> UiCell {
        UiCell { held: RwLock::new(Held { state: UiState::default(), on_change: None }) }
    }

    /// A copy of the state now.
    pub(crate) fn get(&self) -> Result<UiState, CommandError> {
        self.with(|held| held.state.clone())
    }

    /// Changes the state with `change`, and calls what watches it once the lock is let go: the new
    /// state.
    pub(crate) fn update(&self, change: impl FnOnce(&mut UiState)) -> Result<UiState, CommandError> {
        let (state, on_change) = self.with(|held| {
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
        self.with(|held| held.on_change = Some(on_change))
    }

    /// `f` with the lock held. A poisoned lock is taken all the same: the state is plain data, and
    /// every change writes whole fields, so a panic can't leave it half-made.
    fn with<R>(&self, f: impl FnOnce(&mut Held) -> R) -> Result<R, CommandError> {
        let start = Instant::now();
        loop {
            match self.held.try_write() {
                Ok(mut held) => return Ok(f(&mut held)),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(f(&mut poisoned.into_inner())),
                Err(TryLockError::WouldBlock) if start.elapsed() < LOCK_DEADLINE => std::thread::sleep(Duration::from_millis(1)),
                Err(TryLockError::WouldBlock) => {
                    return Err(CommandError::internal(format!("the UI state's lock was held for over {} s: a deadlock", LOCK_DEADLINE.as_secs())));
                }
            }
        }
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
        let held = cell.held.write().unwrap();
        let other = Arc::clone(&cell);
        let error = std::thread::spawn(move || other.get().unwrap_err()).join().unwrap();
        drop(held);
        assert!(error.code == crate::ErrorCode::Internal && error.message.contains("UI state's lock"), "{error}");
        assert!(cell.get().is_ok(), "free again");
    }
}
