//! The rules of the run, one function each. Every call goes through [`ask`], which catches a panic
//! and checks that a failure is complete, so those two rules hold for every call the others make.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};

use super::cases::{self, Case, Key, UNKNOWN_ID, adversarial_params};
use super::values::{self, Schema};
use super::{App, Call, CommandInfo, Enabled, Example, Failure, Outcome, REQUEST_CAP, Rule, Surface, Violation, Violations};

/// The commands a fresh session has disabled, by id, each with why: what the first surface that
/// answers the enabled query says ([`enabled`]).
pub(super) type Disabled = BTreeMap<String, Failure>;

/// The violations so far. A detail is `<topic>: <what was seen>`; a topic already reported for the
/// same rule, command and surface is not reported again, so one defect met by a hundred calls is
/// one line.
#[derive(Default)]
pub(super) struct Found {
    list: Vec<Violation>,
    seen: BTreeSet<(Rule, String, Option<Surface>, String)>,
}

impl Found {
    pub(super) fn add(&mut self, rule: Rule, command: &str, surface: Option<Surface>, detail: String) {
        let topic = detail.split(": ").next().unwrap_or_default().to_owned();
        if self.seen.insert((rule, command.to_owned(), surface, topic)) {
            self.list.push(Violation { rule, command: command.to_owned(), surface, detail });
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub(super) fn into_violations(self) -> Violations {
        Violations(self.list)
    }
}

/// `value` as a message shows it: short.
fn shown(value: &impl ToString) -> String {
    let text = value.to_string();
    let head: String = text.chars().take(80).collect();
    if text.chars().nth(80).is_some() { format!("{head}…") } else { head }
}

/// An outcome as a message shows it.
fn show(outcome: &Outcome) -> String {
    match outcome {
        Ok(value) => format!("a result {}", shown(value)),
        Err(f) => match &f.key {
            Some(key) => format!("`{}` (key `{key}`)", f.code),
            None => format!("`{}`", f.code),
        },
    }
}

/// What a panic said.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned());
    shown(&text.unwrap_or_else(|| "(no message)".to_owned()))
}

/// One call through one surface: `None` when it panicked (a violation of `no-panic`). A failure
/// without a code or a remediation is a violation of `error-shape`.
fn ask(app: &dyn App, call: Call<'_>, found: &mut Found) -> Option<Outcome> {
    let Call { surface, id, params, .. } = call;
    match catch_unwind(AssertUnwindSafe(|| app.call(call))) {
        Ok(outcome) => {
            if let Err(f) = &outcome
                && (f.code.trim().is_empty() || f.remediation.trim().is_empty())
            {
                let detail = format!("a failure without a code or a remediation: with {} it gave {f:?}", shown(params));
                found.add(Rule::ErrorShape, id, Some(surface), detail);
            }
            Some(outcome)
        }
        Err(payload) => {
            found.add(Rule::NoPanic, id, Some(surface), format!("the call panicked: with {} it panicked: {}", shown(params), panic_message(&*payload)));
            None
        }
    }
}

/// The same call, with `consent` or without, through every surface that can carry `params`.
fn ask_all(app: &dyn App, id: &str, params: &Value, consent: bool, found: &mut Found) -> Vec<(Surface, Outcome)> {
    let surfaces: Vec<Surface> = app.surfaces().into_iter().filter(|s| app.accepts(*s, params)).collect();
    surfaces.into_iter().filter_map(|surface| ask(app, Call { surface, id, params, consent }, found).map(|outcome| (surface, outcome))).collect()
}

/// What a refused call must be.
struct Expect {
    rule: Rule,
    code: &'static str,
    key: Key,
}

impl Expect {
    /// Records how `outcome` of the call `id` through `surface` falls short of it. `topic` says what
    /// the call was.
    fn judge(&self, found: &mut Found, id: &str, surface: Surface, topic: &str, outcome: &Outcome) {
        let detail = match outcome {
            Ok(value) => format!("{topic}: it succeeded with {} but should fail `{}`", shown(value), self.code),
            Err(f) if f.code != self.code => format!("{topic}: it failed `{}` but should fail `{}`", f.code, self.code),
            Err(f) => match self.key.problem(f.key.as_deref()) {
                Some(problem) => format!("{topic}: {problem}"),
                None => return,
            },
        };
        found.add(self.rule, id, Some(surface), detail);
    }
}

/// An id by the id rule: `<area>.<name>`, two or more segments `[a-z][a-z0-9_]*`, 64 characters at most.
fn id_problem(id: &str) -> Option<&'static str> {
    let segment_ok = |s: &str| {
        let mut bytes = s.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_lowercase()) && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    if id.len() > 64 {
        Some("it is longer than 64 characters")
    } else if !id.contains('.') {
        Some("it needs two segments, `<area>.<name>`")
    } else if !id.split('.').all(segment_ok) {
        Some("each segment is `[a-z][a-z0-9_]*`")
    } else {
        None
    }
}

