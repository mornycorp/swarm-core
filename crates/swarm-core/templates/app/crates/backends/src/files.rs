//! Files (docs/app-architecture.md › State on disk): **the one atomic writer**, and the
//! capped read that goes with it. Everything {{product}} persists (settings, the UI's state, a
//! cache, a journal) is written through [`write_atomic`], by the one module that owns that file;
//! the `file-writes` scan holds every other file of the app to that.
//!
//! - **Atomic:** the bytes go to a temporary file in the same directory, are flushed to the disk,
//!   and the file is renamed over the old one. A crash leaves the old file or the new, never half
//!   of one, and a failed write leaves the old file as it was.
//! - **Capped:** [`read_capped`] never reads more than it is told to: a file that grew huge (or was
//!   swapped for one) is `too_large`, not an allocation sized by the disk.
//! - **Nothing here knows a format.** The owner of a file gives it its `schema_version`, refuses a
//!   newer one, migrates an older one, and saves before it changes memory. A read that fails
//!   leaves the person's file alone: the owner never writes defaults over what it couldn't read.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use crate::BackendError;

/// Writes `bytes` to `path` atomically: a temporary file beside it, flushed, then renamed over
/// it. The directory must exist. On a failure the old file is as it was and the temporary one is
/// removed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BackendError> {
    let temp = temporary(path)?;
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        BackendError::failed(format!("couldn't save {}: {e}", path.display()))
    })
}

/// The temporary file [`write_atomic`] writes for `path`: hidden, beside it (a rename across
/// directories may not be atomic), and this process's own, so two processes saving the same file
/// never write the same temporary one.
fn temporary(path: &Path) -> Result<PathBuf, BackendError> {
    let name = path.file_name().ok_or_else(|| BackendError::failed(format!("{} names no file to save", path.display())))?;
    Ok(path.with_file_name(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id())))
}

/// The text of the file at `path`, `None` when there is none yet. More than `max` bytes is
/// `too_large`, and text that isn't UTF-8 or a file that can't be read is an error: neither is
/// ever read as "no file", so its owner doesn't start from defaults and save over it.
pub fn read_capped(path: &Path, max: u64) -> Result<Option<String>, BackendError> {
    let unreadable = |e: &dyn std::fmt::Display| BackendError::failed(format!("couldn't read {}: {e}", path.display()));
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(&e)),
    };
    let mut text = String::new();
    file.take(max.saturating_add(1)).read_to_string(&mut text).map_err(|e| unreadable(&e))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > max {
        return Err(BackendError::too_large(
            format!("{} is over {max} bytes", path.display()),
            "it is larger than this state ever is: move it aside and look at what wrote it",
        ));
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    fn dir(name: &str) -> PathBuf {
        {{crate_prefix_ident}}testkit::temp_dir(name).unwrap()
    }

    #[test]
    fn what_is_written_reads_back_and_leaves_no_temporary_file() {
        let dir = dir("state-round-trip");
        let path = dir.join("settings.json");
        assert_eq!(read_capped(&path, 100), Ok(None), "no file yet");
        write_atomic(&path, b"{\"schema_version\": 1}").unwrap();
        write_atomic(&path, b"{\"schema_version\": 2}").unwrap();
        assert_eq!(read_capped(&path, 100).unwrap().as_deref(), Some("{\"schema_version\": 2}"));
        let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["settings.json"], "the temporary file is gone");
    }

    /// A write that fails leaves the old file as it was: here the target is a directory, which a
    /// file can't be renamed over.
    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_one() {
        let dir = dir("state-failed-write");
        let path = dir.join("taken");
        std::fs::create_dir_all(path.join("inside")).unwrap();
        let e = write_atomic(&path, b"new").unwrap_err();
        assert_eq!(e.kind, Kind::Failed);
        assert!(e.message.contains("couldn't save") && path.join("inside").is_dir(), "{e}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
        assert!(write_atomic(Path::new(""), b"x").is_err(), "a path that names no file");
        assert!(write_atomic(&dir.join("no-such-dir").join("f"), b"x").is_err(), "a directory that isn't there");
    }

    /// A file over the cap, or one that isn't text, is an error, never "no file": its owner must
    /// not start from defaults and save over it.
    #[test]
    fn a_file_that_cant_be_read_whole_is_an_error_not_an_absent_file() {
        let dir = dir("state-read");
        let big = dir.join("big.json");
        write_atomic(&big, &[b'a'; 11]).unwrap();
        assert_eq!(read_capped(&big, 10).unwrap_err().kind, Kind::TooLarge);
        assert_eq!(read_capped(&big, 11).unwrap().map(|t| t.len()), Some(11));
        let binary = dir.join("binary");
        write_atomic(&binary, &[0xff, 0xfe]).unwrap();
        assert_eq!(read_capped(&binary, 10).unwrap_err().kind, Kind::Failed);
        assert!(read_capped(&dir, 10).is_err(), "a directory is no file to read");
    }
}
