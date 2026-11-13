//! The control socket's token (docs/app-architecture.md › The JSON-lines frame): the one secret a
//! client proves it has in its `auth` message.
//!
//! - **Where it comes from.** `{{env}}_CONTROL_TOKEN` ([`ENV`]) when that is set, else a file
//!   ([`Token::from_file`]). Neither is a typed error that says what to set ([`Token::resolve`]).
//! - **It is written by the backends' one atomic writer** (`files::write_atomic_as`, with
//!   `Access::Private`): the token goes to a new temporary file that is the owner's alone (`0600` on
//!   Unix) before its first byte, then into place, so it is never readable by others, never half
//!   written, and nothing planted at the path is written through.
//! - **On Unix the file is private and yours.** A missing file is created with a random token, linked
//!   into place only where nothing is (`Existing::Keep`: a file that appeared meanwhile, or a planted
//!   symlink, is kept and then checked, never followed into a write), and an existing one is read
//!   only when it is a regular file (not a symlink, a FIFO or a device:
//!   checked before it is opened, so nothing at the path can block the start), owned by the user
//!   the app runs as, and readable by nobody else. Anything else is refused, not used: a token
//!   someone else planted would be one they know.
//! - **On Windows a new token is written at every start.** `std` can neither read nor set a file's
//!   owner or ACL there, so an existing file can't be told apart from a planted one: it is never
//!   read. Each start writes a fresh token to a new file beside it and renames it over the path, so
//!   a client reads the token after the start-up line, every time. The new file has the ACL its
//!   directory gives it: **keep it in a directory only you can read** (under your profile, such as
//!   `%LOCALAPPDATA%`), not a shared one, or pass the token in the environment, which needs no file.
//! - **A client only reads it.** `{{cli}} mcp --attach` ([`Token::read_existing`]) takes the
//!   variable, else the file the app was started with, read as the server reads one (on Unix: a
//!   regular file, yours and private), and never makes or replaces a file.
//! - **The variable stays in the environment.** `std::env::remove_var` is `unsafe` in edition 2024
//!   and this crate forbids `unsafe`, so [`ENV`] is not removed after it is read, and a child
//!   process would inherit it: the backends' process module (docs/app-architecture.md › Never hang
//!   on another program), the one place a program is started, takes it out of every child's
//!   environment. [`ENV`] is that module's `TOKEN_ENV`, so the two never name different variables.
//! - **It is never shown.** [`Token`]'s `Debug` is redacted, no error here repeats a token's value,
//!   and nothing logs one.
//! - **It is compared in constant time** ([`Token::matches`], [`ct_eq`]): the comparison takes the
//!   same time wherever the first difference is. `Token` has no `==`, so nothing compares it another
//!   way.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use {{crate_prefix_ident}}backends::BackendError;
use {{crate_prefix_ident}}backends::files::{self, Access, Existing, Written};
use {{crate_prefix_ident}}engine::CommandError;

/// The environment variable that carries the token instead of a file: the one the process module
/// takes out of every child's environment.
pub const ENV: &str = {{crate_prefix_ident}}backends::process::TOKEN_ENV;

/// The shortest token accepted: a short one is guessable.
pub const MIN_LEN: usize = 16;

/// The longest token accepted, and the most of a token file that is read.
pub const MAX_LEN: usize = 256;

/// Random bytes in a generated token (64 hexadecimal characters).
const GENERATED_BYTES: usize = 32;

/// A control token: [`MIN_LEN`] to [`MAX_LEN`] visible ASCII characters, no whitespace. It has no
/// `==`: [`Token::matches`] is the only comparison, and it takes constant time.
#[derive(Clone)]
pub struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// Where a token was read from or created, for a person to be told (never the token itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Env,
    File(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Env => write!(f, "env {ENV}"),
            Source::File(path) => write!(f, "file {path}"),
        }
    }
}