/// `schemas`: every command has an id by the id rule, and a params and a result schema.
pub(super) fn schemas(commands: &[CommandInfo], found: &mut Found) {
    if commands.is_empty() {
        found.add(Rule::Schemas, "-", None, "the app has no commands: it needs at least one to run through the surfaces".to_owned());
    }
    let mut ids = BTreeSet::new();
    for command in commands {
        if !ids.insert(command.id.as_str()) {
            found.add(Rule::Schemas, &command.id, None, "a duplicate id: ids are unique".to_owned());
        }
        if let Some(why) = id_problem(&command.id) {
            found.add(Rule::Schemas, &command.id, None, format!("an id that breaks the id rule: {why}"));
        }
        for (what, schema) in [("params", &command.params), ("result", &command.result)] {
            if !schema.as_object().is_some_and(|o| !o.is_empty()) {
                found.add(Rule::Schemas, &command.id, None, format!("no {what} schema: `commands` gave {}", shown(schema)));
            }
        }
    }
}

/// `mcp-descriptors`: a tool per command, named by the id rule, its `inputSchema` the params schema,
/// and no tool without a command.
pub(super) fn mcp_descriptors(app: &dyn App, commands: &[CommandInfo], found: &mut Found) {
    if !app.surfaces().contains(&Surface::Mcp) {
        return;
    }
    let tools = match app.mcp_tools() {
        Ok(tools) => tools,
        Err(e) => return found.add(Rule::McpDescriptors, "-", Some(Surface::Mcp), format!("`tools/list` failed: {e}")),
    };
    let mut by_name: BTreeMap<&str, &Value> = BTreeMap::new();
    for tool in &tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            found.add(Rule::McpDescriptors, "-", Some(Surface::Mcp), format!("a tool without a name: {}", shown(tool)));
            continue;
        };
        if by_name.insert(name, tool).is_some() {
            found.add(Rule::McpDescriptors, name, Some(Surface::Mcp), "a tool listed twice: tool names are unique".to_owned());
        }
    }
    let names: BTreeSet<String> = commands.iter().map(|c| c.id.replace('.', "-")).collect();
    for command in commands {
        let name = command.id.replace('.', "-");
        match by_name.get(name.as_str()) {
            None => found.add(
                Rule::McpDescriptors,
                &command.id,
                Some(Surface::Mcp),
                format!("no tool `{name}`: a tool per command, named by the id with each `.` as `-`"),
            ),
            Some(tool) if tool.get("inputSchema") != Some(&command.params) => found.add(
                Rule::McpDescriptors,
                &command.id,
                Some(Surface::Mcp),
                format!(
                    "the tool `{name}`'s inputSchema is not the command's params schema: it is {}",
                    shown(&tool.get("inputSchema").unwrap_or(&Value::Null))
                ),
            ),
            Some(_) => {}
        }
    }
    for name in by_name.keys().filter(|n| !names.contains(**n)) {
        found.add(Rule::McpDescriptors, name, Some(Surface::Mcp), "a tool no command makes: tools are generated from the registry".to_owned());
    }
}

/// `unknown-command`: an id no command has is `unknown_command` on every surface.
pub(super) fn unknown_command(app: &dyn App, found: &mut Found) {
    let expect = Expect { rule: Rule::UnknownCommand, code: "unknown_command", key: Key::Any };
    for id in [UNKNOWN_ID, "no_such_command"] {
        for (surface, outcome) in ask_all(app, id, &json!({}), false, found) {
            expect.judge(found, id, surface, "an unknown command", &outcome);
        }
    }
}

