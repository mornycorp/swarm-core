# The tracking format

> **Last reviewed:** 2026-10-30 · **Last updated:** 2026-11-02 · **Change:** minor (a failed `ready`'s cause and step; no format change) · **Target:** every consumer's `tracking` branch

**Current version: 3** ([Versions](#versions)). `cargo xtask event` writes it as `format` on every
`session.start`.

The contract every swarm on Swarm Core writes and a timeline viewer reads. The writer is this
crate's `event` module (`crates/swarm-core/src/event.rs`), which every consumer's
`cargo xtask event` runs, so one writer serves every swarm. Its `event::examples()` are every
kind of line it writes, for a reader's round-trip test. The writer refuses a field a reader would
drop or ignore where it is (a `sub` on a session event, an `of` off a `release` or
`session.stop`, an `of` naming the logger itself, a `parent` off a `session.start` or naming its
own instance, `reason: "duplicate"` off a `release` or `session.stop`, `model` or `effort` off a
`session.start` or `sub.start`). It also never appends what an agent's file already holds: the
very same line (a re-sent event that had landed) or a second `sub.stop` of one sub-agent; a
reader should still take both if an older writer wrote them.

Every swarm writes this contract, so a change to it is done only when [the rule](#versions)
holds. How a reader draws it (lanes, outcomes, statistics) is the reader's own.

## The branch

- A branch named `tracking` (never merged; no CI runs on it). A reader reads it from a clone's
  remote-tracking ref `refs/remotes/origin/tracking`, else a local `refs/heads/tracking`, and
  never writes to it.
- One file per agent instance: `tracking/<instance>/work.jsonl`. The instance id is the machine
  name and a random suffix (`mxv9wfm7rr-2021a6`); ids are letters, digits, `.`, `_` and `-`.
  Only that instance writes its file. Other files on the branch (its `README.md`) are ignored.
- JSON Lines, oldest first, one event a line.

## Events

Every event has `t` (UTC, `YYYY-MM-DDTHH:MM:SSZ`, the agent's own clock) and `event`:

| `event` | Fields | Means |
|---|---|---|
| `session.start` | `name`, `role` (`worker`, `coordinator`), `machine`, `parent`?, `model`?, `effort`?, `format`? | the instance begins; `parent` is the instance id of the session that started it (absent for a top-level agent); `model` and `effort` [below](#model-and-effort); `format` the version of this contract its writer wrote ([Versions](#versions); none: 1) |
| `session.pause` / `session.resume` | | it waits (for the owner, say) and goes on |
| `session.stop` | `reason`?, `of`? | it ends; `reason` `duplicate`: because another agent has its issue; `of`: the dead instance whose session the coordinator ends |
| `claim` | `issue`, `mr` | it takes an issue |
| `work` | `issue` | it starts or resumes work on an issue |
| `submit` | `issue`, `mr` | it marks the PR ready |
| `stop` | `issue`, `reason` (`handoff`, `blocked`, `paused`, `done`) | it stops working on the issue |
| `release` | `issue`, `reason`?, `of`? | it gives the issue up, or the coordinator freed a stale claim; `reason` `duplicate`: another agent has the issue, so the work so far is thrown away; `of`: the instance whose claim or work it ends, when not the logger's own |
| `merge`, `sendback` | `issue`, `mr` | the coordinator merged a PR or sent it back |
| `sub.start` | `sub`, `name`?, `model`?, `effort`? | a helper sub-agent (a search, a review, a gate) begins, inside the session |
| `sub.stop` | `sub` | it ends |

`mr` is the pull request's number (the field kept its name from the format's GitLab days). Any
event may carry a one-line `note`. Any issue event (`claim` … `sendback`) may carry `sub`: that
sub-agent did it.

**Sub-agents and started sessions** (additions only, so older files still read):

- A session the coordinator **starts** has its own file, as every instance does; its
  `session.start` names the starter's instance id as `parent`:
  ```json
  {"event":"session.start","machine":"cmaughan-dt9","name":"worker-seek","parent":"cmaughan-dt9-719698","role":"worker","t":"2026-10-17T14:00:00Z"}
  ```
- A **helper sub-agent** gets no file: its parent logs it in its own. `sub` is unique within the
  file; neither `sub.*` event needs an `issue`; a session event never carries a `sub`. A
  coordinator logs each local gate run this way:
  ```json
  {"event":"sub.start","name":"gate","sub":"gate-1","t":"2026-10-17T14:01:00Z"}
  {"event":"work","issue":225,"sub":"gate-1","t":"2026-10-17T14:01:05Z"}
  {"event":"sub.stop","sub":"gate-1","t":"2026-10-17T14:09:00Z"}
  ```

**Duplicate work** (additions only): an agent that finds its issue taken releases
it with `reason: "duplicate"` (and a note saying who has it); the coordinator, stopping another
agent's work (or freeing a stale claim), logs the `release` in its own file and names whose work
ends with `of`:

```json
{"event":"release","issue":226,"note":"claude-mac had filed #218 2.5 h earlier","reason":"duplicate","t":"2026-10-17T14:34:21Z"}
{"event":"release","issue":218,"of":"mxv9wfm7rr-2021a6","reason":"duplicate","note":"worker-seek has it","t":"2026-10-17T15:00:00Z"}
```

A duplicate issue is closed on GitHub with `Duplicate of #<original>`.

**A dead agent's session** (additions only): an agent that dies (its machine rebooted, its
process killed) never logs its own `session.stop`, so its lane would stay live. The coordinator,
once it has checked the agent is dead, ends that session from its own file, naming it with `of`:

```json
{"event":"session.stop","of":"mxv9wfm7rr-2021a6","note":"its machine rebooted; silent 17 h","t":"2026-10-18T03:00:00Z"}
```

The named instance's session ends there, as if it had logged the stop itself. The event is a
mark on the coordinator's own session, which runs on. An `of` naming no known instance is only
that mark.

### Model and effort

Additions only: `session.start` and `sub.start` may name the model running
the session or sub-agent (`model`, an id such as `claude-opus-5-5`) and its reasoning effort
(`effort`: `low`, `medium`, `high`, `xhigh` or `max`), so the owner can compare models. No
other event may carry them; both are optional, and a reader shows **unknown** where they are
missing (every event logged before them).

```json
{"event":"session.start","effort":"high","machine":"cmaughan-dt9","model":"claude-opus-5-5","name":"cedar","parent":"cmaughan-dt9-bbf753","role":"worker","t":"2026-10-17T21:00:00Z"}
{"event":"sub.start","model":"claude-sonnet-5-5","name":"gate","sub":"gate-1","t":"2026-10-17T21:01:00Z"}
```

The writer checks them: `--model` is an id (letters, digits, `-`, `_`, `.`),
`--effort` one of the five, and either on any other event is refused.

### Gates

A convention, not new fields: still version 3. A coordinator's local gate is a sub-agent named
`gate` (`sub` beginning `gate`), and `cargo xtask gate` writes its notes this way, so a reader's
gate analytics can read them:

- **`sub.start`** `note`: the coordinator's text naming what it gates (`local gate: #<m> (#<i>)`,
  `docs gate: #<m>`; a rerun's begins `rerun of <sub>:`), then `; kind=<kind>` (the gate that
  ran: `docs`, `tooling`, `smoke` or `full`), then `; capacity=<n> free=<GB>`: the builds
  `cargo xtask capacity` suggested at the start and the free memory in GB, one decimal (`?` when
  it couldn't be read), then, for a gate that ran without the machine gate lock (a `docs` gate),
  `; machineLock=skipped`. The coordinator's text is cut so the tail always fits.
- **`sub.stop`** `note`: `rc=<n>`, the gate's exit code, first. A failure adds
  `; cause=<cause>`, read from the gate's log: `memory` (os error 1450 or 1455), `link`
  (LNK1104), `toolchain` (`can't find crate for std` or `core`), `git-temp` (git failing in a
  test's temporary repository) are **transient**, the machine's and not the change's; `test`,
  `build` and `other` are **real**. A pass adds what it merged: `rc=0; merged #<m> …`. Both
  carry the gate's kind, `; kind=<kind>`, after the cause or before what it merged. A gate
  stopped part way, before its result (`gate --stop` with no `<sub>.rc`), has **no `rc`**: it
  is stopped, never passed (sc#3).
- **A gate with no kind** comes from an older writer: a reader may infer one from its words
  (`docs gate:`, a `gate-docs` sub, `ci --changed (docs-only)`), else take it as the full
  `cargo xtask ci`. A kind a note names that the reader doesn't know stays unknown.
- **The periodic full gate of `main`** ([the swarm protocol › The periodic full
  gate](../crates/swarm-core/assets/docs/swarm-protocol.md#the-periodic-full-gate)) is a gate
  too, `sub` `gate-full-<short commit>`, whose notes end `; kind=full; commit=<sha>`: its start
  `full gate of main; capacity=<n> free=<GB>; kind=full; commit=<sha>`, its stop
  `rc=<n>[; cause=<cause>]; kind=full; commit=<sha>`, or, when it never ran,
  `superseded: <why>; …` or `abandoned: <error>; …` with no `rc`. A gate whose notes name a
  `commit` tested `main` at that commit; it merges nothing, and one stopped `superseded` or
  `abandoned` is stopped, not failed. Together they are `main`'s health: a commit one passed is
  green until the next landing, one that failed for the change's cause red until its `main-red`
  issue merges.
- **A landed full gate** (sc#15): when the coordinator's `gate --full` lands and leaves `main` at
  exactly the tree it tested, `land` closes it with `main=<sha>`, its stop
  `rc=0; kind=full; main=<sha>; merged #<m> …`. The periodic full gate counts that as a
  pass of `main` at `<sha>`. It is still the coordinator's gate, which merged, so it names no
  `commit`: a reader that colours `main`'s health may take `main=` as a pass of that commit.
- **The merges** a gate allows are logged with its `sub`, between its `sub.start` and its
  `sub.stop`.
- **A rerun** (`--retry-transient`, or `--rerun-failed <sub>`: the failed tests alone and the
  steps they skipped) is a gate of its own, `<sub>-rerun`, its start note `rerun of <sub>: …`.
- **A worker's own gate before it submits** is not one of these: `cargo xtask ready` runs it
  inside a sub named `ready` (`sub.start --sub ready-<short head> --name ready --note "ci
  --changed before submitting"`), so a reader's gate counts keep to the coordinator's gates. Its
  `sub.stop` is `rc=0`, or a failure's `rc=<n>; cause=<cause>; step=<step>`: the exit code, the
  cause as above and the `ci` step it failed at (the `CI summary`'s first `FAIL`), each left out
  when unknown (`rc=1` alone when `ci` couldn't run). An older writer's failure is `rc=1` alone:
  its cause unknown.

```json
{"event":"sub.start","name":"gate","note":"local gate: #154 (#141); capacity=1 free=9.8","sub":"gate-mr154","t":"2026-10-20T10:44:29Z"}
{"event":"sub.stop","note":"rc=101; cause=toolchain","sub":"gate-mr154","t":"2026-10-20T10:45:25Z"}
{"event":"sub.start","name":"gate","note":"rerun of gate-mr154: local gate: #154 (#141); capacity=1 free=9.6","sub":"gate-mr154-rerun","t":"2026-10-20T10:46:19Z"}
{"event":"merge","issue":141,"mr":154,"sub":"gate-mr154-rerun","t":"2026-10-20T10:52:17Z"}
{"event":"sub.stop","note":"rc=0; merged #154","sub":"gate-mr154-rerun","t":"2026-10-20T10:52:41Z"}
```

### Send-backs

A convention, not new fields: still version 3. The coordinator returns a PR to its worker
instead of merging it in two ways, and a reader may count both as a **send-back**:

- **A gate fails for the change's cause** (`rc≠0` and a cause that isn't transient). Each PR
  the gate gated is sent back once, unless its next gate is a rerun saying so (`rerun of <sub>`,
  whose result counts instead: a flake a rerun answers is no send-back). A rerun naming no PR
  gates those of the gate it reruns.
- **A `sendback` event** (`sendback --issue <n> --mr <m> --note "<why>"`, [the swarm protocol ›
  The coordinator's loop](../crates/swarm-core/assets/docs/swarm-protocol.md#the-coordinators-loop)).
  A conflict at `stack`, before any gate ran, is one: its note starts `cause=conflict`. Its cause
  is the note's `cause=<cause>`, else the cause of the failed gate its `sub` names. A `sendback`
  after a gate's failure of the PR, before its next gate, is that same send-back.

A PR is **gated** when a gate names it (its notes, its merges) or a `sendback` sends it back;
**merged** when a `merge` event names it or a gate that passed says `merged #<m>` (a docs gate
logs no `merge`).

## Versions

The contract has a version, a whole number. The writer names it as `format` on every
`session.start`; a reader takes an instance's from its first `session.start` naming one, and a
file with none is version 1. Versions only add: every older file still reads.

| Version | Since | What it added |
|---|---|---|
| 1 | the first writer | The format: `session.*`, `claim`, `work`, `submit`, `stop`, `release`, `merge`, `sendback`, `note` |
| 2 | sub-agents, duplicate work, models | `parent` on `session.start`; helper sub-agents (`sub.start`, `sub.stop`, `sub`); duplicate work (`reason: "duplicate"` on `release` and `session.stop`, `of` on `release`); `model` and `effort` on `session.start` and `sub.start` |
| 3 | dead sessions, declared versions | `of` on `session.stop` (the coordinator ends a dead agent's session); `format` on `session.start` |

No file written before version 3 names its version, so a version-2 file reads as 1: what it holds
says which additions it uses, and a reader should take each field where it finds it, whatever the
version. A reader that checks fields against versions should judge a file declaring none as
version 2 (versions 1 and 2 never named themselves, so only a version-3 field is out of place
there).

**Newer than the reader.** An instance whose `format` is above the version a reader knows was
written by a newer writer. A reader should keep its events: an event it doesn't know as an
instant, a field it doesn't name as an extra.

**The rule.** Every swarm on Swarm Core writes this contract. A change to it is done only when:

1. this section's table has a row for it, and the current version at the top is bumped;
2. the writer writes it (`cargo xtask event`: swarm-core's `event` module, its `FORMAT`, the
   branch README, and a line using it in `event::examples()`);
3. each reader reads it, and a test reads what the writer writes back;
4. each consumer bumps its pin of Swarm Core.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | minor | A failed `ready` check's `sub.stop` adds `cause=` and `step=` (the failing `ci` step), as a gate's adds its cause; no format change |
| 2026-10-30 | minor | Names no consumer: the reader's own details (its commands, panes and measurements) and its issue numbers dropped, the contract kept; no format change |
| 2026-10-30 | minor | GitLab to GitHub (owner, 2026-10-30): an event's `mr` holds the PR's number; no format change |
| 2026-10-30 | minor | swarm-foundation renamed swarm-core (its crate, command and issues); no format change |
| 2026-10-29 | minor | Gates: a landed `--full` gate that left `main` at its tree is closed with `main=<sha>`, which the periodic full gate counts as a pass of that commit; a convention within v3, no format change (sc#15) |
| 2026-10-27 | minor | The other swarm's issue numbers and name dropped; no format change (sc#6) |
| 2026-10-27 | minor | A gate stopped part way, before its result, has no `rc` in its `sub.stop`: stopped, never passed (sc#3) |
| 2026-10-27 | minor | Moved to swarm-core beside the one writer every swarm runs: the contract (the branch, events, versions). No format change: still version 3, every line written byte for byte as before (keys in sorted order) |
