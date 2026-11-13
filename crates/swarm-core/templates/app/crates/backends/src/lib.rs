//! The backends layer of {{product}}: **the one layer that touches the outside**
//! (docs/app-architecture.md › Layers). It does the I/O; the layers above decide what is stored,
//! what is run and when. Every call here has a deadline and a capped result, and fails with a
//! [`BackendError`], which the engine turns into a `CommandError` (`?` in a command).
//!
//! - **The process module** ([`process`]): the one place a program is started, with a wall-clock
//!   and an idle deadline, a capped output, its stdin closed and what it starts stopped with it
//!   (docs/app-architecture.md › Never hang on another program). The `process-module` scan holds
//!   every other file to that.
//! - **The one atomic file writer** ([`files`]): a temporary file beside the target, flushed, then
//!   renamed over it, and the capped read beside it (docs/app-architecture.md › State on disk).
//!   The `file-writes` scan holds every crate but this one, the testkit and the UI crate (which has
//!   a rule of its own) to that.
//! - **The cancel token** ([`CancelToken`]): how a caller stops a call under way, which a backend
//!   polls; the engine re-exports it, so a command hands its own on.
//!
//! A further backend (git, the network, a device) is a module here, or a crate of this layer of its
//! own, under the same rules: the app's choice.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

mod cancel;
mod error;
pub mod files;
pub mod process;

pub use cancel::CancelToken;
pub use error::{BackendError, Kind, clip};
