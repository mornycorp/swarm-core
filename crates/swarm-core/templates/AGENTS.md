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
| `ROADMAP.md` | One page: the stage, the headline numbers, what's next |
| `docs/gaps.md` | Every known shortfall, ranked: **the work list to pick from** |
| `docs/roadmap.md` | Milestones, gates, **Current focus** |
| `docs/scorecard.md` | Per area, what is done, partial and missing |
| `docs/swarm-protocol.md` | The swarm protocol: roles, claims, tracking, gates, the coordinator's loop (a pinned copy) |
| `docs/swarm.md` | This repo's part: machine setup, capabilities, its gate's kinds, each batch, CI |
| `docs/app-architecture.md` | The contract every app keeps: layers, commands, one dispatch, the robustness rules (a pinned copy) |
| `docs/architecture.md` | This repo's long form: its crates, how a command flows, where state lives, **Adding a command** |
| `docs/development.md` | Build, test, run; how many builds a machine takes |
| `docs/progress-docs.md` | How the progress docs are kept honest (a pinned copy) |
| `docs/lessons.md` | The early mistakes swarms made and the guardrails now in place (a pinned copy) |

## 2. Workspace map

<!-- The layer map below is the default for a headless-first app, from the contract
     (docs/app-architecture.md › Layers). It is this repo's to change: name the real crates, cut a
     layer it has no use for, add the binaries. Keep it in step with each crate's
     `[package.metadata.swarm] layer`, which `cargo xtask ci`'s `layers` check reads. -->

Dependencies point down: a crate uses only strictly lower layers
([docs/app-architecture.md](docs/app-architecture.md#layers)). The long form, with each crate and
command, is [docs/architecture.md](docs/architecture.md). **Layering is enforced** by `cargo xtask
layers`, a step of `ci`, from each manifest's layer (`Hooks::architecture`) or from the table in
`xtask/` (`Hooks::layers`).

| Layer | Path | What |
|---|---|---|
| 0 `model` | `crates/model/` | The types and parsers of everything read from outside (files, wire frames, user text). Hostile input, no I/O |
| 1 `domain` | `crates/<area>/` | The libraries that do {{product}}'s work, one crate per area. No I/O |
| 2 `backends` | `crates/backends/`, and `crates/<backend>/` for each further one | The one layer that touches the outside. `crates/backends` holds the process module, the atomic file writer and its capped read; a further backend (git, the network, a device) is a module there or a crate of its own. Each call with a deadline and a capped result |
| 3 `engine` | `crates/engine/` | The session, the command registry and the one dispatch |
| 4 `automation` | `crates/automation/` | The transports: `serve`, the loopback socket, MCP. Thin adapters over the dispatch |
| 5 `ui` | `crates/ui/` | The UI shell: toolkit code, drawing, input. A client of the engine, never a writer |
| 6 `apps` | `apps/{{cli}}/`, `apps/<app>/` | The binaries: the CLI (`{{cli}}`) and the windowed app (`<app>`, with `init --app --ui`) |
| outside | `crates/testkit/` | Temp dirs, fakes and hostile inputs (the package `<prefix>testkit`): a `[dev-dependencies]` entry only. The conformance run is swarm-core's (`swarm_core_testkit`, pinned beside it) |
| outside | `xtask/` | `cargo xtask`: this repo's own commands (`ci`, …) and the swarm's, from swarm-core |

The crates as they are, generated (`cargo xtask layers` fails when this is stale, `--write`
rewrites it):

<!-- crate-map:start (cargo xtask layers --write) -->
<!-- crate-map:end -->

## 3. Golden rules

<!-- The ten below are the contract's headings, the same in every app; each one's text and the
     check that holds it are in docs/app-architecture.md. Add this repo's own rules after them. -->

The contract is [docs/app-architecture.md](docs/app-architecture.md): read it before your first
change. Each rule names where it is written down.

1. **[Layers](docs/app-architecture.md#layers).** A crate depends only on strictly lower layers,
   and says its own in its manifest.
2. **[Every action is a command](docs/app-architecture.md#every-action-is-a-command).** One entry
   in one registry, with typed params and result, error codes, effects and requirements. No
   surface-specific action.
3. **[One dispatch](docs/app-architecture.md#one-dispatch).** The CLI, `serve`, the socket and MCP
   are thin adapters over `Session::call`; adding a command edits no transport.
4. **[The UI is a client](docs/app-architecture.md#the-ui-is-a-client).** It draws memory and calls
   commands on workers; it never writes a file, starts a program or reads a `serde_json::Value`.
   Its state is data `ui.inspect` and `ui.set` read and set, it draws what a command returns, its
   colours come from the theme (an identifier's from a range's `pick`) and its words from the
   catalog, and a change to it ships pictures.
5. **[Never crash](docs/app-architecture.md#never-crash).** A bad file, input or parameter is an
   error the user or agent can act on, never a panic. A crash's fix ships its regression test.
6. **[Never hang on another program](docs/app-architecture.md#never-hang-on-another-program).**
   Every child process starts in the process module (`crates/backends`), with deadlines, a capped output and its own
   process group.
7. **[Never hang on a lock](docs/app-architecture.md#never-hang-on-a-lock).** The app's own lock,
   never `std::sync::Mutex`; one lock read per statement.
8. **[State on disk](docs/app-architecture.md#state-on-disk).** Versioned, owned by one module,
   written atomically, saved before memory changes.
9. **[No secrets out](docs/app-architecture.md#no-secrets-out).** No result, log line or error
   carries a secret value.
10. **[Tests touch nothing real](docs/app-architecture.md#tests-touch-nothing-real).** Temp dirs, no
    network, no installers, no tool the machine may lack.

This repo's own, beyond the contract:

11. **Tests are the gate.** Every change comes with tests; prefer ones that cross a real boundary.
12. **Dependencies are deliberate.** Prefer the standard library and what `Cargo.lock` already has.

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

**A task that adds a command** follows the `add-command` skill (`.claude/skills/add-command`):
the same steps every time, each ending in a check, with this repo's paths in
[docs/architecture.md › Adding a command](docs/architecture.md#adding-a-command).

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
