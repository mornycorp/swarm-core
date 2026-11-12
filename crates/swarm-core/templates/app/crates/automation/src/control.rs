//! Starting the control socket from a command line: the options and the start-up line that every
//! binary which serves it shares (the CLI's `serve --socket`, the windowed app's `--socket`), so
//! they take the same flags and say the same thing.
//!
//! ```text
//! [--yes] [--token-file <path>] [--port <n>]
//! ```
//!
//! - `--token-file <path>`: where the token is, in a directory only the user can read. On Unix it
//!   is read (when it is the user's own and `0600`) or created `0600`; on Windows a new token is
//!   written there at every start ([`token`](crate::token) says why). `{{env}}_CONTROL_TOKEN`
//!   ([`ENV`](crate::token::ENV)) wins over it. With neither the start fails and says so: there is
//!   no default location, because a fixed path in a shared directory is one another user could
//!   have made first.
//! - `--port <n>`: the port on `127.0.0.1` (default: a free one, which the start-up line says).
//! - `--yes`: every connection has agreed to the commands' effects (`consent`), as the CLI's `--yes`.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use {{crate_prefix_ident}}engine::{CommandError, Session};

use crate::Options;
use crate::socket::{Config, Server};
use crate::token::Token;

/// What the control-socket flags asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControlArgs {
    pub token_file: Option<String>,
    pub port: Option<u16>,
    pub yes: bool,
}

/// The flags `args` hold, or what is wrong with them.
pub fn parse(args: &[&str]) -> Result<ControlArgs, String> {
    let mut found = ControlArgs::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--yes" => found.yes = true,
            "--token-file" => found.token_file = Some(rest.next().ok_or("`--token-file` needs a path")?.to_string()),
            "--port" => {
                let port = rest.next().ok_or("`--port` needs a number")?;
                found.port = Some(port.parse().map_err(|_| format!("`--port` takes a number from 0 to 65535, not `{}`", {{crate_prefix_ident}}engine::clip(port)))?);
            }
            other => return Err(format!("no option `{}` here (the options are --yes, --token-file <path>, --port <n>)", {{crate_prefix_ident}}engine::clip(other))),
        }
    }
    Ok(found)
}

/// The line a server prints when it is listening: JSON, so a script reads the port from it. It
/// says where the token is, never what it is.
#[derive(Debug, Serialize)]
struct Listening {
    listening: String,
    token: String,
}

/// Starts the control socket on `session` as `args` say. The server, and the start-up line.
pub fn start(session: Arc<Session>, args: &ControlArgs) -> Result<(Server, String), CommandError> {
    let (token, source) = Token::resolve(args.token_file.as_deref().map(Path::new))?;
    let mut config = Config::new(token, Options { consent: args.yes });
    config.port = args.port.unwrap_or(0);
    let server = Server::start(session, config).map_err(|e| {
        CommandError::unavailable(
            format!("cannot listen on 127.0.0.1:{}: {e}", args.port.unwrap_or(0)),
            "pick another `--port`, or leave it out for a free one",
        )
    })?;
    let line = serde_json::to_string(&Listening { listening: server.addr().to_string(), token: source.to_string() })
        .map_err(|e| CommandError::internal(format!("the start-up line is not JSON: {e}")))?;
    Ok((server, line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<ControlArgs, String> {
        parse(list)
    }

    #[test]
    fn the_flags_are_read_and_a_wrong_one_says_what_is_taken() {
        assert_eq!(args(&[]).unwrap(), ControlArgs::default());
        let all = args(&["--yes", "--token-file", "t.txt", "--port", "4000"]).unwrap();
        assert_eq!(all, ControlArgs { token_file: Some("t.txt".into()), port: Some(4000), yes: true });
        for bad in [&["--port"][..], &["--port", "x"], &["--port", "70000"], &["--token-file"], &["--nope"], &["stray"]] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
    }
}
