//! The session and the one dispatch (docs/app-architecture.md › One dispatch).

use serde_json::Value;

use crate::call::{CallContext, CallOpts};
use crate::commands;
use crate::error::CommandError;
use crate::registry::{Entry, Registry, RegistryError};
use crate::spec::{Command, CommandSpec, CommandState};
use crate::ui::{OnChange, UiCell, UiState};

/// What the app calls itself: `app.info` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

/// The app's state and its commands. Every surface makes its calls on one.
#[derive(Debug)]
pub struct Session {
    app: AppInfo,
    registry: Registry,
    /// The UI's state (docs/app-architecture.md › The UI is a client): `ui.inspect` and `ui.set`.
    ui: UiCell,
}

impl Session {
    /// A session with every command of the app ([`commands::registry`]).
    pub fn new(app: AppInfo) -> Result<Session, RegistryError> {
        Ok(Session::with_registry(app, commands::registry()?))
    }

    /// A session over `registry` (a test's, with commands of its own).
    pub fn with_registry(app: AppInfo, registry: Registry) -> Session {
        Session { app, registry, ui: UiCell::new() }
    }

    pub fn app(&self) -> &AppInfo {
        &self.app
    }

    /// Every command's spec, in id order.
    pub fn commands(&self) -> Vec<&CommandSpec> {
        self.registry.commands()
    }

    /// The UI's state now: what `ui.inspect` returns. Cheap (a copy under its lock): the UI reads it
    /// each frame.
    pub fn ui(&self) -> Result<UiState, CommandError> {
        self.ui.get()
    }

    /// Changes the UI's state (what `ui.set` does, once it has checked its params): the new state.
    pub(crate) fn update_ui(&self, change: impl FnOnce(&mut UiState)) -> Result<UiState, CommandError> {
        self.ui.update(change)
    }

    /// Calls `on_change` after each change of the UI's state, from any surface: the window repaints,
    /// so what an agent sets over the socket shows at once.
    pub fn watch_ui(&self, on_change: OnChange) -> Result<(), CommandError> {
        self.ui.watch(on_change)
    }

    /// Puts back a state the app saved: a selection of a command this version doesn't have, or a dock
    /// layout it can't build, is dropped, and the rest kept. The state now.
    pub fn restore_ui(&self, saved: UiState) -> Result<UiState, CommandError> {
        let selected = saved.selected.filter(|id| self.registry.lookup(id).is_some());
        let dock = saved.dock.filter(|dock| dock.check().is_ok());
        self.ui.update(|state| *state = UiState { selected, dock, ..saved })
    }

    /// Whether each command can run now, in id order: one call, for a menu or a toolbar
    /// (docs/app-architecture.md › Enabled). It is the check the dispatch makes ([`Session::call`]),
    /// so a command this says is disabled fails `unavailable` with the very `why` given here. Cheap
    /// (each check reads memory only): a UI asks it each frame.
    pub fn states(&self) -> Vec<CommandState> {
        self.registry
            .entries()
            .map(|entry| {
                let why = self.availability(&entry).err();
                CommandState { id: entry.spec.id, enabled: why.is_none(), why }
            })
            .collect()
    }

    /// The wire form: params and result are JSON. Only transports call it. It looks the command up
    /// (`unknown_command`), checks the params against its type (`bad_params`, with the key), that
    /// it is enabled (`unavailable`, with why not) and the consent its effects need
    /// (`consent_required`), then runs it; a panic in the run is an `internal` error, never the
    /// caller's crash ([`guarded`]). Absent params (JSON `null`) are `{}`; any other params
    /// that aren't an object are `bad_params`.
    pub fn call(&self, id: &str, params: Value, opts: &CallOpts) -> Result<Value, CommandError> {
        let entry = self.registry.lookup(id).ok_or_else(|| CommandError::unknown_command(id))?;
        let run = entry.call.prepare(object(params)?)?;
        let cx = self.admit(&entry, opts)?;
        guarded(id, || run(self, &cx))
    }

    /// The typed form: the UI and the app's own code call it, and read the typed result. The same
    /// checks as [`Session::call`], with the params already typed, and the same guard on its run.
    pub fn call_typed<C: Command>(&self, params: C::Params, opts: &CallOpts) -> Result<C::Output, CommandError> {
        let entry = self.registry.lookup(C::ID).ok_or_else(|| CommandError::unknown_command(C::ID))?;
        let command = entry.call.command().downcast_ref::<C>().ok_or_else(|| CommandError::unknown_command(C::ID))?;
        let cx = self.admit(&entry, opts)?;
        guarded(C::ID, || command.run(self, params, &cx))
    }

