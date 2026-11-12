//! The conformance run (docs/app-architecture.md › Every action is a command, One dispatch): an app
//! proves its transports are thin with one test, not by review. The app implements [`App`] (its
//! commands with their schemas, and one call through each [`Surface`]), and [`run`] drives every
//! command through every surface and compares. Adding a command adds it to the run with no edit;
//! adding a surface is one more arm in the app's [`App::call`].
//!
//! What it holds, each a named [`Rule`] that a failure's message starts with:
//!
//! - **`schemas`**: every command has an id by the id rule, and a params and a result schema.
//! - **`mcp-descriptors`**: MCP's `tools/list` has a tool per command, named by the id rule, whose
//!   `inputSchema` is the command's params schema, and no tool without a command.
//! - **`unknown-command`**: an id no command has is `unknown_command` on every surface.
//! - **`bad-params`**: params that aren't an object, an unknown key, a missing required key and a
//!   wrong-typed value (of a param, and inside a param that is an object: `key` is then the
//!   top-level param) are each `bad_params` on every surface, the other required params filled
//!   with valid values, and `key` names the param at fault in each, as the contract says. An app whose parser can't yet name the key of a wrong-typed
//!   value (one made from an older skeleton, whose params check was `serde_json`'s) opts out with
//!   [`App::names_key_for_wrong_type`]; the run then checks a key that is there and doesn't
//!   require it.
//! - **`over-cap`**: a request over the contract's cap (1 MiB) is refused with `too_large` on every
//!   surface that frames its requests ([`App::capped_surfaces`]).
//! - **`no-panic`**: a pass of [adversarial params](adversarial_params) through every command and
//!   surface panics nowhere, and so does a pass of **valid** params generated from each command's
//!   params schema (each property in turn with its edge values: empty and huge strings, path
//!   tricks, unicode, the ends of an integer's range, large and deep arrays and maps), which gets
//!   past the params check and reaches `run`.
//! - **`error-shape`**: every failure has a code and a remediation.
//! - **`same-result`**: for the same call, every surface gives the same result, or the same failure
//!   code and key.
//! - **`consent`**: a command whose effects need consent ([`CommandInfo::needs_consent`]) called
//!   without it is `consent_required` on every surface, and on a surface that can't carry consent
//!   ([`App::consents`]: MCP, by default) even when the call asks for it. The valid params and the
//!   examples are called with consent, so such a command runs too.
//! - **`examples`**: the app's own known-good calls succeed (see [`App::examples`]); a command with
//!   required params that no generated call gets to succeed (or whose schema the run can't build a
//!   value of) needs one. A command the app says is disabled needs none.
//! - **`enabled`**: every surface that can ask every command's enabled state in one call
//!   ([`App::enabled`]) gives one state per command and the same states as the others, each
//!   disabled one with the code `unavailable` and a remediation; and a call of a disabled command
//!   with valid params and consent fails `unavailable` with that remediation on every surface.
//!
//! - **`cancelled`**: a call that is cancelled before it runs fails `cancelled`, on every surface
//!   the app can cancel a call through ([`App::cancelled`]: the session's call, by its token; none
//!   by default, and the rule then checks nothing). A cancel is a token the surfaces set from
//!   their own events (a dropped connection, the UI's button), which a request can't carry, so
//!   the app makes the cancelled call itself.
//!
//! Not checked: the reverse of `enabled` (an enabled command's `run` may still answer `unavailable`
//! for its own reasons), and a cancel that arrives while a command runs: whether a slow `run`
//! polls its token is that command's own test. Either code may answer a call of the run: it is
//! then held to `error-shape` and `same-result` like any other. Which state a fresh session is in is the app's: a command disabled
//! there is checked to be refused; one enabled there has its refusal tested by the command's own
//! tests.

use std::fmt;

use serde_json::Value;

mod cases;
mod check;
#[cfg(test)]
mod tests;
mod values;

pub use cases::adversarial_params;

/// The caps of docs/app-architecture.md › The JSON-lines frame: a request line of 1 MiB at most.
pub const REQUEST_CAP: usize = 1024 * 1024;

/// A way into the app: the same call, made through each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Surface {
    /// The session's own call (`Session::call`), what the UI and a test use.
    InProcess,
    /// The binary's `run <id> [json]`.
    Cli,
    /// JSON lines on stdio.
    Serve,
    /// JSON lines on the loopback socket, after `auth`.
    Socket,
    /// MCP's `tools/call`.
    Mcp,
}

impl Surface {
    /// Every surface of the contract.
    pub const ALL: [Surface; 5] = [Surface::InProcess, Surface::Cli, Surface::Serve, Surface::Socket, Surface::Mcp];

