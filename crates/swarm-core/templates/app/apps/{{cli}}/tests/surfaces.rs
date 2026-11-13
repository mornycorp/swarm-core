//! Every command through every surface, compared (the contract's `conformance`): the session's own
//! call, the `{{cli}}` binary, `serve`, the loopback socket and MCP. The run itself is
//! swarm-core's (`swarm_core_testkit::conformance`, one implementation for every app: bump
//! the pin to get its improvements). This file is the app's part of it: [`Ours`] says how to make one
//! call through each surface. Adding a command adds it to the run with no edit; adding a surface is one
//! more arm of [`Ours::call`].
//!
//! A call carries the caller's consent as each surface does: the session's `CallOpts::consenting()`, the
//! binary's `--yes`, `serve`'s `Options { consent }` and the socket server's (consent is the server's,
//! not a frame's, so each call's server is started with it or without). MCP has no way to carry it: the
//! binary's `mcp` serves with `Options::default()`, as its client has granted nothing, so MCP never
//! consents ([`App::consents`]' default) and the run holds it to `consent_required`.
//!
//! Every command's enabled state is asked in one call on each surface that has one ([`App::enabled`]:
//! the session's `states`, the binary's `enabled`, the frame's `enabled` method; MCP has none), and the
//! run holds them to agree and a disabled command's call to fail `unavailable` with the same reason.
//!
//! **Each call is made on a fresh session** ([`App::call`]), the socket's too: a server is started for
//! the call and stopped after it, so a command that changes the session (`ui.set`) never changes what
//! the next call sees on one surface and not on the others.
//!
//! **MCP attached to a running app** (`{{cli}} mcp --attach <port>`) is MCP too, answering through the
//! app's socket: [`mcp_attached_to_a_running_app_gives_the_answers_mcp_gives`] sends every command the
//! same messages through it and through `{{cli}} mcp`, and holds the replies to be the same, line for
//! line, with one exception: a command whose effects need consent is refused `consent_required` before
//! the app sees it, so a call of it that the app would refuse anyway (bad params, disabled) may be
//! refused for consent instead. Neither runs it.

