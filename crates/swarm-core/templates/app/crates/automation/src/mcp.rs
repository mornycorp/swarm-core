//! An MCP server on a pair of streams (the CLI's `mcp` gives it stdio): JSON-RPC 2.0, one message a
//! line, as MCP's stdio transport has it. `initialize`, `ping`, `tools/list` and `tools/call`.
//!
//! **The tools are generated from the registry** (docs/app-architecture.md › One dispatch): a tool
//! per command, named by the id rule ([`CommandId::tool_name`]: `app.info` is `app-info`), with the
//! command's title, its summary as the description, its params schema as the `inputSchema`, and its
//! menu path and shortcut, when it has them, in `_meta`. A call goes through [`Session::call`]; MCP
//! has no method for the commands' enabled states, so a disabled command's call is how a client
//! learns it is: an `unavailable` result that says why. A command that fails is a tool result with `isError`, whose
//! text is the failure as JSON (`code`, `message`, `remediation`, `key`); a tool that doesn't exist
//! is a JSON-RPC error, and so is a request over the cap, each with the failure's stable code and
//! what to do about it in `error.data` (`{"code": "unknown_command", "remediation": "…"}`). **Consent** is never granted by default: a command whose effects need it
//! answers `consent_required` until the app's `Options` say its client has granted it.
//!
//! **One adapter, two sides** ([`Backend`]: list the commands' specs, call one). [`serve`] answers
//! from a session of its own ([`Local`], the CLI's `mcp`); [`serve_on`] from any backend, such as
//! [`Attached`](crate::attach::Attached), a running app's session over its control socket (the CLI's
//! `mcp --attach <port>`). `tools/list`, `tools/call`, their error mapping and the JSON-RPC handling
//! are this one module on both sides, so an attached server gives the answers a direct one does. A
//! backend that can't answer (no app on the port, a wrong token, a connection that dropped) is a
//! JSON-RPC error with the failure's code and remediation in `error.data`, as a tool that is no
//! command is.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use {{crate_prefix_ident}}engine::{AppInfo, CallOpts, Caller, CommandError, ErrorCode, Session, clip};
use {{crate_prefix_ident}}model::ids::CommandId;

use crate::frame::Id;
use crate::limits::{MAX_REPLY, MAX_REQUEST};
use crate::lines::{Line, exchange};
use crate::{Options, commands_json};

/// A call's outcome: the command's result, or its failure.
pub type Outcome = Result<Value, CommandError>;

/// What MCP answers through: the commands' specs, and a call of one. It names no command. A
/// method's `Err` is the backend failing to answer at all (it can't reach the app); a command that
/// fails is an `Ok` outcome that is an `Err`.
pub trait Backend {
    /// What `initialize` says the server is.
    fn app(&self) -> &AppInfo;

    /// Every command's spec, in id order, as the frame's `commands` gives them.
    fn specs(&mut self) -> Result<Vec<Spec>, CommandError>;

    /// One call of the command `id` with `params` (JSON; `null` is `{}`), as MCP makes it: with only
    /// the consent the MCP client has granted.
    fn call(&mut self, id: &str, params: Value) -> Result<Outcome, CommandError>;
}

/// A command's spec as MCP reads it: the frame's `commands` entry, read into a type (its result
/// schema and requirements are not needed here, and a member this doesn't know is ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct Spec {
    pub id: String,
    pub title: String,
    pub summary: String,
    /// The params schema: the tool's `inputSchema`.
    pub params: Value,
    /// The effects by name (`writes`, `runs_code`, `network`, `device`), each `true` or `false`.
    #[serde(default)]
    effects: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub menu: Option<String>,
    #[serde(default)]
    pub shortcut: Option<String>,
}

impl Spec {
    /// The specs in a `commands` result, or `unavailable` when it is not a list of them.
    pub fn list(commands: Value) -> Result<Vec<Spec>, CommandError> {
        serde_json::from_value(commands).map_err(|e| {
            CommandError::unavailable(
                format!("the command specs are not what this MCP server reads: {}", clip(&e.to_string())),
                "attach to an app built from the same version as this binary",
            )
        })
    }

