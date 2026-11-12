<!-- A pinned copy from swarm-core (crates/swarm-core/assets/docs/lessons.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`: the gate's
     `sync-assets --check` fails on a copy that differs. -->
# Lessons for a new swarm

> **Last reviewed:** 2026-10-31 · **Last updated:** 2026-11-12 · **Change:** minor (lessons 5–15: the robustness mistakes, chained PRs, a timeout rerun with no limit, a squash that reverted `main`; the two after them are now 16 and 17) · **Target:** M1

The early design mistakes the first two swarms made: how each was found, what it cost, and the
guardrail now in place (a rule, a check, or an item on the architecture review's checklist). It
lives in swarm-core, beside the tracking format and the
[swarm protocol](swarm-protocol.md), as the shared home of the swarm process; each swarm keeps a
pinned copy and its AGENTS.md §1 links it. **Required reading before starting a new swarm**, and
the source of the [architecture review](swarm-protocol.md#the-coordinators-loop)'s checklist of
known traps: a trap a review finds that isn't listed here is added, with the issue that found it.

Lessons 5–12 are the robustness mistakes apps have already paid for: a hang, a freeze, lost
settings, a leaked secret. Each is a rule for every app, whatever it does.

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

## 5. A child process outlived its caller and held the shell's pipe

- **The mistake:** a command started a program and waited for it, and the program started its own
  child. When the command timed out or was cancelled, only the program was killed; the grandchild
  lived on, still holding the pipe the caller's shell was reading, so the shell never saw the end
  of the output and hung with it. Another start left the child's stdin open, and a program that
  read it waited for ever.
- **How it was found:** by a person: a command that had finished (or been killed) still did not
  return, and the only thing left running was a process nobody had started on purpose.
- **What it cost:** a hung session each time, killed by hand, and no log of what the program had
  said; every place that started a program was suspect, because each had its own way of doing it.
- **The guardrail:**
  - **Rule:** one process module starts every child: stdin closed, a wall-clock and an idle
    deadline, the output capped and drained on its own threads, its own process group killed on a
    timeout, a cancel and a drop, and every failure a typed error.
  - **Check:** the architecture's scan (swarm-core#46) refuses `std::process::Command` outside
    that module; the module's tests run a child that ignores its stdin, one that floods its
    output, one that spawns a grandchild and one that never exits.

## 6. A second lock taken in one statement froze every run

- **The mistake:** a statement took one lock and, while it held it, a second, as in
  `f(a.lock(), b.lock())`. Another thread took the two in the other order, and the app was stuck
  with every run waiting on a lock nobody would release. The standard mutex waits for ever and
  says nothing.
- **How it was found:** by a hang with no error and no log line; only a thread dump showed two
  threads each holding one lock and wanting the other.
- **What it cost:** a frozen app for every run that touched either lock, and a long read of the
  code to find the one statement; nothing in the code said which locks were involved.
- **The guardrail:**
  - **Rule:** one lock read per statement: take one, copy out what is needed, drop it, take the
    next; the app's own mutex errors out on a deadlock instead of waiting, naming the lock.
  - **Check:** clippy's `disallowed-types` refuses `std::sync::Mutex` and `MutexGuard`; the lock's
    own tests; the architecture review reads for a second lock in a statement.

## 7. Files read while drawing froze the window

- **The mistake:** the UI read a file (a preview, a size, a listing) in the code that draws a
  frame. On a local disk it went unnoticed; on a slow or a network disk, or a large folder, every
  frame waited for it, and the window stopped answering.
- **How it was found:** by a person on a slower disk: the window froze while a list was on screen
  and thawed when it was closed. Nothing in a test saw it, because a test's disk is fast.
- **What it cost:** a UI that couldn't be trusted to stay responsive, and a hunt through every
  draw function for a read hidden behind a helper.
- **The guardrail:**
  - **Rule:** the UI draws memory and calls commands on workers; it never writes a file, starts a
    program, blocks its own thread or reads the disk while drawing, and a result arrives as a
    message the next frame draws.
  - **Check:** the architecture's scan (swarm-core#46) refuses file access and process starts in
    the UI crate, and `cargo xtask layers` keeps the toolkit in the UI layer.

## 8. A non-blocking read's defaults were saved over the person's settings

- **The mistake:** the app loaded its settings without blocking, and until the read finished (or
  when it failed, because the file was locked or briefly unreadable) it ran on defaults. Something
  then changed a setting and saved, and the save wrote the whole file: the defaults, and the one
  change, over everything the person had chosen.
- **How it was found:** by a person whose settings were reset after an unrelated change; the file
  on disk held defaults and the one new value.
- **What it cost:** the person's settings, with no copy to restore them from, and a loss of trust
  in every save.
- **The guardrail:**
  - **Rule:** state is loaded without ever writing defaults over what it couldn't read: a read that
    fails or blocks leaves the person's file alone, and nothing saves until the load has
    succeeded.
  - **Check:** the state module owns the read and the write, and the one atomic writer is the only
    writer (the scan, swarm-core#46); a test saves after a failed read and finds the file as it was.

## 9. Memory changed before a failed save

- **The mistake:** a command updated the app's state in memory and then saved it. When the save
  failed (a full disk, a locked file), the screen and every later command went on showing and using
  the new state, while the file held the old one; the next start undid it without a word.
- **How it was found:** by a person: a setting that read as changed until the app restarted,
  when it was back.
- **What it cost:** changes that were reported done and weren't, and a state in memory no one
  could trust to match the disk.
- **The guardrail:**
  - **Rule:** save before memory changes: write the new state to disk first and change memory only
    when that succeeded, so a failed save returns its typed error and leaves both as they were.
  - **Check:** the writer's tests fail a write and find the memory unchanged; the architecture
    review reads each new command that changes state for the order.

## 10. A settings view printed secrets

- **The mistake:** the command that returned the app's settings returned every one, a token
  included, so the CLI, the control channel, MCP and the UI each showed it, and a journal that
  logged results kept it.
- **How it was found:** by reading a result that was pasted into a report: the token was in it.
- **What it cost:** a credential to revoke and replace, and a search of the logs and the journal
  for every place it had been written.
- **The guardrail:**
  - **Rule:** a secret setting is stored and shown redacted, a command that needs one reads it from
    the store, and no result, log line, journal entry or error message carries its value.
  - **Check:** the tests of every result that holds settings must show the redaction, with obvious
    fakes for secrets; the architecture review asks of every new setting, "is this one?".

## 11. A best-effort step hid its error

- **The mistake:** a step that was meant not to matter (a cleanup, a cache write, a notification)
  discarded its error with `let _ =` or `.ok()`. It did matter: when it failed, the work that
  depended on it failed later, somewhere else, with a message about the wrong thing.
- **How it was found:** by tracing a failure back from the symptom, which took longer than the
  fix: the first failing step had left no trace.
- **What it cost:** hours reading code that was behaving as written, and a failure no one could
  reproduce on demand.
- **The guardrail:**
  - **Rule:** every failure is a typed error with a message and a remediation, and a caller that
    chooses to continue past one logs it and says why.
    A step is best-effort only when its failure is reported and the work stays correct without it.
  - **Check:** the architecture review and every PR's reviewer look for a discarded `Result`;
    clippy's `must_use` and unused-result lints hold the rest.

## 12. A test wrote to the real config dir

- **The mistake:** a test saved settings through the app's real config path. It ran on a person's
  machine, as every test does, and reset their settings to the test's values.
- **How it was found:** by a person: their settings were not what they had set, and the values
  were the ones in a test.
- **What it cost:** the person's settings, and the trust of everyone who ran the tests; every test
  was suspect until the config path was audited.
- **The guardrail:**
  - **Rule:** a test uses temporary directories for config and state, makes no network call and
    runs no installer or GUI program; the config dir comes from one function a test points at a
    temp dir.
  - **Check:** the architecture's scan (swarm-core#46) refuses the real config dir named in a test.

## 13. PRs chained because they touched the same files

- **The mistake:** told to stack its PRs that touched the same files, a worker branched a small
  banners change from its own earlier PR (row heights) rather than from `main`, and submitted it
  `--onto` that PR. The banners change needed none of the other's code.
- **How it was found:** the earlier PR failed a gate three times; the banners PR, which never
  failed a gate of its own, could land only after it, and the coordinator saw it still waiting.
- **What it cost:** about four hours for the banners change, and three gates of the shared machine
  spent on a stack whose top was fine.
- **The guardrail:**
  - **Rule:** a worker branches every PR from `main`, and stacks on another open PR only when its
    change needs that PR's code to build or work; PRs that only touch the same files stay on
    `main` ([swarm-protocol.md › Picking work](swarm-protocol.md#picking-work), step 4).
  - **Where ordering lives:** with the coordinator. `stack` composes each gate's batch from the
    ready PRs, in the order it chooses; when it stops on a textual conflict, the second PR's
    author rebases once after the first lands, a few minutes' work, against hours of waiting.
  - **Review:** an PR submitted `--onto` another says in its description what code of the other
    it needs.

## 14. A timeout repeated alone, rerun with no limit

- **The mistake:** the periodic full gate went red on a timeout: nextest's `terminate-after`
  ended one long test at 600 s in the run, and again in its rerun alone. The worker's first step
  was to run it alone with no time limit, "to rule out load".
- **How it was found:** the uncapped run took 75 minutes and showed a real slowdown, which the two
  timeouts had already shown: the rerun alone had ruled load out.
- **What it cost:** `main` red, with every merge held, for about two hours, most of it the uncapped
  run.
- **The guardrail:**
  - **Rule:** a timeout repeated in the rerun alone is proof of a slowdown. Time one unit of the
    test (one event, one capture, one input) on the failing commit and the last green one, bisect
    the merges between them with that unit, fix, and measure the whole test once, on the fix
    ([swarm-protocol.md › The periodic full gate](swarm-protocol.md#the-periodic-full-gate)).
  - **Where the uncapped run belongs:** only after a rerun alone that **passed**, which points at
    load.

## 15. A squash that reverted what `main` gained

- **The mistake:** a worker squashed its branch with `git reset --soft origin/main` and one
  commit, on a branch it hadn't just rebased onto that `origin/main`. The index still held the
  branch's old base, so the commit undid everything `main` had gained since.
- **How it was found:** in one swarm, 2026-10-19: the PR merged, and the next PR had to
  revert it.
- **What it cost:** two PRs and their gates, and `main` without the reverted work until the
  revert landed.
- **The guardrail:**
  - **Rule:** squash onto the merge base, `git reset --soft $(git merge-base HEAD origin/main)`,
    or reset to `origin/main` only right after rebasing onto that same `origin/main`.
  - **Review:** an PR whose diff deletes work it doesn't mention is a squash gone wrong.

## 16. A front end building the core's logic

- **The mistake:** a swarm's UI front end needed rows, row heights, hit-testing and drag state
  before the core had a layout or an interaction module, so it built them itself: which rows are
  on screen, what lies under the pointer, a track's height (taken from the bars the last frame
  happened to draw), the scroller's pixel-to-axis mapping (a second, inexact copy of the core's),
  and the drag in progress. It didn't reach into the engine or mirror its types, so none of the
  review's traps named it, and every UI task after it (pinning, selection, the minimap, the
  keyboard) would have extended it there, out of reach of the CLI, the control channel and MCP.
- **How it was found:** the architecture review at the first ~20 merges (2026-10-30), reading
  the UI crate against the spec's "the widget owns the layout and all interaction" and the
  architecture's layer table, which gave both to the core.
- **What it cost:** caught early: about 5–8 agent-hours to move into the core, against roughly
  40 hours of planned UI work that would have built on it.
- **The guardrail:**
  - **Review:** a new item on the architecture review's checklist: logic the spec or the
    architecture gives the core (layout, hit-testing, interaction state, any mapping between
    pixels and the model), built in a front end.
  - **Rule:** a front end that needs an answer the engine doesn't give asks for the command
    (an issue, or the algorithm in the core first), rather than computing it.

## 17. Requirements in a spec's tables that never became work

- **The mistake:** a swarm built its gap list from the spec's sections, one gap a section, and
  marked each done when the issues filed under it merged. The issues were written from the
  section's headline features, so requirements stated only as a row of a table or a bullet in
  another section (what the host supplies, an optional rendering rule, a flag on a data type, a
  side effect of a mouse action) never got an issue. Their gaps read "done" while about ten such
  requirements had no code: a host hook with its trait already in the core but never wired, a
  track kind that drew nothing, a scale's flag computed but never drawn, a click that didn't
  select first.
- **How it was found:** the architecture review at a stage gate (2026-10-31), reading the spec
  section by section, line by line, against the code, rather than against the gap list.
- **What it cost:** found before the release candidate: about 20 agent-hours of small features,
  filed as one issue each; later, each would have surfaced as a host's bug report against a
  feature the progress docs called done.
- **The guardrail:**
  - **Review:** a new item on the architecture review's checklist: a requirement in the spec
    (any table row or bullet) with no code and no open issue, under a gap marked done.
  - **Rule:** a gap is marked done only when each requirement line of its spec sections cites
    the issue or the code that meets it, or an owner's decision to drop it.

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
| 2026-11-12 | minor | Lessons 5–15, from swarm-foundation: the robustness lessons (5–12: a child that outlived its caller, a second lock in one statement, files read while drawing, defaults saved over settings, memory changed before a failed save, secrets in a settings view, a best-effort step's hidden error, a test in the real config dir), PRs chained for touching the same files (13), a timeout repeated alone and rerun with no limit (14), a squash that reverted `main` (15). The lessons that were 5 and 6 are 16 and 17 (swarm-core#28) |
| 2026-10-31 | minor | Lesson 6: requirements in a spec's tables that never became work, and its checklist item |
| 2026-10-30 | minor | Lesson 5: a front end building the core's logic (layout, hit-testing, interaction state), and its checklist item |
| 2026-10-30 | minor | Names no consumer: the consumers' names and their issue citations dropped, each rule kept |
| 2026-10-30 | minor | GitLab to GitHub (owner, 2026-10-30): pull requests, `gh`, PRs as `#n` |
| 2026-10-30 | minor | swarm-foundation renamed swarm-core: the crate, `cargo xtask swarm-core` (was `foundation`), `docs/swarm-core-assets.md`, and `sc#n` for its issues (was `f#n`) |
| 2026-10-27 | minor | Names one consumer only: the other swarm's issue numbers and name dropped, each lesson and guardrail kept (swarm-core#6) |
| 2026-10-27 | minor | Moved into swarm-core as a pinned asset, issue numbers qualified, the review's checklist now in the swarm protocol |
| 2026-10-24 | minor | The file, with its first four lessons: untyped contracts, a shared target dir testing the wrong tree, the full gate on every merge, flakes from shared machine load; linked from both swarms' AGENTS.md §1 |
