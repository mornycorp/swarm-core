//! The conformance run, run against a fake app: one that conforms passes, and each rule has a
//! fake that breaks it and fails with a message that names the rule and the command.

use serde_json::{Value, json};

use super::*;

/// What a fake does wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    NoParamsSchema,
    BadId,
    McpNoTool,
    McpWrongInputSchema,
    McpExtraTool,
    UnknownCommandOnMcp,
    UnknownKeyUnnamed,
    MissingKeyWrong,
    WrongTypeCoerced,
    WrongTypeUnnamed,
    NoCapOnSocket,
    PanicOnNullOnServe,
    PanicInCli,
    CliDiffers,
    CliErrorsDiffer,
    EmptyRemediation,
    BadExample,
    RunsWithoutConsentOnCli,
    McpConsents,
    StatesDifferOnServe,
    LockedRunsOnCli,
    LockedOtherReasonOnSocket,
    NoStateOnCli,
    DisabledNotUnavailable,
}

/// What `demo.locked`'s state and its call say to do.
const UNLOCK: &str = "unlock the demo first";

/// A four-command app whose surfaces are one function: `demo.echo` (a required `text` string),
/// `demo.ping` (no params), `demo.write` (`demo.echo`'s params, and an effect that needs consent)
/// and `demo.locked` (no params, and always disabled). Every surface but MCP answers the enabled
/// query.
struct Fake {
    fault: Option<Fault>,
    /// Whether the app's parser names the key of a wrong-typed value.
    names_type_key: bool,
    /// Whether the app opts out of the run requiring that key ([`App::names_key_for_wrong_type`]).
    opts_out_of_type_key: bool,
    surfaces: Vec<Surface>,
}

impl Fake {
    fn good() -> Fake {
        Fake { fault: None, names_type_key: true, opts_out_of_type_key: false, surfaces: Surface::ALL.to_vec() }
    }

    fn with(fault: Fault) -> Fake {
        Fake { fault: Some(fault), ..Fake::good() }
    }

    fn is(&self, fault: Fault) -> bool {
        self.fault == Some(fault)
    }

    fn failure(code: &str, key: Option<&str>) -> Outcome {
        Err(Failure::new(code, format!("fix `{code}`"), key.map(str::to_owned)))
    }

    fn echo(&self, surface: Surface, params: &Value) -> Outcome {
        let empty = serde_json::Map::new();
        let fields = match params {
            Value::Null => &empty,
            Value::Object(fields) => fields,
            _ => return Fake::failure("bad_params", None),
        };
        if let Some(extra) = fields.keys().find(|k| *k != "text") {
            // The CLI forgets the key of a key it can't print (an empty one): not a rule of its own, only a difference.
            let unnamed = self.is(Fault::UnknownKeyUnnamed) || (self.is(Fault::CliErrorsDiffer) && surface == Surface::Cli && extra.is_empty());
            return Fake::failure("bad_params", if unnamed { None } else { Some(extra.as_str()) });
        }
        let Some(text) = fields.get("text") else {
            return Fake::failure("bad_params", Some(if self.is(Fault::MissingKeyWrong) { "other" } else { "text" }));
        };
        let Some(text) = text.as_str() else {
            if self.is(Fault::WrongTypeCoerced) {
                return Ok(json!({ "text": text.to_string() }));
            }
            let unnamed = self.is(Fault::WrongTypeUnnamed) || !self.names_type_key;
            return Fake::failure("bad_params", if unnamed { None } else { Some("text") });
        };
        let text = if self.is(Fault::CliDiffers) && surface == Surface::Cli { text.to_uppercase() } else { text.to_owned() };
        Ok(json!({ "text": text }))
    }

    /// `demo.write`: `demo.echo`'s params check, then the consent its effect needs. MCP can't
    /// carry consent unless the fake grants it by fault.
    fn write(&self, surface: Surface, params: &Value, consent: bool) -> Outcome {
        let text = self.echo(surface, params)?;
        let carried = consent && (surface != Surface::Mcp || self.is(Fault::McpConsents));
        let skipped = self.is(Fault::RunsWithoutConsentOnCli) && surface == Surface::Cli;
        if carried || skipped { Ok(json!({ "written": text["text"] })) } else { Fake::failure("consent_required", None) }
    }