use std::io::{BufRead, BufReader, Cursor, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use swarm_core_testkit::conformance::{self, App, Call, CommandInfo, Enabled, Failure, Outcome, Surface};
use swarm_core_testkit::process;

use {{crate_prefix_ident}}automation::socket::{Config, Server};
use {{crate_prefix_ident}}automation::token::Token;
use {{crate_prefix_ident}}automation::{Options, mcp, serve};
use {{crate_prefix_ident}}engine::{AppInfo, CallOpts, Caller, CommandError, CommandSpec, Session};

const CLI: &str = env!("CARGO_BIN_EXE_{{cli}}");

/// How long a started program may take: a hung binary fails the test, it doesn't stall the run.
const DEADLINE: Duration = Duration::from_secs(60);

/// The CLI takes params as one argument, which an operating system caps (Windows at about 32 kB for
/// the whole command line): params over this go through the other surfaces only.
const CLI_PARAMS_MAX: usize = 8_000;

fn session() -> Session {
    Session::new(AppInfo { name: env!("CARGO_PKG_NAME").to_owned(), version: env!("CARGO_PKG_VERSION").to_owned() }).unwrap()
}

/// What a failed reply says, whichever surface it came from.
fn failure(code: &Value, remediation: &Value, key: &Value) -> Outcome {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    Err(Failure { code: text(code), remediation: text(remediation), key: key.as_str().map(str::to_owned) })
}

fn via_session(id: &str, params: &Value, consent: bool) -> Outcome {
    let opts = Options { consent }.call_opts(Caller::Test);
    session().call(id, params.clone(), &opts).map_err(|e| Failure { code: e.code.to_string(), remediation: e.remediation, key: e.key })
}

/// The session's call with its cancel token already set: cancelled before it runs.
fn via_session_cancelled(id: &str, params: &Value, consent: bool) -> Outcome {
    let opts = Options { consent }.call_opts(Caller::Test);
    opts.cancel.cancel();
    session().call(id, params.clone(), &opts).map_err(|e| Failure { code: e.code.to_string(), remediation: e.remediation, key: e.key })
}

fn via_serve(id: &str, params: &Value, consent: bool) -> Outcome {
    let request = json!({"id": 1, "method": id, "params": params});
    let mut out = Vec::new();
    serve::serve(&session(), Cursor::new(format!("{request}\n")), &mut out, &Options { consent }).unwrap();
    let reply: Value = serde_json::from_slice(&out).unwrap();
    if reply["ok"] == true { Ok(reply["result"].clone()) } else { failure(&reply["code"], &reply["remediation"], &reply["key"]) }
}

/// The socket's reply to `request` after `auth` with `token`.
fn socket_reply(addr: SocketAddr, token: &str, request: &Value) -> Value {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(DEADLINE)).unwrap();
    stream.set_write_timeout(Some(DEADLINE)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let auth = json!({"id": 0, "method": "auth", "params": {"token": token}});
    writeln!(writer, "{auth}").unwrap();
    // A request over the cap may be answered, and the connection closed, before it is all sent.
    let _ = writeln!(writer, "{request}");
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["ok"], true, "auth: {line}");
    line.clear();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// The token the test's socket servers take.
const TOKEN: &str = "test-token-not-real-0123456789";

/// A socket server on a fresh session, `consent` its connections' (`serve --socket [--yes]`).
fn socket_server(consent: bool) -> Server {
    let token = Token::parse(TOKEN).unwrap();
    Server::start(Arc::new(session()), Config::new(token, Options { consent })).unwrap()
}

fn via_socket(id: &str, params: &Value, consent: bool) -> Outcome {
    let server = socket_server(consent);
    let reply = socket_reply(server.addr(), TOKEN, &json!({"id": 1, "method": id, "params": params}));
    if reply["ok"] == true { Ok(reply["result"].clone()) } else { failure(&reply["code"], &reply["remediation"], &reply["key"]) }
}

/// The reply of an MCP server to the one request `message`.
fn mcp_reply(message: &Value) -> Value {
    let mut out = Vec::new();
    mcp::serve(&session(), Cursor::new(format!("{message}\n")), &mut out, &Options::default()).unwrap();
    serde_json::from_slice(&out).unwrap()
}

/// MCP's `tools/call`, served as the binary's `mcp` serves it: with no consent, which no client has granted.
fn via_mcp(id: &str, params: &Value) -> Outcome {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": id.replace('.', "-"), "arguments": params}});
    let reply = mcp_reply(&request);
    if !reply["error"].is_null() {
        // A tool that is no command, and a request over the cap, are protocol errors with the code and the
        // remediation in `data`; another protocol error has neither, and the run says so.
        let data = &reply["error"]["data"];
        let code = data["code"].as_str().unwrap_or("rpc_error");
        return failure(&json!(code), &data["remediation"], &Value::Null);
    }
    let text: Value = serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    if reply["result"]["isError"] == true { failure(&text["code"], &text["remediation"], &text["key"]) } else { Ok(text) }
}

/// The binary's exit code, stdout and stderr for `args`, with `stdin` fed to it.
fn cli(args: &[&str], stdin: &str) -> (Option<i32>, String, String) {
    let ran = process::run(Path::new(CLI), args, stdin, DEADLINE).unwrap_or_else(|e| panic!("{e}"));
    (ran.code, ran.stdout, ran.stderr)
}

/// A failure as the binary prints it: `<name>: error [<code>]: <message>`, then `  fix: <what to do>`
/// and, when there is one, `  param: <key>`.
fn cli_failure(stderr: &str) -> Outcome {
    let code = stderr.split_once("error [").and_then(|(_, rest)| rest.split_once(']')).map(|(code, _)| code);
    let after = |lead: &str| stderr.lines().find_map(|line| line.trim_start().strip_prefix(lead)).map(str::to_owned);
    match code {
        Some(code) => Err(Failure { code: code.to_owned(), remediation: after("fix: ").unwrap_or_default(), key: after("param: ") }),
        None => panic!("the binary's failure has no `error [code]` line: {stderr}"),
    }
}

fn via_cli(id: &str, params: &Value, consent: bool) -> Outcome {
    let params = params.to_string();
    let mut args = vec!["run", id, params.as_str()];
    if consent {
        args.push("--yes");
    }
    let (code, out, err) = cli(&args, "");
    match code {
        Some(0) => Ok(serde_json::from_str(&out).unwrap_or_else(|e| panic!("`run {id}` printed something that is not JSON ({e}): {out}"))),
        Some(1) => cli_failure(&err),
        other => panic!("`run {id}` ended with {other:?}, not 0 or 1: {err}"),
    }
}

/// The states the frame's `enabled` method (or the CLI's `enabled`) gave, as JSON.
fn states_from(value: &Value) -> Result<Vec<Enabled>, String> {
    let states = value.as_array().ok_or_else(|| format!("the states are not a list: {value}"))?;
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    let why_of = |why: &Value| Failure { code: text(&why["code"]), remediation: text(&why["remediation"]), key: why["key"].as_str().map(str::to_owned) };
    let state = |s: &Value| {
        let id = s["id"].as_str().ok_or_else(|| format!("a state without an id: {s}"))?.to_owned();
        let why = Some(&s["why"]).filter(|why| !why.is_null()).map(why_of);
        Ok(Enabled { id, why })
    };
    states.iter().map(state).collect()
}

/// The states in a frame's reply to `enabled`.
fn states_in_reply(reply: &Value) -> Result<Vec<Enabled>, String> {
    if reply["ok"] == true { states_from(&reply["result"]) } else { Err(format!("`enabled` failed: {reply}")) }
}

/// The session's own states, as the run compares them.
fn states_in_process() -> Vec<Enabled> {
    let failure = |e: CommandError| Failure { code: e.code.to_string(), remediation: e.remediation, key: e.key };
    session().states().into_iter().map(|s| Enabled { id: s.id.to_owned(), why: s.why.map(failure) }).collect()
}

/// Every command's state, through `surface`'s one call for them: the session's `states`, the binary's
/// `enabled`, the frame's `enabled` method on `serve` and the socket. MCP has none.
fn states_via(surface: Surface) -> Option<Result<Vec<Enabled>, String>> {
    let request = json!({"id": 1, "method": "enabled"});
    Some(match surface {
        Surface::InProcess => Ok(states_in_process()),
        Surface::Cli => match cli(&["enabled"], "") {
            (Some(0), out, _) => serde_json::from_str(&out).map_err(|e| e.to_string()).and_then(|v: Value| states_from(&v)),
            (code, _, err) => Err(format!("`enabled` ended with {code:?}: {err}")),
        },
        Surface::Serve => {
            let mut out = Vec::new();
            serve::serve(&session(), Cursor::new(format!("{request}\n")), &mut out, &Options::default()).unwrap();
            states_in_reply(&serde_json::from_slice(&out).unwrap())
        }
        Surface::Socket => states_in_reply(&socket_reply(socket_server(false).addr(), TOKEN, &request)),
        Surface::Mcp => return None,
    })
}

/// This app, for the conformance run: each call through each surface, on a fresh session.
struct Ours;

impl App for Ours {
    fn commands(&self) -> Result<Vec<CommandInfo>, String> {
        let schema = |s: &_| serde_json::to_value(s).map_err(|e| e.to_string());
        let info = |c: &CommandSpec| {
            Ok(CommandInfo { id: c.id.to_owned(), params: schema(&c.params)?, result: schema(&c.result)?, needs_consent: c.effects.needs_consent() })
        };
        session().commands().into_iter().map(info).collect()
    }

    fn mcp_tools(&self) -> Result<Vec<Value>, String> {
        let reply = mcp_reply(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
        reply["result"]["tools"].as_array().cloned().ok_or_else(|| format!("`tools/list` gave no tools: {reply}"))
    }

    fn call(&self, call: Call<'_>) -> Outcome {
        let Call { surface, id, params, consent } = call;
        match surface {
            Surface::InProcess => via_session(id, params, consent),
            Surface::Cli => via_cli(id, params, consent),
            Surface::Serve => via_serve(id, params, consent),
            Surface::Socket => via_socket(id, params, consent),
            Surface::Mcp => via_mcp(id, params),
        }
    }

    fn accepts(&self, surface: Surface, params: &Value) -> bool {
        surface != Surface::Cli || params.to_string().len() <= CLI_PARAMS_MAX
    }

    fn enabled(&self, surface: Surface) -> Option<Result<Vec<Enabled>, String>> {
        states_via(surface)
    }

    /// A call cancelled before it runs: the session's, by its token. The transports cancel from
    /// their own events (a dropped connection), which their own tests hold.
    fn cancelled(&self, call: Call<'_>) -> Option<Outcome> {
        (call.surface == Surface::InProcess).then(|| via_session_cancelled(call.id, call.params, call.consent))
    }

    // The run calls every command with params generated from its schema (with consent), so its `run` is
    // reached. A command with required params that none of those gets to succeed (a file that must exist,
    // a name that must be known) needs `fn examples(&self) -> Vec<conformance::Example>`: calls known to
    // succeed, which the run makes with consent and compares across the surfaces too (`app.info` needs
    // none, so there are none here).
}

#[test]
fn every_command_gives_the_same_answer_on_every_surface() {
    let report = conformance::run(&Ours).unwrap_or_else(|violations| panic!("{violations}"));
    assert!(report.commands >= 1 && report.calls >= report.commands * 20, "{report:?}");
}

#[test]
fn the_binary_runs_the_example_through_run_commands_serve_and_mcp() {
    let want = session().call("app.info", json!({}), &CallOpts::new(Caller::Test)).unwrap();

    let (code, out, _) = cli(&["run", "app.info"], "");
    assert_eq!((code, serde_json::from_str::<Value>(&out).unwrap()), (Some(0), want.clone()));

    let (code, out, _) = cli(&["commands"], "");
    assert!(code == Some(0) && out.lines().any(|l| l.starts_with("app.info ")), "{out}");

    let (code, out, _) = cli(&["serve"], "{\"id\": 7, \"method\": \"app.info\"}\n");
    let reply: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!((code, reply["id"].clone(), reply["result"].clone()), (Some(0), json!(7), want.clone()));

    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"app-info\",\"arguments\":{}}}\n";
    let (code, out, _) = cli(&["mcp"], call);
    let reply: Value = serde_json::from_str(out.trim()).unwrap();
    let text: Value = serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!((code, text), (Some(0), want));
}

#[test]
fn the_binary_fails_with_a_code_and_a_remediation_not_a_panic() {
    let (code, out, err) = cli(&["run", "app.info", "{\"nope\": 1}"], "");
    assert_eq!((code, out.as_str()), (Some(1), ""));
    assert!(err.contains("[bad_params]") && err.contains("fix: ") && err.contains("param: nope"), "{err}");
    let (code, _, err) = cli(&["serve"], "{\n\u{ff}\n");
    assert_eq!(code, Some(0), "a bad line is answered, not fatal: {err}");
}

/// `serve --socket` as a program: it prints where it listens, makes the token file (private), answers a
/// client that authenticates, and exits cleanly when its stdin closes.
#[test]
fn the_binary_serves_the_control_socket_until_its_stdin_closes() {
    let dir = {{crate_prefix_ident}}testkit::temp_dir("cli-socket-binary").unwrap();
    let file = dir.join("control.token");
    let mut child = Command::new(CLI)
        .args(["serve", "--socket", "--token-file"])
        .arg(&file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut first).unwrap();
    let started: Value = serde_json::from_str(&first).unwrap_or_else(|e| panic!("start-up line `{first}`: {e}"));
    let addr: SocketAddr = started["listening"].as_str().unwrap().parse().unwrap();
    assert!(addr.ip().is_loopback(), "{addr}");
    let token = std::fs::read_to_string(&file).unwrap();
    assert!(!first.contains(token.trim()), "the start-up line never shows the token");
    let want = session().call("app.info", json!({}), &CallOpts::new(Caller::Test)).unwrap();
    let reply = socket_reply(addr, token.trim(), &json!({"id": 3, "method": "app.info"}));
    assert_eq!((reply["id"].clone(), reply["result"].clone()), (json!(3), want));
    let refused = socket_reply_unauthenticated(addr);
    assert_eq!(refused["code"], "unauthenticated");
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the server did not stop when its stdin closed");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
}

/// A token file holding [`TOKEN`], as the app writes one (private on Unix, as a client reads one).
fn token_file(name: &str) -> PathBuf {
    let file = {{crate_prefix_ident}}testkit::temp_dir(name).unwrap().join("control.token");
    std::fs::write(&file, format!("{TOKEN}\n")).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    file
}

/// MCP messages for every command (no params, empty, an unknown key, not an object, and every
/// adversarial value), tools that are no command, and a request over the cap, each with an id: the
/// messages, and which of them call a command whose effects need consent with params the app refuses
/// even with consent.
fn mcp_script() -> (Vec<String>, Vec<usize>) {
    let mut messages = vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}).to_string(),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string(),
    ];
    let mut refused_anyway = Vec::new();
    let mut cases = vec![json!({}), Value::Null, json!({"conformance_unknown": 1}), json!(5)];
    cases.extend(conformance::adversarial_params());
    let call = |at: usize, name: &str, arguments: &Value| {
        json!({"jsonrpc": "2.0", "id": at, "method": "tools/call", "params": {"name": name, "arguments": arguments}}).to_string()
    };
    for spec in session().commands() {
        for params in &cases {
            let at = messages.len();
            if spec.effects.needs_consent() && session().call(spec.id, params.clone(), &CallOpts::new(Caller::Test).consenting()).is_err() {
                refused_anyway.push(at);
            }
            messages.push(call(at, &spec.id.replace('.', "-"), params));
        }
    }
    for name in ["app-nope", "app.info", ""] {
        messages.push(call(messages.len(), name, &json!({})));
    }
    messages.push(call(messages.len(), "app-info", &json!({"conformance_padding": "a".repeat(conformance::REQUEST_CAP)})));
    (messages, refused_anyway)
}

