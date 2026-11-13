# AGENTS.md: guide for agents working on Swarm Core

This crate is the swarm tooling every consumer's agent swarm runs on. [README.md](README.md) says
what it serves and how a consumer wires it in; the swarm protocol
(`crates/swarm-core/assets/docs/swarm-protocol.md`) says how a swarm works.

- **The rules are the consumers' rules.** No panics outside tests, no `unsafe`, typed errors,
  hostile input treated as hostile, a deadline on every git and gh call, and the code-health limits
  in `clippy.toml`. Every crate root carries the lint headers in `crates/swarm-core/src/lib.rs`.
- **Every consumer must keep working.** A change to a command's name, flags, output or
  environment variables, or to the tracking format, is a change for every swarm. Behaviour that
  only one project wants goes behind `Project` settings or a `Hooks` method whose default keeps the
  others' behaviour unchanged. The tests run each module against two fixture projects that differ
  in every setting (`project::fixtures`).
- **Landing:** a PR here merges once the checks below pass locally; GitHub runs no workflows
  (CI is local, for the coordinator and every worker). A consumer picks a change up with `cargo xtask swarm-core bump`, and its own gate runs the new code.
- **GitHub, through `gh`:** every call to GitHub goes through `src/github.rs`, which reads REST
  and GraphQL answers into one shape for a PR and one for an issue; the commands' decisions are
  pure functions over those shapes, tested without a network. A change to `github.rs` also runs
  its live read test against GitHub: `cargo test -p swarm-core --lib -- --ignored live`.
- **The app skeleton, built for real:** a change to `src/skeleton.rs`, `templates/app/`,
  `src/checks`, the testkit's conformance run, `examples/consumer_xtask.rs` or `docs/new-swarm.md`
  also runs `cargo test -p swarm-core --test app_skeleton -- --ignored` before it merges (minutes;
  it needs the crates registry and `cargo nextest`, and fails without them). A change to the
  skeleton's dependencies refreshes its locks: `SWARM_CORE_REFRESH_LOCKS=1` with the same command.
- **Iterating from a consumer:** put a `[patch."https://github.com/mornycorp/swarm-core.git"]`
  table pointing at your checkout in your *user-level* cargo config, never in a repo. The
  consumers' `cargo xtask layers` refuses a committed one.
- **Assets are templates for every consumer.** A file under `crates/swarm-core/assets/` lands in
  each consumer's repo (`cargo xtask swarm-core sync-assets`): write it for any of them, naming
  none (the test `no_asset_names_a_consumer`), with `{{name}}`
  placeholders for what differs (`assets::render`), and add a file to `src/assets/list.rs` (the
  test `every_file_under_assets_is_shipped` says when the list and the directory differ). The
  skills under `.claude/skills/` are copied as they are, but `add-command`, a template.
- **The architectural substrate** is the name for the way this crate defines an app's
  architecture: the contract (`assets/docs/app-architecture.md`), the skeleton that implements
  it and the checks that hold it. Call it that in issues, PRs and docs.
- **The explainer** (`docs/explainer/app-architecture.html`) is a page for people: the
  architectural substrate at a glance. Don't read it to learn the rules: the contract and the skeleton are
  the source. A change to the contract's layers or rules, the checks that hold them, the list of
  commands or the skeleton's theme colours updates the page in the same PR, so it never says
  what the code doesn't.
- **Checks:** `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`. `cargo xtask ci` runs them with the doctests and the app skeleton's
  end to end: the coordinator's gate. `cargo xtask ci --changed --base origin/main` runs them
  without those two, as `ready` does.

## The swarm's commands, here

This repo runs the swarm's commands on itself (#96): `xtask/` is this repo's `cargo xtask`, Swarm
Core by path, so each command runs this checkout's code. `cargo xtask` lists them. The swarm
protocol (`crates/swarm-core/assets/docs/swarm-protocol.md`) is how the swarm works, with this
repo's values: `mornycorp/swarm-core`, branches `<n>-<slug>`, `SWARM_CORE_AGENT` and the other
`SWARM_CORE_*` variables, and the `tracking` branch.

