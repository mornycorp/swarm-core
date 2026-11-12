//! The CLI's arguments and what each does. [`run`] takes the streams as arguments, so a test drives
//! it in memory and `main` gives it the process's.

use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::sync::Arc;

use serde_json::Value;

use {{crate_prefix_ident}}automation::control::{self, ControlArgs};
use {{crate_prefix_ident}}automation::{Options, commands_json, enabled_json, mcp, serve};
use {{crate_prefix_ident}}engine::{AppInfo, CallOpts, Caller, CommandError, CommandSpec, Session, clip};

const USAGE: &str = "usage:
  run <command> [json] [--yes]   run a command with JSON params (an object, quoted for the shell), print its result
  commands [--json]              list the commands; --json adds their schemas
  enabled                        whether each command can run now, and why not (JSON)
  serve [--yes]                  JSON lines on stdin and stdout, one request a line
  serve --socket [--yes] [--token-file <path>] [--port <n>]
                                 the same frames on a loopback socket (127.0.0.1), until stdin closes
  mcp                            an MCP server on stdin and stdout
  help | version

A command id is <area>.<name> (`commands` lists them). A command with an effect needs --yes.
The socket needs a token: {{env}}_CONTROL_TOKEN, or --token-file <path> in a directory only you can
read. A missing file is made there; on Unix it is 0600, and an existing one must be yours and 0600.
On Windows, where the file gets its directory's permissions, a new token is written at every start
(an existing file is never trusted): read it after the line below. It prints one JSON line when it listens: {\"listening\": \"127.0.0.1:<port>\", \"token\": ...}.";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation<'a> {
    Run { id: &'a str, params: Option<&'a str>, yes: bool },
    Commands { json: bool },
    Enabled,
    Serve { yes: bool },
    Socket { control: ControlArgs },
    Mcp,
    Help,
    Version,
}

/// The invocation `args` (without the program name) ask for, or what is wrong with them.
fn parse(args: &[String]) -> Result<Invocation<'_>, String> {
    let Some((command, rest)) = args.split_first() else { return Ok(Invocation::Help) };
    if command == "serve" {
        return parse_serve(rest);
    }
    let flags: Vec<&str> = rest.iter().map(String::as_str).filter(|a| a.starts_with("--")).collect();
    let words: Vec<&str> = rest.iter().map(String::as_str).filter(|a| !a.starts_with("--")).collect();
    let allowed: &[&str] = match command.as_str() {
        "run" => &["--yes"],
        "commands" => &["--json"],
        _ => &[],
    };
    if let Some(bad) = flags.iter().find(|f| !allowed.contains(f)) {
        return Err(format!("`{command}` takes no option `{bad}`"));
    }
    let (yes, json) = (flags.contains(&"--yes"), flags.contains(&"--json"));
    match (command.as_str(), words.as_slice()) {
        ("run", [id]) => Ok(Invocation::Run { id, params: None, yes }),
        ("run", [id, params]) => Ok(Invocation::Run { id, params: Some(params), yes }),
        ("commands", []) => Ok(Invocation::Commands { json }),
        ("enabled", []) => Ok(Invocation::Enabled),
        ("mcp", []) => Ok(Invocation::Mcp),
        ("help" | "--help" | "-h", _) => Ok(Invocation::Help),
        ("version" | "--version", _) => Ok(Invocation::Version),
        ("run" | "commands" | "enabled" | "mcp", _) => Err(format!("`{command}` was given arguments it doesn't take")),
        (other, _) => Err(format!("no command `{}`", clip(other))),
    }
}

/// `serve`: JSON lines on stdio, or with `--socket` on a loopback socket (the flags are
/// [`control::parse`]'s).
fn parse_serve(rest: &[String]) -> Result<Invocation<'static>, String> {
    let words: Vec<&str> = rest.iter().map(String::as_str).collect();
    if words.contains(&"--socket") {
        let flags: Vec<&str> = words.into_iter().filter(|w| *w != "--socket").collect();
        return control::parse(&flags).map(|control| Invocation::Socket { control });
    }
    match words.as_slice() {
        [] => Ok(Invocation::Serve { yes: false }),
        ["--yes"] => Ok(Invocation::Serve { yes: true }),
        _ => Err("`serve` takes --yes; with --socket it takes --yes, --token-file <path> and --port <n>".to_owned()),
    }
}

