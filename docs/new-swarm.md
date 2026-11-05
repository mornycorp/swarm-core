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
| The pinned copies: the protocol, `lessons.md`, `progress-docs.md`, the GitHub templates, `scripts/`, the skills | **Swarm Core's**: `sync-assets --check` fails on an edit | step 4, `cargo xtask swarm-core sync-assets` |
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
   `rev` by it), and the workspace has an `xtask` member:

   ```toml
   [workspace]
   members = ["crates/*", "xtask"]

   [workspace.dependencies]
   # The swarm tooling (claims, tracking, the gates, stack and land), pinned by revision.
   swarm-core = { git = "https://github.com/mornycorp/swarm-core.git", rev = "<Swarm Core's main, 40 hex digits>" }
   serde_json = "1"
   ```

2. **`.cargo/config.toml`** runs xtask, and fetches Swarm Core with git's own client (the
   credentials `gh auth setup-git` gave it), as `git clone` would:

   ```toml
   [alias]
   xtask = "run -p xtask --"

   [net]
   git-fetch-with-cli = true
   ```

3. **`xtask/Cargo.toml`**: a binary with `swarm-core = { workspace = true }` and
   `serde_json = { workspace = true }`.
4. **`xtask/src/main.rs`**: copy [`examples/consumer_xtask.rs`](../crates/swarm-core/examples/consumer_xtask.rs)
   and change every value marked `CHANGE`:
   - **The `Project`** (`swarm_core::Project`, each field documented in
     [`project.rs`](../crates/swarm-core/src/project.rs)): its `name` (the machine lock's swarm
     and the state files' prefix), its `env_prefix` (`<PREFIX>_AGENT`, `<PREFIX>_TRACKING`, …),
     its GitHub host, id and path, and its own file names (`checkout_marker`, `gate_lock_file`,
     `tracking_queue`, `stack_state`) so two swarms on one machine never share one. List the docs
     `cargo xtask` generates in `generated`, so `stack` regenerates one that conflicts.
   - **The `Hooks`** (`swarm_core::Hooks`): every method has a default. Start with
     `changed` (the package prefix), `asset_vars` (the placeholders' values) and `layers` (the
     crates' layers, which `cargo xtask layers` enforces: wire it as a command and a `ci` step, as
     the example does); add a
     `gate_kind` planner when docs and tooling changes should skip the smoke, and `extra_causes`,
     `around_ci`, `full_gate_capable` or `full_gate_env` when the gate needs them.
   - **The dispatch**: `swarm_core::serves(command)` then `swarm_core::run(&PROJECT,
     &Ours, &root, command, rest)`, beside the repo's own commands.

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
  `swarm-core init` (step 5).
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

## 5. The starting points

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
- `docs/owner-directions.md`: the owner's first directions (step 8).
- `docs/gaps.md`, `docs/roadmap.md`, `ROADMAP.md`: the first milestone and its gaps, ranked; the
  protocol's workers pick from them ([progress-docs.md](../crates/swarm-core/assets/docs/progress-docs.md)
  says how they are kept).
- `docs/swarm.md`: the machine setup, the capabilities, the gate's kinds, each batch's steps, CI.
- `docs/development.md`: how to build and test; measure **How many builds at once** on the
  owner's machine.
- `docs/scorecard.md`: by hand to start; generate it later.
- `docs/merge-check.md`: the scheduled merge check's prompt (step 8).

Add `target/`, `log/` and `.claude/worktrees/` to `.gitignore`.

## 6. GitHub

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

## 7. CI

CI is local, for the coordinator and every worker (owner, 2026-11-05): the repo has no
`.github/workflows/`, and GitHub runs nothing. Workers gate with `cargo xtask ci --changed`, the
coordinator gates each stack before `land`, and the periodic full gate runs all of `ci` on
`main`; a red one files `main-red`. If the repo has GitHub Actions enabled, turn them off
(Settings › Actions › Disable actions) so a stray workflow file never runs.

## 8. Each machine, then the first agents

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
