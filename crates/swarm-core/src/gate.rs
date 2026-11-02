//! `cargo xtask gate`: the coordinator's local gate, run in its own
//! worktree (docs/swarm-protocol.md › The coordinator's loop). Phase 3 of
//! Swarm Core; the project's own parts are [`Hooks`]: [`Hooks::gate_kind`],
//! [`Hooks::extra_causes`] and [`Hooks::around_ci`].
//!
//! - **Refuses** a rebase or merge in progress, or conflict markers in the tree, and, without
//!   `--merge`, a HEAD that isn't the top of the worktree's stack record when it has one
//!   ([`stack_refusal`]).
//! - **Takes the gate lock** ([`crate::lock`]). With `--merge <branch>` (repeatable, ns) it then
//!   puts the worktree at `origin/main` and merges each `origin/<branch>` in order (a conflict
//!   aborts the merge and stops the gate before it starts).
//! - **Picks the gate's kind** from the change against `origin/main`: the project's
//!   ([`Hooks::gate_kind`]), the **docs** gate, the **tooling** gate (both `ci --changed`) or the
//!   **full** gate, which the coordinator turns into the **smoke** ([`coordinator_kind`]: `ci
//!   --smoke`, no full test run; the periodic full gate of `main` runs the rest). `--full` forces
//!   the full gate.
//! - **Takes the machine-wide gate lock** both swarms share ([`crate::machine_lock`]) when its kind
//!   builds ([`Kind::takes_machine_lock`]: not a docs gate, whose notes say
//!   `machineLock=skipped`), then checks capacity ([`Kind::waits_for_capacity`]); `--wait` waits
//!   for each, else it refuses (keeping the gate lock). It holds the machine lock until its `ci`
//!   ends; its `ci` gets `CARGO_BUILD_JOBS` from the free memory it saw unless the environment
//!   sets it.
//! - **Logs `sub.start --name gate`** with the capacity it saw and its kind ([`kind_field`]), and
//!   runs the consumer's `cargo xtask ci` (the `ci` contract, docs/ci-contract.md) with this
//!   xtask's own exe, through [`Hooks::around_ci`] (a project's Python and its GPU lease, say). The
//!   whole log goes to `<target>/gate-logs/<sub>.log`, and `rc=<n> <secs>s [cause=<cause>]
//!   kind=<kind> (<why>) head=<commit>` to `<sub>.rc` beside it ([`rc_line`]): `land` lands only
//!   the HEAD it tested. The tests' times, nextest's junit file, are kept as `<sub>.junit.xml`
//!   ([`keep_junit`]).
//! - **A failure** is classified from the log ([`classify`], the project's own causes first),
//!   logged `sub.stop` with `rc=<n>; cause=<cause>; kind=<kind>`, and keeps the lock: the
//!   coordinator still owns the worktree. `--retry-transient` reruns once, as `<sub>-rerun`, when
//!   the cause is the machine's ([`Cause::transient`]). The failing tests are listed.
//! - **A test that failed under load**: `gate --rerun-failed <sub>`
//!   reruns only the failed tests, each alone, and the steps the failure skipped, on the same HEAD,
//!   as `<sub>-rerun`, and records a verified pass `land` accepts ([`rerun`]).
//! - **A pass** leaves the sub-agent open, so the merges it allows are logged with `--sub <sub>`;
//!   `gate --stop <sub> --note "<text>"` closes it (`rc=0; kind=<kind>; <text>`, from its `.rc`) and
//!   drops the lock ([`close`]). A gate stopped part way has no `.rc`, and its stop no `rc=`: it
//!   never passed (mornycorp/swarm-core#3).
//! - **The periodic full gate's runs** ([`full_attempt`]) are this gate's too, kind full, in the
//!   full gate's checkout.
//!
//! A `<sub>.rc` line is `rc=<n> … head=<sha> …`: `key=value` tokens split at whitespace and at
//! `;`, and a rerun's adds `of=<sub> rerun=<test>,<test>`. The notes are a convention within
//! tracking format v3, so a timeline viewer's gate analytics read them with no format change.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::stack::{self, Stack, short};
use crate::{Hooks, Project, capacity, event, fullgate, lock, machine_lock};

pub mod rerun;

/// Where a gate writes its log and records: `<target dir>/gate-logs/<sub>.log`, `.rc`.
pub use crate::gate_times::LOG_DIR;

/// Where nextest's `ci` profile writes its junit file, from the workspace root, whatever
/// `CARGO_TARGET_DIR` says (the `ci` contract).
pub const JUNIT: &str = "target/nextest/ci/junit.xml";
/// The base the gate merges onto and the docs and tooling gates compare against.
pub const BASE: &str = "origin/main";
/// The testkit's temporary repositories' prefix (`swarm-test-*`): git's `fatal:` about one is a
/// test's fixture under load, not the change.
const TEMP_REPO: &str = "swarm-test-";

const USAGE_TEXT: &str =
    "usage: cargo xtask gate --sub <id> --note \"<what it gates>\" [--merge <branch>]... [--full] [--wait] [--retry-transient] [--agent <instance>]
       cargo xtask gate --rerun-failed <sub> [--note \"<text>\"] [--wait] [--agent <instance>]
       cargo xtask gate --stop <sub> --note \"<what it merged>\" [--agent <instance>]";

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask gate --sub <id> --note \"<what>\" [--merge <branch>]… [--full] [--wait] [--retry-transient] | --rerun-failed <sub> | --stop <sub> --note \"<what>\"";

/// How often `--wait` looks at capacity again, and for how long at most.
const WAIT_STEP: Duration = Duration::from_secs(60);
const WAIT_MAX: Duration = Duration::from_secs(2 * 3600);

/// Why a gate failed: the machine's causes, a test's, a build's, and two that a project's gate
/// names through [`Hooks::extra_causes`] (`gpu`, `python`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// The machine ran out: os error 1450 ("Insufficient system resources") or 1455, or the
    /// linker's exit 0xc000009a (`STATUS_NO_MEMORY`).
    Memory,
    /// The linker couldn't open a file it writes (LNK1104): another build or a scanner held it.
    Link,
    /// The toolchain lost its standard library mid-run ("can't find crate for `std`", rustc's
    /// "only metadata stub found").
    Toolchain,
    /// Git failed inside a test's temporary repository.
    GitTemp,
    /// The GPU or the desktop under load (a project's [`Hooks::extra_causes`]).
    Gpu,
    /// No Python for the Python-API oracle tests (a project's [`Hooks::extra_causes`]): the gate
    /// machine's setup, which a rerun fails the same way.
    Python,
    /// A real test failure.
    Test,
    /// A compile error (clippy's included).
    Build,
    Other,
}

impl Cause {
    pub const ALL: [Cause; 9] =
        [Cause::Memory, Cause::Link, Cause::Toolchain, Cause::GitTemp, Cause::Gpu, Cause::Python, Cause::Test, Cause::Build, Cause::Other];

    pub fn name(self) -> &'static str {
        match self {
            Cause::Memory => "memory",
            Cause::Link => "link",
            Cause::Toolchain => "toolchain",
            Cause::GitTemp => "git-temp",
            Cause::Gpu => "gpu",
            Cause::Python => "python",
            Cause::Test => "test",
            Cause::Build => "build",
            Cause::Other => "other",
        }
    }

    /// The cause a note names (`cause=<name>`), as [`Cause::name`] writes it.
    pub fn parse(name: &str) -> Option<Cause> {
        Cause::ALL.into_iter().find(|c| c.name() == name)
    }

    /// Not the change's fault: the same tree passes on a rerun.
    pub fn transient(self) -> bool {
        matches!(self, Cause::Memory | Cause::Link | Cause::Toolchain | Cause::GitTemp | Cause::Gpu)
    }

    /// The gate machine's setup, not the change: a rerun fails the same way, and nothing is filed.
    pub fn setup(self) -> bool {
        self == Cause::Python
    }
}

/// What in a log names each of the machine's causes.
const MEMORY: &[&str] = &["os error 1450", "Insufficient system resources", "os error 1455", "paging file is too small", "0xc000009a"];
const TOOLCHAIN: &[&str] = &["can't find crate for `std`", "can't find crate for `core`", "only metadata stub found"];
/// ns gate 107: a fetch between two temporary repositories, under
/// load, among them.
const GIT_TEMP: &[&str] = &["Function not implemented", "Could not read from remote", "index.lock", "unpack-objects failed", "index-pack failed"];

/// The cause of a failed gate, from its log: the project's own when it names one (`extra`,
/// [`Hooks::extra_causes`]), else the machine's (a build that ran out of memory, or a test whose
/// temporary repository git failed in, also reads as a compile error or a failed test), else a
/// test's or a build's.
pub fn classify(log: &str, extra: Option<Cause>) -> Cause {
    let has = |needles: &[&str]| needles.iter().any(|n| log.contains(n));
    if let Some(c) = extra {
        c
    } else if has(MEMORY) {
        Cause::Memory
    } else if log.contains("LNK1104") {
        Cause::Link
    } else if has(TOOLCHAIN) {
        Cause::Toolchain
    } else if has(GIT_TEMP) || temp_repo_fatal(log) {
        Cause::GitTemp
    } else if has(&["test result: FAILED", "error: test run failed"]) || log.lines().any(|l| l.trim_start().starts_with("FAIL [")) {
        Cause::Test
    } else if has(&["error: could not compile", "error[E", "error: aborting due to"]) {
        Cause::Build
    } else {
        Cause::Other
    }
}