    /// Whether the command's effects need consent: **any effect that isn't `false`** does, so an
    /// effect named after this server was built, or effects that are missing, are never read as
    /// none. For the effects the engine has, it is `Effects::needs_consent`.
    pub fn needs_consent(&self) -> bool {
        self.effects.as_ref().is_none_or(|effects| effects.values().any(|effect| *effect != Value::Bool(false)))
    }
}

/// The local side: a session in this process (the CLI's `mcp`), its calls made with the options
/// the MCP client has granted.
#[derive(Debug)]
pub struct Local<'a> {
    session: &'a Session,
    opts: CallOpts,
}

impl<'a> Local<'a> {
    pub fn new(session: &'a Session, options: &Options) -> Local<'a> {
        Local { session, opts: options.call_opts(Caller::Mcp) }
    }
}

impl Backend for Local<'_> {
    fn app(&self) -> &AppInfo {
        self.session.app()
    }

    fn specs(&mut self) -> Result<Vec<Spec>, CommandError> {
        Spec::list(commands_json(self.session)?)
    }

    fn call(&mut self, id: &str, params: Value) -> Result<Outcome, CommandError> {
        Ok(self.session.call(id, params, &self.opts))
    }
}

/// The protocol version answered when the client asks for one this server doesn't know.
const LATEST: &str = "2025-06-18";
/// The versions this server speaks: the messages it uses are the same in each.
const SUPPORTED: [&str; 3] = [LATEST, "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
/// The app behind the server couldn't answer (JSON-RPC's range for a server's own errors).
const SERVER_ERROR: i32 = -32000;

/// Serves MCP on `session` until the input ends. An error is only a failed read or write.
pub fn serve(session: &Session, input: impl BufRead, output: impl Write, options: &Options) -> io::Result<()> {
    serve_on(&mut Local::new(session, options), input, output)
}

/// Serves MCP until the input ends, answering through `backend`. An error is only a failed read or
/// write: a backend that can't answer is a reply that says so.
pub fn serve_on(backend: &mut dyn Backend, input: impl BufRead, output: impl Write) -> io::Result<()> {
    exchange(input, output, |line| match line {
        Line::Eof => None,
        Line::TooLarge => Some(
            Response::error(None, INVALID_REQUEST, format!("the request is over the {MAX_REQUEST}-byte cap"))
                .coded(ErrorCode::TooLarge, "send a smaller request")
                .encode(),
        ),
        Line::NotUtf8 => Some(Response::error(None, PARSE_ERROR, "the line is not UTF-8".to_owned()).encode()),
        Line::Text(text) if text.trim().is_empty() => None,
        Line::Text(text) => handle(backend, &text).map(|response| response.encode()),
    })
}

/// A JSON-RPC message that has an `id` (a request) or hasn't (a notification: no reply). `id` is
/// `None` when the member is absent and `Some(None)` when it is `null`, which a request may not be
/// (it would be answered as a notification is not, and look like one): that is an invalid request.
#[derive(Debug, Deserialize)]
struct Rpc {
    jsonrpc: String,
    #[serde(default, deserialize_with = "present")]
    id: Option<Option<Id>>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// A member that is there, `null` or not: `Some`, so an absent one (the default) tells from it.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Option<Id>>, D::Error> {
    Option::<Id>::deserialize(deserializer).map(Some)
}

/// The version every message must name.
const JSONRPC: &str = "2.0";

/// A reply to a request.
#[derive(Debug, Serialize)]
struct Response {
    jsonrpc: &'static str,
    id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

/// A JSON-RPC error. When it is one of the contract's failures (a tool that is no command, a request
/// or a reply over its cap) `data.code` is its stable code (`unknown_command`, `too_large`) and
/// `data.remediation` what to do about it, so a client classifies it as it does a tool result's,
/// by code and not by message.
#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i32, message: impl Into<String>) -> RpcError {
        RpcError { code, message: message.into(), data: None }
    }

    /// The same, with the contract's stable `code` and its `remediation` in `data`.
    #[must_use]
    fn coded(mut self, code: ErrorCode, remediation: &str) -> RpcError {
        self.data = Some(json!({ "code": code, "remediation": remediation }));
        self
    }
}

