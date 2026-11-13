//! Files (docs/app-architecture.md › State on disk): **the one atomic writer**, and the capped read
//! beside it. Every file {{product}} writes (its settings, the UI's state, a cache, the control
//! socket's token) is written here, at the call of the one module that owns that file and decides
//! what is in it and when; the `file-writes` scan holds every other crate but the testkit and the
//! UI crate (which writes nothing at all) to that.
//!
//! - **Atomic** ([`write_atomic`]): the bytes go to a new temporary file in the same directory (a
//!   rename across directories may not be atomic), are flushed to the disk, and the file is renamed
//!   over the old one. A crash at any point leaves the old file or the new, never half of one, and a
//!   write that fails leaves the old file as it was and no temporary file behind.
//! - **Private when asked** ([`Access::Private`]): the temporary file is made readable and writable
//!   by its owner only (`0600` on Unix) before a byte is written, so a secret is never readable by
//!   others, not even for a moment. On Windows `std` sets no ACL: the file gets its directory's, so
//!   a secret's directory must be one only its user can read.
//! - **Never over what is there, when asked** ([`Existing::Keep`]): the file is linked into place,
//!   which fails when anything is at the path (a file, a link, a link to nowhere), so a file someone
//!   else made first is kept and never followed.
//! - **Patient with a scanner, on Windows only** (`BUSY_PAUSES`): a rename, a link or a removal
//!   refused with `PermissionDenied` is tried again, with doubling pauses for about 2 s in all, as a
//!   virus scanner or the indexer holds a file just written for a moment (up to 1.5 s, measured).
//!   Nothing here retries without a bound.
//! - **Capped** ([`read_capped`]): a read never takes more than it is told to. A file that grew huge,
//!   or was swapped for one, is `too_large`, not an allocation the disk sizes; a FIFO or a device at
//!   the path is refused before it is opened, so a read never blocks.
//! - **It knows no format.** The owner of a file gives it its `schema_version`, refuses a newer
//!   one, migrates an older one and saves before it changes memory. A read that fails is never "no
//!   file": the owner keeps its defaults in memory and leaves the person's file alone.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::BackendError;

/// Who may read a file the writer makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// As the process's defaults make it (`0666` less the umask on Unix; the directory's ACL on
    /// Windows).
    Shared,
    /// Its owner only: `0600` (a directory `0700`) on Unix. Windows: the directory's ACL, as
    /// [`Access::Shared`] (`std` sets none).
    Private,
}

/// What becomes of a file already at the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// Replaced, in one rename.
    Replace,
    /// Kept as it is, and nothing written: [`Written::Kept`].
    Keep,
}

/// What a write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// The bytes are at the path.
    Wrote,
    /// Something was already there, and was kept ([`Existing::Keep`]).
    Kept,
}

/// Writes `bytes` to `path` atomically, over the file there, as the process's defaults make a file:
/// what almost every owner of a file wants. [`write_atomic_as`] says who may read it, and whether
/// a file already there is kept.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BackendError> {
    write_atomic_as(path, bytes, Access::Shared, Existing::Replace).map(drop)
}

/// Writes `bytes` to `path` atomically: a new temporary file beside it, made with `access`,
/// written, flushed to the disk and closed, then renamed over the path ([`Existing::Replace`]) or
/// linked into place only where nothing is ([`Existing::Keep`]). The directory must exist
/// ([`make_dir`]). A failure says what failed, and leaves the path as it was and
/// no temporary file.
pub fn write_atomic_as(path: &Path, bytes: &[u8], access: Access, existing: Existing) -> Result<Written, BackendError> {
    let (temp, mut file) = temporary(path, access)?;
    let written = fill(&mut file, bytes).and_then(|()| {
        drop(file);
        put_in_place(&temp, path, existing)
    });
    if !matches!(written, Ok(Written::Wrote) if existing == Existing::Replace) {
        let _ = patiently(|| fs::remove_file(&temp));
    }
    let written = written.map_err(|e| failed("save", path, &e))?;
    sync_dir(path);
    Ok(written)
}