/// Whether git said `fatal:` about one of the testkit's temporary repositories
/// (gate-mr314: `fatal: error reading '…/Temp/swarm-test-36668-golden/…'`).
fn temp_repo_fatal(log: &str) -> bool {
    log.lines().any(|l| l.contains("fatal:") && l.contains(TEMP_REPO))
}

/// Which gate a change gets, weakest first: a stack runs the
/// strongest any of its PRs needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// `ci --changed` builds no crate: docs.
    Docs,
    /// `ci --changed` builds only the swarm's tooling (`xtask`), which no product crate depends
    /// on.
    Tooling,
    /// What the coordinator runs on a change the docs and tooling gates don't take, and whenever
    /// it is unsure: `ci --smoke`, the build, the lints and the golden tests, with no full test
    /// run. The periodic full gate of `main` ([`crate::fullgate`]) runs the rest.
    Smoke,
    /// The whole `ci`: `--full`, and the periodic full gate of `main`.
    Full,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Docs, Kind::Tooling, Kind::Smoke, Kind::Full];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Docs => "docs",
            Kind::Tooling => "tooling",
            Kind::Smoke => "smoke",
            Kind::Full => "full",
        }
    }

    /// The kind a note or `.rc` names (`kind=<name>`), as [`Kind::name`] writes it.
    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// `ci`'s arguments for this gate (the `ci` contract).
    pub fn ci_args(self) -> &'static [&'static str] {
        match self {
            Kind::Docs | Kind::Tooling => &["--changed", "--base", BASE],
            Kind::Smoke => &["--smoke"],
            Kind::Full => &[],
        }
    }

    /// Does a gate of this kind build anything worth making room for? A docs gate builds xtask
    /// alone, in seconds; the others build and test crates. The one rule behind
    /// [`Kind::takes_machine_lock`] and [`Kind::waits_for_capacity`].
    pub fn builds(self) -> bool {
        self != Kind::Docs
    }

    /// Does this gate take the machine-wide gate lock? The lock keeps
    /// two gates' builds from filling memory together, so only the kinds that [build](Kind::builds)
    /// take it; a docs gate never waits behind the other swarm's full gate.
    pub fn takes_machine_lock(self) -> bool {
        self.builds()
    }

    /// Does this gate wait for build capacity (`cargo xtask capacity`'s room for one more build)?
    /// Only the kinds that [build](Kind::builds): a docs gate takes no room.
    pub fn waits_for_capacity(self) -> bool {
        self.builds()
    }

    /// What a gate of this kind adds to its `sub.start` note after its kind: `; machineLock=skipped`
    /// when it runs without the machine gate lock, else nothing.
    pub fn lock_note(self) -> &'static str {
        if self.takes_machine_lock() { "" } else { "; machineLock=skipped" }
    }
}

/// The gate the coordinator runs: the smoke where the change asks
/// for the full gate, unless `--full` (`forced`) asks for the full gate itself. The docs and
/// tooling gates stay.
pub fn coordinator_kind(needed: (Kind, String), forced: bool) -> (Kind, String) {
    match needed {
        _ if forced => (Kind::Full, "--full".to_owned()),
        (Kind::Full, why) => (Kind::Smoke, why),
        other => other,
    }
}

/// `kind=<kind> (<why>)`, the why cut short (`kind=<kind>` with none): what a gate's notes and
/// `.rc` record of its kind.
pub fn kind_field(kind: Kind, why: &str) -> String {
    let why = one_line(why, 60);
    if why.is_empty() { format!("kind={}", kind.name()) } else { format!("kind={} ({why})", kind.name()) }
}

/// `sub.start`'s note: the coordinator's, then the capacity the gate saw and its kind
/// ([`kind_field`]), within the event's note limit (the coordinator's text is cut, never the
/// convention).
pub fn start_note(note: &str, more: u64, free_bytes: Option<u64>, kind: &str) -> String {
    let free = free_bytes.map_or_else(|| "?".to_owned(), |b| format!("{:.1}", b as f64 / f64::from(1u32 << 30)));
    let tail = format!("; capacity={more} free={free}; {kind}");
    let room = event::MAX_NOTE.saturating_sub(tail.chars().count());
    format!("{}{tail}", one_line(note, room))
}

/// A failed gate's `sub.stop` note.
pub fn failure_note(rc: i32, cause: Cause, kind: Kind) -> String {
    format!("rc={rc}; cause={}; kind={}", cause.name(), kind.name())
}

/// A closed gate's `sub.stop` note (`gate --stop`, `land`): the exit code, cause and kind its
/// `<sub>.rc` line records, then the text (`rc=0; kind=<kind>; <text>` for a pass). A gate with no
/// `.rc` never finished (stopped part way): its note has no `rc=`, which reads as abandoned, never
/// as passed (mornycorp/swarm-core#3). A text that gives its own `rc=` is kept as it is.
pub fn stop_note(note: &str, rc_line: Option<&str>) -> String {
    let note = note.trim();
    if note.starts_with("rc=") {
        return note.to_owned();
    }
    let rc = rc_line.and_then(|l| tokens(l).next()?.strip_prefix("rc=")?.parse::<i32>().ok());
    let (Some(rc), Some(line)) = (rc, rc_line) else {
        return if note.is_empty() { "stopped before its result".to_owned() } else { note.to_owned() };
    };
    let cause = field(line, "cause").and_then(Cause::parse).map(|c| format!("; cause={}", c.name())).unwrap_or_default();
    let kind = rc_kind(line).map(|k| format!("; kind={}", k.name())).unwrap_or_default();
    if note.is_empty() { format!("rc={rc}{cause}{kind}") } else { format!("rc={rc}{cause}{kind}; {note}") }
}

/// The `<sub>.rc` line: the exit code and the run's time, a failure's cause, the gate's kind
/// ([`kind_field`]), and the HEAD it tested, last.
pub fn rc_line(rc: i32, secs: u64, cause: Option<Cause>, kind: &str, head: &str) -> String {
    match cause {
        Some(c) => format!("rc={rc} {secs}s cause={} {kind} head={head}\n", c.name()),
        None => format!("rc={rc} {secs}s {kind} head={head}\n"),
    }
}

/// The kind a `.rc` line names (`kind=full (why)`, `kind=full; commit=…`).
pub fn rc_kind(rc_line: &str) -> Option<Kind> {
    tokens(rc_line).find_map(|f| f.strip_prefix("kind=")).and_then(Kind::parse)
}

/// A `.rc` line's `key=value` tokens: split at whitespace and at `;`.
pub fn tokens(rc_line: &str) -> impl DoubleEndedIterator<Item = &str> {
    rc_line.split(|c: char| c == ';' || c.is_whitespace()).filter(|t| !t.is_empty())
}

/// The value of `key=` in a `.rc` line: its last (a kind's free text comes before the record's
/// own fields).
pub fn field<'a>(rc_line: &'a str, key: &str) -> Option<&'a str> {
    tokens(rc_line).rev().find_map(|f| f.strip_prefix(key)?.strip_prefix('='))
}

/// The HEAD a passed gate tested, from its `<sub>.rc` line; refused unless it passed.
pub fn passed_head(rc_line: &str) -> Result<&str, String> {
    if tokens(rc_line).next() != Some("rc=0") {
        return Err(format!("the gate did not pass ({})", rc_line.trim()));
    }
    field(rc_line, "head").filter(|h| !h.is_empty()).ok_or_else(|| format!("the gate's record names no HEAD ({})", rc_line.trim()))
}

/// A gate log's `CI summary`, from its last one on.
pub fn ci_summary(log: &str) -> Option<&str> {
    log.rfind("CI summary").and_then(|at| log.get(at..))
}

/// The longest step name [`failed_step`] reads.
const MAX_STEP: usize = 32;

/// The `ci` step a log failed at: its last `CI summary`'s first `FAIL  <step>: …` (the `ci`
/// contract), a name of letters, digits, `-`, `_` and `.`, at most 32 long, so it fits a note's
/// `step=<step>`. None for a log with no summary or no failed step, or a name that isn't one.
pub fn failed_step(log: &str) -> Option<&str> {
    let failed = ci_summary(log)?.lines().find_map(|l| l.trim_start().strip_prefix("FAIL "))?;
    let step = failed.split(':').next()?.trim();
    let name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    (!step.is_empty() && step.len() <= MAX_STEP && step.chars().all(name)).then_some(step)
}

