<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Kept by hand to start with. Once it grows, generate it: a
     `cargo xtask scorecard` of its own writes it from `scorecard/<area>.toml` rows (`id`, `target`, `status`
     done|partial|missing, `note`, optional `issue` and `gap`) and checks it with `--check`; list
     a generated doc in the `Project`'s `generated`, so `stack` regenerates it on a conflict. -->
# Scorecard

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

Per area, what {{product}} does, measured: each row is **done** (a command or code path, tests,
and for UI a visual check), **partial** or **missing**, with its evidence. The PR that changes a
row's status updates it ([progress-docs.md](progress-docs.md) rule 6); gaps live in
[gaps.md](gaps.md).

## Summary

| Area | Done | Partial | Missing |
|---|---|---|---|
| Core | 0 | 0 | 1 |

## Measured

<!-- The numbers the coordinator refreshes once per batch, each with its source and date: test
     counts, commands, open gaps and their estimates. -->

None yet.

## Core

| Id | Target | Status | Evidence |
|---|---|---|---|
| CORE-1 | <!-- what a user can do --> | missing | [G1](gaps.md#g1-the-first-milestones-core) |

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template |