    /// **The one check of "can it run now"**: its requirements, then its own `enabled`. The
    /// dispatch and [`Session::states`] both make it, so a call and the query never disagree.
    fn availability(&self, entry: &Entry<'_>) -> Result<(), CommandError> {
        for requirement in entry.spec.requires {
            requirement.check(self)?;
        }
        Ok(entry.call.enabled(self)?)
    }

    /// The call's context, once the command is enabled, its effects have the consent they need and
    /// the call isn't already cancelled.
    fn admit(&self, entry: &Entry<'_>, opts: &CallOpts) -> Result<CallContext, CommandError> {
        self.availability(entry)?;
        let spec: &CommandSpec = entry.spec;
        if spec.effects.needs_consent() && !opts.consent {
            return Err(CommandError::consent_required(spec.id));
        }
        let cx = CallContext { caller: opts.caller, cancel: opts.cancel.clone() };
        cx.check_cancelled()?;
        Ok(cx)
    }
}

/// The params as an object: `null` is `{}`, anything else that isn't an object is `bad_params`.
fn object(params: Value) -> Result<Value, CommandError> {
    match params {
        Value::Null => Ok(Value::Object(serde_json::Map::new())),
        Value::Object(_) => Ok(params),
        _ => Err(CommandError::bad_params("the params must be a JSON object", "")),
    }
}

