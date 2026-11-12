# The gate daemon

> **Last updated:** 2026-11-12 · **Change:** new (the gate daemon, from swarm-foundation's, over GitHub, mornycorp/swarm-core#44)

`cargo xtask gate --daemon` runs the coordinator's merge loop (the swarm protocol's
[coordinator's loop](../crates/swarm-core/assets/docs/swarm-protocol.md#the-coordinators-loop),
steps 2 and 3) as a long-running process on the gating machine. What it does and how a
coordinator runs it are in the protocol's
[The gate daemon](../crates/swarm-core/assets/docs/swarm-protocol.md#the-gate-daemon); this is how
it is built and why.

## Why

The owner, 2026-11-07: "a daemon is appropriate; encoding behavior like that makes it consistent
and fixable". Run by hand, the loop cost hours in one consumer: ready PRs that conflicted with
`main` sat about 8 hours unnoticed (the ready watch reports only PRs newly ready), about 45 minutes
a day went to an idle or waiting gate (restacks under a waiting gate, wake latency), and the
coordinator slipped: it stacked heads a worker was mid-push, ran git in the gate worktree
mid-gate, killed gates mid-compile (stale incremental state broke a later link), and hit stale
rerere replays.

## Shape

`crates/swarm-core/src/gate/daemon.rs` and its modules:

| Module | What |
|---|---|
| `policy` | Pure decisions: `candidate` (ready to gate), `pause`, `dependencies` and `order`, `with_dependents`, `conflict_note`, `merge_request_number`, `next_sub`, `ran` (a gate's records), `machine_failure`, `push` and `land_prefix`, `bisect`, `gate_note`, `failure_note` |
| `blame` | Pure attribution of a failed gate's log to a PR: `failure` (files, packages, tests a log names), `blame` |
| `daemon.rs` | `Daemon::round`, the order of the steps, over the `World` trait; `State`, kept between rounds |
| `real` | The real `World` (GitHub through `github.rs`, git, the gate as a child, `land` in process) and the command line |
| `tests` | The rounds against a fake `World` |

`gate.rs` only hands `--daemon` to it: plain `gate`, `stack` and `land` are unchanged, and need no
`Hooks` switch.

## Decisions

- **The gate is a child process** (`gate --sub gate-<n> --wait --rerun`, this xtask's own exe), not
  a call: the daemon reads only its records (`<sub>.rc`, `<sub>-rerun.rc`, the logs) after it
  ends, and the gate's own refusals, locks, reruns and events stay its own.
- **Never kill a gate.** Folding newly ready PRs in "while the gate waits for room" is done before
  the gate starts: the daemon asks for room itself (the machine gate lock free for it, and
  `capacity`), and while there is none it keeps the stack unstarted, restacking when the ready set
  changes, for up to `--fold` seconds. Then it starts the gate, which waits itself if it must
  (and, where the project gives its gates priority, goes ahead after its own wait). Killing a gate that only waits
  races its start of `ci`, which is what left stale incremental state before; this way there is
  nothing to kill.
- **Rerere off on the daemon's path only:** the daemon re-runs itself with
  `GIT_CONFIG_PARAMETERS` adding `'rerere.enabled=false'` (edition 2024 makes `set_var` unsafe,
  which the crate forbids), so its stacks, its gates' git and its lands never replay a recorded
  resolution; a coordinator's own `stack` is untouched.
- **Pushes during a gate are compared by patch-id**, not SHA: each head's own change against
  `main` (from where it forks), old and new. The same patch is a rebase, and lands as gated (`land`
  checks the gated tree again before it pushes); another patch, or a branch gone, waits with every
  PR above it.
- **Attribution,** strongest evidence first, each step deciding only when it matches exactly one
  PR (several: bisect; none: the next step): the files an `error`'s `-->` or rustfmt's `Diff in`
  names; the file a failed test lives in, from its package (the workspace's `Cargo.toml`s), binary
  and module path, the longest match winning (a `mod` line in `tests/it/main.rs` loses to the
  test's own file); the packages that failed (`could not compile`, a failed test's). Warnings'
  locations never count. A stack of one that fails is that PR's.
- **The machine's failures drop nothing.** A transient cause the gate's own rerun didn't clear,
  or the gate machine's setup, gates again next round; three in a row set the off switch.
- **Conflicts are told once per head**, and only conflicts with `main` itself (`git merge-tree`
  against `main`, the generated docs left out since `stack` regenerates them). A PR that
  conflicts only with the PRs below it waits for them to land, with no note; it is told if it
  still conflicts once they have.
- **A bisection's last suspect is gated alone** before it is sent back (#45). The others passing
  proves nothing of it when the failure took two of them together, or was a flake the rerun didn't
  clear: when it passes alone too, nobody is sent back.
- **The off switch is read** when a round starts, before each gate, and before a pass lands. A
  gate is never interrupted.
- **GitHub's conflict flag is only where to look.** The daemon asks git which files conflict with
  `main`, and leaves out the generated docs, which `stack` regenerates: a PR that conflicts only
  there is gated.
- **A PR it can't put back to draft is held**, not dropped: it is still ready, and would be gated
  again each round. One that moved since is a new head, gated like any other. What the daemon
  keeps of a PR goes when the PR merges or closes.
- **One daemon a worktree** (`daemon.pid`, checked against the running processes): two would share
  the gate lock's name and overwrite each other's state. A dry run may run beside one.
- **State** (`<target>/gate-daemon/state.json`): the heads noted, dropped and held, when each PR was
  first seen ready, its pause and its latest decisions; the off switch is the file `off` beside it.
  A head dropped or noted is not gated again; a push is a new head.

## On GitHub, with no server CI

- **No pipeline to pause on.** GitHub runs no workflows here, so the daemon pauses on its off
  switch and on an open `main-red` issue, and on nothing else. A red `main` that nobody has filed
  is seen only by its own check of `main` (`Hooks::verify_main`): before it blames a stack's first
  failure at its tests, it runs those tests alone on `main` (`gate --test-alone`), and when they
  fail there too it sets the switch and sends nothing back. A project that runs the daemon should
  turn it on.
- **A PR is sent back by converting it to a draft** (`github::convert_to_draft`), read back, with
  a comment that holds the log's excerpt.
- **A conflict is GitHub's `dirty`.** `unknown` (not worked out yet) is no conflict: `stack` finds
  a real one.
- **A landed PR's number** is read from `Merge pull request #<n> from …`, the merge commit's first
  line.

## Timeline

The daemon logs as the coordinator's instance (`{prefix}_AGENT`): the gates' own events, `land`'s
merges, and a `sendback` for each PR it drops (a conflict's note `cause=conflict: <files>`, a
failure's `--sub gate-<n> --note "cause=<cause>: <evidence>"`). Pauses and waits are printed only.

Each gate's start note names its step and the PRs it gates, no other (`policy::gate_note`):
`daemon gate: #a #b #c`, `regate of gate-<n>: #a #c` (without the PR it dropped, which a reader
would otherwise count as gated), `bisect of gate-<n>: #a #b`. A PR a failure only sets aside (one
stacked on the dropped PR, or one a bisection left out) gets no `sendback`. A reader that took
every PR of a failed gate as sent back would count a failed stack's innocent PRs, so the tracking
format reads a daemon gate's failure as sending back only what its `sendback` events name
([tracking-format.md](tracking-format.md) › Send-backs), and `prwatch` logs a worker's rework only
for those (`prwatch/sendback.rs`).

## Tests

`policy` and `blame` have unit tests; `tests` runs rounds against a fake `World`: a pass lands, a
pause is said once per change, a conflict GitHub reports is noted once per head, a stack conflict
drops the PR and those on it, a conflict with the PRs below waits, the fold restacks only when the
ready set changes, a failure drops the PR it points at and regates the rest, an unattributed one
bisects, the notes and events of a regate, of a bisection that clears its PRs and of one that
blames a culprit (only the culprit's `sendback`), machine failures set the switch, a push during the gate waits with what is above it, a
dry run changes nothing, a land that stops twice holds the PR, and a gate never lands once `main`
went red.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-12 | minor | The last suspect gated alone; the switch read before each gate; `--retry <pr>`; one daemon a worktree; the conflict flag only where to look; a `main-red` issue when `main` fails here; a failed re-draft held; counters kept across a restart; gates that don't run set the switch (mornycorp/swarm-core#45) |
| 2026-11-12 | new | The gate daemon, from swarm-foundation's, over GitHub: no pipeline pause, a send-back by converting to a draft, `prwatch` reading its gates' failures as sending back only what a `sendback` names (mornycorp/swarm-core#44) |
