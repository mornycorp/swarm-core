//! The automation layer of {{product}}: the transports, each a thin adapter over the engine's one
//! dispatch (docs/app-architecture.md › One dispatch). A transport frames: it reads a request,
//! calls the engine's `Session::call`, writes a reply. It never builds a command spec
//! and never names a command: **adding a command edits nothing here**.
//!
//! - [`serve`]: JSON lines on stdio, the frame of the contract ([`frame`]).
//! - [`mcp`]: an MCP server (`initialize`, `tools/list`, `tools/call`) whose tools are generated
//!   from the registry: a tool per command, its `inputSchema` the command's params schema, its name
//!   by the id rule.
//! - [`socket`]: the loopback control socket, the same frames over TCP on `127.0.0.1` with `auth`
//!   first ([`token`]: where the token comes from, kept private and compared in constant time). It
//!   answers through the same function as `serve`, so it is one more adapter, not another dispatch.
//!   [`control`] has the flags and start-up line the binaries that serve it share.
//! - [`lines`] and [`limits`]: the capped line reader they use, and the contract's caps.
//!
//! Every byte read here is hostile input: a line over the cap is discarded and answered, a line
//! that isn't a request is an error reply, and nothing here panics.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

pub mod control;
pub mod frame;
pub mod limits;
pub mod lines;
pub mod mcp;
pub mod serve;
pub mod socket;
pub mod token;

use serde_json::Value;

use {{crate_prefix_ident}}engine::{CallOpts, Caller, CommandError, Session};

/// What the transport's caller has agreed to.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Consent to every command's effects. Off by default: a command with an effect answers
    /// `consent_required`. MCP sets it only from what its client has granted, never by default.
    pub consent: bool,
}

impl Options {
    /// The options of a call made by `caller`.
    pub fn call_opts(&self, caller: Caller) -> CallOpts {
        let opts = CallOpts::new(caller);
        if self.consent { opts.consenting() } else { opts }
    }
}

/// `commands`: every command's spec with its schemas, in id order, as JSON. The reserved method of
/// the frame, and what the CLI's `commands --json` prints.
pub fn commands_json(session: &Session) -> Result<Value, CommandError> {
    serde_json::to_value(session.commands()).map_err(|e| CommandError::internal(format!("the command specs are not JSON: {e}")))
}

/// `enabled`: whether each command can run now, and why not, in id order, as JSON
/// (`[{"id": "app.info", "enabled": true}, {"id": …, "enabled": false, "why": {"code": "unavailable", …}}]`).
/// The frame's other reserved method, and what the CLI's `enabled` prints: one call, for a menu.
pub fn enabled_json(session: &Session) -> Result<Value, CommandError> {
    serde_json::to_value(session.states()).map_err(|e| CommandError::internal(format!("the command states are not JSON: {e}")))
}