    fn dispatch(&self, call: Call<'_>) -> Outcome {
        let Call { surface, id, params, consent } = call;
        if self.is(Fault::PanicOnNullOnServe) && surface == Surface::Serve && params.is_null() {
            panic!("a null param reached an unwrap");
        }
        if self.is(Fault::PanicInCli) && surface == Surface::Cli && params.get("path").is_some() {
            panic!("a path was trusted");
        }
        let wire = matches!(surface, Surface::Serve | Surface::Socket | Surface::Mcp);
        if wire && params.to_string().len() > REQUEST_CAP && !(self.is(Fault::NoCapOnSocket) && surface == Surface::Socket) {
            return Fake::failure("too_large", None);
        }
        let no_params = params.is_null() || params.as_object().is_some_and(serde_json::Map::is_empty);
        match id {
            "demo.echo" => self.echo(surface, params),
            "demo.write" => self.write(surface, params, consent),
            "demo.ping" | "demo.locked" if !no_params => Fake::failure("bad_params", params.as_object().and_then(|o| o.keys().next()).map(String::as_str)),
            "demo.ping" => Ok(json!({"pong": true})),
            "demo.locked" => self.locked(surface),
            _ if self.is(Fault::UnknownCommandOnMcp) && surface == Surface::Mcp => Fake::failure("rpc_error", None),
            _ => Fake::failure("unknown_command", None),
        }
    }

    /// `demo.locked`, its params checked: disabled, so `unavailable` with what its state says.
    fn locked(&self, surface: Surface) -> Outcome {
        if self.is(Fault::LockedRunsOnCli) && surface == Surface::Cli {
            return Ok(json!({"pong": true}));
        }
        let fix = if self.is(Fault::LockedOtherReasonOnSocket) && surface == Surface::Socket { "try again later" } else { UNLOCK };
        Err(Failure::new("unavailable", fix, None))
    }

    /// The enabled query on `surface`: every command enabled but `demo.locked`.
    fn states(&self, surface: Surface) -> Vec<Enabled> {
        let code = if self.is(Fault::DisabledNotUnavailable) { "internal" } else { "unavailable" };
        let locked_enabled = self.is(Fault::StatesDifferOnServe) && surface == Surface::Serve;
        let state = |id: &str, why: Option<Failure>| Enabled { id: id.to_owned(), why };
        let mut states = vec![state("demo.echo", None), state("demo.locked", (!locked_enabled).then(|| Failure::new(code, UNLOCK, None)))];
        if !(self.is(Fault::NoStateOnCli) && surface == Surface::Cli) {
            states.push(state("demo.ping", None));
        }
        states.push(state("demo.write", None));
        states
    }
}

fn echo_params() -> Value {
    json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"], "additionalProperties": false})
}

impl App for Fake {
    fn commands(&self) -> Result<Vec<CommandInfo>, String> {
        let no_params = json!({"type": "object", "additionalProperties": false});
        let result = json!({"type": "object"});
        let echo_params = if self.is(Fault::NoParamsSchema) { json!({}) } else { echo_params() };
        let echo_id = if self.is(Fault::BadId) { "Demo-Echo" } else { "demo.echo" };
        Ok(vec![
            CommandInfo { id: echo_id.to_owned(), params: echo_params, result: result.clone(), needs_consent: false },
            CommandInfo { id: "demo.ping".to_owned(), params: no_params.clone(), result: result.clone(), needs_consent: false },
            CommandInfo { id: "demo.write".to_owned(), params: self::echo_params(), result: result.clone(), needs_consent: true },
            CommandInfo { id: "demo.locked".to_owned(), params: no_params, result, needs_consent: false },
        ])
    }

    fn mcp_tools(&self) -> Result<Vec<Value>, String> {
        let mut tools = Vec::new();
        if !self.is(Fault::McpNoTool) {
            let schema = if self.is(Fault::McpWrongInputSchema) { json!({"type": "object"}) } else { echo_params() };
            tools.push(json!({"name": if self.is(Fault::BadId) { "Demo-Echo" } else { "demo-echo" }, "inputSchema": schema}));
        }
        tools.push(json!({"name": "demo-ping", "inputSchema": {"type": "object", "additionalProperties": false}}));
        tools.push(json!({"name": "demo-write", "inputSchema": echo_params()}));
        tools.push(json!({"name": "demo-locked", "inputSchema": {"type": "object", "additionalProperties": false}}));
        if self.is(Fault::McpExtraTool) {
            tools.push(json!({"name": "demo-secret", "inputSchema": {"type": "object"}}));
        }
        Ok(tools)
    }