    /// The surface as a failure message names it.
    pub const fn name(self) -> &'static str {
        match self {
            Surface::InProcess => "in-process",
            Surface::Cli => "cli",
            Surface::Serve => "serve",
            Surface::Socket => "socket",
            Surface::Mcp => "mcp",
        }
    }
}

impl fmt::Display for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A failed call, as every surface reports it (docs/app-architecture.md › The error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The stable code: `bad_params`, `unknown_command`, `too_large`, …
    pub code: String,
    /// What to do about it: never empty.
    pub remediation: String,
    /// The param that is wrong, by its JSON key, when the failure names one.
    pub key: Option<String>,
}

impl Failure {
    pub fn new(code: impl Into<String>, remediation: impl Into<String>, key: Option<String>) -> Failure {
        Failure { code: code.into(), remediation: remediation.into(), key }
    }
}

/// The result of one call through one surface.
pub type Outcome = Result<Value, Failure>;

/// A command as the registry describes it (what `commands` and `tools/list` are made from).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandInfo {
    /// `<area>.<name>`.
    pub id: String,
    /// The JSON Schema of its params.
    pub params: Value,
    /// The JSON Schema of its result.
    pub result: Value,
    /// Its effects need the caller's consent (any of the spec's `effects`: `writes`, `runs_code`,
    /// `network`, `device`): a call without it is `consent_required`.
    pub needs_consent: bool,
}

/// Whether a command can run now, as a surface's one call for every command reports it
/// (docs/app-architecture.md › Enabled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enabled {
    /// `<area>.<name>`.
    pub id: String,
    /// `None` when it is enabled; else why not, as the failure a call of it gets (`unavailable`).
    pub why: Option<Failure>,
}

/// One call of the run: command `id` with `params`, through `surface`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Call<'a> {
    pub surface: Surface,
    pub id: &'a str,
    pub params: &'a Value,
    /// The caller agrees to the command's effects: the session's `CallOpts::consenting()`, the CLI's
    /// `--yes`, `serve`'s and the socket's `Options { consent: true }`. A surface that can't carry
    /// consent ([`App::consents`] false: MCP, unless the app's server has a client's grant) makes
    /// the call as it always does, without.
    pub consent: bool,
}

/// A call known to succeed: the id and params of a command whose generated params don't succeed (a
/// file that must exist, a name that must be known), which the run can't invent. It is made with
/// consent.
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    pub id: String,
    pub params: Value,
}

/// What the app gives the conformance run.
pub trait App {
    /// Every command of the registry, with its schemas.
    fn commands(&self) -> Result<Vec<CommandInfo>, String>;

    /// MCP's `tools/list`: the `tools` array, as the server sent it.
    fn mcp_tools(&self) -> Result<Vec<Value>, String>;