/// A rerun's evidence from its `.rc` line: the gate it verified, the tests that failed there and
/// passed alone, and that gate's summary (`original`, its log); `None` for a gate that isn't a
/// rerun.
pub fn evidence(rc: &str, original: Option<&str>) -> Option<String> {
    let of = field(rc, "of")?;
    let tests = field(rc, "rerun")?;
    let mut s = format!("Verified by a rerun of gate `{of}`: only these tests failed there, each passed alone, and so did the steps the failure skipped:\n");
    for t in tests.split(',') {
        s.push_str(&format!("  {t}\n"));
    }
    if let Some(summary) = original.and_then(ci_summary) {
        s.push_str(&format!("\n{of}'s {}", summary.trim_end()));
    }
    Some(s)
}

/// [`evidence`] for the gate whose `.rc` line is `rc_line`, its original's log read from `logs`.
pub fn rerun_evidence(logs: &Path, rc_line: &str) -> Option<String> {
    let of = field(rc_line, "of").filter(|of| event::check_id(of).is_ok());
    let original = of.and_then(|of| std::fs::read_to_string(logs.join(format!("{of}.log"))).ok());
    evidence(rc_line, original.as_deref())
}

/// Why `head` isn't the tree to gate: the worktree has a stack record (`record`) whose top is
/// another commit (an aborted `stack` once left the failing PR's
/// unrebased branch here, and the gate passed it).
pub fn stack_refusal(record: Option<&Stack>, head: &str) -> Option<String> {
    let top = record?.entries.last()?;
    (top.new_tip != head).then(|| {
        format!(
            "gate refused: HEAD is {}, but the stack's top is {} ({}, #{}): restack (`cargo xtask stack <pr>…`) and gate what it leaves, or gate branches with --merge",
            short(head),
            short(&top.new_tip),
            stack::local_branch(top.mr),
            top.mr
        )
    })
}

/// HEAD, checked against the worktree's stack record ([`stack_refusal`]).
fn stacked_head(project: &Project, root: &Path) -> Result<String, String> {
    let head = stack::rev(root, "HEAD")?;
    match stack_refusal(stack::load_any(project, root)?.as_ref(), &head) {
        Some(why) => Err(why),
        None => Ok(head),
    }
}

/// Why the tree at `root` can't be gated now: a rebase or merge in progress, or conflict markers
/// in tracked files.
pub fn tree_refusal(root: &Path) -> Option<String> {
    for state in ["rebase-merge", "rebase-apply", "MERGE_HEAD"] {
        let p = crate::process::git(root, &["rev-parse", "--git-path", state], None, &[]).map(|s| s.trim().to_owned()).unwrap_or_default();
        let p = if Path::new(&p).is_absolute() { PathBuf::from(&p) } else { root.join(&p) };
        if p.exists() {
            return Some(format!("gate refused: a {} in progress ({state})", if state == "MERGE_HEAD" { "merge" } else { "rebase" }));
        }
    }
    let marked = conflict_markers(root);
    (!marked.is_empty()).then(|| format!("gate refused: conflict markers in {}", marked.join(", ")))
}

/// Tracked files with a conflict marker at the start of a line (`git grep`: exit 1 is none).
fn conflict_markers(root: &Path) -> Vec<String> {
    let out = Command::new("git").current_dir(root).args(["grep", "-lE", "^(<<<<<<<|>>>>>>>) ", "--", "."]).stdin(Stdio::null()).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).take(10).collect(),
        _ => Vec::new(),
    }
}

/// Can `b` name a branch to merge: no option, no whitespace, no `..`?
pub fn check_branch(b: &str) -> Result<(), String> {
    if stack::valid_branch(b) { Ok(()) } else { Err(format!("gate: `{b}` is not a branch to merge")) }
}

/// Put the worktree at `root` at `origin/main` and merge each `origin/<branch>` in order (ns): the
/// tree the gate tests. A conflict aborts that merge (the tree is left at what merged before it)
/// and names the branch. Returns the commit the gate tests, as `git log --oneline -1` says it.
pub fn merge_branches(root: &Path, branches: &[String]) -> Result<String, String> {
    let dirty = crate::process::git(root, &["status", "--porcelain", "--untracked-files=no"], None, &[])?;
    if !dirty.trim().is_empty() {
        return Err(format!("gate refused: the worktree has uncommitted changes, so it can't be put at {BASE}:\n{}", dirty.trim_end()));
    }
    stack::git_slow(root, &["fetch", "-q", "origin"], &[])?;
    stack::git_slow(root, &["checkout", "-q", "--detach", BASE], &[])?;
    for b in branches {
        let theirs = format!("origin/{b}");
        if let Err(e) = stack::git_slow(root, &["merge", "-q", "--no-edit", &theirs], &[]) {
            let _ = stack::git_slow(root, &["merge", "--abort"], &[]);
            return Err(format!("gate: the merge of {b} conflicts with {BASE} and the branches before it: {e}"));
        }
    }
    Ok(crate::process::git(root, &["log", "--oneline", "-1"], None, &[])?.trim().to_owned())
}

/// The commit the gate tests, once it holds the gate lock: the branches `merge` names merged onto
/// `origin/main`, else HEAD, which must be the stack record's top ([`stack_refusal`]).
fn tested_head(project: &Project, root: &Path, merge: &[String], sub: &str) -> Result<String, String> {
    if merge.is_empty() {
        return stacked_head(project, root);
    }
    let line = merge_branches(root, merge).map_err(lock_kept)?;
    println!("gate {sub}: testing {line}");
    stack::rev(root, "HEAD")
}

/// How far a file's time may read behind the clock it is compared with ([`keep_junit`]): well
/// above a coarse file-time tick, well below the gap to an earlier run.
const MTIME_SLACK: Duration = Duration::from_secs(2);

/// Keep the junit file nextest's `ci` profile wrote: copy `from` to
/// `to` when it was written at or after `since`, the start of the gate's `ci`; otherwise no tests
/// ran (a docs gate, a build failure) and an earlier run's file must not pass for this gate's, so
/// `to` is removed. Whether it copied.
///
/// "At or after" allows [`MTIME_SLACK`]: Linux stamps a file's time from a coarse clock that can
/// read a few milliseconds behind `since`, so a quick run's own file looked older (#11).
pub fn keep_junit(from: &Path, to: &Path, since: SystemTime) -> Result<bool, String> {
    let _ = std::fs::remove_file(to);
    let since = since.checked_sub(MTIME_SLACK).unwrap_or(since);
    let fresh = std::fs::metadata(from).and_then(|m| m.modified()).is_ok_and(|t| t >= since);
    if !fresh {
        return Ok(false);
    }
    std::fs::copy(from, to).map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))?;
    Ok(true)
}

