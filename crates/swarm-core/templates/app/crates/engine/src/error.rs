//! The one failure type of a call, the same on every surface (docs/app-architecture.md › The
//! error). A consumer classifies a failure by its [`ErrorCode`], never by its message.

use std::fmt;

use serde::Serialize;

use {{crate_prefix_ident}}backends::{BackendError, Kind};

/// `clip` is the backends' own, so the message of a call and of a backend are clipped alike.
pub use {{crate_prefix_ident}}backends::clip;

/// A failure's stable, machine-read code. Their spelling and meaning never change; an app adds
/// codes of its own beside the contract's (here: `unavailable`) and never reuses one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The params aren't the command's: the wrong type, a missing or an unknown key.
    BadParams,
    /// No command has this id.
    UnknownCommand,
    /// The thing the params name (a file, an item, a device) doesn't exist.
    NotFound,
    /// The request, a param or a result is over a cap.
    TooLarge,
    /// A transport that needs a token didn't get the right one.
    Unauthenticated,
    /// The command's effects need a consent the caller didn't give.
    ConsentRequired,
    /// The caller (or a deadline) cancelled the call.
    Cancelled,
    /// The app's fault: a bug, not the caller's.
    Internal,
    /// The app's own code: a requirement of the command doesn't hold (no session, no open project).
    Unavailable,
}

impl ErrorCode {
    /// Every code.
    pub const ALL: [ErrorCode; 9] = [
        ErrorCode::BadParams,
        ErrorCode::UnknownCommand,
        ErrorCode::NotFound,
        ErrorCode::TooLarge,
        ErrorCode::Unauthenticated,
        ErrorCode::ConsentRequired,
        ErrorCode::Cancelled,
        ErrorCode::Internal,
        ErrorCode::Unavailable,
    ];

