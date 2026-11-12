//! What a scan looks for in a stripped source ([`super::source::strip`]): each matcher returns the
//! 1-based line of every match, one entry per match. They read text, not Rust, so each is a little
//! conservative: `v["key"]` can be a map's index as well as a `Value`'s, and the allow-list is where
//! a reasoned exception goes.

/// The file writes the UI and the state module are held to (`std::fs`, `tokio::fs` and `File`).
pub const FILE_WRITES: &[&str] = &[
    "fs::write",
    "fs::remove_file",
    "fs::remove_dir",
    "fs::remove_dir_all",
    "fs::create_dir",
    "fs::create_dir_all",
    "fs::copy",
    "fs::rename",
    "fs::set_permissions",
    "fs::hard_link",
    "fs::soft_link",
    "fs::symlink",
    "fs::symlink_file",
    "fs::symlink_dir",
    "File::create",
    "File::create_new",
    ".set_len(",
];

/// How a file is opened by its options: [`write_opens`] counts one that asks to write.
const OPENS: &[&str] = &["OpenOptions", "File::options"];

/// What an `OpenOptions` chain says when it opens to write.
const WRITES: &[&str] = &[".write(true)", ".append(true)", ".create(true)", ".create_new(true)", ".truncate(true)"];

/// How far a statement is read for its options: a chain is nowhere near it.
const MAX_STATEMENT: usize = 2000;

/// Files opened to write by their options: `OpenOptions` or `File::options`, in a statement (to
/// its `;`) that turns on `write`, `append`, `create`, `create_new` or `truncate`. An import of
/// the name, and a read-only open, are no write.
pub fn write_opens(source: &str) -> Vec<usize> {
    let starts = line_starts(source);
    let mut lines: Vec<usize> = OPENS
        .iter()
        .flat_map(|name| words(source, name))
        .filter(|(_, end)| {
            let rest = source.get(*end..).unwrap_or("");
            let rest = rest.get(..rest.char_indices().nth(MAX_STATEMENT).map_or(rest.len(), |(i, _)| i)).unwrap_or(rest);
            let statement = rest.split(';').next().unwrap_or("");
            // Blanks and line breaks inside the call don't hide it: `.write( true )`.
            let squeezed: String = statement.chars().filter(|c| !c.is_whitespace()).collect();
            WRITES.iter().any(|w| squeezed.contains(w))
        })
        .map(|(at, _)| line_at(&starts, at))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// A program started other than through `std::process::Command`: opening a path or a URL with
/// the system's handler.
pub const OPENERS: &[&str] = &["open::that", "open::with", "opener::open", "webbrowser::open"];

/// A colour made in place rather than taken from a theme's token: egui's colour types' constructors
/// and constants (`Color32::from_rgb(…)`, `Color32::RED`, `Rgba::…`, `Hsva::…`) and its `hex_color!`.
pub const COLOURS: &[&str] = &["Color32::", "Rgba::", "Hsva::", "HsvaGamma::", "hex_color!"];

/// Names that reach the real config, data or home directory: a test uses a temp dir instead.
pub const REAL_CONFIG_DIR: &[&str] = &[
    "dirs::config_dir",
    "dirs::config_local_dir",
    "dirs::data_dir",
    "dirs::data_local_dir",
    "dirs::home_dir",
    "dirs::state_dir",
    "dirs::cache_dir",
    "ProjectDirs",
    "BaseDirs",
    "UserDirs",
    "home_dir",
    "\"XDG_CONFIG_HOME\"",
    "\"XDG_DATA_HOME\"",
    "\"XDG_STATE_HOME\"",
    "\"APPDATA\"",
    "\"LOCALAPPDATA\"",
    "\"HOME\"",
    "\"USERPROFILE\"",
    "\".config\"",
    "\"Application Support\"",
];

/// The byte offsets where each line of `text` starts.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect()
}

/// The 1-based line holding byte `at`.
fn line_at(starts: &[usize], at: usize) -> usize {
    starts.partition_point(|s| *s <= at)
}

/// Whether `c` can be in an identifier.
fn ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The char before byte `at` in `text`.
fn before(text: &str, at: usize) -> Option<char> {
    text.get(..at).and_then(|t| t.chars().next_back())
}

/// The char at byte `at` of `text`.
fn after(text: &str, at: usize) -> Option<char> {
    text.get(at..).and_then(|t| t.chars().next())
}