    fn call(&self, call: Call<'_>) -> Outcome {
        let id = if self.is(Fault::BadId) && call.id == "Demo-Echo" { "demo.echo" } else { call.id };
        let surface = call.surface;
        let outcome = self.dispatch(Call { id, ..call });
        match outcome {
            Err(f) if self.is(Fault::EmptyRemediation) && surface == Surface::Socket && f.code == "unknown_command" => Err(Failure::new(f.code, "  ", f.key)),
            other => other,
        }
    }

    fn surfaces(&self) -> Vec<Surface> {
        self.surfaces.clone()
    }

    fn accepts(&self, surface: Surface, params: &Value) -> bool {
        surface != Surface::Cli || params.to_string().len() < 8_000
    }

    fn examples(&self) -> Vec<Example> {
        let params = if self.is(Fault::BadExample) { json!({"text": 5}) } else { json!({"text": "hi"}) };
        vec![Example { id: "demo.echo".to_owned(), params }]
    }

    fn names_key_for_wrong_type(&self) -> bool {
        !self.opts_out_of_type_key
    }

    fn enabled(&self, surface: Surface) -> Option<Result<Vec<Enabled>, String>> {
        (surface != Surface::Mcp && self.surfaces.contains(&surface)).then(|| Ok(self.states(surface)))
    }
}

#[test]
fn an_app_that_conforms_passes_and_every_command_is_run_on_every_surface() {
    let report = run(&Fake::good()).unwrap_or_else(|v| panic!("{v}"));
    assert_eq!(report.commands, 4);
    // Per command: `{}` and each adversarial param on every surface (the CLI declines the two that
    // are over 8 kB); then the valid params made from its schema (`text` has a couple of dozen) with
    // consent, and `demo.write`'s without too, and the example on all five.
    let hostile = (1 + adversarial_params().len()) * 5;
    assert!(report.calls > 3 * (hostile - 8) + 2 * 20 * 4, "{report:?}");
}

/// The fake's `demo.write` needs consent: with it, it runs on every surface but MCP, which never
/// consents by default; without it, it is refused everywhere.
#[test]
fn consent_is_part_of_a_call_and_mcp_never_consents_by_default() {
    let fake = Fake::good();
    let call = |surface, consent| fake.call(Call { surface, id: "demo.write", params: &json!({"text": "a"}), consent });
    assert_eq!(call(Surface::Cli, true), Ok(json!({"written": "a"})));
    assert_eq!(call(Surface::Mcp, true).unwrap_err().code, "consent_required");
    assert_eq!(call(Surface::InProcess, false).unwrap_err().code, "consent_required");
    // An app whose MCP server has a client's grant says so, and the run then holds MCP to the others.
    struct Granted(Fake);
    impl App for Granted {
        fn commands(&self) -> Result<Vec<CommandInfo>, String> {
            self.0.commands()
        }
        fn mcp_tools(&self) -> Result<Vec<Value>, String> {
            self.0.mcp_tools()
        }
        fn call(&self, call: Call<'_>) -> Outcome {
            self.0.call(call)
        }
        fn consents(&self, _: Surface) -> bool {
            true
        }
    }
    let granted = Granted(Fake::with(Fault::McpConsents));
    assert!(run(&granted).is_ok(), "{}", run(&granted).unwrap_err());
    let found = run(&Granted(Fake::good())).unwrap_err().to_string();
    assert!(found.contains("[same-result] demo.write on mcp"), "{found}");
}

/// The contract says `key` names the wrong param for every `bad_params`: the run requires it for a
/// wrong-typed value unless the app opts out.
#[test]
fn an_app_that_does_not_name_the_key_of_a_wrong_typed_value_fails_by_default() {
    let fake = Fake { names_type_key: false, ..Fake::good() };
    let found = run(&fake).unwrap_err().to_string();
    assert!(found.contains("[bad-params] demo.echo") && found.contains("a wrong-typed `text`"), "{found}");
}

