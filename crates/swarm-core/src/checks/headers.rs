//! The lint headers of a crate root (`#![forbid(unsafe_code)]`, the deny list, the code-health
//! limits), read as the attributes at the top of `src/lib.rs`, `src/main.rs` and the other roots,
//! whatever their layout: one attribute per line or several, wrapped, with comments between.

use toml::Value;

use super::manifest::{self, Member};
use super::{Finding, source};

/// The code-health lints every root warns on (the thresholds are in `clippy.toml`) and allows under
/// `cfg(test)`.
pub const LIMITS: [&str; 2] = ["clippy::cognitive_complexity", "clippy::too_many_lines"];

/// What a crate root says of `unsafe`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Safety {
    /// `#![forbid(unsafe_code)]`: every root but an allowed library's.
    Forbidden,
    /// The library root of a package `Layers::unsafe_crates` lists: [`super::layers::UNSAFE_LINES`]
    /// instead, and an `allow(unsafe_code)` is no cancelled deny.
    Allowed,
}

/// `member`'s library root (`[lib] path`, else `src/lib.rs`), from the root, when it is one of
/// `files` (every file of the workspace, sorted); `None` for a binary-only package.
pub fn library(member: &Member, files: &[String]) -> Option<String> {
    let declared = member.table.get("lib").and_then(Value::as_table).and_then(|t| t.get("path")).and_then(Value::as_str);
    manifest::join(&member.dir, declared.unwrap_or("src/lib.rs")).filter(|lib| files.binary_search(lib).is_ok())
}

/// The crate roots of `member` that exist (`files`: every file of the workspace, sorted): its
/// library (`[lib] path`, else `src/lib.rs`), its binaries (`[[bin]]` paths, `src/main.rs`,
/// `src/bin/*.rs`, `src/bin/*/main.rs`).
pub fn roots(member: &Member, files: &[String]) -> Vec<String> {
    // A declared path as the listing names it: `./src/core.rs`, `src\\core.rs`, `../x/lib.rs`.
    let at = |p: &str| manifest::join(&member.dir, p);
    let declared = |key: &str| -> Vec<String> {
        let tables = member.table.get(key);
        let one = tables.and_then(Value::as_table).into_iter();
        let many = tables.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_table);
        one.chain(many).filter_map(|t| t.get("path").and_then(Value::as_str)).filter_map(&at).collect()
    };
    let mut found: Vec<String> = [at("src/lib.rs"), at("src/main.rs")].into_iter().flatten().chain(declared("lib")).chain(declared("bin")).collect();
    let bins = format!("{}/", at("src/bin").unwrap_or_default());
    found.extend(
        files
            .iter()
            .filter(|f| f.strip_prefix(&bins).is_some_and(|rest| (!rest.contains('/') && rest.ends_with(".rs")) || rest.ends_with("/main.rs")))
            .cloned(),
    );
    found.sort();
    found.dedup();
    found.retain(|f| files.binary_search(f).is_ok());
    found
}

/// The inner attributes at the top of `stripped` (a source without comments or string contents),
/// each as its path tokens in order: `#![deny(clippy::panic)]` is `["deny", "clippy::panic"]`.
fn inner_attributes(stripped: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut rest = stripped.trim_start();
    while let Some(body) = rest.strip_prefix("#![") {
        let (mut depth, mut end) = (1usize, body.len());
        for (i, c) in body.char_indices() {
            depth = match c {
                '[' => depth.saturating_add(1),
                ']' => depth.saturating_sub(1),
                _ => depth,
            };
            if depth == 0 {
                end = i;
                break;
            }
        }
        let attr = body.get(..end).unwrap_or("");
        out.push(attr.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).filter(|t| !t.is_empty()).map(str::to_owned).collect());
        rest = body.get(end + 1..).unwrap_or("").trim_start();
    }
    out
}

