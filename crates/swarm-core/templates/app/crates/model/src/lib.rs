//! The model layer of {{product}}: the types and parsers of everything read from outside the
//! program (files, wire frames, user text). It reads text and returns a `Result`; it does no I/O
//! and depends on nothing but the standard library (docs/app-architecture.md › Layers).
//!
//! Here: the command id ([`ids`]), the one parser the skeleton starts with. A new parser (a file
//! format, a wire type) is a module here, with the hostile inputs it must refuse in its tests.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

pub mod ids;

/// The version of the shapes {{product}} exposes and persists. A change to one of them that an
/// older reader can't take bumps it, with a migration from every earlier version
/// (docs/app-architecture.md › State on disk). `app.info` reports it.
pub const SCHEMA_VERSION: u32 = 1;
