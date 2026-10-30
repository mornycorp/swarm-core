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
  protection requires no status checks ([docs/new-swarm.md](docs/new-swarm.md) steps 6 and 7).

The tracking format doesn't change: an event's `mr` field holds the PR's number. Issue numbers
in this repo's docs before the move (`#7`, `sc#15`, …) are the GitLab project's.

**Moved to mornycorp** (2026-10-30): the repository is `mornycorp/swarm-core`. A consumer changes
its workspace dependency's `git` URL to `https://github.com/mornycorp/swarm-core.git` (and the
`[patch."…"]` key in any user-level cargo config), bumps, and runs `sync-assets`; its CI's
`insteadOf` line and `SWARM_CORE_TOKEN` (now a token on `mornycorp`) follow. GitHub redirects the
old URL for now, but don't rely on it.

**Starting a new swarm:** [docs/new-swarm.md](docs/new-swarm.md) walks a new repo through it from
Swarm Core alone: the xtask that wires it in (from
[`examples/consumer_xtask.rs`](crates/swarm-core/examples/consumer_xtask.rs)), its `ci`, the pin
and the pinned copies, the starting points `cargo xtask swarm-core init` writes, GitHub, CI, each
machine, and the first coordinator and workers.

## What it serves

| Phase | Commands | Library modules |
|---|---|---|
| 1 | `capacity`, `event`, `gate-times`, `lock`, `tidy` | `checkout`, `machine_lock`, `process`, `time` |
| 2 | `claim`, `prwatch`, `ready`, `stack`, `land`, `fullgate` | `needs`, `empty` |
| 3 | `gate` (and `gate --rerun-failed`) | `gate::rerun` |
| 4 | `swarm-core` (`--version`, `bump`, `sync-assets [--check]`) | `changed` (with `changed::lock`, `changed::reads`), `pin`, `assets` |
| new swarms (sc#7) | `swarm-core init` | `templates` (and `examples/consumer_xtask.rs`) |
| ending a session (sc#10) | `finish`, `install-tools` | `finish` |

A consumer describes itself in a `Project` (a `const`) and implements `Hooks`, whose methods all
have defaults:

- `gate_kind`: the gate a change needs (docs, tooling or full), from the consumer's own change
  planner. The default is full, unsure, so the coordinator smokes.
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
- `asset_vars`: the consumer's values for the assets' placeholders (`product`, `cli`, …).

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
  periodic full gate), `docs/lessons.md`, `docs/progress-docs.md`;
- `.github/` (the issue and PR templates), `scripts/hooks/pre-push`, `scripts/github-labels.sh`;
- `.claude/skills/` (`refactoring`, `complexity-analyzer`).

`cargo xtask swarm-core sync-assets` writes them into the consumer from its pinned revision, with
`{{name}}` placeholders filled in from its `Project` and `Hooks::asset_vars`, and lists them in
`docs/swarm-core-assets.md`; `--check`, which the consumer's gate runs, fails on a copy that
differs. Change an asset here, then bump each consumer.

**Templates** are the other kind: starting points a new consumer copies once and then owns
(`AGENTS.md`, its `docs/swarm.md` and `docs/development.md`, the progress docs, the owner's
directions, the scheduled merge check's prompt, nextest's `ci` profile, the GitHub Actions
workflow). They live in
`crates/swarm-core/templates/` (`templates::TEMPLATES`). `cargo xtask swarm-core init` writes each
one the consumer doesn't have yet, with the same placeholders and `{{today}}`, and never
overwrites. Nothing lists or checks them after ([docs/new-swarm.md](docs/new-swarm.md)).

**The skills are pinned copies, not a Claude Code plugin**. A copy in the
checkout needs no install on any machine, keeps the skills' names (`refactoring`, as each AGENTS.md
§5 calls it), serves Codex as well as Claude Code (`agents/openai.yaml`), keeps the path
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
