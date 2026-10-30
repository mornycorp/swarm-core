<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Keep it to docs/progress-docs.md's rules; nothing checks it against Swarm Core. -->
# Roadmap

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

{{product}}'s milestones, the gates between them and the ranked **Current focus**. Detail lives
in [gaps.md](gaps.md); this page keeps one short sentence a milestone and one line a focus item
([progress-docs.md](progress-docs.md) rule 8). The coordinator edits it, in a batch's progress
commit, only when a milestone or the ranking moves.

## Milestones

| Id | Milestone | Done when |
|---|---|---|
| M1 | <!-- name --> | <!-- one short sentence: what a user can do --> |
| M2 | <!-- name --> | <!-- one short sentence --> |

## Gates

A milestone is passed when its gaps are closed, with tests and (for UI) a visual check
([progress-docs.md](progress-docs.md) rule 7), and the owner agrees. An architecture review runs
after the first ~20 merges and at each stage gate ([the swarm protocol](swarm-protocol.md#the-coordinators-loop)
step 10).

## Current focus

The same ranked list as `ROADMAP.md`'s **What's next**: its rank, its bold name, its gap link and
at most a short clause. Workers pick from the top ([the swarm protocol](swarm-protocol.md#picking-work)).

1. **The first milestone's core** ([G1](gaps.md#g1-the-first-milestones-core))

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template |
