//! Reading Rust source as the scans need it, as text and not as Rust (`syn` would cost seconds on a
//! big workspace): its comments and, where a pattern could match inside one, its string literals
//! taken out ([`strip`]), and the split between production code and its test modules
//! ([`test_modules`], [`regions`]).

/// `text` without its comments (line, block and doc; nested blocks too) and with each string
/// literal (normal, raw, byte) either kept (`keep_strings`) or blanked to `"____"` of the same
/// length, and each char literal made `'x'`. Newlines are all kept, so a line number in the result
/// is the line number in `text`. A lexer, not a parser: an unterminated comment or string takes the
/// rest of the file, which is what a compiler would refuse anyway.
pub fn strip(text: &str, keep_strings: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        i = step(&chars, i, keep_strings, &mut out);
    }
    out
}

/// Whether `c` can be in an identifier.
fn ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Lex the token at `i` into `out`; the index after it.
fn step(chars: &[char], i: usize, keep: bool, out: &mut String) -> usize {
    let at = |k: usize| chars.get(k).copied();
    let Some(c) = at(i) else { return i + 1 };
    let after_ident = i.checked_sub(1).and_then(at).is_some_and(ident);
    match c {
        '/' if at(i + 1) == Some('/') => line_comment(chars, i),
        '/' if at(i + 1) == Some('*') => block_comment(chars, i, out),
        '"' => string(chars, i + 1, None, keep, out),
        '\'' => char_literal(chars, i, out),
        'r' | 'b' | 'c' if !after_ident => match raw_start(chars, i) {
            Some((hashes, content)) => string(chars, content, Some(hashes), keep, out),
            None => {
                out.push(c);
                i + 1
            }
        },
        _ => {
            out.push(c);
            i + 1
        }
    }
}

/// Skip a `//` comment up to (not past) its newline.
fn line_comment(chars: &[char], i: usize) -> usize {
    chars.iter().skip(i).position(|c| *c == '\n').map_or(chars.len(), |n| i + n)
}

/// Skip a `/* */` comment, which nests, keeping its newlines; the index after it.
fn block_comment(chars: &[char], start: usize, out: &mut String) -> usize {
    let (mut i, mut depth) = (start, 0usize);
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('*') {
            depth += 1;
            i += 2;
        } else if c == '*' && next == Some('/') {
            depth = depth.saturating_sub(1);
            i += 2;
            if depth == 0 {
                break;
            }
        } else {
            if c == '\n' {
                out.push('\n');
            }
            i += 1;
        }
    }
    i
}

/// `r"`, `r#"`, `br"`, `br##"`, `cr#"`…: the number of `#`s and the index of the first content
/// char.
fn raw_start(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let mut j = i + usize::from(matches!(chars.get(i), Some('b' | 'c')));
    if chars.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let hashes = chars.iter().skip(j).take_while(|c| **c == '#').count();
    (chars.get(j + hashes) == Some(&'"')).then_some((hashes, j + hashes + 1))
}

/// A string literal whose content starts at `start`: a normal one honours `\` escapes
/// (`raw_hashes` `None`), a raw one is closed by `"` and that many `#`s. Its content is kept or
/// blanked; the index after it.
fn string(chars: &[char], start: usize, raw_hashes: Option<usize>, keep: bool, out: &mut String) -> usize {
    out.push('"');
    let hashes = raw_hashes.unwrap_or(0);
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        if c == '"' && closes(chars, i, hashes) {
            i += 1 + hashes;
            break;
        }
        let take = if c == '\\' && raw_hashes.is_none() { 2 } else { 1 };
        for d in chars.iter().skip(i).take(take) {
            out.push(if keep || *d == '\n' { *d } else { '_' });
        }
        i += take;
    }
    out.push('"');
    i
}

/// Whether the `"` at `i` is followed by `hashes` `#`s.
fn closes(chars: &[char], i: usize, hashes: usize) -> bool {
    chars.iter().skip(i + 1).take(hashes).filter(|c| **c == '#').count() == hashes
}

/// A `'`: a char literal (`'a'`, `'\n'`, `'\u{1F600}'`, `'"'`), made `'x'`, or a lifetime or label,
/// which is left alone.
fn char_literal(chars: &[char], i: usize, out: &mut String) -> usize {
    let at = |k: usize| chars.get(k).copied();
    let end = if at(i + 1) == Some('\\') {
        (i + 3..i + 14).find(|k| at(*k) == Some('\''))
    } else if at(i + 2) == Some('\'') && at(i + 1) != Some('\'') {
        Some(i + 2)
    } else {
        None
    };
    match end {
        Some(k) => {
            out.push_str("'x'");
            k + 1
        }
        None => {
            out.push('\'');
            i + 1
        }
    }
}