/// The pauses before each retry of a rename, a link or a removal that Windows refused with
/// `PermissionDenied`: doubling from 5 ms, about 2 s in all, on Windows; none elsewhere. On Windows
/// another process (a virus scanner, the search indexer) opens a file just written, without
/// sharing its deletion, and a rename over it, a link to it or its removal is refused until it lets
/// go. Measured on the skeleton's tests, four runs at once: without a retry 4
/// runs in 200 failed so; nearly every run met one refusal that cleared within 15 ms, and 7 in 400
/// needed 0.75 to 1.55 s, so a bound of 155 ms still failed 5 in 400. A refusal that outlasts the
/// pauses is taken as real, and fails. On Unix the refusal is the permissions' own, and is never
/// retried.
const BUSY_PAUSES: &[Duration] = if cfg!(windows) {
    &[
        Duration::from_millis(5),
        Duration::from_millis(10),
        Duration::from_millis(20),
        Duration::from_millis(40),
        Duration::from_millis(80),
        Duration::from_millis(160),
        Duration::from_millis(320),
        Duration::from_millis(640),
        Duration::from_millis(800),
    ]
} else {
    &[]
};

/// `op`, tried again after each of [`BUSY_PAUSES`] while it fails with `PermissionDenied`: at most
/// `1 + BUSY_PAUSES.len()` tries, never a loop without a bound.
fn patiently<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut result = op();
    for pause in BUSY_PAUSES {
        if !matches!(&result, Err(e) if e.kind() == ErrorKind::PermissionDenied) {
            break;
        }
        std::thread::sleep(*pause);
        result = op();
    }
    result
}

/// The bytes, written and flushed to the disk.
fn fill(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

/// Moves the full temporary file to `path`: a rename over what is there, or a link that fails when
/// anything is (the temporary file is then removed by the caller).
fn put_in_place(temp: &Path, path: &Path, existing: Existing) -> io::Result<Written> {
    match existing {
        Existing::Replace => patiently(|| fs::rename(temp, path)).map(|()| Written::Wrote),
        Existing::Keep => match patiently(|| fs::hard_link(temp, path)) {
            Ok(()) => Ok(Written::Wrote),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(Written::Kept),
            Err(e) => Err(e),
        },
    }
}

/// How many temporary names a write tries before it gives up: a name is taken only by a write in
/// flight or one a crash left behind.
const TEMPORARY_TRIES: u32 = 16;

/// Tells this process's writes apart, so two threads saving one file never share a temporary one.
static WRITES: AtomicU64 = AtomicU64::new(0);

/// A new temporary file beside `path`, made with `access`: hidden, named for the file, this process
/// and this write, and made only where nothing is (`create_new`), so a file or a link planted at
/// that name is never written through.
fn temporary(path: &Path, access: Access) -> Result<(PathBuf, File), BackendError> {
    let name = path
        .file_name()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| BackendError::failed(format!("`{}` names no file to save", path.display()), "name a file, not a directory or an empty path"))?;
    let mut last = io::Error::from(ErrorKind::AlreadyExists);
    for _ in 0..TEMPORARY_TRIES {
        let write = WRITES.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_file_name(format!(".{}.{}-{write}.tmp", name.to_string_lossy(), std::process::id()));
        match new_file(&temp, access) {
            Ok(file) => return Ok((temp, file)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(failed("save", path, &e)),
        }
    }
    Err(failed("save", path, &last))
}

/// A file made at `path` only where nothing is, with `access`.
fn new_file(path: &Path, access: Access) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if access == Access::Private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    #[cfg(not(unix))]
    let _ = access;
    options.open(path)
}

