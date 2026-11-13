<!-- A pinned copy from swarm-core (crates/swarm-core/assets/docs/app-architecture.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`: the gate's
     `sync-assets --check` fails on a copy that differs. -->
# The app architecture contract

> **Last reviewed:** 2026-11-12 · **Last updated:** 2026-11-13 · **Change:** minor (the skeleton's colours are the palette the swarm's tools share; only `backends` touches the outside: the process module and the atomic file writer live there; MCP attached to a running app; the `sources` layer is `backends`) · **Target:** M1

How {{product}} is shaped, and what keeps it robust. An app built on Swarm Core is **headless
first**: every action is a typed command in one registry, every surface (the CLI, JSON lines on
stdio, the app's loopback socket, MCP, the UI) is a thin adapter over one dispatch, and a person
or an agent can run the whole app without a window. The rules below are the same in every app, so
this document is pinned: `AGENTS.md` links to it instead of restating it, and a change to it is a
Swarm Core change and a bump in each app ([swarm-protocol.md](swarm-protocol.md) › The
coordinator's loop, step 9).

This is the short, generic contract. The app's own long form is its
[architecture.md](architecture.md) (its crates, its commands, where its state lives) and its own
protocol document; the framing limits below are recommendations with numbers, not a wire spec.

Each rule names **the check that holds it**. `layers` and `scan` are Swarm Core's shared
checks (`swarm_core::checks`) that the app's `cargo xtask ci` calls; `clippy` is a lint in
`clippy.toml` or a crate root's header; `conformance` is the testkit's run over every surface;
`review` means no tool can check it, so the architecture review and every PR's reviewer do
([lessons.md](lessons.md) holds the mistakes behind each rule).

| Rule | Check |
|---|---|
| [Layers](#layers) | `layers`, `review` |
| [Every action is a command](#every-action-is-a-command) | `scan`, `conformance` |
| [One dispatch](#one-dispatch) | `conformance`, `scan` |
| [The UI is a client](#the-ui-is-a-client) | `scan`, `layers`, `conformance`, the views' tests, `review` |
| [Never crash](#never-crash) | `clippy`, `conformance`, `review` |
| [Never hang on another program](#never-hang-on-another-program) | `scan`, the process module's tests |
| [Never hang on a lock](#never-hang-on-a-lock) | `clippy`, the lock's tests, `review` |
| [State on disk](#state-on-disk) | `scan`, the writer's tests, `review` |
| [No secrets out](#no-secrets-out) | tests, `review` |
| [Tests touch nothing real](#tests-touch-nothing-real) | `scan` |

## Layers

Dependencies point down; a crate depends only on strictly lower layers.

| # | Layer | Holds | May use |
|---|---|---|---|
| 0 | `model` | the types and parsers of everything that is read from outside: files, wire frames, user text. **Hostile input, no I/O.** | the standard library, `serde` |
| 1 | `domain` | libraries that do the app's work (one crate per area). **No I/O:** what it needs from outside is handed to it by a layer above | model |
| 2 | `backends` | **the one layer that touches the outside**: it starts every child process, writes every file, and holds what the app reads from and writes to outside itself (files, git, the network, a device). **Every call has a deadline and a capped result** | model, domain |
| 3 | `engine` | the session, the command registry, the one dispatch | model, domain, backends |
| 4 | `automation` | the transports: `serve`, the loopback socket, MCP | engine |
| 5 | `ui` | the UI shell: toolkit code, drawing, input | engine |
| 6 | `apps` | the binaries: the CLI and the windowed app | any lower layer |

`xtask` and the testkit sit outside the stack: `xtask` is the repo's tooling, the testkit
(`crates/testkit`, the app's own; Swarm Core's `swarm-core-testkit`, with the conformance run,
beside it) is **only a `[dev-dependencies]` entry**, never a dependency of production code.

- **Declared in the manifest.** Every member crate says its layer in its own `Cargo.toml`
  (`[package.metadata.swarm] layer = "engine"`), so there is no table to keep in step. It names
  the layer, never its number: the numbers above are only the order. (An app that keeps its
  layers in its xtask's table, `Hooks::layers`, is held to the same order from there.)
- **Only `backends` touches the outside.** The process module
  ([Never hang on another program](#never-hang-on-another-program)) and the one atomic file
  writer with its capped read ([State on disk](#state-on-disk)) live in a `backends` crate
  (`layer = "backends"`; the skeleton's is `crates/backends`), and so does whatever else the app
  reads from or writes to outside itself: files, git, a network service, a device. So every
  outside call, in either direction, has one home, a backend that runs a program or writes a
  file uses the same two modules as everything else, and the domain stays pure. Each call there
  has a deadline (a hung server or a slow disk is a typed error, not a hang) and caps what it
  keeps (a huge answer is `too_large`, never an allocation sized by the other side); what it
  reads is parsed by the model, as hostile input.
  - **It does the I/O; the layers above decide.** The engine, or the module that owns a file,
    says what is stored, its `schema_version`, its migrations and when it is saved; a command
    declares the effect (`writes`, `runs_code`, `network`, `device`) and the dispatch asks for
    consent. A backend's failure is its own typed error, which the engine turns into the
    command's: what couldn't be done there (a deadline passed, the disk refused) is
    `unavailable`, not `internal`, as it is no bug in the app.
  - **A transport's own socket and stdio are not this layer's**: they are how the app is
    reached, in `automation`.
  - **A further backend is the app's choice** (a module in that crate, or a crate of its own in
    the layer); an app with no data outside itself has only the two modules.
  - An app made from a skeleton before 2026-11-13 keeps them in its engine until it moves them:
    nothing below the engine can then start a program or write a file. The layer was `sources`
    until then too; a manifest that still says so is read as `backends`.
- **UI toolkits only in the UI layer** and the apps built on it (the windowed app). The CLI and
  every layer below never link one.
- **Lints and headers.** `[lints] workspace = true` in every member; every crate root carries the
  `forbid(unsafe_code)`, deny and code-health headers (`crates/swarm-core/src/lib.rs` is the
  model).
- **`unsafe` only where the app grants it.** A library that binds what can't be written without
  `unsafe` (an OS or GPU API) is listed in the app's `Hooks::unsafe_crates` with a one-line
  reason. Its library root carries `#![deny(unsafe_op_in_unsafe_fn)]` and
  `#![warn(clippy::undocumented_unsafe_blocks)]` in place of the forbid; its binaries, and every
  other crate, still forbid it. A workspace with such a crate sets `unsafe_code = "deny"`, not
  `"forbid"`, in `[workspace.lints.rust]` (a forbid can't be lowered), and the listed root says
  `#![allow(unsafe_code)]`.
- **No committed `[patch]` of Swarm Core** (a checkout under test goes in the user-level cargo
  config, never in a repo).

**Check:** `layers` (`swarm_core::checks::layers`; its error names the file, the rule and the
fix): the order, so a `domain` or `model` crate can't use a `backends` one. `review` for the rest:
that `model` and `domain` do no I/O, and that each outside call in `backends` has its deadline and
its cap. `scan` holds the process starts and the file writes to that layer (below).

## Every action is a command

An action a person, an agent, a test or another program can take is a **command**: one entry in
one registry, registered in one place, with

- an **id** (the rule [below](#the-id-rule));
- a **title** and a **summary**: one line each, the words a command list shows;
- **typed params** and a **typed result**: Rust types, and the JSON Schemas that describe them are
  generated from the types (`schemars`), never written by hand;
- stable **error codes**, each with a remediation ([The error](#the-error));
- declared **effects**: what it can change outside the app's memory (`writes` files,
  `runs_code` other programs, uses the `network`, changes a `device`), so a surface can ask for
  consent before a command that needs it and a reader can see what a command may do;
- declared **requirements**: what must hold for it to run (a session, an open project, a connected
  device); one that doesn't hold fails with the app's `unavailable` code and says which;
- an **enabled check**: whether it can run now, on the session's state, and why not
  ([Enabled](#enabled-can-it-run-now));
- an optional **menu path** and **shortcut**: where a UI puts it and the keys that run it, as
  plain text (`File/Open recent`, `Mod+O`), so a UI builds its menus, toolbars and shortcuts from
  the registry without naming a command;
- `run`: the function.

There is no surface-specific action. A button, a menu item, a CLI flag or an MCP tool that does
something calls a command; one that has no command gets one first.

**Check:** `scan` (a `CommandSpec` built outside the typed adapter); `conformance` (every command
has params and result schemas, behaves the same on every surface, and has the same enabled state
on each).

### The minimum interfaces

These are the signatures the skeleton (`swarm-core init --app`) implements. They are the
contract: where code and this text differ, this text changes first.

```rust
/// What a command is, as every surface sees it. Built only by `Registry::register`.
pub struct CommandSpec {
    pub id: &'static str,        // "app.info", by the id rule
    pub title: &'static str,
    pub summary: &'static str,
    pub params: schemars::Schema, // generated from `Command::Params`
    pub result: schemars::Schema, // generated from `Command::Output`
    pub effects: Effects,
    pub requires: &'static [Requirement],
    pub menu: Option<&'static str>,     // "File/Open recent": the menus, then the item's label
    pub shortcut: Option<&'static str>, // "Mod+O": portable, `Mod` is Cmd on macOS, Ctrl elsewhere
}

/// Why a command can't run now. A call of it fails `unavailable` with this message and remediation.
pub struct Why { pub message: String, pub remediation: String }

/// Whether a command can run now: `why` is the very error a call of it gets, `None` when enabled.
pub struct CommandState { pub id: &'static str, pub enabled: bool, pub why: Option<CommandError> }

/// What a command can change outside the app's memory. All false: it only reads.
#[derive(Clone, Copy, Default)]
pub struct Effects { pub writes: bool, pub runs_code: bool, pub network: bool, pub device: bool }

/// The typed adapter: a command is a type. `Registry::register` is the only place a
/// `CommandSpec` is built, so no command is registered without schemas or effects.
pub trait Command: Send + Sync + 'static {
    type Params: serde::de::DeserializeOwned + schemars::JsonSchema;
    type Output: serde::Serialize + schemars::JsonSchema;
    const ID: &'static str;
    const TITLE: &'static str;
    const SUMMARY: &'static str;
    const EFFECTS: Effects = Effects { writes: false, runs_code: false, network: false, device: false };
    const REQUIRES: &'static [Requirement] = &[];
    const MENU: Option<&'static str> = None;
    const SHORTCUT: Option<&'static str> = None;
    /// Whether it can run now, beyond `REQUIRES`: cheap, reads memory only. The default: always.
    fn enabled(&self, session: &Session) -> Result<(), Why> { Ok(()) }
    fn run(&self, session: &Session, params: Self::Params, cx: &CallContext) -> Result<Self::Output, CommandError>;
}

impl Registry {
    /// Registers `command`; refuses an id the rule rejects or one already registered, and a menu
    /// path or a shortcut of the wrong form or another command's.
    pub fn register<C: Command>(&mut self, command: C) -> Result<(), RegistryError>;
    /// Every command's spec, in id order (what `commands`, `tools/list` and the docs are made from).
    pub fn commands(&self) -> Vec<&CommandSpec>;
}

impl Session {
    /// Every command's state now, in id order: one call, for a menu.
    pub fn states(&self) -> Vec<CommandState>;
}
```

`Requirement` is the app's own enum (the skeleton has `Session` only); `Requirement::check` returns
`Result<(), Why>`. A transport never builds a `CommandSpec` and never names a command: it asks the
registry.

**A menu path** is the item's place: the menus from the menu bar down, then the item's own label,
joined by `/` (`File/Open recent`, `Help/App info`), two to four parts. **A shortcut** is portable
text: modifiers (`Mod`, `Ctrl`, `Alt`, `Shift`, each at most once, in that order), then one key (a
letter `A`-`Z`, a digit, `F1`-`F24`, or a named key such as `Enter`, `Escape`, `Tab`, `Delete`,
`PageUp`, `Left`, `Plus`), joined by `+`: `Mod+O`, `Mod+Shift+P`, `F5`. `Mod` is the platform's
command key (Cmd on macOS, Ctrl elsewhere). Both are UI metadata in the engine's spec: plain text,
never a toolkit type, so no layer below the UI links a toolkit (`layers`); the UI maps them to its
toolkit's menus and keys. The registry refuses one of the wrong form, and one another command has.

### Enabled: can it run now

A UI greys out a menu item, a toolbar button or a shortcut that can't run now, and says why, without
naming the command. So the registry answers, for every command at once, whether it can run on the
session's state and, if not, why: `Session::states`, one call for a whole menu. Each state is
enabled, or disabled with the `unavailable` error a call of it would get (its message says what
doesn't hold, its remediation what to do). A call of a disabled command fails `unavailable` with
the **same** reason.

**The decision (2026-11-12): one check, made in one place, from two sources.**
Whether a command can run now is its declared **requirements** (`REQUIRES`: conditions many
commands share, such as an open project, which every reader of `commands` sees) and then its own
**`enabled`** method (a condition only it has: nothing to undo, nothing selected). Both are made by
**one function of the session** that the dispatch's `call` and `call_typed` and the `states` query
all call, so a disabled call and the query can't disagree: there is no second copy of the check to
drift. Requirements alone were the other choice; they can't say "nothing to undo" without a variant
per command in a shared enum, and a test can't make one. `enabled` alone would hide the shared
conditions inside code no surface can read. A `Why` can only become `unavailable`, so a command is
never disabled with another code.

- **Cheap and pure.** A requirement's check and `enabled` read memory, do no I/O, never wait on a
  lock and change nothing: a UI asks every command's state each frame. Anything slower is the
  command's `run`, which fails on its own terms.
- **No params.** The state is the session's, not a call's: a check that needs the params belongs
  in `run` (`bad_params`, `not_found`), not in `enabled`.
- **Not consent.** Consent is the caller's, per call: a command that needs it is enabled, and its
  call without consent fails `consent_required` (after the enabled check: a disabled command says
  so first).
- **On every surface.** The session's `states`; the frame's reserved method `enabled` on `serve`
  and the socket; `{{cli}} enabled`. MCP has no such method: a client learns a tool is disabled
  from its call, an `isError` result with `unavailable` and the reason.

### The id rule

A command id is **`<area>.<name>`**: two or more dot-separated segments, each `[a-z][a-z0-9_]*`,
at most 64 characters in all (`app.info`, `project.open_recent`). Ids are never renamed once
released: a rename is a new command and the old one stays, deprecated, until no one calls it.
`Registry::register` refuses an id that doesn't match and one already taken.

Every surface derives its own name from the id **by the same function** (the skeleton's
`ids` module), never by hand for a command:

| Surface | Name | Example `project.open_recent` |
|---|---|---|
| CLI | `{{cli}} run <id>`, always; an app that also offers subcommands maps each dot to a space and each `_` to `-` | `{{cli}} run project.open_recent`, or `{{cli}} project open-recent` |
| JSON lines, socket | the request's `method` is the id | `"method": "project.open_recent"` |
| MCP | the tool name is the id with each `.` replaced by `-` (MCP allows only `[A-Za-z0-9_-]`, at most 64) | `project-open_recent` |

The mapping is one to one (an id has no `-`, a tool name has no `.`), so a tool name always
resolves to one command. Two segments at least keep an id clear of the protocol's own methods
(`commands`, `enabled`, `auth`).

### The error

Every failure of a call is one type, on every surface:

```rust
pub struct CommandError {
    pub code: ErrorCode,         // stable, machine-read, one of the codes below or the app's own
    pub message: String,         // what happened, for a person
    pub remediation: String,     // what to do about it: never empty
    pub key: Option<String>,     // `bad_params` only: the param that is wrong, by its JSON key
}
```

```json
{"code": "bad_params", "message": "missing field `path`", "remediation": "pass `path`, a file path", "key": "path"}
```

The codes every app keeps (their spelling and meaning never change; an app may add codes of its own
beside them, such as `unavailable`, and never reuses one of these):

| Code | Meaning |
|---|---|
| `bad_params` | the params aren't the command's: the wrong type, a missing or an unknown key (`key` names it) |
| `unknown_command` | no command has this id |
| `not_found` | the thing the params name (a file, an item, a device) doesn't exist |
| `too_large` | the request, a param or a result is over a cap |
| `unauthenticated` | a transport that needs a token didn't get the right one |
| `consent_required` | the command's effects need a consent the caller didn't give |
| `cancelled` | the caller (or a deadline) cancelled the call |
| `internal` | the app's fault: a bug, not the caller's. The message says what, the remediation says to report it; the app logs the detail |

A consumer classifies a failure by `code`, never by its message.

## One dispatch

The CLI, `serve`, the app's loopback socket and MCP are **thin adapters over the same call**:

```rust
pub struct CallOpts {
    pub caller: Caller,          // Cli | Serve | Socket | Mcp | Ui | Test: logged, never branched on
    pub cancel: CancelToken,     // set by a dropped connection, a deadline, the UI
    pub consent: bool,           // the caller has agreed to the command's effects
}

impl Session {
    /// The wire form: params and result are JSON. Only transports call it.
    pub fn call(&self, id: &str, params: serde_json::Value, opts: &CallOpts) -> Result<serde_json::Value, CommandError>;
    /// The typed form: the UI and the app's own code call it, and read the typed result.
    pub fn call_typed<C: Command>(&self, params: C::Params, opts: &CallOpts) -> Result<C::Output, CommandError>;
}
```

`call` is the one place that looks the command up (`unknown_command`), checks the params against
its type (`bad_params`, with the key), checks that it is enabled (its requirements, then its own
check: `unavailable`, with why not; [Enabled](#enabled-can-it-run-now)), refuses an effect that needs a
consent the caller hasn't given (`consent_required`), runs it, and turns a panic in it into an
`internal` error (the lints leave none in the app's own code, [Never crash](#never-crash); a
dependency or an index can still raise one, and it must not take the surface down). A transport only frames: it reads a request, calls,
writes a reply. **Adding a command adds it to every surface**, with no edit to any transport.

- **`serve`** speaks JSON lines on stdio; **the loopback socket** the same frames over TCP on
  `127.0.0.1`; **MCP** (`tools/list`, `tools/call`) generates its tools from the registry: a tool
  per command, its `inputSchema` the command's params schema, its name by the id rule, and its menu
  path and shortcut, when it has them, in the tool's `_meta` (`{"menu": …, "shortcut": …}`). A large
  registry discloses its tools by toolset, progressively, rather than all at once.
- **MCP and consent.** A command whose effects need a consent returns `consent_required` until the
  call carries one; the MCP server sets it only from what its client has granted, never by default.
- **MCP on its own session, or attached.** The MCP server runs on a session of its own
  (`{{cli}} mcp`), or attached to an app that is already running (`{{cli}} mcp --attach <port>`):
  its calls then go to that app's session over the app's loopback socket, as a frame client's do
  (`auth`, `commands` once, then a request per call), so an agent works in the window a person has
  open. It is the same MCP code over a seam of two calls (list the specs, call one), with **the
  same answers** and **the same consent rule**: it names no command and grants no consent, so a
  command whose effects need it is `consent_required` there even when the app's socket was started
  with `--yes`. That is decided from the command's effects, as `commands` gives them, before the
  call is sent, so the app never sees it (such a call that the app would refuse anyway, for its
  params or because it is disabled, may then be `consent_required` rather than `bad_params` or
  `unavailable`; it runs on neither). An app it can't reach, a wrong token or a connection that
  drops is a JSON-RPC error with the code and a remediation; connecting and each reply have a
  deadline, and a connection is made again at most once a call, never in a loop.
- **Cancel.** A dropped connection, an idle timeout or the UI's cancel button sets the call's
  token; a slow `run` polls it and returns `cancelled`.

### The JSON-lines frame

One JSON object per line, UTF-8, `\n`-terminated.

```json
{"id": 1, "method": "app.info", "params": {}}
{"id": 1, "ok": true, "result": {"name": "{{cli}}", "version": "0.1.0", "schema_version": 1}}
{"id": 2, "ok": false, "error": "missing field `path`", "code": "bad_params", "remediation": "pass `path`, a file path", "key": "path"}
```

- A request is `{id, method, params}`: `id` is a number or a string the reply echoes, `method` a
  command id or a reserved method (`commands`: the registry's specs with schemas, menu paths and
  shortcuts; `enabled`: every command's state now, `[{"id": "app.info", "enabled": true}, {"id": …,
  "enabled": false, "why": {"code": "unavailable", "message": …, "remediation": …}}]`; `auth`),
  `params` an object (absent means `{}`).
- A reply is `{id, ok: true, result}` or `{id, ok: false, error, code}`: `error` is the failure's
  message, `code` its code, and `remediation` and `key` ride beside them when there are any. A
  line that isn't a request gets `{id: null, ok: false, …}` with `bad_params`.
- **A spec in `commands` carries its effects** as `"effects": {"writes": false, "runs_code": false,
  "network": false, "device": false}`. A client that decides consent from them (`mcp --attach`)
  counts **any effect that isn't `false`**, and effects that are missing, as needing consent, so an
  effect an app adds later is never read as none.
- **The loopback socket's `auth`** comes first: `{"id": 0, "method": "auth", "params": {"token": "…"}}`.
  The token is read from a file made `0600` (or `{{env}}_CONTROL_TOKEN`), compared in constant
  time, and never logged. Anything but a good `auth` first gets `unauthenticated` and the
  connection closes.
- **Limits** (recommended, so every app starts from the same numbers; each app's protocol document
  states its own): a request line of 1 MiB at most, a reply of 8 MiB, 16 connections at a time, a
  connection closed after 30 s idle. A request over its cap gets `too_large` (the transport
  discards the rest of the line, or closes); a reply over its cap is `too_large` too, never a
  truncated result.
- The socket binds `127.0.0.1` only, never `0.0.0.0`.

**Check:** `conformance` runs every command through each surface and compares, with hostile params
and with valid ones built from its params schema (which reach `run`), with consent and without (a
command whose effects need it is `consent_required` without it, and on MCP even with it); it asks
every command's enabled state on each surface that has the query and holds them to agree, and a
disabled command's call to fail `unavailable` with its state's reason; it holds a call cancelled
before it runs to fail `cancelled`; the app's surface test sends
every command the same MCP messages through `mcp --attach` (on a socket started with consent) and
through `mcp`, and holds the replies to be the same; `scan` holds the transports to the registry
(no command named in one).

## The UI is a client

The UI draws memory and calls commands on **workers**. It never writes a file, starts a program,
blocks its own thread on a call, or reads the disk while drawing; a result arrives as a message
the next frame draws. (A call that only reads or changes memory, as `ui.inspect`, `ui.set` and the
enabled query do, may be made on its own thread.) It reads **typed results** (`call_typed`), never indexes a `serde_json::Value`:
a field it needs is in a result type, so a rename is a compile error, not a blank panel.

It **builds its menus, toolbars and shortcuts from the registry** (each command's menu path and
shortcut) and greys out what can't run now with the reason (`Session::states`, asked each frame:
the dispatch's own check), so it names no command to make a menu and never disagrees with a call.
**It asks; it doesn't compute.** An answer about the model is a command's, in a lower layer: what
is on screen and where, how big each thing is, what lies under a point, how a screen position
maps to the model, what a drag in progress is doing. The UI turns pixels and input into a
command's params and draws its result. A view that needs an answer no command gives asks for the
command (an issue, or the algorithm lower down first) rather than working it out itself, so the
CLI, the socket, MCP and a headless test can ask the same question ([lessons.md](lessons.md) 16).

The contract picks no toolkit. The skeleton's UI shell uses egui, and the choice is the
app's.

**Nothing the UI does is untestable headless**: its state is data the
commands read and set, what it draws is what a command returns, its colours and words come from
one place each, and a change to it ships with pictures. Each of these names its check:

| Rule | Check |
|---|---|
| [UI state is data](#ui-state-is-data) | `conformance` (`ui.inspect` and `ui.set` on every surface), the state's tests, `review` |
| [What is drawn is what a command returns](#what-is-drawn-is-what-a-command-returns) | the view's test, `review` |
| [Colours from the theme, words from the catalog](#colours-from-the-theme-words-from-the-catalog) | `scan` (`ui-colours`, `ui-strings`), the theme's tests (contrast, the pick's pins), `conformance` (`ui.theme`), `review` (an identifier coloured anywhere but `pick`) |
| [Verify visually](#verify-visually) | `review` (the PR's Screenshots section) |

### UI state is data

The UI's state is **plain data**: the views a person has open, what is selected, where each view
is scrolled to, the layout they chose (a pane shown or hidden, a split, a column's width). It is
`serde` types with no toolkit type in them, **versioned with the rest of the state on disk** (its
`schema_version`), and **commands read and set it**: `ui.inspect` returns it, and `ui.set` changes
what its params give and leaves the rest (ids [by the id rule](#the-id-rule); a param that names
what isn't there is `not_found`, and changes nothing). The CLI, the socket, MCP and a headless test
then see and drive what the window shows, as a click does: the UI changes its own state through
the same `ui.set`, and draws what `ui.inspect` gives.

- **The app saves it and restores it**, by [State on disk](#state-on-disk)'s rules: one module
  owns the file, the one atomic writer writes it, a file from a newer version is refused and never
  overwritten, and one that can't be read is left alone (the window starts from the default). A
  saved selection of something that is gone (a command an older version had) is dropped. It is
  saved whole when the window closes, not at each click: it changes with each one, and a crash
  then costs a layout, not a setting.
- **What isn't UI state:** what the toolkit works out each frame (a pixel, a hover, an animation's
  phase), a command's result (the command's, asked again) and a consent (a person's, for one run of
  one command: a selection changed elsewhere withdraws it).
- **Where it lives** is the app's: the skeleton keeps it in the session, which every surface
  reaches; an app whose UI crate owns it answers `ui.*` on its UI thread between frames. Either way
  the commands are the only way in.

### What is drawn is what a command returns

A view draws a **command's typed result**, as the command gave it: it computes nothing the command
didn't (no total, no filter, no derived state; [lessons.md](lessons.md) 16), and a value it needs
that the result lacks is a field added to the result, in the command. **A test checks that what
the view drew equals the command's answer**: it draws a frame headlessly (egui's context needs no
window and no GPU), reads the text the frame drew, and compares it with `call_typed`'s result for
the same params; or `ui.inspect` reports what a pane drew beside the command's answer, and the test
compares the two.

### Colours from the theme, words from the catalog

Drawing code makes **no colour** and writes **no word**. Each colour is a **theme token**, named
for what it means (`error`, `warning`, `selection`) and made in the theme module, from the
toolkit's look or the app's palette; each text a person reads is an entry in the **string
catalog** (a function of its values for a text with values in it). A theme, a contrast fix, a
rewording or a second language then changes one file, and a test can ask what a view says. The
words of the app's data (a command's title, a result's values) come from the registry and the
commands as they are, and an id the toolkit keeps state under is a typed salt (an enum's
variant), never a string.

**The skeleton's colours are the palette the swarm's tools share**, so an app built on Swarm
Core looks like the swarm's other tools: its tokens are that palette's chrome (a dark look, and
Qt Fusion's light palette as the light one), each under a name for what it means. An app may
change them; the skeleton starts from the swarm's.

**Categorical ranges** are the theme's colours for **identifiers**, where a colour tells things
apart rather than saying what they mean: the rows of a timeline, the bars of a scrubber, a legend's
entries, badges. Each is from a published palette chosen so its entries stay far apart (not an even
cycle of hues), with a dark and a light version. The skeleton's first two are the shared palette's:
the **series** (ColorBrewer Dark2 dark, Set2 light: a number takes entry `n % 7`, and something
with no number the series' grey, `none`) and the **markers** (ColorBrewer Set1 with the shared
orange `#FF7F1C`, then grey, the same in both looks). Its **16 and 32 ranges are Swarm Core's
own**, beyond those, for many identifiers side by side. A filled entry (a bar, a mark) is edged in
its darker border (each channel × 0.8).
**An identifier's colour comes from the range's `pick`**, a fixed and
documented hash of the id (the skeleton's: FNV-1a over the id's text), never a colour made where it
is drawn and never std's `DefaultHasher`, whose output can change: the same id gets the same colour
on every run, surface and machine, and tests pin known ids to known entries. A number in order (a
series, a marker) takes `at(n)`. A range colour drawn behind text is first made a text background
for the look (its lightness at most half in dark, at least half in light, then on until the text on
it reads).

The colours have their **contrast measured** (WCAG 2), in each look: a token read as text reaches
4.5:1 against what it is drawn on, a range entry 3:1 against the background (a part of the
interface), and text on an entry made a text background 4.5:1. **A shortfall is never nudged
silently**: a value the app takes from elsewhere (the skeleton's from the shared palette) that falls
short is either kept, and the theme's tests record it with its measured ratio and why, or nudged and
listed as a divergence from its source; a colour of the app's own that falls short is nudged, and the
theme says so beside it. **The theme is data a command returns**
(`ui.theme`: each token's name, meaning and value per look, each range's entries), so every
surface can ask it, and a swatch view draws that answer like any other view.

### Verify visually

A UI change ships with **pictures**, before and after, in the PR's Screenshots section. Where the
toolkit renders offscreen on the machine (egui's `egui_kittest` with its `wgpu` renderer, on a GPU
or a software Vulkan), an example renders the app to a PNG driven by a script of commands
(`ui.set`, then the commands whose views changed), so a picture is the same each time it is made.
Elsewhere the windowed app is run with its control socket (`--socket`), driven over it with the
same commands, and the window captured. CI machines have no GPU, so a picture is not a CI step:
the headless test of what is drawn is the check CI runs, and the pictures are the reviewer's.

**Check:** `scan` (file writes and process starts in the UI crate; `Value` reads in the UI,
automation and apps; `ui-colours` and `ui-strings`: a colour made, or a string literal written, in
the UI crate's drawing code outside the theme and catalog modules, each file held to an allow-list
that only ratchets down), `layers` (a toolkit only in the UI layer and the apps above it),
`conformance` (`ui.inspect`, `ui.set` and `ui.theme` behave the same on every surface), the views'
tests (what each drew equals its command's answer), the theme's tests (each value against its
source, each token and range entry's contrast in both looks with every shortfall kept recorded and
its ratio, the pick's pinned ids) and `review` (logic the UI computes that a lower
layer owns; UI state kept outside what `ui.inspect` returns; an identifier coloured other than by
its range's `pick`; a UI change without its pictures).

## Never crash

A hostile file, a malformed frame or a hostile param is the normal case, not the rare one.

- No `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!` or `unimplemented!` in production code,
  and no indexing or slicing on input (`.get(..)` and a typed error).
- Arithmetic on input is checked (`checked_add`, `saturating_sub`); recursion over input is
  bounded; an allocation sized by input is capped first.
- A parser returns a `Result` and says where it failed.
- **A fix for a crash ships its regression test**, the input that crashed, in the same PR.

**Check:** `clippy` (the deny list in every crate root; `allow-*-in-tests` in `clippy.toml`);
`conformance` (a pass of adversarial params that must panic nowhere); `review` for arithmetic,
recursion and allocation bounds.

## Never hang on another program

One module (the **process module**, in the `backends` layer) starts every child process. It, and
nothing else:

- closes the child's **stdin** (unless the command feeds it);
- sets a **wall-clock deadline** and an **idle deadline** (no output for so long);
- **caps the output** it keeps, and drains stdout and stderr on their own threads, so a chatty
  child never blocks on a full pipe;
- puts the child in its own **process group**, and kills the group on a timeout, on cancel and on
  drop, so a grandchild never outlives its caller or holds the caller's pipe;
- reports every failure (not found, timed out, killed, output over the cap) as a typed error
  with a remediation.

`std::process::Command` appears nowhere else. A new reason to start a program is a new function in
that module, not a new `Command` elsewhere.

**Check:** `scan` (`std::process::Command` outside the process module; allow-listed spots each
give a reason and a count); the process module's own tests (a child that ignores its stdin, one
that floods its output, one that spawns a grandchild, one that never exits).

## Never hang on a lock

A deadlock is a hang the person can't see the cause of.

- The app's own **mutex notices a deadlock and errors out** (a timeout, or a lock-order check)
  rather than waiting for ever; the error is a typed `internal` failure that names the lock.
- **`std::sync::Mutex` and `MutexGuard` are refused** by clippy's `disallowed-types`, which points
  at the app's own lock.
- **One lock read per statement**: a statement that takes a second lock while it holds the first,
  such as `f(a.lock(), b.lock())`, is how a whole app froze once. Take one, copy what you need
  out, drop it, take the next.

**Check:** `clippy` (`disallowed-types`); the lock's own tests; `review` for the one-per-statement
rule.

## State on disk

Everything the app persists (settings, UI state, caches, journals) is:

- **versioned**: a `schema_version` in the file, a schema, and a **migration path** from every
  earlier version (a file from a newer version is refused with a typed error, never overwritten);
- **owned** by one module that reads and writes it, and no other;
- written by **the one atomic writer**, in the `backends` layer (a temporary file in the same
  directory, flushed, then renamed), so a crash leaves the old file or the new, never half of one;
- **saved before memory changes**: the app writes the new state to disk first, and changes its
  memory only when that succeeded, so a failed save never leaves the screen saying what the file
  doesn't;
- loaded without ever **writing defaults over what it couldn't read**: a read that fails or blocks
  leaves the person's file alone.

**Check:** `scan` (a file write in any crate outside the `backends` layer, a crate the app adds
later too; allow-listed with a reason and a count); the writer's and each migration's tests; `review`.

## No secrets out

A settings view, a command's result, a log line, a journal entry and an error message never carry
a secret value (a token, a key, a password, a credential in a URL). A secret setting is stored and
shown redacted; a command that needs one reads it from the store, and the surface never sees it.
Tests use obvious fakes (`test-token-not-real`), never a real credential.

**Check:** the tests of each result that holds settings (it must show the redaction), and
`review`. No tool can find a secret in a message it doesn't know about, so every new setting asks
"is this one?".

## Tests touch nothing real

A test uses **temporary directories** for config and state (the testkit makes and removes them),
makes no network call, and runs no installer, no GUI program and no tool the machine may lack. A
test that wrote to the real config dir once reset a person's settings; the app's config dir
comes from one function that a test points at a temp dir.

**Check:** `scan` (the real config dir named in a test).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-13 | minor | The UI is a client, [Colours from the theme](#colours-from-the-theme-words-from-the-catalog): the skeleton's colours are the palette the swarm's tools share, so an app looks like them: its chrome tokens (surfaces, text, controls and highlights, states, each under a name for what it means), its series (Dark2 dark, Set2 light, `at(n)` as `n % 7` and a grey for none), its markers (Set1 with `#FF7F1C`, then grey) and its darker border for a filled entry; the 16 and 32 ranges are Swarm Core's own beyond them; a value taken from elsewhere that falls short of a contrast minimum is kept and recorded with its measured ratio, or nudged and listed as a divergence, never nudged silently (mornycorp/swarm-core#100). |
| 2026-11-13 | minor | State on disk: the skeleton's one writer also makes a private file and one that is never written over what is there, so the control socket's token goes through it and no file write is allow-listed; a backend's `Failed` (a deadline, a refusing disk) reaches a caller as `unavailable`, not `internal` (swarm-core#94) |
| 2026-11-13 | minor | Layers: only `backends` touches the outside. The process module and the one atomic file writer live there (the skeleton's `crates/backends`), so a backend can start a program or write a file through the same two modules; the engine keeps what is stored and when, and turns a backend's error into the command's. The layer is always there; a further backend is the app's choice. The `file-writes` scan reads every crate outside the layer (swarm-core#91) |
| 2026-11-13 | minor | One dispatch: MCP runs on a session of its own or attached to a running app's control socket (`{{cli}} mcp --attach <port> [--token-file <path>]`), one MCP adapter over a seam of two calls, with the same answers and the same consent rule (a command whose effects need consent is `consent_required` even on a socket started with `--yes`); The JSON-lines frame: a spec in `commands` carries its effects, and a client counts any effect that isn't `false` as needing consent; the check adds the surface test that compares `mcp --attach` with `mcp` (swarm-core#89) |
| 2026-11-13 | minor | Layers: the optional `sources` layer is `backends`, and it reads and writes the outside (a write there is a command's declared effect; the app's own state stays with the state module). A manifest that still says `sources` is read as `backends` (swarm-core#87) |
| 2026-11-12 | minor | One dispatch's check: the conformance run holds a call cancelled before it runs to fail `cancelled`, on the surfaces an app can cancel a call through (swarm-core#61) |
| 2026-11-12 | minor | One dispatch: a panic in a command's `run` is caught by the dispatch and returned as `internal`, on the wire form and the typed one, and the session goes on (swarm-core#52) |
| 2026-11-12 | major | The contract, from swarm-foundation's: the layers (with the optional `sources`), every action a command (the spec, the id rule, the enabled check, the menu path and shortcut, the error), one dispatch (the JSON-lines frame, the socket's auth, MCP from the registry), the UI as a client (its state as data, what is drawn is a command's result, the theme and the catalog, pictures), and the robustness rules, each naming the check that holds it (swarm-core#47) |