/// Every place `needle` is in `text`, as `(start, end)` byte offsets: not in the middle of an
/// identifier, where `needle` begins or ends with an identifier character
/// (`File::create` is not `MyFile::create`, nor `File::create_new`).
fn words<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    let (first, last) = (needle.chars().next(), needle.chars().next_back());
    text.match_indices(needle).map(|(i, m)| (i, i + m.len())).filter(move |(start, end)| {
        let left = first.is_some_and(ident) && before(text, *start).is_some_and(ident);
        let right = last.is_some_and(ident) && after(text, *end).is_some_and(ident);
        !left && !right
    })
}

/// Each of `needles`, as the lines it is on.
pub fn text(source: &str, needles: &[&str]) -> Vec<usize> {
    let starts = line_starts(source);
    let mut lines: Vec<usize> = needles.iter().filter(|n| !n.is_empty()).flat_map(|n| words(source, n)).map(|(i, _)| line_at(&starts, i)).collect();
    lines.sort_unstable();
    lines
}

/// Mentions of `std::process::Command`: the path `process::Command` (also `tokio::process::…`),
/// `Command` inside a `process::{…}` group (which can span lines and nest), a `process::*` glob,
/// and the module by another name (`use std::process as p;` then `p::Command`). A `process`
/// module of the crate's own (`crate::process::Command`, `self::`, `super::`) is not the standard
/// library's: the app's process module is what the rule sends callers to.
pub fn process_command(source: &str) -> Vec<usize> {
    module_items(source, "process", &["Command"], true)
}

/// The `std::fs` functions that write, as a `fs::{…}` group or a `fs::*` glob can import them
/// (`fs::write` itself is one of [`FILE_WRITES`]).
const FS_WRITES: &[&str] =
    &["write", "remove_file", "remove_dir", "remove_dir_all", "create_dir", "create_dir_all", "copy", "rename", "set_permissions", "hard_link", "soft_link"];

/// The file writes imported into scope, which [`FILE_WRITES`] doesn't see: a write named in a
/// `fs::{…}` group (`use std::fs::{self, write};` then `write(…)`), a `fs::*` glob, and the module by
/// another name (`use std::fs as f;` then `f::write`). The import is the match.
pub fn fs_imports(source: &str) -> Vec<usize> {
    module_items(source, "fs", FS_WRITES, false)
}

/// The `dirs` functions that reach a real directory, as a `dirs::{…}` group or a `dirs::*` glob can
/// import them (`home_dir` is one of [`REAL_CONFIG_DIR`] by itself).
const DIRS: &[&str] = &["config_dir", "config_local_dir", "data_dir", "data_local_dir", "state_dir", "cache_dir"];

/// The `dirs` functions imported into scope, which [`REAL_CONFIG_DIR`] doesn't see: in a
/// `dirs::{…}` group, a `dirs::*` glob, or the crate by another name. The import is the match.
pub fn dirs_imports(source: &str) -> Vec<usize> {
    module_items(source, "dirs", DIRS, false)
}

/// The line of each mention of one of `items` of module `module`: by the module's path
/// (`module::item`, counted only when `direct`: a text pattern may count it already) or by an
/// alias's (`use x::module as m;`, `module::{self as m}`, then `m::item`), in a `module::{…}`
/// group (at the group's line), and each `module::*` glob. Linear in `source`.
fn module_items(source: &str, module: &str, items: &[&str], direct: bool) -> Vec<usize> {
    let starts = line_starts(source);
    let aliases = aliases(source, module);
    let mut lines = Vec::new();
    let mut opens = Vec::new();
    for (at, _) in source.match_indices("::") {
        let name = ident_before(source, at);
        if !aliases.contains(name) || own_module(source, at - name.len()) {
            continue;
        }
        let rest = source.get(at + 2..).unwrap_or("");
        let item = |i: &&str| rest.strip_prefix(*i).is_some_and(|r| !r.chars().next().is_some_and(ident));
        if rest.starts_with('{') {
            opens.push(at + 2);
        } else if rest.starts_with('*') || ((direct || name != module) && items.iter().any(item)) {
            lines.push(line_at(&starts, at));
        }
    }
    for (open, group) in groups(source, &opens) {
        lines.extend(items.iter().flat_map(|i| words(group, i)).map(|_| line_at(&starts, open)));
    }
    lines.sort_unstable();
    lines
}

