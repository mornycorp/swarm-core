//! `cargo xtask event`: the swarm's timeline (a timeline viewer reads this format,
//! so the writer and the reader must agree: [docs/tracking-format.md](../../../docs/tracking-format.md)
//! is the contract). Every agent instance, worker or coordinator, logs when it starts and stops,
//! and when it claims, works on, submits, stops or releases an issue, so the owner's tracker can
//! tell which agent worked on which ticket, when.
//!
//! - **Where:** the project's tracking branch ([`crate::project::Branches::tracking`], never
//!   merged; no CI runs on it), one file per agent instance, `tracking/<instance>/work.jsonl`. An
//!   instance writes only its own file, so two agents never conflict: a push that loses a race
//!   fetches and tries again.
//! - **Format:** JSON Lines, one event a line: `t` (UTC, `YYYY-MM-DDTHH:MM:SSZ`), `event`, and
//!   `issue`, `mr`, `reason`, `note` as they apply; `session.start` also has `name`, `role`,
//!   `machine` and, for a session another started, `parent`; a helper sub-agent is logged by its
//!   parent (`sub.start`, `sub.stop`, `sub`); a `release` ending another's work names it (`of`),
//!   as does a `session.stop` ending a dead agent's session; `session.start` and `sub.start` may
//!   name the `model` and its `effort`; `session.start` names the format version, [`FORMAT`].
//!   [`README`] is the branch's own description. A line's keys are in sorted order, as
//!   [`Event::line`] writes them, whatever serde_json's features.
//! - **How:** git plumbing, no checkout, index or ref of its own (so any checkout or worktree of
//!   the clone can log while it works): read the branch, append, commit with `fast-import`, push. Nothing writes a local ref ([`fetch_tip`], [`commit`]),
//!   so worktrees logging at once never race for one. A claim that holds its issue here first
//!   sends its event only onto the branch it judged ([`send_unless`], mornycorp/swarm-core#96):
//!   a push that loses a race reads the branch and judges again, rather than appending.
//! - **Never in the way:** each git call has a deadline; on any failure the event is queued in
//!   the clone's git directory and sent with the next one, and the command still succeeds. Only a
//!   malformed event (an unknown name, a missing issue, a field the reader would drop) is an
//!   error. `<PREFIX>_TRACKING=off` (`APP_SWARM_TRACKING`, `GPU_SWARM_TRACKING`) turns it off.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::Project;
use crate::process::{git, git_bytes};

/// Every event an agent logs.
pub const EVENTS: &[&str] = &[
    "session.start",
    "session.pause",
    "session.resume",
    "session.stop",
    "sub.start",
    "sub.stop",
    "claim",
    "work",
    "submit",
    "stop",
    "release",
    "merge",
    "sendback",
];
/// Why an agent stops working on an issue (`stop`).
pub const STOP_REASONS: &[&str] = &["handoff", "blocked", "paused", "done"];
/// The reason that throws work away: another agent has the issue. Only a `release`
/// or a `session.stop` can carry it; the reader counts it on nothing else.
pub const DUPLICATE: &str = "duplicate";
/// The flags `cargo xtask event` takes, each with a value.
const FLAGS: &[&str] = &["agent", "issue", "mr", "reason", "note", "name", "role", "parent", "sub", "of", "model", "effort"];
/// The tracking format version this writer writes as `format` on `session.start`,
/// as a reader checks it.
/// docs/tracking-format.md › Versions is the contract and its changelog; a change to the format
/// bumps it there and here.
pub const FORMAT: u32 = 3;
/// The format's contract, `docs/tracking-format.md` in this repo, as a reader's tests check it
/// (a reader checks its versions table against this text).
pub const FORMAT_DOC: &str = include_str!("../../../docs/tracking-format.md");
/// The reasoning efforts `--effort` takes, as `swarm_model::event::EFFORTS`.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The roles an agent plays.
pub const ROLES: &[&str] = &["worker", "coordinator"];
/// Longest `note`, in characters.
pub const MAX_NOTE: usize = 200;
/// Longest instance id.
pub const MAX_ID: usize = 64;
/// Pushes tried before the event is queued; also fetches tried while another worktree holds the ref.
const ATTEMPTS: usize = 8;

/// The branch's README, written by the first event pushed to a new branch.
pub const README: &str = "\
# Swarm tracking

The swarm's timeline: every agent instance (worker or coordinator) logs its sessions and
its work on issues with `cargo xtask event` (AGENTS.md › Tracking). This branch is never merged.

- `tracking/<instance>/work.jsonl`: one agent instance's events, one JSON object a line, oldest
  first. Only that instance writes it.
- Every event: `t` (UTC, `YYYY-MM-DDTHH:MM:SSZ`, the agent's clock) and `event`.
- `session.start`: `name` (the agent's stable name, such as `claude-mac` or `coordinator`),
  `role` (`worker` or `coordinator`), `machine`, and `parent`: the instance id of the agent that
  started this session (a session the coordinator starts); none for a top-level agent. A tracker
  nests an instance under its parent. Optional `model` (the model running the session, such as
  `claude-opus-5-5`) and `effort` (`low`, `medium`, `high`, `xhigh` or `max`), for comparing
  models; a tracker shows `unknown` where they are missing. `format`: the version of this format
  the writer wrote (a whole number; none means 1). A change to the format bumps it.
- `sub.start` (`sub`, `name`?, `model`?, `effort`?), `sub.stop` (`sub`): a helper sub-agent inside this instance's
  session (a search, a review, a gate) starts and ends. Sub-agents have no file of their own:
  their parent logs them here, and any issue event may carry `sub` to say the sub-agent did it.
  `sub` is unique within the file.
- `session.pause`, `session.resume`, `session.stop` (`reason`?, `of`?): the instance pauses,
  resumes, ends; `reason` `duplicate`: because another agent has its issue. `of`: the dead
  instance whose session this stop ends (the coordinator closing a dead agent's session).
- `claim` (`issue`, `mr`): it takes an issue. `work` (`issue`, `mr`?): it starts or resumes work
  on it; with `mr`, it picks up that PR sent back (`sendback`, or a failed gate naming it): its
  rework starts here, and the time since the send-back was waiting.
- `submit` (`issue`, `mr`): it marks the PR ready for merge.
- `stop` (`issue`, `reason`: handoff, blocked, paused or done): it stops working on the issue.
- `release` (`issue`, `reason`?, `of`?): it gives up the issue (lost a claim race, gave up), or the
  coordinator released a stale claim. `reason` `duplicate`: another agent has the issue, so the
  work so far is thrown away. `of`: the instance whose claim or work this release ends, when it
  isn't the logger's own (the coordinator stopping another agent's work).
- `merge`, `sendback` (`issue`, `mr`): the coordinator merged a PR or sent it back.
- Any event may carry a one-line `note`.
";

/// One event, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub event: String,
    pub issue: Option<u64>,
    pub mr: Option<u64>,
    pub reason: Option<String>,
    pub note: Option<String>,
    /// `session.start` only: the agent's stable name, its role and the machine.
    pub name: Option<String>,
    pub role: Option<String>,
    pub machine: Option<String>,
    /// `session.start` only: the instance that started this one.
    pub parent: Option<String>,
    /// The helper sub-agent the event is about or by, logged by its parent.
    pub sub: Option<String>,
    /// `release` and `session.stop` only: the instance whose claim, work or session
    /// it ends, when not the logger's.
    pub of: Option<String>,
    /// `session.start` and `sub.start` only: the model running it and its reasoning effort.
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl Event {
    /// An event named `event` with no fields: the start of one built by hand.
    pub fn named(event: &str) -> Event {
        Event {
            event: event.into(),
            issue: None,
            mr: None,
            reason: None,
            note: None,
            name: None,
            role: None,
            machine: None,
            parent: None,
            sub: None,
            of: None,
            model: None,
            effort: None,
        }
    }

    /// Check an event: a known name, an issue where it needs one, a reason where it needs one,
    /// and no field the reader would drop or ignore where it is.
    pub fn check(&self) -> Result<(), String> {
        let e = self.event.as_str();
        if !EVENTS.contains(&e) {
            return Err(format!("unknown event `{e}` (expected one of {})", EVENTS.join(", ")));
        }
        let session = e.starts_with("session.");
        let sub_life = e.starts_with("sub.");
        if !session && !sub_life && self.issue.is_none() {
            return Err(format!("`{e}` needs --issue <n>"));
        }
        self.check_sub(e)?;
        check_link(
            self.of.as_deref(),
            "--of",
            e,
            &["release", "session.stop"],
            "--of belongs on `release` (the instance whose work it ends) or `session.stop` (the dead instance whose session it ends)",
        )?;
        check_link(self.parent.as_deref(), "--parent", e, &["session.start"], "--parent belongs on `session.start`: the instance that started this session")?;
        self.check_model(e)?;
        self.check_names(e)?;
        if self.reason.as_deref() == Some(DUPLICATE) && !matches!(e, "release" | "session.stop") {
            return Err(format!("--reason {DUPLICATE} belongs on `release` (or `session.stop`): the work it ends is thrown away"));
        }
        if e == "stop" && !self.reason.as_deref().is_some_and(|r| STOP_REASONS.contains(&r)) {
            return Err(format!("`stop` needs --reason {}", STOP_REASONS.join("|")));
        }
        if e == "session.start" {
            self.check_start()?;
        }
        if let Some(r) = &self.reason
            && (r.is_empty() || r.chars().count() > 40 || r.contains(char::is_whitespace))
        {
            return Err(format!("--reason `{r}` must be one short word"));
        }
        Ok(())
    }

    /// `--sub`: on a sub-agent's own events, never on a session's, and an id.
    fn check_sub(&self, e: &str) -> Result<(), String> {
        match &self.sub {
            None if e.starts_with("sub.") => Err(format!("`{e}` needs --sub <the sub-agent's id: letters, digits, `-`, `_`, `.`>")),
            Some(_) if e.starts_with("session.") => Err("--sub can't go on a session event: a sub-agent's life is `sub.start` and `sub.stop`".into()),
            Some(s) => check_id(s).map_err(|why| format!("--sub: {why}")),
            None => Ok(()),
        }
    }

    /// `--model` and `--effort`: on a session's or sub-agent's start, a model's id and a known effort.
    fn check_model(&self, e: &str) -> Result<(), String> {
        if (self.model.is_some() || self.effort.is_some()) && e != "session.start" && e != "sub.start" {
            return Err("--model and --effort belong on `session.start` and `sub.start`".into());
        }
        if let Some(m) = &self.model {
            check_id(m).map_err(|why| format!("--model: {why}"))?;
        }
        match &self.effort {
            Some(x) if !EFFORTS.contains(&x.as_str()) => Err(format!("--effort `{x}`: expected {}", EFFORTS.join("|"))),
            _ => Ok(()),
        }
    }

    /// `--role` only on `session.start`; `--name` only there and on `sub.start` (an id there).
    fn check_names(&self, e: &str) -> Result<(), String> {
        if self.role.is_some() && e != "session.start" {
            return Err("--role belongs on `session.start`".into());
        }
        match &self.name {
            Some(n) if e == "sub.start" => check_id(n).map_err(|why| format!("--name: {why}")),
            Some(_) if e != "session.start" => Err("--name belongs on `session.start` (the agent) or `sub.start` (the sub-agent)".into()),
            _ => Ok(()),
        }
    }

    /// What a `session.start` needs: the agent's name, and a role.
    fn check_start(&self) -> Result<(), String> {
        if self.name.as_deref().is_none_or(|n| check_id(n).is_err()) {
            return Err("`session.start` needs --name <the agent's name: letters, digits, `-`, `_`, `.`>".into());
        }
        if !self.role.as_deref().is_some_and(|r| ROLES.contains(&r)) {
            return Err(format!("`session.start` needs --role {}", ROLES.join("|")));
        }
        Ok(())
    }

    /// [`Event::check`], logged by `agent`: a session can't be its own parent (the reader would
    /// drop the link as a cycle), and a release or session stop of the logger's own needs no
    /// `--of`, and one naming itself would read as ending someone else's.
    pub fn check_as(&self, agent: &str) -> Result<(), String> {
        self.check()?;
        if self.parent.as_deref() == Some(agent) {
            return Err(format!("--parent {agent} is this session itself: pass the instance id of the agent that started it"));
        }
        if self.of.as_deref() == Some(agent) {
            return Err(format!("--of {agent} is this instance itself: a release or session stop of your own needs no --of"));
        }
        Ok(())
    }

    /// The event as one JSON line (no newline), timestamped `t`, its keys in sorted order.
    pub fn line(&self, t: &str) -> String {
        let mut m: BTreeMap<&str, Value> = BTreeMap::new();
        m.insert("t", json!(t));
        m.insert("event", json!(self.event));
        if self.event == "session.start" {
            m.insert("format", json!(FORMAT));
        }
        for (key, value) in [("issue", self.issue), ("mr", self.mr)] {
            if let Some(v) = value {
                m.insert(key, Value::from(v));
            }
        }
        for (key, value) in [
            ("reason", &self.reason),
            ("name", &self.name),
            ("role", &self.role),
            ("machine", &self.machine),
            ("parent", &self.parent),
            ("model", &self.model),
            ("effort", &self.effort),
            ("sub", &self.sub),
            ("of", &self.of),
            ("note", &self.note),
        ] {
            if let Some(v) = value {
                m.insert(key, json!(v));
            }
        }
        // Sorted by the BTreeMap, so the same with or without serde_json's `preserve_order`.
        Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()).to_string()
    }
}

/// `flag`'s instance id, `id` when given: on one of the events it `belongs` on (else `misplaced`),
/// and an id that can name an agent.
fn check_link(id: Option<&str>, flag: &str, e: &str, belongs: &[&str], misplaced: &str) -> Result<(), String> {
    let Some(id) = id else { return Ok(()) };
    if !belongs.contains(&e) {
        return Err(misplaced.into());
    }
    check_id(id).map_err(|why| format!("{flag}: {why}"))
}

/// Whether `id` can name an agent instance (and its folder): letters, digits, `-`, `_`, `.`,
/// not starting with `.`, at most [`MAX_ID`] characters.
pub fn check_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty() && id.len() <= MAX_ID && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok { Ok(()) } else { Err(format!("`{id}` can't name an agent: use letters, digits, `-`, `_`, `.` (at most {MAX_ID})")) }
}

/// A one-line note, cut to [`MAX_NOTE`] characters.
pub fn one_line(note: &str) -> String {
    note.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_NOTE).collect()
}

/// This machine's short name: lower case, letters, digits and `-`, cut at the first `.`.
pub fn machine() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()))
        .unwrap_or_default();
    let short: String = raw.split('.').next().unwrap_or_default().to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
    if short.is_empty() { "host".into() } else { short }
}

