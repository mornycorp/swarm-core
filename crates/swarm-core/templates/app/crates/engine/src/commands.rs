//! The app's commands, registered in one place ([`registry`]). The example is `app.info`: it stays
//! until the first real command replaces it. A command is a type that implements
//! [`Command`], and a line in [`registry`]: no transport changes.
//!
//! `ui.inspect` and `ui.set` read and set the UI's state ([`UiState`]), so the socket, MCP and a test
//! see and drive what the window shows (docs/app-architecture.md › The UI is a client). They read and
//! change memory only, as the enabled query does: the UI calls them on its own thread. `ui.theme`
//! gives the theme as data ([`crate::theme`]): the colour tokens and the categorical ranges, which
//! the UI's swatch view draws.

use serde::{Deserialize, Serialize};

use {{crate_prefix_ident}}model::SCHEMA_VERSION;

use crate::error::clip;
use crate::theme::{self, ThemeData};
use crate::{CallContext, Command, CommandError, DockNode, Registry, RegistryError, Requirement, Session, UiState};

/// Every command of the app: the one place they are registered.
pub fn registry() -> Result<Registry, RegistryError> {
    let mut registry = Registry::new();
    registry.register(InfoCommand)?;
    registry.register(UiInspectCommand)?;
    registry.register(UiSetCommand)?;
    registry.register(UiThemeCommand)?;
    Ok(registry)
}

/// `app.info` takes no params.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InfoParams {}

/// What `app.info` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Info {
    /// The app's name.
    pub name: String,
    /// The app's version.
    pub version: String,
    /// The version of the shapes the app exposes and persists (`model::SCHEMA_VERSION`).
    pub schema_version: u32,
}

/// `app.info`: the app's name, version and schema version.
#[derive(Debug, Clone, Copy)]
pub struct InfoCommand;

impl Command for InfoCommand {
    type Params = InfoParams;
    type Output = Info;
    const ID: &'static str = "app.info";
    const TITLE: &'static str = "App info";
    const SUMMARY: &'static str = "The app's name, version and schema version";
    const REQUIRES: &'static [Requirement] = &[Requirement::Session];
    const MENU: Option<&'static str> = Some("Help/App info");

    fn run(&self, session: &Session, _params: InfoParams, _cx: &CallContext) -> Result<Info, CommandError> {
        let app = session.app();
        Ok(Info { name: app.name.clone(), version: app.version.clone(), schema_version: SCHEMA_VERSION })
    }
}

/// `ui.inspect` takes no params.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiInspectParams {}

/// `ui.inspect`: the UI's state, as the window draws it.
#[derive(Debug, Clone, Copy)]
pub struct UiInspectCommand;

impl Command for UiInspectCommand {
    type Params = UiInspectParams;
    type Output = UiState;
    const ID: &'static str = "ui.inspect";
    const TITLE: &'static str = "Inspect the UI";
    const SUMMARY: &'static str = "The UI's state: the command whose view is open, and whether the command list is shown";
    const REQUIRES: &'static [Requirement] = &[Requirement::Session];

    fn run(&self, session: &Session, _params: UiInspectParams, _cx: &CallContext) -> Result<UiState, CommandError> {
        session.ui()
    }
}

/// What `ui.set` changes: each param given is set, each left out stays as it is.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiSetParams {
    /// The command whose view to open, by id (`app.info`); `""` closes it.
    #[serde(default)]
    pub selected: Option<String>,
    /// Whether to show the command list.
    #[serde(default)]
    pub show_list: Option<bool>,
    /// Where the dock's panes are (`ui::DockNode`): tabs of panes by id, or a split into parts.
    #[serde(default)]
    pub dock: Option<DockNode>,
}

/// `ui.set`: changes the UI's state, as a click does; its result is the state now.
#[derive(Debug, Clone, Copy)]
pub struct UiSetCommand;

impl Command for UiSetCommand {
    type Params = UiSetParams;
    type Output = UiState;
    const ID: &'static str = "ui.set";
    const TITLE: &'static str = "Set the UI";
    const SUMMARY: &'static str = "Opens a command's view or closes it, shows or hides the command list, and lays out the dock's panes";
    const REQUIRES: &'static [Requirement] = &[Requirement::Session];

    fn run(&self, session: &Session, params: UiSetParams, _cx: &CallContext) -> Result<UiState, CommandError> {
        let selected = match params.selected.as_deref() {
            None => None,
            Some("") => Some(None),
            Some(id) => Some(Some(registered(session, id)?)),
        };
        if let Some(dock) = &params.dock {
            dock.check()?;
        }
        session.update_ui(|state| {
            if let Some(selected) = selected {
                state.selected = selected;
            }
            if let Some(show) = params.show_list {
                state.show_list = show;
            }
            if let Some(dock) = params.dock {
                state.dock = Some(dock);
            }
        })
    }
}