/// Why an invocation failed.
#[derive(Debug)]
enum Failure {
    Command(CommandError),
    Io(io::Error),
}

impl From<CommandError> for Failure {
    fn from(e: CommandError) -> Failure {
        Failure::Command(e)
    }
}

impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Failure {
        Failure::Io(e)
    }
}

/// Runs the command line `args` as the operating system gives them: an argument that isn't Unicode is
/// a wrong use (exit 2, with the usage), never a panic. Otherwise [`run`].
pub fn run_os(args: impl IntoIterator<Item = OsString>, input: impl BufRead, out: &mut impl Write, err: &mut impl Write) -> u8 {
    let mut text = Vec::new();
    for (at, arg) in args.into_iter().enumerate() {
        match arg.into_string() {
            Ok(arg) => text.push(arg),
            Err(arg) => {
                let shown = clip(&arg.to_string_lossy());
                let _ = writeln!(err, "{}: argument {} is not Unicode text (`{shown}`)\n\n{USAGE}", env!("CARGO_PKG_NAME"), at + 1);
                return 2;
            }
        }
    }
    run(&text, input, out, err)
}

/// Runs the command line `args`; the exit code: 0 done, 1 failed, 2 used wrongly.
pub fn run(args: &[String], input: impl BufRead, out: &mut impl Write, err: &mut impl Write) -> u8 {
    let name = env!("CARGO_PKG_NAME");
    let invocation = match parse(args) {
        Ok(invocation) => invocation,
        Err(message) => {
            let _ = writeln!(err, "{name}: {message}\n\n{USAGE}");
            return 2;
        }
    };
    let app = AppInfo { name: name.to_owned(), version: env!("CARGO_PKG_VERSION").to_owned() };
    let outcome = match Session::new(app) {
        Ok(session) => execute(invocation, &Arc::new(session), input, out),
        Err(e) => Err(Failure::Command(CommandError::internal(e.to_string()))),
    };
    match outcome {
        Ok(()) => 0,
        Err(failure) => {
            let _ = report(name, &failure, err);
            1
        }
    }
}

fn execute(invocation: Invocation<'_>, session: &Arc<Session>, input: impl BufRead, out: &mut impl Write) -> Result<(), Failure> {
    match invocation {
        Invocation::Run { id, params, yes } => run_command(session, id, params, yes, out),
        Invocation::Commands { json: true } => Ok(writeln!(out, "{}", pretty(&commands_json(session)?)?)?),
        Invocation::Commands { json: false } => Ok(list(session, out)?),
        Invocation::Enabled => Ok(writeln!(out, "{}", pretty(&enabled_json(session)?)?)?),
        Invocation::Serve { yes } => Ok(serve::serve(session, input, out, &Options { consent: yes })?),
        Invocation::Socket { control } => serve_socket(session, &control, input, out),
        Invocation::Mcp => Ok(mcp::serve(session, input, out, &Options::default())?),
        Invocation::Help => Ok(writeln!(out, "{USAGE}")?),
        Invocation::Version => Ok(writeln!(out, "{} {}", session.app().name, session.app().version)?),
    }
}

/// `serve --socket`: the control socket on the session, its start-up line printed, until `input`
/// closes (whoever started it holds stdin; closing it is the clean stop).
fn serve_socket(session: &Arc<Session>, args: &ControlArgs, mut input: impl BufRead, out: &mut impl Write) -> Result<(), Failure> {
    let (mut server, line) = control::start(Arc::clone(session), args)?;
    writeln!(out, "{line}")?;
    out.flush()?;
    io::copy(&mut input, &mut io::sink())?;
    server.stop();
    Ok(())
}

