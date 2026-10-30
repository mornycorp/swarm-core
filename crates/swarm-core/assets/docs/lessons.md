<!-- A pinned copy from swarm-core (crates/swarm-core/assets/docs/lessons.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`: the gate's
     `sync-assets --check` fails on a copy that differs. -->
# Lessons for a new swarm

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-30 · **Change:** minor (names no consumer) · **Target:** M1

The early design mistakes the first two swarms made: how each was found, what it cost, and the
guardrail now in place (a rule, a check, or an item on the architecture review's checklist). It
lives in swarm-core, beside the tracking format and the
[swarm protocol](swarm-protocol.md), as the shared home of the swarm process; each swarm keeps a
pinned copy and its AGENTS.md §1 links it. **Required reading before starting a new swarm**, and
the source of the [architecture review](swarm-protocol.md#the-coordinators-loop)'s checklist of
known traps: a trap a review finds that isn't listed here is added, with the issue that found it.

## 1. Untyped contracts at the engine's boundary

- **The mistake:** one swarm's first architecture made every command's params and results a
  `serde_json::Value`, documented params with a free-text string, returned errors mostly as
  `Other(String)`, and persisted UI state with no version. One dispatch path served the UI, the
  CLI, the control channel and MCP, and the swarm built 100+ commands on it in a week.
- **How it was found:** late, by an architecture review the owner asked for (2026-10-19), after
  about a week and about 103 commands. Nothing in the process had scheduled one. It found about
  240 hand-written param checks, UI code reading replies as `v["field"]` with `unwrap_or_default`
  (a renamed field failed silently), panes keeping their own serde mirrors of engine results, and
  panes calling engine functions directly.
- **What it cost:** seven slices (all 115 commands typed), then five follow-ups and the UI's typed
  client. Found at the 20th merge, it would have been a few commands' work.
- **The guardrail:**
  - **Rule:** typed contracts from the first commit: params, results, protocol messages,
    persisted state (with a version) and error codes; consumers never index a `Value`.
  - **Check:** `cargo xtask layers` refuses untyped commands and `Value`-indexed replies outside
    the typed adapter.
  - **Review:** the standing architecture review, after the first ~20 merges of a new product or
    area and at each stage gate, from this file's traps.

## 2. A shared target dir tested the wrong tree

- **The mistake:** xtask found the repository from the directory it was *compiled* in
  (`env!("CARGO_MANIFEST_DIR")`), and agents shared one `CARGO_TARGET_DIR` across worktrees. With
  identical xtask sources cargo reused the other worktree's xtask, so `cargo xtask ci` linted,
  built and tested the other worktree, and reported green for a tree it never built.
- **How it was found:** by a test count, in a coordinator's gate of one PR (2026-10-16): its log
  compiled every crate from the other worktree, whose fonts test binary had 4 tests where the PR
  had 5.
- **What it cost:** a green gate for a tree nobody had built, so every gate run with a shared
  target dir before it was suspect; a rerun after touching xtask's source.
- **The guardrail:**
  - **Check:** xtask resolves its root at run time (the `CARGO_MANIFEST_DIR` `cargo run` sets,
    else a walk up to the workspace manifest), with a test that runs it from another directory;
    `cargo xtask ci` refuses a target dir another checkout used last.
  - **Rule:** one target dir per checkout or worktree, never shared (both swarms' AGENTS.md §6).

## 3. The whole gate, GPU and all, on every merge

- **The mistake:** the coordinator merged each batch only after the full `cargo xtask ci` (with
  the GPU tests, where the project has them) on the stacked tree. Every merge, a docs fix or a
  tooling change included, paid for the slowest gate, and the coordinator was the swarm's
  bottleneck.
- **How it was found:** merge throughput: one swarm's gates took 10–26 minutes each, and on
  2026-10-21 eight tooling PRs took the full gate, about an hour of it.
- **What it cost:** hours of queued PRs a day, and a chain of six issues to move the full gate off
  the merge path.
- **The guardrail:**
  - **Rule:** the gate's kind follows the change (docs, tooling, smoke, full); the
    coordinator merges after a smoke, and the full gate runs on `main` periodically, by the next
    capable agent to claim a task; a worker's own `cargo xtask ci --changed` before ready
    proves its change; `--full` only when the author's GPU run can't speak for the gating
    machine.
  - **Measured** (2026-10-23): the merge gate went from 10–26 minutes to 0.2–5 minutes, and about
    60 PRs merged in about 36 hours.

## 4. Flakes from shared machine load

- **The mistake:** both swarms' coordinators, workers and gates ran on one 32 GB desktop with one
  GPU, and nothing coordinated them across processes: nextest's test groups cap a single run, and
  `cargo xtask capacity` looked only when a gate started.
- **How it was found:** red gates that passed on a rerun. In one swarm's gate (2026-10-21) the
  test step took 989 s against 288 s two gates before, beside two workers' `ci --changed`, four
  other nextest runs, a Linux clippy and the other swarm's tests, and its load flakes showed up
  then. The same day one swarm's gate linked with 22 `link.exe` processes holding about
  30 GB of the 32 GB.
- **What it cost:** gates rerun or gated again in full (one PR was gated twice in a row for one
  test that passed alone in 12 s), and time spent chasing failures that weren't the change's.
- **The guardrail:**
  - **Check:** a machine-wide GPU lease the gate's tests take first; build capacity
    before a gate or a worker's build; one gate on the machine, either swarm's;
    a gate's build jobs follow free memory.
  - **Rule:** a red gate is read before it's believed: its cause is named, and tests that failed
    under load rerun alone on the same HEAD; to fix a flake, make the failing condition
    deterministic in a test, never loop it (a swarm's AGENTS.md test rules).

## Adding a lesson

A lesson is an early design or process mistake that cost a swarm real time, with its guardrail in
place. Add it when its guardrail lands, in the same PR or the twin's: the mistake, how it was found
(the issue, the date, the evidence), what it cost (measured: slices, hours, PRs), and the
guardrail (a rule, a check, a review item, each with its issue). One section a lesson; keep each
short and link the issues for the detail. A lesson about design (a boundary, a format, ownership)
also adds its trap to the architecture review's checklist ([the swarm
protocol](swarm-protocol.md#the-coordinators-loop), step 10). Both are Swarm Core's: add it
there, then bump.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-30 | minor | Names no consumer: the consumers' names and their issue citations dropped, each rule kept |
| 2026-10-30 | minor | GitLab to GitHub (owner, 2026-10-30): pull requests, `gh`, PRs as `#n` |
| 2026-10-30 | minor | swarm-foundation renamed swarm-core: the crate, `cargo xtask swarm-core` (was `foundation`), `docs/swarm-core-assets.md`, and `sc#n` for its issues (was `f#n`) |
| 2026-10-27 | minor | Names one consumer only: the other swarm's issue numbers and name dropped, each lesson and guardrail kept (swarm-core#6) |
| 2026-10-27 | minor | Moved into swarm-core as a pinned asset, issue numbers qualified, the review's checklist now in the swarm protocol |
| 2026-10-24 | minor | The file, with its first four lessons: untyped contracts, a shared target dir testing the wrong tree, the full gate on every merge, flakes from shared machine load; linked from both swarms' AGENTS.md §1 |