/// The names `module` goes by in `source`: its own, and each alias (`module as m`, and
/// `module::{self as m}`).
fn aliases<'a>(source: &'a str, module: &'a str) -> std::collections::BTreeSet<&'a str> {
    let mut names = std::collections::BTreeSet::from([module]);
    let after_as = |end: usize| {
        let rest = source.get(end..).unwrap_or("").trim_start().strip_prefix("as")?;
        let name = rest.trim_start();
        let len = name.find(|c: char| !ident(c)).unwrap_or(name.len());
        (rest.starts_with(char::is_whitespace) && len > 0).then(|| name.get(..len)).flatten()
    };
    names.extend(words(source, module).filter_map(|(_, end)| after_as(end)));
    let opens: Vec<usize> = source.match_indices("::{").filter(|(at, _)| ident_before(source, *at) == module).map(|(at, _)| at + 2).collect();
    for (open, group) in groups(source, &opens) {
        names.extend(words(group, "self").filter_map(|(_, end)| after_as(open + end)));
    }
    names
}

/// Whether the path segment that starts at byte `at` is reached from the crate itself: it follows
/// `crate::`, `self::` or `super::`.
fn own_module(source: &str, at: usize) -> bool {
    let Some(head) = source.get(..at).and_then(|h| h.strip_suffix("::")) else { return false };
    matches!(ident_before(head, head.len()), "crate" | "self" | "super")
}

/// The identifier that ends at byte `at` (`""` when there is none).
fn ident_before(source: &str, at: usize) -> &str {
    let head = source.get(..at).unwrap_or("");
    let start = head.char_indices().rev().take_while(|(_, c)| ident(*c)).last().map_or(at, |(i, _)| i);
    head.get(start..).unwrap_or("")
}

/// Each `{` at `opens` (ascending) with its group: the text up to the `}` that closes it, the
/// groups nested in it included (`process::{io::{Read}, Command}`). An open inside an earlier
/// group is that group's already, so no text is in two groups and the whole is linear.
fn groups<'a>(source: &'a str, opens: &[usize]) -> Vec<(usize, &'a str)> {
    let mut close = 0;
    let mut out = Vec::new();
    for open in opens.iter().copied() {
        if open < close {
            continue;
        }
        let mut depth = 0usize;
        let rest = source.get(open..).unwrap_or("");
        let end = rest.char_indices().find_map(|(i, c)| match c {
            '{' => {
                depth += 1;
                None
            }
            '}' => {
                depth = depth.saturating_sub(1);
                (depth == 0).then_some(i)
            }
            _ => None,
        });
        close = end.map_or(source.len(), |n| open + n);
        out.push((open, source.get(open..close).unwrap_or("")));
    }
    out
}

/// Reads of a `serde_json::Value` by key: `v["key"]` (an index, not an array literal; `call()?["key"]`
/// too), `.get("key")`, `.as_object()` and `.pointer(…)`. The key is any string literal (a blanked
/// one included).
pub fn value_reads(source: &str) -> Vec<usize> {
    let starts = line_starts(source);
    let mut lines: Vec<usize> = Vec::new();
    for (at, _) in source.match_indices("[\"") {
        if before(source, at).is_some_and(|c| ident(c) || matches!(c, ')' | ']' | '?')) && literal_then(source, at + 1, "]") {
            lines.push(line_at(&starts, at));
        }
    }
    for (at, m) in source.match_indices(".get(\"") {
        if literal_then(source, at + m.len() - 1, ")") {
            lines.push(line_at(&starts, at));
        }
    }
    lines.extend(source.match_indices(".as_object()").map(|(at, _)| line_at(&starts, at)));
    lines.extend(words(source, ".pointer(").chain(words(source, ".pointer_mut(")).map(|(at, _)| line_at(&starts, at)));
    lines.sort_unstable();
    lines
}

/// The string literals of a source whose strings are blanked ([`super::source::strip`]: each is
/// `"` and its content as `_`s, then `"`, whatever its kind): the line of each that holds text, not
/// counting `""` or one inside an attribute (`#[expect(…, reason = "…")]`, `#[serde(rename = "…")]`),
/// which is never drawn. Linear: each char is looked at once.
pub fn string_literals(source: &str) -> Vec<usize> {
    let starts = line_starts(source);
    let mut lines = Vec::new();
    // The depth of `[` inside the attribute being read; 0 outside one.
    let mut attribute = 0usize;
    let mut chars = source.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '#' if attribute == 0 && opens_attribute(&mut chars) => attribute = 1,
            '[' if attribute > 0 => attribute += 1,
            ']' if attribute > 0 => attribute -= 1,
            '"' if literal_has_text(&mut chars) && attribute == 0 => lines.push(line_at(&starts, at)),
            _ => {}
        }
    }
    lines
}