/// The test modules of `stripped` (a source from [`strip`] with its strings blanked), as `[start,
/// end)` char ranges: each item marked `#[cfg(test)]` (or by a `cfg` that names `test` other than
/// under `not(…)` or `any(…)`: `all(test, …)`) that is a `mod`, from that attribute to the
/// module's end: its `;` for `mod tests;`, its closing brace for an inline one. Everything else is
/// production code, before a test module or after it, and a `#[cfg(test)]` helper function marks
/// no module. One pass: each char is looked at a bounded number of times, so a hostile file is
/// linear.
pub fn test_modules(stripped: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = stripped.chars().collect();
    let at = |k: usize| chars.get(k).copied();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if at(i) == Some('#') && at(i + 1) == Some('[') {
            let (test, item) = attributes(&chars, i);
            match module_end(&chars, item).filter(|_| test) {
                Some(end) => {
                    out.push((i, end));
                    i = end;
                }
                None => i = item.max(i + 1),
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The outer attributes from `i` on (with the whitespace between them): whether one is a `cfg` of
/// `test`, and the index of the item they mark.
fn attributes(chars: &[char], mut i: usize) -> (bool, usize) {
    let mut test = false;
    loop {
        i = skip_space(chars, i);
        if chars.get(i) != Some(&'#') || chars.get(i + 1) != Some(&'[') {
            return (test, i);
        }
        let (mut depth, mut k) = (0usize, i + 1);
        while let Some(&c) = chars.get(k) {
            depth = match c {
                '[' => depth + 1,
                ']' => depth.saturating_sub(1),
                _ => depth,
            };
            k += 1;
            if depth == 0 {
                break;
            }
        }
        let body: String = chars.get(i + 2..k.saturating_sub(1).max(i + 2)).unwrap_or_default().iter().collect();
        test |= cfg_of_test(&body);
        i = k;
    }
}

/// The index of the first char at or after `i` that isn't whitespace.
fn skip_space(chars: &[char], i: usize) -> usize {
    chars.iter().skip(i).position(|c| !c.is_whitespace()).map_or(chars.len(), |n| i + n)
}

/// Whether an attribute's body (`cfg(all(test, unix))`) is a `cfg` that holds only under `test`:
/// it names `test` other than under a `not(…)` or an `any(…)`. `any(test, feature = "x")` is
/// compiled without `test` too, with the feature on: production code, and scanned as it.
fn cfg_of_test(body: &str) -> bool {
    let Some(predicate) = body.trim_start().strip_prefix("cfg").map(str::trim_start).and_then(|p| p.strip_prefix('(')) else { return false };
    // For each open paren, whether it is inside a `not(` or an `any(`.
    let mut negated = vec![false];
    let mut word = String::new();
    for c in predicate.chars().chain([' ']) {
        if ident(c) {
            word.push(c);
            continue;
        }
        let inside_not = negated.last().copied().unwrap_or(false);
        match c {
            '(' => negated.push(inside_not || word == "not" || word == "any"),
            _ if word == "test" && !inside_not && c != '=' && !c.is_whitespace() => return true,
            ')' => drop(negated.pop()),
            _ => {}
        }
        if !c.is_whitespace() || word.is_empty() {
            word.clear();
        }
    }
    false
}

/// The longest visibility's parentheses [`module_end`] looks through (`pub(in a::b::c)`).
const MAX_VISIBILITY: usize = 256;

/// When the item at `i` is a `mod` (with or without a visibility), the index after its end: its
/// `;`, or its closing brace (the end of the text when it never closes).
fn module_end(chars: &[char], mut i: usize) -> Option<usize> {
    let word = |k: usize, w: &str| {
        let n = w.chars().count();
        chars.get(k..k + n).is_some_and(|s| s.iter().copied().eq(w.chars())) && !chars.get(k + n).copied().is_some_and(ident)
    };
    if word(i, "pub") {
        i = skip_space(chars, i + 3);
        if chars.get(i) == Some(&'(') {
            // `pub(crate)`, `pub(in a::b)`: short, so a `pub(` that never closes costs no more.
            i = chars.iter().skip(i).take(MAX_VISIBILITY).position(|c| *c == ')').map(|n| i + n + 1)?;
        }
    }
    i = skip_space(chars, i);
    if !word(i, "mod") {
        return None;
    }
    i += 3;
    i = skip_space(chars, i);
    i += chars.iter().skip(i).take_while(|c| ident(**c)).count();
    i = skip_space(chars, i);
    match chars.get(i) {
        Some(';') => Some(i + 1),
        Some('{') => {
            let mut depth = 0usize;
            while let Some(&c) = chars.get(i) {
                i += 1;
                depth = match c {
                    '{' => depth + 1,
                    '}' => depth.saturating_sub(1),
                    _ => depth,
                };
                if depth == 0 {
                    break;
                }
            }
            Some(i)
        }
        _ => None,
    }
}

/// `stripped` as its production code and its test code (`modules` from [`test_modules`], by char
/// index, which [`strip`] keeps the same whether it keeps the strings or blanks them): each the
/// whole text with the other's chars made spaces and its newlines kept, so a line number in
/// either is the line number in the file.
pub fn regions(stripped: &str, modules: &[(usize, usize)]) -> (String, String) {
    let (mut production, mut tests) = (String::with_capacity(stripped.len()), String::with_capacity(stripped.len()));
    let mut next = modules.iter().peekable();
    for (k, c) in stripped.chars().enumerate() {
        while next.peek().is_some_and(|(_, end)| *end <= k) {
            next.next();
        }
        let in_test = next.peek().is_some_and(|(start, _)| *start <= k);
        let (keep, blank) = if in_test { (&mut tests, &mut production) } else { (&mut production, &mut tests) };
        keep.push(c);
        blank.push(if c == '\n' { '\n' } else { ' ' });
    }
    (production, tests)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_go_and_lines_stay() {
        let text = "a // x\n/// doc v[\"k\"]\n/* b /* nested */ c\n d */ e\n//! inner\nf";
        assert_eq!(strip(text, true), "a \n\n\n e\n\nf");
        assert_eq!(strip(text, true).lines().count(), text.lines().count());
        assert_eq!(strip("x /* never closed\n y", true), "x \n");
    }

    #[test]
    fn strings_are_kept_or_blanked_and_never_start_a_comment() {
        let text = "let a = \"// not a comment\"; let b = \"q\\\"uote\"; // gone";
        assert_eq!(strip(text, true), "let a = \"// not a comment\"; let b = \"q\\\"uote\"; ");
        assert_eq!(strip(text, false), "let a = \"________________\"; let b = \"_______\"; ");
        assert_eq!(strip("let s = \"a\nb\";", false), "let s = \"_\n_\";");
    }

    #[test]
    fn raw_and_byte_strings_and_char_literals_are_lexed() {
        assert_eq!(strip("r#\"a \" // b\"# x", false), "\"________\" x");
        assert_eq!(strip("br##\"a\"#b\"## y", false), "\"____\" y");
        assert_eq!(strip("r\"C:\\dir\" z", false), "\"______\" z");
        assert_eq!(strip("b\"ab\" z", false), "b\"__\" z");
        assert_eq!(strip("let q = '\"'; // c\nlet r = '\\'';\nlet u = '\\u{1F600}';", true), "let q = 'x'; \nlet r = 'x';\nlet u = 'x';");
        assert_eq!(strip("fn f<'a>(x: &'a str) -> &'static str { \"s\" }", true), "fn f<'a>(x: &'a str) -> &'static str { \"s\" }");
        assert_eq!(strip("for x in 0..1 { loop {} } er\"x\"", true), "for x in 0..1 { loop {} } er\"x\"", "`er` is not a raw prefix");
    }

    #[test]
    fn raw_c_strings_are_lexed_and_keep_the_chars_where_they_were() {
        assert_eq!(strip("let s = cr#\"a \" b\"#; f(); // x", false), "let s = \"_____\"; f(); ");
        assert_eq!(strip("let s = c\"a\\\"b\"; f();", false), "let s = c\"____\"; f();");
        assert_eq!(strip("let s = br#\"a \" b\"#; f();", false), "let s = \"_____\"; f();");
        // Kept or blanked, the text has the same chars at the same places outside the strings, so
        // the test modules found in the blanked text mark the kept one too.
        let text = "a(r#\"x\"#, cr\"y\", b'z', \"\\\"q\", '\\u{1F600}', br##\"w\"##); /* c */ // d\n#[cfg(test)] mod t { \"{\" }\nb();";
        let (kept, blank) = (strip(text, true), strip(text, false));
        assert_eq!(kept.chars().count(), blank.chars().count());
        let (production, tests) = regions(&kept, &test_modules(&blank));
        assert_eq!(
            (words(&production), words(&tests)),
            ("a(\"x\", \"y\", b'x', \"\\\"q\", 'x', \"w\"); b();".to_owned(), "#[cfg(test)] mod t { \"{\" }".to_owned())
        );
    }

    /// The text's words, single-spaced: what [`regions`] keeps of it, without the blanks.
    fn words(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `text` as its production code and test code, single-spaced, after checking each has the
    /// lines of `text`.
    fn split(text: &str) -> (String, String) {
        let (production, tests) = regions(text, &test_modules(text));
        assert_eq!((production.lines().count(), tests.lines().count()), (text.lines().count(), text.lines().count()), "{text}");
        (words(&production), words(&tests))
    }

    #[test]
    fn only_the_test_modules_are_test_code() {
        let text = "fn a() {}\n#[cfg(test)]\nfn helper() {}\nfn b() {}\n#[cfg(test)]\n#[allow(x)]\npub(crate) mod tests {\n}\n";
        assert_eq!(split(text), ("fn a() {} #[cfg(test)] fn helper() {} fn b() {}".to_owned(), "#[cfg(test)] #[allow(x)] pub(crate) mod tests { }".to_owned()));
        // A `mod tests;` at the top is one item: the production code after it is production.
        let (production, tests) = split("#[cfg(test)]\nmod tests;\n\npub fn run() { go(); }\n");
        assert_eq!((production.as_str(), tests.as_str()), ("pub fn run() { go(); }", "#[cfg(test)] mod tests;"));
        // An inline module mid-file ends at its own closing brace, nested braces and all.
        let (production, tests) = split("fn a() {}\n#[cfg(test)]\nmod tests {\n    fn t() { if x { y() } }\n}\npub fn run() { go(); }\n");
        assert_eq!((production.as_str(), tests.as_str()), ("fn a() {} pub fn run() { go(); }", "#[cfg(test)] mod tests { fn t() { if x { y() } } }"));
        // Inside another module, and one that never closes takes the rest.
        let (production, tests) = split("mod outer {\n    #[cfg(test)] mod t { a(); }\n    fn b() {}\n}\n#[cfg(test)] pub mod u {\n c();");
        assert_eq!((production.as_str(), tests.as_str()), ("mod outer { fn b() {} }", "#[cfg(test)] mod t { a(); } #[cfg(test)] pub mod u { c();"));
        assert_eq!(split("fn a() {}\n"), ("fn a() {}".to_owned(), String::new()));
        assert_eq!(split("#[cfg(test)]\n").1, "", "an attribute with no item marks nothing");
    }

    #[test]
    fn a_cfg_that_needs_test_marks_a_test_module_and_one_that_excludes_it_does_not() {
        for test in ["#[cfg(all(test, unix))]", "#[cfg( test )]", "#[cfg(all(not(windows), test))]"] {
            assert_eq!(split(&format!("{test} mod m {{ a(); }}\nb();")), ("b();".to_owned(), format!("{test} mod m {{ a(); }}")), "{test}");
        }
        for production in [
            "#[cfg(not(test))]",
            "#[cfg(any(test, feature = \"_\"))]",
            "#[cfg(all(unix, any(test, debug_assertions)))]",
            "#[cfg(any(unix, not(test)))]",
            "#[cfg_attr(test, allow(x))]",
            "#[cfg(tests)]",
            "#[cfg(testing)]",
        ] {
            assert_eq!(split(&format!("{production} mod m {{ a(); }}")).1, "", "{production}");
        }
    }

    #[test]
    fn a_hostile_file_splits_in_linear_time() {
        // Each of these made every `#[cfg(test)]`, or every `#[`, look at the rest of the file.
        for unit in ["#[cfg(test)]\n", "#[cfg(test)] fn f() {}\n", "#[", "#[cfg(test)] mod m {\n", "#[cfg(test)] pub(\n"] {
            let text = unit.repeat(4 * 1024 * 1024 / unit.len());
            let start = std::time::Instant::now();
            let modules = test_modules(&text);
            let (production, tests) = regions(&text, &modules);
            assert_eq!(production.len() + tests.len(), 2 * text.len());
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "{unit:?}: {:?}", start.elapsed());
        }
    }
}
