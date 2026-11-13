# Swarm Core

The swarm tooling that agent swarms share: claims, the tracking timeline, the gates, `stack` and `land`, over GitHub issues and pull requests (through
the `gh` CLI). Each repo used to keep its own copy of these, and the copies drifted apart. This
crate is the single copy.

Each consumer pins it by git revision in its workspace manifest and dispatches the swarm commands
to it from its own `cargo xtask`:

```toml
[workspace.dependencies]
swarm-core = { git = "https://github.com/mornycorp/swarm-core.git", rev = "<sha>" }
```

`cargo xtask swarm-core bump` in a consumer moves the pin to this repo's `main`.

**Renamed from swarm-foundation** (2026-10-30). A consumer pinned to `swarm-foundation` moves in
one PR: the workspace dependency becomes `swarm-core` at this repo's URL (and its `[patch]` in any
user-level cargo config), `swarm_foundation::` becomes `swarm_core::`, `cargo xtask foundation`
becomes `cargo xtask swarm-core` (its gate's `sync-assets --check` among them),
`docs/foundation-assets.md` is deleted (`sync-assets` writes `docs/swarm-core-assets.md`), and
`docs/development.md`'s "The swarm foundation" section becomes "Swarm Core", which `layers`
names. Issue references `f#n` become `sc#n`.

**Moved from GitLab to GitHub** (2026-10-30). Every command now works on a GitHub repository
through `gh`: issues, labels and comments for claims; draft pull requests; `ready` marks a PR
ready for review; `prwatch` (was `mrwatch`) reads each PR's merge state, checks and comments;
`land` merges with a merge commit, rebasing on the server only a PR that is `behind`. A consumer
moves in the same PR as the rename:

- its `Project` takes `github: GitHub { host, repo }` (was `gitlab: GitLab { host, id, path }`)
  and `pr_template` (was `mr_template`, now `.github/pull_request_template.md`);
- `cargo xtask mrwatch` becomes `prwatch`, `claim --no-mr` becomes `--no-pr`, and PRs are `#n`
  (were `!n`) in every argument and line;
- the assets' placeholders are `{{github.host}}` and `{{github.repo}}` (`gitlab.*` are gone); the
  pinned `.gitlab/` templates become `.github/ISSUE_TEMPLATE/` and
  `.github/pull_request_template.md`, and `scripts/gitlab-labels.sh` becomes
  `scripts/github-labels.sh` (its project's own labels in `scripts/github-labels.project.sh`);
- its CI becomes a GitHub Actions workflow (`cargo xtask swarm-core init` writes a starting one),
  fetching this private repository with a read-only `SWARM_CORE_TOKEN` secret, and `main`'s
  protection requires no status checks ([docs/new-swarm.md](docs/new-swarm.md) steps 7 and 8).

The tracking format doesn't change: an event's `mr` field holds the PR's number. Issue numbers
in this repo's docs before the move (`#7`, `sc#15`, …) are the GitLab project's.

**Moved to mornycorp** (2026-10-30): the repository is `mornycorp/swarm-core`. A consumer changes
its workspace dependency's `git` URL to `https://github.com/mornycorp/swarm-core.git` (and the
`[patch."…"]` key in any user-level cargo config), bumps, and runs `sync-assets`; its CI's
`insteadOf` line and `SWARM_CORE_TOKEN` (now a token on `mornycorp`) follow. GitHub redirects the
old URL for now, but don't rely on it.

**No GitHub Actions** (owner, 2026-11-05): CI is local, for the coordinator and every worker.
`init` writes no workflow, `land` cancels no runs, and this repo has none either: a PR here lands
on its local checks (below). A consumer deletes its `.github/workflows/` and turns Actions off.

**Starting a new swarm:** [docs/new-swarm.md](docs/new-swarm.md) walks a new repo through it from
Swarm Core alone: the xtask that wires it in (from
[`examples/consumer_xtask.rs`](crates/swarm-core/examples/consumer_xtask.rs)), its `ci`, the pin
and the pinned copies, the starting points `cargo xtask swarm-core init` writes, GitHub, CI, each
machine, and the first coordinator and workers.

## What it serves

| Phase | Commands | Library modules |
|---|---|---|
| 1 | `capacity`, `event`, `gate-times`, `lock`, `tidy` | `checkout`, `machine_lock`, `test_runs`, `process`, `time` |
| 2 | `claim`, `prwatch`, `ready`, `stack`, `land`, `fullgate` | `needs`, `empty` |
| 3 | `gate` (and `gate --rerun-failed`) | `gate::rerun` |
| 4 | `swarm-core` (`--version`, `bump`, `sync-assets [--check]`) | `changed` (with `changed::lock`, `changed::reads`), `pin`, `assets` |
| new swarms (sc#7) | `swarm-core init` | `templates` (and `examples/consumer_xtask.rs`) |
| ending a session (sc#10) | `finish`, `install-tools` | `finish` |
| the gate daemon (sc#44) | `gate --daemon` (`--once`, `--dry-run`, `--status`, `--stop`, `--resume`): the coordinator's stack, gate and land loop ([docs/gate-daemon.md](docs/gate-daemon.md)) | `gate::daemon` (`policy`, `blame`) |
| the app skeleton (sc#49) | `swarm-core init --app [--ui]` | `skeleton` (and `templates/app/`) |
| architecture | `layers` (the consumer wires it: its own command and a `ci` step) | `layers` (and the `docs/architecture.md` template), `checks` (`checks::layers`, `checks::scan`, `checks::check`) |

A consumer describes itself in a `Project` (a `const`) and implements `Hooks`, whose methods all
have defaults. `Project::proof` says what proves a change before it merges: `Proof::Ready` (each
PR's `ready` runs the whole `ci --changed` and the coordinator smokes) or `Proof::Batch {
ready_env }` (`ready` runs with `ready_env` set, which leaves a class of tests out, and the
coordinator's full gate of the stack runs them once for every PR in it; the protocol's "What
proves a change"). The hooks:

- `gate_kind`: the gate a change needs (docs, tooling or full), from the consumer's own change
  planner. The default is full, unsure, so the coordinator smokes.
- `gate_reruns`: whether a failed gate reruns once by itself: a failure only at the tests, its
  failed tests alone; else a transient failure, the whole gate. Off by default; `gate --rerun` and
  `--no-rerun` choose for one run.
- `verify_main`: whether the gate daemon runs a failed stack's tests alone on `main` before it
  blames a PR, and pauses when they fail there too. Off by default; a project that runs the daemon
  should turn it on, since nothing else here reads `main`.
- `gate_priority`: whether a gate has priority over workers' builds: with `--wait` it goes ahead
  of the running builds after 2 minutes, `capacity` reserves a build for it while it holds the
  machine lock, and `ready` yields to it before each `ci`. Off by default.
- `worker_build_jobs`: the build jobs `ready` gives its builds on a machine where gates run
  (`CARGO_BUILD_JOBS`; `capacity` suggests the number), so a gate keeps memory for its own. None
  by default: no cap.
- `extra_causes`: the consumer's own causes of a failed gate (`gpu`, `python`, say), read
  before the machine's.
- `around_ci`: wraps the gate's run of the consumer's `ci` (its Python, a GPU lease, its stack's
  own xtask, a leftover test process, say).
- `full_gate_capable`, `full_gate_env`: whether this machine runs the periodic full gate, and what
  the project adds to it (a GPU, the path to its test data, say).
- `regenerate`: how `stack` regenerates a generated doc that conflicted (default: the consumer's
  own `cargo xtask <command>`).
- `changed`, `xtask_reads`, `file_class`: what `ci --changed`'s plan needs from the consumer: its
  package prefix, the files its packages read from outside their directory, its runtime edges
  (a package whose tests launch or load another's build), the files xtask's tests read through the root, and any class of its own.
- `asset_vars`: the consumer's values for the assets' placeholders (`product`, `cli`, …). The app
  skeleton's crates also take `changed`'s package prefix as `{{crate_prefix}}` and
  `{{crate_prefix_ident}}` (`my-`, `my_`), or a `crate_prefix` here when that prefix is empty.
- `layers`: the consumer's crates and their layers, its UI crates, and where command replies are
  read, for `cargo xtask layers` (`layers::Table`); none, and `layers` checks nothing.
- `unsafe_crates`: the library packages allowed `unsafe` (a GPU or OS binding), each with why;
  `layers` holds their roots to `#![deny(unsafe_op_in_unsafe_fn)]` and
  `#![warn(clippy::undocumented_unsafe_blocks)]` instead of `#![forbid(unsafe_code)]`. None by
  default. Such a consumer's workspace sets `unsafe_code = "deny"`, not `"forbid"`, since a
  crate can't lift a `forbid`; every other library root still forbids it.
- `architecture`: the layers as each manifest declares them (`[package.metadata.swarm] layer =
  "engine"`), in place of the table of `layers`: the layers' names, the UI toolkits and the lints
  a crate root denies (`checks::Layers::STANDARD` is the usual set). None by default, so no
  manifest is read for a layer.
- `scans`: the pattern scans, with the globs of the files each reads and its allow-list. None by
  default.

**The manifests' checks** (`checks`), which `cargo xtask layers` runs when a project sets
`architecture` or `scans`, beside or in place of the table. They read the repository's files (what
git tracks, and the untracked files it doesn't ignore) and never build.
`checks::layers(root, &Layers)`: every member declares its layer, dependencies point to strictly
lower layers, a UI toolkit is linked only by the UI layer and the apps built on it, the testkit is
only a dev-dependency, `[lints] workspace = true`, the crate roots' headers, and no committed
override of Swarm Core's pin, in any member's manifest or cargo config, read as TOML.
`checks::scan(root, &[Scan])` reads production code (outside `#[cfg(test)]` modules, `tests`
directories and `tests.rs` files, without comments or string literals) for patterns a layer may
not use; each file is held to an allow-list whose every entry gives a reason and a count, a
ratchet (more matches fail, fewer ask to lower the count). The built-in scans
(`Scan::process_command`, `file_writes`, `ui_file_writes`, `ui_process_starts`, `ui_colours`,
`ui_strings`, `value_reads`, `command_spec`, `real_config_dir`) are `const fn`s a project points
at its files with globs. Every finding names the file, the line, the rule and the fix. On this
path `layers` still counts the code-health exemptions, and holds AGENTS.md's crate map to the
manifests (each member's layer, name and `description`) where it has the markers.

**`ci --changed`'s plan** (`changed`): the consumer's `ci` runs the steps; Swarm Core says for
which packages. The changed files map to packages or classes (docs, tooling, data read from
outside a package, everything); the set closes over reverse dependencies; a `Cargo.lock` change
tests what it reaches; a bump of Swarm Core's pin tests what depends on Swarm Core.
`changed::reads::scan` and `changed::missed` back each consumer's test that no read outside a
package falls outside the plan.

## Assets

The files both swarms keep the same live in `crates/swarm-core/assets/`, at the path each takes in
a consumer, and are built into the crate (`assets::ASSETS`):

- `docs/swarm-protocol.md` (the roles, the claim, tracking, the gates, the coordinator's loop, the
  periodic full gate), `docs/app-architecture.md` (the contract every app keeps: the layers, one
  typed command registry behind every surface, the UI as a client, the robustness rules, and the
  check that holds each), `docs/lessons.md`, `docs/progress-docs.md`;
- `.github/` (the issue and PR templates), `scripts/hooks/pre-push`, `scripts/github-labels.sh`;
- `.claude/skills/` (`complexity-analyzer`, `add-command`; the `refactoring` skill was dropped on
  2026-10-31).

`cargo xtask swarm-core sync-assets` writes them into the consumer from its pinned revision, with
`{{name}}` placeholders filled in from its `Project` and `Hooks::asset_vars`, and lists them in
`docs/swarm-core-assets.md`; `--check`, which the consumer's gate runs, fails on a copy that
differs. Change an asset here, then bump each consumer.

**Templates** are the other kind: starting points a new consumer copies once and then owns
(`AGENTS.md` with the layer map and golden rules, its `docs/swarm.md`, `docs/development.md` and
`docs/architecture.md`, `clippy.toml`, the progress docs, the owner's directions, the scheduled merge check's prompt, nextest's `ci` profile). They live in
`crates/swarm-core/templates/` (`templates::TEMPLATES`). `cargo xtask swarm-core init` writes each
one the consumer doesn't have yet, with the same placeholders and `{{today}}`, and never
overwrites. Nothing lists or checks them after ([docs/new-swarm.md](docs/new-swarm.md)).

**The app skeleton** (`skeleton`, `crates/swarm-core/templates/app/`) is a small workspace in
which one example command (`app.info`) already runs through every surface and the checks pass:
`crates/model` (the id rule), `engine` (the session, the typed registry, the one dispatch),
`automation` (JSON lines with the contract's caps, the loopback control socket with token auth, an
MCP server generated from the registry), `testkit` (temp dirs, hostile request lines; the
conformance run is this repo's testkit) and `apps/<cli>` (`run`, `commands`, `enabled`,
`serve [--socket]`, `mcp`), with the lint headers and each crate's layer.
`cargo xtask swarm-core init --app` writes it (and the starting points) where files are missing,
never editing the app's root manifest: it reads it, writes nothing when the workspace dependencies
or lints the crates inherit are missing, and fails after writing until the manifest's `members`
cover the crates. **`init --app --ui`** adds the UI shell, `crates/ui` (an egui shell that lists
the commands, runs one on a worker thread and draws its typed result; the toolkit is the app's
choice), and `apps/<app>`, the window that hosts it. `skeleton::is_app(root)` and
`skeleton::scans_at(root)` read which of these the workspace has, so the example xtask runs the
skeleton's scans and the `conformance` step only in an app and serves a repo with no app unchanged.

The tests that build it (`tests/app_skeleton.rs`: the rendered skeleton's own tests and checks,
and a new repo that follows [docs/new-swarm.md](docs/new-swarm.md) to a green `cargo xtask ci`,
without an app and then with one) need the crates registry and are `#[ignore]`d:
`cargo test -p swarm-core --test app_skeleton -- --ignored`. They build in their own `CARGO_HOME`
(a user-level `[patch]` doesn't reach them) against the locks in `crates/swarm-core/tests/locks/`,
so a crate's new release doesn't change what they build; `SWARM_CORE_REFRESH_LOCKS=1` with the
same command re-resolves and rewrites them. They fail, rather than skip, where the registry can't
be reached.

**`add-command`** is the one skill that is a template: its commands name the app's CLI and
environment prefix, so `{{cli}}` and `{{env}}` take each repo's values, while the other skill is
copied as it is. It walks an agent through the contract's steps for a new command, each ending in
a check, and links to the app's `docs/architecture.md` › Adding a command for the paths. The test
`the_add_command_skill_names_only_what_the_skeleton_has` holds it to the skeleton, so a rename
there fails here first.

**The skills are pinned copies, not a Claude Code plugin**. A copy in the
checkout needs no install on any machine, keeps the skills' names, serves Codex as well as Claude Code (`agents/openai.yaml`), keeps the path
`cargo xtask health` runs the analyzer from, and moves with the commit that pins it, as the rest
of Swarm Core does. A plugin would be installed per machine from a marketplace, update outside
the repos' history, rename the skills under its namespace, and serve Claude Code alone. A
consumer's own files beside the copy (`refactoring/references/project-targets.md`, say) stay its
own.

**The `ci` contract** ([docs/ci-contract.md](docs/ci-contract.md)): the gate runs the consumer's own
`cargo xtask ci`, and this is everything the two agree on. A fake consumer
(`crates/swarm-core/fake/xtask.rs`) tests Swarm Core's side.

**Cross-repository needs:** an issue's `Needs` line may name another repository's issue in full,
`owner/repo#n` (`mornycorp/swarm-core#4`). `claim` notes it while it is open, and `prwatch`
says when it closes or gets a PR. A short name (`ns#381`) is not a need.