/// A new instance id: the machine and six random hex digits.
pub fn new_instance(machine: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.write_u32(std::process::id());
    format!("{machine}-{:06x}", h.finish() & 0xff_ffff)
}

/// Parse `cargo xtask event`'s arguments, `<event> [--flag value]...`: the agent that logs it and
/// the event, or `None` for `flush` (send the queued events). `env_agent` is `project`'s
/// `<PREFIX>_AGENT`; `machine` names this machine (asked only by `session.start`). A flag given
/// twice is refused rather than the last one winning, and the event is checked as its agent logs
/// it ([`Event::check_as`]), so nothing the reader would drop is ever written.
pub fn parse(project: &Project, args: &[&str], env_agent: Option<&str>, machine: impl FnOnce() -> String) -> Result<Option<(String, Event)>, String> {
    let mut it = args.iter().copied();
    let event = it.next().ok_or(USAGE)?;
    let mut opts: BTreeMap<&str, String> = BTreeMap::new();
    while let Some(flag) = it.next() {
        let key = flag.strip_prefix("--").filter(|k| FLAGS.contains(k));
        let (Some(key), Some(value)) = (key, it.next()) else { return Err(format!("bad argument `{flag}`\n\n{USAGE}")) };
        if opts.insert(key, value.to_owned()).is_some() {
            return Err(format!("--{key} given twice: pass it once"));
        }
    }
    if event == "flush" {
        return Ok(None);
    }
    let number =
        |key: &str| opts.get(key).map(|v| v.trim_start_matches(['#', '!']).parse::<u64>().map_err(|_| format!("--{key} `{v}` is not a number"))).transpose();
    let starting = event == "session.start";
    let machine = starting.then(machine);
    let agent = match opts.get("agent").cloned().or_else(|| env_agent.filter(|s| !s.is_empty()).map(str::to_owned)) {
        Some(a) => a,
        None if starting => new_instance(machine.as_deref().unwrap_or("host")),
        None => return Err(format!("which agent? pass --agent <instance> (session.start prints it) or set {}", project.env_name("AGENT"))),
    };
    check_id(&agent)?;
    let e = Event {
        event: event.to_owned(),
        issue: number("issue")?,
        mr: number("mr")?,
        reason: opts.get("reason").cloned(),
        note: opts.get("note").map(|n| one_line(n)).filter(|n| !n.is_empty()),
        name: opts.get("name").cloned(),
        role: opts.get("role").cloned(),
        machine,
        parent: opts.get("parent").cloned(),
        sub: opts.get("sub").cloned(),
        of: opts.get("of").cloned(),
        model: opts.get("model").cloned(),
        effort: opts.get("effort").cloned(),
    };
    e.check_as(&agent)?;
    Ok(Some((agent, e)))
}

/// `cargo xtask event <event> [--agent <id>] [--issue n] [--mr n] [--reason r] [--note text]
/// [--name n --role r [--parent id]] [--sub id [--name n]] [--model m] [--effort e] [--of id]`, or
/// `cargo xtask event flush`, for `project` in the clone at `root`.
pub fn run(project: &Project, root: &Path, args: &[&str]) -> Result<(), String> {
    if project.env("TRACKING").is_some_and(|v| v.eq_ignore_ascii_case("off")) {
        println!("event: tracking is off ({}=off)", project.env_name("TRACKING"));
        return Ok(());
    }
    let env_agent = project.env("AGENT");
    let request = parse(project, args, env_agent.as_deref(), machine)?;
    let queue = queue_path(project, root);
    let Some((agent, e)) = request else {
        let sent = send(project, root, &queue, &[])?;
        println!("event: {sent} queued event(s) sent");
        return Ok(());
    };
    let line = e.line(&crate::time::now_iso());
    if e.event == "session.start" {
        println!("agent: {agent}  (pass --agent {agent} to every event this session, or set {})", project.env_name("AGENT"));
    }
    let event = &e.event;
    match send(project, root, &queue, &[(agent.clone(), line)]) {
        Ok(_) => println!("event: {event} logged for {agent}"),
        Err(e) => eprintln!("warning: event {event} not sent ({e}); queued in {} for the next event", queue.display()),
    }
    Ok(())
}