/// The opt-out is for an app whose parser predates the contract's wording: the key is then checked
/// only when it is there, and a wrong one still fails.
#[test]
fn an_app_that_opts_out_of_the_wrong_type_key_is_checked_only_when_the_key_is_there() {
    let opted_out = Fake { names_type_key: false, opts_out_of_type_key: true, ..Fake::good() };
    assert!(run(&opted_out).is_ok(), "{}", run(&opted_out).unwrap_err());
    assert!(run(&Fake { opts_out_of_type_key: true, ..Fake::good() }).is_ok(), "naming it is always fine");
}

#[test]
fn each_broken_rule_fails_with_a_message_naming_the_rule_and_the_command() {
    let broken = [
        (Fault::NoParamsSchema, "schemas", "demo.echo"),
        (Fault::BadId, "schemas", "Demo-Echo"),
        (Fault::McpNoTool, "mcp-descriptors", "demo.echo"),
        (Fault::McpWrongInputSchema, "mcp-descriptors", "demo.echo"),
        (Fault::McpExtraTool, "mcp-descriptors", "demo-secret"),
        (Fault::UnknownCommandOnMcp, "unknown-command", "conformance.no_such_command"),
        (Fault::UnknownKeyUnnamed, "bad-params", "demo.echo"),
        (Fault::MissingKeyWrong, "bad-params", "demo.echo"),
        (Fault::WrongTypeCoerced, "bad-params", "demo.echo"),
        (Fault::WrongTypeUnnamed, "bad-params", "demo.echo"),
        (Fault::NoCapOnSocket, "over-cap", "demo.echo"),
        (Fault::PanicOnNullOnServe, "no-panic", "demo.echo"),
        (Fault::PanicInCli, "no-panic", "demo.echo"),
        (Fault::CliDiffers, "same-result", "demo.echo"),
        (Fault::CliErrorsDiffer, "same-result", "demo.echo"),
        (Fault::EmptyRemediation, "error-shape", "conformance.no_such_command"),
        (Fault::BadExample, "examples", "demo.echo"),
        (Fault::RunsWithoutConsentOnCli, "consent", "demo.write on cli: without consent"),
        (Fault::McpConsents, "consent", "demo.write on mcp: with consent, on a surface that can't carry it"),
        (Fault::StatesDifferOnServe, "enabled", "demo.locked on serve: the surfaces disagree"),
        (Fault::LockedRunsOnCli, "enabled", "demo.locked on cli: a disabled command's call isn't refused `unavailable`"),
        (Fault::LockedOtherReasonOnSocket, "enabled", "demo.locked on socket: a disabled command's call gives another reason"),
        (Fault::NoStateOnCli, "enabled", "demo.ping on cli: no state"),
        (Fault::DisabledNotUnavailable, "enabled", "demo.locked on in-process: a disabled state that isn't `unavailable`"),
    ];
    for (fault, rule, command) in broken {
        let Err(found) = run(&Fake::with(fault)) else { panic!("{fault:?} passed") };
        let text = found.to_string();
        assert!(text.contains(&format!("[{rule}] {command}")), "{fault:?} should break [{rule}] {command}:\n{text}");
    }
}

/// A defect that every call meets is one line: a failure message is read, not scrolled.
#[test]
fn a_defect_met_by_many_calls_is_reported_once() {
    let Err(found) = run(&Fake::with(Fault::CliDiffers)) else { panic!("passed") };
    let lines = found.0.iter().filter(|v| v.rule == Rule::SameResult && v.command == "demo.echo" && v.surface == Some(Surface::Cli)).count();
    assert_eq!(lines, 1, "{found}");
}

#[test]
fn a_message_names_the_surface_and_what_was_seen() {
    let Err(found) = run(&Fake::with(Fault::UnknownKeyUnnamed)) else { panic!("passed") };
    let text = found.to_string();
    assert!(text.contains("[bad-params] demo.echo on in-process: an unknown key: `key` is absent but should be `conformance_unknown_key`"), "{text}");
}

#[test]
fn a_surface_the_app_lacks_is_not_run() {
    let fake = Fake { surfaces: vec![Surface::InProcess, Surface::Serve], fault: Some(Fault::McpNoTool), ..Fake::good() };
    assert!(run(&fake).is_ok(), "no MCP surface, so no MCP descriptors to check");
}

