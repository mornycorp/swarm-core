# Starting a new swarm

> **Last updated:** 2026-11-05 · **Change:** minor (no GitHub Actions: CI is local)

How a new repo starts an agent swarm from Swarm Core alone: the xtask that wires it in, the
`ci` it needs, the pin and the shared files, GitHub, each machine, and the first coordinator and
workers.

What the swarm then runs on is [the swarm protocol](../crates/swarm-core/assets/docs/swarm-protocol.md),
which the repo pins as `docs/swarm-protocol.md`: read it before starting the first agent.

## What the repo will hold

| What | Whose | How it gets there |
|---|---|---|
| `xtask/`: the `Project`, the `Hooks`, the dispatch, `ci` | the repo's | step 2, from [`examples/consumer_xtask.rs`](../crates/swarm-core/examples/consumer_xtask.rs) |
| The pinned copies: the protocol, `app-architecture.md`, `lessons.md`, `progress-docs.md`, the GitHub templates, `scripts/`, the skills | **Swarm Core's**: `sync-assets --check` fails on an edit | step 4, `cargo xtask swarm-core sync-assets` |
| The starting points: `AGENTS.md`, `docs/swarm.md`, `docs/development.md`, the progress docs, the owner's directions, the merge check's prompt, nextest's `ci` profile | **the repo's**, once copied: edit freely, nothing checks them | step 5, `cargo xtask swarm-core init` |

Why the two kinds: the pinned copies are the process every swarm shares, so a change to them is a
Swarm Core PR and a bump in each repo. The starting points describe the repo itself (its crates,
its machines, its milestones), so they diverge from the first edit. They are copied by a command
rather than by hand because they take the repo's values (its name, GitHub repository and environment
prefix) in the same `{{name}}` placeholders as the pinned copies. `init` writes only what is
missing, so it never overwrites the repo's own.

## 1. Before anything: access

- **A GitHub repository** for the repo, `owner/name`.
- **Read access to Swarm Core** for every machine and person that builds: it is a private
  repository, and cargo fetches it from `https://github.com/mornycorp/swarm-core.git`. Ask
  Swarm Core's owner to add the account as a collaborator on `mornycorp/swarm-core`.
- **gh**, authenticated, and git using its token: `gh auth login`, then `gh auth setup-git`.

## 2. The xtask

1. **The workspace manifest** pins Swarm Core by its whole commit id (cargo fetches a git
   `rev` by it), and the workspace has an `xtask` member. It also holds what the app skeleton's
   crates inherit (step 5): `serde`, `serde_json`, `serde_path_to_error` and `schemars`, the workspace lints
   (`[lints] workspace = true` in every member), and Swarm Core's testkit
   (`swarm-core-testkit`: the conformance run, pinned **at the same `rev`** as Swarm Core;
   `cargo xtask swarm-core bump` moves both). A repo that isn't an app can leave those out. Its
   `members` name only `xtask` for now: cargo refuses a member glob that matches nothing, so the
   crates' globs are added in step 5, when the crates exist.

   ```toml
   [workspace]
   resolver = "3"
   members = ["xtask"]

   [workspace.dependencies]
   # The swarm tooling (claims, tracking, the gates, stack and land), pinned by revision.
   swarm-core = { git = "https://github.com/mornycorp/swarm-core.git", rev = "<Swarm Core's main, 40 hex digits>" }
   # The app skeleton's conformance run (step 5), a dev-dependency of the app's crates: the same rev.
   swarm-core-testkit = { git = "https://github.com/mornycorp/swarm-core.git", rev = "<the same rev>" }
   serde = "1"
   serde_json = "1"
   # The app skeleton's bad_params: the path of a wrong-typed value names its key (step 5).
   serde_path_to_error = "0.1"
   # The app skeleton's command schemas, generated from the types (step 5).
   schemars = "1"

   [workspace.lints.rust]
   unsafe_code = "forbid"

   [workspace.lints.clippy]
   dbg_macro = "warn"
   todo = "warn"
   ```

