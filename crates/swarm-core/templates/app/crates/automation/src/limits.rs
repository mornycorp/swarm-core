//! The framing caps (docs/app-architecture.md › The JSON-lines frame): recommended numbers, so
//! every app starts from the same ones. Change them in the app's protocol document and here.

use std::time::Duration;

/// A request line of at most this many bytes (1 MiB), without its newline. A longer one is
/// discarded to its newline and answered `too_large`.
pub const MAX_REQUEST: usize = 1024 * 1024;

/// A reply of at most this many bytes (8 MiB). A longer one is `too_large`, never a truncated
/// result.
pub const MAX_REPLY: usize = 8 * 1024 * 1024;

/// Connections at a time, for a transport that accepts them (the loopback socket; stdio is one).
pub const MAX_CONNECTIONS: usize = 16;

/// A connection closed after this long with no request, for a transport that accepts them.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
