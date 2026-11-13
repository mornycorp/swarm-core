<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Fill in the parts in comments like this one and keep it true as the crates change.
     The contract it grows from is docs/app-architecture.md (a pinned copy): don't restate it here,
     say what is {{product}}'s own. Keep the heading "Adding a command": the pinned `add-command`
     skill links to it. -->
# Architecture

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

How {{product}} is built: its crates and layers, how a command flows through it, where its state
lives, and how a new command is added. The rules every change keeps are the pinned contract,
[app-architecture.md](app-architecture.md); this is the part that is {{product}}'s own.

## Layers

<!-- One row per crate, in the layer its manifest declares. Cut the layers this repo has no use
     for (`backends` is optional: an app with no data outside itself has none); the default map is
     AGENTS.md §2. -->

| Layer | Crate | What it holds |
|---|---|---|
| 0 `model` | <!-- `crates/model` --> | <!-- the types and parsers of what is read from outside --> |
| 1 `domain` | <!-- one crate per area --> | <!-- what each does: no I/O --> |
| 2 `backends` (optional) | <!-- `crates/<backend>`, one crate per backend or one for all --> | <!-- what it reads from and writes to outside itself (files, git, the network, a device): each call with a deadline and a cap --> |
| 3 `engine` | <!-- `crates/engine` --> | the session, the command registry, the one dispatch |
| 4 `automation` | <!-- `crates/automation` --> | `serve`, the loopback socket, MCP |
| 5 `ui` | <!-- `crates/ui` --> | the UI shell |
| 6 `apps` | <!-- `apps/{{cli}}`, `apps/<app>` --> | the CLI (`{{cli}}`) and the windowed app |

`xtask/` and the testkit (`crates/testkit`, a `[dev-dependencies]` entry only) sit outside the
stack. `cargo xtask ci` runs the `layers` check: a crate that uses a layer at or above its own
fails, and the error names the file and the rule.

**What each layer never does:** `model` and `domain` do no I/O; nothing below `ui` links a UI
toolkit; `automation` and `ui` never name a command or read a reply by field (a `serde_json::Value`
indexed by a string: one swarm made every command's params and results untyped, and spent seven
slices typing them, [lessons.md](lessons.md) 1); `apps` hold no logic a library could. The
**panic hunt** is each crate root's deny list: no `unwrap`, `expect` or `panic!` outside tests.

## How a command flows

Every surface is a thin adapter over one dispatch:

```text
 CLI (`{{cli}} run <id> <json>`) ─┐
 serve (JSON lines on stdio)      │
 loopback socket (JSON lines)     ├─▶  Session::call(id, params, opts)  ─▶  typed result
 MCP (`tools/call`)               │      │                                  or CommandError
 UI (`call_typed`, on a worker)  ─┘      └─ look up the command  (`unknown_command`)
                                            check the params    (`bad_params`, names the key)
                                            check it is enabled (`unavailable`, says why)
                                            check consent       (`consent_required`)
                                            run
```

