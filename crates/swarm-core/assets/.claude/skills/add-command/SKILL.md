---
name: add-command
description: Adds a command to this app the one way every command is added, end to end - the id, typed params and result, registration through the typed adapter, `run`, tests with the conformance run, the generated docs, the checks, and a try from the CLI. Use whenever asked to add, create or expose a command, an action, an MCP tool, or a button, menu item or CLI flag that does something the app doesn't yet do.
---

# Add a command

Every action of the app is a **command**: one entry in one registry, and every surface (the CLI,
`serve`, the loopback socket, MCP, the UI) reaches it through the one dispatch. Adding a command
adds it to every surface, with no edit to any transport. This skill is the same seven steps every
time, so a new command looks like the contract says and not like whichever command sits nearby.
Each step ends in a **check you run**; don't go on until it passes.

Read first, once: the contract's [Every action is a command](../../../docs/app-architecture.md#every-action-is-a-command)
(the interfaces, the id rule, the error codes) and the app's own
[Adding a command](../../../docs/architecture.md#adding-a-command), which names the app's paths
and the commands that regenerate its docs. Where the two differ on a path or a command, the app's
wins; on a rule, the contract's. Where the app's doc leaves a path blank (it is a starting point
with comments to fill in), use the skeleton's, named below. A command that needs an error code, an effect or a requirement
the contract doesn't have is not this skill's job: stop and say so.

## Steps

1. **Name it.** An id `<area>.<name>`: two or more dot-separated segments, each `[a-z][a-z0-9_]*`,
   at most 64 characters (`project.open_recent`). Reuse an existing area where one fits; a one-line
   title and a one-line summary, in the words a command list shows. An id is never renamed once
   released.
   *Check:* `cargo run -q -p {{cli}} -- commands` (shortened below to `{{cli}} commands`; or build
   once and run `target/debug/{{cli}}`) lists the ids taken; yours isn't among them and no `const ID` in
   the source has it (`grep -rn '"<id>"' crates apps`). The registry refuses a bad or a taken id
   when it is built (step 3), which fails every test that builds a session.

2. **Write the types.** A `Params` type (`Deserialize`, `schemars::JsonSchema`, and
   `#[serde(deny_unknown_fields)]`, so a misspelt key is a `bad_params` naming it) and an `Output`
   type (`Serialize`, `schemars::JsonSchema`, `Deserialize` where a test or the UI reads it back).
   A command with no params has an empty `Params` struct with `deny_unknown_fields`, as `app.info`
   does. Put every field a client needs in the output, so no one reads a `serde_json::Value`: a
   `Value` field needs a reason, in a comment, and usually isn't one. Doc comments on the types and
   their fields become the schemas' descriptions; write them. Text read from outside (a file, a
   wire frame, user text) is parsed by a parser in the model layer, which returns a `Result` and
   does no I/O. The schemas are generated from these types, never written by hand.
   Where they live: in the area's crate when the app has one; the skeleton has none, so a module
   of the engine crate beside the registry (`crates/engine/src/commands/<name>.rs`, declared
   `mod <name>;` in `commands.rs`, with the types and the `impl Command` of step 3 in the same
   file).
   *Check:* it compiles (`cargo check -p <crate>`; dead-code warnings for types nothing uses yet
   are expected until step 3), and `grep -n 'Value' <your files>` shows no
   field of that type in a non-test type without its reason (a test may build its params as a
   `Value`).

