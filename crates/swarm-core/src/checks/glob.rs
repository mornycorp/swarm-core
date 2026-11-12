//! The globs the checks name files by (a scan's files, a manifest's `members`): `/`-separated, `*`
//! any run of characters inside one segment, `?` one character, and a `**` segment any number of
//! whole segments (none too). Nothing else is special, and a pattern never fails: it matches or not.

/// Whether `path` (relative, `/`-separated) matches `pattern`.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.trim_start_matches("./").split('/').filter(|s| !s.is_empty()).collect();
    let path: Vec<&str> = path.trim_start_matches("./").split('/').filter(|s| !s.is_empty()).collect();
    segments(&pattern, &path)
}

/// Whether any of `patterns` matches `path`.
pub fn any(patterns: &[&str], path: &str) -> bool {
    patterns.iter().any(|p| matches(p, path))
}

/// `pattern`'s segments against `path`'s, by dynamic programming: `row[j]` says the pattern so far
/// matches the first `j` path segments, so `**` can't make it run exponentially long.
fn segments(pattern: &[&str], path: &[&str]) -> bool {
    let mut row = vec![false; path.len() + 1];
    if let Some(first) = row.first_mut() {
        *first = true;
    }
    for seg in pattern {
        let mut next = vec![false; path.len() + 1];
        for j in 0..=path.len() {
            let before = row.get(j).copied().unwrap_or(false);
            let skipped = j.checked_sub(1).is_some_and(|k| next.get(k).copied().unwrap_or(false));
            let stepped = j.checked_sub(1).is_some_and(|k| row.get(k).copied().unwrap_or(false) && path.get(k).is_some_and(|p| segment(seg, p)));
            let here = if *seg == "**" { before || skipped } else { stepped };
            if let Some(slot) = next.get_mut(j) {
                *slot = here;
            }
        }
        row = next;
    }
    row.last().copied().unwrap_or(false)
}

/// One segment against one name: `*` and `?` as above.
fn segment(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    // The classic two-pointer match: remember the last `*` and let it take one more character.
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some(pi);
                mark = ni;
                pi += 1;
            }
            Some(c) if *c == '?' || Some(c) == n.get(ni) => {
                pi += 1;
                ni += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    mark += 1;
                    ni = mark;
                }
                None => return false,
            },
        }
    }
    p.get(pi..).is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_stay_inside_a_segment_and_double_stars_cross_them() {
        assert!(matches("crates/*/src/lib.rs", "crates/model/src/lib.rs"));
        assert!(!matches("crates/*/src/lib.rs", "crates/a/b/src/lib.rs"));
        assert!(matches("crates/ui-egui/src/**", "crates/ui-egui/src/a/b/c.rs"));
        assert!(matches("crates/ui-egui/src/**", "crates/ui-egui/src"), "`**` matches no segment too");
        assert!(matches("**/tests/**/*.rs", "crates/x/tests/common/mod.rs"));
        assert!(matches("**/tests/**/*.rs", "tests/a.rs"));
        assert!(!matches("**/tests/**/*.rs", "crates/x/src/a.rs"));
        assert!(matches("crates/*", "crates/model") && !matches("crates/*", "crates"));
        assert!(matches("src/*_tests.rs", "src/state_tests.rs") && !matches("src/*_tests.rs", "src/state.rs"));
        assert!(matches("a?c", "abc") && !matches("a?c", "ac"));
        assert!(matches("./xtask", "xtask") && matches("xtask", "./xtask"));
    }

    #[test]
    fn hostile_patterns_just_match_or_not() {
        let deep = "a/".repeat(200) + "b";
        assert!(matches(&format!("{}b", "**/".repeat(50)), &deep));
        assert!(!matches("***x", "yyy") && matches("***", "yyy"));
        assert!(!matches("", "a") && matches("", ""));
        assert!(any(&["a", "b/*"], "b/c") && !any(&[], "x"));
    }
}