2. **`.cargo/config.toml`** runs xtask, and fetches Swarm Core with git's own ssh (keys,
   agent, known hosts), as `git clone` would:

   ```toml
   [alias]
   xtask = "run -p xtask --"

   [net]
   git-fetch-with-cli = true
   ```

3. **`xtask/Cargo.toml`**: a binary with `swarm-core = { workspace = true }` and
   `serde_json = { workspace = true }`. It is the repo's tooling layer: it declares that, and takes
   the workspace's lints, as every member does (`cargo xtask layers` checks both):

   ```toml
   [package]
   name = "xtask"
   version = "0.1.0"
   edition = "2024"
   publish = false

   [package.metadata.swarm]
   layer = "tooling"

   [dependencies]
   swarm-core = { workspace = true }
   serde_json = { workspace = true }

   [lints]
   workspace = true
   ```

4. **`xtask/src/main.rs`**: copy [`examples/consumer_xtask.rs`](../crates/swarm-core/examples/consumer_xtask.rs)
   and change every value marked `CHANGE`:
   - **The `Project`** (`swarm_core::Project`, each field documented in
     [`project.rs`](../crates/swarm-core/src/project.rs)): its `name` (the machine lock's swarm
     and the state files' prefix), its `env_prefix` (`<PREFIX>_AGENT`, `<PREFIX>_TRACKING`, …),
     its GitHub host and repository, and its own file names (`checkout_marker`, `gate_lock_file`,
     `tracking_queue`, `stack_state`) so two swarms on one machine never share one. List the docs
     `cargo xtask` generates in `generated`, so `stack` regenerates one that conflicts.
   - **The `Hooks`** (`swarm_core::Hooks`): every method has a default. Start with
     `changed` (the package prefix, which is also the prefix of the app skeleton's crates) and
     `asset_vars` (the placeholders' values: `product` and `cli` name the skeleton's directories
     and packages, so set them before step 5); `architecture` says the layers each crate's
     manifest declares are held to the contract's order (`cargo xtask layers`, a command and a
     `ci` step, as the example has them; a repo that keeps a table in its xtask sets `layers`
     instead); `scans` holds the architecture's scans
     (`skeleton::scans_at`: the skeleton's, once the repo has it). Add a `gate_kind` planner when docs and
     tooling changes should skip the smoke, and `extra_causes`, `around_ci`, `full_gate_capable` or
     `full_gate_env` when the gate needs them.
   - **The dispatch**: `swarm_core::serves(command)` then `swarm_core::run(&PROJECT,
     &Ours, &root, command, rest)`, beside the repo's own commands.
   - **App or not** is nothing to set: the example asks the workspace (`skeleton::is_app`: the
     skeleton's `crates/engine` is there). In a repo with no app it leaves out the skeleton's scans
     and `ci`'s `conformance` step, which name files and a package it doesn't have; once step 5 has
     written the skeleton they run, and the UI crate's scans once `--ui` has written it. A repo
     whose app isn't laid out as the skeleton is replaces those two calls with its own answer.

5. **`rustfmt.toml`**: the style the example is formatted in, which is the app skeleton's (step 5
   keeps this one). `ci`'s `fmt` step checks it, and clippy's line limits (`clippy.toml`, step 6)
   are counted in it:

   ```toml
   max_width = 160
   use_small_heuristics = "Max"
   ```

## 3. The `ci` command

Swarm Core's gate never builds the repo itself: it runs the repo's own `cargo xtask ci` and
reads what it prints. [docs/ci-contract.md](ci-contract.md) is everything the two agree on: the
forms it is called with (`--changed --base <ref>`, `--smoke`, none, the rerun's `--test-alone`,
`--doctests` and `--only`), the `CI summary` it ends with, and nextest's lines and junit file.

- The example's `ci` keeps the contract with fmt, clippy, nextest, `layers` and the pinned copies' check, and
  narrows `--changed` with Swarm Core's plan (`swarm_core::changed`). Grow it with the
  repo's own steps (the smoke's golden tests and headless run, generated docs' checks).
- `ci` records its checkout in the target dir (`swarm_core::checkout::claim`), so two
  checkouts never share one.
- nextest's `ci` profile writes the junit file the gate keeps: `.config/nextest.toml`, from
  `swarm-core init` (step 6).
- `crates/swarm-core/fake/xtask.rs` is a fake consumer that accepts exactly the contract's forms:
  Swarm Core's tests run the gate against it.

## 4. The pin and the pinned copies

```sh
cargo xtask swarm-core --version       # the pinned version and commit
cargo xtask swarm-core sync-assets     # write the pinned copies; docs/swarm-core-assets.md lists them
```

Commit them as they are. `ci` runs `cargo xtask swarm-core sync-assets --check`, which fails on a
copy that differs. Later, `cargo xtask swarm-core bump` moves the pin to Swarm Core's `main`;
`sync-assets` then refreshes the copies. A change to a pinned copy is a Swarm Core PR, never an
edit in the repo.

## 5. The app's skeleton

*Do this step when the product is an app: something with commands a person, an agent or a test
can run. Skip it (go to step 6) for a repo that isn't, such as a library or a tooling repo.*

An app starts from code, not prose: `swarm-core init --app` writes a small workspace in which one
example command already runs through every surface and the checks pass. It implements the
[app architecture contract](../crates/swarm-core/assets/docs/app-architecture.md)'s minimum
interfaces as written there (the command trait, the registry, the id rule, the dispatch, the
error, the JSON-lines frame). Follow these in order, alone:

1. **Set the names first.** They become directory names, package names and Rust paths, so a
   later change is a rename across the workspace. In the xtask (step 2):
   - `cli` in `ASSET_VARS`: the CLI binary's name. It becomes `apps/<cli>` and the package of the
     same name; give it the crates' prefix (`my-cli` beside `my-engine`).
   - `package_prefix` in `Hooks::changed`: the prefix of the crates' package names (`my-`, with
     the dash: `my-model`, `my-engine`, `my-automation`, `my-testkit`). The skeleton's placeholders
     are `{{cli}}`, `{{crate_prefix}}` and `{{crate_prefix_ident}}` (`my_`, for Rust paths).
   - `product` in `ASSET_VARS`: what the app is called, one line without quotes.
2. **Check the root manifest** has what the crates inherit: `serde`, `serde_json`, `serde_path_to_error` and `schemars`
   in `[workspace.dependencies]`, Swarm Core's testkit there too (`swarm-core-testkit`,
   at the same `rev` as `swarm-core`), and `[workspace.lints]` (step 2's manifest has them). `init --app`
   never edits the manifest. When one is missing it writes **nothing** and lists the lines to
   add: add them, and run it again.
3. **Write it:**

   ```sh
   cargo xtask swarm-core init --app      # the skeleton, and the starting points (step 6)
   ```

   It writes only where a file is missing, as `init` does, so it is safe to run again:
   `crates/model` (the id rule: one parser of hostile text, with a `Result` and tests),
   `crates/engine` (the session, the typed command registry, the one dispatch with typed errors, and
   the example command `app.info`), `crates/automation` (JSON lines with the contract's caps, `serve`
   on stdio, the loopback control socket, an MCP server generated from the registry), `apps/<cli>`
   (`run <command> [json]`, `commands`, `serve [--socket]`, `mcp`), `crates/testkit` (temp dirs, hostile request lines), `apps/<cli>/tests/surfaces.rs` (the
   conformance run over every surface, below), and `clippy.toml` and `rustfmt.toml`. Every crate root carries the lint
   headers and every manifest `[package.metadata.swarm] layer = "<name>"` and `[lints] workspace =
   true`.

   **It then fails on purpose** when the manifest's `members` don't cover the new crates, which is
   the case now: the files are written, and it asks you to add them. They can only be added after
   the crates exist (cargo refuses a member glob that matches nothing, and wouldn't run the xtask
   that writes them), and `ci` would pass without building a crate that isn't a member. Add the
   globs, keeping the repo's own members:

   ```toml
   members = ["crates/*", "apps/*", "xtask"]
   ```

   then write the pinned copies (step 4), which also puts the new crates in `Cargo.lock`, and
   the crate map in `AGENTS.md`, which `layers` holds to the crates as they are:

   ```sh
   cargo xtask swarm-core sync-assets
   cargo xtask layers --write
   ```

4. **Make it green before anything else:**

   ```sh
   cargo fmt --all                        # only if your own rustfmt.toml isn't the one it wrote
   cargo xtask ci
   ```

   **If `cargo xtask ci` isn't green, stop.** Don't write a command, edit the skeleton or the
   xtask to get past it: the guide, the skeleton or this machine is at fault, not your code.
   Report the failing step and its output (as an issue in swarm-core), and wait.
5. **The example command stays.** `app.info` (name, version, schema version) is the proof that
   every surface works: run it (`cargo run -p <cli> -- run app.info`), list it (`commands`), call
   it over `serve` and `mcp`. Keep it until the first real command replaces it, in the PR that
   adds that command; the tests that name it (`apps/<cli>/tests/surfaces.rs`, the transports'
   tests) go with it. A real command is a type that implements `Command` and one line in
   `crates/engine/src/commands.rs`: no transport changes.

**The conformance run** is Swarm Core's, not a copy in the app: `swarm_core_testkit::conformance`
(a `[dev-dependencies]` entry of `apps/<cli>` and the engine, from the testkit pin of step 2) takes
a small trait the app implements, `App` (`commands()` with their schemas and whether their effects
need consent, `mcp_tools()`, and `call(Call { surface, id, params, consent })` for the session, the
CLI, `serve`, the socket and MCP), and `conformance::run` drives every command through every
surface: every command has params and result schemas, MCP's tools match the registry, an unknown
command and unknown, missing or mistyped params give `unknown_command` / `bad_params`, a request
over the cap is `too_large`, a pass of hostile params panics nowhere, valid params built from each
command's schema reach its `run` (with consent) and panic nowhere either, a command with an effect
is `consent_required` without consent on every surface and on MCP even with it (MCP never consents
by default: `App::consents`), and every surface gives the same result or the same failure code. A
command with required params that no generated call gets to succeed needs a known-good call in
`App::examples`.
`apps/<cli>/tests/surfaces.rs` is the app's part (how to make one call on each surface); a failure
names the rule, the command and the surface. `cargo xtask ci` runs it with the rest of the tests, so
a command that breaks a surface fails the gate; the app gets the run's improvements by
`cargo xtask swarm-core bump`. `key` names the wrong param for every `bad_params`, a wrong-typed
value included (the skeleton's params check tracks the path of the value serde refused,
`serde_path_to_error`), and the run requires it. An app made from an older skeleton, whose check
named the key only for a missing, unknown or duplicate field, takes the new check from
`engine/src/registry.rs` of the skeleton, or opts out for the time being with
`App::names_key_for_wrong_type` returning `false`.

**The control socket** is in every app, `--ui` or not: `<cli> serve --socket` serves the JSON-lines
frame on `127.0.0.1` (never another address) so an agent or a test drives the running app as the
CLI does. It starts with an `auth` message carrying a token, sent within 3 s of connecting;
anything else first is answered `unauthenticated` and the connection closes. The token is
`<ENV>_CONTROL_TOKEN` (your project's `env_prefix`), or a file named with `--token-file <path>`, in
a directory only you can read. On Unix the file is made `0600` when it doesn't exist, and an
existing one is used only when it is a regular file (not a link, a FIFO or a device), yours, and
`0600`. On Windows `std` can't read or set a file's owner or ACL, so an existing file can't be told
from a planted one: each start writes a new token over the path (a client reads it after the
start-up line), and the file gets its directory's permissions, so keep it under your profile, or
use the variable. The variable is not removed from the app's environment (`remove_var` is
`unsafe` in edition 2024), so the process module (`crates/engine/src/process.rs`, the one place
the app starts another program) starts every child without it. There is
no default location, so one is always chosen. The caps are the contract's (a request over 1 MiB is
answered `too_large`, a reply over 8 MiB too; 16 authenticated connections, and a seventeenth is
told `unavailable` and closed; 30 s idle); a connection that hasn't authenticated yet takes none
of those 16 (at most 8 wait, and a ninth closes the oldest), so peers without the token can't lock
a client out. A refusal's reply survives the close: the connection drains what the peer sent
behind it for a moment first. `serve --socket` prints one JSON line when it listens
(`{"listening": "127.0.0.1:<port>", "token": "file <path>"}`: where the token is, never what it is)
and runs until its stdin closes, so whoever started it stops it by closing the pipe. A client that
disconnects while its call runs cancels it, as the contract says: the call's token is set, a slow
command that polls it returns `cancelled` (to nobody) and the connection is freed at once. A
request already sent behind the running one is read, not lost; a client that wants its replies
keeps its sending side open until it has them (a half-close counts as going away), and once the
call is cancelled (the client went, or the server stopped) the requests still queued behind it
don't run.