/// `{{cli}} mcp --attach` against a control socket on a session (started with consent, as
/// `serve --socket --yes` is), and `{{cli}} mcp` on a session of its own, given the same messages: the
/// same replies (results, failure codes, `unknown_command`, `bad_params`, `unavailable`, `too_large`),
/// and a command whose effects need consent `consent_required` on both, the socket's consent
/// notwithstanding. Both sessions start fresh and take the same calls in the same order.
#[test]
fn mcp_attached_to_a_running_app_gives_the_answers_mcp_gives() {
    if std::env::var_os({{crate_prefix_ident}}automation::token::ENV).is_some() {
        return; // the binary would take that token, not the test's
    }
    let (messages, refused_anyway) = mcp_script();
    let stdin = format!("{}\n", messages.join("\n"));
    let (code, direct, err) = cli(&["mcp"], &stdin);
    assert_eq!(code, Some(0), "{err}");
    let server = socket_server(true);
    let (port, file) = (server.addr().port().to_string(), token_file("surfaces-mcp-attach"));
    let (code, attached, err) = cli(&["mcp", "--attach", &port, "--token-file", &file.to_string_lossy()], &stdin);
    assert_eq!(code, Some(0), "{err}");
    let (direct, attached): (Vec<&str>, Vec<&str>) = (direct.lines().collect(), attached.lines().collect());
    assert_eq!((direct.len(), attached.len()), (messages.len(), messages.len()), "a reply a message");
    for (at, (want, got)) in direct.iter().zip(&attached).enumerate() {
        let message: String = messages[at].chars().take(200).collect();
        if refused_anyway.contains(&at) {
            let (want, got): (Value, Value) = (serde_json::from_str(want).unwrap(), serde_json::from_str(got).unwrap());
            assert_eq!((want["result"]["isError"].clone(), got["result"]["isError"].clone()), (json!(true), json!(true)), "{message}");
            assert!(got["result"]["content"][0]["text"].as_str().is_some_and(|t| t.contains("\"consent_required\"")), "{message}: {got}");
        } else {
            assert_eq!(got, want, "{message}");
        }
    }
}

/// What a connection that sends a wrong token is told.
fn socket_reply_unauthenticated(addr: SocketAddr) -> Value {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut writer = stream.try_clone().unwrap();
    writeln!(writer, "{}", json!({"id": 0, "method": "auth", "params": {"token": "test-token-not-real-wrong-one"}})).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}