/// `run <id> [json]`: one call through the dispatch, the result printed as JSON.
fn run_command(session: &Session, id: &str, params: Option<&str>, yes: bool, out: &mut impl Write) -> Result<(), Failure> {
    let params = match params {
        Some(text) => serde_json::from_str::<Value>(text).map_err(|e| {
            let message = format!("the params are not JSON: {}", clip(&e.to_string()));
            CommandError::bad_params(message, "pass one JSON object, quoted for your shell: '{\"key\": 1}'")
        })?,
        None => Value::Null,
    };
    let opts = CallOpts::new(Caller::Cli);
    let opts = if yes { opts.consenting() } else { opts };
    let result = session.call(id, params, &opts)?;
    Ok(writeln!(out, "{}", pretty(&result)?)?)
}

fn pretty(value: &Value) -> Result<String, CommandError> {
    serde_json::to_string_pretty(value).map_err(|e| CommandError::internal(format!("not JSON: {e}")))
}

/// One line a command: its id, title and summary, and its menu path and shortcut when it has them.
fn list(session: &Session, out: &mut impl Write) -> io::Result<()> {
    let specs = session.commands();
    let width = specs.iter().map(|s| s.id.len()).max().unwrap_or(0);
    for spec in specs {
        writeln!(out, "{:<width$}  {}: {}{}", spec.id, spec.title, spec.summary, place(spec))?;
    }
    Ok(())
}

/// `  (menu Help/App info, shortcut Mod+I)`, of what `spec` has; nothing when it has neither.
fn place(spec: &CommandSpec) -> String {
    let parts: Vec<String> = [("menu", spec.menu), ("shortcut", spec.shortcut)].into_iter().filter_map(|(what, v)| v.map(|v| format!("{what} {v}"))).collect();
    if parts.is_empty() { String::new() } else { format!("  ({})", parts.join(", ")) }
}