/// `cargo xtask event`'s usage, shown with a bad argument.
pub const USAGE: &str = "usage: cargo xtask event <event> [--agent <instance>] [--issue <n>] [--mr <n>] [--reason <word>] [--note <text>]
       cargo xtask event session.start --name <agent name> --role worker|coordinator [--parent <starter's instance>] [--model <id>] [--effort low|medium|high|xhigh|max]   (prints the instance id)
       cargo xtask event sub.start|sub.stop --agent <instance> --sub <id> [--name <what it is>] [--model <id>] [--effort <e>]   (a helper sub-agent; --sub on any issue event)
       cargo xtask event release --agent <instance> --issue <n> [--reason duplicate] [--of <instance whose work it ends>]
       cargo xtask event session.stop --agent <instance> --of <dead instance> --note \"<how you know>\"   (the coordinator closing a dead agent's session)
       cargo xtask event flush   (send queued events)";

/// The clone's common git directory (`git rev-parse --git-common-dir`): one for all its worktrees.
pub fn common_dir(root: &Path) -> PathBuf {
    let common = git(root, &["rev-parse", "--git-common-dir"], None, &[]).map(|s| s.trim().to_owned()).unwrap_or_else(|_| ".git".into());
    if Path::new(&common).is_absolute() { PathBuf::from(common) } else { root.join(common) }
}

/// The queue of `project`'s events not yet sent: in the clone's git directory, shared by its
/// worktrees, named by [`Project::tracking_queue`].
pub fn queue_path(project: &Project, root: &Path) -> PathBuf {
    common_dir(root).join(project.tracking_queue)
}

/// Send the queued events and `new` (agent, line) to `project`'s tracking branch on its remote,
/// in one commit. On failure everything is queued (so nothing is lost) and the error returned; on
/// success the queue is emptied. Returns how many events were sent.
pub fn send(project: &Project, root: &Path, queue: &Path, new: &[(String, String)]) -> Result<usize, String> {
    // An agent id that can't name a folder is refused, never queued: it would block the queue.
    if let Some((agent, _)) = new.iter().find(|(a, _)| check_id(a).is_err()) {
        return check_id(agent).map(|_| 0);
    }
    // Queued lines that don't parse, or name no safe folder, are dropped.
    let mut pending: Vec<(String, String)> = std::fs::read_to_string(queue)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Some((v.get("agent")?.as_str()?.to_owned(), v.get("line")?.as_str()?.to_owned())))
        .filter(|(a, _)| check_id(a).is_ok())
        .collect();
    pending.extend(new.iter().cloned());
    if pending.is_empty() {
        return Ok(0);
    }
    match push(project, root, &pending) {
        Ok(()) => {
            let _ = std::fs::remove_file(queue);
            Ok(pending.len())
        }
        Err(e) => {
            let text: String = pending.iter().map(|(a, l)| format!("{}\n", json!({"agent": a, "line": l}))).collect();
            let tmp = queue.with_extension("tmp");
            if std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, queue)).is_err() {
                return Err(format!("{e}; and the queue {} couldn't be written", queue.display()));
            }
            Err(e)
        }
    }
}

/// Append `events` (agent, line) to their agents' files on `project`'s tracking branch and push,
/// fetching and trying again when another agent pushed first.
fn push(project: &Project, root: &Path, events: &[(String, String)]) -> Result<(), String> {
    let remote = project.branches.remote;
    let src = format!("refs/heads/{}", project.branches.tracking);
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        // The branch as the remote has it now; none yet on a new remote. The commit fetches it
        // when the clone lacks it.
        let base = remote_tip(root, remote, &src)?;
        match push_on(project, root, base.as_deref(), events)? {
            None => return Ok(()),
            Some(e) => {
                last = e;
                backoff(attempt);
            }
        }
    }
    Err(format!("the tracking branch kept moving ({ATTEMPTS} tries): {last}"))
}

/// Commit `events` on `base` (the branch's tip as read; `None`: no branch yet) and push that commit
/// as the branch, never forced: `None` when it landed, or git's refusal when the branch had moved
/// on from `base` (another agent pushed first). Any other failure is an error.
fn push_on(project: &Project, root: &Path, base: Option<&str>, events: &[(String, String)]) -> Result<Option<String>, String> {
    let (remote, src) = (project.branches.remote, format!("refs/heads/{}", project.branches.tracking));
    let commit = commit(project, root, base, events)?;
    #[cfg(test)]
    tests::hook(tests::Point::Push, root);
    // No hooks: a clone's pre-push hook is for its issue branches, and starting it costs a
    // shell for every event.
    match git(root, &["push", "--quiet", "--no-verify", remote, &format!("{commit}:{src}")], None, &[]) {
        Ok(_) => Ok(None),
        Err(e) if e.contains("rejected") || e.contains("fetch first") || e.contains("non-fast-forward") || e.contains("stale info") => Ok(Some(e)),
        Err(e) => Err(e),
    }
}

/// What a conditional send ([`send_unless`]) makes of the tracking branch as it read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Append the event: as an answer, it landed on the branch it was read from.
    Send,
    /// Send nothing: the branch already says it (a claim run again by its own agent).
    Skip,
    /// Send nothing, because of this: someone else's event on the branch stands in the way.
    Refuse(String),
}

/// Send `agent`'s event `line` only onto the tracking branch that `decide` judged
/// (mornycorp/swarm-core#96): `decide` reads the branch at the tip it is given (`None`: no
/// branch yet) and says whether to send. The push is never forced, so when another agent pushed
/// between the read and the push git refuses it, and the branch is read and judged again: of two
/// agents sending at once, one lands and the other is judged on the branch with the first's event
/// on it. Unlike [`send`], nothing is queued: a send that can't be made is an error, for the
/// caller to act on. The queue is left as it is.
pub fn send_unless(
    project: &Project,
    root: &Path,
    agent: &str,
    line: &str,
    decide: &dyn Fn(Option<&str>) -> Result<Verdict, String>,
) -> Result<Verdict, String> {
    check_id(agent)?;
    let src = format!("refs/heads/{}", project.branches.tracking);
    let events = [(agent.to_owned(), line.to_owned())];
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        // The tip, fetched: `decide` reads the branch's files from it.
        let base = fetch_tip(root, project.branches.remote, &src)?;
        match decide(base.as_deref())? {
            Verdict::Send => {}
            other => return Ok(other),
        }
        match push_on(project, root, base.as_deref(), &events)? {
            None => return Ok(Verdict::Send),
            Some(e) => {
                last = e;
                backoff(attempt);
            }
        }
    }
    Err(format!("the tracking branch kept moving ({ATTEMPTS} tries): {last}"))
}

/// The commit `remote`'s `src` (a full ref name) is at now, fetched with its history; `None` when
/// the remote has no such ref.
///
/// It writes and reads no local ref. Every worktree of a clone shares its refs: two worktrees that
/// fetch into one ref race for its lock, and git fails the loser with "cannot lock ref"; a ref of
/// the call's own was, under load, not there to read a moment after its fetch.
/// So `ls-remote` names the tip, and only when the clone lacks that commit is it fetched, into no
/// ref (`--refmap=` keeps git from updating `refs/remotes/<remote>/…` too). The remote's branch
/// can only have moved on from that tip since, so the fetch brings it.
pub fn fetch_tip(root: &Path, remote: &str, src: &str) -> Result<Option<String>, String> {
    let Some(tip) = remote_tip(root, remote, src)? else {
        return Ok(None);
    };
    if !has_commit(root, &tip) {
        fetch_commit(root, remote, src, &tip)?;
    }
    Ok(Some(tip))
}

/// Does the clone at `root` have the commit `tip`?
fn has_commit(root: &Path, tip: &str) -> bool {
    git(root, &["cat-file", "-e", &format!("{tip}^{{commit}}")], None, &[]).is_ok()
}

/// The commit `remote`'s `src` is at now, as `ls-remote` lists it; `None` when it has no such ref.
fn remote_tip(root: &Path, remote: &str, src: &str) -> Result<Option<String>, String> {
    Ok(parse_ls_remote(&git(root, &["ls-remote", remote, src], None, &[])?, src))
}

/// Fetch `remote`'s `src` into no ref (see [`fetch_tip`]); an error unless that brings `tip`.
fn fetch_commit(root: &Path, remote: &str, src: &str, tip: &str) -> Result<(), String> {
    git(root, &["fetch", "--quiet", "--no-tags", "--refmap=", "--no-write-fetch-head", "--no-auto-maintenance", remote, src], None, &[])?;
    if has_commit(root, tip) { Ok(()) } else { Err(format!("{remote} {src} was at {tip}, but its fetch didn't bring that commit (was the branch rewound?)")) }
}

/// The object id `git ls-remote` lists for exactly `name` (it also lists refs ending in it).
fn parse_ls_remote(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|l| {
        let (id, r) = l.split_once('\t')?;
        (r == name && matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_owned())
    })
}

/// Fetch `src` from `remote` into the local ref `dst`. Every worktree of a clone shares its refs,
/// so two worktrees fetching into `dst` at once race to update it, and git fails the loser
/// ([`lost_the_lock`]): that fetch is tried again. Read what was fetched with
/// [`fetch_tip`] instead when it must be this fetch's result.
pub fn fetch(root: &Path, remote: &str, src: &str, dst: &str) -> Result<(), String> {
    let refspec = format!("+{src}:{dst}");
    let mut attempt = 0;
    loop {
        match git(root, &["fetch", "--quiet", "--no-tags", "--refmap=", remote, &refspec], None, &[]) {
            Err(e) if lost_the_lock(&e) && attempt + 1 < ATTEMPTS => {
                #[cfg(test)]
                tests::hook(tests::Point::Refetch, root);
                backoff(attempt);
                attempt += 1;
            }
            r => return r.map(|_| ()),
        }
    }
}

/// Whether git failed a fetch because another process held the ref's lock. Older git says
/// "cannot lock ref"; git 2.53, which updates a fetch's refs as one batch, says "fetching ref
/// <ref> failed: reference already exists" for a held lock, new ref or not. A forced refspec
/// is never refused for a ref that really exists, so that text is the lock too.
fn lost_the_lock(error: &str) -> bool {
    error.contains("cannot lock ref") || (error.contains("fetching ref") && error.contains("failed: reference already exists"))
}

/// Wait after failed try `attempt` (from 0), longer each time and with jitter: two agents that
/// collided on a fixed schedule would collide again on every retry.
fn backoff(attempt: usize) {
    let jitter = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::from(d.subsec_nanos() % 200));
    std::thread::sleep(Duration::from_millis(100 * (attempt as u64 + 1) + jitter));
}

