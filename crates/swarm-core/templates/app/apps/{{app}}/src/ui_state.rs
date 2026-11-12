//! The UI's state on disk (docs/app-architecture.md › State on disk, The UI is a client): read into
//! the session when the window opens, written from it when the window closes, at the file
//! `--ui-state` names. This module is the one that reads and writes that file.
//!
//! - **Versioned:** the file is the engine's `UiState` as JSON, with its `schema_version`; one from a
//!   newer app is refused (`UiState::parse`), never overwritten.
//! - **Never defaults over what it couldn't read:** a file that can't be read or parsed is left as it
//!   is, and the window doesn't write it when it closes ([`restore`] says so with an `Err`).
//! - **Atomic:** [`save`] writes a temporary file beside it, flushes it and renames it over, so a
//!   crash leaves the old file or the new, never half of one.
//!
//! The state is saved whole when the window closes, not at each change: it changes with each click,
//! and a crash then costs the person a layout. A setting is different: it is saved before memory
//! changes, so a failed save never leaves the screen saying what the file doesn't.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;

use {{crate_prefix_ident}}engine::{Session, UiState};

/// The largest file read back: a UI state is a few lines.
const MAX_BYTES: u64 = 1024 * 1024;

/// Reads the state saved at `path` into `session`. `Ok`: the window writes it back when it closes
/// (it was read, or there is no file yet). `Err` says why it wasn't read: the window starts from the
/// default and leaves the file alone.
pub fn restore(session: &Session, path: &Path) -> Result<(), String> {
    let Some(text) = read(path)? else { return Ok(()) };
    let saved = UiState::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    session.restore_ui(saved).map(drop).map_err(|e| e.message)
}

/// The file's text, `None` when there is none yet.
fn read(path: &Path) -> Result<Option<String>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let mut text = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut text).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_BYTES {
        return Err(format!("{} is over {MAX_BYTES} bytes, more than a UI state is", path.display()));
    }
    Ok(Some(text))
}

/// Writes `session`'s UI state to `path`, atomically: a temporary file beside it, flushed, then
/// renamed over it.
pub fn save(session: &Session, path: &Path) -> Result<(), String> {
    let text = session.ui().map_err(|e| e.message)?.to_json().map_err(|e| e.to_string())?;
    let name = path.file_name().ok_or_else(|| format!("{} names no file", path.display()))?;
    let temp = path.with_file_name(format!(".{}.tmp", name.to_string_lossy()));
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot save the UI state to {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use {{crate_prefix_ident}}engine::AppInfo;

    fn session() -> Session {
        Session::new(AppInfo { name: "demo".into(), version: "0.3.0".into() }).unwrap()
    }

    #[test]
    fn a_saved_state_comes_back_in_the_next_session() {
        let dir = {{crate_prefix_ident}}testkit::temp_dir("window-ui-state").unwrap();
        let path = dir.join("ui-state.json");
        let first = session();
        assert_eq!(restore(&first, &path), Ok(()), "no file yet: the default, and saved on close");
        let state = UiState { selected: Some("app.info".into()), show_list: false, ..UiState::default() };
        first.restore_ui(state.clone()).unwrap();
        save(&first, &path).unwrap();
        let next = session();
        restore(&next, &path).unwrap();
        assert_eq!(next.ui().unwrap(), state);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file is left");
    }

    #[test]
    fn a_file_that_cannot_be_read_is_refused_and_left_alone() {
        let dir = {{crate_prefix_ident}}testkit::temp_dir("window-ui-state-bad").unwrap();
        let path = dir.join("ui-state.json");
        let newer = format!("{{\"schema_version\": {}, \"selected\": null, \"show_list\": true}}", u32::MAX);
        for text in ["{not json", newer.as_str(), &"x".repeat(1024 * 1024 + 1)] {
            std::fs::write(&path, text).unwrap();
            let s = session();
            assert!(restore(&s, &path).is_err(), "{}", text.get(..40).unwrap_or(text));
            assert_eq!(s.ui().unwrap(), UiState::default(), "the default, not half a state");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "the file is the person's: untouched");
        }
    }
}
