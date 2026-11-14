//! `prwatch`'s send-backs (mornycorp/swarm-core#14): when the coordinator sends one of the
//! agent's open PRs back, `prwatch` says so and logs `work --issue <n> --mr <m>` for it, once, so a
//! timeline reader sees where the rework starts and can read the time before it as waiting.
//!
//! - **A send-back** is read from the coordinators' files on the tracking branch, as
//!   docs/tracking-format.md › Send-backs defines it: a gate (a sub-agent `gate…`) whose notes
//!   name the PR (`#<m>` before the note's first `;`) and whose `sub.stop` failed for the change's
//!   cause (`rc≠0` and a cause that isn't transient), unless a later gate reruns it (`rerun of
//!   <sub>:`, whose result counts instead; `gate` writes it for a gate of the same unchanged stack
//!   too, mornycorp/swarm-core#20); or a `sendback` event naming the PR. A `sendback`
//!   after a gate's failure of the PR, before its next gate, is that same send-back.
//! - **The gate daemon's gates** (`gate --daemon`, mornycorp/swarm-core#44: a start note that
//!   begins `daemon gate:`, `regate of`, `bisect of` or `verify main for`, and their reruns) send
//!   back only what their `sendback` events name: the daemon gates a stack of several PRs, blames
//!   one for a failure (or none: it bisects, or `main` itself fails), and gates the rest again, so
//!   a failed stack is no send-back of every PR in it.
//! - **Once:** a send-back is due only when the agent's own file holds no `claim`, `work` or
//!   `submit` of the issue since it (the worker logged its own `work`, or already resubmitted),
//!   and `prwatch`'s record doesn't hold it (a `work` still queued, or a clock behind the
//!   coordinator's). A question comment is no send-back: nothing is logged for one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::gate::{self, Cause};

/// The latest send-back of one PR: when the coordinator sent it back, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendBack {
    pub mr: u64,
    /// The issue its `sendback` names, when one does.
    pub issue: Option<u64>,
    /// When (the failed gate's `sub.stop`, or the `sendback`): `YYYY-MM-DDTHH:MM:SSZ`.
    pub t: String,
    /// Why, in a few words: the `sendback`'s note, else the failed gate's result.
    pub why: String,
}

impl SendBack {
    /// Its key in `prwatch`'s record: one `work` a send-back.
    pub fn key(&self) -> String {
        format!("#{}@{}", self.mr, self.t)
    }
}

/// A send-back the agent hasn't picked up yet: log `work --issue <issue> --mr <back.mr>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    pub issue: u64,
    pub back: SendBack,
}

impl Resume {
    /// The `work` event's note: the send-back it picks up.
    pub fn note(&self) -> String {
        crate::event::one_line(&format!("rework: #{} sent back {}: {}", self.back.mr, self.back.t, self.back.why))
    }

    /// What `prwatch` says of it: when and why, then `logged`, what became of its `work`.
    pub fn line(&self, title: &str, logged: &str) -> String {
        let SendBack { mr, t, why, .. } = &self.back;
        format!("#{mr} sent back ({t}): {why}; {logged} · {title}")
    }
}

/// What happened to a PR at a moment, as the coordinators' files say.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// A gate that names it started.
    Gated,
    /// A gate that names it failed for the change's cause.
    Failed(String),
    /// A `sendback` names it, with its note.
    SentBack { issue: Option<u64>, why: String },
}