**`--app` and `--ui`.** Pass `--app` for a headless-first app, which is every app: its commands
come before any window, and an app that has none can leave `--ui` out for good. **Pass `--ui` when
the product has a window** (`cargo xtask swarm-core init --app --ui`, in the same step 3, or later
on a headless app: it writes only what is missing, so it adds the UI's files and touches nothing
else):

- `crates/ui` (layer `ui`): an egui shell that lists the registry's commands, runs one on a
  **worker thread** and draws its **typed** result (`call_typed`), through the same dispatch as
  `<cli> run`. It never writes a file, starts a program, blocks on a call or reads a
  `serde_json::Value`. The crate says in its docs that **the toolkit is the app's choice**: the
  contract picks none, the template uses egui because both reference apps do, and swapping it
  replaces the crate's drawing code, not the engine or a transport.
- `apps/<app>` (layer `apps`): the window, which hosts the shell in eframe, and with `--socket`
  serves the control socket on the same session. Only these two link a toolkit; the CLI and every
  layer below never do, and `layers` fails if one does.
- Before `--ui` set `app` in `ASSET_VARS`: the window's binary name, a lowercase word of its own
  (not `cli`'s), which becomes `apps/<app>` and its package; set `ui_crate` to the UI crate's
  package (`<prefix>ui`) too, as the pull request template's snapshot line names it. The example
  xtask's `Hooks::scans` (`skeleton::scans_at`) adds the UI crate's scans (no file writes, no process
  starts, no untyped reads) once `crates/ui` is there: they name its files, so they belong to an
  app that has it. `--ui` alone is refused, as the shell is part of the skeleton.
- A command appears in the UI's list with no edit; to run it from the window, give it a **view**:
  a function in `crates/ui/src/views.rs` that makes its typed call and fills the lines the shell
  draws, and a line in `views()`. A command with no view is listed, greyed, and says so.
- It brings `eframe` and `egui` (the versions in the two manifests build together; the first build
  takes a few minutes) and, on Linux, needs the windowing libraries a desktop has to run (not to
  build).

The skeleton is **the app's own** once written: edit it freely. `cargo xtask layers` keeps it to
the contract (the layers, the headers, the scans).

## 6. The starting points

```sh
cargo xtask swarm-core init            # write each one the repo doesn't have yet
```

Then make them the repo's own, before the first agent starts:

- `AGENTS.md`: what the product is, its golden rules, its shared files (§6); `cargo xtask layers
  --write` fills in its crate map. Keep §4 and §5: the protocol links to them.
- `docs/architecture.md`: the layers and what each means for this product, how a command flows,
  typed contracts, the UI and automation (the CLI, the control channel, MCP). **Decide the layers
  before the first issue**: every swarm on Swarm Core builds an engine of typed commands that every
  front end drives, and retrofitting it costs far more ([lessons.md](../crates/swarm-core/assets/docs/lessons.md) §1).
- `docs/owner-directions.md`: the owner's first directions (step 9).
- `docs/gaps.md`, `docs/roadmap.md`, `ROADMAP.md`: the first milestone and its gaps, ranked; the
  protocol's workers pick from them ([progress-docs.md](../crates/swarm-core/assets/docs/progress-docs.md)
  says how they are kept).
