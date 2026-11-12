//! The JSON-lines frame (docs/app-architecture.md › The JSON-lines frame): one JSON object per
//! line, a request `{id, method, params}` and a reply `{id, ok, result}` or
//! `{id, ok, error, code, remediation, key}`.
//!
//! Requests are read into types, never indexed as a `serde_json::Value`: the only `Value` is the
//! params, which the dispatch checks against the command's own type.

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use {{crate_prefix_ident}}engine::{CommandError, ErrorCode, clip};

/// What a request calls itself, and the reply echoes: a number or a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(Number),
    Text(String),
}

/// A request: `method` is a command id or a reserved method (`commands`, `enabled`). Absent or `null` params
/// are `{}`; any other that isn't an object is `bad_params`, from the dispatch.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: Id,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// What a line that isn't a request is told.
const REQUEST_SHAPE: &str = "send one JSON object per line: {\"id\": 1, \"method\": \"app.info\", \"params\": {}}";

/// The request in `line`, or the `bad_params` to answer a line that isn't one.
pub fn parse_request(line: &str) -> Result<Request, CommandError> {
    serde_json::from_str(line).map_err(|e| CommandError::bad_params(format!("not a request: {}", clip(&e.to_string())), REQUEST_SHAPE))
}

/// A reply: `ok` with a `result`, or not, with the failure's `error` (its message), `code`,
/// `remediation` and `key`. The `id` is null for a line that wasn't a request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reply {
    id: Option<Id>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

impl Reply {
    pub fn success(id: Id, result: Value) -> Reply {
        Reply { id: Some(id), ok: true, result: Some(result), error: None, code: None, remediation: None, key: None }
    }

    pub fn failure(id: Option<Id>, error: &CommandError) -> Reply {
        Reply {
            id,
            ok: false,
            result: None,
            error: Some(error.message.clone()),
            code: Some(error.code),
            remediation: Some(error.remediation.clone()),
            key: error.key.clone(),
        }
    }

    /// The reply as one line: when it is over `max` bytes, a `too_large` failure instead, never a
    /// truncated result.
    pub fn encode(&self, max: usize) -> String {
        match serde_json::to_string(self) {
            Ok(line) if line.len() <= max => line,
            Ok(_) => {
                let e = CommandError::too_large(format!("the reply is over the {max}-byte cap"), "narrow what the call asks for, or ask for less at a time");
                Reply::failure(self.id.clone(), &e).encode_small()
            }
            Err(_) => INTERNAL.to_owned(),
        }
    }

    /// A reply that is small by construction (a failure with no result).
    fn encode_small(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| INTERNAL.to_owned())
    }
}

/// The reply for a reply that could not be written at all.
const INTERNAL: &str = "{\"id\":null,\"ok\":false,\"error\":\"the reply could not be written\",\"code\":\"internal\",\"remediation\":\"report this: it is a bug in the app, not in the call\"}";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_has_an_id_a_method_and_optional_params() {
        let r = parse_request("{\"id\": 1, \"method\": \"app.info\", \"params\": {}}").unwrap();
        assert_eq!((r.id, r.method.as_str(), r.params), (Id::Number(1.into()), "app.info", Some(json!({}))));
        let r = parse_request("{\"id\": \"a\", \"method\": \"commands\"}").unwrap();
        assert_eq!((r.id, r.params), (Id::Text("a".into()), None));
        assert_eq!(parse_request("{\"id\": 1.5, \"method\": \"x.y\", \"params\": null, \"extra\": true}").unwrap().params, None);
    }

    #[test]
    fn a_line_that_is_not_a_request_is_bad_params_with_what_to_send() {
        for line in
            ["", "{", "null", "[]", "{\"id\": 1}", "{\"id\": null, \"method\": \"a.b\"}", "{\"id\": 1, \"method\": 5}", "{\"id\": [], \"method\": \"a.b\"}"]
        {
            let e = parse_request(line).unwrap_err();
            assert_eq!(e.code, ErrorCode::BadParams, "{line}");
            assert!(e.remediation.contains("\"method\"") && e.message.chars().count() < 300, "{line}: {e}");
        }
    }

    #[test]
    fn a_reply_is_ok_with_a_result_or_not_with_the_failure() {
        let ok = Reply::success(Id::Number(1.into()), json!({"name": "x"}));
        assert_eq!(ok.encode(1000), "{\"id\":1,\"ok\":true,\"result\":{\"name\":\"x\"}}");
        let e = CommandError::bad_params("missing field `path`", "pass `path`, a file path").with_key("path");
        let line = Reply::failure(Some(Id::Text("a".into())), &e).encode(1000);
        let want =
            json!({"id": "a", "ok": false, "error": "missing field `path`", "code": "bad_params", "remediation": "pass `path`, a file path", "key": "path"});
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), want);
        let no_id = Reply::failure(None, &CommandError::cancelled()).encode(1000);
        assert!(no_id.starts_with("{\"id\":null,\"ok\":false,") && !no_id.contains("\"key\""), "{no_id}");
    }

    #[test]
    fn a_reply_over_its_cap_is_too_large_never_truncated() {
        let big = Reply::success(Id::Number(7.into()), json!("x".repeat(500)));
        let line = big.encode(300);
        assert!(line.len() < 300 && line.contains("\"too_large\"") && line.contains("\"id\":7") && line.contains("\"ok\":false"), "{line}");
        assert!(serde_json::from_str::<Value>(INTERNAL).is_ok());
    }
}