3. **Register it through the typed adapter.** `impl Command for <YourCommand>` with `ID`, `TITLE`,
   `SUMMARY`, `type Params`, `type Output`, and honest `EFFECTS` and `REQUIRES`:
   - `EFFECTS`: `writes`, `runs_code`, `network`, `device`, each `true` only if the command can do
     it (the default, all false, means it only reads). Any `true` makes a surface ask for consent
     (`--yes` on the CLI); never leave one false to avoid that.
   - `REQUIRES`: what must hold for it to run (`Requirement::Session`, an open project, a device).
     A requirement the app's `Requirement` enum lacks is a new variant and a check in
     `Requirement::check`.
   - `fn enabled(&self, session) -> Result<(), Why>`, only for a condition no other command shares
     (nothing to undo, nothing selected): `Err(Why::new(what doesn't hold, what to do))`. It reads
     memory only (no I/O, no waiting, no change): a UI asks it every frame. A shared condition is a
     requirement instead.
   - `MENU` and `SHORTCUT`, when a person reaches it from a menu or the keyboard: `Some("File/Open
     recent")` (the menus, then the item's label) and `Some("Mod+O")` (`Mod` is Cmd on macOS, Ctrl
     elsewhere), plain text, never a toolkit type. The registry refuses a bad or a taken one.

   Then add `registry.register(<YourCommand>)?;` (qualified by its module, `echo::EchoCommand`,
   when it is in a private module) to **the one place the commands are registered**:
   the skeleton's is `commands::registry()` in `crates/engine/src/commands.rs`, and the app's
   `docs/architecture.md` says if it moved. The `impl Command` lives in the engine crate (it holds
   the trait); the work it does lives in the area's crate, below it. `Registry::register` is the
   only call that builds a `CommandSpec`: never build one by hand.
   *Check:* `cargo test -p <engine crate>` builds a session, so a bad or a duplicate id fails it
   here, naming the id; and `{{cli}} commands` (after `cargo build`) now lists your id with its
   title and summary. A test that expects the registry to hold only the example commands may fail
   here too: that one is fixed in step 5, not worked around.

