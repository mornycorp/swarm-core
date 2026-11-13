//! `{{app}}`, {{product}}'s window: the UI shell (`{{crate_prefix}}ui`) in an egui window, on the same
//! engine session as every other surface. It adds no command and no logic: a click calls a command on
//! a worker, as `{{cli}} run` does (docs/app-architecture.md › The UI is a client).
//!
//! ```text
//! {{app}}                                          the window
//! {{app}} --socket [--yes] [--token-file <path>] [--port <n>]
//!                                              the window, and the control socket on the same session
//! {{app}} --ui-state <file> [--socket …]           the window, its UI state read from the file at the
//!                                              start and saved to it at the end
//! ```
//!
//! With `--socket` the app also serves the loopback control socket (`{{cli}} serve --socket` has the
//! same flags and the same token rules), so an agent or a test drives the running app, and what it
//! does shows in the window. It prints the start-up line (the address, where the token is) on stderr.
//! An agent that speaks MCP attaches with `{{cli}} mcp --attach <port>` and the same token.
//!
//! The UI's state (which command's view is open, whether the list is shown) is the session's:
//! `ui.inspect` and `ui.set` read and set it over the socket as the window's clicks do, and with
//! `--ui-state` it outlives the window ([`ui_state`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

mod ui_state;
mod window;

use std::process::ExitCode;

fn main() -> ExitCode {
    match window::args(std::env::args_os().skip(1)).and_then(|args| window::run(&args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("{}: {failure}", env!("CARGO_PKG_NAME"));
            ExitCode::from(failure.exit_code())
        }
    }
}