impl Response {
    fn result(id: Id, result: Value) -> Response {
        Response { jsonrpc: "2.0", id: Some(id), result: Some(result), error: None }
    }

    fn error(id: Option<Id>, code: i32, message: String) -> Response {
        Response { jsonrpc: "2.0", id, result: None, error: Some(RpcError::new(code, message)) }
    }

    /// The same, with the contract's stable `code` and its `remediation` in the error's `data`.
    #[must_use]
    fn coded(mut self, code: ErrorCode, remediation: &str) -> Response {
        self.error = self.error.map(|e| e.coded(code, remediation));
        self
    }

    /// One line; a reply over the cap is an error instead, never a truncated result.
    fn encode(&self) -> String {
        match serde_json::to_string(self) {
            Ok(line) if line.len() <= MAX_REPLY => line,
            Ok(_) => {
                let message = format!("too_large: the reply is over the {MAX_REPLY}-byte cap: narrow what the call asks for");
                Response::error(self.id.clone(), INTERNAL_ERROR, message)
                    .coded(ErrorCode::TooLarge, "narrow what the call asks for, or ask for less at a time")
                    .encode_small()
            }
            Err(_) => "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"the reply could not be written\"}}".to_owned(),
        }
    }

    fn encode_small(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| UNWRITABLE.to_owned())
    }
}

/// The reply when a reply can't be written: never an empty line.
const UNWRITABLE: &str = "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"the reply could not be written\"}}";

/// The reply to the message in `text`, if it is one that gets a reply.
fn handle(backend: &mut dyn Backend, text: &str) -> Option<Response> {
    let request: Rpc = match serde_json::from_str(text) {
        Ok(request) => request,
        Err(e) => {
            let code = if e.is_data() { INVALID_REQUEST } else { PARSE_ERROR };
            return Some(Response::error(None, code, format!("not a JSON-RPC message: {}", clip(&e.to_string()))));
        }
    };
    let id = match request.id.clone() {
        Some(None) => return Some(Response::error(None, INVALID_REQUEST, "a request's id must be a number or a string, not null".to_owned())),
        id => id.flatten(),
    };
    if request.jsonrpc != JSONRPC {
        return Some(Response::error(id, INVALID_REQUEST, format!("`jsonrpc` must be \"{JSONRPC}\", not \"{}\"", clip(&request.jsonrpc))));
    }
    let outcome = route(backend, &request);
    let id = id?;
    Some(match outcome {
        Ok(result) => Response::result(id, result),
        Err(e) => Response { jsonrpc: "2.0", id: Some(id), result: None, error: Some(e) },
    })
}

fn route(backend: &mut dyn Backend, request: &Rpc) -> Result<Value, RpcError> {
    let params = request.params.clone();
    match request.method.as_str() {
        "initialize" => initialize(backend.app(), params),
        "ping" => Ok(json!({})),
        "tools/list" => tools_list(backend),
        "tools/call" => tools_call(backend, params),
        other => Err(RpcError::new(METHOD_NOT_FOUND, format!("no method `{}`", clip(other)))),
    }
}

/// `params` as the type `T`: anything else is `invalid params`.
fn typed<T: DeserializeOwned>(params: Option<Value>) -> Result<T, RpcError> {
    serde_json::from_value(params.unwrap_or(Value::Null)).map_err(|e| RpcError::new(INVALID_PARAMS, format!("invalid params: {}", clip(&e.to_string()))))
}

