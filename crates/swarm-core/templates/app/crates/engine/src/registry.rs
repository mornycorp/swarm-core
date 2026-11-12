//! The registry: every command, in id order, each registered through the typed adapter
//! ([`Registry::register`]), which is the one place a [`CommandSpec`] is built.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde_json::Value;

use {{crate_prefix_ident}}model::ids::{CommandId, IdError};

use crate::call::CallContext;
use crate::error::{CommandError, clip};
use crate::menu;
use crate::session::Session;
use crate::spec::{Command, CommandSpec, Why};

/// Why a command can't be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The id breaks the id rule.
    BadId { id: &'static str, error: IdError },
    /// Another command has this id.
    Duplicate { id: &'static str },
    /// A title or summary that is empty or has more than one line.
    BadText { id: &'static str, field: &'static str },
    /// A menu path or a shortcut that breaks its form ([`crate::menu`]).
    BadPlace { id: &'static str, field: &'static str, problem: &'static str },
    /// Another command has this menu path or shortcut.
    Taken { id: &'static str, field: &'static str, other: &'static str },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::BadId { id, error } => write!(f, "`{id}` is not a command id: {error}"),
            RegistryError::Duplicate { id } => write!(f, "`{id}` is registered twice: ids are unique, and never renamed once released"),
            RegistryError::BadText { id, field } => write!(f, "`{id}`'s {field} must be one non-empty line"),
            RegistryError::BadPlace { id, field, problem } => write!(f, "`{id}`'s {field} breaks its form: {problem}"),
            RegistryError::Taken { id, field, other } => write!(f, "`{id}`'s {field} is `{other}`'s already: each is one command's"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// A command's params, parsed and ready to run.
pub(crate) type Prepared = Box<dyn FnOnce(&Session, &CallContext) -> Result<Value, CommandError>>;

/// A registered command with its types erased: what the dispatch holds.
pub(crate) trait Erased: Send + Sync {
    /// The params checked against the command's type (`bad_params`), as a call to run.
    fn prepare(&self, params: Value) -> Result<Prepared, CommandError>;
    /// The command itself, for the typed call (`Session::call_typed`).
    fn command(&self) -> &dyn Any;
    /// The command's own check of whether it can run now ([`Command::enabled`]).
    fn enabled(&self, session: &Session) -> Result<(), Why>;
}

struct Typed<C: Command> {
    command: Arc<C>,
}

impl<C: Command> Erased for Typed<C> {
    fn prepare(&self, params: Value) -> Result<Prepared, CommandError> {
        let params: C::Params = serde_path_to_error::deserialize(&params).map_err(|e| params_error(&e, &params))?;
        let command = Arc::clone(&self.command);
        Ok(Box::new(move |session, cx| {
            let output = command.run(session, params, cx)?;
            serde_json::to_value(output).map_err(|e| CommandError::internal(format!("{} produced a result that is not JSON: {e}", C::ID)))
        }))
    }

    fn command(&self) -> &dyn Any {
        &*self.command
    }

    fn enabled(&self, session: &Session) -> Result<(), Why> {
        self.command.enabled(session)
    }
}

/// `bad_params` for what serde refused in `params`, always naming the top-level param at fault
/// (docs/app-architecture.md › The error) and never repeating a hostile value whole. The key is the first
/// key of the path to the value that failed (`{"a": {"b": 1}}` with a wrong `b` names `a`). A missing,
/// unknown or duplicate field fails on the object itself: there serde's message is what names it.
fn params_error(error: &serde_path_to_error::Error<serde_json::Error>, params: &Value) -> CommandError {
    let message = error.to_string();
    let key = top_level_key(error.path()).or_else(|| key_in_message(&error.inner().to_string(), params)).map(|key| clip(&key));
    let e = CommandError::bad_params(clip(&message), "pass the params the command's schema describes (`commands` lists them)");
    match key {
        Some(key) => e.with_key(key),
        None => e,
    }
}

/// The first key of `path`, the param a failure inside it belongs to.
fn top_level_key(path: &serde_path_to_error::Path) -> Option<String> {
    match path.iter().next()? {
        serde_path_to_error::Segment::Map { key } => Some(key.clone()),
        _ => None,
    }
}

/// The field serde's own message names: a missing, unknown or duplicate one. A field name may hold a
/// backtick, so the name isn't cut at the first one: a missing or duplicate field's message ends with
/// the name and its closing backtick (from a `Value` there is no position after it), and an unknown
/// one is the key of `params` the message goes on from with "`, " (serde lists what it expected next).
fn key_in_message(message: &str, params: &Value) -> Option<String> {
    if let Some(rest) = message.strip_prefix("unknown field `") {
        let given = params.as_object().into_iter().flat_map(|o| o.keys());
        let named = given.filter(|k| rest.strip_prefix(k.as_str()).is_some_and(|after| after.starts_with("`, "))).max_by_key(|k| k.len());
        return named.cloned().or_else(|| rest.split_once("`, ").map(|(name, _)| name.to_owned()));
    }
    ["missing field `", "duplicate field `"].iter().find_map(|lead| message.strip_prefix(lead)).map(|rest| rest.strip_suffix('`').unwrap_or(rest).to_owned())
}

/// A command's spec and its typed adapter.
struct Registered {
    spec: CommandSpec,
    call: Box<dyn Erased>,
}

/// What the dispatch gets for an id: the spec and the adapter.
pub(crate) struct Entry<'a> {
    pub spec: &'a CommandSpec,
    pub call: &'a dyn Erased,
}

/// Every command, by id.
#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<&'static str, Registered>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Registers `command`; refuses an id the rule rejects or one already registered, a title or a
    /// summary that isn't one line, and a menu path or a shortcut that breaks its form or is
    /// another command's. The spec's schemas are generated from the command's types.
    pub fn register<C: Command>(&mut self, command: C) -> Result<(), RegistryError> {
        CommandId::parse(C::ID).map_err(|error| RegistryError::BadId { id: C::ID, error })?;
        if self.entries.contains_key(C::ID) {
            return Err(RegistryError::Duplicate { id: C::ID });
        }
        for (field, text) in [("title", C::TITLE), ("summary", C::SUMMARY)] {
            if text.trim().is_empty() || text.contains('\n') {
                return Err(RegistryError::BadText { id: C::ID, field });
            }
        }
        self.check_place::<C>()?;
        let spec = CommandSpec {
            id: C::ID,
            title: C::TITLE,
            summary: C::SUMMARY,
            params: schemars::SchemaGenerator::default().into_root_schema_for::<C::Params>(),
            result: schemars::SchemaGenerator::default().into_root_schema_for::<C::Output>(),
            effects: C::EFFECTS,
            requires: C::REQUIRES,
            menu: C::MENU,
            shortcut: C::SHORTCUT,
        };
        self.entries.insert(C::ID, Registered { spec, call: Box::new(Typed { command: Arc::new(command) }) });
        Ok(())
    }

    /// `C`'s menu path and shortcut: each of its form ([`crate::menu`]), and no other command's.
    fn check_place<C: Command>(&self) -> Result<(), RegistryError> {
        type Problem = fn(&str) -> Option<&'static str>;
        type Of = fn(&CommandSpec) -> Option<&'static str>;
        let places: [(&'static str, Option<&'static str>, Problem, Of); 2] =
            [("menu path", C::MENU, menu::menu_problem, |s| s.menu), ("shortcut", C::SHORTCUT, menu::shortcut_problem, |s| s.shortcut)];
        for (field, place, problem, of) in places {
            let Some(place) = place else { continue };
            if let Some(problem) = problem(place) {
                return Err(RegistryError::BadPlace { id: C::ID, field, problem });
            }
            if let Some(other) = self.entries.values().find(|e| of(&e.spec) == Some(place)) {
                return Err(RegistryError::Taken { id: C::ID, field, other: other.spec.id });
            }
        }
        Ok(())
    }

    /// Every command's spec, in id order: what `commands`, `tools/list` and the docs are made from.
    pub fn commands(&self) -> Vec<&CommandSpec> {
        self.entries.values().map(|e| &e.spec).collect()
    }

    /// Every command's spec and adapter, in id order.
    pub(crate) fn entries(&self) -> impl Iterator<Item = Entry<'_>> {
        self.entries.values().map(|e| Entry { spec: &e.spec, call: &*e.call })
    }

    pub(crate) fn lookup(&self, id: &str) -> Option<Entry<'_>> {
        self.entries.get(id).map(|e| Entry { spec: &e.spec, call: &*e.call })
    }
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.entries.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use crate::{AppInfo, CallOpts, Caller, ErrorCode};

    /// Params whose field names hold a backtick, and what serde writes after one.
    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Odd {
        #[serde(rename = "a`b")]
        _ab: String,
        #[serde(rename = "c`, expected `d")]
        _cd: Option<String>,
    }

    #[derive(Debug, Serialize, schemars::JsonSchema)]
    struct Nothing {}

    struct OddCommand;
    impl crate::Command for OddCommand {
        type Params = Odd;
        type Output = Nothing;
        const ID: &'static str = "test.odd";
        const TITLE: &'static str = "Odd";
        const SUMMARY: &'static str = "Takes field names with backticks";
        fn run(&self, _: &Session, _: Odd, _: &CallContext) -> Result<Nothing, CommandError> {
            Ok(Nothing {})
        }
    }

    fn key_of(params: Value) -> Option<String> {
        let mut registry = Registry::new();
        registry.register(OddCommand).unwrap();
        let session = Session::with_registry(AppInfo { name: "t".into(), version: "1".into() }, registry);
        let e = session.call("test.odd", params, &CallOpts::new(Caller::Test)).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadParams, "{e}");
        e.key
    }

    /// The key is the whole field name, a backtick in it or not: serde's message is not cut at the first.
    #[test]
    fn a_field_name_with_a_backtick_is_named_whole() {
        assert_eq!(key_of(json!({})).as_deref(), Some("a`b"), "missing");
        assert_eq!(key_of(json!({"a`b": 5})).as_deref(), Some("a`b"), "wrong type");
        assert_eq!(key_of(json!({"a`b": "x", "x`, expected `y": 1})).as_deref(), Some("x`, expected `y"), "unknown");
        assert_eq!(key_of(json!({"a`b": "x", "q`r": 1})).as_deref(), Some("q`r"), "unknown");
    }

    /// A command with a menu path and a shortcut: `P` and `S` pick them (and so its id).
    struct Placed<const P: usize, const S: usize>;
    const PATHS: [&str; 3] = ["Test/Odd", "Test", "Test/Other"];
    const KEYS: [&str; 2] = ["Mod+O", "Mod+o"];
    const IDS: [&str; 4] = ["test.placed", "test.placed_one", "test.placed_two", "test.placed_three"];
    impl<const P: usize, const S: usize> crate::Command for Placed<P, S> {
        type Params = Odd;
        type Output = Nothing;
        const ID: &'static str = IDS[P + S];
        const TITLE: &'static str = "Placed";
        const SUMMARY: &'static str = "Sits in a menu";
        const MENU: Option<&'static str> = Some(PATHS[P]);
        const SHORTCUT: Option<&'static str> = Some(KEYS[S]);
        fn run(&self, _: &Session, _: Odd, _: &CallContext) -> Result<Nothing, CommandError> {
            Ok(Nothing {})
        }
    }

    /// The menu path and the shortcut are in the spec, each of its form and each one command's.
    #[test]
    fn a_menu_path_and_a_shortcut_are_checked_and_unique() {
        let mut registry = Registry::new();
        registry.register(Placed::<0, 0>).unwrap();
        let spec = registry.commands()[0].clone();
        assert_eq!((spec.menu, spec.shortcut), (Some("Test/Odd"), Some("Mod+O")));
        let wire = serde_json::to_value(&spec).unwrap();
        assert_eq!((wire["menu"].clone(), wire["shortcut"].clone()), (json!("Test/Odd"), json!("Mod+O")));
        assert!(matches!(registry.register(Placed::<1, 0>), Err(RegistryError::BadPlace { field: "menu path", .. })));
        assert!(matches!(registry.register(Placed::<2, 1>), Err(RegistryError::BadPlace { field: "shortcut", .. })));
        let taken = registry.register(Placed::<2, 0>).unwrap_err();
        assert_eq!(taken, RegistryError::Taken { id: "test.placed_two", field: "shortcut", other: "test.placed" });
        assert!(taken.to_string().contains("test.placed"), "{taken}");
        let mut other = Registry::new();
        other.register(OddCommand).unwrap();
        let unplaced = serde_json::to_value(other.commands()[0]).unwrap();
        assert!(unplaced.get("menu").is_none() && unplaced.get("shortcut").is_none(), "none is left out: {unplaced}");
    }

    #[test]
    fn the_message_is_read_for_each_kind_of_field_error() {
        let params = json!({"k`, x": 1, "k": 2});
        assert_eq!(key_in_message("missing field `a`b`", &params).as_deref(), Some("a`b"));
        assert_eq!(key_in_message("duplicate field `z`", &params).as_deref(), Some("z"));
        assert_eq!(key_in_message("unknown field `k`, x`, expected `a`", &params).as_deref(), Some("k`, x"), "the longest given key that fits");
        assert_eq!(key_in_message("unknown field `k`, there are no fields", &params).as_deref(), Some("k"));
        assert_eq!(key_in_message("unknown field `gone`, expected `a`", &json!({})).as_deref(), Some("gone"), "no params to match: cut at serde's list");
        assert_eq!(key_in_message("invalid type: string", &params), None);
    }
}