/// One gate as a file logs it.
#[derive(Clone, Debug, Default)]
struct Gate {
    start: Option<String>,
    prs: Vec<u64>,
    rerun_of: Option<String>,
    /// The gate daemon's (or a rerun of one): its failure sends back only what a `sendback` names.
    daemon: bool,
    /// Its `sub.stop`: when, and its note.
    stop: Option<(String, String)>,
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// How the gate daemon starts a gate's note ([`crate::gate::daemon`]'s `gate_note`).
const DAEMON_NOTES: [&str; 4] = ["daemon gate:", "regate of ", "bisect of ", "verify main for "];

/// Whether a gate's start note is the gate daemon's, after any `rerun of <sub>:`.
fn daemons(note: &str) -> bool {
    let what = note.strip_prefix("rerun of ").and_then(|r| r.split_once(':')).map_or(note, |(_, rest)| rest).trim_start();
    DAEMON_NOTES.iter().any(|start| what.starts_with(start))
}

/// Every PR `#<m>` a gate's own text names, each once: the start note before its first `;`,
/// after any `rerun of <sub>:`, which comes back as the gate it reruns.
fn gated(note: &str) -> (Option<String>, Vec<u64>) {
    let what = note.split(';').next().unwrap_or_default();
    let (rerun_of, what) = match what.strip_prefix("rerun of ").and_then(|r| r.split_once(':')) {
        Some((of, rest)) => (Some(of.trim().to_owned()), rest),
        None => (None, what),
    };
    let mut prs = Vec::new();
    for part in what.split('#').skip(1) {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u64>()
            && !prs.contains(&n)
        {
            prs.push(n);
        }
    }
    (rerun_of, prs)
}

/// The gates one file logs, by `sub`; a rerun that names no PR gates those of the gate it reruns.
fn gates(lines: &[Value]) -> BTreeMap<String, Gate> {
    let mut gates: BTreeMap<String, Gate> = BTreeMap::new();
    for v in lines {
        let Some(sub) = text(v, "sub").filter(|s| s.starts_with("gate")) else { continue };
        let (t, note) = (text(v, "t").unwrap_or_default().to_owned(), text(v, "note").unwrap_or_default());
        let g = gates.entry(sub.to_owned()).or_default();
        match text(v, "event") {
            Some("sub.start") => {
                (g.rerun_of, g.prs) = gated(note);
                g.daemon = daemons(note);
                g.start = Some(t);
            }
            Some("sub.stop") => {
                g.stop.get_or_insert((t, note.to_owned()));
            }
            _ => {}
        }
    }
    // Reruns of reruns: a few rounds settle any chain a coordinator logs.
    for _ in 0..4 {
        let inherited: Vec<(String, Vec<u64>)> =
            gates.iter().filter(|(_, g)| g.prs.is_empty()).filter_map(|(sub, g)| Some((sub.clone(), gates.get(g.rerun_of.as_deref()?)?.prs.clone()))).collect();
        for (sub, prs) in inherited {
            if let Some(g) = gates.get_mut(&sub) {
                g.prs = prs;
            }
        }
        // A rerun of a daemon's gate is the daemon's.
        let daemons: Vec<String> = gates
            .iter()
            .filter(|(_, g)| !g.daemon)
            .filter(|(_, g)| g.rerun_of.as_deref().and_then(|of| gates.get(of)).is_some_and(|of| of.daemon))
            .map(|(sub, _)| sub.clone())
            .collect();
        for sub in daemons {
            if let Some(g) = gates.get_mut(&sub) {
                g.daemon = true;
            }
        }
    }
    gates
}

/// Why a gate's `sub.stop` note says it failed for the change's cause, or `None`: it passed, it
/// was stopped before its result (no `rc`), or its cause is the machine's (transient, or its
/// setup).
fn failed(note: &str) -> Option<String> {
    let rc: i64 = gate::tokens(note).next()?.strip_prefix("rc=")?.parse().ok()?;
    let cause = gate::field(note, "cause");
    let machines = cause.and_then(Cause::parse).is_some_and(|c| c.transient() || c.setup());
    (rc != 0 && !machines).then(|| match cause {
        Some(c) => format!("rc={rc}; cause={c}"),
        None => format!("rc={rc}"),
    })
}

/// One file's steps for the PRs in `mrs`, each with its time.
fn steps(lines: &[Value], mrs: &BTreeSet<u64>) -> Vec<(String, u64, Step)> {
    let gates = gates(lines);
    let rerun: BTreeSet<&str> = gates.values().filter_map(|g| g.rerun_of.as_deref()).collect();
    let mut out = Vec::new();
    for (sub, g) in &gates {
        for &mr in g.prs.iter().filter(|m| mrs.contains(m)) {
            if let Some(t) = &g.start {
                out.push((t.clone(), mr, Step::Gated));
            }
            // A daemon's failed stack sends back only what its `sendback` events name (#44).
            let failure = g.stop.as_ref().filter(|_| !g.daemon && !rerun.contains(sub.as_str())).and_then(|(t, note)| Some((t, failed(note)?)));
            if let Some((t, why)) = failure {
                out.push((t.clone(), mr, Step::Failed(format!("gate {sub} failed ({why})"))));
            }
        }
    }
    for v in lines.iter().filter(|v| text(v, "event") == Some("sendback")) {
        let Some(mr) = v.get("mr").and_then(Value::as_u64).filter(|m| mrs.contains(m)) else { continue };
        let why = text(v, "note").filter(|n| !n.is_empty()).unwrap_or("no note").to_owned();
        out.push((text(v, "t").unwrap_or_default().to_owned(), mr, Step::SentBack { issue: v.get("issue").and_then(Value::as_u64), why }));
    }
    out
}

/// The latest send-back among one PR's steps, oldest first.
fn latest(mr: u64, steps: &[&(String, u64, Step)]) -> Option<SendBack> {
    let mut last: Option<SendBack> = None;
    // A gate's failure not yet followed by its PR's next gate or a `sendback`.
    let mut failure_open = false;
    for (t, _, step) in steps {
        match step {
            Step::Gated => failure_open = false,
            Step::Failed(why) => {
                last = Some(SendBack { mr, issue: None, t: t.clone(), why: why.clone() });
                failure_open = true;
            }
            Step::SentBack { issue, why } => {
                match last.as_mut().filter(|_| failure_open) {
                    // The same send-back as the gate's failure: the coordinator's words say why.
                    Some(same) => {
                        same.issue = *issue;
                        same.why.clone_from(why);
                    }
                    None => last = Some(SendBack { mr, issue: *issue, t: t.clone(), why: why.clone() }),
                }
                failure_open = false;
            }
        }
    }
    last
}

/// The latest send-back of each PR in `mrs`, from every agent's file (`(instance, text)`).
pub fn send_backs(files: &[(String, String)], mrs: &BTreeSet<u64>) -> BTreeMap<u64, SendBack> {
    let mut all: Vec<(String, u64, Step)> = Vec::new();
    for (_, text) in files {
        let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        all.extend(steps(&lines, mrs));
    }
    // Stable: a file's own order holds for events logged in the same second.
    all.sort_by(|a, b| a.0.cmp(&b.0));
    mrs.iter()
        .filter_map(|&mr| {
            let mine: Vec<&(String, u64, Step)> = all.iter().filter(|s| s.1 == mr).collect();
            Some((mr, latest(mr, &mine)?))
        })
        .collect()
}

/// The issue of each PR the agent's own file names (`claim`, `submit`), and the latest time it
/// logged `claim`, `work` or `submit` of each issue.
fn own_events(own: &str) -> (BTreeMap<u64, u64>, BTreeMap<u64, String>) {
    let (mut issue_of, mut since) = (BTreeMap::new(), BTreeMap::new());
    for v in own.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let Some(issue) = v.get("issue").and_then(Value::as_u64) else { continue };
        if !matches!(text(&v, "event"), Some("claim" | "work" | "submit")) || v.get("sub").is_some() {
            continue;
        }
        if let Some(mr) = v.get("mr").and_then(Value::as_u64) {
            issue_of.insert(mr, issue);
        }
        let t = text(&v, "t").unwrap_or_default().to_owned();
        let latest = since.entry(issue).or_insert_with(String::new);
        if t > *latest {
            *latest = t;
        }
    }
    (issue_of, since)
}

