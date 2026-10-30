<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Fill in the parts in comments like this one; keep the rules, which the swarms before
     you paid for (docs/lessons.md). -->
# Architecture

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

{{product}} is built the way every swarm on Swarm Core builds: **an engine of typed commands that
every front end drives.** The UI, the CLI, the control channel and MCP are thin callers of one
command registry, so whatever a person can do, an agent can do and a test can check, without a
window.

## Layers

From the bottom. A crate depends only on crates in strictly lower layers; apps and tooling sit
on top and nothing depends on them. `cargo xtask layers` enforces it from the table in `xtask/`
(`Hooks::layers`) and writes the crate map in [AGENTS.md](../AGENTS.md#2-workspace-map).

| Layer | Holds | Never |
|---|---|---|
| **L0 model** | The domain's data and its pure algorithms: parsing hostile input, the core computations, no I/O, no clock | a UI crate, I/O, `SystemTime::now` |
| **L1 domain** | What the product computes over the model: layouts, statistics, queries | a UI crate, I/O |
| **L2 sources** (when there are any) | Where data comes from: files, git, the network, each call with a deadline and capped output | a UI crate |
| **L3 engine** | The `Session` and the **command registry**: every action is a typed command with an id | a UI crate |
| **L4 front ends** | `{{ui_crate}}` (the UI shell: panes, menus, UI state as data) and automation (the control channel's protocol, the headless server, MCP) | engine internals: only commands |
| **apps, tooling** | `{{app}}` and `{{cli}}`, the binaries that wire it together; `xtask` | (nothing depends on them) |

<!-- This repo's crates, a line each, with their layer: the crate map in AGENTS.md is generated,
     so here say what each layer means for this product. Merge layers that would be empty. -->

## How a command flows

```text
 menu click / shortcut ─┐
 control channel (TCP) ─┼─► the shell's dispatch ─► (pure UI state? handle it here) ─► Session::execute(id, params)
 headless / CLI / MCP ──┘                                                                  │
                                                       registry: find(id) ─► enabled? ─► run ─► typed result
```

- **Everything is a command.** New user-visible behaviour is an engine command first: an id
  (`area.name`, lowerCamelCase segments), a label, its menu path and shortcut if it has them, an
  `enabled` check that says why not, and `run`. The UI, the CLI, the control channel, the headless
  server and MCP all dispatch by id: a command with a menu path shows in the menu with no UI
  change, and is an MCP tool with no MCP change. Only pure view state (theme, open windows) is the
  shell's own.
- **`Session::execute`** checks the params and enablement, catches a panic inside a command (an
  error; the session stays usable) and journals the commands that change state.
- **The registry lists itself.** `engine.commands` returns every command with its params and
  result schemas, which the CLI's help and MCP's tools are built from.

## Typed contracts (from the first commit)

Every boundary another tool or agent crosses is a type: command params and results, control
messages, persisted state, errors. A swarm that started untyped built 100+ commands in a week and
then spent seven slices typing them ([lessons.md](lessons.md) §1).

- **Commands are typed.** A command is a params type (`Deserialize`, a JSON Schema, unknown
  fields refused) and a result type (`Serialize`, a JSON Schema), registered through one typed
  adapter. The adapter bounds and parses the params and serializes the result: no command reads
  raw JSON.
- **Consumers read typed results.** The UI, the CLI, automation and tests deserialize a reply into
  its result type; they never index a `serde_json::Value` (`v["field"]`, `.get("field")`,
  `.as_object()`). `cargo xtask layers` refuses a new one in the directories the table names.
- **Errors are typed**, each variant with a stable error code in transport replies.
- **Persisted state carries a version**, and a state from an older or newer build still reads,
  keeping every section it can.

## The UI

- **Thin and data-driven.** The UI's state is plain data (serde), so the control channel reads it
  (`ui.inspect`) and drives it (`ui.set`), and the app saves and restores it.
- **What is drawn is what a command returns.** A pane draws a command's result; a test checks
  that what it drew equals the command's answer. Nothing the UI decides is untestable headless.
- **Colours from theme tokens, text from a string catalog**, never literals.
- **Never block the UI.** Slow reads run in the background and land through the session's poll;
  the CLI and agents run them in the foreground.
- **Verify visually.** Render offscreen (a snapshot example writing a PNG, driven by a script of
  commands) or launch with the control channel and drive it; attach before and after images to
  the PR.

## Automation

- **The CLI** (`{{cli}}`) runs the engine headless: list the commands, run one or a script of them
  (`{{cli}} run …`), serve the control protocol, serve MCP. It is how an agent checks a change
  without a window, and the smoke gate's headless run.
- **The control channel**: the desktop app, started with `--control`, listens on loopback with a
  token, JSON lines in and out: every engine command, plus `ui.inspect` and `ui.set`.
- **MCP**: the same registry as tools.

<!-- docs/control-protocol.md, when there is one: the methods, framing, errors. -->

## Never crash

A malformed file, a bad command or control param, a corrupt saved state or an odd keystroke is an
error the user or agent can act on, never a panic.

- Non-test code never panics (`unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`); no
  `unsafe`. Every crate root says so, and `cargo xtask layers` checks it.
- Input-derived numbers are hostile: `get()` not `[i]`, `checked_*`/`saturating_*`, division and
  NaN guarded, allocations and strings capped.
- Nothing reaches a command line unchecked; every external call has a deadline.
- A **panic hunt** test runs every command with adversarial params and must stay green; every
  crash fix comes with the test that panicked before it.

## Adding a command

1. **The algorithm** goes in the lowest layer that fits, with its own tests and no UI.
2. **The command** goes in the engine (`<area>_cmds.rs`), typed, registered in one line.
3. **Tests:** behaviour, disabled states and bad params (each an `Err`, never a panic), reading
   the reply as its result type; the panic hunt picks it up.
4. **UI** only if it needs a pane or dialog, its state in the UI state, its text in the catalog.

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template |