/// Unix: flushes the directory of `path`, so the rename itself survives a power cut. Best effort:
/// the file is in place either way, and Windows has no handle to flush a directory by.
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = File::open(dir).and_then(|d| d.sync_all());
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Makes the directory `dir` and those above it, as `access` says (`0700` on Unix when private).
/// Nothing is changed when it is already there.
pub fn make_dir(dir: &Path, access: Access) -> Result<(), BackendError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    if access == Access::Private {
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    }
    #[cfg(not(unix))]
    let _ = access;
    builder.create(dir).map_err(|e| failed("make the directory", dir, &e))
}

/// Removes the file at `path`; none there is no failure.
pub fn remove(path: &Path) -> Result<(), BackendError> {
    match patiently(|| fs::remove_file(path)) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(failed("remove", path, &e)),
        _ => Ok(()),
    }
}

/// The bytes of the file at `path`, `None` when there is none. More than `max` bytes is
/// `too_large`; a path that isn't a regular file (a directory, a FIFO, a device) is refused before
/// it is opened, so the read never blocks; and a file that can't be read is an error. None of
/// these is ever "no file": its owner doesn't start from defaults and save them over it.
pub fn read_capped(path: &Path, max: u64) -> Result<Option<Vec<u8>>, BackendError> {
    match fs::metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(failed("read", path, &e)),
        Ok(meta) if !meta.is_file() => {
            return Err(BackendError::failed(format!("{} is not a regular file", path.display()), "point it at a file, or move what is there aside"));
        }
        Ok(_) => {}
    }
    let file = File::open(path).map_err(|e| failed("read", path, &e))?;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| failed("read", path, &e))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max {
        return Err(BackendError::too_large(
            format!("{} is over {max} bytes, more than this file ever holds", path.display()),
            "move it aside and look at what wrote it; the app starts without it",
        ));
    }
    Ok(Some(bytes))
}

/// [`read_capped`], as UTF-8 text: a file that isn't is an error too.
pub fn read_capped_text(path: &Path, max: u64) -> Result<Option<String>, BackendError> {
    let Some(bytes) = read_capped(path, max)? else { return Ok(None) };
    String::from_utf8(bytes).map(Some).map_err(|e| {
        BackendError::failed(format!("{} is not UTF-8 text: {e}", path.display()), "move it aside and look at what wrote it; the app starts without it")
    })
}

