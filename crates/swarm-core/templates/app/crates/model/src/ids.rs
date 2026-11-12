//! The id rule (docs/app-architecture.md › The id rule), and the one place every surface derives
//! its name for a command from.
//!
//! A command id is `<area>.<name>`: two or more dot-separated segments, each `[a-z][a-z0-9_]*`,
//! at most [`MAX_LEN`] characters in all (`app.info`, `project.open_recent`). The CLI and the
//! JSON-lines `method` use the id as it is; MCP's tool name is the id with each `.` replaced by `-`
//! ([`CommandId::tool_name`]). An id has no `-` and a tool name no `.`, so the mapping is one to
//! one and a tool name always resolves to one command ([`CommandId::from_tool_name`]).

use std::fmt;

/// The longest id, and so the longest tool name (MCP allows 64).
pub const MAX_LEN: usize = 64;

/// A command id that follows the rule: the only way to make one is [`CommandId::parse`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(String);

/// Why a text is not a command id, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    Empty,
    /// More than [`MAX_LEN`] characters.
    TooLong {
        len: usize,
    },
    /// A single segment: an id is `<area>.<name>` at least, which keeps it clear of the
    /// protocol's own methods (`commands`, `enabled`, `auth`).
    OneSegment,
    /// The 0-based `segment` is empty, doesn't start with a lowercase letter, or holds a character
    /// outside `[a-z0-9_]`; `at` is the 0-based character in the whole text.
    BadSegment {
        segment: usize,
        at: usize,
        found: Option<char>,
    },
    /// A tool name with a `.`: tool names replace every dot.
    DotInToolName {
        at: usize,
    },
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::Empty => write!(f, "the id is empty"),
            IdError::TooLong { len } => write!(f, "the id is {len} characters, over the {MAX_LEN} an id may have"),
            IdError::OneSegment => write!(f, "the id has one segment: an id is `<area>.<name>`"),
            IdError::BadSegment { segment, at, found: Some(c) } => {
                write!(f, "segment {segment} breaks `[a-z][a-z0-9_]*` at character {at} ({c:?})")
            }
            IdError::BadSegment { segment, at, found: None } => write!(f, "segment {segment} is empty (character {at})"),
            IdError::DotInToolName { at } => write!(f, "a tool name has no `.` (character {at}): it replaces them with `-`"),
        }
    }
}

impl std::error::Error for IdError {}

impl CommandId {
    /// `text` as an id, or why it is not one.
    pub fn parse(text: &str) -> Result<CommandId, IdError> {
        let len = text.chars().count();
        if len == 0 {
            return Err(IdError::Empty);
        }
        if len > MAX_LEN {
            return Err(IdError::TooLong { len });
        }
        let mut at = 0;
        let mut segments = 0;
        for (segment, part) in text.split('.').enumerate() {
            check_segment(part, segment, at)?;
            at += part.chars().count() + 1;
            segments += 1;
        }
        if segments < 2 {
            return Err(IdError::OneSegment);
        }
        Ok(CommandId(text.to_owned()))
    }

    /// The id as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The area: the first segment (`app` of `app.info`).
    pub fn area(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }

    /// MCP's tool name for this id: each `.` replaced by `-`.
    pub fn tool_name(&self) -> String {
        self.0.replace('.', "-")
    }