#[test]
fn an_app_with_no_commands_or_that_cannot_list_them_fails_the_schemas_rule() {
    struct Empty(Option<String>);
    impl App for Empty {
        fn commands(&self) -> Result<Vec<CommandInfo>, String> {
            self.0.clone().map_or(Ok(Vec::new()), Err)
        }
        fn mcp_tools(&self) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        fn call(&self, _: Call<'_>) -> Outcome {
            Fake::failure("unknown_command", None)
        }
    }
    let none = run(&Empty(None)).unwrap_err().to_string();
    assert!(none.contains("[schemas] -: the app has no commands"), "{none}");
    let broken = run(&Empty(Some("registry is locked".into()))).unwrap_err().to_string();
    assert!(broken.contains("[schemas] -: the app could not list its commands: registry is locked"), "{broken}");
}

#[test]
fn the_schema_cases_follow_the_schema() {
    let cases = cases::bad_params(&echo_params(), true);
    let whats: Vec<&str> = cases.iter().map(|c| c.what.as_str()).collect();
    assert!(whats.iter().any(|w| w.starts_with("params that are not an object")));
    assert!(whats.contains(&"an unknown key") && whats.contains(&"a missing required key"));
    assert!(whats.iter().any(|w| w.starts_with("a wrong-typed `text`")));
    // A free-form map takes any key, so no unknown key is refused; a command with no properties has no wrong type.
    let open = cases::bad_params(&json!({"type": "object", "additionalProperties": true}), false);
    assert!(open.iter().all(|c| c.what != "an unknown key" && !c.what.starts_with("a wrong-typed")));
    let by_type =
        |t: Value| cases::bad_params(&json!({"properties": {"n": t}}), true).into_iter().find(|c| c.what.starts_with("a wrong-typed")).map(|c| c.params);
    assert_eq!(by_type(json!({"type": "string"})), Some(json!({"n": true})));
    assert_eq!(by_type(json!({"type": "integer"})), Some(json!({"n": "conformance"})));
    assert_eq!(by_type(json!({"type": ["string", "null"]})), Some(json!({"n": true})));
    assert_eq!(by_type(json!({"$ref": "#/$defs/x"})), None, "a property with no stated type isn't guessed at");
    assert_eq!(by_type(json!({"enum": ["a", "b"]})), Some(json!({"n": true})), "an enum's values say its type");
    assert_eq!(by_type(json!({"anyOf": [{"type": "integer"}, {"type": "null"}]})), Some(json!({"n": "conformance"})));
}

/// A param that is a struct (a `$ref`) is given a wrong type, and so is a value inside it: `key` is
/// the top-level param for both, with the other required params filled.
#[test]
fn a_wrong_type_inside_a_nested_param_is_checked_by_the_top_level_key() {
    let schema = json!({
        "type": "object",
        "properties": {"opts": {"$ref": "#/$defs/Opts"}, "name": {"type": "string"}},
        "required": ["opts", "name"],
        "$defs": {"Opts": {"type": "object", "properties": {"depth": {"type": "integer", "format": "uint8"}}, "required": ["depth"]}}
    });
    let cases = cases::bad_params(&schema, true);
    let case = |what: &str| cases.iter().find(|c| c.what.starts_with(what)).unwrap_or_else(|| panic!("no `{what}` in {cases:?}"));
    let inside = case("a wrong-typed `depth` inside `opts`");
    assert_eq!(inside.params, json!({"opts": {"depth": "conformance"}, "name": "conformance"}));
    assert_eq!(inside.key, cases::Key::Is("opts".to_owned()));
    assert_eq!(case("a wrong-typed `opts`").params, json!({"opts": "conformance", "name": "conformance"}));
    assert_eq!(case("an unknown key").params, json!({"opts": {"depth": 0}, "name": "conformance", "conformance_unknown_key": 1}));
}

/// The reviewer's app (#31): its params check is faithful, so every rule but the ones that reach
/// `run` holds; its `run` is broken three ways, each of which used to pass the run.
mod broken {
    use super::*;

    fn fail(code: &str, key: Option<&str>) -> Outcome {
        Err(Failure::new(code, "fix it", key.map(str::to_owned)))
    }

    /// The value of `field` in `params`, checked like the skeleton's dispatch: not an object,
    /// unknown, missing and wrong-typed are each `bad_params` with the key.
    fn check(params: &Value, field: &str, ty: fn(&Value) -> bool, required: bool) -> Result<Option<Value>, Outcome> {
        let empty = serde_json::Map::new();
        let obj = match params {
            Value::Null => &empty,
            Value::Object(o) => o,
            _ => return Err(fail("bad_params", None)),
        };
        if let Some(k) = obj.keys().find(|k| *k != field) {
            return Err(fail("bad_params", Some(k)));
        }
        match obj.get(field) {
            None if required => Err(fail("bad_params", Some(field))),
            None => Ok(None),
            Some(v) if !ty(v) => Err(fail("bad_params", Some(field))),
            Some(v) => Ok(Some(v.clone())),
        }
    }

