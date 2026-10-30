//! What an issue **needs**: the issues its description names on a `Needs`
//! line, such as `Needs S4 (#267).` or `**Needs:** #265 and #266`. Only the line's first clause
//! counts, up to a `;` or the sentence's end: in `Needs S4 (#267); S5 (#268) supplies fly-to.` only
//! #267 is needed. `Depends on` and `Blocked by` read the same.
//!
//! - **Another project's issue** is a need when the line names it in full,
//!   `owner/repo#n` (`cmaughan/swarm-core#4`): a consumer's change that waits on
//!   Swarm Core says so. A short name (`ns#381`) is ambiguous and not a need.
//!   The repository's own name (`example/app-swarm#300` in its own issue) is its own issue.
//!
//! `cargo xtask prwatch` watches what an agent's claimed issues need and says when one closes or
//! gets a pull request; `cargo xtask claim` notes the needs still open. The parsing and the
//! comparing are here; [`Source`] fetches.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::Project;
use crate::claim::Pr;
use crate::github;

/// Most needs read from one description.
pub const MAX_NEEDS: usize = 32;
/// Longest line read, in bytes: a hostile description's longer lines are cut.
const MAX_LINE: usize = 2_000;
/// Longest repository name read before a `#`.
const MAX_PATH: usize = 200;

/// A needed issue: this repository's (`project` `None`), or another's by its `owner/name`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ref {
    pub project: Option<String>,
    pub iid: u64,
}

impl Ref {
    /// This project's issue `iid`.
    pub fn here(iid: u64) -> Self {
        Ref { project: None, iid }
    }

    /// `#n`, or `owner/repo#n`.
    pub fn label(&self) -> String {
        match &self.project {
            Some(p) => format!("{p}#{}", self.iid),
            None => format!("#{}", self.iid),
        }
    }

    /// A pull request of the same repository: `#m`, or `owner/repo#m`.
    pub fn pr_label(&self, pr: u64) -> String {
        match &self.project {
            Some(p) => format!("{p}#{pr}"),
            None => format!("#{pr}"),
        }
    }
}

/// The issues `description` needs, as written: another project's with its path.
pub fn parse(description: &str) -> BTreeSet<Ref> {
    description.lines().filter_map(clause).flat_map(refs).take(MAX_NEEDS).collect()
}

/// The issues `description` needs, read by `project`: its own path is its own issue.
pub fn parse_for(project: &Project, description: &str) -> BTreeSet<Ref> {
    parse(description).into_iter().map(|r| own(project, r)).collect()
}

/// `r`, with `project`'s own repository read as this one.
fn own(project: &Project, r: Ref) -> Ref {
    match r.project {
        Some(p) if p.eq_ignore_ascii_case(project.github.repo) => Ref::here(r.iid),
        p => Ref { project: p, iid: r.iid },
    }
}

/// A `Needs` line's first clause, after the word: `None` for any other line.
fn clause(line: &str) -> Option<&str> {
    let cut = (0..=line.len().min(MAX_LINE)).rev().find(|&k| line.is_char_boundary(k)).unwrap_or(0);
    let line = line.get(..cut)?;
    let bare = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '*' | '>' | '_'));
    let lower = bare.to_ascii_lowercase();
    let word = ["needs", "depends on", "blocked by"].into_iter().find(|w| lower.starts_with(w))?;
    let rest = bare.get(word.len()..)?;
    let end = rest
        .char_indices()
        .find(|&(at, c)| c == ';' || (c == '.' && rest.get(at + 1..).is_none_or(|r| r.is_empty() || r.starts_with(' '))))
        .map_or(rest.len(), |(at, _)| at);
    rest.get(..end)
}

/// A character of a GitHub repository's `owner/name`.
fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.')
}

/// Is `path` a whole repository name (`owner/repo`)?
fn full_path(path: &str) -> bool {
    path.len() <= MAX_PATH && !path.ends_with('.') && github::check_repo(path).is_ok()
}

/// The project path written just before a `#` at the end of `before`: `None` when there is none
/// (`#5`), `Err` when what is there is no full path (`ns#5`, `a#8`, `&#5`): not a need.
fn project_before(before: &str) -> Result<Option<String>, ()> {
    let Some(last) = before.chars().next_back() else { return Ok(None) };
    if !(path_char(last) || last.is_alphanumeric() || last == '&') {
        return Ok(None);
    }
    let start = before.char_indices().rev().take_while(|&(_, c)| path_char(c)).last().map_or(before.len(), |(at, _)| at);
    let path = before.get(start..).unwrap_or_default();
    let clean_start = before.get(..start).and_then(|s| s.chars().next_back()).is_none_or(|c| !c.is_alphanumeric());
    if full_path(path) && clean_start { Ok(Some(path.to_owned())) } else { Err(()) }
}