/// What the header of root `shown` (its `text`) lacks, given the lints it must deny and what it
/// says of `unsafe`.
pub fn check(shown: &str, text: &str, deny: &[&str], safety: Safety) -> Vec<Finding> {
    let attrs = inner_attributes(&source::strip(text, false));
    // The lints named by an attribute whose first token is one of `levels` (`deny(a, b)`).
    let named = |levels: &[&str]| -> Vec<&str> {
        attrs.iter().filter(|a| a.first().is_some_and(|l| levels.contains(&l.as_str()))).flat_map(|a| a.iter().skip(1).map(String::as_str)).collect()
    };
    let in_test = |a: &Vec<String>| {
        a.first().map(String::as_str) == Some("cfg_attr") && a.get(1).map(String::as_str) == Some("test") && a.get(2).map(String::as_str) == Some("allow")
    };
    let allowed_in_test: Vec<&str> = attrs.iter().filter(|a| in_test(a)).flat_map(|a| a.iter().skip(3).map(String::as_str)).collect();
    let missing = |have: Vec<&str>, want: &[&str]| -> Vec<String> { want.iter().filter(|w| !have.contains(w)).map(|w| (*w).to_owned()).collect() };
    let mut out = Vec::new();
    let mut lacks = |what: String, fix: String| out.push(Finding::new(shown, Some(1), "crate-root-header", format!("the crate root lacks {what}"), fix));
    match safety {
        Safety::Forbidden if !named(&["forbid"]).contains(&"unsafe_code") => {
            lacks("`#![forbid(unsafe_code)]`".to_owned(), "add it as the root's first attribute (crates/swarm-core/src/lib.rs is the model)".to_owned());
        }
        Safety::Forbidden => {}
        Safety::Allowed => {
            let unsafe_lines =
                [(&["deny", "forbid"][..], "unsafe_op_in_unsafe_fn", 0), (&["warn", "deny", "forbid"][..], "clippy::undocumented_unsafe_blocks", 1)];
            for (levels, lint, k) in unsafe_lines {
                if !named(levels).contains(&lint) {
                    let line = super::layers::UNSAFE_LINES.get(k).copied().unwrap_or_default();
                    lacks(format!("`{line}`, which a library allowed `unsafe` carries"), format!("add `{line}`"));
                }
            }
        }
    }
    let no_deny = missing(named(&["deny", "forbid"]), deny);
    if !no_deny.is_empty() {
        lacks(format!("a deny of {}", no_deny.join(", ")), format!("add `#![deny({})]`", no_deny.join(", ")));
    }
    let no_limits = missing(named(&["warn", "deny", "forbid"]), &LIMITS);
    if !no_limits.is_empty() {
        lacks(format!("the code-health limits ({})", no_limits.join(", ")), format!("add `#![warn({})]`", no_limits.join(", ")));
    }
    let no_allow = missing(allowed_in_test, &LIMITS);
    if !no_allow.is_empty() {
        let fix = format!("add `#![cfg_attr(test, allow({}))]`: the limits are for production code", LIMITS.join(", "));
        lacks(format!("the limits' allowance under test ({})", no_allow.join(", ")), fix);
    }
    let undone: Vec<&str> = cancelled(&attrs, deny).into_iter().filter(|lint| !(safety == Safety::Allowed && *lint == "unsafe_code")).collect();
    if !undone.is_empty() {
        let problem = format!("the crate root allows {}, which it must deny: a later `allow`, `warn` or `expect` undoes the deny", undone.join(", "));
        let fix = "remove it from the header; a reasoned exception goes on the one item (`#[expect(…, reason = \"…\")]`)";
        out.push(Finding::new(shown, Some(1), "crate-root-header", problem, fix));
    }
    out
}