    /// The id a tool `name` stands for: each `-` replaced by `.`, then the rule. A name with a `.`
    /// is refused, since no tool name has one.
    pub fn from_tool_name(name: &str) -> Result<CommandId, IdError> {
        if let Some(at) = name.chars().position(|c| c == '.') {
            return Err(IdError::DotInToolName { at });
        }
        CommandId::parse(&name.replace('-', "."))
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One segment against `[a-z][a-z0-9_]*`; `start` is its first character's place in the text.
fn check_segment(part: &str, segment: usize, start: usize) -> Result<(), IdError> {
    let bad = |offset: usize, found: Option<char>| IdError::BadSegment { segment, at: start + offset, found };
    for (offset, c) in part.chars().enumerate() {
        let fine = if offset == 0 { c.is_ascii_lowercase() } else { c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' };
        if !fine {
            return Err(bad(offset, Some(c)));
        }
    }
    if part.is_empty() { Err(bad(0, None)) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_accepts_area_dot_name() {
        for id in ["app.info", "project.open_recent", "a.b", "a.b.c", "x1.y_2", &format!("a.{}", "b".repeat(62))] {
            let parsed = CommandId::parse(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(parsed.as_str(), id);
        }
        assert_eq!(CommandId::parse("project.open_recent").unwrap().area(), "project");
    }

    #[test]
    fn each_way_to_break_the_rule_is_refused_with_where() {
        let at = |id: &str| CommandId::parse(id).unwrap_err();
        assert_eq!(at(""), IdError::Empty);
        assert_eq!(at("commands"), IdError::OneSegment);
        assert_eq!(at(&format!("a.{}", "b".repeat(63))), IdError::TooLong { len: 65 });
        assert_eq!(at("app..info"), IdError::BadSegment { segment: 1, at: 4, found: None });
        assert_eq!(at(".info"), IdError::BadSegment { segment: 0, at: 0, found: None });
        assert_eq!(at("app."), IdError::BadSegment { segment: 1, at: 4, found: None });
        assert_eq!(at("App.info"), IdError::BadSegment { segment: 0, at: 0, found: Some('A') });
        assert_eq!(at("app.1st"), IdError::BadSegment { segment: 1, at: 4, found: Some('1') });
        assert_eq!(at("app._x"), IdError::BadSegment { segment: 1, at: 4, found: Some('_') });
        assert_eq!(at("app.in-fo"), IdError::BadSegment { segment: 1, at: 6, found: Some('-') });
        assert_eq!(at("app.info "), IdError::BadSegment { segment: 1, at: 8, found: Some(' ') });
        assert_eq!(at("app.in\u{fb01}o"), IdError::BadSegment { segment: 1, at: 6, found: Some('\u{fb01}') });
        assert_eq!(at("app.\u{0}"), IdError::BadSegment { segment: 1, at: 4, found: Some('\u{0}') });
        assert!(at("app..info").to_string().contains("segment 1 is empty"));
    }

    #[test]
    fn a_tool_name_and_its_id_map_one_to_one() {
        let id = CommandId::parse("project.open_recent").unwrap();
        assert_eq!(id.tool_name(), "project-open_recent");
        assert_eq!(CommandId::from_tool_name("project-open_recent").unwrap(), id);
        assert_eq!(CommandId::from_tool_name("app.info").unwrap_err(), IdError::DotInToolName { at: 3 });
        assert!(CommandId::from_tool_name("appinfo").is_err() && CommandId::from_tool_name("-info").is_err());
        assert!(CommandId::from_tool_name("app-").is_err() && CommandId::from_tool_name("").is_err());
    }

    /// Every text of up to five characters over an alphabet of the troublemakers is parsed
    /// without a panic, and what parses round-trips through the tool name.
    #[test]
    fn hostile_texts_never_panic_and_what_parses_round_trips() {
        let alphabet = ['a', 'Z', '0', '_', '.', '-', ' ', '\u{e9}'];
        let mut texts = vec![String::new()];
        let mut parsed = 0;
        for _ in 0..5 {
            let longer: Vec<String> = texts.iter().flat_map(|t| alphabet.iter().map(move |c| format!("{t}{c}"))).collect();
            for text in &longer {
                if let Ok(id) = CommandId::parse(text) {
                    parsed += 1;
                    assert_eq!(CommandId::from_tool_name(&id.tool_name()).unwrap(), id, "{text}");
                    assert!(!id.tool_name().contains('.') && !id.as_str().contains('-'));
                }
            }
            texts = longer;
        }
        assert!(parsed > 0);
        let huge = "a.".repeat(10_000);
        assert!(matches!(CommandId::parse(&huge), Err(IdError::TooLong { .. })));
    }
}