/// `bad-params`: what the command's schema says is wrong is `bad_params` on every surface.
pub(super) fn bad_params(app: &dyn App, command: &CommandInfo, found: &mut Found) {
    let expect_for = |case: &Case| Expect { rule: Rule::BadParams, code: "bad_params", key: case.key.clone() };
    for case in cases::bad_params(&command.params, app.names_key_for_wrong_type()) {
        let expect = expect_for(&case);
        for (surface, outcome) in ask_all(app, &command.id, &case.params, false, found) {
            expect.judge(found, &command.id, surface, &case.what, &outcome);
        }
    }
}

/// `over-cap`: a request over the cap is `too_large` on every surface that reads capped lines.
pub(super) fn over_cap(app: &dyn App, commands: &[CommandInfo], found: &mut Found) {
    let Some(first) = commands.first() else { return };
    let params = json!({ "conformance_padding": "a".repeat(REQUEST_CAP) });
    let expect = Expect { rule: Rule::OverCap, code: "too_large", key: Key::Any };
    let surfaces = app.surfaces();
    for surface in app.capped_surfaces().into_iter().filter(|s| surfaces.contains(s) && app.accepts(*s, &params)) {
        if let Some(outcome) = ask(app, Call { surface, id: &first.id, params: &params, consent: false }, found) {
            expect.judge(found, &first.id, surface, "a request over the 1 MiB cap", &outcome);
        }
    }
}

/// A command's state, as a message shows it.
fn show_state(why: Option<&Failure>) -> String {
    match why {
        None => "enabled".to_owned(),
        Some(f) => format!("disabled (`{}`, \"{}\")", f.code, shown(&f.remediation)),
    }
}

/// `enabled`: every surface that answers the enabled query gives one state per command, each
/// disabled one `unavailable` with a remediation, and the same states as the first that answers.
/// The disabled commands, as that first one says.
pub(super) fn enabled(app: &dyn App, commands: &[CommandInfo], found: &mut Found) -> Disabled {
    let mut first: Option<(Surface, BTreeMap<String, Option<Failure>>)> = None;
    for surface in app.surfaces() {
        let states = match app.enabled(surface) {
            None => continue,
            Some(Ok(states)) => states,
            Some(Err(e)) => {
                found.add(Rule::Enabled, "-", Some(surface), format!("the states could not be asked: {e}"));
                continue;
            }
        };
        let by_id = one_state_each(surface, commands, states, found);
        match &first {
            None => first = Some((surface, by_id)),
            Some((first_surface, want)) => states_agree(*first_surface, want, surface, &by_id, found),
        }
    }
    first.map(|(_, by_id)| by_id.into_iter().filter_map(|(id, why)| why.map(|why| (id, why))).collect()).unwrap_or_default()
}

/// The states `surface` gave, by id: one per command, none for an id no command has, each disabled
/// one `unavailable` with a remediation.
fn one_state_each(surface: Surface, commands: &[CommandInfo], states: Vec<Enabled>, found: &mut Found) -> BTreeMap<String, Option<Failure>> {
    let mut by_id = BTreeMap::new();
    for Enabled { id, why } in states {
        if !commands.iter().any(|c| c.id == id) {
            found.add(Rule::Enabled, &id, Some(surface), "a state for no command: the states are the registry's".to_owned());
        }
        if let Some(why) = &why
            && (why.code != "unavailable" || why.remediation.trim().is_empty())
        {
            found.add(Rule::Enabled, &id, Some(surface), format!("a disabled state that isn't `unavailable` with a remediation: it is {why:?}"));
        }
        if by_id.insert(id.clone(), why).is_some() {
            found.add(Rule::Enabled, &id, Some(surface), "two states: a command has one".to_owned());
        }
    }
    for command in commands.iter().filter(|c| !by_id.contains_key(&c.id)) {
        found.add(Rule::Enabled, &command.id, Some(surface), "no state: every command has one".to_owned());
    }
    by_id
}

/// `surface`'s states are `first`'s: the same commands enabled, the same disabled for the same reason.
fn states_agree(first: Surface, want: &BTreeMap<String, Option<Failure>>, surface: Surface, got: &BTreeMap<String, Option<Failure>>, found: &mut Found) {
    for (id, why) in got {
        let Some(wanted) = want.get(id) else { continue };
        let same = match (wanted, why) {
            (None, None) => true,
            (Some(a), Some(b)) => a.code == b.code && a.remediation == b.remediation,
            _ => false,
        };
        if !same {
            let detail = format!("the surfaces disagree: {first} says {} but {surface} says {}", show_state(wanted.as_ref()), show_state(why.as_ref()));
            found.add(Rule::Enabled, id, Some(surface), detail);
        }
    }
}