/// The lints of `deny` (and `unsafe_code`) that the header leaves lowered: named under `allow`,
/// `warn` or `expect`, in a plain attribute or a `cfg_attr(…)`, or by the group that holds them
/// (`clippy::restriction`), with no `deny` or `forbid` of the same lint after it. The compiler
/// takes the last level a lint is given, so an `allow` before its `deny` undoes nothing.
fn cancelled<'a>(attrs: &'a [Vec<String>], deny: &[&str]) -> Vec<&'a str> {
    let held =
        |token: &str| deny.contains(&token) || token == "unsafe_code" || (token == "clippy::restriction" && deny.iter().any(|d| d.starts_with("clippy::")));
    // Each lint's last level in the header: whether it is lowered there.
    let mut last: Vec<(&str, bool)> = Vec::new();
    for attr in attrs {
        let mut lowered = None;
        for token in attr.iter().map(String::as_str) {
            match (token, lowered) {
                ("allow" | "warn" | "expect", _) => lowered = Some(true),
                ("deny" | "forbid", _) => lowered = Some(false),
                (_, Some(lowers)) if held(token) => {
                    last.retain(|(lint, _)| *lint != token);
                    last.push((token, lowers));
                }
                _ => {}
            }
        }
    }
    let mut out: Vec<&str> = last.into_iter().filter(|(_, lowered)| *lowered).map(|(lint, _)| lint).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last level wins, as it does for the compiler: an `allow` before the `deny` undoes
    /// nothing, and one after it does.
    #[test]
    fn an_allow_before_its_deny_cancels_nothing_and_one_after_does() {
        let attr = |level: &str, lint: &str| vec![level.to_owned(), lint.to_owned()];
        let deny = ["clippy::unwrap_used", "clippy::panic"];
        let before = [attr("allow", "clippy::unwrap_used"), attr("deny", "clippy::unwrap_used")];
        assert!(cancelled(&before, &deny).is_empty());
        let after = [attr("deny", "clippy::unwrap_used"), attr("allow", "clippy::unwrap_used")];
        assert_eq!(cancelled(&after, &deny), ["clippy::unwrap_used"]);
        let group = [attr("deny", "clippy::panic"), attr("warn", "clippy::restriction"), attr("forbid", "unsafe_code")];
        assert_eq!(cancelled(&group, &deny), ["clippy::restriction"]);
        let back = [attr("allow", "unsafe_code"), attr("forbid", "unsafe_code")];
        assert!(cancelled(&back, &deny).is_empty());
    }

    const DENY: &[&str] = &["clippy::unwrap_used", "clippy::panic"];
    const GOOD: &str = "//! docs\n#![forbid(unsafe_code)]\n#![deny(clippy::unwrap_used, clippy::panic)]\n#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]\n\
                        #![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]\n\nfn main() {}\n";

    #[test]
    fn a_complete_header_passes_in_any_layout() {
        assert!(check("a.rs", GOOD, DENY, Safety::Forbidden).is_empty());
        let wrapped = "#![forbid(unsafe_code)]\n// c\n#![deny(\n    clippy::unwrap_used, // x\n    clippy::panic,\n)]\n#![forbid(clippy::cognitive_complexity)]\n#![warn(clippy::too_many_lines)]\n\
                       #![cfg_attr(test, allow(clippy::cognitive_complexity))]\n#![cfg_attr(test, allow(clippy::too_many_lines))]\n";
        assert!(check("a.rs", wrapped, DENY, Safety::Forbidden).is_empty(), "{:?}", check("a.rs", wrapped, DENY, Safety::Forbidden));
    }

    #[test]
    fn each_missing_part_is_a_finding_that_names_the_fix() {
        for (without, what) in [
            ("#![forbid(unsafe_code)]\n", "forbid(unsafe_code)"),
            ("#![deny(clippy::unwrap_used, clippy::panic)]\n", "a deny of clippy::unwrap_used, clippy::panic"),
            (
                "#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]\n",
                "the code-health limits (clippy::cognitive_complexity, clippy::too_many_lines)",
            ),
            ("#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]\n", "allowance under test"),
        ] {
            let found = check("src/lib.rs", &GOOD.replace(without, ""), DENY, Safety::Forbidden);
            assert_eq!(found.len(), 1, "{what}: {found:?}");
            assert!(
                found[0].problem.contains(what) && found[0].file == "src/lib.rs" && found[0].line == Some(1) && !found[0].fix.is_empty(),
                "{what}: {found:?}"
            );
        }
        assert_eq!(check("a.rs", "fn main() {}", DENY, Safety::Forbidden).len(), 4);
        // An attribute after the first item, or inside a module, is not the root's header.
        assert_eq!(check("a.rs", &format!("fn x() {{}}\n{GOOD}"), DENY, Safety::Forbidden).len(), 4);
        // A comment or a string is not an attribute.
        assert_eq!(check("a.rs", "// #![forbid(unsafe_code)]\nconst S: &str = \"#![forbid(unsafe_code)]\";\n", DENY, Safety::Forbidden).len(), 4);
    }

    #[test]
    fn the_roots_are_the_library_and_the_binaries() {
        let table: toml::Table = "[lib]\npath = \"core/lib.rs\"\n[[bin]]\nname = \"x\"\npath = \"fake/x.rs\"\n".parse().unwrap();
        let member = Member { name: "a".into(), dir: "crates/a".into(), manifest: String::new(), text: String::new(), table };
        let mut files: Vec<String> =
            ["src/lib.rs", "src/main.rs", "src/bin/one.rs", "src/bin/two/main.rs", "src/bin/two/helper.rs", "core/lib.rs", "fake/x.rs", "src/other.rs"]
                .iter()
                .map(|f| format!("crates/a/{f}"))
                .collect();
        files.push("crates/a/src/bin/three.txt".to_owned());
        files.sort();
        let found = roots(&member, &files);
        let want: Vec<String> = ["core/lib.rs", "fake/x.rs", "src/bin/one.rs", "src/bin/two/main.rs", "src/lib.rs", "src/main.rs"]
            .iter()
            .map(|f| format!("crates/a/{f}"))
            .collect();
        assert_eq!(found, want);
    }

    #[test]
    fn a_declared_root_path_is_normalised_before_it_is_looked_for() {
        for path in ["./src/core.rs", "src\\\\core.rs", "src/./x/../core.rs"] {
            let table: toml::Table = format!("[lib]\npath = \"{path}\"\n").parse().unwrap();
            let member = Member { name: "a".into(), dir: "crates/a".into(), manifest: String::new(), text: String::new(), table };
            assert_eq!(roots(&member, &["crates/a/src/core.rs".to_owned()]), ["crates/a/src/core.rs"], "{path}");
        }
        let table: toml::Table = "[lib]\npath = \"../../../outside.rs\"\n".parse().unwrap();
        let member = Member { name: "a".into(), dir: "crates/a".into(), manifest: String::new(), text: String::new(), table };
        assert!(roots(&member, &["crates/a/src/core.rs".to_owned()]).is_empty());
    }

    #[test]
    fn a_later_allow_of_a_denied_lint_undoes_the_header() {
        for (extra, named) in [
            ("#![allow(clippy::unwrap_used, clippy::panic)]\n", "clippy::panic, clippy::unwrap_used"),
            ("#![cfg_attr(not(test), allow(clippy::panic))]\n", "clippy::panic"),
            ("#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::unwrap_used))]\n", "clippy::unwrap_used"),
            ("#![warn(clippy::panic)]\n", "clippy::panic"),
            ("#![expect(unsafe_code)]\n", "unsafe_code"),
            ("#![allow(clippy::restriction)]\n", "clippy::restriction"),
        ] {
            let found = check("src/lib.rs", &GOOD.replace("\nfn main", &format!("{extra}fn main")), DENY, Safety::Forbidden);
            assert_eq!(found.len(), 1, "{extra}: {found:?}");
            assert!(
                found[0].problem.starts_with(&format!("the crate root allows {named}, which it must deny")) && found[0].rule == "crate-root-header",
                "{found:?}"
            );
        }
        // Allowing another lint, or denying one again, is no undoing.
        assert!(check("a.rs", &GOOD.replace("\nfn main", "#![allow(dead_code)]\n#![deny(clippy::panic)]\nfn main"), DENY, Safety::Forbidden).is_empty());
    }

    #[test]
    fn a_library_allowed_unsafe_carries_the_unsafe_lines_instead_of_the_forbid() {
        let allowed = GOOD.replace("#![forbid(unsafe_code)]\n", "#![deny(unsafe_op_in_unsafe_fn)]\n#![warn(clippy::undocumented_unsafe_blocks)]\n");
        assert!(check("src/lib.rs", &allowed, DENY, Safety::Allowed).is_empty(), "{:?}", check("src/lib.rs", &allowed, DENY, Safety::Allowed));
        // Forbidden still wants the forbid; allowed doesn't, and may say `allow(unsafe_code)`.
        assert_eq!(check("src/lib.rs", &allowed, DENY, Safety::Forbidden).len(), 1);
        let with_allow = allowed.replace("\nfn main", "#![allow(unsafe_code)]\nfn main");
        assert!(check("src/lib.rs", &with_allow, DENY, Safety::Allowed).is_empty());
        // The forbid alone is not enough for an allowed root: each unsafe line is its own finding.
        let found = check("src/lib.rs", GOOD, DENY, Safety::Allowed);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].problem.contains("#![deny(unsafe_op_in_unsafe_fn)]") && found[1].problem.contains("#![warn(clippy::undocumented_unsafe_blocks)]"));
        // A deny list still can't be lowered there.
        let lowered = allowed.replace("\nfn main", "#![allow(clippy::panic)]\nfn main");
        assert!(check("src/lib.rs", &lowered, DENY, Safety::Allowed).iter().any(|f| f.problem.contains("clippy::panic")));
    }
}