/// The tests a log says failed (nextest's `FAIL [ 1.2s] (n/m) <binary> <test>`), each once.
pub fn failed_tests(log: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in log.lines() {
        let Some(rest) = l.trim_start().strip_prefix("FAIL [") else { continue };
        let Some((_, after)) = rest.split_once(']') else { continue };
        let after = after.trim_start();
        // The `(n/m)` counter, when nextest prints one.
        let name = if after.starts_with('(') { after.split_once(')').map_or("", |(_, n)| n) } else { after };
        let name = name.trim().to_owned();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// What of a log is worth showing: its failure lines, the failing tests, and its `CI summary`
/// (else its last three lines).
pub fn summary(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let mut failures: Vec<&str> = Vec::new();
    for l in lines.iter().filter(|l| l.contains("panicked at") || l.starts_with("error") || l.contains("test result: FAILED")) {
        if !failures.contains(l) && failures.len() < 10 {
            failures.push(l);
        }
    }
    let tail = match lines.iter().rposition(|l| l.starts_with("CI summary")) {
        Some(i) => lines.get(i..).unwrap_or_default().iter().take(12).copied().collect::<Vec<_>>(),
        None => lines.iter().rev().take(3).rev().copied().collect(),
    };
    let mut s: String = failures.iter().map(|l| format!("  {}\n", l.trim_end())).collect();
    for t in failed_tests(log) {
        s.push_str(&format!("  failed test: {t}\n"));
    }
    s.extend(tail.iter().map(|l| format!("  {l}\n")));
    s
}

/// Room for a gate's build now: whether there is, and the capacity seen (more builds, free memory).
pub fn room(root: &Path, target: &Path) -> (bool, u64, Option<u64>) {
    let m = capacity::read(root, target);
    let s = capacity::suggest(&m);
    (s.more > 0, s.more, m.free_memory)
}

/// The capacity a gate saw: how many more builds, and the free memory in bytes.
pub type Seen = (u64, Option<u64>);

/// Capacity for the gate: at once, or after waiting (`wait`) up to [`WAIT_MAX`].
fn capacity_for_gate(root: &Path, target: &Path, wait: bool) -> Result<Seen, String> {
    let started = Instant::now();
    loop {
        let (ok, more, free) = room(root, target);
        if ok {
            return Ok((more, free));
        }
        if !wait {
            return Err("gate refused: no room for another build now (`cargo xtask capacity`); --wait waits for it".to_owned());
        }
        if started.elapsed() > WAIT_MAX {
            return Err(format!("gate: still no room for a build after {} min", WAIT_MAX.as_secs() / 60));
        }
        if started.elapsed() < WAIT_STEP {
            eprintln!("gate: no room for another build now: waiting (every {} s)", WAIT_STEP.as_secs());
        }
        std::thread::sleep(WAIT_STEP);
    }
}

/// The machine for one build of a `kind` gate: the machine-wide gate lock when the kind takes it
/// ([`Kind::takes_machine_lock`]), then capacity when it waits for it
/// ([`Kind::waits_for_capacity`]), each at once or, with `wait`, once it is free. The lock's guard
/// (none for a docs gate) and the capacity seen (a docs gate's as it is now).
pub fn room_to_build(
    project: &Project,
    agent: &str,
    root: &Path,
    target: &Path,
    wait: bool,
    kind: Kind,
) -> Result<(Option<machine_lock::Guard>, Seen), String> {
    let machine = if kind.takes_machine_lock() {
        Some(machine_lock::take(project, agent, root, wait)?)
    } else {
        println!("gate: the machine gate lock is skipped: a {} gate builds no product crate", kind.name());
        None
    };
    let seen = if kind.waits_for_capacity() {
        capacity_for_gate(root, target, wait)?
    } else {
        let (_, more, free) = room(root, target);
        if more == 0 {
            println!("gate: no room for another build now, but a {} gate builds none: going ahead", kind.name());
        }
        (more, free)
    };
    Ok((machine, seen))
}

/// The `CARGO_BUILD_JOBS` gate `sub` sets from `free` memory ([`machine_lock::build_jobs`]), said
/// aloud; `None` when the environment sets it.
pub fn gate_jobs(sub: &str, free: Option<u64>) -> Option<String> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let jobs = machine_lock::build_jobs(std::env::var("CARGO_BUILD_JOBS").ok().as_deref(), free, cores);
    if let Some(j) = &jobs {
        println!("gate {sub}: CARGO_BUILD_JOBS={j} (free memory / 1.5 GB, within 2 and the cores)");
    }
    jobs
}

/// Log an event for a gate: never fails it (offline, it queues); a failure is a warning.
pub fn log_event(project: &Project, root: &Path, args: &[&str]) {
    if let Err(e) = event::run(project, root, args) {
        eprintln!("warning: gate: event {}: {e}", args.first().copied().unwrap_or_default());
    }
}

/// `text` on one line, its whitespace runs single spaces, cut to `max` characters.
pub fn one_line(text: &str, max: usize) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// What [`Hooks::around_ci`] is told about the gate's `ci` run it wraps.
#[derive(Debug, Clone, Copy)]
pub struct CiRun<'a> {
    pub project: &'a Project,
    /// The tree it runs in.
    pub root: &'a Path,
    /// Its target dir (the logs are in `gate-logs` there).
    pub target: &'a Path,
    pub sub: &'a str,
    pub kind: Kind,
    /// Its log, which `ci`'s output goes to.
    pub log: &'a Path,
}

/// What every run of one gate shares: whose (`project`, `hooks`), where, as which agent, its kind
/// (with [`kind_field`]'s record of it), its environment, the target dir its logs go in, the
/// commit it tests, what a failure's `sub.stop` note ends with (the periodic full gate's
/// `; commit=<sha>`), what it reruns when it verifies a failed gate's tests, and the xtask that
/// runs `ci` (this one's exe; a fake consumer's in the tests).
#[derive(Clone, Copy)]
pub struct Gate<'a> {
    pub project: &'a Project,
    pub hooks: &'a dyn Hooks,
    pub root: &'a Path,
    pub agent: &'a str,
    pub kind: Kind,
    pub kind_field: &'a str,
    pub env: &'a [(String, String)],
    pub target: &'a Path,
    pub head: &'a str,
    pub stop_tail: &'a str,
    pub rerun: Option<&'a rerun::Rerun>,
    pub program: &'a Path,
}

impl Gate<'_> {
    /// `ci`'s arguments: the gate's kind's, or a rerun's ([`rerun::Rerun::ci_args`]).
    pub fn ci_args(&self) -> Vec<String> {
        match self.rerun {
            Some(r) => r.ci_args(),
            None => self.kind.ci_args().iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// A run's exit code, cause and `.rc` line, from `ci`'s exit code and log: a rerun's says what
    /// it verified, and is a pass only when it ran every test alone ([`rerun::outcome`]).
    pub fn outcome(&self, rc: i32, log: &str, secs: u64) -> (i32, Option<Cause>, String) {
        run_outcome(self.hooks, self.rerun, rc, log, secs, (self.kind_field, self.head))
    }
}

/// A run's exit code, cause and `.rc` line, from `ci`'s exit code and log, the project's extra
/// causes read first ([`Hooks::extra_causes`]): a rerun's says what it verified, and is a pass only
/// when it ran every test alone ([`rerun::outcome`]). The gate's and `ready`'s
/// ([`crate::ready::own_gate`]).
pub fn run_outcome(hooks: &dyn Hooks, rerun: Option<&rerun::Rerun>, rc: i32, log: &str, secs: u64, (kind, head): (&str, &str)) -> (i32, Option<Cause>, String) {
    let extra = if rc == 0 { None } else { hooks.extra_causes(log) };
    match rerun {
        Some(r) => rerun::outcome(r, rc, log, secs, (kind, head), extra),
        None => {
            let cause = (rc != 0).then(|| classify(log, extra));
            (rc, cause, rc_line(rc, secs, cause, kind, head))
        }
    }
}

/// One run of the gate as sub-agent `sub`: its `sub.start` (unless `note` is `None`: the periodic
/// full gate logs its own first, to race for it), the run, and on a failure its `sub.stop`. The
/// exit code and, on a failure, the cause.
pub fn attempt(g: &Gate<'_>, sub: &str, note: Option<&str>, seen: Seen) -> Result<(i32, Option<Cause>), String> {
    let Gate { project, root, agent, kind, kind_field, target, stop_tail, .. } = *g;
    let dir = target.join(LOG_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let log = dir.join(format!("{sub}.log"));
    let rc_file = dir.join(format!("{sub}.rc"));
    let _ = std::fs::remove_file(&rc_file);
    if let Some(note) = note {
        let note = start_note(note, seen.0, seen.1, &format!("{kind_field}{}", kind.lock_note()));
        log_event(project, root, &["sub.start", "--agent", agent, "--sub", sub, "--name", "gate", "--note", &note]);
    }
    let args = g.ci_args();
    println!("gate {sub}: ci {}, {kind_field}; log {}", args.join(" "), log.display());
    let started = Instant::now();
    let since = SystemTime::now();
    let rc = run_ci(g, sub, &args, &log, gate_jobs(sub, seen.1))?;
    let junit = dir.join(format!("{sub}.junit.xml"));
    match keep_junit(&root.join(JUNIT), &junit, since) {
        Ok(true) => println!("gate {sub}: test times in {} (cargo xtask gate-times {sub})", junit.display()),
        Ok(false) => {}
        Err(e) => eprintln!("warning: gate: the test times: {e}"),
    }
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    print!("{}", summary(&text));
    let (rc, cause, line) = g.outcome(rc, &text, started.elapsed().as_secs());
    if let Err(e) = std::fs::write(&rc_file, line) {
        eprintln!("warning: gate: {}: {e}", rc_file.display());
    }
    let Some(cause) = cause else { return Ok((0, None)) };
    log_event(project, root, &["sub.stop", "--agent", agent, "--sub", sub, "--note", &format!("{}{stop_tail}", failure_note(rc, cause, kind))]);
    println!("gate {sub} failed: rc={rc}, cause {}{}", cause.name(), if cause.transient() { " (the machine's: a rerun may pass)" } else { "" });
    Ok((rc, Some(cause)))
}

/// Run the consumer's `ci <args>` at the gate's root with its program, its whole output in `log`,
/// `jobs` as `CARGO_BUILD_JOBS` (when set) and the gate's environment, through
/// [`Hooks::around_ci`]; its exit code (-1: killed).
fn run_ci(g: &Gate<'_>, sub: &str, args: &[String], log: &Path, jobs: Option<String>) -> Result<i32, String> {
    let file = File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = file.try_clone().map_err(|e| format!("{}: {e}", log.display()))?;
    let mut cmd = Command::new(g.program);
    cmd.current_dir(g.root).arg("ci").args(args).stdin(Stdio::null()).stdout(file).stderr(err);
    cmd.envs(jobs.map(|j| ("CARGO_BUILD_JOBS".to_owned(), j))).envs(g.env.iter().cloned());
    let run = CiRun { project: g.project, root: g.root, target: g.target, sub, kind: g.kind, log };
    g.hooks.around_ci(&run, cmd)
}

/// [`Hooks::around_ci`]'s default: run `cmd` as it is; its exit code (-1: killed).
pub fn run_status(mut cmd: Command) -> Result<i32, String> {
    let status = cmd.status().map_err(|e| format!("gate: ci: {e}"))?;
    Ok(status.code().unwrap_or(-1))
}

/// `e`, saying that the gate lock is kept: a gate that stops once it holds it.
fn lock_kept(e: String) -> String {
    format!("{e}\nthe gate lock is kept; `cargo xtask lock drop` when done")
}

/// `gate --stop <sub>`: its `sub.stop`, with what its `.rc` records ([`stop_note`]: no `rc=` when
/// it has none), and the gate lock dropped. `land` closes its gate so.
pub fn close(project: &Project, root: &Path, agent: &str, sub: &str, note: &str) -> Result<(), String> {
    let rc = capacity::our_target(root).map(|t| t.join(LOG_DIR).join(format!("{sub}.rc")));
    let line = rc.ok().and_then(|p| std::fs::read_to_string(p).ok());
    log_event(project, root, &["sub.stop", "--agent", agent, "--sub", sub, "--note", &stop_note(note, line.as_deref())]);
    lock::drop(project, root, agent, false)?;
    println!("gate {sub} closed; gate lock dropped");
    Ok(())
}

/// The gate's options.
#[derive(Debug, Default, PartialEq, Eq)]
struct Opts {
    sub: Option<String>,
    stop: Option<String>,
    /// `--rerun-failed <sub>`: the failed gate whose tests to rerun alone.
    rerun: Option<String>,
    note: Option<String>,
    agent: Option<String>,
    merge: Vec<String>,
    wait: bool,
    retry: bool,
    full: bool,
}

fn parse_args(args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        let mut value = || it.next().map(|v| (*v).to_owned()).ok_or_else(|| format!("gate: {a} needs a value\n{USAGE_TEXT}"));
        match a {
            "--sub" => o.sub = Some(value()?),
            "--stop" => o.stop = Some(value()?),
            "--rerun-failed" => o.rerun = Some(value()?),
            "--note" => o.note = Some(value()?),
            "--agent" => o.agent = Some(value()?),
            "--merge" => o.merge.push(value()?),
            "--wait" => o.wait = true,
            "--retry-transient" => o.retry = true,
            "--full" => o.full = true,
            other => return Err(format!("gate: unknown argument `{other}`\n{USAGE_TEXT}")),
        }
    }
    check_opts(&o)?;
    Ok(o)
}

/// One of `--sub`, `--rerun-failed` and `--stop`, with the options that go with it, a note (but
/// for a rerun, whose note says what it reruns), ids that can name a file and branches to merge.
fn check_opts(o: &Opts) -> Result<(), String> {
    if [&o.sub, &o.rerun, &o.stop].iter().filter(|m| m.is_some()).count() != 1 {
        return Err(format!("gate: give --sub <id> to run a gate, --rerun-failed <sub> to rerun its failed tests, or --stop <sub> to close one\n{USAGE_TEXT}"));
    }
    if o.sub.is_none() && (o.retry || o.full || !o.merge.is_empty()) {
        return Err(format!("gate: --merge, --retry-transient and --full go with --sub\n{USAGE_TEXT}"));
    }
    if o.stop.is_some() && o.wait {
        return Err(format!("gate: --wait goes with --sub or --rerun-failed\n{USAGE_TEXT}"));
    }
    if o.rerun.is_none() && o.note.as_deref().is_none_or(|n| n.trim().is_empty()) {
        return Err(format!("gate: --note \"<text>\" is needed: what it gates (--sub) or what it merged (--stop)\n{USAGE_TEXT}"));
    }
    for id in o.sub.iter().chain(&o.stop).chain(&o.rerun) {
        event::check_id(id)?;
    }
    o.merge.iter().try_for_each(|b| check_branch(b))
}

/// This xtask's own exe, which runs the gate's `ci` (so cargo never rebuilds the running xtask on
/// Windows).
fn this_xtask() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("gate: this xtask's path: {e}"))
}