fn to_value(value: &impl Serialize) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|e| RpcError::new(INTERNAL_ERROR, format!("not JSON: {e}")))
}

#[derive(Debug, Deserialize)]
struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
}

#[derive(Debug, Serialize)]
struct Initialized {
    #[serde(rename = "protocolVersion")]
    protocol_version: &'static str,
    capabilities: Capabilities,
    #[serde(rename = "serverInfo")]
    server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
struct Capabilities {
    tools: ToolsCapability,
}

#[derive(Debug, Serialize)]
struct ToolsCapability {}

#[derive(Debug, Serialize)]
struct ServerInfo {
    name: String,
    version: String,
}

fn initialize(app: &AppInfo, params: Option<Value>) -> Result<Value, RpcError> {
    let asked: InitializeParams = typed(params)?;
    let protocol_version = SUPPORTED.iter().copied().find(|v| *v == asked.protocol_version).unwrap_or(LATEST);
    to_value(&Initialized {
        protocol_version,
        capabilities: Capabilities { tools: ToolsCapability {} },
        server_info: ServerInfo { name: app.name.clone(), version: app.version.clone() },
    })
}

#[derive(Debug, Serialize)]
struct Tool<'a> {
    name: String,
    title: &'a str,
    description: &'a str,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
    annotations: Annotations,
    /// The command's menu path and shortcut, when it has either: MCP's place for what a tool's
    /// other fields don't say.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    meta: Option<Meta<'a>>,
}

#[derive(Debug, Serialize)]
struct Meta<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    menu: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shortcut: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct Annotations {
    #[serde(rename = "readOnlyHint")]
    read_only_hint: bool,
}

#[derive(Debug, Serialize)]
struct ToolList<'a> {
    tools: Vec<Tool<'a>>,
}

/// The tool for `spec`: none when its id doesn't parse (the registry only holds ids that do).
fn tool(spec: &Spec) -> Option<Tool<'_>> {
    let id = CommandId::parse(&spec.id).ok()?;
    let (menu, shortcut) = (spec.menu.as_deref(), spec.shortcut.as_deref());
    Some(Tool {
        name: id.tool_name(),
        title: &spec.title,
        description: &spec.summary,
        input_schema: spec.params.clone(),
        annotations: Annotations { read_only_hint: !spec.needs_consent() },
        meta: (menu.is_some() || shortcut.is_some()).then_some(Meta { menu, shortcut }),
    })
}

/// A backend that couldn't answer, as a JSON-RPC error with the failure's code and remediation.
fn unanswered(e: &CommandError) -> RpcError {
    RpcError::new(SERVER_ERROR, e.message.clone()).coded(e.code, &e.remediation)
}

fn tools_list(backend: &mut dyn Backend) -> Result<Value, RpcError> {
    let specs = backend.specs().map_err(|e| unanswered(&e))?;
    to_value(&ToolList { tools: specs.iter().filter_map(tool).collect() })
}

