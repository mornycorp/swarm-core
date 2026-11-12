//! The engine layer of {{product}}: the session, the typed command registry and the one dispatch
//! every surface calls (docs/app-architecture.md › Every action is a command, One dispatch).
//!
//! - **A command is a type** ([`Command`]): an id, a title and a summary, typed params and a typed
//!   output, its [`Effects`] and [`Requirement`]s, its place in a menu and its shortcut ([`menu`]),
//!   its own `enabled` check, and `run`. [`Registry::register`] is the only
//!   place a [`CommandSpec`] is built, so no command exists without schemas (generated from its
//!   types) or effects. Every command is registered in one place ([`commands::registry`]).
//! - **One dispatch** ([`Session::call`]): looks the command up, checks the params against its
//!   type, that it is enabled (its requirements, then its own check) and its consent, and runs it.
//!   Every failure is a [`CommandError`] with a stable [`ErrorCode`] and a remediation. A transport
//!   only frames: it never names a command.
//! - **Enabled** ([`Session::states`]): every command's "can it run now, and why not", in one call
//!   for a menu. It is the dispatch's own check, so the two never disagree.
//! - **The UI's state is plain data** ([`UiState`], in the session): `ui.inspect` and `ui.set`
//!   ([`commands::UiInspectCommand`], [`commands::UiSetCommand`]) read and set it from every surface,
//!   and the UI shell draws it (docs/app-architecture.md › The UI is a client).
//! - **The theme is data** ([`theme`]): the colour tokens and the categorical ranges, in a dark and a
//!   light look, with the stable pick of an identifier's colour; `ui.theme`
//!   ([`commands::UiThemeCommand`]) gives them to every surface, and the UI crate's theme draws with
//!   them.
//! - **The example** is `app.info` ([`commands::InfoCommand`]). It stays until the first real
//!   command replaces it.
//!
//! Adding a command: a type that implements [`Command`], a line in [`commands::registry`], and its
//! tests. No transport changes.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

mod call;
pub mod commands;
mod error;
pub mod menu;
mod registry;
mod session;
mod spec;
pub mod theme;
mod ui;

pub use call::{CallContext, CallOpts, Caller, CancelToken};
pub use error::{CommandError, ErrorCode, clip};
pub use registry::{Registry, RegistryError};
pub use session::{AppInfo, Session};
pub use spec::{Command, CommandSpec, CommandState, Effects, Requirement, Why};
pub use ui::{OnChange, UiState, UiStateError};
