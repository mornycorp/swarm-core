//! A backend's failure: its kind, what happened and what to do about it. The engine turns it into
//! the `CommandError` a caller sees, its kind into the code (`From`), so a command uses `?`.

use std::fmt;

/// What kind of failure it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// What the call names (a program, a file) isn't there.
    NotFound,
    /// A result (a file read, a program's output) is over its cap.
    TooLarge,
    /// The caller cancelled the call.
    Cancelled,
    /// It couldn't be done: a deadline passed, the disk or the system refused.
    Failed,
}

/// A failure of a call to the outside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub kind: Kind,
    /// What happened, for a person: it never repeats an input whole ([`clip`]).
    pub message: String,
    /// What to do about it: never empty.
    pub remediation: String,
}

impl BackendError {
    /// A failure of `kind`. An empty `remediation` gets the kind's own: it is never empty.
    pub fn new(kind: Kind, message: impl Into<String>, remediation: impl Into<String>) -> BackendError {
        let remediation = remediation.into();
        let remediation = if remediation.trim().is_empty() { default_remediation(kind).to_owned() } else { remediation };
        BackendError { kind, message: message.into(), remediation }
    }

    pub fn not_found(message: impl Into<String>, remediation: impl Into<String>) -> BackendError {
        BackendError::new(Kind::NotFound, message, remediation)
    }

    pub fn too_large(message: impl Into<String>, remediation: impl Into<String>) -> BackendError {
        BackendError::new(Kind::TooLarge, message, remediation)
    }

    pub fn cancelled() -> BackendError {
        BackendError::new(Kind::Cancelled, "the call was cancelled", "")
    }

    pub fn failed(message: impl Into<String>, remediation: impl Into<String>) -> BackendError {
        BackendError::new(Kind::Failed, message, remediation)
    }
}

/// What to do about a failure of `kind` that gives no remediation of its own.
const fn default_remediation(kind: Kind) -> &'static str {
    match kind {
        Kind::NotFound => "check the name or path, and that the thing exists",
        Kind::TooLarge => "ask for less, or raise the cap where the call is made",
        Kind::Cancelled => "call again if the result is still wanted",
        Kind::Failed => "look at what the message names, then call again",
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BackendError {}

/// The longest text of a hostile input a message repeats.
const CLIP: usize = 200;

/// `text` cut to [`CLIP`] characters, so a message never repeats a hostile input whole: a path or
/// an argument here, what a transport echoes in the engine (which re-exports it).
pub fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(CLIP).collect();
    if text.chars().nth(CLIP).is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remediation_is_never_empty() {
        for kind in [Kind::NotFound, Kind::TooLarge, Kind::Cancelled, Kind::Failed] {
            assert!(!BackendError::new(kind, "m", " ").remediation.is_empty(), "{kind:?}");
            assert_eq!(BackendError::new(kind, "m", "do this").remediation, "do this");
        }
        assert_eq!(BackendError::cancelled().to_string(), "the call was cancelled");
    }

    #[test]
    fn a_clipped_text_is_short() {
        assert_eq!(clip("short"), "short");
        assert_eq!(clip(&"x".repeat(1_000)).chars().count(), CLIP + 1);
    }
}