/// A file operation on `path` that failed with `e`, with what to look at: `not_found` for a path that isn't there.
fn failed(what: &str, path: &Path, e: &io::Error) -> BackendError {
    if e.kind() == ErrorKind::NotFound {
        return BackendError::not_found(format!("couldn't {what} {}: {e}", path.display()), "make its directory first, or name one that exists");
    }
    BackendError::failed(
        format!("couldn't {what} {}: {e}", path.display()),
        "check that the directory exists and can be written, that nothing else holds the file, and that the disk has room",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    fn dir(name: &str) -> PathBuf {
        {{crate_prefix_ident}}testkit::temp_dir(name).unwrap()
    }

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn what_is_written_reads_back_and_no_temporary_file_is_left() {
        let dir = dir("state-round-trip");
        let path = dir.join("settings.json");
        assert_eq!(read_capped(&path, 100), Ok(None), "no file yet");
        write_atomic(&path, b"{\"schema_version\": 1}").unwrap();
        write_atomic(&path, b"{\"schema_version\": 2}").unwrap();
        assert_eq!(read_capped_text(&path, 100).unwrap().as_deref(), Some("{\"schema_version\": 2}"));
        assert_eq!(names(&dir), ["settings.json"], "the temporary files are gone");
    }

    /// A crash between the temporary file and the rename leaves the old file whole: its temporary
    /// file is the only trace, it doesn't stop the next write, and the next write is whole.
    #[test]
    fn a_crash_before_the_rename_leaves_the_old_file_whole() {
        let dir = dir("state-crash");
        let path = dir.join("settings.json");
        write_atomic(&path, b"old, whole").unwrap();
        let (temp, mut file) = temporary(&path, Access::Shared).unwrap();
        file.write_all(b"new, half").unwrap();
        drop(file); // the crash: no flush, no rename
        assert_eq!(read_capped(&path, 100).unwrap().as_deref(), Some(&b"old, whole"[..]));
        write_atomic(&path, b"new, whole").unwrap();
        assert_eq!(read_capped(&path, 100).unwrap().as_deref(), Some(&b"new, whole"[..]));
        assert_eq!(names(&dir).len(), 2, "the crashed write's temporary file is the only other: {:?}", names(&dir));
        assert!(temp.is_file());
    }

    /// A reader never sees half a file, however the writes and the reads interleave.
    #[test]
    fn a_reader_sees_the_old_file_or_the_new_never_half() {
        let dir = dir("state-interleaved");
        let path = dir.join("big.bin");
        let versions: Vec<Vec<u8>> = (0..4u8).map(|v| vec![b'a' + v; 256 * 1024]).collect();
        write_atomic(&path, versions.first().unwrap()).unwrap();
        let reader = {
            let (path, versions) = (path.clone(), versions.clone());
            std::thread::spawn(move || {
                for _ in 0..200 {
                    // Windows may refuse an open while the rename replaces the file: a retry, not half a file.
                    if let Ok(Some(bytes)) = read_capped(&path, 1024 * 1024) {
                        assert!(versions.contains(&bytes), "a read saw {} bytes of no version", bytes.len());
                    }
                }
            })
        };
        for round in 0..40 {
            let version = versions.get(round % versions.len()).unwrap();
            // Windows may refuse a rename over a file the reader has open: the old file is still whole.
            let _ = write_atomic(&path, version);
        }
        reader.join().unwrap();
        assert!(versions.contains(&read_capped(&path, 1024 * 1024).unwrap().unwrap()));
    }

    /// A write that fails leaves what was at the path as it was, and no temporary file: here the
    /// path is a directory, which a file can't be renamed over.
    #[test]
    fn a_failed_write_leaves_the_old_and_no_temporary_file() {
        let dir = dir("state-failed-write");
        let taken = dir.join("taken");
        fs::create_dir_all(taken.join("inside")).unwrap();
        let e = write_atomic(&taken, b"new").unwrap_err();
        assert_eq!(e.kind, Kind::Failed);
        assert!(e.message.contains("couldn't save") && !e.remediation.is_empty(), "{e}");
        assert!(taken.join("inside").is_dir());
        assert_eq!(names(&dir), ["taken"], "no temporary file is left");
        assert_eq!(write_atomic(Path::new(""), b"x").unwrap_err().kind, Kind::Failed, "a path that names no file");
        assert!(write_atomic(&dir.join("no-such-dir").join("f"), b"x").is_err(), "a directory that isn't there");
    }

    /// `Keep` writes only where nothing is, and keeps what is there; `Replace` replaces it.
    #[test]
    fn keep_never_replaces_what_is_there() {
        let dir = dir("state-keep");
        let path = dir.join("token");
        assert_eq!(write_atomic_as(&path, b"first", Access::Private, Existing::Keep), Ok(Written::Wrote));
        assert_eq!(write_atomic_as(&path, b"second", Access::Private, Existing::Keep), Ok(Written::Kept));
        assert_eq!(read_capped(&path, 100).unwrap().as_deref(), Some(&b"first"[..]));
        assert_eq!(write_atomic_as(&path, b"third", Access::Shared, Existing::Replace), Ok(Written::Wrote));
        assert_eq!(read_capped(&path, 100).unwrap().as_deref(), Some(&b"third"[..]));
        assert_eq!(names(&dir), ["token"], "no temporary file is left");
    }

    /// A refusal is tried again only on Windows, only for `PermissionDenied` and only a bounded few
    /// times; any other failure, and a success, end at once.
    #[test]
    fn a_refused_operation_is_retried_a_bounded_few_times_on_windows_only() {
        let tries = std::cell::Cell::new(0);
        let refused = patiently(|| {
            tries.set(tries.get() + 1);
            Err::<(), _>(io::Error::from(ErrorKind::PermissionDenied))
        });
        assert_eq!(refused.unwrap_err().kind(), ErrorKind::PermissionDenied);
        assert_eq!(tries.get(), 1 + BUSY_PAUSES.len(), "every pause, then the failure");
        assert_eq!(BUSY_PAUSES.is_empty(), !cfg!(windows));
        assert!(BUSY_PAUSES.iter().sum::<Duration>() < Duration::from_millis(2_500), "a bound a person barely notices");
        tries.set(0);
        let other = patiently(|| {
            tries.set(tries.get() + 1);
            Err::<(), _>(io::Error::from(ErrorKind::NotFound))
        });
        assert_eq!((other.unwrap_err().kind(), tries.get()), (ErrorKind::NotFound, 1), "another failure is not retried");
        tries.set(0);
        let later = patiently(|| {
            tries.set(tries.get() + 1);
            if tries.get() < 3 { Err(io::Error::from(ErrorKind::PermissionDenied)) } else { Ok(tries.get()) }
        });
        assert_eq!(later.map_err(|e| e.kind()), if cfg!(windows) { Ok(3) } else { Err(ErrorKind::PermissionDenied) });
    }

    /// A private file is its owner's alone from its first byte, and its new directory too.
    #[cfg(unix)]
    #[test]
    fn a_private_file_and_directory_are_the_owners_alone() {
        use std::os::unix::fs::MetadataExt;
        let base = dir("state-private");
        let inner = base.join("secret").join("deeper");
        make_dir(&inner, Access::Private).unwrap();
        assert_eq!(fs::metadata(&inner).unwrap().mode() & 0o777, 0o700);
        let path = inner.join("token");
        write_atomic_as(&path, b"x", Access::Private, Existing::Replace).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        write_atomic(&base.join("shared"), b"x").unwrap();
        assert_ne!(fs::metadata(base.join("shared")).unwrap().mode() & 0o077, 0, "a shared file gets the process's defaults");
    }

    /// A link at the path, even one to nowhere, is kept by `Keep` and never written through.
    #[cfg(unix)]
    #[test]
    fn keep_never_follows_a_link() {
        let dir = dir("state-link");
        let target = dir.join("elsewhere");
        let link = dir.join("token");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(write_atomic_as(&link, b"secret", Access::Private, Existing::Keep), Ok(Written::Kept));
        assert!(!target.exists(), "nothing was written through the link");
    }

    #[test]
    fn a_read_is_capped_and_never_takes_a_bad_file_for_no_file() {
        let dir = dir("state-read");
        let big = dir.join("big.json");
        write_atomic(&big, &[b'a'; 11]).unwrap();
        assert_eq!(read_capped(&big, 10).unwrap_err().kind, Kind::TooLarge);
        assert_eq!(read_capped(&big, 11).unwrap().map(|b| b.len()), Some(11));
        assert_eq!(read_capped(&big, 0).unwrap_err().kind, Kind::TooLarge);
        let binary = dir.join("binary");
        write_atomic(&binary, &[0xff, 0xfe]).unwrap();
        assert_eq!(read_capped_text(&binary, 10).unwrap_err().kind, Kind::Failed);
        let e = read_capped(&dir, 10).unwrap_err();
        assert!(e.message.contains("not a regular file"), "a directory is no file to read: {e}");
    }

    #[test]
    fn a_directory_is_made_and_a_file_removed() {
        let base = dir("state-dirs");
        let inner = base.join("a").join("b");
        make_dir(&inner, Access::Shared).unwrap();
        make_dir(&inner, Access::Shared).unwrap();
        assert!(inner.is_dir());
        let path = inner.join("f");
        write_atomic(&path, b"x").unwrap();
        remove(&path).unwrap();
        remove(&path).unwrap();
        assert!(!path.exists());
        assert!(remove(&base).is_err(), "a directory is no file to remove");
    }
}