/// `cargo xtask gate …` ([`USAGE`]) for `project`.
pub fn run(project: &Project, hooks: &dyn Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    let o = parse_args(args)?;
    let agent = lock::agent(project, o.agent.clone(), "gate")?;
    let note = o.note.clone().unwrap_or_default();
    if let Some(sub) = &o.stop {
        return close(project, root, &agent, sub, &note);
    }
    if let Some(of) = &o.rerun {
        return rerun_failed(project, hooks, root, &agent, of, &o);
    }
    let sub = o.sub.clone().unwrap_or_default();
    if let Some(why) = tree_refusal(root) {
        return Err(why);
    }
    // Before waiting for the machine: a HEAD that isn't the stack's top is refused at once.
    if o.merge.is_empty() {
        stacked_head(project, root)?;
    }
    let target = capacity::our_target(root)?;
    lock::take(project, root, &agent)?;
    let head = tested_head(project, root, &o.merge, &sub)?;
    // The kind before the machine: a docs gate doesn't take the machine lock.
    let (kind, why) = if o.full { coordinator_kind((Kind::Full, String::new()), true) } else { coordinator_kind(hooks.gate_kind(root), false) };
    println!("gate {sub}: the {} gate ({why})", kind.name());
    let field = kind_field(kind, &why);
    let program = this_xtask()?;
    let g = Gate {
        project,
        hooks,
        root,
        agent: &agent,
        kind,
        kind_field: &field,
        env: &[],
        target: &target,
        head: &head,
        stop_tail: "",
        rerun: None,
        program: &program,
    };
    // Held until the run ends: dropped on every way out, pass, failure or error.
    let (rc, cause) = {
        let (_machine, seen) = room_to_build(project, &agent, root, &target, o.wait, kind).map_err(lock_kept)?;
        attempt(&g, &sub, Some(&note), seen)?
    };
    let (sub, rc) = match cause {
        Some(c) if o.retry && c.transient() => {
            let rerun = format!("{sub}-rerun");
            event::check_id(&rerun)?;
            println!("gate: rerunning once as {rerun} (cause {})", c.name());
            let (_machine, seen) = room_to_build(project, &agent, root, &target, true, kind).map_err(lock_kept)?;
            let (rc, _) = attempt(&g, &rerun, Some(&format!("rerun of {sub}: {note}")), seen)?;
            (rerun, rc)
        }
        _ => (sub, rc),
    };
    finish(&sub, rc)
}

/// The last word of a gate run as `sub`: a failure keeps the lock, a pass says how to land.
fn finish(sub: &str, rc: i32) -> Result<(), String> {
    if rc != 0 {
        return Err(format!("gate {sub} failed (rc={rc}): the gate lock is kept; `cargo xtask lock drop` when done"));
    }
    println!("gate {sub} passed: log each merge with --sub {sub}, then `cargo xtask gate --stop {sub} --note \"merged !<m> …\"` (drops the lock)");
    Ok(())
}

/// Gate `of`'s rerun as `sub`, from the records in `dir`: `of`'s `.rc` and log, and `sub`'s `.rc`
/// if an earlier rerun left one ([`rerun::plan`]).
pub fn plan_from_records(project: &Project, dir: &Path, of: &str, sub: &str, head: &str) -> Result<rerun::Rerun, String> {
    let read =
        |name: &str| std::fs::read_to_string(dir.join(name)).map_err(|e| format!("gate: {}: {e}: no failed gate {of} to rerun", dir.join(name).display()));
    let previous = std::fs::read_to_string(dir.join(format!("{sub}.rc"))).ok();
    rerun::plan(project.golden_binary, of, &read(&format!("{of}.rc"))?, &read(&format!("{of}.log"))?, head, previous.as_deref())
        .map_err(|e| format!("gate --rerun-failed refused: {e}"))
}

/// `gate --rerun-failed <of>`: gate `of`'s failed tests, each alone, then the steps its failure
/// skipped, on the HEAD it tested, as `<of>-rerun`; [`rerun::plan`] decides whether there is a
/// rerun path, and the record is a pass only when every one passes. It takes the gate lock and,
/// for a kind that builds, the machine's.
fn rerun_failed(project: &Project, hooks: &dyn Hooks, root: &Path, agent: &str, of: &str, o: &Opts) -> Result<(), String> {
    if let Some(why) = tree_refusal(root) {
        return Err(why);
    }
    let head = stack::rev(root, "HEAD")?;
    let target = capacity::our_target(root)?;
    let sub = format!("{of}-rerun");
    event::check_id(&sub)?;
    let plan = plan_from_records(project, &target.join(LOG_DIR), of, &sub, &head)?;
    lock::take(project, root, agent)?;
    let tests: Vec<String> = plan.tests.iter().map(rerun::Test::spec).collect();
    println!("gate {sub}: {of}'s failed tests, each alone ({}), then the steps it skipped ({})", tests.join(", "), plan.steps.join(", "));
    let field = kind_field(plan.kind, &format!("rerun of {of}"));
    let extra = o.note.as_deref().map(|n| format!("; {}", n.trim())).unwrap_or_default();
    let note = format!("rerun of {of}: its failed tests alone ({}), then the steps it skipped{extra}", plan.tests.len());
    let program = this_xtask()?;
    let g = Gate {
        project,
        hooks,
        root,
        agent,
        kind: plan.kind,
        kind_field: &field,
        env: &[],
        target: &target,
        head: &head,
        stop_tail: "",
        rerun: Some(&plan),
        program: &program,
    };
    let (_machine, seen) = room_to_build(project, agent, root, &target, o.wait, plan.kind).map_err(lock_kept)?;
    let (rc, _) = attempt(&g, &sub, Some(&note), seen)?;
    finish(&sub, rc)
}

