//! The backends layer of {{product}}: the one place that touches the outside
//! (docs/app-architecture.md › Layers). Every layer above reaches a program or a file through
//! here, and `model` and `domain`, below, reach nothing.
//!
//! - **Another program is started in one place** ([`process`]): with a wall-clock and an idle
//!   deadline, a capped output, its own process group, and without the control socket's token
//!   (› Never hang on another program).
//! - **The one atomic writer** ([`files::write_atomic`]) and its capped read
//!   ([`files::read_capped`]): every file the app writes goes through them (› State on disk). The
//!   engine, or the module that owns a file, decides what is stored, its `schema_version` and when
//!   it is saved; this crate only puts the bytes there.
//! - **Every failure is a [`BackendError`]**: a [`Kind`], what happened and what to do. The engine
//!   turns it into the `CommandError` a caller sees.
//! - **Every call can be cancelled** ([`CancelToken`]): the token a call's caller sets, which the
//!   engine hands down.
//!
//! A further backend (git, a network service, a device) is a module here or a crate of its own in
//! this layer (`layer = "backends"`): each call with a deadline and a capped result, a program
//! through [`process`], a file through [`files`].
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

mod cancel;
mod error;
pub mod files;
pub mod process;

pub use cancel::CancelToken;
pub use error::{BackendError, Kind};