impl Token {
    /// `text` as a token, or why it isn't one. The failure never repeats the text.
    pub fn parse(text: &str) -> Result<Token, CommandError> {
        let ok_chars = text.bytes().all(|b| (0x21..=0x7e).contains(&b));
        if !ok_chars || !(MIN_LEN..=MAX_LEN).contains(&text.len()) {
            return Err(CommandError::unavailable(
                format!("the control token must be {MIN_LEN} to {MAX_LEN} visible ASCII characters with no whitespace ({} given)", text.len()),
                "generate one with `--token-file <path>` naming a file that doesn't exist yet, or set a longer random value",
            ));
        }
        Ok(Token(text.to_owned()))
    }

    /// A fresh random token from the operating system's generator.
    pub fn generate() -> Result<Token, CommandError> {
        random_hex(GENERATED_BYTES).map(Token)
    }

    /// The token as a client sends it. Only `mcp --attach`'s `auth` ([`crate::attach`]) and a test's
    /// client read it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether `candidate` is this token, compared in constant time.
    pub fn matches(&self, candidate: &str) -> bool {
        ct_eq(self.0.as_bytes(), candidate.as_bytes())
    }

    /// The token the environment or `file` gives, and where it came from: [`ENV`] wins; else
    /// `file` gives it ([`Token::from_file`]); else `unavailable`, naming both.
    pub fn resolve(file: Option<&Path>) -> Result<(Token, Source), CommandError> {
        if let Some(token) = Token::from_env()? {
            return Ok((token, Source::Env));
        }
        let Some(path) = file else {
            return Err(CommandError::unavailable(
                format!("no control token: {ENV} is not set and no token file was given"),
                format!(
                    "set {ENV}, or pass `--token-file <path>` in a directory only you can read (a new token is made there when the file doesn't exist; on Windows, at every start)"
                ),
            ));
        };
        Token::from_file(path).map(|t| (t, Source::File(path.display().to_string())))
    }

    /// The token [`ENV`] holds: `None` when it is unset or empty.
    fn from_env() -> Result<Option<Token>, CommandError> {
        match std::env::var(ENV) {
            Ok(value) if !value.is_empty() => Token::parse(&value).map(Some),
            Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                Err(CommandError::unavailable(format!("{ENV} is not UTF-8"), format!("set {ENV} to a random ASCII string, or unset it")))
            }
        }
    }

    /// Whether [`ENV`] is set (to anything but empty): a client then needs no token file.
    pub fn in_env() -> bool {
        std::env::var_os(ENV).is_some_and(|value| !value.is_empty())
    }

    /// The token a **client** sends (`{{cli}} mcp --attach`): [`ENV`] wins, else the token in `file`,
    /// which the app made. It is only read, never created or replaced, and on Unix only when it is a
    /// regular file that is the user's own and private, as the server reads one.
    pub fn read_existing(file: Option<&Path>) -> Result<Token, CommandError> {
        if let Some(token) = Token::from_env()? {
            return Ok(token);
        }
        let Some(path) = file else {
            return Err(CommandError::unavailable(
                format!("no control token: {ENV} is not set and no token file was given"),
                format!("set {ENV} to the app's token, or pass `--token-file <path>` naming the file the app was started with"),
            ));
        };
        read_private(path)
    }

    /// The token in `path`. On Unix: read when the file exists (and is a regular file that is yours
    /// and private), else created with a random one. On Windows: a random one, written over `path`
    /// at every call (an existing file is never read).
    pub fn from_file(path: &Path) -> Result<Token, CommandError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            files::make_dir(dir, Access::Private).map_err(stored)?;
        }
        if cfg!(windows) {
            return renew(path);
        }
        match fs::symlink_metadata(path) {
            Ok(_) => read_private(path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => create(path),
            Err(e) => Err(io_failure(path, "read", &e)),
        }
    }
}

/// Whether `a` and `b` are equal, in time that depends on their lengths and not on where they
/// differ.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for at in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(at).copied().unwrap_or(0), b.get(at).copied().unwrap_or(0));
        diff |= usize::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