/// One run of the periodic full gate's `ci` ([`crate::fullgate`]): this gate, [`Kind::Full`], in
/// the full gate's checkout, with this xtask's own exe (`main`'s, handed the gate), waiting for the
/// machine and room; a rerun's start logged here, and a rerun of failed tests alone planned from
/// the run it reruns. `fullgate` decides what comes next.
pub fn full_attempt(project: &Project, hooks: &dyn Hooks, a: &fullgate::Attempt<'_>) -> Result<fullgate::Ran, String> {
    let program = this_xtask()?;
    full_attempt_with(project, hooks, a, &program)
}

/// [`full_attempt`] with `program` running `ci`.
pub fn full_attempt_with(project: &Project, hooks: &dyn Hooks, a: &fullgate::Attempt<'_>, program: &Path) -> Result<fullgate::Ran, String> {
    let logs = a.target.join(LOG_DIR);
    let rerun = a.tests_of.map(|of| plan_from_records(project, &logs, of, a.sub, a.commit)).transpose()?;
    let field = fullgate::tail(a.commit).trim_start_matches("; ").to_owned();
    let stop_tail = format!("; commit={}", a.commit);
    let g = Gate {
        project,
        hooks,
        root: a.checkout,
        agent: a.agent,
        kind: Kind::Full,
        kind_field: &field,
        env: a.env,
        target: a.target,
        head: a.commit,
        stop_tail: &stop_tail,
        rerun: rerun.as_ref(),
        program,
    };
    let (rc, cause) = {
        let (_machine, seen) = room_to_build(project, a.agent, a.checkout, a.target, true, Kind::Full)?;
        if let Some(what) = a.start {
            let note = fullgate::start_note(what, seen.0, seen.1, a.commit, None);
            log_event(project, a.checkout, &["sub.start", "--agent", a.agent, "--sub", a.sub, "--name", "gate", "--note", &note]);
        }
        attempt(&g, a.sub, None, seen)?
    };
    Ok(ran_from_records(project, &logs, a.sub, a.commit, rc, cause))
}