#[derive(Debug, Deserialize)]
struct CallParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Debug, Serialize)]
struct Content {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
struct ToolResult {
    content: Vec<Content>,
    #[serde(rename = "isError")]
    is_error: bool,
}

fn unknown_tool(name: &str) -> RpcError {
    RpcError::new(INVALID_PARAMS, format!("unknown tool `{}`: `tools/list` has the tools", clip(name)))
        .coded(ErrorCode::UnknownCommand, "call a tool `tools/list` names")
}

fn tools_call(backend: &mut dyn Backend, params: Option<Value>) -> Result<Value, RpcError> {
    let call: CallParams = typed(params)?;
    let id = CommandId::from_tool_name(&call.name).map_err(|_| unknown_tool(&call.name))?;
    let outcome = backend.call(id.as_str(), call.arguments.unwrap_or(Value::Null)).map_err(|e| unanswered(&e))?;
    match outcome {
        Err(e) if e.code == ErrorCode::UnknownCommand => Err(unknown_tool(&call.name)),
        other => tool_result(other),
    }
}

/// A call's outcome as a tool result: the result as JSON text, or the failure as JSON text with
/// `isError`.
fn tool_result(outcome: Outcome) -> Result<Value, RpcError> {
    let (text, is_error) = match outcome {
        Ok(value) => (value.to_string(), false),
        Err(e) => (serde_json::to_string(&e).map_err(|e| RpcError::new(INTERNAL_ERROR, e.to_string()))?, true),
    };
    to_value(&ToolResult { content: vec![Content { kind: "text", text }], is_error })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use {{crate_prefix_ident}}engine::AppInfo;

    fn session() -> Session {
        Session::new(AppInfo { name: "demo".into(), version: "0.3.0".into() }).unwrap()
    }

    /// The replies to `messages` (one a line), parsed.
    fn replies(messages: &[&str]) -> Vec<Value> {
        let input = format!("{}\n", messages.join("\n"));
        let mut out = Vec::new();
        serve(&session(), Cursor::new(input.into_bytes()), &mut out, &Options::default()).unwrap();
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn initialize_answers_with_the_clients_version_when_known() {
        let got = replies(&[
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{}}}",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"1999-01-01\"}}",
        ]);
        assert_eq!(got[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(got[1]["result"]["protocolVersion"], LATEST);
        assert_eq!(got[0]["result"]["serverInfo"], json!({"name": "demo", "version": "0.3.0"}));
        assert_eq!(got[0]["result"]["capabilities"], json!({"tools": {}}));
    }

    #[test]
    fn tools_are_generated_from_the_registry_by_the_id_rule() {
        let got = replies(&["{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}"]);
        let tools = got[0]["result"]["tools"].as_array().unwrap();
        // The example is looked up by name: the commands added beside it must not break this test.
        let tool = tools.iter().find(|t| t["name"] == "app-info").unwrap();
        assert_eq!(tool["description"], "The app's name, version and schema version");
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["_meta"], json!({"menu": "Help/App info"}), "the menu path rides in `_meta`, and an absent shortcut is left out");
        assert!(tools.iter().all(|t| t["name"].as_str().is_some_and(|n| !n.contains('.'))), "no tool name has a dot");
    }

    #[test]
    fn a_tool_call_goes_through_the_dispatch() {
        let got = replies(&["{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"app-info\",\"arguments\":{}}}"]);
        assert_eq!(got[0]["id"], 3);
        let result = &got[0]["result"];
        assert_eq!(result["isError"], false);
        let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, json!({"name": "demo", "version": "0.3.0", "schema_version": 1}));
    }

    #[test]
    fn a_failing_command_is_an_error_result_and_an_unknown_tool_a_protocol_error() {
        let got = replies(&[
            "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"app-info\",\"arguments\":{\"x\":1}}}",
            "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"app-nope\"}}",
            "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"app.info\"}}",
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{}}",
        ]);
        assert_eq!(got[0]["result"]["isError"], true);
        let failure: Value = serde_json::from_str(got[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!((failure["code"].clone(), failure["key"].clone()), (json!("bad_params"), json!("x")));
        for unknown in [&got[1], &got[2], &got[3]] {
            assert_eq!(unknown["error"]["code"], INVALID_PARAMS, "{unknown}");
        }
        assert_eq!(got[1]["error"]["data"]["code"], "unknown_command", "a tool that is no command says so by code");
        assert!(got[1]["error"]["data"]["remediation"].as_str().is_some_and(|r| r.contains("tools/list")), "and what to do: {}", got[1]);
    }

    #[test]
    fn a_request_over_the_cap_is_a_protocol_error_with_the_code_too_large_and_the_stream_goes_on() {
        let big = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":\"{}\"}}\n", "a".repeat(MAX_REQUEST));
        let input = format!("{big}{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}}\n");
        let mut out = Vec::new();
        serve(&session(), Cursor::new(input.into_bytes()), &mut out, &Options::default()).unwrap();
        let got: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!((got[0]["error"]["code"].clone(), got[0]["error"]["data"]["code"].clone()), (json!(INVALID_REQUEST), json!("too_large")));
        assert!(got[0]["error"]["data"]["remediation"].as_str().is_some_and(|r| !r.is_empty()), "{}", got[0]);
        assert_eq!(got[1]["id"], 2);
    }