/// `bytes` random bytes from the operating system's generator, as hexadecimal.
fn random_hex(bytes: usize) -> Result<String, CommandError> {
    let mut buf = vec![0_u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| CommandError::internal(format!("the system's random generator failed: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn io_failure(path: &Path, what: &str, e: &io::Error) -> CommandError {
    CommandError::unavailable(
        format!("cannot {what} the token file {}: {e}", path.display()),
        "check the path and its permissions, or set the token in the environment instead",
    )
}

/// A refusal of the file at `path`, with what to do about it.
fn refused(path: &Path, why: &str, fix: String) -> CommandError {
    CommandError::unavailable(format!("the token file {} {why}", path.display()), fix)
}

/// The writer's failure on the token file, with what to do about it here.
fn stored(e: BackendError) -> CommandError {
    CommandError::unavailable(format!("the token file: {}", e.message), "check the path and its permissions, or set the token in the environment instead")
}

/// The token as its file holds it: one line.
fn line(token: &Token) -> String {
    format!("{}\n", token.0)
}

/// Unix: a random token at `path`, where nothing was. When something appeared there meanwhile it is
/// kept, never written through, and read as an existing file is (with its checks).
fn create(path: &Path) -> Result<Token, CommandError> {
    let token = Token::generate()?;
    match files::write_atomic_as(path, line(&token).as_bytes(), Access::Private, Existing::Keep).map_err(stored)? {
        Written::Wrote => Ok(token),
        Written::Kept => read_private(path),
    }
}

/// What is at `path`, looked at without following a link: a regular file, or a refusal. A link
/// could be swapped for another file, and opening a FIFO or a device can block for ever.
fn regular(path: &Path) -> Result<fs::Metadata, CommandError> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_failure(path, "read", &e))?;
    if meta.file_type().is_symlink() {
        return Err(refused(path, "is a symbolic link", "point `--token-file` at the file itself, not a link to it".to_owned()));
    }
    if !meta.is_file() {
        return Err(refused(path, "is not a regular file", "point `--token-file` at a file, or at a path where one can be made".to_owned()));
    }
    Ok(meta)
}

/// Windows: a fresh token written to a new file beside `path`, renamed over it (the one atomic
/// writer). What was at `path` is replaced unread (it may be a link or a file someone else made);
/// only a directory there is refused.
fn renew(path: &Path) -> Result<Token, CommandError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            return Err(refused(path, "is a directory", "point `--token-file` at a file, or at a path where one can be made".to_owned()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_failure(path, "read", &e)),
    }
    let token = Token::generate()?;
    files::write_atomic_as(path, line(&token).as_bytes(), Access::Private, Existing::Replace).map_err(stored)?;
    Ok(token)
}

/// A path beside `path` that nothing has: its name, a random part and `suffix`.
#[cfg(unix)]
fn beside(path: &Path, suffix: &str) -> Result<std::path::PathBuf, CommandError> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(path.with_file_name(format!(".{name}.{}.{suffix}", random_hex(8)?)))
}

/// The token in an existing file: a regular file, the user's own, that only they can reach, of at
/// most [`MAX_LEN`] characters.
fn read_private(path: &Path) -> Result<Token, CommandError> {
    let before = regular(path)?;
    let file = File::open(path).map_err(|e| io_failure(path, "read", &e))?;
    let meta = file.metadata().map_err(|e| io_failure(path, "read", &e))?;
    if !meta.is_file() {
        return Err(refused(path, "is not a regular file", "point `--token-file` at a file".to_owned()));
    }
    #[cfg(unix)]
    owned_and_private(path, &before, &meta)?;
    #[cfg(not(unix))]
    let _ = before;
    let mut text = String::new();
    file.take(MAX_LEN as u64 + 2).read_to_string(&mut text).map_err(|e| io_failure(path, "read", &e))?;
    Token::parse(text.trim_end_matches(['\r', '\n']))
}

