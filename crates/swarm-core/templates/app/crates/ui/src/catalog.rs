//! The string catalog: every word the shell draws is here, never a literal where it is drawn
//! (docs/app-architecture.md › The UI is a client). A rewording, a second language or a test of
//! what a view says then reads one file. A text with values in it is a function of them, so its
//! words and their order stay here too. `scan` holds the rest of the crate to it (`ui-strings`: a
//! string literal outside this module is a finding, held to an allow-list for one that is never
//! drawn, such as a thread's name).
//!
//! The words of the app's data (a command's title, a result's values) aren't here: they come from
//! the registry and the commands, as they are.

/// The command list's heading.
pub const COMMANDS: &str = "Commands";
/// The button that hides the command list.
pub const HIDE_LIST: &str = "Hide";
/// The button that shows it again.
pub const SHOW_LIST: &str = "Show the command list";
/// What the open view says when no command is selected.
pub const SELECT_A_COMMAND: &str = "Select a command in the list or the menus.";
/// The consent box of a command with effects.
pub const CONSENT: &str = "I agree to this command's effects (it changes something outside the app)";
/// A command the UI can't run.
pub const NO_VIEW: &str = "The UI has no view for this command yet. Run it with the CLI, or add a view (crates/ui/src/views.rs).";
pub const RUN: &str = "Run";
pub const CANCEL: &str = "Cancel";

/// `app.info`'s fields, as its view labels them.
pub const INFO_NAME: &str = "Name";
pub const INFO_VERSION: &str = "Version";
pub const INFO_SCHEMA_VERSION: &str = "Schema version";

/// A categorical range's size, in the swatch view (`ui.theme`).
pub fn range_size(size: u32) -> String {
    format!("{size} colours")
}

/// Where a range's colours come from, in the swatch view.
pub fn source(source: &str) -> String {
    format!("Source: {source}")
}

/// A line of the command list.
pub fn row(title: &str, id: &str) -> String {
    format!("{title}  {id}")
}

/// Why a command can't run now.
pub fn not_available(message: &str) -> String {
    format!("Not available now: {message}")
}

/// What to do about a failure.
pub fn fix(remediation: &str) -> String {
    format!("Fix: {remediation}")
}

/// A disabled menu item's tooltip: why, and what to do.
pub fn why_not(message: &str, remediation: &str) -> String {
    format!("{message}\n{}", fix(remediation))
}

/// A call in flight.
pub fn running(id: &str) -> String {
    format!("Running {id}...")
}

/// A failed call: its code and message.
pub fn failed(code: &str, message: &str) -> String {
    format!("{code}: {message}")
}

/// The param a failure names.
pub fn param(key: &str) -> String {
    format!("Param: {key}")
}

/// A view that panicked: the `internal` failure its call becomes.
pub fn view_panicked(id: &str) -> String {
    format!("the view of `{id}` panicked")
}

/// A worker that couldn't be started.
pub fn no_worker(id: &str, why: &str) -> String {
    format!("cannot start a worker for `{id}`: {why}")
}
