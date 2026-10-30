<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Nothing checks it against Swarm Core. -->
# Owner directions

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

The owner's standing directions and decisions for {{product}}, dated, newest first in each
section. Every agent reads this with `AGENTS.md`. The owner writes it, or an agent records a
direction the owner just gave, quoting it and linking where it was given (an issue, a PR note).
A direction stands until the owner replaces it; a replaced one moves to **Superseded** with the
date it ended.

Write each as: **the direction**, in a sentence or two; *why*, when the owner gave a reason; the
date and where (`(owner, YYYY-MM-DD, #n)`). The protocol and AGENTS.md cite them as
`(owner, YYYY-MM-DD, #n)`.

## The product

- <!-- What we are building, for whom, and what it must never do. -->

## How we work

- **Swarm size:** at most three workers per swarm on the owner's desktop, and only while
  `cargo xtask capacity` shows room (the swarm protocol's default; change it here).
- **Merging:** only the coordinator merges, on the local gate
  ([the swarm protocol](swarm-protocol.md#the-coordinators-loop)).
- <!-- Anything else the owner decides about process: what needs their approval, when to pause. -->

## Decisions

| Date | Decision | Why | Where |
|---|---|---|---|
| {{today}} | The swarm runs on swarm-core's protocol and commands | one process shared with the other swarms | <!-- issue --> |

## Superseded

None yet.

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template |
