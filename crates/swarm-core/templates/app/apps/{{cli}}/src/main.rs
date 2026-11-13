//! `{{cli}}`, {{product}}'s command line: a thin adapter over the engine's one dispatch, like every
//! other surface (docs/app-architecture.md › One dispatch).
//!
//! ```text
//! {{cli}} run <command> [json] [--yes]   run a command with JSON params, print its result
//! {{cli}} commands [--json]              list the commands (with their schemas, as JSON)
//! {{cli}} serve [--yes]                  JSON lines on stdin and stdout
//! {{cli}} mcp                            an MCP server on stdin and stdout
//! {{cli}} mcp --attach <port> [--token-file <path>]
//!                                        the same MCP server, on a running app's session
//! ```
//!
//! `mcp --attach` runs its commands on an app that is already running (the window with `--socket`,
//! or `{{cli}} serve --socket`), over that app's loopback control socket on `127.0.0.1:<port>`, with
//! the app's token (`{{env}}_CONTROL_TOKEN`, or the token file). It gives the answers `mcp` gives,
//! and never consents to a command's effects, even when the app's socket was started with `--yes`.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    let stdin = std::io::stdin();
    let (stdout, stderr) = (std::io::stdout(), std::io::stderr());
    ExitCode::from(cli::run_os(std::env::args_os().skip(1), stdin.lock(), &mut stdout.lock(), &mut stderr.lock()))
}