/// `ui.theme` takes no params.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiThemeParams {}

/// `ui.theme`: the theme as data, the colours the UI draws with (docs/app-architecture.md › The UI
/// is a client): each token's name, meaning and `#RRGGBB` in the dark and the light look, and each
/// categorical range's entries, each also made a background for text. It reads constants only.
#[derive(Debug, Clone, Copy)]
pub struct UiThemeCommand;

impl Command for UiThemeCommand {
    type Params = UiThemeParams;
    type Output = ThemeData;
    const ID: &'static str = "ui.theme";
    const TITLE: &'static str = "Theme";
    const SUMMARY: &'static str = "The theme's colours as data: each token's meaning and value and each categorical range's entries, dark and light";
    const REQUIRES: &'static [Requirement] = &[Requirement::Session];
    const MENU: Option<&'static str> = Some("View/Theme");

    fn run(&self, _session: &Session, _params: UiThemeParams, _cx: &CallContext) -> Result<ThemeData, CommandError> {
        Ok(theme::data())
    }
}

/// `id` as a command the session has, or `not_found`.
fn registered(session: &Session, id: &str) -> Result<String, CommandError> {
    match session.commands().iter().find(|spec| spec.id == id) {
        Some(spec) => Ok(spec.id.to_owned()),
        None => Err(CommandError::not_found(
            format!("no command has the id `{}`, so there is no view of it to open", clip(id)),
            "pass an id `commands` lists, or \"\" to close the view",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppInfo, CallOpts, Caller, ErrorCode};
    use serde_json::{Value, json};

    fn session() -> Session {
        Session::new(AppInfo { name: "demo".into(), version: "0.3.0".into() }).unwrap()
    }

    #[test]
    fn app_info_reports_name_version_and_schema_version() {
        let wire = session().call("app.info", json!({}), &CallOpts::new(Caller::Test)).unwrap();
        assert_eq!(wire, json!({"name": "demo", "version": "0.3.0", "schema_version": SCHEMA_VERSION}));
        let typed = session().call_typed::<InfoCommand>(InfoParams::default(), &CallOpts::new(Caller::Test)).unwrap();
        assert_eq!(typed, Info { name: "demo".into(), version: "0.3.0".into(), schema_version: SCHEMA_VERSION });
    }

    /// `app.info` has no effect and only `Requirement::Session`, which always holds: it is never
    /// disabled and never needs consent (the dispatch's disabled states are tested in `session.rs`).
    #[test]
    fn app_info_is_always_enabled() {
        let states = session().states();
        assert!(states.iter().any(|s| s.id == "app.info" && s.enabled && s.why.is_none()), "{states:?}");
    }

    #[test]
    fn app_info_takes_no_params() {
        let e = session().call("app.info", json!({"verbose": true}), &CallOpts::new(Caller::Test)).unwrap_err();
        assert_eq!((e.code, e.key.as_deref()), (ErrorCode::BadParams, Some("verbose")));
    }

    /// `ui.set` changes only what it is given, and `ui.inspect` (and the session) then say so; an id
    /// that is no command is `not_found` and changes nothing.
    #[test]
    fn ui_set_changes_what_it_is_given_and_ui_inspect_reads_it_back() {
        let s = session();
        let call = |id: &str, params| s.call(id, params, &CallOpts::new(Caller::Test));
        let start = json!({"schema_version": SCHEMA_VERSION, "selected": null, "show_list": true, "dock": null});
        assert_eq!(call("ui.inspect", json!({})).unwrap(), start);
        let opened = call("ui.set", json!({"selected": "app.info"})).unwrap();
        assert_eq!((opened["selected"].clone(), opened["show_list"].clone()), (json!("app.info"), json!(true)));
        assert_eq!(call("ui.set", json!({"show_list": false})).unwrap()["selected"], "app.info", "what isn't given stays");
        assert_eq!(call("ui.inspect", json!({})).unwrap(), json!({"schema_version": SCHEMA_VERSION, "selected": "app.info", "show_list": false, "dock": null}));
        let e = call("ui.set", json!({"selected": "app.nope", "show_list": true})).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(!s.ui().unwrap().show_list, "a refused call changes nothing");
        assert_eq!(call("ui.set", json!({"selected": ""})).unwrap()["selected"], Value::Null, "\"\" closes the view");
        let e = call("ui.set", json!({"show_list": "yes"})).unwrap_err();
        assert_eq!((e.code, e.key.as_deref()), (ErrorCode::BadParams, Some("show_list")));
    }

    /// `ui.set` lays out the dock: a layout it checks is kept, and one too deep, too big, with an
    /// empty part or a pane the dock doesn't have is refused and changes nothing.
    #[test]
    fn ui_set_lays_out_the_dock_and_refuses_a_layout_it_cant_build() {
        let s = session();
        let call = |params| s.call("ui.set", params, &CallOpts::new(Caller::Test));
        let stacked = json!({"kind": "split", "across": false, "parts": [
            {"share": 1, "node": {"kind": "tabs", "panes": ["view"], "active": 0}},
            {"share": 2, "node": {"kind": "tabs", "panes": ["commands", "view"], "active": 9}}]});
        assert_eq!(call(json!({"dock": stacked.clone()})).unwrap()["dock"], stacked);
        let mut deep = json!({"kind": "tabs", "panes": ["view"], "active": 0});
        for _ in 0..crate::ui::DOCK_DEPTH {
            deep = json!({"kind": "split", "across": true, "parts": [{"share": 1, "node": deep}]});
        }
        let many = json!({"kind": "tabs", "panes": vec!["view"; crate::ui::DOCK_ITEMS], "active": 0});
        let refused = [
            (deep, ErrorCode::TooLarge),
            (many, ErrorCode::TooLarge),
            (json!({"kind": "tabs", "panes": [], "active": 0}), ErrorCode::BadParams),
            (json!({"kind": "split", "across": true, "parts": []}), ErrorCode::BadParams),
            (json!({"kind": "split", "across": true, "parts": [{"share": 0, "node": {"kind": "tabs", "panes": ["view"], "active": 0}}]}), ErrorCode::BadParams),
            (json!({"kind": "tabs", "panes": ["nope"], "active": 0}), ErrorCode::NotFound),
        ];
        for (dock, code) in refused {
            assert_eq!(call(json!({"dock": dock.clone()})).unwrap_err().code, code, "{dock}");
        }
        assert_eq!(s.ui().unwrap().dock.map(|d| serde_json::to_value(d).unwrap()), Some(stacked), "a refused layout changes nothing");
        assert!(DockNode::default_layout().check().is_ok());
    }

    /// `ui.theme` gives the theme's tokens and ranges as `#RRGGBB`, the same on the wire as typed, and
    /// is in the View menu.
    #[test]
    fn ui_theme_gives_the_theme_as_data() {
        let wire = session().call("ui.theme", json!({}), &CallOpts::new(Caller::Test)).unwrap();
        let typed = session().call_typed::<UiThemeCommand>(UiThemeParams::default(), &CallOpts::new(Caller::Test)).unwrap();
        assert_eq!(wire, serde_json::to_value(&typed).unwrap());
        assert_eq!(typed, theme::data());
        let error = typed.tokens.iter().find(|t| t.name == "error").unwrap();
        assert_eq!((error.dark.as_str(), error.light.as_str()), ("#FF8A80", "#B42318"), "the shared palette's");
        assert_eq!(wire["ranges"][3]["entries"][31]["index"], 31);
        assert_eq!((wire["ranges"][0]["name"].as_str(), wire["ranges"][0]["none"]["dark"].as_str()), (Some("series"), Some("#666666")));
        assert!(wire["ranges"][1]["none"].is_null(), "the markers have no none");
        let e = session().call("ui.theme", json!({"look": "dark"}), &CallOpts::new(Caller::Test)).unwrap_err();
        assert_eq!((e.code, e.key.as_deref()), (ErrorCode::BadParams, Some("look")));
        let registry = registry().unwrap();
        let specs = registry.commands();
        let spec = specs.iter().find(|s| s.id == "ui.theme").unwrap();
        assert_eq!((spec.menu, spec.effects.needs_consent()), (Some("View/Theme"), false));
    }

    /// A saved state comes back, less a selection of a command this version doesn't have.
    #[test]
    fn a_restored_state_drops_a_command_that_is_gone() {
        let s = session();
        let saved = UiState { selected: Some("app.gone".into()), show_list: false, ..UiState::default() };
        assert_eq!(s.restore_ui(saved).unwrap(), UiState { selected: None, show_list: false, ..UiState::default() });
        let saved = UiState { selected: Some("app.info".into()), ..UiState::default() };
        assert_eq!(s.restore_ui(saved.clone()).unwrap(), saved);
        assert_eq!(s.ui().unwrap(), saved);
    }

    #[test]
    fn the_registry_holds_the_example_with_its_schemas() {
        let registry = registry().unwrap();
        let specs = registry.commands();
        // The example is looked up by id: the commands added beside it must not break this test.
        let info = specs.iter().find(|s| s.id == "app.info").unwrap();
        assert!(!info.effects.needs_consent() && info.requires == [Requirement::Session]);
        assert_eq!((info.menu, info.shortcut), (Some("Help/App info"), None), "it is in the Help menu, with no shortcut");
        let result = serde_json::to_value(&info.result).unwrap();
        assert_eq!(result["properties"]["schema_version"]["type"], "integer");
    }
}