/// The send-backs of the agent's open PRs (`open`) it hasn't picked up: none in its own file
/// (`own`) since, nor in `record`.
pub fn due(files: &[(String, String)], own: &str, open: &BTreeSet<u64>, record: &BTreeSet<String>) -> Vec<Resume> {
    let (issue_of, since) = own_events(own);
    send_backs(files, open)
        .into_values()
        .filter(|b| !record.contains(&b.key()))
        .filter_map(|back| {
            let issue = issue_of.get(&back.mr).copied().or(back.issue)?;
            let picked_up = since.get(&issue).is_some_and(|t| *t >= back.t);
            (!picked_up).then_some(Resume { issue, back })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(a, t)| ((*a).to_owned(), (*t).to_owned())).collect()
    }

    fn set(mrs: &[u64]) -> BTreeSet<u64> {
        mrs.iter().copied().collect()
    }

    const WORKER: &str = r#"{"event":"claim","issue":7,"mr":9,"t":"2026-10-17T10:00:00Z"}
{"event":"work","issue":7,"t":"2026-10-17T10:00:05Z"}
{"event":"submit","issue":7,"mr":9,"t":"2026-10-17T11:00:00Z"}"#;

    #[test]
    fn a_sendback_is_due_once_until_the_worker_picks_it_up() {
        let coordinator = r#"{"event":"sendback","issue":7,"mr":9,"note":"cause=conflict: Cargo.toml","t":"2026-10-17T12:00:00Z"}"#;
        let all = files(&[("pc-c", coordinator), ("pc-w", WORKER)]);
        let due_now = due(&all, WORKER, &set(&[9]), &BTreeSet::new());
        assert_eq!(due_now.len(), 1, "{due_now:?}");
        let r = &due_now[0];
        assert_eq!((r.issue, r.back.mr, r.back.t.as_str()), (7, 9, "2026-10-17T12:00:00Z"));
        assert_eq!(r.note(), "rework: #9 sent back 2026-10-17T12:00:00Z: cause=conflict: Cargo.toml");
        assert_eq!(r.line("T", "logged it"), "#9 sent back (2026-10-17T12:00:00Z): cause=conflict: Cargo.toml; logged it · T");

        // Logged: its `work` is in the agent's file, or still queued (in the record), so not again.
        let worked = format!("{WORKER}\n{{\"event\":\"work\",\"issue\":7,\"mr\":9,\"t\":\"2026-10-17T12:02:00Z\"}}");
        assert!(due(&all, &worked, &set(&[9]), &BTreeSet::new()).is_empty(), "the work is on the branch");
        assert!(due(&all, WORKER, &set(&[9]), &[r.back.key()].into_iter().collect()).is_empty(), "the record holds it");
        // A worker whose clock is behind the coordinator's: the record still holds it.
        let behind = format!("{WORKER}\n{{\"event\":\"work\",\"issue\":7,\"mr\":9,\"t\":\"2026-10-17T11:59:00Z\"}}");
        assert!(due(&all, &behind, &set(&[9]), &[r.back.key()].into_iter().collect()).is_empty());
        // A PR no longer open, or another agent's, is none of its business.
        assert!(due(&all, WORKER, &set(&[10]), &BTreeSet::new()).is_empty());
    }

    #[test]
    fn a_resubmitted_pr_or_a_question_logs_nothing() {
        let coordinator = r#"{"event":"sendback","issue":7,"mr":9,"note":"fix the lint","t":"2026-10-17T12:00:00Z"}"#;
        let resubmitted = format!("{WORKER}\n{{\"event\":\"submit\",\"issue\":7,\"mr\":9,\"t\":\"2026-10-17T13:00:00Z\"}}");
        assert!(due(&files(&[("pc-c", coordinator)]), &resubmitted, &set(&[9]), &BTreeSet::new()).is_empty());
        // No send-back on the branch (a comment that asks a question is no event): nothing due.
        assert!(due(&files(&[("pc-w", WORKER)]), WORKER, &set(&[9]), &BTreeSet::new()).is_empty());
    }

    #[test]
    fn a_gate_that_fails_for_the_change_sends_its_prs_back() {
        let coordinator = r#"{"event":"sub.start","name":"gate","note":"local gate: #9 (#7); capacity=1 free=9.8; kind=smoke","sub":"gate-mr9","t":"2026-10-17T12:00:00Z"}
{"event":"sub.stop","note":"rc=101; cause=test; kind=smoke","sub":"gate-mr9","t":"2026-10-17T12:10:00Z"}
{"event":"sendback","issue":7,"mr":9,"note":"cause=test: the tests fail","sub":"gate-mr9","t":"2026-10-17T12:11:00Z"}"#;
        let backs = send_backs(&files(&[("pc-c", coordinator)]), &set(&[9]));
        let b = &backs[&9];
        assert_eq!(
            (b.t.as_str(), b.why.as_str(), b.issue),
            ("2026-10-17T12:10:00Z", "cause=test: the tests fail", Some(7)),
            "the sendback after the failure is the same send-back"
        );
        let due_now = due(&files(&[("pc-c", coordinator)]), WORKER, &set(&[9]), &BTreeSet::new());
        assert_eq!(due_now.len(), 1, "one work for the failure and its sendback");

        // A failure with no sendback is one too, its why the gate's result.
        let alone = coordinator.lines().take(2).collect::<Vec<_>>().join("\n");
        assert_eq!(send_backs(&files(&[("pc-c", &alone)]), &set(&[9]))[&9].why, "gate gate-mr9 failed (rc=101; cause=test)");
    }

    /// #44: the gate daemon gates three PRs, the stack fails, and it blames one: only that one
    /// is sent back, by its `sendback`. The others are gated again, and their failed first gate,
    /// or its own rerun, is no send-back; nor is a bisection's failed half, or a check of `main`.
    #[test]
    fn a_daemons_failed_stack_sends_back_only_the_pr_it_blames() {
        let coordinator = r#"{"event":"sub.start","name":"gate","note":"daemon gate: #9 #10 #11; capacity=1 free=9.8; kind=smoke","sub":"gate-1","t":"2026-10-17T12:00:00Z"}
{"event":"sub.stop","note":"rc=101; cause=test; kind=smoke","sub":"gate-1","t":"2026-10-17T12:10:00Z"}
{"event":"sub.start","name":"gate","note":"rerun of gate-1: its failed tests alone (1), then the steps it skipped; kind=smoke","sub":"gate-1-rerun","t":"2026-10-17T12:10:30Z"}
{"event":"sub.stop","note":"rc=101; cause=test; kind=smoke","sub":"gate-1-rerun","t":"2026-10-17T12:12:00Z"}
{"event":"sendback","issue":8,"mr":10,"note":"cause=test: its test fails","sub":"gate-1","t":"2026-10-17T12:12:30Z"}
{"event":"sub.start","name":"gate","note":"regate of gate-1: #9 #11; capacity=1 free=9.8; kind=smoke","sub":"gate-2","t":"2026-10-17T12:13:00Z"}
{"event":"sub.stop","note":"rc=101; cause=build; kind=smoke","sub":"gate-2","t":"2026-10-17T12:20:00Z"}
{"event":"sub.start","name":"gate","note":"bisect of gate-2: #9; kind=smoke","sub":"gate-3","t":"2026-10-17T12:21:00Z"}
{"event":"sub.stop","note":"rc=0; kind=smoke","sub":"gate-3","t":"2026-10-17T12:30:00Z"}"#;
        let backs = send_backs(&files(&[("pc-c", coordinator)]), &set(&[9, 10, 11]));
        assert_eq!(backs.keys().copied().collect::<Vec<_>>(), [10], "{backs:?}");
        assert_eq!((backs[&10].t.as_str(), backs[&10].why.as_str(), backs[&10].issue), ("2026-10-17T12:12:30Z", "cause=test: its test fails", Some(8)));
        // The same stack gated by hand is a send-back of each, as before.
        let by_hand = coordinator.lines().take(2).collect::<Vec<_>>().join("\n").replace("daemon gate: ", "local gate: ");
        assert_eq!(send_backs(&files(&[("pc-c", &by_hand)]), &set(&[9, 10, 11])).len(), 3);
        assert!(daemons("rerun of gate-4: daemon gate: #1 #2") && daemons("verify main for gate-1") && !daemons("local gate: #9 (regate of the last)"));
    }

    #[test]
    fn a_transient_failure_a_rerun_or_a_stopped_gate_is_no_send_back() {
        let start = r#"{"event":"sub.start","name":"gate","note":"local gate: #9 (#7); capacity=1 free=9.8","sub":"gate-mr9","t":"2026-10-17T12:00:00Z"}"#;
        for stop in ["rc=101; cause=memory", "rc=1; cause=python", "rc=0; merged #9", "superseded: main moved"] {
            let file = format!("{start}\n{{\"event\":\"sub.stop\",\"note\":\"{stop}\",\"sub\":\"gate-mr9\",\"t\":\"2026-10-17T12:10:00Z\"}}");
            assert!(send_backs(&files(&[("pc-c", &file)]), &set(&[9])).is_empty(), "{stop}");
        }
        // A failed test that a rerun answers: the rerun's result counts, and it names no PR itself.
        let rerun = format!(
            "{start}\n{}\n{}\n{}",
            r#"{"event":"sub.stop","note":"rc=101; cause=test","sub":"gate-mr9","t":"2026-10-17T12:10:00Z"}"#,
            r#"{"event":"sub.start","name":"gate","note":"rerun of gate-mr9: the failed tests alone; capacity=1 free=9","sub":"gate-mr9-rerun","t":"2026-10-17T12:11:00Z"}"#,
            r#"{"event":"sub.stop","note":"rc=0; merged #9","sub":"gate-mr9-rerun","t":"2026-10-17T12:15:00Z"}"#,
        );
        assert!(send_backs(&files(&[("pc-c", &rerun)]), &set(&[9])).is_empty());
        let failed_again = rerun.replace(r#""rc=0; merged #9","sub":"gate-mr9-rerun""#, r#""rc=101; cause=test","sub":"gate-mr9-rerun""#);
        assert_eq!(send_backs(&files(&[("pc-c", &failed_again)]), &set(&[9]))[&9].t, "2026-10-17T12:15:00Z", "the rerun gates the PRs of the gate it reruns");
    }

    /// mornycorp/swarm-core#20: a gate of the same stack straight after a failure, with no flag,
    /// starts as its rerun (`gate::silent`): its pass answers the flake, and its failure is the
    /// one send-back, at its own time.
    #[test]
    fn a_silent_rerun_the_gate_notes_itself_counts_as_a_rerun() {
        let note = gate::silent::note("local gate: #9 (#7); capacity=1 free=9.8; kind=smoke", Some("gate-mr9"));
        let lines = |rc: &str| {
            [
                r#"{"event":"sub.start","name":"gate","note":"local gate: #9 (#7); capacity=1 free=9.8; kind=smoke","sub":"gate-mr9","t":"2026-10-17T12:00:00Z"}"#.to_owned(),
                r#"{"event":"sub.stop","note":"rc=101; cause=test; kind=smoke","sub":"gate-mr9","t":"2026-10-17T12:10:00Z"}"#.to_owned(),
                format!(r#"{{"event":"sub.start","name":"gate","note":"{note}","sub":"gate-mr9-2","t":"2026-10-17T12:11:00Z"}}"#),
                format!(r#"{{"event":"sub.stop","note":"{rc}","sub":"gate-mr9-2","t":"2026-10-17T12:20:00Z"}}"#),
            ]
            .join("\n")
        };
        assert!(send_backs(&files(&[("pc-c", &lines("rc=0; kind=smoke; merged #9"))]), &set(&[9])).is_empty(), "a pass: no send-back");
        let failed = send_backs(&files(&[("pc-c", &lines("rc=101; cause=test; kind=smoke"))]), &set(&[9]));
        assert_eq!((failed.len(), failed[&9].t.as_str()), (1, "2026-10-17T12:20:00Z"), "a failure again: one send-back, the rerun's");
        assert!(failed[&9].why.contains("gate-mr9-2"), "{failed:?}");
    }

    #[test]
    fn each_send_back_after_a_resubmit_is_new() {
        let coordinator = r#"{"event":"sendback","issue":7,"mr":9,"note":"first","t":"2026-10-17T12:00:00Z"}
{"event":"sub.start","name":"gate","note":"local gate: #9 (#7)","sub":"gate-2","t":"2026-10-17T14:00:00Z"}
{"event":"sub.stop","note":"rc=1; cause=build","sub":"gate-2","t":"2026-10-17T14:05:00Z"}"#;
        let reworked = format!(
            "{WORKER}\n{}\n{}",
            r#"{"event":"work","issue":7,"mr":9,"t":"2026-10-17T12:01:00Z"}"#, r#"{"event":"submit","issue":7,"mr":9,"t":"2026-10-17T13:00:00Z"}"#
        );
        let due_now = due(&files(&[("pc-c", coordinator)]), &reworked, &set(&[9]), &BTreeSet::new());
        assert_eq!(due_now.iter().map(|r| r.back.t.as_str()).collect::<Vec<_>>(), ["2026-10-17T14:05:00Z"], "the second send-back, after the resubmit");
        assert_eq!(gated("rerun of gate-1: local gate: #12 #13 (#4 #5); kind=smoke"), (Some("gate-1".into()), vec![12, 13, 4, 5]));
        assert_eq!(gated("full gate of main; capacity=1; kind=full; commit=abc"), (None, vec![]));
    }
}