/// The issue references in `text`: `#n` (this repository's) and `owner/repo#n` (another's).
fn refs(text: &str) -> Vec<Ref> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices('#') {
        let Ok(project) = project_before(text.get(..at).unwrap_or_default()) else { continue };
        let digits: String = text.get(at + 1..).unwrap_or_default().chars().take_while(char::is_ascii_digit).collect();
        if let Ok(iid) = digits.parse::<u64>()
            && iid > 0
        {
            out.push(Ref { project, iid });
        }
    }
    out
}

/// An issue that one or more claimed issues need, as a poll saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    /// Another repository's `owner/name`; `None`: this one's.
    pub project: Option<String>,
    pub iid: u64,
    pub title: String,
    /// `open` or `closed`.
    pub state: String,
    /// The claimed issues that need it.
    pub by: BTreeSet<u64>,
    /// An open pull request working on it, if any (in its own repository).
    pub mr: Option<u64>,
}

impl Need {
    /// What it is: its project and number.
    pub fn at(&self) -> Ref {
        Ref { project: self.project.clone(), iid: self.iid }
    }

    /// Its status in one line, for `prwatch --once` and `claim`.
    pub fn status(&self) -> String {
        let at = self.at();
        let held = self.mr.map_or_else(|| "no pull request yet".to_owned(), |m| format!("PR {}", at.pr_label(m)));
        format!("{} {} (needed by {}), {held} · {}", at.label(), self.state, issues(&self.by), self.title)
    }

    pub fn to_json(&self) -> Value {
        json!({"project": self.project, "iid": self.iid, "title": self.title, "state": self.state, "by": self.by, "mr": self.mr})
    }

    /// A need as [`Need::to_json`] saved it; one saved before cross-project needs is this
    /// project's.
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Need {
            project: v.get("project").and_then(Value::as_str).map(str::to_owned),
            iid: v.get("iid")?.as_u64()?,
            title: v.get("title").and_then(Value::as_str).unwrap_or_default().to_owned(),
            state: v.get("state").and_then(Value::as_str).unwrap_or_default().to_owned(),
            by: v.get("by").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default(),
            mr: v.get("mr").and_then(Value::as_u64),
        })
    }
}

/// `#1, #2`.
fn issues(set: &BTreeSet<u64>) -> String {
    set.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ")
}

/// What `claimed` (`project`'s open issues, in [`github`]'s shape) need, each with the claimed
/// issues needing it. A claimed issue is never its own or another claimed issue's need: those are
/// the agent's own.
pub fn wanted(project: &Project, claimed: &[Value]) -> BTreeMap<Ref, BTreeSet<u64>> {
    let mine: BTreeSet<u64> = github::numbers(claimed).into_iter().collect();
    let mut out: BTreeMap<Ref, BTreeSet<u64>> = BTreeMap::new();
    for i in claimed {
        let Some(iid) = i.get("number").and_then(Value::as_u64) else { continue };
        for n in parse_for(project, i.get("body").and_then(Value::as_str).unwrap_or_default()) {
            if n.project.is_some() || !mine.contains(&n.iid) {
                out.entry(n).or_default().insert(iid);
            }
        }
    }
    out
}

/// The projects `wanted` names: this one is `None`.
pub fn projects(wanted: &BTreeMap<Ref, BTreeSet<u64>>) -> BTreeSet<Option<String>> {
    wanted.keys().map(|r| r.project.clone()).collect()
}

/// The issue numbers `wanted` needs of `project` (`None`: this one).
pub fn iids_of(wanted: &BTreeMap<Ref, BTreeSet<u64>>, project: Option<&str>) -> Vec<u64> {
    wanted.keys().filter(|r| r.project.as_deref() == project).map(|r| r.iid).collect()
}

