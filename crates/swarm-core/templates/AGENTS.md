<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Fill in the parts in comments like this one, cut what doesn't apply, and keep §4 and §5's
     numbering: the shared protocol (docs/swarm-protocol.md) links to them. -->
# AGENTS.md: guide for AI agents and contributors

{{product}} is <!-- one paragraph: what it is, its language and stack, who it is for -->.
It is built by a swarm of agents on [swarm-core](https://github.com/mornycorp/swarm-core)'s
protocol and commands. Read this file first, then [docs/swarm-protocol.md](docs/swarm-protocol.md)
(the protocol every swarm shares) and [docs/swarm.md](docs/swarm.md) (this repo's part of it).

## 1. Orientation

| Read | Why |
|---|---|
| `docs/owner-directions.md` | **The owner's standing directions and decisions:** read it with this file |
| `docs/architecture.md` | **The layers, how a command flows, typed contracts, the UI and automation:** read it before your first change |
| `ROADMAP.md` | One page: the stage, the headline numbers, what's next |
| `docs/gaps.md` | Every known shortfall, ranked: **the work list to pick from** |
| `docs/roadmap.md` | Milestones, gates, **Current focus** |
| `docs/scorecard.md` | Per area, what is done, partial and missing |
| `docs/swarm-protocol.md` | The swarm protocol: roles, claims, tracking, gates, the coordinator's loop (a pinned copy) |
| `docs/swarm.md` | This repo's part: machine setup, capabilities, its gate's kinds, each batch, CI |
| `docs/development.md` | Build, test, run; how many builds a machine takes |
| `docs/progress-docs.md` | How the progress docs are kept honest (a pinned copy) |
| `docs/lessons.md` | The early mistakes swarms made and the guardrails now in place (a pinned copy) |

## 2. Workspace map

The crates and their layers, generated from the layers table in `xtask/` (`Hooks::layers`;
`cargo xtask layers` fails when this is stale, `--write` rewrites it):

<!-- crate-map:start (cargo xtask layers --write) -->
<!-- crate-map:end -->

**Layering is enforced** by `cargo xtask layers`, a step of `ci` ([docs/architecture.md](docs/architecture.md#layers)): a
crate depends only on strictly lower layers, nothing below the UI layer uses a UI crate, and a
new crate is registered in the table at the lowest layer it fits.

## 3. Golden rules

1. **Never crash.** A bad file, input or parameter is an error the user or agent can act on, never
   a panic. Non-test code doesn't `unwrap`, `expect` or `panic!`.
2. **Typed contracts from the first commit.** Every boundary another tool or agent crosses
   (commands' params and results, protocol messages, persisted state, errors) is a type, and
   persisted state carries a version.
3. **Tests are the gate.** Every change comes with tests; prefer ones that cross a real boundary.
4. **Dependencies are deliberate.** Prefer the standard library and what `Cargo.lock` already has.
5. **Everything is a command.** New behaviour is a typed engine command first; the UI, the CLI
   (`{{cli}}`), the control channel and MCP call it by id, and consumers read its typed result,
   never a `Value` by field ([docs/architecture.md](docs/architecture.md#how-a-command-flows)).
6. **The UI is thin and data-driven.** Its state is plain data the control channel reads and sets;
   what a pane draws is what a command returns, and a test checks they are equal.
7. **Verify UI changes visually**: an offscreen snapshot or the app driven over its control
   channel; attach the images to the PR.

## 4. Picking work

The protocol's [Picking work](docs/swarm-protocol.md#picking-work) and
[Tracking](docs/swarm-protocol.md#tracking) hold the whole of it. In short:

0. **Read `ROADMAP.md`, then `docs/gaps.md`.** Workers don't edit the progress summaries
   (`ROADMAP.md`, `docs/roadmap.md`, `docs/gaps.md`): report in your PR's **Progress** section.
1. Pick the highest-ranked open issue in **Current focus** and `docs/gaps.md` that nobody holds and
   that needs nothing your machine lacks ([docs/swarm.md › Capabilities](docs/swarm.md#capabilities)).
2. **Claim it at the moment you start:** `cargo xtask claim <n> --agent <your instance id>`. Always
   pass `--repo {{github.repo}}` to gh.
3. Work in the open; push often. Blocked: `status::blocked` and a comment why.
4. **Finish** with §5's checks, then `cargo xtask ready <m> --agent <id>`. Workers never merge.

### Tracking

Log your session and your work with `cargo xtask event` (the protocol's
[Tracking](docs/swarm-protocol.md#tracking)): `session.start --name <name> --role worker --model
<model> --effort <effort>` prints your instance id; pass `--agent <id>` to every other event, or
set `{{env}}_AGENT`.

## 5. Before you finish a task

**First, one refactoring pass** with the `refactoring` skill (`.claude/skills/refactoring`):
Apply mode, only the functions your branch's diff changed, at most three refactorings, each
verified with the tests before the next. Record them in the PR's Progress section
(**Refactoring:** their names, or "none").

```sh
cargo fmt --all
cargo test -p <the crates you touched>
cargo clippy -p <the crates> --all-targets -- -D warnings
cargo xtask ci --changed    # your gate before ready: what your change affects, and what depends on it
```

Before each run that builds, `cargo xtask capacity`: when it suggests 0 more builds, iterate with
`cargo test -p <crate>` or wait. Then fill in the PR's **Progress** section and submit with
`cargo xtask ready <m> --agent <id>`. Append a terse entry to `log/devlog.md` (local,
gitignored): what landed, the numbers, what's still open.

## 6. Parallel agents

The protocol's [Parallel agents](docs/swarm-protocol.md#parallel-agents) has the rules: your own
target dir per checkout (`CARGO_TARGET_DIR=target/agent-<name>`), your own worktree on your
claimed branch, the owner's checkout left alone. This repo's shared files, which get small,
surgical edits and are re-read before each edit:

- `Cargo.toml` (the workspace manifest)
- <!-- the registries, lists and tables many changes touch -->

The pinned copies (`docs/swarm-core-assets.md` lists them) are Swarm Core's: change them in
swarm-core, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`.

## 7. Where things are tracked

- [GitHub issues and PRs](https://{{github.host}}/{{github.repo}}): what is being worked on now
  (claims), bugs, discussion. Labels: `scripts/github-labels.sh`.
- The `tracking` branch: every agent's sessions and issue events (`cargo xtask event`), which
  a timeline viewer shows.
- `docs/gaps.md`, `docs/roadmap.md`, `ROADMAP.md`: where we stand, kept by the coordinator once
  per batch.
- `log/devlog.md` (local, gitignored): the running dev log.