    /// `demo.div` (a required integer `n`): its run always panics. `demo.open` (an optional string
    /// `file`): its run panics when `file` is given. `demo.echo` (a required string `body`): the CLI
    /// answers differently. `demo.nest` (a required object `opts` with an integer `depth`): a wrong
    /// `depth` is named `depth`, not `opts`. No examples.
    struct Broken;

    impl App for Broken {
        fn commands(&self) -> Result<Vec<CommandInfo>, String> {
            let r = json!({"type": "object", "properties": {"ok": {"type": "boolean"}}});
            let command = |id: &str, params: Value| CommandInfo { id: id.into(), params, result: r.clone(), needs_consent: false };
            Ok(vec![
                command("demo.div", json!({"type":"object","properties":{"n":{"type":"integer"}},"required":["n"],"additionalProperties":false})),
                command("demo.echo", json!({"type":"object","properties":{"body":{"type":"string"}},"required":["body"],"additionalProperties":false})),
                command("demo.open", json!({"type":"object","properties":{"file":{"type":["string","null"]}},"additionalProperties":false})),
                command(
                    "demo.nest",
                    json!({"type":"object","properties":{"opts":{"$ref":"#/$defs/Opts"}},"required":["opts"],"additionalProperties":false,
                       "$defs":{"Opts":{"type":"object","properties":{"depth":{"type":"integer"}},"required":["depth"]}}}),
                ),
            ])
        }

        fn mcp_tools(&self) -> Result<Vec<Value>, String> {
            Ok(self.commands()?.into_iter().map(|c| json!({"name": c.id.replace('.', "-"), "inputSchema": c.params})).collect())
        }

        fn call(&self, call: Call<'_>) -> Outcome {
            let Call { surface, id, params, .. } = call;
            if matches!(surface, Surface::Serve | Surface::Socket | Surface::Mcp) && params.to_string().len() > REQUEST_CAP {
                return fail("too_large", None);
            }
            match id {
                "demo.div" => match check(params, "n", Value::is_i64, true) {
                    Err(o) => o,
                    Ok(_) => panic!("run of demo.div reached: division by zero"),
                },
                "demo.open" => match check(params, "file", |v| v.is_string() || v.is_null(), false) {
                    Err(o) => o,
                    Ok(Some(Value::String(_))) => panic!("run of demo.open reached with a file"),
                    Ok(_) => Ok(json!({"ok": true})),
                },
                "demo.echo" => match check(params, "body", Value::is_string, true) {
                    Err(o) => o,
                    Ok(_) => Ok(json!({"ok": surface != Surface::Cli})),
                },
                "demo.nest" => match check(params, "opts", Value::is_object, true) {
                    Err(o) => o,
                    Ok(opts) => match opts.as_ref().and_then(|o| o.get("depth")).map(Value::is_i64) {
                        Some(true) => Ok(json!({"ok": true})),
                        _ => fail("bad_params", Some("depth")),
                    },
                },
                _ => fail("unknown_command", None),
            }
        }
    }

    #[test]
    fn the_run_reaches_run_and_catches_each_break() {
        let found = run(&Broken).unwrap_err().to_string();
        for want in [
            "[no-panic] demo.div",
            "[no-panic] demo.open",
            "[same-result] demo.echo on cli",
            "[examples] demo.div: no call reaches `run`",
            "[bad-params] demo.nest on in-process: a wrong-typed `depth` inside `opts`: `key` is `depth` but should be `opts`",
        ] {
            assert!(found.contains(want), "no `{want}` in:\n{found}");
        }
        assert!(!found.contains("[examples] demo.echo") && !found.contains("[examples] demo.open"), "they succeed somewhere: {found}");
    }
}

#[test]
fn adversarial_params_cover_every_json_shape() {
    let params = adversarial_params();
    for shape in [Value::is_null, Value::is_boolean, Value::is_number, Value::is_string, Value::is_array, Value::is_object] {
        assert!(params.iter().any(shape));
    }
}