/// Why `line` would repeat what an agent's file `text` already says, if it would: the very same line (a queued event whose push did land but
/// read as failed, sent again), or a second `sub.stop` of a sub-agent the file has stopped (`sub`
/// is unique within a file, so it has one life). The inspector reads past both, but draws them.
pub fn repeat_of(text: &str, line: &str) -> Option<String> {
    if text.lines().any(|l| l == line) {
        return Some("the same event is already on the branch".into());
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("event").and_then(Value::as_str) != Some("sub.stop") {
        return None;
    }
    let sub = v.get("sub").and_then(Value::as_str)?;
    let stopped = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .any(|o| o.get("event").and_then(Value::as_str) == Some("sub.stop") && o.get("sub").and_then(Value::as_str) == Some(sub));
    stopped.then(|| format!("sub-agent `{sub}` is already stopped (a sub-agent stops once)"))
}

/// `agent`'s file `old` with its lines of `events` appended, less any it has already logged that
/// can only be logged once.
fn appended(old: &[u8], agent: &str, events: &[(String, String)]) -> String {
    let mut text = String::from_utf8_lossy(old).into_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for (_, line) in events.iter().filter(|(a, _)| a == agent) {
        match repeat_of(&text, line) {
            Some(why) => eprintln!("event: not logged again for {agent}: {why}"),
            None => {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    text
}

/// A commit on `base` (or a new root with the README) with `events` appended to their files.
///
/// Two git processes, whatever the number of agents: one
/// `cat-file --batch` checks the clone has `base` (fetching it from the remote when not) and reads
/// the agents' files, one `fast-import` writes the blobs, the tree and the commit. Each process
/// costs 100 to 300 ms on a loaded Windows machine, and the plumbing this replaced took eight for
/// one agent. `fast-import` must name a ref to commit to: a ref of this call's own, deleted in the
/// same stream, so it never reaches the ref store the worktrees share.
fn commit(project: &Project, root: &Path, base: Option<&str>, events: &[(String, String)]) -> Result<String, String> {
    let mut agents: Vec<&str> = events.iter().map(|(a, _)| a.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();
    for agent in &agents {
        check_id(agent)?;
    }
    let old = read_files(project, root, base, &agents)?;
    let mut stream = Vec::new();
    let mut files = Vec::new();
    let mut blob = |stream: &mut Vec<u8>, path: String, bytes: &[u8]| {
        files.push(path);
        stream.extend(format!("blob\nmark :{}\ndata {}\n", files.len(), bytes.len()).as_bytes());
        stream.extend(bytes);
        stream.push(b'\n');
    };
    if base.is_none() {
        blob(&mut stream, "README.md".into(), README.as_bytes());
    }
    for (agent, old) in agents.iter().zip(old) {
        blob(&mut stream, agent_file(agent), appended(&old, agent, events).as_bytes());
    }
    let who = agents.first().copied().unwrap_or("agent");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let ident = format!("{who} <{who}@{}.invalid> {} +0000", project.name, now.as_secs());
    let message = format!("{}: {} event(s)", agents.join(", "), events.len());
    let scratch = format!("refs/swarm-tracking-commit/{}-{:?}-{}", std::process::id(), std::thread::current().id(), now.subsec_nanos()).replace(['(', ')'], "");
    let mark = files.len() + 1;
    stream.extend(format!("commit {scratch}\nmark :{mark}\nauthor {ident}\ncommitter {ident}\ndata {}\n{message}\n", message.len()).as_bytes());
    if let Some(b) = base {
        stream.extend(format!("from {b}\n").as_bytes());
    }
    for (i, f) in files.iter().enumerate() {
        stream.extend(format!("M 100644 :{} {f}\n", i + 1).as_bytes());
    }
    // `get-mark` prints the commit; resetting the scratch ref to the null id deletes it.
    stream.extend(format!("\nget-mark :{mark}\nreset {scratch}\nfrom {}\n\ndone\n", "0".repeat(40)).as_bytes());
    // A pack of its own however few the objects: no `unpack-objects` process to loosen them.
    let out = git(root, &["-c", "fastimport.unpackLimit=0", "fast-import", "--quiet", "--done"], Some(&stream), &[])?;
    let commit = out.trim();
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("git fast-import: no commit id in its output `{commit}`"));
    }
    Ok(commit.to_owned())
}

/// An agent's file on the tracking branch.
fn agent_file(agent: &str) -> String {
    format!("tracking/{agent}/work.jsonl")
}

/// `agents`' files as they are on `base` (empty where none yet, and for every agent on a new
/// branch). The base itself is read first: a clone that lacks it fetches it (as [`fetch_tip`]
/// does) and reads again. A missing base must never read as files that don't exist yet, which
/// would start each agent's file again.
fn read_files(project: &Project, root: &Path, base: Option<&str>, agents: &[&str]) -> Result<Vec<Vec<u8>>, String> {
    let Some(b) = base else {
        return Ok(vec![Vec::new(); agents.len()]);
    };
    let names: Vec<String> = std::iter::once(format!("{b}^{{commit}}")).chain(agents.iter().map(|a| format!("{b}:{}", agent_file(a)))).collect();
    let mut read = read_objects(root, &names)?;
    if read.first().is_none_or(Option::is_none) {
        fetch_commit(root, project.branches.remote, &format!("refs/heads/{}", project.branches.tracking), b)?;
        read = read_objects(root, &names)?;
    }
    Ok(read.into_iter().skip(1).map(Option::unwrap_or_default).collect())
}

/// The contents of `names` (objects as `git cat-file` names them, such as `<commit>:<path>`), read
/// by one `git cat-file --batch`; `None` for each the clone doesn't have.
fn read_objects(root: &Path, names: &[String]) -> Result<Vec<Option<Vec<u8>>>, String> {
    let input: String = names.iter().map(|n| format!("{n}\n")).collect();
    let out = git_bytes(root, &["cat-file", "--batch"], Some(input.as_bytes()), &[])?;
    let mut rest = out.as_slice();
    let mut read = Vec::with_capacity(names.len());
    for name in names {
        let bad = || format!("git cat-file --batch: a bad answer for {name}");
        let nl = rest.iter().position(|&b| b == b'\n').ok_or_else(bad)?;
        let (header, after) = rest.split_at(nl);
        let after = after.get(1..).unwrap_or_default();
        let header = String::from_utf8_lossy(header);
        // `<name> missing`, or `<oid> <type> <size>` and the contents and a newline.
        if header.ends_with(" missing") {
            read.push(None);
            rest = after;
            continue;
        }
        let size: usize = header.rsplit(' ').next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
        let contents = after.get(..size).ok_or_else(bad)?;
        read.push(Some(contents.to_vec()));
        rest = after.get(size.saturating_add(1)..).unwrap_or_default();
    }
    Ok(read)
}

/// The instances of [`examples`]: a coordinator, a worker it started, and a worker that died.
pub const EXAMPLE_INSTANCES: [&str; 3] = ["pc-c00rd1", "pc-a0b1c2", "pc-dead1"];

/// What [`examples`] logs: when, by which of [`EXAMPLE_INSTANCES`], with which arguments.
const EXAMPLE_SCRIPT: &[(&str, usize, &[&str])] = &[
    ("2026-10-17T10:00:00Z", 0, &["session.start", "--name", "coordinator", "--role", "coordinator", "--model", "claude-opus-5-5", "--effort", "max"]),
    ("2026-10-17T10:01:00Z", 1, &["session.start", "--name", "seek", "--role", "worker", "--parent", "pc-c00rd1"]),
    ("2026-10-17T10:01:30Z", 2, &["session.start", "--name", "alder", "--role", "worker", "--model", "claude-sonnet-5-5"]),
    ("2026-10-17T10:02:00Z", 1, &["claim", "--issue", "7", "--mr", "9"]),
    ("2026-10-17T10:02:30Z", 2, &["claim", "--issue", "61", "--mr", "62"]),
    ("2026-10-17T10:03:00Z", 2, &["work", "--issue", "61"]),
    ("2026-10-17T10:05:00Z", 1, &["work", "--issue", "7"]),
    ("2026-10-17T10:06:00Z", 1, &["session.pause", "--note", "waiting for the owner"]),
    ("2026-10-17T10:07:00Z", 1, &["session.resume"]),
    ("2026-10-17T10:10:00Z", 0, &["sub.start", "--sub", "gate-1", "--name", "gate", "--model", "claude-sonnet-5-5", "--effort", "low"]),
    ("2026-10-17T10:11:00Z", 0, &["work", "--issue", "28", "--sub", "gate-1"]),
    ("2026-10-17T10:20:00Z", 0, &["sub.stop", "--sub", "gate-1", "--note", "rc=0"]),
    ("2026-10-17T10:21:00Z", 0, &["merge", "--issue", "28", "--mr", "29", "--sub", "gate-1"]),
    ("2026-10-17T10:22:00Z", 1, &["submit", "--issue", "7", "--mr", "9"]),
    ("2026-10-17T10:23:00Z", 0, &["sendback", "--issue", "7", "--mr", "9", "--note", "cause=conflict"]),
    ("2026-10-17T10:23:30Z", 1, &["work", "--issue", "7", "--mr", "9", "--note", "rework: #9 sent back 2026-10-17T10:23:00Z: cause=conflict"]),
    ("2026-10-17T10:24:00Z", 1, &["stop", "--issue", "7", "--reason", "blocked"]),
    ("2026-10-17T10:30:00Z", 0, &["release", "--issue", "7", "--of", "pc-a0b1c2", "--reason", "duplicate", "--note", "spruce has it"]),
    ("2026-10-17T10:31:00Z", 1, &["session.stop"]),
    ("2026-10-18T03:00:00Z", 0, &["session.stop", "--of", "pc-dead1", "--note", "its machine rebooted; silent 17 h"]),
    ("2026-10-18T03:05:00Z", 0, &["release", "--issue", "61", "--reason", "stale", "--of", "pc-dead1"]),
    ("2026-10-18T03:06:00Z", 0, &["session.stop", "--reason", "duplicate"]),
];

/// Every kind of line the writer emits, as `(instance, line)` in time order: each event, each
/// optional field, a started session and a sub-agent nested under the coordinator, duplicate work
/// thrown away and a dead agent's session ended by the coordinator. Each comes from [`parse`] and
/// [`Event::line`], so a reader's round-trip test reads exactly
/// what `cargo xtask event` writes.
pub fn examples(project: &Project) -> Result<Vec<(String, String)>, String> {
    EXAMPLE_SCRIPT
        .iter()
        .map(|(t, who, args)| {
            let instance = EXAMPLE_INSTANCES.get(*who).copied().ok_or("no such example instance")?;
            let (agent, e) = parse(project, args, Some(instance), || "pc".into())?.ok_or("flush is no event")?;
            Ok((agent, e.line(t)))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::fixtures::{APP_SWARM, BOTH, GPU_SWARM};
    use std::cell::RefCell;

    /// Where a test can step in: a send has read the branch and committed, and is about to push
    /// ([`push_on`]); a fetch has failed on a held ref and is about to wait and try again ([`fetch`]).
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub(crate) enum Point {
        Push,
        Refetch,
    }

    type Hook = (Point, PathBuf, Box<dyn FnOnce()>);

    thread_local! {
        static HOOKS: RefCell<Vec<Hook>> = const { RefCell::new(Vec::new()) };
    }

    /// Run `f` once, when this thread next reaches `point` in `repo`: how a test puts another
    /// agent's push between a send's read of the branch and its own push, every time, rather than
    /// racing threads until it happens.
    pub(crate) fn at(point: Point, repo: &Path, f: impl FnOnce() + 'static) {
        HOOKS.with(|h| h.borrow_mut().push((point, repo.to_owned(), Box::new(f))));
    }

    /// The hooks [`at`] set that haven't run.
    pub(crate) fn pending() -> usize {
        HOOKS.with(|h| h.borrow().len())
    }

    /// Called at `point` in `root`: runs the first hook set for it (which may send, reaching
    /// points of its own).
    pub(crate) fn hook(point: Point, root: &Path) {
        let f = HOOKS.with(|h| {
            let mut h = h.borrow_mut();
            let i = h.iter().position(|(p, r, _)| *p == point && r == root)?;
            Some(h.remove(i).2)
        });
        if let Some(f) = f {
            f();
        }
    }

    fn event(name: &str) -> Event {
        Event::named(name)
    }

    /// `cargo xtask event <args>` as logged by `agent` (`<PREFIX>_AGENT`), on machine `pc`.
    fn parsed(args: &[&str], agent: Option<&str>) -> Result<(String, Event), String> {
        parse(&APP_SWARM, args, agent, || "pc".into())?.ok_or_else(|| "flush".into())
    }

    #[test]
    fn events_are_checked_and_written_as_one_json_line() {
        assert!(event("nap").check().unwrap_err().contains("unknown event `nap`"));
        assert!(event("claim").check().unwrap_err().contains("needs --issue"));
        assert!(event("session.pause").check().is_ok());
        let stop = Event { issue: Some(201), ..event("stop") };
        assert!(stop.check().unwrap_err().contains("--reason handoff|blocked|paused|done"));
        assert!(Event { reason: Some("handoff".into()), ..stop.clone() }.check().is_ok());
        assert!(Event { reason: Some("two words".into()), issue: Some(1), ..event("release") }.check().is_err());
        let start = Event { name: Some("claude-mac".into()), role: Some("worker".into()), machine: Some("mac".into()), ..event("session.start") };
        assert!(start.check().is_ok());
        assert!(Event { role: Some("boss".into()), ..start.clone() }.check().unwrap_err().contains("--role worker|coordinator"));
        assert!(Event { name: None, ..start.clone() }.check().is_err());

        let claim = Event { issue: Some(201), mr: Some(200), note: Some(one_line("Events list\n  sorting")), ..event("claim") };
        let line = claim.line("2026-10-17T09:12:03Z");
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v, json!({"t": "2026-10-17T09:12:03Z", "event": "claim", "issue": 201, "mr": 200, "note": "Events list sorting"}));
        assert_eq!(one_line(&"x".repeat(500)).len(), MAX_NOTE);
    }

    /// The contract is byte for byte: a line's keys are sorted, as both consumers' writers wrote
    /// them before the move (serde_json without `preserve_order`), whatever the build's features.
    #[test]
    fn lines_are_written_byte_for_byte_as_before() {
        let (_, start) = parsed(
            &["session.start", "--name", "cedar", "--role", "worker", "--parent", "pc-1", "--model", "claude-opus-5-5", "--effort", "high", "--note", "hi"],
            Some("pc-2"),
        )
        .unwrap();
        assert_eq!(
            start.line("2026-10-17T21:00:00Z"),
            r#"{"effort":"high","event":"session.start","format":3,"machine":"pc","model":"claude-opus-5-5","name":"cedar","note":"hi","parent":"pc-1","role":"worker","t":"2026-10-17T21:00:00Z"}"#
        );
        let (_, release) = parsed(&["release", "--issue", "#7", "--mr", "#9", "--reason", "duplicate", "--of", "pc-3", "--sub", "g"], Some("pc-2")).unwrap();
        assert_eq!(
            release.line("2026-10-17T21:00:01Z"),
            r#"{"event":"release","issue":7,"mr":9,"of":"pc-3","reason":"duplicate","sub":"g","t":"2026-10-17T21:00:01Z"}"#
        );
    }

    /// A started session's `session.start` names its parent instance, which must be
    /// a safe instance id other than its own, and only `session.start` carries one.
    #[test]
    fn a_session_start_names_its_parent_instance() {
        let (agent, child) = parsed(&["session.start", "--name", "worker-seek", "--role", "worker", "--parent", "pc-719698"], None).unwrap();
        assert!(agent.starts_with("pc-"), "{agent}");
        assert_eq!((child.parent.as_deref(), child.machine.as_deref()), (Some("pc-719698"), Some("pc")));
        let v: Value = serde_json::from_str(&child.line("2026-10-17T13:00:00Z")).unwrap();
        assert_eq!(v["parent"], "pc-719698");
        let (_, top) = parsed(&["session.start", "--name", "spruce", "--role", "worker"], None).unwrap();
        assert!(serde_json::from_str::<Value>(&top.line("2026-10-17T13:00:00Z")).unwrap().get("parent").is_none(), "top level: no parent");
        assert!(Event { parent: Some("pc-719698".into()), issue: Some(1), ..event("claim") }.check().unwrap_err().contains("session.start"));
        for (args, why) in [
            (&["session.start", "--name", "w", "--role", "worker", "--parent", "../x"][..], "--parent"),
            (&["session.start", "--name", "w", "--role", "worker", "--parent", ""], "--parent"),
            (&["session.start", "--agent", "pc-1", "--name", "w", "--role", "worker", "--parent", "pc-1"], "this session itself"),
            (&["claim", "--agent", "pc-1", "--issue", "1", "--parent", "pc-2"], "--parent belongs on `session.start`"),
            (&["work", "--agent", "pc-1", "--issue", "1", "--role", "worker"], "--role belongs"),
            (&["work", "--agent", "pc-1", "--issue", "1", "--name", "x"], "--name belongs"),
            (&["session.start", "--name", "w", "--role", "worker", "--parent", "a", "--parent", "b"], "twice"),
        ] {
            let err = parsed(args, None).unwrap_err();
            assert!(err.contains(why), "{args:?}: {err}");
        }
    }

    /// A helper sub-agent has no file: its parent logs `sub.start` and `sub.stop`
    /// with its id, and any issue event may name it with `sub`; a session event can't.
    #[test]
    fn a_parent_logs_its_sub_agents_in_its_own_file() {
        let (_, start) = parsed(&["sub.start", "--agent", "pc-1", "--sub", "gate-1", "--name", "gate"], None).unwrap();
        let v: Value = serde_json::from_str(&start.line("2026-10-17T14:00:00Z")).unwrap();
        assert_eq!(v, json!({"t": "2026-10-17T14:00:00Z", "event": "sub.start", "sub": "gate-1", "name": "gate"}), "no issue needed");
        assert!(parsed(&["sub.stop", "--sub", "gate-1"], Some("pc-1")).is_ok());
        let (_, work) = parsed(&["work", "--issue", "#28", "--sub", "gate-1"], Some("pc-1")).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&work.line("2026-10-17T14:00:01Z")).unwrap()["sub"], "gate-1");
        for (args, why) in [
            (&["sub.stop", "--agent", "pc-1"][..], "needs --sub"),
            (&["sub.start", "--agent", "pc-1", "--sub", "a b"], "--sub"),
            (&["sub.start", "--agent", "pc-1", "--sub", ".x"], "--sub"),
            (&["sub.start", "--agent", "pc-1", "--sub", "x", "--name", "two words"], "--name"),
            (&["sub.stop", "--agent", "pc-1", "--sub", "x", "--name", "gate"], "--name belongs"),
            (&["sub.start", "--agent", "pc-1", "--sub", "x", "--role", "worker"], "--role belongs"),
            (&["session.pause", "--agent", "pc-1", "--sub", "x"], "session event"),
            (&["session.start", "--name", "w", "--role", "worker", "--sub", "x"], "session event"),
        ] {
            let err = parsed(args, None).unwrap_err();
            assert!(err.contains(why), "{args:?}: {err}");
        }
    }

    /// Duplicate work is released with reason `duplicate`; the coordinator ending
    /// another agent's work names it with `of`, which only `release` carries.
    #[test]
    fn duplicate_work_is_released_naming_whose_work_ends() {
        let (_, mine) = parsed(&["release", "--issue", "218", "--reason", "duplicate", "--note", "claude-mac has it"], Some("pc-1")).unwrap();
        assert_eq!((mine.reason.as_deref(), mine.of.as_deref()), (Some(DUPLICATE), None));
        let (_, theirs) = parsed(&["release", "--issue", "218", "--reason", "duplicate", "--of", "mxv9wfm7rr-2021a6"], Some("pc-1")).unwrap();
        let v: Value = serde_json::from_str(&theirs.line("2026-10-17T15:00:00Z")).unwrap();
        assert_eq!((v["reason"].as_str(), v["of"].as_str()), (Some("duplicate"), Some("mxv9wfm7rr-2021a6")));
        assert!(theirs.check_as("mxv9wfm7rr-2021a6").unwrap_err().contains("this instance itself"));
        assert!(parsed(&["release", "--issue", "7", "--reason", "stale", "--of", "pc-2"], Some("pc-1")).is_ok(), "a stale claim freed");
        assert!(parsed(&["session.stop", "--reason", "duplicate"], Some("pc-1")).is_ok());
        for (args, why) in [
            (&["claim", "--issue", "1", "--of", "pc-2"][..], "--of belongs on `release`"),
            (&["release", "--issue", "1", "--of", "../x"], "--of"),
            (&["release", "--issue", "1", "--of", "pc-1"], "this instance itself"),
            (&["claim", "--issue", "1", "--reason", "duplicate"], "--reason duplicate belongs on `release`"),
            (&["sendback", "--issue", "1", "--mr", "2", "--reason", "duplicate"], "--reason duplicate belongs"),
            (&["stop", "--issue", "1", "--reason", "duplicate"], "--reason duplicate belongs on `release`"),
        ] {
            let err = parsed(args, Some("pc-1")).unwrap_err();
            assert!(err.contains(why), "{args:?}: {err}");
        }
        assert_eq!(parse(&APP_SWARM, &["flush"], None, || "pc".into()).unwrap(), None);
    }

    /// Every `session.start` names the format version this
    /// writer writes; no other event does, and nobody sets it by hand (`--format` is no flag).
    /// That the reader reads it back is the consumer's test ([`examples`]).
    #[test]
    fn a_session_start_names_the_format_version() {
        let (_, start) = parsed(&["session.start", "--name", "rowan", "--role", "worker"], None).unwrap();
        let v: Value = serde_json::from_str(&start.line("2026-10-18T10:00:00Z")).unwrap();
        assert_eq!(v["format"].as_u64(), Some(u64::from(FORMAT)));
        assert_eq!(FORMAT, 3, "a change to the format is done only when docs/tracking-format.md › Versions' rule holds");
        for args in [&["claim", "--issue", "1"][..], &["session.stop"], &["sub.start", "--sub", "g"]] {
            let (_, e) = parsed(args, Some("pc-1")).unwrap();
            let v: Value = serde_json::from_str(&e.line("2026-10-18T10:00:00Z")).unwrap();
            assert!(v.get("format").is_none(), "{args:?}: {v}");
        }
        let e = parsed(&["session.start", "--name", "w", "--role", "worker", "--format", "9"], None).unwrap_err();
        assert!(e.contains("--format"), "{e}");
    }

    /// `session.start` and `sub.start` may name the model and its effort,
    /// checked; no other event may, and both stay optional.
    #[test]
    fn a_session_or_sub_agent_may_name_its_model_and_effort() {
        let args = ["session.start", "--name", "cedar", "--role", "worker", "--model", "claude-opus-5-5", "--effort", "high"];
        let (_, start) = parsed(&args, None).unwrap();
        let v: Value = serde_json::from_str(&start.line("2026-10-17T21:00:00Z")).unwrap();
        assert_eq!((v["model"].as_str(), v["effort"].as_str()), (Some("claude-opus-5-5"), Some("high")));
        let (_, plain) = parsed(&["session.start", "--name", "cedar", "--role", "worker"], None).unwrap();
        let v: Value = serde_json::from_str(&plain.line("2026-10-17T21:00:00Z")).unwrap();
        assert!(v.get("model").is_none() && v.get("effort").is_none(), "optional: {v}");
        let (_, sub) = parsed(&["sub.start", "--sub", "gate-1", "--name", "gate", "--model", "claude-sonnet-5-5"], Some("pc-1")).unwrap();
        let v: Value = serde_json::from_str(&sub.line("2026-10-17T21:00:00Z")).unwrap();
        assert_eq!(v, json!({"t": "2026-10-17T21:00:00Z", "event": "sub.start", "sub": "gate-1", "name": "gate", "model": "claude-sonnet-5-5"}));
        assert!(parsed(&["sub.start", "--sub", "g", "--effort", "max"], Some("pc-1")).is_ok(), "effort alone");
        for (args, why) in [
            (&["session.start", "--name", "w", "--role", "worker", "--effort", "huge"][..], "--effort `huge`: expected low|medium|high|xhigh|max"),
            (&["session.start", "--name", "w", "--role", "worker", "--effort", "HIGH"], "--effort"),
            (&["session.start", "--name", "w", "--role", "worker", "--model", "a model"], "--model"),
            (&["session.start", "--name", "w", "--role", "worker", "--model", ""], "--model"),
            (&["claim", "--agent", "pc-1", "--issue", "1", "--model", "claude-opus-5-5"], "belong on `session.start` and `sub.start`"),
            (&["sub.stop", "--agent", "pc-1", "--sub", "g", "--effort", "low"], "belong on"),
            (&["session.stop", "--agent", "pc-1", "--model", "m"], "belong on"),
            (&["session.start", "--name", "w", "--role", "worker", "--model", "a", "--model", "b"], "twice"),
        ] {
            let err = parsed(args, None).unwrap_err();
            assert!(err.contains(why), "{args:?}: {err}");
        }
    }

    /// The writer refuses what a reader would
    /// drop, so nothing it writes is silently lost on the reader's side. For each consumer: the
    /// agent's variable is named by its own prefix.
    #[test]
    fn the_writer_refuses_what_the_inspector_would_drop() {
        for project in BOTH {
            let parsed = |args: &[&str]| parse(&project, args, Some("pc-e15424"), || "pc".into());
            let refused = |args: &[&str], why: &str| {
                let e = parsed(args).unwrap_err();
                assert!(e.contains(why), "{args:?}: {e}");
            };
            // --role anywhere but session.start.
            refused(&["work", "--issue", "1", "--role", "worker"], "--role belongs on `session.start`");
            refused(&["sub.start", "--sub", "g", "--role", "worker"], "--role belongs on `session.start`");
            // --name anywhere but session.start and sub.start.
            refused(&["claim", "--issue", "1", "--mr", "2", "--name", "w"], "--name belongs on `session.start`");
            refused(&["session.stop", "--name", "w"], "--name belongs on `session.start`");
            // --reason duplicate anywhere but release and session.stop.
            refused(&["stop", "--issue", "1", "--reason", "duplicate"], "--reason duplicate belongs on `release`");
            refused(&["claim", "--issue", "1", "--reason", "duplicate"], "--reason duplicate belongs on `release`");
            // A session can't be its own parent.
            refused(&["session.start", "--agent", "pc-e15424", "--name", "w", "--role", "worker", "--parent", "pc-e15424"], "is this session itself");
            // A flag given twice: the last one used to win.
            refused(&["work", "--issue", "1", "--issue", "2"], "--issue given twice");
            refused(&["session.start", "--name", "a", "--name", "b", "--role", "worker"], "--name given twice");
            refused(&["flush", "--agent", "x", "--agent", "y"], "--agent given twice");
            refused(&["work", "--issue", "1", "--bogus", "x"], "bad argument `--bogus`");

            // What stays allowed.
            let (agent, e) = parsed(&["session.start", "--name", "w", "--role", "worker", "--parent", "pc-719698"]).unwrap().unwrap();
            assert!(agent.starts_with("pc-") && agent != "pc-719698", "{agent}");
            assert_eq!((e.machine.as_deref(), e.parent.as_deref()), (Some("pc"), Some("pc-719698")));
            let (_, e) = parsed(&["sub.start", "--sub", "gate-1", "--name", "gate"]).unwrap().unwrap();
            assert_eq!((e.name.as_deref(), e.machine), (Some("gate"), None), "machine asked only on session.start");
            for ok in [&["release", "--issue", "1", "--reason", "duplicate"][..], &["session.stop", "--reason", "duplicate"]] {
                assert!(parsed(ok).is_ok(), "{ok:?}");
            }
            assert_eq!(parsed(&["flush"]).unwrap(), None);
            let which = parse(&project, &["work", "--issue", "1"], None, || "pc".into()).unwrap_err();
            assert!(which.contains(&project.env_name("AGENT")), "{}: {which}", project.name);
            assert!(parse(&project, &["work", "--issue", "1"], Some(""), || "pc".into()).is_err(), "an empty variable names no agent");
        }
        assert!(parse(&GPU_SWARM, &["work", "--issue", "1"], None, || "pc".into()).unwrap_err().ends_with("set GPU_SWARM_AGENT"));
        assert!(parse(&APP_SWARM, &[], None, || "pc".into()).unwrap_err().starts_with("usage: cargo xtask event"));
    }

    #[test]
    fn instance_ids_are_safe_folder_names() {
        let id = new_instance("mac");
        assert!(id.starts_with("mac-") && id.len() == 10, "{id}");
        check_id(&id).unwrap();
        assert_ne!(new_instance("mac"), new_instance("mac"));
        for bad in ["", "../x", ".hidden", "a/b", "a b", &"x".repeat(65)] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
        let m = machine();
        assert!(!m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'), "{m}");
    }

    /// The coordinator closes a dead agent's session with
    /// `session.stop --of`, logged in its own file; only a session stop or a release takes
    /// `--of`, and never the logger's own id. (How the reader ends the dead lane's session is the
    /// consumer's round-trip test, [`examples`].)
    #[test]
    fn a_dead_agents_session_is_stopped_naming_it() {
        let (agent, e) = parsed(&["session.stop", "--of", "pc-dead1", "--note", "its machine rebooted; silent 17 h"], Some("pc-c00rd1")).unwrap();
        assert_eq!(agent, "pc-c00rd1", "logged in the coordinator's own file");
        let v: Value = serde_json::from_str(&e.line("2026-10-18T10:00:00Z")).unwrap();
        assert_eq!(
            (v["event"].as_str(), v["of"].as_str(), v["note"].as_str()),
            (Some("session.stop"), Some("pc-dead1"), Some("its machine rebooted; silent 17 h"))
        );
        for (args, why) in [
            (&["session.stop", "--of", "pc-c00rd1"][..], "this instance itself"),
            (&["session.stop", "--of", "a b"], "--of"),
            (&["session.pause", "--of", "pc-dead1"], "`session.stop`"),
            (&["session.resume", "--of", "pc-dead1"], "`session.stop`"),
            (&["session.start", "--name", "c", "--role", "coordinator", "--of", "pc-dead1"], "`session.stop`"),
        ] {
            let e = parsed(args, Some("pc-c00rd1")).unwrap_err();
            assert!(e.contains(why), "{args:?}: {e}");
        }
    }

    /// What a consumer's round-trip test reads: every event, every field, each checked as its
    /// agent logs it, in time order, from the instances it names; the same for both consumers.
    #[test]
    fn examples_cover_every_event_and_field() {
        let lines = examples(&APP_SWARM).unwrap();
        assert_eq!(lines, examples(&GPU_SWARM).unwrap());
        assert_eq!(lines.len(), EXAMPLE_SCRIPT.len());
        let values: Vec<Value> = lines.iter().map(|(_, l)| serde_json::from_str(l).unwrap()).collect();
        for e in EVENTS {
            assert!(values.iter().any(|v| v["event"] == *e), "no example of {e}");
        }
        let fields = ["t", "event", "format", "issue", "mr", "reason", "note", "name", "role", "machine", "parent", "sub", "of", "model", "effort"];
        for f in fields {
            assert!(values.iter().any(|v| v.get(f).is_some()), "no example with {f}");
        }
        assert!(values.iter().all(|v| v.as_object().unwrap().keys().all(|k| fields.contains(&k.as_str()))));
        let times: Vec<&str> = values.iter().map(|v| v["t"].as_str().unwrap()).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "in time order");
        assert!(lines.iter().all(|(a, _)| EXAMPLE_INSTANCES.contains(&a.as_str())));
        assert!(values.iter().any(|v| v["of"] == "pc-dead1" && v["event"] == "session.stop"));
        assert!(values.iter().any(|v| v["reason"] == DUPLICATE && v["of"] == "pc-a0b1c2"));
    }

    /// A bare "remote" and a clone of it, in a fresh directory.
    fn repos(name: &str) -> (PathBuf, PathBuf, String) {
        let dir = swarm_core_testkit::temp_dir(&format!("swarm-core-event-{name}"));
        let remote = dir.join("remote.git");
        git(&dir, &["init", "--quiet", "--bare", remote.to_str().unwrap()], None, &[]).unwrap();
        let clone = dir.join("clone");
        git(&dir, &["init", "--quiet", clone.to_str().unwrap()], None, &[]).unwrap();
        git(&clone, &["remote", "add", "origin", remote.to_str().unwrap()], None, &[]).unwrap();
        (dir, clone, remote.display().to_string())
    }

    /// `path` on the remote's tracking branch (both consumers name it `tracking`).
    fn file(remote: &str, path: &str) -> String {
        git(Path::new(remote), &["cat-file", "blob", &format!("{}:{path}", APP_SWARM.branches.tracking)], None, &[]).unwrap_or_default()
    }

    /// The branch never holds an event twice. A queued event
    /// whose push landed but read as failed is sent again, and a coordinator logged gates'
    /// `sub.stop` twice (gate-22 at 10:34 with `rc=0`, again at 10:36 without). The writer drops
    /// the repeat; other agents' files and other sub-agents are untouched.
    #[test]
    fn an_event_already_on_the_branch_is_not_logged_again() {
        let stop = |sub: &str, t: &str, note: &str| format!(r#"{{"event":"sub.stop","note":"{note}","sub":"{sub}","t":"{t}"}}"#);
        let first = stop("gate-22", "2026-10-18T10:34:47Z", "rc=0");
        let text = format!("{}\n{first}\n", r#"{"event":"sub.start","name":"gate","sub":"gate-22","t":"2026-10-18T10:20:07Z"}"#);
        assert!(repeat_of(&text, &first).is_some_and(|w| w.contains("same event")));
        let again = r#"{"event":"sub.stop","sub":"gate-22","t":"2026-10-18T10:36:23Z"}"#;
        assert!(repeat_of(&text, again).is_some_and(|w| w.contains("gate-22")));
        assert_eq!(repeat_of(&text, &stop("gate-23", "2026-10-18T10:46:41Z", "rc=0")), None);
        assert_eq!(repeat_of(&text, r#"{"event":"merge","issue":254,"t":"2026-10-18T10:35:00Z"}"#), None);
        assert_eq!(repeat_of(&text, "not json"), None);
        assert_eq!(repeat_of(&text, r#"{"event":"sub.stop","t":"2026-10-18T10:36:23Z"}"#), None, "no sub: not a repeat");
        // A work event at another time is no repeat: only the very same line is.
        let work = r#"{"event":"work","issue":7,"t":"2026-10-18T10:00:00Z"}"#;
        let worked = format!("{work}\n");
        assert_eq!(repeat_of(&worked, r#"{"event":"work","issue":7,"t":"2026-10-18T11:00:00Z"}"#), None);

        let (dir, clone, remote) = repos("repeat");
        let queue = dir.join("queue.jsonl");
        let start = r#"{"event":"sub.start","sub":"gate-22","t":"2026-10-18T10:20:07Z"}"#.to_owned();
        assert_eq!(send(&APP_SWARM, &clone, &queue, &[("c-1".into(), start), ("c-1".into(), first.clone())]).unwrap(), 2);
        // The same stop again (a re-send), a second stop, and the same lines in another's file.
        send(&APP_SWARM, &clone, &queue, &[("c-1".into(), first.clone()), ("c-1".into(), again.into()), ("w-2".into(), first.clone())]).unwrap();
        let read = |agent: &str| file(&remote, &format!("tracking/{agent}/work.jsonl"));
        assert_eq!(read("c-1").lines().filter(|l| l.contains("sub.stop")).collect::<Vec<_>>(), [first.as_str()]);
        assert_eq!(read("w-2").lines().collect::<Vec<_>>(), [first.as_str()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The race runs once, with the first project's settings: it is some 60 git processes, 45 s or
    /// more on a loaded Windows desktop. What differs for the other
    /// project, its commits' address, is checked with one send ([`each_projects_commits_are_its_own`]).
    #[test]
    fn events_append_to_each_agents_own_file_and_racing_pushes_both_land() {
        for project in [crate::project::fixtures::APP_SWARM] {
            let (dir, clone, remote) = repos(&format!("race-{}", project.name));
            // Two clones (two agents) race: each time the clone has read the branch and committed
            // on it, the other pushes first, twice in a row. The clone's push is rejected; it reads
            // the branch, commits on it again and lands. Neither conflicts; every event lands. The
            // first push makes the branch, with its README: the clone's first commit, a new root
            // like it, loses to it.
            let other = dir.join("clone2");
            git(&dir, &["clone", "--quiet", &remote, other.to_str().unwrap()], None, &[]).unwrap();
            let n = |i: u32| format!("{{\"n\":{i}}}");
            for i in 0..2 {
                let (other, q) = (other.clone(), dir.join("q-b.jsonl"));
                at(Point::Push, &clone, move || assert_eq!(send(&project, &other, &q, &[("b-2".into(), n(i))]), Ok(1)));
            }
            assert_eq!(send(&project, &clone, &dir.join("queue.jsonl"), &[("a-1".into(), n(0))]), Ok(1));
            assert_eq!(pending(), 0);
            assert!(file(&remote, "README.md").contains("# Swarm tracking"));
            let read = |agent: &str| file(&remote, &format!("tracking/{agent}/work.jsonl"));
            assert_eq!(read("a-1").lines().collect::<Vec<_>>(), [n(0)]);
            assert_eq!(read("b-2").lines().collect::<Vec<_>>(), [n(0), n(1)]);
            // Each push landed on the one before it: three sends, three commits in one line, each
            // by its first agent at the project's own `.invalid` domain.
            let branch = project.branches.tracking;
            assert_eq!(git(Path::new(&remote), &["rev-list", "--count", branch], None, &[]).unwrap().trim(), "3");
            let email = git(Path::new(&remote), &["log", "-1", "--format=%ae", branch], None, &[]).unwrap();
            assert_eq!(email.trim(), format!("a-1@{}.invalid", project.name));
            // The other clone lacks the clone's last commit, so a read of the branch fetches it,
            // into no ref; a branch the remote doesn't have reads as none.
            let (tip, src) = (git(Path::new(&remote), &["rev-parse", branch], None, &[]).unwrap(), format!("refs/heads/{branch}"));
            let refs = git(&other, &["for-each-ref"], None, &[]).unwrap();
            assert!(git(&other, &["cat-file", "-e", tip.trim()], None, &[]).is_err());
            assert_eq!(fetch_tip(&other, "origin", &src).unwrap().map(|t| t + "\n"), Some(tip.clone()));
            assert_eq!(git(&other, &["cat-file", "-t", tip.trim()], None, &[]).unwrap(), "commit\n");
            assert_eq!(fetch_tip(&other, "origin", "refs/heads/none").unwrap(), None);
            assert_eq!(git(&other, &["for-each-ref"], None, &[]).unwrap(), refs);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Each project's tracking commits are its own: on its own branch, by the agent at the
    /// project's own `.invalid` domain.
    #[test]
    fn each_projects_commits_are_its_own() {
        for project in BOTH {
            let (dir, clone, remote) = repos(&format!("own-{}", project.name));
            assert_eq!(send(&project, &clone, &dir.join("queue.jsonl"), &[("a-1".into(), "{\"n\":0}".into())]), Ok(1));
            let branch = project.branches.tracking;
            let email = git(Path::new(&remote), &["log", "-1", "--format=%ae", branch], None, &[]).unwrap();
            assert_eq!(email.trim(), format!("a-1@{}.invalid", project.name));
            assert_eq!(file(&remote, "tracking/a-1/work.jsonl").trim(), "{\"n\":0}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A second worktree of `clone` (which gets a first commit to check out).
    fn worktree(dir: &Path, clone: &Path) -> PathBuf {
        let ident: &[(&str, &str)] =
            &[("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t.invalid"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t.invalid")];
        git(clone, &["commit", "--quiet", "--allow-empty", "-m", "first"], None, ident).unwrap();
        let wt = dir.join("worktree");
        git(clone, &["worktree", "add", "--quiet", "--detach", wt.to_str().unwrap()], None, &[]).unwrap();
        wt
    }

    /// The `.lock` file another worktree of `repo` holds while it updates `name`.
    fn lock(repo: &Path, name: &str) -> PathBuf {
        let common = PathBuf::from(git(repo, &["rev-parse", "--path-format=absolute", "--git-common-dir"], None, &[]).unwrap().trim());
        let lock = common.join(format!("{name}.lock"));
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        lock
    }

    /// Sends and reads fetched the branch into the clone's
    /// one `refs/swarm-tracking/origin` (another consumer's: `refs/<name>-tracking/origin`) and git's
    /// `refs/remotes/origin/tracking`: after each push, the worktrees raced to update it, and the
    /// losers failed with "cannot lock ref"; then a read's ref of its own was, under load, gone
    /// when it read it. Now neither writes a ref, so every send and read succeeds while another
    /// worktree holds those refs; the sends interleaved so that each reads the branch while the
    /// other's push lands (deterministic, where threads racing for a while
    /// proved it by chance).
    #[test]
    fn two_worktrees_of_one_clone_send_and_read_while_another_holds_the_shared_refs() {
        let project = APP_SWARM;
        let (dir, clone, remote) = repos("worktrees");
        let wt = worktree(&dir, &clone);
        let (src, queue) = (format!("refs/heads/{}", project.branches.tracking), dir.join("queue.jsonl"));
        let n = |i: u32| format!("{{\"n\":{i}}}");
        let refs = git(&clone, &["for-each-ref"], None, &[]).unwrap();
        let held: Vec<PathBuf> =
            ["refs/remotes/origin/tracking", "refs/swarm-tracking/origin", "refs/gpu-swarm-tracking/origin"].iter().map(|r| lock(&clone, r)).collect();
        for l in &held {
            std::fs::write(l, "").unwrap();
        }
        // The worktree reads the branch (none yet) and commits; before it pushes, the clone sends,
        // making the branch, and both read. The worktree's push is rejected; it reads again and
        // lands.
        let (c, w, q, s) = (clone.clone(), wt.clone(), queue.clone(), src.clone());
        at(Point::Push, &wt, move || {
            assert_eq!(send(&project, &c, &q, &[("a-1".into(), n(0))]), Ok(1));
            for repo in [&c, &w] {
                assert!(fetch_tip(repo, "origin", &s).unwrap().is_some());
            }
        });
        assert_eq!(send(&project, &wt, &queue, &[("b-2".into(), n(0))]), Ok(1));
        assert_eq!(pending(), 0);
        assert_eq!(file(&remote, "tracking/a-1/work.jsonl").lines().collect::<Vec<_>>(), [n(0)]);
        assert_eq!(file(&remote, "tracking/b-2/work.jsonl").lines().collect::<Vec<_>>(), [n(0)]);
        // A read names the tip; the clone has it (the worktrees pushed it), so it fetches nothing.
        let tip = git(Path::new(&remote), &["rev-parse", project.branches.tracking], None, &[]).unwrap();
        assert_eq!(fetch_tip(&wt, "origin", &src).unwrap().map(|t| t + "\n"), Some(tip));
        // None of it wrote a ref (when a push can't lock the remote-tracking ref, git only warns:
        // the push has landed). That a held lock fails a fetch into its ref is
        // `a_fetch_into_a_shared_ref_waits_while_another_worktree_holds_it`.
        assert_eq!(git(&clone, &["for-each-ref"], None, &[]).unwrap(), refs);
        for l in &held {
            std::fs::remove_file(l).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fetch into a shared ref waits out another worktree's lock on it, and
    /// lands once the other lets go.
    #[test]
    fn a_fetch_into_a_shared_ref_waits_while_another_worktree_holds_it() {
        let (dir, clone, remote) = repos("lock");
        let branch = APP_SWARM.branches.tracking;
        // The remote's branch, made in place (a send would push).
        let ident: &[(&str, &str)] =
            &[("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t.invalid"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t.invalid")];
        let empty = git(Path::new(&remote), &["hash-object", "-t", "tree", "--stdin"], Some(b""), &[]).unwrap();
        let first = git(Path::new(&remote), &["commit-tree", empty.trim(), "-m", "first"], None, ident).unwrap();
        git(Path::new(&remote), &["update-ref", &format!("refs/heads/{branch}"), first.trim()], None, &[]).unwrap();
        let (src, shared) = (format!("refs/heads/{branch}"), "refs/swarm-test/shared");
        // Another worktree holds the ref: the fetch fails, waits and tries again, and lands once
        // the other lets go (the hook runs when the fetch has failed and is about to wait).
        let held = lock(&clone, shared);
        std::fs::write(&held, "").unwrap();
        let released = std::rc::Rc::new(std::cell::Cell::new(false));
        let r = released.clone();
        at(Point::Refetch, &clone, move || {
            std::fs::remove_file(&held).unwrap();
            r.set(true);
        });
        fetch(&clone, "origin", &src, shared).unwrap();
        assert!(released.get());
        let tip = git(Path::new(&remote), &["rev-parse", branch], None, &[]).unwrap();
        assert_eq!(git(&clone, &["rev-parse", shared], None, &[]).unwrap(), tip);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ls_remote_names_exactly_the_ref_asked_for() {
        let id = "0123456789abcdef0123456789abcdef01234567";
        let listing = format!("{}\trefs/heads/x/refs/heads/tracking\n{id}\trefs/heads/tracking\n", "f".repeat(40));
        assert_eq!(parse_ls_remote(&listing, "refs/heads/tracking").as_deref(), Some(id));
        assert_eq!(parse_ls_remote("", "refs/heads/tracking"), None);
        assert_eq!(parse_ls_remote("not-an-id\trefs/heads/tracking\n", "refs/heads/tracking"), None);
        assert_eq!(parse_ls_remote(&format!("{id} refs/heads/tracking"), "refs/heads/tracking"), None);
    }

    #[test]
    fn an_event_that_cant_be_sent_is_queued_and_sent_with_the_next() {
        let (dir, clone, remote) = repos("queue");
        let queue = dir.join("queue.jsonl");
        // No such remote: queued, not lost.
        git(&clone, &["remote", "set-url", "origin", dir.join("missing.git").to_str().unwrap()], None, &[]).unwrap();
        assert!(send(&APP_SWARM, &clone, &queue, &[("a-1".into(), "{\"n\":1}".into())]).is_err());
        let queued = std::fs::read_to_string(&queue).unwrap();
        assert_eq!(queued, "{\"agent\":\"a-1\",\"line\":\"{\\\"n\\\":1}\"}\n", "the queue's own lines are as they were");
        // An agent id that isn't a safe folder name is refused and never queued; one already in
        // the queue (hand-edited) is dropped rather than blocking the rest.
        assert!(send(&APP_SWARM, &clone, &queue, &[("../evil".into(), "{}".into())]).is_err());
        assert_eq!(std::fs::read_to_string(&queue).unwrap(), queued);
        std::fs::write(&queue, format!("{{\"agent\":\"../evil\",\"line\":\"{{}}\"}}\n{queued}not json\n")).unwrap();
        // Back online: the queued event goes first, then the new one; the queue empties.
        git(&clone, &["remote", "set-url", "origin", &remote], None, &[]).unwrap();
        assert_eq!(send(&APP_SWARM, &clone, &queue, &[("a-1".into(), "{\"n\":2}".into())]).unwrap(), 2);
        assert_eq!(file(&remote, "tracking/a-1/work.jsonl"), "{\"n\":1}\n{\"n\":2}\n");
        assert!(!queue.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each consumer's queue keeps the name it had before the move, so events queued by its old
    /// xtask are sent by the new one.
    #[test]
    fn each_projects_queue_keeps_its_name() {
        let (dir, clone, _) = repos("queue-name");
        for (project, name) in [(APP_SWARM, "swarm-tracking-queue.jsonl"), (GPU_SWARM, "gpu-tracking-queue.jsonl")] {
            let q = queue_path(&project, &clone);
            assert_eq!(q.file_name().and_then(|n| n.to_str()), Some(name));
            assert_eq!(q.parent().map(Path::to_path_buf), Some(common_dir(&clone)));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `cargo xtask event` end to end, in a clone, for each consumer: `session.start` logs to its
    /// tracking branch, an event that can't be sent is queued (and the command still succeeds),
    /// and `flush` sends it. The project's prefix is one no environment sets, so the test reads
    /// neither the caller's agent nor its `TRACKING=off`.
    #[test]
    fn run_logs_an_event_queues_what_cant_be_sent_and_flushes_it() {
        for base in BOTH {
            let project = Project { env_prefix: "SWARM_CORE_TEST_NEVER_SET", ..base };
            let (dir, clone, remote) = repos(&format!("run-{}", base.name));
            run(&project, &clone, &["session.start", "--agent", "pc-r1", "--name", "rowan", "--role", "worker", "--effort", "high"]).unwrap();
            let logged = file(&remote, "tracking/pc-r1/work.jsonl");
            let v: Value = serde_json::from_str(logged.trim()).unwrap();
            assert_eq!((v["event"].as_str(), v["format"].as_u64(), v["effort"].as_str()), (Some("session.start"), Some(3), Some("high")));
            assert!(run(&project, &clone, &["work", "--issue", "1"]).unwrap_err().contains("SWARM_CORE_TEST_NEVER_SET_AGENT"));
            git(&clone, &["remote", "set-url", "origin", dir.join("missing.git").to_str().unwrap()], None, &[]).unwrap();
            run(&project, &clone, &["work", "--agent", "pc-r1", "--issue", "7"]).unwrap();
            assert!(queue_path(&project, &clone).exists());
            git(&clone, &["remote", "set-url", "origin", &remote], None, &[]).unwrap();
            run(&project, &clone, &["flush"]).unwrap();
            assert!(!queue_path(&project, &clone).exists());
            assert_eq!(file(&remote, "tracking/pc-r1/work.jsonl").lines().count(), 2);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