- `docs/swarm.md`: the machine setup, the capabilities, the gate's kinds, each batch's steps, CI.
- `docs/development.md`: how to build and test; measure **How many builds at once** on the
  owner's machine.
- `docs/scorecard.md`: by hand to start; generate it later.
- `docs/merge-check.md`: the scheduled merge check's prompt (step 9).

Add `target/`, `log/` and `.claude/worktrees/` to `.gitignore`.

## 7. GitHub

- **Labels:** `scripts/github-labels.sh` (a pinned copy) creates the labels the protocol uses
  (`status::in-progress`, `status::blocked`, `main-red`, `needs::human`, `sync`, …); run it once,
  after `gh auth login`. The repo's own labels (its capabilities, its areas) go in
  `scripts/github-labels.project.sh`, which it runs after.
- **The `tracking` branch** needs nothing: the first `cargo xtask event` makes it, with its
  README. A timeline viewer reads it to show the swarm.
- **The issue and PR templates** are pinned copies (`.github/ISSUE_TEMPLATE/`,
  `.github/pull_request_template.md`): `claim` opens each draft PR from the PR template.
- **Protect `main`** (a ruleset or branch protection): changes only through PRs, merge commits
  allowed, and **no required status checks**: the coordinator's local gate is the check before a
  merge, and GitHub runs nothing to report one, so a required check would block every merge. Only
  the coordinator merges.