/// Unix: the file opened (`meta`) is the one looked at (`before`), it belongs to the user the app
/// runs as, and nobody else can read or write it.
#[cfg(unix)]
fn owned_and_private(path: &Path, before: &fs::Metadata, meta: &fs::Metadata) -> Result<(), CommandError> {
    use std::os::unix::fs::MetadataExt;
    if (before.dev(), before.ino()) != (meta.dev(), meta.ino()) {
        return Err(refused(
            path,
            "changed while it was opened",
            "start again; if it happens again, keep the token file in a directory only you can write".to_owned(),
        ));
    }
    owned_by(path, meta.uid(), my_uid(path)?)?;
    if meta.mode() & 0o077 != 0 {
        return Err(refused(
            path,
            "can be read by others",
            format!("run `chmod 600 {}` (or delete it to have a new token made), then start again", path.display()),
        ));
    }
    Ok(())
}

/// Unix: a file owned by `owner` is refused unless that is `me`: a token file someone else made
/// holds a token they know.
#[cfg(unix)]
fn owned_by(path: &Path, owner: u32, me: u32) -> Result<(), CommandError> {
    if owner == me {
        return Ok(());
    }
    Err(refused(path, "belongs to another user", format!("delete it to have a new token made, or point `--token-file` at a file of yours (user id {me})")))
}

