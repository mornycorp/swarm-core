//! What the UI shows of a command: one view per command that has one. A view makes the command's
//! **typed** call (`Session::call_typed`) and turns the typed output into [`Shown`] lines, so it
//! reads fields, never a `serde_json::Value`. A view runs on a worker ([`crate::jobs`]), never on the
//! UI thread.
//!
//! **What is drawn is what the command returns** (docs/app-architecture.md › The UI is a client): a
//! view labels the result's fields (its words from the [`catalog`]) and draws their values as the
//! command gave them, computing nothing of its own, and a test draws a frame and checks the drawn
//! text against the command's typed answer (`the_view_draws_app_infos_result`). A colour a result
//! gives (`ui.theme`'s) is a [`Swatch`], drawn in the look the window is in; the view names no
//! colour itself (`the_swatch_view_draws_ui_themes_answer`).

use {{crate_prefix_ident}}engine::commands::{InfoCommand, InfoParams, UiThemeCommand, UiThemeParams};
use {{crate_prefix_ident}}engine::theme::{EntryData, Look, RangeData, TokenData};
use {{crate_prefix_ident}}engine::{CallOpts, Command, CommandError, Session};

use crate::catalog;

/// A typed result as the UI draws it: labelled lines, then rows of colours.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shown {
    pub fields: Vec<(String, String)>,
    pub swatches: Vec<SwatchRow>,
}

impl Shown {
    /// This result with one more line.
    #[must_use]
    pub fn field(mut self, label: &str, value: impl ToString) -> Shown {
        self.fields.push((label.to_owned(), value.to_string()));
        self
    }
}

/// A colour a command gave, `#RRGGBB`, in each look: the frame draws the one its look is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PerLook {
    pub dark: String,
    pub light: String,
}

impl PerLook {
    /// The value in `look`.
    pub fn of(&self, look: Look) -> &str {
        match look {
            Look::Dark => &self.dark,
            Look::Light => &self.light,
        }
    }
}

/// A named row of colours: a token (one square, its value under it) or a range (a square per entry,
/// each numbered).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SwatchRow {
    pub label: String,
    /// What it is, a line each.
    pub lines: Vec<String>,
    pub swatches: Vec<Swatch>,
}

/// A filled square of a colour.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Swatch {
    pub colour: PerLook,
    /// A range entry's number, written on its text background; `None`: a token, whose value is written.
    pub number: Option<Numbered>,
}

/// A range entry's index, and the entry made a background for text, on which the index is written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Numbered {
    pub index: String,
    pub behind_text: PerLook,
}

/// How the UI calls one command and shows its typed result. `run` is made with the call's options
/// (the caller is `Caller::Ui`, and the consent is what the user gave).
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// The command's id (`Command::ID`).
    pub id: &'static str,
    pub run: fn(&Session, &CallOpts) -> Result<Shown, CommandError>,
}

/// The app's views: one line per command the UI can run. The commands without one are still
/// listed, as not yet viewable.
pub fn views() -> Vec<View> {
    vec![View { id: InfoCommand::ID, run: show_info }, View { id: UiThemeCommand::ID, run: show_theme }]
}

/// `app.info`: its typed output, a line a field.
fn show_info(session: &Session, opts: &CallOpts) -> Result<Shown, CommandError> {
    let info = session.call_typed::<InfoCommand>(InfoParams::default(), opts)?;
    Ok(Shown::default()
        .field(catalog::INFO_NAME, info.name)
        .field(catalog::INFO_VERSION, info.version)
        .field(catalog::INFO_SCHEMA_VERSION, info.schema_version))
}

/// `ui.theme`, the swatch view: each token a square with its name, meaning and value; each range a
/// row of squares numbered by index, with its size, meaning and source.
fn show_theme(session: &Session, opts: &CallOpts) -> Result<Shown, CommandError> {
    let theme = session.call_typed::<UiThemeCommand>(UiThemeParams::default(), opts)?;
    let tokens = theme.tokens.into_iter().map(token_row);
    let ranges = theme.ranges.into_iter().map(range_row);
    Ok(Shown { fields: Vec::new(), swatches: tokens.chain(ranges).collect() })
}

fn token_row(token: TokenData) -> SwatchRow {
    let colour = PerLook { dark: token.dark, light: token.light };
    SwatchRow { label: token.name, lines: vec![token.meaning], swatches: vec![Swatch { colour, number: None }] }
}

fn range_row(range: RangeData) -> SwatchRow {
    let lines = vec![catalog::range_size(range.size), range.meaning, catalog::source(&range.source)];
    SwatchRow { label: range.name, lines, swatches: range.entries.into_iter().map(entry).collect() }
}

fn entry(entry: EntryData) -> Swatch {
    let behind_text = PerLook { dark: entry.behind_text_dark, light: entry.behind_text_light };
    Swatch { colour: PerLook { dark: entry.dark, light: entry.light }, number: Some(Numbered { index: entry.index.to_string(), behind_text }) }
}