1. A surface reads a request in its own framing and names a command by id
   ([the id rule](app-architecture.md#the-id-rule): MCP replaces each `.` with `-`).
2. It calls `Session::call` (the wire form, JSON in and out) or, for the UI and the app's own code,
   `Session::call_typed` (typed in and out), with a `CallOpts`: who is calling, a cancel token and
   whether the caller has consented to the command's effects.
3. The dispatch is the one place that looks the command up, checks the params against the
   command's type, checks its requirements and consent, and runs it.
4. The result, or a `CommandError` (a stable `code`, a `message`, a `remediation`, and for
   `bad_params` the `key`), goes back through the surface's framing. A surface never inspects a
   command's result and never names a command.

MCP runs on a session of its own (`{{cli}} mcp`) or attached to the app that is already running
(`{{cli}} mcp --attach <port> [--token-file <path>]`): its calls then go to that app's session over
its control socket (the window's `--socket`, or `{{cli}} serve --socket`), so an agent drives the
window a person has open. The tools and the answers are the same either way, and so is consent: MCP
grants none, so a command with an effect is `consent_required` on it even when the socket was
started with `--yes`.

<!-- The app's own part: its requirements (what `Requirement` holds), its extra error codes, how
     long-running commands report progress, anything a surface does beyond framing. -->

## Where state lives

<!-- One row per thing the app persists or keeps in memory. The contract's rules (versioned,
     one owner, the one atomic writer, saved before memory changes) are
     app-architecture.md › State on disk. -->

| State | Lives in | Owner (the one module that reads and writes it) | Version |
|---|---|---|---|
| Settings | <!-- the config dir --> | <!-- module --> | `schema_version` in the file |
| The session | memory, per process | the engine | not persisted |
| The UI's state (`ui.inspect`, `ui.set`) | the session; `--ui-state <file>` when the window closes | the window's `ui_state` module | `schema_version` in the file |
| <!-- projects, caches, journals --> | | | |

Every file is written by the engine's one atomic writer (`state::write_atomic`) and read with its
capped read, by the module that owns it; shared state in memory sits behind the engine's lock
(`lock::Lock`). The config directory comes from one function a test points at a temp dir; nothing
else names it.

## Adding a command

The recipe every new command follows. Each step has a check that fails when it is skipped. (The
pinned `add-command` skill, under `.claude/skills/`, carries the same steps for agents and links
here for {{product}}'s own details: the paths and commands below. Change a step here and the skill
is Swarm Core's to change.)

1. **Name it.** An id `<area>.<name>` by [the id rule](app-architecture.md#the-id-rule), a one-line
   title and a one-line summary. Ids are never renamed once released.
2. **Write the types.** A `Params` type and an `Output` type in the area's crate (the model layer
   if a parser is involved), deriving `serde` and `schemars::JsonSchema`. Put
   `#[serde(deny_unknown_fields)]` on the params so a misspelt key is a `bad_params`, not ignored.
   The result carries every field a client needs, so the UI never reads a `serde_json::Value`.
   <!-- where the area's types live, e.g. crates/<area>/src/<command>.rs -->
3. **Implement `Command`**, the typed adapter
   ([the minimum interfaces](app-architecture.md#the-minimum-interfaces)): `ID`, `TITLE`,
   `SUMMARY`, `type Params`, `type Output` and `run(&self, session, params, cx)`. Return a
   `CommandError` with a code and a remediation, never panic, and poll `cx`'s cancel token in
   anything slow.
4. **Declare its effects and requirements.** `EFFECTS`: `writes`, `runs_code`, `network`,
   `device`, each true only if the command can do it (the default is all false: it only reads).
   `REQUIRES`: what must hold for it to run (`Requirement::Session`, an open project, a device).
   A command that writes outside the atomic writer, or starts a program outside the process
   module, fails `scan`.
5. **Register it, in one place:** `Registry::register(MyCommand)` in
   <!-- the file that lists the commands -->. It is the only call that builds a `CommandSpec`, and
   it refuses a bad or a taken id. No transport changes: the CLI, `serve`, the socket and MCP all
   pick the command up.
6. **Regenerate the schemas and the command docs:**
   <!-- the commands, e.g. `cargo xtask schemas`, `cargo xtask docs` -->. Commit the output: the
   gate checks it is current.
7. **Test it**, in the area's crate and in the testkit:
   - **behaviour:** the result for good params, and each error code it can return, with its
     remediation;
   - **bad params name the key:** a missing and an unknown param each give `bad_params` with
     `key` set to the offending JSON key, and so does a wrongly typed one (the dispatch tracks the
     path of the value serde refused: `key` is its top-level param; the conformance run requires it);
   - **a disabled state:** with an effect, the call without consent gives `consent_required`
     (and runs with it); with a requirement that doesn't hold, `unavailable` naming it;
   - **the conformance run:** swarm-core's run (`swarm_core_testkit::conformance`,
     which `apps/<cli>/tests/surfaces.rs` calls for every surface) includes the new command
     (registering is enough): it calls `run` with valid params built from the params schema,
     with consent, and holds that it never panics, that the surfaces agree, and that without
     consent a command with an effect is `consent_required` everywhere (on MCP even with it). A
     command with required params that none of those calls gets to succeed needs a known-good
     call in `Ours::examples` in that file.
8. **Check it from the CLI:** `{{cli}} run <id> '<json params>'` prints the result, and a wrong
   key prints the error with its code, message and remediation. Try it through `serve` or the
   socket too if it is long-running or needs consent.
9. **Record it:** the command's area docs if it is a capability a person sees, and this file's
   revision history if it changes how a command flows.

A command that needs a new error code, effect or requirement the contract doesn't have is a
change to the app, or, for the pinned ones, to Swarm Core.

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template: the layers, how a command flows, where state lives, Adding a command |
