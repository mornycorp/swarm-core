//! What a command is (docs/app-architecture.md › The minimum interfaces): the [`Command`] trait
//! an author implements, and the [`CommandSpec`] every surface reads.

use serde::Serialize;

use crate::call::CallContext;
use crate::error::CommandError;
use crate::session::Session;

/// What must hold for a command to run: a condition many commands share (an open project, a
/// connected device), declared in their specs. One that doesn't hold makes the command disabled
/// ([`Why`]): its call fails with the `unavailable` code and says which. The skeleton has the
/// session only; an app adds its own and checks them in [`Requirement::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    /// A session exists (always true: a call is made on one).
    Session,
}

impl Requirement {
    /// Whether this requirement holds on `session`, and why not. Cheap, as [`Command::enabled`] is.
    pub fn check(self, session: &Session) -> Result<(), Why> {
        let _ = session;
        match self {
            Requirement::Session => Ok(()),
        }
    }
}

/// Why a command can't run now: what a disabled menu item says, and what a call of it fails with
/// (`unavailable`, with this message and remediation). Only the code `unavailable` can be made from
/// one, so a disabled command never fails another way for being disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Why {
    /// What doesn't hold, for a person: "no project is open".
    pub message: String,
    /// What to do about it: "open a project first". Empty gets `unavailable`'s default.
    pub remediation: String,
}

impl Why {
    pub fn new(message: impl Into<String>, remediation: impl Into<String>) -> Why {
        Why { message: message.into(), remediation: remediation.into() }
    }
}

impl From<Why> for CommandError {
    fn from(why: Why) -> CommandError {
        CommandError::unavailable(why.message, why.remediation)
    }
}

/// Whether a command can run now ([`Session::states`]): `why` is the very error a call of it gets
/// (`unavailable`), and `None` when it is enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandState {
    pub id: &'static str,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<CommandError>,
}

/// What a command can change outside the app's memory. All false: it only reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Effects {
    /// Writes files.
    pub writes: bool,
    /// Runs other programs.
    pub runs_code: bool,
    /// Uses the network.
    pub network: bool,
    /// Changes a device.
    pub device: bool,
}

impl Effects {
    /// Whether a surface asks for consent before the command runs: any effect does.
    pub const fn needs_consent(self) -> bool {
        self.writes || self.runs_code || self.network || self.device
    }
}

/// What a command is, as every surface sees it. Built only by [`Registry::register`], so no
/// command is registered without schemas or effects.
///
/// [`Registry::register`]: crate::Registry::register
#[derive(Debug, Clone, Serialize)]
pub struct CommandSpec {
    /// `app.info`, by the id rule.
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    /// Generated from [`Command::Params`].
    pub params: schemars::Schema,
    /// Generated from [`Command::Output`].
    pub result: schemars::Schema,
    pub effects: Effects,
    pub requires: &'static [Requirement],
    /// Where it sits in a menu, `File/Open recent` ([`crate::menu`]): the menus, then its label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu: Option<&'static str>,
    /// The keys that run it, portable: `Mod+O` ([`crate::menu`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<&'static str>,
}

/// The typed adapter: a command is a type. [`Registry::register`] is the only place a
/// [`CommandSpec`] is built from one.
///
/// Params that take no keys are an empty struct with `#[serde(deny_unknown_fields)]`, so an
/// unknown key is a `bad_params` that names it. Doc comments on the params and output types (and
/// their fields) become the descriptions in the schemas.
///
/// [`Registry::register`]: crate::Registry::register
pub trait Command: Send + Sync + 'static {
    type Params: serde::de::DeserializeOwned + schemars::JsonSchema;
    type Output: serde::Serialize + schemars::JsonSchema;
    /// `<area>.<name>`: the id rule (`ids::CommandId`). Never renamed once released.
    const ID: &'static str;
    /// One line, the words a command list shows.
    const TITLE: &'static str;
    /// One line: what it does.
    const SUMMARY: &'static str;
    const EFFECTS: Effects = Effects { writes: false, runs_code: false, network: false, device: false };
    const REQUIRES: &'static [Requirement] = &[];
    /// Its place in the app's menus, `File/Open recent`: the menus, then the item's label
    /// ([`crate::menu`]). None: it is in no menu.
    const MENU: Option<&'static str> = None;
    /// The keys that run it, `Mod+O` ([`crate::menu`]). None: it has no shortcut.
    const SHORTCUT: Option<&'static str> = None;
    /// Whether it can run now on `session`'s state, beyond its [`Command::REQUIRES`] (the dispatch
    /// checks those first): `Err` says why not. For a condition only this command has (nothing to
    /// undo, nothing selected); one many commands share is a [`Requirement`]. It never sees the
    /// params, and it is **cheap**: it reads memory, does no I/O, never waits and changes nothing,
    /// because a UI asks it of every command each frame ([`Session::states`]). The dispatch makes
    /// the same check, so a disabled command's call fails `unavailable` with this reason.
    fn enabled(&self, session: &Session) -> Result<(), Why> {
        let _ = session;
        Ok(())
    }
    /// Run it. A slow one polls `cx.cancel` ([`CallContext::check_cancelled`]) and returns
    /// `cancelled`. Never panics: a failure is a [`CommandError`].
    fn run(&self, session: &Session, params: Self::Params, cx: &CallContext) -> Result<Self::Output, CommandError>;
}