4. **Write `run`.** `fn run(&self, session, params, cx) -> Result<Output, CommandError>`:
   - a failure is a `CommandError` with a stable code and a **remediation** that says what to do
     (`CommandError::bad_params(message, remediation).with_key("path")` for a param that is wrong
     in a way serde can't see, `not_found`, `too_large`, `unavailable`, `internal`); a caller
     classifies by code, never by message. No secret in a message or a result.
   - no panic: no `unwrap`, `expect`, `panic!`, `todo!`, `unreachable!`, no indexing or slicing
     on input (`.get(..)`); checked arithmetic on input; a cap before an allocation the input
     sizes; bounded recursion.
   - slow work is cancellable: poll `cx.check_cancelled()?` between steps.
   - another program starts **only** through the app's process module, and a file is written
     **only** through its atomic writer. If the app has neither yet, adding one is its own change,
     not part of this command.

   *Check:* `cargo clippy -p <crate> --all-targets -- -D warnings` (the crate roots deny the
   panicking forms) is clean.

5. **Test it**, in the same module (`#[cfg(test)]`, a `Session` and `Caller::Test`) and through the
   testkit:
   - **behaviour:** the result for good params, through `session.call("<id>", json!({..}), &CallOpts::new(Caller::Test))`
     and through `call_typed::<YourCommand>`, and each error code `run` can return, with its
     remediation;
   - **a bad param names its key:** a missing and an unknown param each give `ErrorCode::BadParams`
     with `key` set to the JSON key (`e.key.as_deref() == Some("path")`), and so does a wrongly
     typed one (the dispatch tracks the path of the value serde refused, so `key` is the
     top-level param, also for a value deep inside it; a value `run` rejects itself carries
     `.with_key("<key>")` from `run`);
   - **a disabled state:** the call with an effect and no consent gives `consent_required`
     (add `.consenting()` for the call that should run), and the call with a requirement or an
     `enabled` check that doesn't hold gives `unavailable` naming it, the same error
     `session.states()` gives for the command. A command with no effect, only
     `Requirement::Session` (which always holds) and no `enabled` has neither state: say so in the
     test module's comment instead of leaving it out silently;
   - **the conformance run:** `apps/{{cli}}/tests/surfaces.rs` implements swarm-core's
     `swarm_core_testkit::conformance::App` and calls `conformance::run`: every registered
     command through every surface (the session, the CLI, `serve`, the socket and MCP), holding
     that it has params and result schemas, that MCP's tools match the registry, that an unknown
     command and unknown, missing and mistyped params give `bad_params` / `unknown_command`
     (with `key` naming the param, a mistyped one's too, and the top-level param for a value
     inside one), that a request over the cap is `too_large`, that the `adversarial_params()`
     panic nowhere, and that every surface gives the same result or code. It also **reaches
     `run`**: it builds valid params from the command's params schema (each param in turn with
     its edge values: empty and huge strings, path tricks, unicode, the ends of an integer's
     range, large and deep arrays and maps) and calls them **with consent** (the CLI's `--yes`),
     so `run` must not panic and must answer the same on every surface for each. And it holds
     **consent**: a command with an effect called without consent is `consent_required` on every
     surface, and on MCP even with it (MCP never consents by default). Registering is enough; no
     edit there. A command with required params that none of the generated calls gets to succeed
     (a file that must exist, a name that must be known) adds one known-good call to
     `Ours::examples`, made with consent; without one the run fails with an `[examples]` finding
     that says no call reaches `run`. Since a command with an effect really runs, with hostile paths, its
     session keeps its state under a temp dir. The run is Swarm Core's, not the app's: a
     failure that is the run's own is a swarm-core issue. A test of yours that touches a
     file uses the app's testkit crate's `temp_dir` (`crates/testkit`), never the real config
     dir, a network or an installed tool.

   Don't weaken a test the new command breaks: a test that counts the registry's commands, or
   expects `app.info` to be the only or the first one, is wrong (the skeleton's own tests look the
   example up by id), and gets fixed to what it means (look the command up by id) in the smallest
   edit. That is a test edit, not a transport edit.

   *Check:* `cargo fmt --all` has been run, and `cargo test --workspace` passes, which includes the conformance run
   (`cargo test -p {{cli}} --test surfaces`).

6. **Regenerate the generated docs and schemas, then run the repo's checks.** Run whatever the
   app's `docs/architecture.md › Adding a command` names for the command docs and the schemas,
   and commit their output. (A skeleton app has none: its schemas are generated from the types at
   run time, `{{cli}} commands --json` prints the ones you just made, and nothing to commit is a
   pass.) Commit your work (the `ci` plan is made from the diff against its base), then:
   - `cargo xtask layers`: the layers, the lint headers and the scans (a process start or a file
     write outside its module, a `CommandSpec` built by hand, a `Value` read in a transport). Its
     summary counts the spots a scan allows (`command-spec 1 (allowed 1)` is the registry's own
     build of the spec): a finding is a printed error and a non-zero exit;
   - `cargo xtask ci --changed --base <ref>`, `<ref>` the branch you started from (the
     skeleton's xtask refuses `--changed` without it; the repo's own may default it).

   *Check:* both exit 0. A finding names the file, the rule and the fix: fix the code, never the
   check (see Never).

7. **Try it from the CLI.** `{{cli}} run <id> '<json params>'` prints the result as JSON; a
   command with an effect needs `--yes`. Then a wrong key, `{{cli}} run <id> '{"nope": 1}'`: it
   exits 1 and prints `error [bad_params]`, a `fix:` line and `param: nope`. For an app with MCP,
   read it back: it is in `{{cli}} commands` (and `commands --json`, with its schemas), and a
   `tools/list` request to `{{cli}} mcp` has a tool named by the id with each `.` replaced by `-`.
   A long-running or consent-needing command is also worth a call through `{{cli}} serve`
   (`{"id": 1, "method": "<id>", "params": {..}}` on stdin); a command through the app's loopback
   socket sends `auth` first, with the token from `{{env}}_CONTROL_TOKEN` or the token file the
   app's docs name.

   *Check:* the good call, the bad call and the listing each print what is said above.

Then record it where the app's `docs/architecture.md › Adding a command` says, when it applies
(the area's docs if a person sees the command; that file's revision history if the command changes
how a command flows), and say in the PR which id you added and which checks you ran.

## Never

- **A surface-specific path.** No CLI-only or UI-only action: a button, a menu item, a flag or a
  tool that does something calls a command, and one that has none gets a command first (this
  skill) and then calls it.
- **A transport edit to expose one command.** `crates/automation`, the CLI's `run`/`commands`
  code and the MCP server never name a command; if the new command seems to need an edit there,
  the command is wrong, or the transport has a bug to fix for every command.
- **Loosening a check to pass it.** No new entry in a scan's allow-list, no `#[allow(..)]` of a
  denied lint, no weaker header, no skipped id in the conformance run, no `[patch]` of the
  Swarm Core, to make a finding go away. Fix what it found. An allowance is for a spot that cannot
  be otherwise, and carries a reason and a count: add one only when the owner has said so.
- **A hand-built `CommandSpec`, a hand-written schema, a rename of a released id, or a code that
  isn't in the contract** (the app's own codes are the app's to add, beside the contract's).
- **Edit a pinned copy** (`docs/app-architecture.md`, this skill): change it in swarm-core,
  then bump.