/// The needs in `wanted` of `project` (`None`: this one), with each one's title and state from
/// `found` (that repository's issues, any state) and its pull request from `open_prs` (that
/// repository's open ones whose branch, title or body is for it). A need GitHub didn't list
/// (deleted, or a repository it can't see) is left out.
pub fn resolve(project: Option<&str>, wanted: &BTreeMap<Ref, BTreeSet<u64>>, found: &[Value], open_prs: &[Pr]) -> BTreeMap<Ref, Need> {
    found
        .iter()
        .filter_map(|i| {
            let iid = i.get("number")?.as_u64()?;
            let at = Ref { project: project.map(str::to_owned), iid };
            let by = wanted.get(&at)?.clone();
            let title = i.get("title").and_then(Value::as_str).unwrap_or_default().chars().take(80).collect();
            let state = i.get("state").and_then(Value::as_str).unwrap_or("unknown").to_owned();
            let mr = open_prs.iter().filter(|m| m.is_for(iid)).map(|m| m.number).min();
            Some((at.clone(), Need { project: at.project, iid, title, state, by, mr }))
        })
        .collect()
}

/// What changed between two polls, as lines: a need that closed (what waited on it can start),
/// and one that got a pull request (someone took it).
pub fn changes(before: &BTreeMap<Ref, Need>, now: &BTreeMap<Ref, Need>) -> Vec<String> {
    let mut lines = Vec::new();
    for (at, n) in now {
        let Some(b) = before.get(at) else { continue };
        if b.state == "open" && n.state == "closed" {
            lines.push(format!("{} closed (needed by {}): start what waited on it · {}", at.label(), issues(&n.by), n.title));
        } else if n.state == "open"
            && let (None, Some(m)) = (b.mr, n.mr)
        {
            lines.push(format!("{} (needed by {}) taken: PR {} · {}", at.label(), issues(&n.by), at.pr_label(m), n.title));
        }
    }
    lines
}

/// Where the needs' issues and pull requests are read: GitHub ([`GitHub`]), or a test's fake.
pub trait Source {
    /// The issues numbered `numbers` of this repository (`of` `None`) or another, any state.
    fn issues(&self, of: Option<&str>, numbers: &[u64]) -> Result<Vec<Value>, String>;
    /// The open pull requests of another repository.
    fn open_prs(&self, of: &str) -> Result<Vec<Value>, String>;
}

/// [`Source`] on GitHub, for `project`.
pub struct GitHub<'a>(pub &'a Project);

impl Source for GitHub<'_> {
    fn issues(&self, of: Option<&str>, numbers: &[u64]) -> Result<Vec<Value>, String> {
        github::issues(self.0, of, numbers)
    }
    fn open_prs(&self, of: &str) -> Result<Vec<Value>, String> {
        github::open_prs(self.0, Some(of))
    }
}