/// `enabled`: a call of a disabled command (valid params, with consent) fails `unavailable` with the
/// remediation its state gave, on every surface that carried the call.
fn refused_as_disabled(command: &CommandInfo, why: &Failure, params: &Value, seen: &[(Surface, Outcome)], found: &mut Found) {
    for (surface, outcome) in seen {
        let detail = match outcome {
            Err(f) if f.code == "unavailable" && f.remediation == why.remediation => continue,
            Err(f) if f.code == "unavailable" => format!(
                "a disabled command's call gives another reason than its state: with {} the call says \"{}\" but the state \"{}\"",
                shown(params),
                shown(&f.remediation),
                shown(&why.remediation)
            ),
            other => format!("a disabled command's call isn't refused `unavailable`: with {} it gave {}", shown(params), show(other)),
        };
        found.add(Rule::Enabled, &command.id, Some(*surface), detail);
    }
}

/// Whether two outcomes are the same: equal results, or failures of the same code and key.
fn same(a: &Outcome, b: &Outcome) -> bool {
    match (a, b) {
        (Ok(a), Ok(b)) => a == b,
        (Err(a), Err(b)) => a.code == b.code && a.key == b.key,
        _ => false,
    }
}

/// `same-result`: the first surface's outcome is every other's.
fn agree(id: &str, params: &Value, seen: &[(Surface, Outcome)], found: &mut Found) {
    let Some((first_surface, first)) = seen.first() else { return };
    for (surface, outcome) in seen.iter().skip(1).filter(|(_, o)| !same(first, o)) {
        let detail = format!("the surfaces disagree: with {} {first_surface} says {} but {surface} says {}", shown(params), show(first), show(outcome));
        found.add(Rule::SameResult, id, Some(*surface), detail);
    }
}

/// `no-panic`, `error-shape`, `same-result`, `consent`, `examples` and `enabled`'s calls: every
/// command with `{}` and every adversarial param, without consent; its params generated from its
/// schema and the app's examples of it, with consent (and, for a command that needs consent,
/// without). A command in `disabled` is held to its refusal instead of reaching `run`. The calls
/// made.
pub(super) fn every_surface_agrees(app: &dyn App, commands: &[CommandInfo], disabled: &Disabled, found: &mut Found) -> usize {
    let mut hostile = vec![json!({})];
    hostile.extend(adversarial_params());
    let examples = app.examples();
    let mut calls = 0;
    for command in commands {
        for params in &hostile {
            let seen = ask_all(app, &command.id, params, false, found);
            agree(&command.id, params, &seen, found);
            calls += seen.len();
        }
        let own: Vec<&Example> = examples.iter().filter(|e| e.id == command.id).collect();
        let why = disabled.get(&command.id);
        calls += reach_run(app, command, !own.is_empty(), why, found);
        calls += own.into_iter().map(|example| one_example(app, command, example, why, found)).sum::<usize>();
    }
    for example in examples.iter().filter(|e| !commands.iter().any(|c| c.id == e.id)) {
        found.add(Rule::Examples, &example.id, None, "an example of a command the registry doesn't have".to_owned());
    }
    calls
}

/// What a valid call with consent gave: the outcomes of the surfaces that carried the consent,
/// and how many calls were made in all.
struct Valid {
    seen: Vec<(Surface, Outcome)>,
    calls: usize,
}

impl Valid {
    /// Whether it ran somewhere: a surface gave a result.
    fn ran(&self) -> bool {
        self.seen.iter().any(|(_, outcome)| outcome.is_ok())
    }
}

/// One valid call, with consent, through every surface: the surfaces that carry the consent agree;
/// for a command that needs consent, one that can't carry it ([`App::consents`]) refuses.
fn valid_call(app: &dyn App, command: &CommandInfo, params: &Value, found: &mut Found) -> Valid {
    let seen = ask_all(app, &command.id, params, true, found);
    let calls = seen.len();
    let (seen, refusing): (Vec<_>, Vec<_>) = seen.into_iter().partition(|(surface, _)| !command.needs_consent || app.consents(*surface));
    agree(&command.id, params, &seen, found);
    let valid = Valid { seen, calls };
    for (surface, outcome) in &refusing {
        if let Some(detail) = unrefused(params, outcome, valid.ran(), "with consent, on a surface that can't carry it") {
            found.add(Rule::Consent, &command.id, Some(*surface), detail);
        }
    }
    valid
}