/// Run `run`, a command's own code: a panic in it is the app's fault, returned as an `internal`
/// error naming the command, so a bug in one command never takes down the surface that called it
/// (docs/app-architecture.md › Never crash). The lints forbid a panic in production code; a
/// dependency, an arithmetic overflow or a slice index can still raise one. The panic's own text
/// goes to stderr with the default hook, not into the error: it may hold a param's value. State
/// the command changed before it panicked stays as it left it, which is why a command saves
/// before it changes memory (› State on disk).
fn guarded<T>(id: &str, run: impl FnOnce() -> Result<T, CommandError>) -> Result<T, CommandError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .unwrap_or_else(|_| Err(CommandError::internal(format!("{id} failed with a bug (a panic): it is on the app's stderr"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Caller, Effects, ErrorCode, Why};
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// What to echo.
    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct EchoParams {
        /// The text that comes back.
        text: String,
    }

    #[derive(Debug, PartialEq, Serialize, schemars::JsonSchema)]
    struct EchoOutput {
        text: String,
    }

    struct Echo;
    impl Command for Echo {
        type Params = EchoParams;
        type Output = EchoOutput;
        const ID: &'static str = "test.echo";
        const TITLE: &'static str = "Echo";
        const SUMMARY: &'static str = "Gives the text back";
        fn run(&self, _: &Session, params: EchoParams, _: &CallContext) -> Result<EchoOutput, CommandError> {
            Ok(EchoOutput { text: params.text })
        }
    }

    /// A command with an effect: it needs the caller's consent.
    struct Writer;
    impl Command for Writer {
        type Params = EchoParams;
        type Output = EchoOutput;
        const ID: &'static str = "test.write";
        const TITLE: &'static str = "Write";
        const SUMMARY: &'static str = "Pretends to write a file";
        const EFFECTS: Effects = Effects { writes: true, runs_code: false, network: false, device: false };
        fn run(&self, _: &Session, params: EchoParams, _: &CallContext) -> Result<EchoOutput, CommandError> {
            Ok(EchoOutput { text: params.text })
        }
    }

    /// A slow command: it polls its cancel token.
    /// A command with a bug: its `run` panics.
    struct Panics;
    impl Command for Panics {
        type Params = EchoParams;
        type Output = EchoOutput;
        const ID: &'static str = "test.panics";
        const TITLE: &'static str = "Panics";
        const SUMMARY: &'static str = "Has a bug";
        fn run(&self, _: &Session, params: EchoParams, _: &CallContext) -> Result<EchoOutput, CommandError> {
            panic!("a bug in the command, with {}", params.text)
        }
    }

    struct Slow;
    impl Command for Slow {
        type Params = EchoParams;
        type Output = EchoOutput;
        const ID: &'static str = "test.slow";
        const TITLE: &'static str = "Slow";
        const SUMMARY: &'static str = "Works until cancelled";
        fn run(&self, _: &Session, _: EchoParams, cx: &CallContext) -> Result<EchoOutput, CommandError> {
            cx.cancel.cancel();
            cx.check_cancelled()?;
            Ok(EchoOutput { text: String::new() })
        }
    }

    /// A command that runs only while its flag is set, and needs consent: its own `enabled` check.
    struct Locked(Arc<AtomicBool>);
    impl Command for Locked {
        type Params = EchoParams;
        type Output = EchoOutput;
        const ID: &'static str = "test.locked";
        const TITLE: &'static str = "Locked";
        const SUMMARY: &'static str = "Runs once unlocked";
        const EFFECTS: Effects = Effects { writes: true, runs_code: false, network: false, device: false };
        fn enabled(&self, _: &Session) -> Result<(), Why> {
            if self.0.load(Ordering::SeqCst) { Ok(()) } else { Err(Why::new("the test has not unlocked it", "unlock it first")) }
        }
        fn run(&self, _: &Session, params: EchoParams, _: &CallContext) -> Result<EchoOutput, CommandError> {
            Ok(EchoOutput { text: params.text })
        }
    }

    fn session() -> Session {
        session_with(Arc::new(AtomicBool::new(false)))
    }

    /// The test commands, `test.locked` unlocked while `unlocked` is set.
    fn session_with(unlocked: Arc<AtomicBool>) -> Session {
        let mut registry = Registry::new();
        registry.register(Echo).unwrap();
        registry.register(Writer).unwrap();
        registry.register(Slow).unwrap();
        registry.register(Locked(unlocked)).unwrap();
        Session::with_registry(AppInfo { name: "test".into(), version: "1.2.3".into() }, registry)
    }

    /// The state query and the call make one check: a disabled command's call fails `unavailable`
    /// with the reason the query gave, before its consent is asked; enabled, it runs.
    #[test]
    fn a_disabled_command_says_why_and_its_call_fails_with_the_same_reason() {
        let unlocked = Arc::new(AtomicBool::new(false));
        let s = session_with(Arc::clone(&unlocked));
        let states = s.states();
        let ids: Vec<&str> = states.iter().map(|st| st.id).collect();
        assert_eq!(ids, ["test.echo", "test.locked", "test.slow", "test.write"], "every command, in id order");
        let locked = states.iter().find(|st| st.id == "test.locked").unwrap().clone();
        let why = locked.why.clone().unwrap();
        assert!(!locked.enabled && why.code == ErrorCode::Unavailable && why.remediation == "unlock it first", "{locked:?}");
        assert!(states.iter().filter(|st| st.id != "test.locked").all(|st| st.enabled && st.why.is_none()));
        let wire = serde_json::to_value(&locked).unwrap();
        assert_eq!(wire["why"]["code"], "unavailable");
        assert_eq!(serde_json::to_value(&states[0]).unwrap(), json!({"id": "test.echo", "enabled": true}));
        let yes = CallOpts::new(Caller::Test).consenting();
        assert_eq!(s.call("test.locked", json!({"text": "a"}), &yes).unwrap_err(), why, "the call fails with the state's reason");
        assert_eq!(call(&s, "test.locked", json!({"text": "a"})).unwrap_err(), why, "disabled is said before consent is asked");
        assert_eq!(s.call_typed::<Locked>(EchoParams { text: "a".into() }, &yes).unwrap_err(), why);
        unlocked.store(true, Ordering::SeqCst);
        assert!(s.states().iter().all(|st| st.enabled), "the state follows the session's");
        assert_eq!(s.call("test.locked", json!({"text": "a"}), &yes).unwrap(), json!({"text": "a"}));
        assert_eq!(call(&s, "test.locked", json!({"text": "a"})).unwrap_err().code, ErrorCode::ConsentRequired);
    }

    fn call(s: &Session, id: &str, params: Value) -> Result<Value, CommandError> {
        s.call(id, params, &CallOpts::new(Caller::Test))
    }

    #[test]
    fn a_call_runs_the_command_and_the_typed_call_agrees() {
        let s = session();
        assert_eq!(call(&s, "test.echo", json!({"text": "hi"})).unwrap(), json!({"text": "hi"}));
        let typed = s.call_typed::<Echo>(EchoParams { text: "hi".into() }, &CallOpts::new(Caller::Test)).unwrap();
        assert_eq!(typed, EchoOutput { text: "hi".into() });
    }

    #[test]
    fn the_registry_lists_commands_in_id_order_with_schemas_from_the_types() {
        let s = session();
        let ids: Vec<&str> = s.commands().iter().map(|c| c.id).collect();
        assert_eq!(ids, ["test.echo", "test.locked", "test.slow", "test.write"]);
        let echo = s.commands()[0].clone();
        let params = serde_json::to_value(&echo.params).unwrap();
        assert_eq!(params["properties"]["text"]["type"], "string");
        assert_eq!(params["required"], json!(["text"]));
        assert_eq!(params["properties"]["text"]["description"], "The text that comes back.");
        assert!(!serde_json::to_value(&echo.result).unwrap().is_null());
    }

    #[test]
    fn an_unknown_command_says_what_to_do() {
        let e = call(&session(), "test.nope", json!({})).unwrap_err();
        assert_eq!(e.code, ErrorCode::UnknownCommand);
        assert!(e.remediation.contains("commands"), "{e}");
        assert_eq!(call(&session(), "echo", json!({})).unwrap_err().code, ErrorCode::UnknownCommand);
    }

    #[test]
    fn bad_params_name_the_key() {
        let s = session();
        let missing = call(&s, "test.echo", json!({})).unwrap_err();
        assert_eq!((missing.code, missing.key.as_deref()), (ErrorCode::BadParams, Some("text")));
        let unknown = call(&s, "test.echo", json!({"text": "a", "extra": 1})).unwrap_err();
        assert_eq!((unknown.code, unknown.key.as_deref()), (ErrorCode::BadParams, Some("extra")));
        let wrong = call(&s, "test.echo", json!({"text": 5})).unwrap_err();
        assert_eq!((wrong.code, wrong.key.as_deref()), (ErrorCode::BadParams, Some("text")), "a wrong-typed value names its key: {wrong}");
        for wrong in [json!({"text": null}), json!({"text": ["a"]}), json!({"text": {"k": 1}}), json!({"text": true})] {
            let e = call(&s, "test.echo", wrong.clone()).unwrap_err();
            assert_eq!((e.code, e.key.as_deref()), (ErrorCode::BadParams, Some("text")), "{wrong}: {e}");
        }
        for not_an_object in [json!([]), json!("x"), json!(5), json!(true)] {
            assert_eq!(call(&s, "test.echo", not_an_object).unwrap_err().code, ErrorCode::BadParams);
        }
        assert_eq!(call(&s, "test.echo", Value::Null).unwrap_err().key.as_deref(), Some("text"), "null is an empty object: the key is missing");
    }

    #[test]
    fn an_effect_needs_the_callers_consent() {
        let s = session();
        let e = call(&s, "test.write", json!({"text": "a"})).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConsentRequired);
        let yes = CallOpts::new(Caller::Test).consenting();
        assert_eq!(s.call("test.write", json!({"text": "a"}), &yes).unwrap(), json!({"text": "a"}));
        assert!(s.commands().iter().any(|c| c.id == "test.write" && c.effects.writes));
    }

    #[test]
    fn a_cancelled_call_is_cancelled_before_and_during_the_run() {
        let s = session();
        assert_eq!(call(&s, "test.slow", json!({"text": ""})).unwrap_err().code, ErrorCode::Cancelled);
        let opts = CallOpts::new(Caller::Test);
        opts.cancel.cancel();
        assert_eq!(s.call("test.echo", json!({"text": "a"}), &opts).unwrap_err().code, ErrorCode::Cancelled);
    }

    /// A panic in a command's `run` is the app's fault, said as `internal` on both forms of the
    /// call; the session goes on serving the next call, and the panic's own text (which may hold a
    /// param) is not in the error.
    #[test]
    fn a_panic_in_a_command_is_an_internal_error_and_the_session_goes_on() {
        let mut registry = Registry::new();
        registry.register(Echo).unwrap();
        registry.register(Panics).unwrap();
        let s = Session::with_registry(AppInfo { name: "test".into(), version: "1.2.3".into() }, registry);
        let wire = call(&s, "test.panics", json!({"text": "secret-not-real"})).unwrap_err();
        assert_eq!(wire.code, ErrorCode::Internal);
        assert!(wire.message.contains("test.panics") && !wire.message.contains("secret-not-real") && !wire.remediation.is_empty(), "{wire}");
        let typed = s.call_typed::<Panics>(EchoParams { text: "x".into() }, &CallOpts::new(Caller::Test)).unwrap_err();
        assert_eq!(typed.code, ErrorCode::Internal);
        assert_eq!(call(&s, "test.echo", json!({"text": "after"})).unwrap()["text"], "after");
    }

    #[test]
    fn a_hostile_param_is_an_error_never_a_panic() {
        let s = Session::new(AppInfo { name: "test".into(), version: "1".into() }).unwrap();
        for params in swarm_core_testkit::conformance::adversarial_params() {
            for id in ["app.info", "test.echo", "", "..", "app.info\u{0}"] {
                match s.call(id, params.clone(), &CallOpts::new(Caller::Test)) {
                    Ok(_) => {}
                    Err(e) => assert!(!e.remediation.is_empty() && e.message.chars().count() < 1_000, "{id}: {e}"),
                }
            }
        }
    }
}