type Chars<'a> = std::iter::Peekable<std::str::CharIndices<'a>>;

/// After a `#`: whether `[` or `![` follows (an attribute), taken from `chars` when it does.
fn opens_attribute(chars: &mut Chars<'_>) -> bool {
    let mut ahead = chars.clone();
    ahead.next_if(|(_, c)| *c == '!');
    if ahead.next_if(|(_, c)| *c == '[').is_some() {
        *chars = ahead;
        return true;
    }
    false
}

/// After a blanked literal's opening `"`: takes it from `chars` to its closing `"`, and says whether
/// it held anything.
fn literal_has_text(chars: &mut Chars<'_>) -> bool {
    let mut empty = true;
    for (_, c) in chars.by_ref() {
        if c == '"' {
            break;
        }
        empty = false;
    }
    !empty
}

/// Whether the string literal that starts (its opening quote) at byte `at` is followed by `close`.
fn literal_then(source: &str, at: usize, close: &str) -> bool {
    let rest = source.get(at + 1..).unwrap_or("");
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => return rest.get(i + 1..).is_some_and(|r| r.starts_with(close)),
            '\n' => return false,
            _ => {}
        }
    }
    false
}

/// Values of struct `name` built other than by its one constructor: its literals (`Name { … }`), not
/// its definition (`struct Name {`), an `impl` block, a return type, a trait's `for Name {` or a
/// pattern (`let Name { id, .. } = s;`, `Name { .. } =>`); and `Name::default()`, alone or as a
/// literal's `..Name::default()`.
pub fn struct_literal(source: &str, name: &str) -> Vec<usize> {
    let starts = line_starts(source);
    let mut braces = None;
    let mut lines = Vec::new();
    for (start, end) in words(source, name) {
        let rest = source.get(end..).unwrap_or("").trim_start();
        let open = source.len() - rest.len();
        let literal = if rest.strip_prefix("::").is_some_and(|r| r.trim_start().strip_prefix("default").is_some_and(|d| !d.starts_with(ident))) {
            true
        } else if rest.starts_with('{') && !is_declaration(source.get(..start).unwrap_or("")) {
            let braces = braces.get_or_insert_with(|| brace_pairs(source));
            !is_pattern(source, start, open, braces.get(&open).copied())
        } else {
            false
        };
        if literal {
            lines.push(line_at(&starts, start));
        }
    }
    lines
}

/// Each `{` of `source` with its `}`, by byte offset; one that never closes has none.
fn brace_pairs(source: &str) -> std::collections::BTreeMap<usize, usize> {
    let (mut pairs, mut open) = (std::collections::BTreeMap::new(), Vec::new());
    for (at, c) in source.char_indices() {
        match c {
            '{' => open.push(at),
            '}' => {
                if let Some(o) = open.pop() {
                    pairs.insert(o, at);
                }
            }
            _ => {}
        }
    }
    pairs
}

/// Whether the braces `open`..`close` after a type's name at `start` are a pattern: after a `let`,
/// ending in a rest `..`, or followed by `=>`, `=`, `:` (a parameter), `|` or a guard's `if`.
fn is_pattern(source: &str, start: usize, open: usize, close: Option<usize>) -> bool {
    let Some(close) = close else { return false };
    let head = source.get(..start).unwrap_or("").trim_end();
    if head.rsplit(|c: char| !ident(c)).next() == Some("let") {
        return true;
    }
    let inside = source.get(open + 1..close).unwrap_or("").trim_end();
    if inside.strip_suffix("..").is_some_and(|before| before.trim_end().ends_with([',', '{']) || before.trim_end().is_empty()) {
        return true;
    }
    let after = source.get(close + 1..).unwrap_or("").trim_start();
    let guard = after.strip_prefix("if").is_some_and(|r| !r.starts_with(ident));
    after.starts_with("=>")
        || (after.starts_with('=') && !after.starts_with("=="))
        || (after.starts_with(':') && !after.starts_with("::"))
        || (after.starts_with('|') && !after.starts_with("||"))
        || guard
}