/// `consent`: a call of a command that needs consent, made without it (`how` says why), never runs:
/// it is `consent_required`, or another failure when the call wouldn't have run anyway (`ran`
/// false: its params or a requirement refused it with consent too). What `outcome` does instead.
fn unrefused(params: &Value, outcome: &Outcome, ran: bool, how: &str) -> Option<String> {
    match outcome {
        Ok(value) => Some(format!("{how}: it ran with {} and gave {}, but should fail `consent_required`", shown(params), shown(value))),
        Err(f) if ran && f.code != "consent_required" => {
            Some(format!("{how}: it failed `{}` with {} but should fail `consent_required`", f.code, shown(params)))
        }
        Err(_) => None,
    }
}

/// The call of a command that needs consent, without it, on every surface: each refuses, and the
/// same way. `ran`: the call with consent ran. The calls made.
fn without_consent(app: &dyn App, command: &CommandInfo, params: &Value, ran: bool, found: &mut Found) -> usize {
    let seen = ask_all(app, &command.id, params, false, found);
    agree(&command.id, params, &seen, found);
    for (surface, outcome) in &seen {
        if let Some(detail) = unrefused(params, outcome, ran, "without consent") {
            found.add(Rule::Consent, &command.id, Some(*surface), detail);
        }
    }
    seen.len()
}

/// Valid params, generated from the command's schema, through every surface with consent, so its
/// `run` is reached: a command with required params that none of them gets to succeed, and that has
/// no example, is a finding of `examples`. A disabled command (`disabled`: why) reaches no `run`:
/// its first call, the plainest params, must be refused as its state says. The calls made.
fn reach_run(app: &dyn App, command: &CommandInfo, has_example: bool, disabled: Option<&Failure>, found: &mut Found) -> usize {
    let required = Schema::root(&command.params).resolved().is_some_and(|top| !top.required().is_empty());
    let needs_example = required && !has_example && disabled.is_none();
    let generated = match values::valid_params(&command.params) {
        Ok(generated) => generated,
        Err(why) => {
            if needs_example {
                found.add(Rule::Examples, &command.id, None, format!("no call reaches `run`: {why}; add a known-good call to `App::examples`"));
            }
            return 0;
        }
    };
    let (mut calls, mut ran) = (0, false);
    for (n, params) in generated.iter().enumerate() {
        let valid = valid_call(app, command, params, found);
        calls += valid.calls;
        if n == 0 && command.needs_consent {
            calls += without_consent(app, command, params, valid.ran(), found);
        }
        if let Some(why) = disabled.filter(|_| n == 0) {
            refused_as_disabled(command, why, params, &valid.seen, found);
        }
        ran |= valid.ran();
    }
    if !ran && needs_example {
        let detail = format!(
            "no call reaches `run`: none of the {} calls made from its params schema succeeded on any surface; add a known-good call to `App::examples`",
            generated.len()
        );
        found.add(Rule::Examples, &command.id, None, detail);
    }
    calls
}

/// `examples`: a known-good call succeeds, and the same, on every surface that carries its consent;
/// for a command that needs consent, it is refused without. Of a disabled command (`disabled`: why),
/// it is refused as its state says. The calls made.
fn one_example(app: &dyn App, command: &CommandInfo, example: &Example, disabled: Option<&Failure>, found: &mut Found) -> usize {
    let valid = valid_call(app, command, &example.params, found);
    if let Some(why) = disabled {
        refused_as_disabled(command, why, &example.params, &valid.seen, found);
    }
    for (surface, outcome) in valid.seen.iter().filter(|_| disabled.is_none()) {
        if outcome.is_err() {
            let detail = format!("an example that fails: with {} it gave {}", shown(&example.params), show(outcome));
            found.add(Rule::Examples, &example.id, Some(*surface), detail);
        }
    }
    let without = if command.needs_consent { without_consent(app, command, &example.params, valid.ran(), found) } else { 0 };
    valid.calls + without
}
