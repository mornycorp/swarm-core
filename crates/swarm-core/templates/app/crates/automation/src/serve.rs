//! `serve`: the JSON-lines frame on a pair of streams (the CLI's `serve` gives it stdio). One
//! request a line, one reply a line, in order, until the input ends.
//!
//! The method is a command id (the dispatch looks it up), `commands` (the registry's specs with
//! their schemas) or `enabled` (every command's state now, and why not). Nothing here names a
//! command.

use std::io::{self, BufRead, Write};

use serde_json::Value;

use {{crate_prefix_ident}}engine::{CallOpts, Caller, CommandError, Session};

use crate::frame::{self, Reply, Request};
use crate::limits::{MAX_REPLY, MAX_REQUEST};
use crate::lines::{Line, exchange};
use crate::{Options, commands_json, enabled_json};

/// Answers every line of `input` on `output`, until the input ends. An error is only a failed read
/// or write; a bad line is answered, and the next one is read.
pub fn serve(session: &Session, input: impl BufRead, output: impl Write, options: &Options) -> io::Result<()> {
    let opts = options.call_opts(Caller::Serve);
    exchange(input, output, |line| answer(session, line, &opts))
}

/// The reply line for `line`, called with `opts`: none for a blank one. The loopback socket answers
/// its lines with this too ([`crate::socket`]), so a request means the same on every transport.
pub(crate) fn answer(session: &Session, line: Line, opts: &CallOpts) -> Option<String> {
    let reply = match line {
        Line::Eof => return None,
        Line::TooLarge => failure(&CommandError::too_large(format!("the request is over the {MAX_REQUEST}-byte cap"), "send a smaller request")),
        Line::NotUtf8 => failure(&CommandError::bad_params("the line is not UTF-8", frame_advice())),
        Line::Text(text) if text.trim().is_empty() => return None,
        Line::Text(text) => match frame::parse_request(&text) {
            Ok(request) => dispatch(session, request, opts),
            Err(e) => failure(&e),
        },
    };
    Some(reply.encode(MAX_REPLY))
}

fn failure(error: &CommandError) -> Reply {
    Reply::failure(None, error)
}

fn frame_advice() -> &'static str {
    "send UTF-8 text, one JSON request object per line"
}

/// One request through the dispatch.
fn dispatch(session: &Session, request: Request, opts: &CallOpts) -> Reply {
    let Request { id, method, params } = request;
    let result = match method.as_str() {
        "commands" => commands_json(session),
        "enabled" => enabled_json(session),
        _ => session.call(&method, params.unwrap_or(Value::Null), opts),
    };
    match result {
        Ok(value) => Reply::success(id, value),
        Err(e) => Reply::failure(Some(id), &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    use {{crate_prefix_ident}}engine::AppInfo;

    fn session() -> Session {
        Session::new(AppInfo { name: "demo".into(), version: "0.3.0".into() }).unwrap()
    }

    /// Every reply to `input`, parsed.
    fn replies(input: &[u8]) -> Vec<Value> {
        let mut out = Vec::new();
        serve(&session(), Cursor::new(input.to_vec()), &mut out, &Options::default()).unwrap();
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn app_info_over_the_frame() {
        let got = replies(b"{\"id\": 1, \"method\": \"app.info\", \"params\": {}}\n{\"id\": \"b\", \"method\": \"app.info\"}\n");
        let info = json!({"name": "demo", "version": "0.3.0", "schema_version": 1});
        assert_eq!(got, [json!({"id": 1, "ok": true, "result": info}), json!({"id": "b", "ok": true, "result": info})]);
    }

    #[test]
    fn commands_lists_the_registry_with_schemas() {
        let got = replies(b"{\"id\": 2, \"method\": \"commands\"}\n");
        // The example is looked up by id: the commands added beside it must not break this test.
        let info = got[0]["result"].as_array().unwrap().iter().find(|s| s["id"] == "app.info").unwrap();
        assert_eq!(info["params"]["type"], "object");
        assert_eq!(info["effects"]["writes"], false);
        assert_eq!(info["menu"], "Help/App info", "the menu path rides in the spec");
    }

    #[test]
    fn enabled_gives_every_commands_state_in_one_call() {
        let got = replies(b"{\"id\": 3, \"method\": \"enabled\"}\n");
        let states = got[0]["result"].as_array().unwrap();
        assert_eq!(states.len(), session().commands().len(), "one state per command");
        assert!(states.iter().any(|s| *s == json!({"id": "app.info", "enabled": true})), "{states:?}");
    }

    #[test]
    fn failures_carry_the_code_the_remediation_and_the_key() {
        let got = replies(b"{\"id\": 3, \"method\": \"app.nope\"}\n{\"id\": 4, \"method\": \"app.info\", \"params\": {\"x\": 1}}\n");
        assert_eq!((got[0]["ok"].clone(), got[0]["code"].clone()), (json!(false), json!("unknown_command")));
        assert_eq!((got[1]["code"].clone(), got[1]["key"].clone()), (json!("bad_params"), json!("x")));
        assert!(!got[1]["remediation"].as_str().unwrap().is_empty());
    }

    #[test]
    fn a_blank_line_is_skipped_and_a_bad_line_gets_a_null_id_and_the_next_is_answered() {
        let got = replies(b"\n{nope\n\xff\n{\"id\": 5, \"method\": \"app.info\"}\n");
        assert_eq!(got.len(), 3);
        assert_eq!((got[0]["id"].clone(), got[0]["code"].clone()), (Value::Null, json!("bad_params")));
        assert_eq!((got[1]["id"].clone(), got[1]["code"].clone()), (Value::Null, json!("bad_params")));
        assert_eq!(got[2]["ok"], true);
    }

    #[test]
    fn a_request_over_the_cap_is_too_large_and_the_stream_goes_on() {
        let mut input = vec![b'x'; MAX_REQUEST + 1];
        input.extend_from_slice(b"\n{\"id\": 6, \"method\": \"app.info\"}\n");
        let got = replies(&input);
        assert_eq!(got[0]["code"], "too_large");
        assert_eq!(got[1]["id"], 6);
    }

    #[test]
    fn hostile_lines_are_answered_never_fatal() {
        for line in {{crate_prefix_ident}}testkit::adversarial_lines() {
            let mut input = line.clone();
            input.extend_from_slice(b"\n{\"id\": 9, \"method\": \"app.info\"}\n");
            let got = replies(&input);
            let last = got.last().unwrap();
            assert_eq!((last["id"].clone(), last["ok"].clone()), (json!(9), json!(true)));
            assert!(got.iter().all(|r| r["ok"].as_bool().is_some()));
        }
    }
}
