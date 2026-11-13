//! The UI layer of {{product}}: a shell that **draws memory and calls commands**, nothing else
//! (docs/app-architecture.md › The UI is a client).
//!
//! - **The toolkit is the app's choice.** The contract picks none; this shell uses egui because
//!   both reference apps do. Swapping it replaces `draw.rs` and this crate's dependency, not the
//!   engine, a transport or a command. Only this crate and the windowed app (`apps/{{app}}`, which
//!   hosts the window) link a toolkit; `layers` refuses one anywhere else.
//! - **It lists the commands** the session has ([`Shell::rows`], from the registry: this crate
//!   names no command to list one), **builds its menu bar from their menu paths** ([`Shell::menus`],
//!   each item with its shortcut), and **greys out what can't run now**, saying why
//!   ([`Shell::refresh`]: the session's one query of every command's state, the dispatch's own
//!   check). It **runs a command on a worker thread** ([`jobs`]): the frame that
//!   starts a call returns at once, and the result arrives as a message the next frame draws. The
//!   UI thread never blocks on a call.
//! - **It reads typed results.** A command's view ([`views`]) calls [`Session::call_typed`] and
//!   reads the typed output, so a renamed field is a compile error here, not a blank panel; the
//!   crate never indexes a `serde_json::Value`. The dispatch is the one `{{cli}} run` uses: the same
//!   lookup, requirements, consent and errors, with `Caller::Ui`.
//! - **Its state is data the session holds** ([`Shell::state`]: which command's view is open, whether
//!   the list is shown). It reads it with `ui.inspect` and changes it with `ui.set`, the calls an
//!   agent makes over the socket, so the CLI, MCP and a headless test see and drive the window.
//! - **What it draws is what a command returns.** A view draws its command's typed result as it
//!   came, and a test draws a frame and checks the text against the command's answer.
//! - **Colours from the theme, words from the catalog** ([`theme`], [`catalog`]): drawing code makes
//!   no colour and writes no word, so a theme or a rewording changes one file. An identifier's
//!   colour (a row, a bar, a badge) is a categorical range's pick (`theme.range16.pick(id)`), the
//!   same on every run; the swatch view (`ui.theme`, View › Theme) shows every token and range. The
//!   colours are the palette the swarm's tools share, and `theme::apply` styles egui in them.
//! - **It never writes a file and never starts a program.** A button that does something calls a
//!   command whose effects say so, and the user's consent (a checkbox) is what lets it run.
//!   `scan` holds the crate to this and to the theme and the catalog (`skeleton::UI_SCANS`).
//!
//! **Checking a change by eye**: run the app with its control socket (`{{app}} --socket`), drive it
//! with commands (`ui.set`, then the command whose view you changed) and take a screenshot of the
//! window, before and after, for the PR's Screenshots section (docs/app-architecture.md › The UI is
//! a client).
//!
//! To add a view for a new command: a function in [`views`] that makes its typed call and fills a
//! [`Shown`] (its labels from the [`catalog`]), a line in [`views::views`], and a test that the frame
//! draws the command's answer. A command without a view is listed, and says it has none yet.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

pub mod catalog;
mod draw;
pub mod jobs;
mod shell;
pub mod theme;
pub mod views;

pub use jobs::Wake;
pub use shell::{Call, MenuNode, Row, Shell};
pub use views::{Shown, View};