/// A failure for a person, and for an agent: the code, the message, what to do, and the key.
fn report(name: &str, failure: &Failure, err: &mut impl Write) -> io::Result<()> {
    match failure {
        Failure::Io(e) => writeln!(err, "{name}: {e}"),
        Failure::Command(e) => {
            writeln!(err, "{name}: error [{}]: {}\n  fix: {}", e.code, e.message, e.remediation)?;
            e.key.as_ref().map_or(Ok(()), |key| writeln!(err, "  param: {key}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// The exit code, stdout and stderr of `{{cli}} <args>` with `stdin`.
    fn cli(args: &[&str], stdin: &str) -> (u8, String, String) {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, Cursor::new(stdin.as_bytes().to_vec()), &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn run_prints_the_result_as_json() {
        let (code, out, err) = cli(&["run", "app.info"], "");
        assert_eq!((code, err.as_str()), (0, ""));
        let info: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(info.get("name").and_then(Value::as_str), Some(env!("CARGO_PKG_NAME")));
        assert_eq!(cli(&["run", "app.info", "{}"], "").1, out);
    }

    #[test]
    fn a_failed_run_says_the_code_what_to_do_and_the_key() {
        let (code, out, err) = cli(&["run", "app.info", "{\"x\": 1}"], "");
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("error [bad_params]") && err.contains("fix: ") && err.contains("param: x"), "{err}");
        let (code, _, err) = cli(&["run", "app.nope"], "");
        assert!((code, err.contains("[unknown_command]")) == (1, true), "{err}");
        let (code, _, err) = cli(&["run", "app.info", "{nope"], "");
        assert!((code, err.contains("[bad_params]")) == (1, true), "{err}");
    }

    #[test]
    fn commands_lists_them_and_json_adds_the_schemas() {
        let (code, out, _) = cli(&["commands"], "");
        // Ids are padded to the longest, and the commands added beside the example sort around it.
        assert!((code, out.lines().any(|l| l.starts_with("app.info ") && l.contains(" App info: "))) == (0, true), "{out}");
        assert!(out.lines().any(|l| l.starts_with("app.info ") && l.ends_with("  (menu Help/App info)")), "{out}");
        let (_, json, _) = cli(&["commands", "--json"], "");
        let specs: Value = serde_json::from_str(&json).unwrap();
        let ids: Vec<&str> = specs.as_array().unwrap().iter().filter_map(|s| s.get("id").and_then(Value::as_str)).collect();
        assert!(ids.contains(&"app.info") && ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    }

    #[test]
    fn enabled_prints_every_commands_state() {
        let (code, out, err) = cli(&["enabled"], "");
        assert_eq!((code, err.as_str()), (0, ""));
        let states: Vec<Value> = serde_json::from_str(&out).unwrap();
        let info = serde_json::json!({"id": "app.info", "enabled": true});
        assert!(states.contains(&info), "{out}");
        assert_eq!(cli(&["enabled", "x"], "").0, 2);
    }

    #[test]
    fn serve_and_mcp_speak_on_the_streams() {
        let (code, out, _) = cli(&["serve"], "{\"id\": 1, \"method\": \"app.info\"}\n");
        assert!((code, out.starts_with("{\"id\":1,\"ok\":true,\"result\":{")) == (0, true), "{out}");
        let (code, out, _) = cli(&["mcp"], "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        assert_eq!((code, out.as_str()), (0, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n"));
    }

    #[test]
    fn serve_socket_prints_where_it_listens_and_stops_when_the_input_closes() {
        let dir = {{crate_prefix_ident}}testkit::temp_dir("cli-serve-socket").unwrap();
        let file = dir.join("control.token");
        let (code, out, err) = cli(&["serve", "--socket", "--token-file", &file.to_string_lossy()], "");
        assert_eq!((code, err.as_str()), (0, ""));
        let line: Value = serde_json::from_str(out.trim()).unwrap();
        let listening = line.get("listening").and_then(Value::as_str).unwrap_or_default();
        assert!(listening.starts_with("127.0.0.1:"), "{out}");
        assert!(file.is_file(), "the token file was made");
        let token = std::fs::read_to_string(&file).unwrap();
        assert!(!out.contains(token.trim()), "the start-up line says where the token is, never what it is: {out}");
    }

    #[test]
    fn serve_socket_without_a_token_says_what_to_set() {
        if std::env::var_os({{crate_prefix_ident}}automation::token::ENV).is_none() {
            let (code, out, err) = cli(&["serve", "--socket"], "");
            assert_eq!((code, out.as_str()), (1, ""));
            assert!(err.contains("--token-file") && err.contains("fix: "), "{err}");
        }
        for args in [&["serve", "--port", "1"][..], &["serve", "--socket", "--port", "x"], &["serve", "--socket", "--nope"], &["serve", "extra"]] {
            assert_eq!(cli(args, "").0, 2, "{args:?}");
        }
    }

    /// An argument the OS gives that isn't Unicode: on Unix a byte that isn't UTF-8, on Windows a lone
    /// surrogate.
    fn not_unicode() -> OsString {
        #[cfg(unix)]
        let arg = std::os::unix::ffi::OsStringExt::from_vec(vec![b'a', 0xff]);
        #[cfg(windows)]
        let arg = std::os::windows::ffi::OsStringExt::from_wide(&[u16::from(b'a'), 0xd800]);
        arg
    }

    #[test]
    fn an_argument_that_is_not_unicode_is_wrong_use_not_a_panic() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args = [OsString::from("run"), not_unicode()];
        let code = run_os(args, Cursor::new(Vec::new()), &mut out, &mut err);
        let err = String::from_utf8(err).unwrap();
        assert_eq!((code, out.is_empty()), (2, true));
        assert!(err.contains("argument 2 is not Unicode") && err.contains("usage:"), "{err}");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run_os([OsString::from("version")], Cursor::new(Vec::new()), &mut out, &mut err), 0, "Unicode arguments run");
    }

    #[test]
    fn wrong_use_is_exit_code_2_with_the_usage() {
        for args in [&["run"][..], &["run", "a.b", "{}", "extra"], &["serve", "--json"], &["frobnicate"], &["commands", "x"]] {
            let (code, out, err) = cli(args, "");
            assert_eq!((code, out.as_str()), (2, ""), "{args:?}");
            assert!(err.contains("usage:"), "{args:?}: {err}");
        }
        let (code, out, _) = cli(&[], "");
        assert!((code, out.contains("usage:")) == (0, true));
        assert!(cli(&["version"], "").1.starts_with(env!("CARGO_PKG_NAME")));
    }
}