    /// The code as the wire spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadParams => "bad_params",
            ErrorCode::UnknownCommand => "unknown_command",
            ErrorCode::NotFound => "not_found",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::Unauthenticated => "unauthenticated",
            ErrorCode::ConsentRequired => "consent_required",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Internal => "internal",
            ErrorCode::Unavailable => "unavailable",
        }
    }

    /// What to do about a failure of this code, for one that gives no remediation of its own.
    const fn default_remediation(self) -> &'static str {
        match self {
            ErrorCode::BadParams => "pass the params the command's schema describes (`commands` lists them)",
            ErrorCode::UnknownCommand => "run `commands` for the ids this app has",
            ErrorCode::NotFound => "check the name or path, and that the thing exists",
            ErrorCode::TooLarge => "send less: split the request, or narrow what it asks for",
            ErrorCode::Unauthenticated => "send the token this app's transport requires first",
            ErrorCode::ConsentRequired => "agree to the command's effects (`--yes` on the CLI) and call again",
            ErrorCode::Cancelled => "call again if the result is still wanted",
            ErrorCode::Internal => "report this: it is a bug in the app, not in the call",
            ErrorCode::Unavailable => "meet the command's requirement, then call again",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a call failed, for every surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandError {
    /// Stable and machine-read.
    pub code: ErrorCode,
    /// What happened, for a person.
    pub message: String,
    /// What to do about it: never empty.
    pub remediation: String,
    /// `bad_params` only: the param that is wrong, by its JSON key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl CommandError {
    /// A failure of `code`. An empty `remediation` gets the code's default: it is never empty.
    pub fn new(code: ErrorCode, message: impl Into<String>, remediation: impl Into<String>) -> CommandError {
        let remediation = remediation.into();
        let remediation = if remediation.trim().is_empty() { code.default_remediation().to_owned() } else { remediation };
        CommandError { code, message: message.into(), remediation, key: None }
    }

    /// The param `key` is the wrong one.
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> CommandError {
        self.key = Some(key.into());
        self
    }

    pub fn bad_params(message: impl Into<String>, remediation: impl Into<String>) -> CommandError {
        CommandError::new(ErrorCode::BadParams, message, remediation)
    }

    pub fn unknown_command(id: &str) -> CommandError {
        CommandError::new(ErrorCode::UnknownCommand, format!("no command has the id `{}`", clip(id)), "")
    }

    pub fn not_found(message: impl Into<String>, remediation: impl Into<String>) -> CommandError {
        CommandError::new(ErrorCode::NotFound, message, remediation)
    }

    pub fn too_large(message: impl Into<String>, remediation: impl Into<String>) -> CommandError {
        CommandError::new(ErrorCode::TooLarge, message, remediation)
    }

    /// A transport that needs a token didn't get the right one (the loopback socket's `auth`).
    pub fn unauthenticated(message: impl Into<String>, remediation: impl Into<String>) -> CommandError {
        CommandError::new(ErrorCode::Unauthenticated, message, remediation)
    }

    pub fn cancelled() -> CommandError {
        CommandError::new(ErrorCode::Cancelled, "the call was cancelled", "")
    }

    pub fn consent_required(id: &str) -> CommandError {
        CommandError::new(ErrorCode::ConsentRequired, format!("`{id}` changes something outside the app's memory and needs the caller's consent"), "")
    }

    pub fn unavailable(message: impl Into<String>, remediation: impl Into<String>) -> CommandError {
        CommandError::new(ErrorCode::Unavailable, message, remediation)
    }

    /// The app's fault. The caller's message says what; the app logs the detail.
    pub fn internal(message: impl Into<String>) -> CommandError {
        CommandError::new(ErrorCode::Internal, message, "")
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

/// A backend's failure as a call's, so a command calls a backend with `?`: its kind is the code,
/// and its message and remediation are kept. `Failed` (a deadline passed, the disk or the system
/// refused) is `unavailable`: not a bug in the app, and worth a retry once its cause is gone.
impl From<BackendError> for CommandError {
    fn from(e: BackendError) -> CommandError {
        let code = match e.kind {
            Kind::NotFound => ErrorCode::NotFound,
            Kind::TooLarge => ErrorCode::TooLarge,
            Kind::Cancelled => ErrorCode::Cancelled,
            Kind::Failed => ErrorCode::Unavailable,
        };
        CommandError::new(code, e.message, e.remediation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remediation_is_never_empty() {
        for code in ErrorCode::ALL {
            assert!(!CommandError::new(code, "m", "  ").remediation.is_empty(), "{code}");
            assert_eq!(CommandError::new(code, "m", "do this").remediation, "do this");
        }
    }

    #[test]
    fn the_codes_keep_their_spelling_on_the_wire() {
        let spelled: Vec<String> = ErrorCode::ALL.iter().map(|c| serde_json::to_string(c).unwrap()).collect();
        let want = ["bad_params", "unknown_command", "not_found", "too_large", "unauthenticated", "consent_required", "cancelled", "internal", "unavailable"];
        assert_eq!(spelled, want.map(|w| format!("\"{w}\"")));
        assert!(ErrorCode::ALL.iter().zip(want).all(|(c, w)| c.as_str() == w));
    }

    #[test]
    fn the_error_serializes_as_the_contract_shows_it() {
        let e = CommandError::bad_params("missing field `path`", "pass `path`, a file path").with_key("path");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"code": "bad_params", "message": "missing field `path`", "remediation": "pass `path`, a file path", "key": "path"})
        );
        assert!(serde_json::to_value(CommandError::cancelled()).unwrap().get("key").is_none());
    }

    #[test]
    fn a_backends_failure_keeps_its_words_and_takes_its_code() {
        let cases = [
            (BackendError::not_found("no `git` here", "install it"), ErrorCode::NotFound),
            (BackendError::too_large("over the cap", "ask for less"), ErrorCode::TooLarge),
            (BackendError::cancelled(), ErrorCode::Cancelled),
            (BackendError::failed("it hung", "run it by hand"), ErrorCode::Unavailable),
        ];
        for (backend, code) in cases {
            let e = CommandError::from(backend.clone());
            assert_eq!((e.code, e.message, e.remediation, e.key), (code, backend.message, backend.remediation, None));
        }
    }

    #[test]
    fn a_message_never_repeats_a_hostile_input_whole() {
        let e = CommandError::unknown_command(&"x".repeat(100_000));
        assert!(e.message.chars().count() < 300, "{}", e.message.len());
        assert_eq!(clip("short"), "short");
    }
}