    #[test]
    fn notifications_get_no_reply_and_ping_and_unknown_methods_do() {
        let got = replies(&[
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"resources/list\"}",
        ]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["result"], json!({}));
        assert_eq!(got[1]["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn hostile_lines_are_answered_with_a_null_id_and_the_stream_goes_on() {
        for line in {{crate_prefix_ident}}testkit::adversarial_lines() {
            let mut input = line.clone();
            input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n");
            let mut out = Vec::new();
            serve(&session(), Cursor::new(input), &mut out, &Options::default()).unwrap();
            let last: Value = serde_json::from_str(String::from_utf8(out).unwrap().lines().last().unwrap()).unwrap();
            assert_eq!(last["id"], 9);
        }
        let got = replies(&["{", "[]"]);
        assert_eq!((got[0]["error"]["code"].clone(), got[0]["id"].clone()), (json!(PARSE_ERROR), Value::Null));
        assert_eq!(got[1]["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn a_null_id_or_a_wrong_jsonrpc_is_an_invalid_request_and_nothing_runs() {
        let got = replies(&[
            "{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"ping\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"tools/call\",\"params\":{\"name\":\"app-info\"}}",
            "{\"jsonrpc\":\"1.0\",\"id\":3,\"method\":\"ping\"}",
            "{\"id\":4,\"method\":\"ping\"}",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"ping\"}",
        ]);
        assert_eq!(got.len(), 5, "a null id is answered, not run as a notification: {got:?}");
        for (reply, id) in got.iter().zip([Value::Null, Value::Null, json!(3), Value::Null]) {
            assert_eq!((reply["error"]["code"].clone(), reply["id"].clone()), (json!(INVALID_REQUEST), id), "{reply}");
            assert!(reply.get("result").is_none(), "{reply}");
        }
        assert_eq!((got[4]["id"].clone(), got[4]["result"].clone()), (json!(5), json!({})));
    }

    #[test]
    fn a_spec_needs_consent_for_any_effect_that_is_not_false() {
        let spec = |effects: Value| {
            let spec = json!({"id": "a.b", "title": "t", "summary": "s", "params": {}, "effects": effects});
            serde_json::from_value::<Spec>(spec).unwrap()
        };
        for bits in 0..16_u8 {
            let effects = {{crate_prefix_ident}}engine::Effects { writes: bits & 1 != 0, runs_code: bits & 2 != 0, network: bits & 4 != 0, device: bits & 8 != 0 };
            assert_eq!(spec(serde_json::to_value(effects).unwrap()).needs_consent(), effects.needs_consent(), "{effects:?}");
        }
        assert!(spec(json!({"writes": false, "teleports": true})).needs_consent(), "an effect named later still needs it");
        assert!(spec(json!({"writes": "no"})).needs_consent(), "only `false` is no effect");
        assert!(spec(Value::Null).needs_consent() && !spec(json!({})).needs_consent());
        let none: Spec = serde_json::from_value(json!({"id": "a.b", "title": "t", "summary": "s", "params": {}})).unwrap();
        assert!(none.needs_consent(), "a spec without effects is never read as one without");
    }

    #[test]
    fn a_reply_is_never_an_empty_line() {
        let small = Response::error(None, INTERNAL_ERROR, "x".to_owned()).encode_small();
        assert!(!small.is_empty() && serde_json::from_str::<Value>(&small).is_ok());
        assert!(serde_json::from_str::<Value>(UNWRITABLE).is_ok_and(|v| v["error"]["code"] == INTERNAL_ERROR));
    }
}
