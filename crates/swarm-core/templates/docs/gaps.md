<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Keep it to docs/progress-docs.md's rules; nothing checks it against Swarm Core. -->
# Gaps

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

Every known shortfall of {{product}}, ranked: **the work list, and the only home for progress
detail** ([progress-docs.md](progress-docs.md) rule 8). The coordinator folds each batch's merged
PRs' Progress sections into these entries, once per batch, in one commit; workers report in their
PR instead of editing this file. `ROADMAP.md` and [roadmap.md](roadmap.md) link here, one line an
item.

Each entry: its id (`G1`, never reused), what is missing and why it matters, what's done and
what's left, the evidence (code paths, tests, commands, issues), measured numbers with their
source and date, and an estimate in agent-hours (a range). Work an agent can't finish alone is
marked **Human:** with what is needed.

A gap that covers spec sections lists **every requirement line** of them (each table row and
bullet, not only the headline features) under **Requirements**, each citing the issue or the code
that meets it, or the owner's decision to drop it. It is marked done only when none is left bare
([progress-docs.md](progress-docs.md) rule 7).

## Ranked

| Rank | Gap | Status | Issues | Estimate |
|---|---|---|---|---|
| 1 | [G1 The first milestone's core](#g1-the-first-milestones-core) | open | | |

## G1 The first milestone's core

- **Missing:** <!-- what the product can't do yet, and who needs it -->
- **Done:** nothing yet.
- **Left:** <!-- the tasks, each small enough for one worker's session, each an issue -->
- **Requirements:** <!-- each requirement line of the spec sections this gap covers: "§3.2 row 4: … — #12" or "— `crates/x/src/y.rs`" or "— dropped (owner, date)" -->
- **Evidence:** <!-- code paths, tests, commands, issues -->
- **Measured:** <!-- the number, its source (a test, a command) and its date; or "none yet" -->
- **Estimate:** <!-- agent-hours, a range -->

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template |
