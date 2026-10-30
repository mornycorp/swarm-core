<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Keep the heading the shared protocol links to: How many builds at once. -->
# Development

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

How to build, test and run {{product}}, and how many builds a machine takes.

## Build, test, run

```sh
cargo build
cargo nextest run --workspace     # or cargo test
cargo xtask ci                    # the whole gate: what the periodic full gate runs
cargo xtask ci --changed          # a worker's gate before ready: what the change affects
```

<!-- How to run the product, its CLI, and anything that drives it programmatically. -->

## `cargo xtask ci`

The gate's steps, as `CI summary` names them (the `ci` contract, swarm-core's
docs/ci-contract.md, says which forms Swarm Core's gate calls and what it reads back):

| Step | Runs | In |
|---|---|---|
| `fmt` | `cargo fmt --all -- --check` | full, `--changed`, `--smoke` |
| `clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` | full, `--changed` (its packages), `--smoke` |
| `test` | `cargo nextest run --profile ci --workspace`, then `cargo test --doc` | full, `--changed` (its packages) |
| `build` | `cargo build --workspace --exclude xtask` | `--smoke` |
| `assets` | `cargo xtask swarm-core sync-assets --check` | every form |
| <!-- golden, cli, … --> | | |

`ci` records its checkout in the target dir (`swarm_core::checkout::claim`) and refuses one
another checkout used last: one target dir per checkout, never shared.

## How many builds at once

Measure it on each machine the swarm builds on, and record it here: the protocol's worker limit
and `cargo xtask capacity`'s suggestion rest on it
([the swarm protocol](swarm-protocol.md#helpers-in-worktrees)).

How: run the full `cargo xtask ci` with dependencies warm and the workspace's crates rebuilt (as a
gate after a change), 1, 2 and 4 at once, each in its own worktree and target dir at one commit,
and note each gate's time, the memory above the desktop's own at the peak, and whether it passed.

| Builds at once | Each gate | Memory above the desktop's own | Result |
|---|---|---|---|
| 1 | <!-- min --> | <!-- GB at its peak --> | |
| 2 | | | |
| 4 | | | |

`cargo xtask capacity` counts about one build per 8 free cores and 5 GB of free memory, at most 3
more, and none under 20 GB of free disk (swarm-core's `capacity`). Memory usually decides,
not cores. If this machine's numbers differ, record them here and tell the coordinator.

## Swarm Core

`xtask` dispatches the swarm's commands (`claim`, `event`, `gate`, `stack`, `land`, …) to
swarm-core, pinned by `rev` in the workspace manifest. `cargo xtask swarm-core --version`
says which; `cargo xtask swarm-core bump` moves it to Swarm Core's `main`, then
`cargo xtask swarm-core sync-assets` refreshes the pinned copies (`docs/swarm-core-assets.md`).
To try a Swarm Core change before it merges, patch it in your **user-level**
`~/.cargo/config.toml`, never in the repo:

```toml
[patch."https://github.com/mornycorp/swarm-core.git"]
swarm-core = { path = "<your swarm-core checkout>/crates/swarm-core" }
```

## Scorecard

`docs/scorecard.md` says, per area, what is done, partial and missing, with evidence. The PR that
changes a row's status updates it; the coordinator refreshes its measured numbers once per batch
([docs/swarm.md › Each batch](swarm.md#each-batch)).

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template |