/// Unix: the user id the app runs as. `std` has no `geteuid` (and this crate no `unsafe`), but a
/// file the process creates is owned by it: a probe file beside the token file (or in the temp
/// directory, when that one can't be written), made by the one atomic writer, says, and is removed.
#[cfg(unix)]
fn my_uid(path: &Path) -> Result<u32, CommandError> {
    use std::os::unix::fs::MetadataExt;
    let mut last = None;
    for probe in [beside(path, "probe")?, std::env::temp_dir().join(format!(".control-token.{}.probe", random_hex(8)?))] {
        match files::write_atomic_as(&probe, b"", Access::Private, Existing::Keep) {
            Ok(Written::Wrote) => {
                let owner = fs::symlink_metadata(&probe).map(|meta| meta.uid());
                let _ = files::remove(&probe);
                match owner {
                    Ok(uid) => return Ok(uid),
                    Err(e) => last = Some(io_failure(path, "check the owner of", &e)),
                }
            }
            Ok(Written::Kept) => last = Some(refused(&probe, "was already there", "start again".to_owned())),
            Err(e) => last = Some(stored(e)),
        }
    }
    Err(last.unwrap_or_else(|| io_failure(path, "check the owner of", &io::Error::from(io::ErrorKind::PermissionDenied))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        {{crate_prefix_ident}}testkit::temp_dir(name).unwrap()
    }

    /// `text` in a file at `path`, private on Unix as a token file must be.
    fn write_private(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    }

    #[test]
    fn ct_eq_is_equality() {
        assert!(ct_eq(b"abc", b"abc") && ct_eq(b"", b""));
        for (a, b) in [(&b"abc"[..], &b"abd"[..]), (b"abc", b"ab"), (b"ab", b"abc"), (b"", b"a"), (b"abc", b"")] {
            assert!(!ct_eq(a, b), "{a:?} {b:?}");
        }
    }

    #[test]
    fn a_token_is_visible_ascii_of_a_sane_length_and_is_never_shown() {
        assert!(Token::parse("0123456789abcdef").is_ok());
        for bad in ["", "short", "has a space in it 0123456", "tab\there0123456789", "caf\u{e9}0123456789abcdef", &"x".repeat(MAX_LEN + 1)] {
            let e = Token::parse(bad).unwrap_err();
            assert!(!e.message.contains(bad) || bad.is_empty(), "the failure repeats the input: {e}");
            assert!(!e.remediation.is_empty());
        }
        let token = Token::parse("test-token-not-real-0123").unwrap();
        assert!(!format!("{token:?}").contains("not-real"), "{token:?}");
        assert!(token.matches("test-token-not-real-0123") && !token.matches("test-token-not-real-0124") && !token.matches(""));
    }

    #[test]
    fn a_generated_token_is_random_and_valid() {
        let (a, b) = (Token::generate().unwrap(), Token::generate().unwrap());
        assert!(!a.matches(b.expose()));
        assert!(Token::parse(a.expose()).is_ok() && a.expose().len() == GENERATED_BYTES * 2);
    }

    #[test]
    fn a_missing_file_is_created_with_a_token() {
        let path = dir("token-create").join("inner").join("control.token");
        let made = Token::from_file(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim_end(), made.expose());
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&path).unwrap()) & 0o777, 0o600);
        let again = Token::from_file(&path).unwrap();
        if cfg!(windows) {
            assert!(!again.matches(made.expose()), "on Windows each start writes a new token");
            assert_eq!(std::fs::read_to_string(&path).unwrap().trim_end(), again.expose());
        } else {
            assert!(again.matches(made.expose()), "the second start reads what the first made");
        }
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["control.token"], "no probe or new file is left beside it");
    }

    #[test]
    fn an_existing_file_is_read_on_unix_and_never_trusted_on_windows() {
        let path = dir("token-planted").join("control.token");
        write_private(&path, "test-token-not-real-planted-0123\n");
        let token = Token::from_file(&path).unwrap();
        if cfg!(windows) {
            assert!(!token.matches("test-token-not-real-planted-0123"), "a file this start didn't write is not used");
            assert_eq!(std::fs::read_to_string(&path).unwrap().trim_end(), token.expose(), "it is replaced");
        } else {
            assert!(token.matches("test-token-not-real-planted-0123"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_token_file_others_can_read_is_refused() {
        let path = dir("token-open").join("control.token");
        std::fs::write(&path, "0123456789abcdef0123\n").unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let e = Token::from_file(&path).unwrap_err();
        assert!(e.message.contains("can be read by others") && e.remediation.contains("chmod 600"), "{e}");
        assert!(!e.message.contains("0123456789abcdef"));
    }

    #[cfg(unix)]
    #[test]
    fn a_token_file_of_another_user_is_refused() {
        let path = dir("token-owner").join("control.token");
        write_private(&path, "0123456789abcdef0123\n");
        let me = my_uid(&path).unwrap();
        let owner = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&path).unwrap());
        assert_eq!(owner, me, "a file the test made is the test's");
        assert!(owned_by(&path, me, me).is_ok());
        let e = owned_by(&path, me.wrapping_add(1), me).unwrap_err();
        assert!(e.message.contains("belongs to another user") && e.remediation.contains("delete it"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_or_a_special_file_at_the_path_is_refused_before_it_is_opened() {
        let base = dir("token-special");
        let real = base.join("real.token");
        write_private(&real, "0123456789abcdef0123\n");
        let link = base.join("link.token");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = Token::from_file(&link).unwrap_err();
        assert!(e.message.contains("symbolic link"), "{e}");
        // A socket file stands in for a FIFO or a device: not a regular file, so it is never opened.
        let socket = base.join("socket.token");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let e = Token::from_file(&socket).unwrap_err();
        assert!(e.message.contains("not a regular file"), "{e}");
    }

    #[test]
    fn a_bad_file_is_a_typed_failure_never_a_panic() {
        let base = dir("token-bad");
        let e = Token::from_file(&base).unwrap_err();
        assert!(e.message.contains("directory") || e.message.contains("not a regular file"), "a directory is not a token file: {e}");
        if cfg!(windows) {
            return; // an existing file is never read there
        }
        let short = base.join("short");
        write_private(&short, "tiny\n");
        assert!(Token::from_file(&short).unwrap_err().message.contains("16 to 256"));
        let huge = base.join("huge");
        write_private(&huge, &"x".repeat(10 * MAX_LEN));
        assert!(Token::from_file(&huge).is_err());
    }

    #[test]
    fn with_no_token_source_it_says_what_to_set() {
        // `resolve` reads the environment, which the test doesn't change: the variable is unset here.
        if std::env::var_os(ENV).is_none() {
            let e = Token::resolve(None).unwrap_err();
            assert!(e.message.contains(ENV) && e.remediation.contains("--token-file"), "{e}");
        }
    }
}