- **Draft PRs** need a plan that has them for the repo's visibility (a private repository needs
  GitHub Pro, Team or Enterprise): `claim` opens every PR as a draft.

## 8. CI

CI is local, for the coordinator and every worker (owner, 2026-11-05): the repo has no
`.github/workflows/`, and GitHub runs nothing. Workers gate with `cargo xtask ci --changed`, the
coordinator gates each stack before `land`, and the periodic full gate runs all of `ci` on
`main`; a red one files `main-red`. If the repo has GitHub Actions enabled, turn them off
(Settings › Actions › Disable actions) so a stray workflow file never runs.

## 9. Each machine, then the first agents

**Each machine** follows the repo's `docs/swarm.md › Machine setup`: the tools, `gh auth login`
and `gh auth setup-git` for an account that reads the repo and Swarm Core, the clone, and
`cargo xtask ci` passing on `main`.

**The owner decides**, and records in `docs/owner-directions.md`:

- the product and its first milestone, and which gaps come first;
- how many workers run at once (the protocol's default: at most three per swarm on a shared
  desktop, and only while `cargo xtask capacity` shows room);
- which machines run the periodic full gate (`full_gate_capable`, `<PREFIX>_FULLGATE`);
- the models and efforts the coordinator and the workers run at;
- what needs the owner's approval (plans before they become issues, `needs::human`).

**The coordinator** is one agent session in a worktree of its own (`<the clone>-coord`, its own
target dir), never the owner's checkout. Its brief: read `AGENTS.md`, `docs/swarm-protocol.md`
and `docs/swarm.md`; log `cargo xtask event session.start --name coordinator --role coordinator`;
turn the first milestone's gaps into small, independent issues with capability labels; run
[the coordinator's loop](../crates/swarm-core/assets/docs/swarm-protocol.md#the-coordinators-loop)
and keep the ready-PR watch armed ([Staying awake](../crates/swarm-core/assets/docs/swarm-protocol.md#staying-awake)).

**The scheduled merge check**: fill in `docs/merge-check.md`'s prompt (the owner's checkout, the
coordinator's worktree, its instance id) and add it as a scheduled task every 30 minutes, so
ready PRs merge while no coordinator is awake.

**The workers**: each a fresh session in its own worktree, given the protocol's
[worker instruction](../crates/swarm-core/assets/docs/swarm-protocol.md#starting-a-worker) with
its name, its machine's capabilities and the coordinator's instance id. They pick, claim, work,
gate and submit; the coordinator stacks, gates and lands.

Then point a timeline viewer at the repo's `tracking` branch to watch it.