/// The needs in `wanted` with their state and pull request, from each repository they name: for
/// each, its issues and its open pull requests (`open_here`: this repository's, which the caller
/// has already). A repository that can't be read is said, and its needs left out: a note, never a
/// failure.
pub fn fetch(project: &Project, wanted: &BTreeMap<Ref, BTreeSet<u64>>, open_here: &[Pr], source: &dyn Source) -> BTreeMap<Ref, Need> {
    let mut out = BTreeMap::new();
    for of in projects(wanted) {
        let of = of.as_deref();
        let read = || -> Result<BTreeMap<Ref, Need>, String> {
            let found = source.issues(of, &iids_of(wanted, of))?;
            let theirs;
            let open = match of {
                None => open_here,
                Some(o) => {
                    theirs = crate::claim::parse_prs(&source.open_prs(o)?);
                    &theirs
                }
            };
            Ok(resolve(of, wanted, &found, open))
        };
        match read() {
            Ok(needs) => out.extend(needs),
            Err(e) => eprintln!("warning: what is needed of {}: {e}", of.unwrap_or(project.github.repo)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::{APP_SWARM, BOTH};

    fn set(v: &[u64]) -> BTreeSet<Ref> {
        v.iter().copied().map(Ref::here).collect()
    }

    fn by(v: &[u64]) -> BTreeSet<u64> {
        v.iter().copied().collect()
    }

    fn other(p: &str, iid: u64) -> Ref {
        Ref { project: Some(p.into()), iid }
    }

    #[test]
    fn needs_lines_name_issues_in_their_first_clause_only() {
        assert_eq!(parse("Goal.\n\nNeeds S4 (#267).\n"), set(&[267]));
        assert_eq!(parse("Needs S4 (#267); S5 (#268) supplies fly-to. S8 (#271) and S9 (#272) feed it."), set(&[267]), "S6's own line");
        assert_eq!(parse("Needs S6 (#269)'s highlight sets."), set(&[269]));
        assert_eq!(parse("- **Needs:** #265 and #266, then #10.5 later"), set(&[265, 266, 10]));
        assert_eq!(parse("Depends on #3\n> blocked by #4. Not #5\nneeds #6; not #7"), set(&[3, 4, 6]));
        assert!(parse("It needs nothing, see #12.\nNo needs here #13").is_empty(), "a Needs line starts with the word");
    }

    #[test]
    fn another_projects_issue_is_a_need_when_named_in_full() {
        let mut want = set(&[9]);
        want.insert(other("example/gpu-swarm", 5));
        assert_eq!(parse("Needs gs#381, example/gpu-swarm#5, !7, a#8, #0 and #9"), want, "short names and #0 are not needs");
        assert_eq!(parse("**Needs:** cmaughan/swarm-core#4 and #300"), BTreeSet::from([other("cmaughan/swarm-core", 4), Ref::here(300)]));
        assert_eq!(parse("Needs (cmaughan/swarm-core#4)"), BTreeSet::from([other("cmaughan/swarm-core", 4)]));
        for bad in ["Needs /x#4", "Needs x/#4", "Needs a//b#4", "Needs a/../b#4", "Needs .a/b#4", "Needs é/b#4", "Needs &#4", "Needs a/b/c#4"] {
            assert!(parse(bad).is_empty(), "{bad}: {:?}", parse(bad));
        }
        let own = parse_for(&APP_SWARM, "Needs example/app-swarm#299 and example/gpu-swarm#5");
        assert_eq!(own, BTreeSet::from([Ref::here(299), other("example/gpu-swarm", 5)]), "its own path is its own issue");
        assert_eq!(other("g/r", 4).label(), "g/r#4");
        assert_eq!(other("g/r", 4).pr_label(7), "g/r#7");
        assert_eq!(Ref::here(4).pr_label(7), "#7");
    }

    #[test]
    fn hostile_descriptions_parse_without_panicking() {
        let many = format!("Needs {}", (1..1000).map(|n| format!("#{n}")).collect::<Vec<_>>().join(" "));
        assert_eq!(parse(&many).len(), MAX_NEEDS);
        assert!(parse(&format!("Needs #{}", "9".repeat(40))).is_empty(), "too big for a number");
        let long = format!("Needs {}é #5", "x".repeat(MAX_LINE - 7));
        assert!(parse(&long).is_empty(), "cut at the line limit, on a char boundary");
        let path = format!("Needs {}/b#5", "a".repeat(MAX_PATH));
        assert!(parse(&path).is_empty(), "a path too long is no need");
        for s in ["", "Needs", "Needs #", "Needs .", "needs;#1", "#", "Needs é#1 ü", "\u{0}Needs #1", "Needs /#1", "Needs a/#", "Needs ü/ö#1"] {
            let _ = parse(s);
        }
    }

    fn issue(iid: u64, state: &str, body: &str) -> Value {
        json!({"number": iid, "title": format!("issue {iid}"), "state": state, "body": body})
    }

    fn need(at: Ref, state: &str, by_: &[u64], mr: Option<u64>) -> Need {
        Need { project: at.project, iid: at.iid, title: format!("issue {}", at.iid), state: state.into(), by: by(by_), mr }
    }

    #[test]
    fn claimed_issues_needs_resolve_with_their_state_and_mr() {
        let claimed = [issue(269, "open", "Needs S4 (#267)."), issue(271, "open", "Needs #269 and #267."), issue(280, "open", "none")];
        let w = wanted(&APP_SWARM, &claimed);
        assert_eq!(w, BTreeMap::from([(Ref::here(267), by(&[269, 271]))]), "#269 is claimed too: not a need");
        let mrs = [
            Pr { number: 293, head_ref: "267-gpu".into(), ..Default::default() },
            Pr { number: 300, head_ref: "300-other".into(), body: "mentions #267".into(), ..Default::default() },
        ];
        let found = [issue(267, "open", ""), issue(999, "open", "")];
        let r = resolve(None, &w, &found, &mrs);
        let at = Ref::here(267);
        assert_eq!(r, BTreeMap::from([(at.clone(), need(at.clone(), "open", &[269, 271], Some(293)))]), "a mention isn't a pull request for it");
        assert_eq!(r[&at].status(), "#267 open (needed by #269, #271), PR #293 · issue 267");
        assert_eq!(Need::from_json(&r[&at].to_json()).as_ref(), Some(&r[&at]));
    }

    #[test]
    fn another_projects_need_resolves_against_that_project() {
        for p in BOTH {
            let claimed = [issue(300, "open", "Needs cmaughan/swarm-core#3 and #299.")];
            let w = wanted(&p, &claimed);
            let f = Some("cmaughan/swarm-core");
            assert_eq!(projects(&w), BTreeSet::from([None, f.map(str::to_owned)]));
            assert_eq!(iids_of(&w, f), [3]);
            assert_eq!(iids_of(&w, None), [299]);
            let mrs = [Pr { number: 4, head_ref: "3-phase2".into(), ..Default::default() }];
            let r = resolve(f, &w, &[issue(3, "open", ""), issue(299, "open", "")], &mrs);
            let at = other("cmaughan/swarm-core", 3);
            assert_eq!(r.keys().collect::<Vec<_>>(), [&at], "only that project's need, and #299 isn't Swarm Core's");
            assert_eq!(r[&at].status(), "cmaughan/swarm-core#3 open (needed by #300), PR cmaughan/swarm-core#4 · issue 3");
            assert_eq!(Need::from_json(&r[&at].to_json()).as_ref(), Some(&r[&at]));
        }
    }

    /// A fake [`Source`]: `cmaughan/gone` can't be read; what was asked, in `asked`.
    struct Fake {
        asked: std::cell::RefCell<Vec<String>>,
    }

    impl Source for Fake {
        fn issues(&self, of: Option<&str>, numbers: &[u64]) -> Result<Vec<Value>, String> {
            self.asked.borrow_mut().push(format!("issues {of:?} {numbers:?}"));
            match of {
                Some("cmaughan/gone") => Err("Not Found (HTTP 404)".into()),
                Some(_) => Ok(vec![issue(3, "closed", "")]),
                None => Ok(vec![issue(299, "open", "")]),
            }
        }
        fn open_prs(&self, of: &str) -> Result<Vec<Value>, String> {
            self.asked.borrow_mut().push(format!("prs {of}"));
            Ok(vec![json!({"number": 4, "head_ref": "3-x", "state": "open"})])
        }
    }

    #[test]
    fn fetch_asks_each_project_it_names_and_survives_one_that_fails() {
        let p = APP_SWARM;
        let w = wanted(&p, &[issue(300, "open", "Needs cmaughan/swarm-core#3, cmaughan/gone#1 and #299.")]);
        let fake = Fake { asked: std::cell::RefCell::new(Vec::new()) };
        let open_here = [Pr { number: 325, head_ref: "299-phase1".into(), state: "open".into(), ..Default::default() }];
        let got = fetch(&p, &w, &open_here, &fake);
        let keys: Vec<String> = got.keys().map(Ref::label).collect();
        assert_eq!(keys, ["#299", "cmaughan/swarm-core#3"], "the repository that failed is left out");
        assert_eq!(got[&Ref::here(299)].mr, Some(325), "this repository's pull requests come from the caller");
        assert_eq!(got[&other("cmaughan/swarm-core", 3)].mr, Some(4), "another's from its own list");
        assert!(!fake.asked.borrow().iter().any(|a| a.starts_with("prs example/app-swarm")), "{:?}", fake.asked.borrow());
    }

    #[test]
    fn a_need_saved_before_cross_project_needs_reads_as_this_projects() {
        let n = Need::from_json(&json!({"iid": 5, "title": "t", "state": "open", "by": [9], "mr": null})).unwrap();
        assert_eq!(n.at(), Ref::here(5));
    }

    #[test]
    fn a_need_that_closes_or_is_taken_is_news() {
        let f = |iid| other("cmaughan/swarm-core", iid);
        let before = BTreeMap::from([
            (Ref::here(1), need(Ref::here(1), "open", &[9], None)),
            (Ref::here(2), need(Ref::here(2), "open", &[9], None)),
            (Ref::here(3), need(Ref::here(3), "open", &[9], Some(5))),
            (f(1), need(f(1), "open", &[9], None)),
        ]);
        let now = BTreeMap::from([
            (Ref::here(1), need(Ref::here(1), "closed", &[9], None)),
            (Ref::here(2), need(Ref::here(2), "open", &[9], Some(7))),
            (Ref::here(3), need(Ref::here(3), "open", &[9], Some(5))),
            (Ref::here(4), need(Ref::here(4), "closed", &[9], None)),
            (f(1), need(f(1), "open", &[9], Some(2))),
        ]);
        assert_eq!(
            changes(&before, &now),
            [
                "#1 closed (needed by #9): start what waited on it · issue 1",
                "#2 (needed by #9) taken: PR #7 · issue 2",
                "cmaughan/swarm-core#1 (needed by #9) taken: PR cmaughan/swarm-core#2 · issue 1"
            ],
            "no news: unchanged, or first seen"
        );
    }
}