    /// One call (its surface, command, params and consent): the result, or the failure with its
    /// code, remediation and key as that surface reports them. Each call is made on a fresh
    /// session, or on one the commands don't change, so surfaces are comparable; a command with an
    /// effect is called with consent and hostile params (paths among them), so its session's
    /// state is under a temp dir (docs/app-architecture.md › Tests touch nothing real). The app's
    /// transports must not be bypassed: the CLI call starts the binary, `serve` runs the real
    /// loop, the socket connects and authenticates, MCP sends `tools/call`.
    fn call(&self, call: Call<'_>) -> Outcome;

    /// The surfaces the app has. All five unless an app really lacks one.
    fn surfaces(&self) -> Vec<Surface> {
        Surface::ALL.to_vec()
    }

    /// Whether `surface` can carry a caller's consent. By default every surface but MCP: the
    /// contract's MCP server consents only to what its client has granted, never by default, so a
    /// command with an effect is `consent_required` there however the run calls it. An app whose
    /// MCP server has a client's grant returns `true` for MCP and passes the call's consent on.
    fn consents(&self, surface: Surface) -> bool {
        surface != Surface::Mcp
    }

    /// Whether `surface` can carry these `params` at all: the CLI takes them as one argument, which
    /// an operating system caps. A surface that declines is left out of that call's comparison.
    fn accepts(&self, _surface: Surface, _params: &Value) -> bool {
        true
    }

    /// The surfaces that read requests as capped lines, which must refuse one over [`REQUEST_CAP`].
    fn capped_surfaces(&self) -> Vec<Surface> {
        vec![Surface::Serve, Surface::Socket, Surface::Mcp]
    }

    /// Calls known to succeed, for the commands with required params that the run's generated params
    /// don't get to succeed: each is made with consent, compared across the surfaces and must
    /// succeed. Results must be the same every time (no clock, no counter).
    fn examples(&self) -> Vec<Example> {
        Vec::new()
    }

    /// Whether a wrong-typed value is `bad_params` with `key` naming the param, which the run
    /// requires (the default: the contract says `key` names the wrong param for every
    /// `bad_params`). An app whose parser can't say which value it refused, such as one made from a
    /// skeleton older than the path-tracking params check, returns `false` to opt out until it takes
    /// the fix: the run then only checks a `key` that is there.
    fn names_key_for_wrong_type(&self) -> bool {
        true
    }

    /// Every command's enabled state, as `surface` gives it in one call (the session's `states`,
    /// the frame's `enabled` method on `serve` and the socket, the CLI's `enabled`), made on a fresh
    /// session as [`App::call`]'s are; `Err` says why the surface couldn't. `None`: the surface has
    /// no such call (MCP has none), and the `enabled` rule leaves it out. The default is `None` on
    /// every surface, for an app made before the contract had the query: the rule then checks nothing.
    fn enabled(&self, _surface: Surface) -> Option<Result<Vec<Enabled>, String>> {
        None
    }

    /// The same call as [`App::call`], **cancelled before it runs**: its cancel token already set
    /// (the session's `CallOpts`, with `cancel.cancel()` called). `None`: this surface has no way
    /// to make a call cancelled from the start (a request can't carry a cancel; a dropped
    /// connection is the transport's own test), and the `cancelled` rule leaves it out. The default
    /// is `None` on every surface, for an app made before the rule: it then checks nothing.
    fn cancelled(&self, _call: Call<'_>) -> Option<Outcome> {
        None
    }
}

/// A rule of the run: what a [`Violation`] says it broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rule {
    Schemas,
    McpDescriptors,
    UnknownCommand,
    BadParams,
    OverCap,
    NoPanic,
    ErrorShape,
    SameResult,
    Consent,
    Examples,
    Enabled,
    Cancelled,
}

impl Rule {
    /// The rule as a message names it, in brackets.
    pub const fn name(self) -> &'static str {
        match self {
            Rule::Schemas => "schemas",
            Rule::McpDescriptors => "mcp-descriptors",
            Rule::UnknownCommand => "unknown-command",
            Rule::BadParams => "bad-params",
            Rule::OverCap => "over-cap",
            Rule::NoPanic => "no-panic",
            Rule::ErrorShape => "error-shape",
            Rule::SameResult => "same-result",
            Rule::Consent => "consent",
            Rule::Examples => "examples",
            Rule::Enabled => "enabled",
            Rule::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One thing the app does that the contract says it must not: the rule, the command (`-` for one
/// that is about the app as a whole), the surface when it is one's, and what was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: Rule,
    pub command: String,
    pub surface: Option<Surface>,
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.rule, self.command)?;
        if let Some(surface) = self.surface {
            write!(f, " on {surface}")?;
        }
        write!(f, ": {}", self.detail)
    }
}

/// Every [`Violation`] of a run. A defect met by many calls is reported once (per rule, command and
/// surface), by its first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violations(pub Vec<Violation>);

impl fmt::Display for Violations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "the conformance run found {} violation(s) of docs/app-architecture.md:", self.0.len())?;
        for violation in &self.0 {
            writeln!(f, "  {violation}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Violations {}

/// What a passing run covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// The commands the registry has.
    pub commands: usize,
    /// The calls made, across every surface.
    pub calls: usize,
}

/// Runs the conformance rules against `app`: every command through every surface. The commands and
/// calls covered, or every violation found.
pub fn run(app: &dyn App) -> Result<Report, Violations> {
    let mut found = check::Found::default();
    let commands = match app.commands() {
        Ok(commands) => commands,
        Err(e) => {
            found.add(Rule::Schemas, "-", None, format!("the app could not list its commands: {e}"));
            return Err(found.into_violations());
        }
    };
    check::schemas(&commands, &mut found);
    check::mcp_descriptors(app, &commands, &mut found);
    check::unknown_command(app, &mut found);
    for command in &commands {
        check::bad_params(app, command, &mut found);
    }
    check::over_cap(app, &commands, &mut found);
    let disabled = check::enabled(app, &commands, &mut found);
    let calls = check::every_surface_agrees(app, &commands, &disabled, &mut found);
    let calls = calls + check::cancelled(app, &commands, &disabled, &mut found);
    if found.is_empty() { Ok(Report { commands: commands.len(), calls }) } else { Err(found.into_violations()) }
}
