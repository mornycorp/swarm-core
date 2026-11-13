//! A backend's failure: what kind, what happened, and what to do about it.

use std::fmt;

/// What kind of failure it is. The engine maps each to the code a caller sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// What the call names (a program, a file) isn't there.
    NotFound,
    /// A result is over its cap.
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
    /// What happened, for a person.
    pub message: String,
    /// What to do about it; empty when there is nothing the caller can do.
    pub remediation: String,
}

impl BackendError {
    pub fn new(kind: Kind, message: impl Into<String>, remediation: impl Into<String>) -> BackendError {
        BackendError { kind, message: message.into(), remediation: remediation.into() }
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

    pub fn failed(message: impl Into<String>) -> BackendError {
        BackendError::new(Kind::Failed, message, "")
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BackendError {}