/// How the full gate's run `sub` of `commit` went: its exit code and cause, and, when it failed,
/// the failed tests its log names and how many of them a rerun alone could take, from its records
/// in `logs` (why none can is said).
pub fn ran_from_records(project: &Project, logs: &Path, sub: &str, commit: &str, rc: i32, cause: Option<Cause>) -> fullgate::Ran {
    let log = logs.join(format!("{sub}.log"));
    if rc == 0 {
        return fullgate::Ran { rc, cause: None, rerun_tests: None, failed: Vec::new(), log };
    }
    let plan = plan_from_records(project, logs, sub, &format!("{sub}-rerun"), commit)
        .map_err(|e| println!("fullgate: no rerun of {sub}'s failed tests alone: {e}"))
        .ok();
    let failed = rerun::failed_names(&std::fs::read_to_string(&log).unwrap_or_default());
    fullgate::Ran { rc, cause, rerun_tests: plan.map(|p| p.tests.len()), failed, log }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::APP_SWARM as P;

    #[test]
    fn failures_are_classified_by_cause() {
        let cases = [
            (
                "error: could not compile `swarm-engine`\nwarning: build failed\nThe paging file is too small for this operation to complete. (os error 1455)",
                Cause::Memory,
            ),
            ("error: failed to write: Insufficient system resources exist to complete the requested service. (os error 1450)", Cause::Memory),
            ("error: linking with `link.exe` failed: exit code: 0xc000009a", Cause::Memory),
            ("LINK : fatal error LNK1104: cannot open file 'swarm_cli.exe'\nerror: could not compile `swarm-cli`", Cause::Link),
            ("error[E0463]: can't find crate for `std`\nerror: could not compile `serde`", Cause::Toolchain),
            ("error[E0463]: can't find crate for `core`", Cause::Toolchain),
            ("error: only metadata stub found for `rlib` dependency `serde`", Cause::Toolchain),
            ("fatal: Unable to create '/tmp/x/.git/index.lock': File exists.\ntest result: FAILED. 1 passed; 1 failed", Cause::GitTemp),
            ("fatal: Could not read from remote repository.\n        FAIL [   0.2s] xtask event::tests::send", Cause::GitTemp),
            ("error: cannot fork: Function not implemented", Cause::GitTemp),
            ("fatal: index-pack failed\ntest result: FAILED. 0 passed; 1 failed", Cause::GitTemp),
            (
                "git [\"fetch\"] failed: fatal: error reading 'C:/Users/x/AppData/Local/Temp/swarm-test-36668-golden/origin.git/.git'\ntest result: FAILED. 1 passed; 1 failed",
                Cause::GitTemp,
            ),
            ("fatal: not a git repository: '/home/x/project'\ntest result: FAILED. 0 passed; 1 failed", Cause::Test),
            ("        FAIL [   1.2s] swarm-engine::panic_hunt every_command\nerror: test run failed", Cause::Test),
            ("test result: FAILED. 10 passed; 1 failed", Cause::Test),
            ("error[E0308]: mismatched types\nerror: could not compile `swarm-model`", Cause::Build),
            ("error: this `if` has identical blocks\nerror: could not compile `xtask` due to 1 previous error", Cause::Build),
            ("Diff in xtask/src/gate.rs at line 3:", Cause::Other),
            // A passing test whose name says "timeout" is not a timeout.
            ("        PASS [   0.353s] (  52/1635) gpu-capture settings::tests::the_hang_watchdog_follows_its_options_and_timeout", Cause::Other),
            ("", Cause::Other),
        ];
        for (log, want) in cases {
            assert_eq!(classify(log, None), want, "{log}");
        }
        // The project's own cause comes first (`gpu`, `python`).
        assert_eq!(classify("os error 1455\nback to WARP", Some(Cause::Gpu)), Cause::Gpu);
        assert_eq!(classify("", Some(Cause::Python)), Cause::Python);
        for c in Cause::ALL {
            assert_eq!(Cause::parse(c.name()), Some(c));
        }
        let transient: Vec<&str> = Cause::ALL.into_iter().filter(|c| c.transient()).map(Cause::name).collect();
        assert_eq!(transient, ["memory", "link", "toolchain", "git-temp", "gpu"]);
        assert!(Cause::Python.setup() && !Cause::Python.transient() && !Cause::Test.setup());
    }

    /// The coordinator smokes what needed the full gate, and what it is unsure of; the docs and
    /// tooling gates stay, and `--full` forces the full gate.
    #[test]
    fn the_coordinator_smokes_what_needed_the_full_gate() {
        let smoke = |why: &str| (Kind::Smoke, why.to_owned());
        assert_eq!(coordinator_kind((Kind::Full, "reaches model".into()), false), smoke("reaches model"));
        assert_eq!(coordinator_kind(crate::Defaults.gate_kind(Path::new(".")), false), smoke("the project plans no gate kind: unsure"));
        assert_eq!(coordinator_kind((Kind::Docs, "x".into()), false).0, Kind::Docs);
        assert_eq!(coordinator_kind((Kind::Tooling, "x".into()), false).0, Kind::Tooling);
        for k in Kind::ALL {
            assert_eq!(coordinator_kind((k, "x".into()), true), (Kind::Full, "--full".into()));
            assert_eq!(Kind::parse(k.name()), Some(k));
        }
        assert_eq!(Kind::Tooling.ci_args(), ["--changed", "--base", "origin/main"]);
        assert_eq!(Kind::Smoke.ci_args(), ["--smoke"]);
        assert!(Kind::Full.ci_args().is_empty());
        assert!(Kind::Docs < Kind::Tooling && Kind::Tooling < Kind::Smoke && Kind::Smoke < Kind::Full);
    }

    /// Only the gates that build take the machine gate lock and wait for build capacity; a docs
    /// gate (xtask alone) waits for neither, and its note says so.
    #[test]
    fn only_the_gates_that_build_take_the_machine_lock_and_wait_for_capacity() {
        let taking: Vec<&str> = Kind::ALL.into_iter().filter(|k| k.takes_machine_lock()).map(Kind::name).collect();
        assert_eq!(taking, ["tooling", "smoke", "full"]);
        let waiting: Vec<&str> = Kind::ALL.into_iter().filter(|k| k.waits_for_capacity()).map(Kind::name).collect();
        assert_eq!(waiting, ["tooling", "smoke", "full"]);
        assert!(Kind::ALL.into_iter().all(|k| k.takes_machine_lock() == k.builds() && k.waits_for_capacity() == k.builds()), "one rule decides both");
        assert_eq!((Kind::Docs.lock_note(), Kind::Full.lock_note()), ("; machineLock=skipped", ""));
    }

    #[test]
    fn the_notes_follow_the_convention() {
        let full = kind_field(Kind::Full, "reaches engine");
        assert_eq!(full, "kind=full (reaches engine)");
        assert_eq!(kind_field(Kind::Docs, " "), "kind=docs");
        assert_eq!(start_note("local gate: #184", 2, Some(11 << 30), &full), "local gate: #184; capacity=2 free=11.0; kind=full (reaches engine)");
        let docs = format!("{}{}", kind_field(Kind::Docs, "builds no crate"), Kind::Docs.lock_note());
        assert_eq!(start_note("docs gate: #9", 1, None, &docs), "docs gate: #9; capacity=1 free=?; kind=docs (builds no crate); machineLock=skipped");
        let long = start_note(&"x".repeat(500), 3, Some(0), &kind_field(Kind::Tooling, &"y".repeat(100)));
        assert_eq!(long.chars().count(), event::MAX_NOTE);
        assert!(long.contains("; capacity=3 free=0.0; kind=tooling (yyy"), "the convention survives a long note: {long}");
        assert_eq!(failure_note(101, Cause::GitTemp, Kind::Full), "rc=101; cause=git-temp; kind=full");
        let head = "d".repeat(40);
        let passed = rc_line(0, 412, None, &kind_field(Kind::Tooling, "builds only xtask"), &head);
        assert_eq!(stop_note("merged #184 #185", Some(&passed)), "rc=0; kind=tooling; merged #184 #185");
        assert_eq!(stop_note("merged #184 #185", Some(&format!("rc=0 412s head={head}"))), "rc=0; merged #184 #185", "a .rc from before kinds");
        assert_eq!(stop_note("rc=0; merged #184", Some(&passed)), "rc=0; merged #184");
        assert_eq!(stop_note(" ", Some(&rc_line(0, 1, None, "kind=docs", &head))), "rc=0; kind=docs");
        let failed = rc_line(101, 30, Some(Cause::Test), "kind=full", &head);
        assert_eq!(stop_note("stopped", Some(&failed)), "rc=101; cause=test; kind=full; stopped", "a failure's rc, not 0");
        // mornycorp/swarm-core#3: a gate stopped part way, before its `.rc`, never passed.
        assert_eq!(stop_note("stopped: the owner paused test runs", None), "stopped: the owner paused test runs");
        assert_eq!(stop_note(" ", None), "stopped before its result");
        assert_eq!(stop_note("stopped", Some("garbage")), "stopped", "a .rc with no rc is none");
    }

    #[test]
    fn arguments_are_checked() {
        assert_eq!(
            parse_args(&["--sub", "gate-mr1", "--note", "local gate: #1", "--wait", "--retry-transient"]),
            Ok(Opts { sub: Some("gate-mr1".into()), note: Some("local gate: #1".into()), wait: true, retry: true, ..Opts::default() })
        );
        assert_eq!(parse_args(&["--sub", "g", "--note", "x", "--full"]).map(|o| o.full), Ok(true));
        assert_eq!(parse_args(&["--sub", "g", "--note", "x", "--merge", "1-a", "--merge", "2-b"]).map(|o| o.merge), Ok(vec!["1-a".into(), "2-b".into()]));
        assert!(parse_args(&["--stop", "gate-mr1", "--note", "merged #1"]).is_ok());
        // A rerun needs no note (it says what it reruns), and may wait.
        assert_eq!(parse_args(&["--rerun-failed", "gate-mr1", "--wait"]), Ok(Opts { rerun: Some("gate-mr1".into()), wait: true, ..Opts::default() }));
        for (bad, why) in [
            (&["--note", "x"][..], "give --sub"),
            (&["--sub", "a", "--stop", "b", "--note", "x"][..], "give --sub"),
            (&["--sub", "a", "--rerun-failed", "b", "--note", "x"][..], "--rerun-failed <sub>"),
            (&["--stop", "a", "--note", "x", "--wait"][..], "goes with --sub"),
            (&["--stop", "a", "--note", "x", "--full"][..], "go with --sub"),
            (&["--stop", "a", "--note", "x", "--merge", "b"][..], "go with --sub"),
            (&["--rerun-failed", "a", "--retry-transient"][..], "go with --sub"),
            (&["--rerun-failed", "../a"][..], "can't name"),
            (&["--sub", "a"][..], "--note"),
            (&["--sub", "a", "--note", "  "][..], "--note"),
            (&["--sub", "../a", "--note", "x"][..], "can't name"),
            (&["--sub", "a", "--note", "x", "--merge", "-f"][..], "not a branch to merge"),
            (&["--sub", "a", "--note", "x", "--merge", "a..b"][..], "not a branch to merge"),
            (&["--sub"][..], "needs a value"),
            (&["--fast"][..], "unknown argument"),
        ] {
            let e = parse_args(bad).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
    }

    /// Through git: a tree in the middle of a rebase, or with a conflict marker, is not gated.
    #[test]
    fn a_tree_mid_rebase_or_with_conflict_markers_is_refused() {
        use swarm_core_testkit::git::git;
        let dir = swarm_core_testkit::temp_dir("swarm-core-gate-tree");
        let repo = dir.join("repo");
        git(&dir, &["init", "--quiet", "-b", "main", repo.to_str().unwrap()]);
        std::fs::write(repo.join("a.md"), "one\n").unwrap();
        git(&repo, &["add", "a.md"]);
        git(&repo, &["commit", "--quiet", "-m", "one"]);
        assert_eq!(tree_refusal(&repo), None, "a clean tree is gated");
        git(&repo, &["checkout", "--quiet", "-b", "topic"]);
        std::fs::write(repo.join("a.md"), "topic\n").unwrap();
        git(&repo, &["commit", "--quiet", "-am", "topic"]);
        git(&repo, &["checkout", "--quiet", "main"]);
        std::fs::write(repo.join("a.md"), "main\n").unwrap();
        git(&repo, &["commit", "--quiet", "-am", "main"]);
        git(&repo, &["checkout", "--quiet", "topic"]);
        let id = [("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")];
        assert!(crate::process::git(&repo, &["rebase", "main"], None, &id).is_err(), "the rebase conflicts");
        assert!(tree_refusal(&repo).unwrap().contains("rebase in progress"));
        git(&repo, &["rebase", "--abort"]);
        assert_eq!(tree_refusal(&repo), None);
        // A marker committed by mistake (built here, so this file holds none at a line's start).
        std::fs::write(repo.join("b.md"), format!("{} HEAD\nx\n", "<".repeat(7))).unwrap();
        git(&repo, &["add", "b.md"]);
        assert!(tree_refusal(&repo).unwrap().contains("conflict markers in b.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Through git, with a bare `origin` (ns): the branches merge onto `origin/main` in order; one
    /// that conflicts stops the gate and leaves no merge in progress; a dirty worktree is refused.
    #[test]
    fn branches_merge_onto_origin_main_in_order() {
        use crate::stack::testgit;
        let dir = swarm_core_testkit::temp_dir("swarm-core-gate-merge");
        let (origin, gate) = (dir.join("origin.git"), dir.join("gate"));
        testgit::bare(&origin);
        let mut h = testgit::History::default();
        let base = h.commit("main", &[], &[("a.md", "base\n")], "base");
        for (branch, file, text) in [("1-one", "one.md", "one\n"), ("2-two", "two.md", "two\n"), ("3-clash", "one.md", "clash\n")] {
            h.commit(branch, &[&base], &[(file, text)], branch);
        }
        h.import(&origin);
        testgit::clone(&origin, &gate);
        let head = merge_branches(&gate, &["1-one".into(), "2-two".into()]).unwrap();
        assert!(head.contains("Merge"), "{head}");
        assert!(gate.join("one.md").exists() && gate.join("two.md").exists(), "both branches merged");
        let e = merge_branches(&gate, &["1-one".into(), "3-clash".into()]).unwrap_err();
        assert!(e.contains("merge of 3-clash conflicts"), "{e}");
        assert_eq!(tree_refusal(&gate), None, "the conflicting merge was aborted");
        assert_eq!(std::fs::read_to_string(gate.join("one.md")).unwrap().replace('\r', ""), "one\n", "left at what merged before it");
        std::fs::write(gate.join("a.md"), "edited\n").unwrap();
        assert!(merge_branches(&gate, &["1-one".into()]).unwrap_err().contains("uncommitted changes"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The gate tests the top of the worktree's stack record, and says which HEAD it tested.
    #[test]
    fn a_head_that_isnt_the_stacks_top_is_refused() {
        let entry = |mr: u64, new_tip: &str| stack::Entry { mr, source_branch: format!("{mr}-x"), new_tip: new_tip.to_owned(), ..stack::Entry::default() };
        let top = "b".repeat(40);
        let record = Stack { remote: "origin".into(), base: "main".into(), entries: vec![entry(366, &"a".repeat(40)), entry(368, &top)] };
        assert_eq!(stack_refusal(Some(&record), &top), None);
        assert_eq!(stack_refusal(None, &top), None, "no record: nothing to check");
        assert_eq!(stack_refusal(Some(&Stack::default()), &top), None);
        for head in ["c".repeat(40), "a".repeat(40)] {
            let e = stack_refusal(Some(&record), &head).unwrap();
            assert!(e.contains(&format!("HEAD is {}", short(&head))) && e.contains("top is bbbbbbb (s-368, #368)") && e.contains("--merge"), "{e}");
        }
    }

    #[test]
    fn the_rc_file_names_the_head_and_only_a_pass_has_one_to_land() {
        let head = "d".repeat(40);
        let tooling = kind_field(Kind::Tooling, "builds only xtask");
        assert_eq!(rc_line(0, 412, None, &tooling, &head), format!("rc=0 412s kind=tooling (builds only xtask) head={head}\n"));
        assert_eq!(rc_line(101, 30, Some(Cause::Test), "kind=full", &head), format!("rc=101 30s cause=test kind=full head={head}\n"));
        assert_eq!(passed_head(&rc_line(0, 412, None, &tooling, &head)), Ok(head.as_str()));
        assert!(passed_head(&rc_line(101, 30, Some(Cause::Test), "kind=full", &head)).unwrap_err().contains("did not pass"));
        // A full gate whose kind has free text first, `;`-separated.
        assert_eq!(passed_head("rc=0; kind=full (why head=x); commit=c head=def5678"), Ok("def5678"));
        // `gate --stop` reads the kind back for its note; a gate from before it has none.
        assert_eq!(rc_kind(&rc_line(0, 412, None, &tooling, &head)), Some(Kind::Tooling));
        assert_eq!(rc_kind(&format!("rc=0 412s head={head}\n")), None);
        assert_eq!(rc_kind("rc=0 1s kind=weird head=x"), None);
        assert!(passed_head("rc=0 412s\n").unwrap_err().contains("names no HEAD"));
        assert!(passed_head("rc=0 1s head=\n").unwrap_err().contains("names no HEAD"));
        assert!(passed_head("").is_err());
        assert!(passed_head("rc=00 1s head=x").is_err());
    }

    #[test]
    fn a_reruns_evidence_names_its_tests_and_the_original_summary() {
        let rc = "rc=0 head=abc of=g-1 rerun=xtask::bin/xtask%20a::t,xtask::bin/xtask%20b::u";
        let log = "…\nCI summary: FAIL test: 2 tests failed\n  skip layers\n";
        let e = evidence(rc, Some(log)).unwrap();
        assert!(e.starts_with("Verified by a rerun of gate `g-1`"), "{e}");
        assert!(e.contains("  xtask::bin/xtask%20a::t\n") && e.contains("g-1's CI summary: FAIL test"), "{e}");
        assert_eq!(evidence("rc=0 head=abc", None), None, "not a rerun");
        let dir = swarm_core_testkit::temp_dir("swarm-core-gate-evidence");
        std::fs::write(dir.join("g-1.log"), log).unwrap();
        assert!(rerun_evidence(&dir, rc).unwrap().contains("g-1's CI summary"));
        assert!(rerun_evidence(&dir, "rc=0 head=a of=../x rerun=t").unwrap().ends_with("  t\n"), "a hostile `of` reads no file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The run's junit file is kept beside its log; an earlier run's is not, and replaces nothing.
    #[test]
    fn a_runs_junit_is_kept_and_an_earlier_runs_is_not() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-gate-junit");
        let (from, to) = (dir.join("junit.xml"), dir.join("gate-1.junit.xml"));
        let before = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(keep_junit(&from, &to, before), Ok(false), "no test step ran: no file");
        std::fs::write(&from, "<testsuites/>").unwrap();
        assert_eq!(keep_junit(&from, &to, before), Ok(true));
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "<testsuites/>");
        let later = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(keep_junit(&from, &to, later), Ok(false), "an earlier run's file");
        assert!(!to.exists(), "and the stale copy is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #11: a file stamped a few milliseconds before the gate started (a coarse file clock) is
    /// this run's, and kept; one a minute older is an earlier run's.
    #[test]
    fn a_file_time_just_behind_the_start_is_this_runs() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-gate-junit-slack");
        let (from, to) = (dir.join("junit.xml"), dir.join("gate-1.junit.xml"));
        let since = SystemTime::now();
        std::fs::write(&from, "<testsuites/>").unwrap();
        let file = std::fs::File::options().write(true).open(&from).unwrap();
        file.set_modified(since - Duration::from_millis(4)).unwrap();
        assert_eq!(keep_junit(&from, &to, since), Ok(true), "a coarse tick behind: kept");
        file.set_modified(since - Duration::from_secs(60)).unwrap();
        assert_eq!(keep_junit(&from, &to, since), Ok(false), "a minute behind: an earlier run's");
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_failing_tests_are_listed_once() {
        let log = "        PASS [   0.1s] (  1/9) a::b ok\n        FAIL [   7.681s] (1621/1635) gpu-vk-layer::map_memory2 writes_through\n\
                   --- STDOUT ---\n        FAIL [   7.681s] (1621/1635) gpu-vk-layer::map_memory2 writes_through\n        FAIL [   0.2s] xtask::bin/xtask event::tests::send\n  FAIL  test: cargo nextest run";
        assert_eq!(failed_tests(log), ["gpu-vk-layer::map_memory2 writes_through", "xtask::bin/xtask event::tests::send"]);
        assert!(failed_tests("FAIL [ no bracket end").is_empty());
    }

    #[test]
    fn the_summary_shows_failures_the_failing_tests_and_the_ci_summary() {
        let log = "Compiling a\nerror[E0308]: mismatched\nmore\n        FAIL [   1.0s] (1/2) a::b c\nCI summary:\n  ok    fmt\n  FAIL  test: x\n  skip  layers";
        assert_eq!(summary(log), "  error[E0308]: mismatched\n  failed test: a::b c\n  CI summary:\n    ok    fmt\n    FAIL  test: x\n    skip  layers\n");
        assert_eq!(summary("a\nb\nc\nd"), "  b\n  c\n  d\n", "no CI summary: the tail");
    }

    /// The step a log failed at is its last `CI summary`'s first failed one, a name fit for a
    /// note's `step=`; none without a summary, a failure, or a name that is one.
    #[test]
    fn the_failed_step_is_read_from_the_last_ci_summary() {
        let summary = "CI summary:\n  ok    fmt\n  FAIL  clippy: cargo clippy failed\n  skip  test\n";
        assert_eq!(failed_step(summary), Some("clippy"));
        assert_eq!(failed_step(&format!("{summary}\nCI summary:\n  ok    fmt\n  FAIL  golden-1.x: cargo test\n")), Some("golden-1.x"), "the last summary's");
        assert_eq!(failed_step("CI summary: all 3 steps passed (fmt, clippy, test) in 2 min\n"), None);
        assert_eq!(failed_step("  FAIL  test: no summary before it\n"), None);
        for odd in ["two words", "semi;colon", "cause=test", "", &"x".repeat(33)] {
            assert_eq!(failed_step(&format!("CI summary:\n  FAIL  {odd}: x\n")), None, "{odd:?}");
        }
    }

    /// The full gate reads a run from its records: a pass; tests that failed under load, named,
    /// which a rerun alone can take; a build failure, which none can, nor a record of another
    /// commit.
    #[test]
    fn a_full_gate_run_reads_back_from_its_records() {
        let logs = swarm_core_testkit::temp_dir("swarm-core-gate-full-ran");
        let (sub, head) = ("gate-full-aaaaaaa", "a".repeat(40));
        let failed = "swarm-engine::panic_hunt every_command";
        let log = format!(
            "=== ci: test ===\n     Summary [ 314.665s] 921 tests run: 920 passed, 1 failed, 0 skipped\n        FAIL [  49.697s] (1/921) {failed}\nerror: test run failed\n\nCI summary:\n  ok    fmt\n  ok    clippy\n  FAIL  test: cargo nextest run failed\n  skip  layers\n  skip  wasm\n"
        );
        std::fs::write(logs.join(format!("{sub}.log")), &log).unwrap();
        let full = fullgate::tail(&head).trim_start_matches("; ").to_owned();
        std::fs::write(logs.join(format!("{sub}.rc")), rc_line(100, 763, Some(Cause::Test), &full, &head)).unwrap();
        let ran = ran_from_records(&P, &logs, sub, &head, 100, Some(Cause::Test));
        assert_eq!((ran.rc, ran.cause, ran.rerun_tests), (100, Some(Cause::Test), Some(1)));
        assert_eq!((ran.failed, ran.log), (vec![failed.to_owned()], logs.join(format!("{sub}.log"))));
        assert_eq!(ran_from_records(&P, &logs, sub, &"b".repeat(40), 100, Some(Cause::Test)).rerun_tests, None, "main moved: not this commit's record");
        let passed = ran_from_records(&P, &logs, sub, &head, 0, None);
        assert_eq!((passed.rc, passed.cause, passed.rerun_tests, passed.failed.len()), (0, None, None, 0));
        std::fs::write(logs.join(format!("{sub}.rc")), rc_line(101, 60, Some(Cause::Build), &full, &head)).unwrap();
        let build = ran_from_records(&P, &logs, sub, &head, 101, Some(Cause::Build));
        assert_eq!((build.cause, build.rerun_tests), (Some(Cause::Build), None), "a build failure has no rerun path");
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn one_line_squeezes_and_cuts() {
        assert_eq!(one_line("a\n  b\tc", 100), "a b c");
        assert_eq!(one_line("héllo wörld", 4), "héll");
    }
}