/// Whether `before` (the text up to a type's name) ends in a keyword or an arrow that makes
/// `Name {` a declaration, an impl or a function's body rather than a literal.
fn is_declaration(before: &str) -> bool {
    let head = before.trim_end();
    if head.ends_with("->") {
        return true;
    }
    let word = head.rsplit(|c: char| !ident(c)).next().unwrap_or("");
    ["struct", "enum", "union", "trait", "impl", "for", "type", "mod", "dyn"].contains(&word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_matches_whole_words_and_reports_each_line() {
        let src = "fs::write(a);\nMyFile::create(x);\nFile::create(y); File::create_new(z);\nlet q = std::fs::write(b);\n";
        assert_eq!(text(src, &["fs::write"]), [1, 4]);
        assert_eq!(text(src, &["File::create"]), [3], "not MyFile::create, not File::create_new");
        assert_eq!(text(src, &["File::create", "File::create_new"]), [3, 3]);
        assert_eq!(text("let h = \"HOME\"; let w = \"XDG_CONFIG_HOME\";", &["\"HOME\""]), [1]);
        assert!(text("", &["x"]).is_empty() && text("abc", &[""]).is_empty());
    }

    #[test]
    fn process_command_is_every_way_to_name_it() {
        assert_eq!(process_command("use std::process::Command;\nfn f() { std::process::Command::new(\"x\"); }\n"), [1, 2]);
        assert_eq!(process_command("use std::process::{Child, Command, Stdio};\n"), [1]);
        assert_eq!(process_command("use std::process::{\n    Child,\n    Command,\n};\nuse std::process::{Child, Stdio};\n"), [1]);
        assert_eq!(process_command("use tokio::process::Command;\n"), [1]);
        assert!(process_command("use std::os::unix::process::CommandExt;\nlet c = clap::Command::new(\"x\");\nstruct MyCommand;\n").is_empty());
    }

    #[test]
    fn value_reads_are_indexes_gets_and_objects_not_array_literals() {
        let reads = |s: &str| value_reads(s).len();
        assert_eq!(reads("let a = v[\"field\"].as_str();"), 1);
        assert_eq!(reads("x.get(\"activity\").and_then(|a| a.get(\"kind\"))"), 2);
        assert_eq!(reads("let Some(o) = v.as_object() else { return };"), 1);
        assert_eq!(reads("rows[0][\"id\"] == reply()[\"id\"]"), 2);
        assert_eq!(reads("v[\"a-b\"]; v.get(\"x y\")"), 2, "any literal key is a read");
        // Not reads: array literals, macros, a computed key, as_object_mut, a literal that never closes.
        assert_eq!(reads("const A: &[&str] = &[\"egui\"]; f([\"a\", \"b\"]); vec![\"x\"]; let a = [\"k\"]; m.get(key); v.as_object_mut();"), 0);
        assert_eq!(reads("v[\"unclosed\nx]"), 0);
        // On a blanked source the key is underscores, and it is still a read.
        assert_eq!(reads("v[\"_____\"]; x.get(\"_\")"), 2);
        // After a `?`, and by pointer.
        assert_eq!(reads("let id = call()?[\"id\"].as_str(); v.pointer(\"/a/b\"); v.pointer_mut(p); x.pointers(1);"), 3);
        assert_eq!(value_reads("a\nb\nv[\"k\"]\n"), [3]);
    }

    #[test]
    fn struct_literals_are_not_declarations_impls_or_return_types() {
        let n = |s: &str| struct_literal(s, "CommandSpec").len();
        assert_eq!(n("        CommandSpec { id: \"a.b\", title: \"A\" }"), 1);
        assert_eq!(n("let s = CommandSpec\n{ id: 1 };"), 1);
        assert_eq!(n("pub struct CommandSpec {\n}"), 0);
        assert_eq!(n("impl CommandSpec {\n}\nimpl Default for CommandSpec {\n}"), 0);
        assert_eq!(n("fn make() -> CommandSpec {\n}\nfn v() -> Vec<CommandSpec> {\n}"), 0);
        assert_eq!(n("struct MyCommandSpec {}\nlet a = CommandSpecs { };"), 0);
        assert_eq!(struct_literal("a\nb\nCommandSpec {}\n", "CommandSpec"), [3]);
    }

    #[test]
    fn a_struct_pattern_is_no_literal() {
        let n = |s: &str| struct_literal(s, "CommandSpec").len();
        assert_eq!(n("pub fn f(s: CommandSpec) { let CommandSpec { id, .. } = s; match s { CommandSpec { .. } => {} } }"), 0);
        assert_eq!(n("if let CommandSpec { id, title } = s {}\nwhile let Some(CommandSpec { id, .. }) = it.next() {}"), 0);
        assert_eq!(
            n(
                "fn f(CommandSpec { id, .. }: CommandSpec) {}\nlet g = |CommandSpec { id, title }| id;\nmatch s { CommandSpec { id, title } if id > 0 => 1, _ => 2 }"
            ),
            0
        );
        assert_eq!(n("CommandSpec { id, title } = s;\nmatch s { CommandSpec { id: 1, title } | CommandSpec { id: 2, title } => {} }"), 0);
        // Still literals: a functional update, one compared, one in a closing match arm, one never closed.
        assert_eq!(n("let a = CommandSpec { id, ..base };\nif a == CommandSpec { id: 1 } {}\nmatch x { _ => CommandSpec { id: 0 } }\nCommandSpec { id"), 4);
        assert_eq!(n("let r = 0..; let s = CommandSpec { range: 0.., id };"), 1);
    }

    #[test]
    fn a_default_built_struct_is_a_literal_too() {
        let n = |s: &str| struct_literal(s, "CommandSpec");
        assert_eq!(n("pub fn f() {\n    let mut s = CommandSpec::default();\n    s.id = \"x\";\n}"), [2]);
        assert_eq!(n("let s = CommandSpec { id, ..CommandSpec::default() };\nlet t = CommandSpec :: default ();"), [1, 1, 2]);
        assert!(n("CommandSpec::defaults(); CommandSpec::new(); fn default() -> CommandSpec {}").is_empty());
    }

    #[test]
    fn process_command_by_a_glob_or_an_alias() {
        assert_eq!(process_command("use std::process::*;\npub fn run() { Command::new(\"x\"); }\n"), [1]);
        assert_eq!(process_command("use std::process as p;\npub fn run() { p::Command::new(\"x\"); }\n"), [2]);
        assert_eq!(process_command("use tokio::process::{self as tp, Child};\nfn r() { tp::Command::new(\"x\"); }\n"), [2]);
        assert_eq!(process_command("use std::{process::{Child, Command}, io};\n"), [1]);
        // Not it: another module's alias, another crate's `Command`, a cast.
        assert!(process_command("use clap as p;\nfn r() { p::Command::new(\"x\"); let n = process as u32; }\nuse std::io::{self as sio};\n").is_empty());
    }

    #[test]
    /// A file opened by its options is a write only when the options say so: a read-only open and
    /// the import of the name are none, and one statement is one match however it is laid out.
    fn a_file_opened_to_write_is_a_write_and_one_opened_to_read_is_not() {
        assert!(write_opens("use std::fs::OpenOptions;\nlet f = OpenOptions::new().read(true).open(p)?;\n").is_empty());
        assert_eq!(write_opens("use std::fs::OpenOptions;\nlet f = OpenOptions::new()\n    .create( true )\n    .append(true)\n    .open(p)?;\n"), [2]);
        assert!(write_opens("let f = File::options().read(true).open(p)?; let g = other.write(true);").is_empty(), "the next statement's is not this one's");
        assert_eq!(write_opens("File::options().truncate(true).open(p)?;"), [1]);
        assert!(write_opens("MyOpenOptions::new().write(true);").is_empty());
        // The writes a path names: links and a truncation among them.
        for write in ["std::fs::hard_link(a, b)", "std::os::unix::fs::symlink(a, b)", "file.set_len(0)", "fs::soft_link(a, b)"] {
            assert_eq!(text(write, FILE_WRITES), [1], "{write}");
        }
        assert_eq!(fs_imports("use std::fs::{hard_link};"), [1]);
    }

    /// The crate's own `process` module is not the standard library's, and a nested group is read
    /// to its end.
    #[test]
    fn the_crates_own_process_module_is_not_std_and_a_nested_group_is_read() {
        for own in ["crate::process::Command::new()", "use crate::process::{Command, run};", "self::process::Command", "super::process::*"] {
            assert!(process_command(own).is_empty(), "{own}");
        }
        for std in [
            "std::process::Command::new(\"x\")",
            "tokio::process::Command",
            "use std::process::{io::{Read}, Command};",
            "use std::process::{\n    Stdio,\n    Command,\n};",
        ] {
            assert_eq!(process_command(std).len(), 1, "{std}");
        }
        assert!(process_command("use std::process::{io::{Command}, Stdio};").len() == 1, "nested, it is still named in the group");
        assert!(process_command("use std::process::{Stdio};\nfn f() { let c = Command { x: 1 }; }").is_empty(), "a `Command` after the group is not in it");
    }

    #[test]
    fn imported_writes_and_config_dirs_are_matched_at_the_import() {
        assert_eq!(fs_imports("use std::fs::{self, write};\npub fn w() { write(\"p\", b\"x\"); }\n"), [1]);
        assert_eq!(fs_imports("use std::fs::{\n    create_dir_all,\n    write,\n};\n"), [1, 1]);
        assert_eq!(fs_imports("use tokio::fs::*;\nuse std::fs as f;\nfn w() { f::remove_file(\"p\"); }\n"), [1, 3]);
        assert!(
            fs_imports("use std::fs::{self, File, read_to_string};\nfn w() { fs::write(\"p\", b\"\"); std::fs::copy(a, b); }\n").is_empty(),
            "the paths are FILE_WRITES' own"
        );
        assert_eq!(write_opens("std::fs::File::options().write(true).open(\"p\");"), [1]);
        assert_eq!(dirs_imports("#[cfg(test)]\nmod tests {\n use dirs::{config_dir};\n fn t() { config_dir(); }\n}\n"), [3]);
        assert!(dirs_imports("fn t() { dirs::config_dir(); }").is_empty(), "REAL_CONFIG_DIR's own");
    }

    /// What the strings scan counts: each literal with text (of any kind), on its line; not `""`,
    /// not one in an attribute, not a char literal, and none of a comment.
    #[test]
    fn string_literals_are_every_literal_with_text_outside_attributes() {
        let blanked = |s: &str| string_literals(&super::super::source::strip(s, false));
        assert_eq!(blanked("ui.label(\"Run\");\nui.label(format!(\"{} of {}\", a, b));\n"), [1, 2]);
        assert_eq!(blanked("let a = r#\"raw \"quoted\"\"#; let b = b\"bytes\"; let c = \"esc \\\" aped\";\n"), [1, 1, 1]);
        assert_eq!(blanked("let e = \"\"; let q = '\"'; let l: &'static str = x; // \"comment\"\n/* \"block\" */\n"), Vec::<usize>::new());
        let attributes = "#[expect(clippy::x, reason = \"why\")]\n#![doc = \"crate\"]\n#[serde(rename = \"k\", alias = [\"a\"])]\nfn f() { g(\"after\"); }\n";
        assert_eq!(blanked(attributes), [4], "an attribute's strings are never drawn; the code after it is read");
        assert_eq!(blanked("let a = [\"x\"]; let b = v[0]; m!(#x \"y\");\n\"multi\nline\"\n"), [1, 1, 2]);
        assert_eq!(string_literals("\"unclosed"), [1], "an unterminated literal is still one");
    }

    #[test]
    fn colours_are_made_by_their_types_or_the_macro() {
        let src = "let a = Color32::from_rgb(1, 2, 3);\nlet b = egui::Color32::RED; let c = ecolor::Rgba::WHITE;\nfn f(c: Color32) -> MyColor32::X {}\nlet d = hex_color!(\"#fff\");\nlet e = Hsva::new(0.0, 1.0, 1.0, 1.0);\n";
        assert_eq!(text(src, COLOURS), [1, 2, 2, 4, 5], "a type named as a type is no colour, nor another type's constant");
    }

    #[test]
    fn hostile_groups_are_linear() {
        for (unit, f) in
            [("process::{\n", process_command as fn(&str) -> Vec<usize>), ("fs::{write,\n", fs_imports), ("use std::process as p;\n", process_command)]
        {
            let text = unit.repeat(4 * 1024 * 1024 / unit.len());
            let start = std::time::Instant::now();
            let _ = f(&text);
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "{unit:?}: {:?}", start.elapsed());
        }
        let text = "CommandSpec { ".repeat(4 * 1024 * 1024 / 14);
        let start = std::time::Instant::now();
        assert_eq!(struct_literal(&text, "CommandSpec").len(), text.len() / 14);
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
    }
}