- **Start a session:** `cargo xtask event session.start --name <your name> --role worker --model
  <the model you run as> --effort <low|medium|high|xhigh|max>` (`--role coordinator` for the
  coordinator; `--parent <its instance id>` for a session the coordinator started). It prints
  your instance id: pass `--agent <id>` to every command after it, or set `SWARM_CORE_AGENT`.
- **Check capacity before each run that builds:** `cargo xtask capacity` before a `cargo build`,
  `test`, `clippy`, `xtask ci` or `xtask ready`. The machine may be shared with other repos'
  gates. When it suggests 0 more builds, wait a few minutes and check again rather than start
  one; on a machine where gates run, cap your build at its `worker builds:` line
  (`CARGO_BUILD_JOBS=<n>`). `ready` here waits for a test-run slot, not for room to build
  (`Hooks::gate_priority` is off), so check before it too.
- **Claim with the command, never by hand:** `cargo xtask claim <n> --agent <id>`. Never claim
  with a label or a `claimed:` comment of your own: nothing settles two hand claims made at
  once. Here a claim first holds the issue on the `tracking` branch, before it makes anything
  (`Hooks::claim_holds_first`, `claim::hold`): of two claims at once one lands, and the other is
  refused naming the holder. Then it makes the branch, the draft PR, the label, the comment and
  the `work` event. Run it again to finish a claim that failed half way.
- **A hand claim with no draft PR doesn't hold an issue.** An issue with the label or a
  `claimed:` comment but no open draft PR and no live claim on the `tracking` branch is free:
  comment on it saying so, and claim it with `--force`. One with a stale claim's PR to keep:
  `--take-over`.
- **Submit with the command:** squash onto the merge base (the protocol's standing rule), fill in
  the PR's Progress, then `cargo xtask ready <pr> --agent <id>`: it rebases, runs `ci --changed`,
  pushes, marks the PR ready and logs `submit`. Watch your PRs with `cargo xtask prwatch --agent
  <id>`.
- **End the session with `cargo xtask event session.stop --agent <id> --note "<one line>"`.**
  That is the event that ends a session. `cargo xtask event stop` is another event: it stops
  work on an issue and takes `--issue <n> --reason handoff|blocked|paused|done`; it doesn't end
  the session. A worker here ends after `ready`, with its PR still open for the coordinator to
  land, so it logs `session.stop` and leaves its worktree as it is.
- **A worker doesn't run `cargo xtask finish`; it leaves its worktree to the coordinator.**
  `finish` (`crates/swarm-core/src/finish.rs`) is the protocol's one-step end of a session for a
  worker whose PRs have all merged. It refuses while any PR of yours or issue you claimed is open,
  as it is after `ready`. Past that, it installs the xtask in `.git/swarm-tools/`, removes your
  clean worktrees on your merged or closed PRs' branches with their `target/`, deletes your merged
  local branches and logs `session.stop`. It never removes the main checkout, a worktree whose PR
  is open, or the worktree its own executable runs from: `cargo xtask` builds that executable in
  the worktree's `target/`, so the worktree you run it in is kept, and it prints the `git worktree
  remove` for it instead. Run as `.git/swarm-tools/xtask finish` it runs from no worktree, so it
  would remove the one you are in, under you. So a worker never runs `finish` from either place.
  The coordinator removes a worker's worktree once its PR has landed and its session has ended
  (`git -C <main checkout> worktree remove <path>`).
- **The coordinator** runs `prwatch --ready`, `stack`, `gate` (the full `ci`: every check and the
  app skeleton's end to end), `land` and `gate --daemon` here as in a consumer.
- **Not here:** `cargo xtask swarm-core` (`bump`, `sync-assets`, `init`, `--version`) is refused:
  this repo pins no copy of itself. There is no `layers`. No machine runs the periodic full gate
  of `main` by default (`SWARM_CORE_FULLGATE=on` does): the coordinator's gate proves each batch
  before it lands, and a claim here neither runs the gate nor says it will.
