<!-- A starting point from swarm-core's templates (`cargo xtask swarm-core init`): this repo's
     own now. Keep the headings the shared protocol links to: Capabilities, The gate's kinds here,
     Each batch. -->
# Running the swarm

> **Last reviewed:** {{today}} · **Last updated:** {{today}} · **Change:** major (first version, from swarm-core's template) · **Target:** M1

{{product}}'s part of running many agents. The protocol every swarm shares (the roles, starting a
worker, picking and claiming work, tracking, watching PRs, helpers, the coordinator's loop,
staying awake, the periodic full gate) is [swarm-protocol.md](swarm-protocol.md), a pinned copy
from swarm-core. This page holds what is this repo's own.

## Machine setup

Once per machine, before an agent starts there:

1. **Tools:** Rust (the workspace's `rust-version`), [cargo-nextest](https://nexte.st),
   [gh](https://cli.github.com), git; on Windows, Git Bash.
   <!-- and whatever the product needs: targets, SDKs, a GPU driver -->
2. **GitHub:** `gh auth login --hostname {{github.host}}`, then `gh auth setup-git`, so git and
   cargo (`.cargo/config.toml` sets `net.git-fetch-with-cli`) fetch with gh's token. The account
   must read this repository and [swarm-core](https://github.com/mornycorp/swarm-core), a private
   git dependency. Set `GH_REPO={{github.repo}}`, or always pass `--repo {{github.repo}}` to gh.
3. **The clone:** `gh repo clone {{github.repo}}`, and `git fetch origin tracking` so the
   timeline is there to read.
4. **The pre-push hook** (optional): `git config core.hooksPath scripts/hooks`.
5. **`cargo xtask ci` passes** on `main` before any agent starts.

## Capabilities

What a machine can run. An issue that needs one carries its label; a worker skips the labels its
machine lacks (the protocol's [Picking work](swarm-protocol.md#picking-work)).

| Label | Needs | Typical tasks |
|---|---|---|
| (none) | the machine setup above | most tasks |
| `full-gate` | a machine capability, not a label: `claim` runs the periodic full gate there when it is due. The default: a Windows machine with room for a build (`{{env}}_FULLGATE=on\|off` overrides it) | the periodic full gate of `main` |
| `needs::human` | a person | credentials, decisions, anything an agent can't do |

<!-- Add the project's own: a GPU, an SDK, a second OS, the reference data. -->

## The gate's kinds here

`cargo xtask gate` picks one of the protocol's kinds from the change
([The coordinator's loop](swarm-protocol.md#the-coordinators-loop) step 3):

- **docs:** only Markdown.
- **tooling:** what this repo's planner counts as tooling (`Hooks::gate_kind` in `xtask/`): until
  it has one, nothing is, and every code change is smoked.
- **smoke:** anything else. This repo's `cargo xtask ci --smoke` runs: fmt, clippy, the build,
  <!-- layers, the generated docs' checks, the golden tests (a step named golden), a short
  headless run of the CLI -->.
- **full:** the whole `cargo xtask ci`, the periodic full gate's.

The shared files that conflict most in a stack are AGENTS.md §6's.

## Each batch

In the coordinator's progress commit ([The coordinator's loop](swarm-protocol.md#the-coordinators-loop)
step 4), after folding the batch into `docs/gaps.md`:

- refresh the scorecard (`docs/scorecard.md`: by hand, or `cargo xtask scorecard` once this repo
  generates it);
- <!-- the project's own measurements -->

## What CI runs

<!-- One line a job: what it runs and about how long it takes. -->

`.github/workflows/ci.yml` runs `cargo xtask ci`. The rules it keeps:

- No run on draft PRs or on the `tracking` branch; one when a PR is marked ready, on each push to
  a ready PR, and on `main`. A new run of a ref cancels the one still going.
- **Fetching Swarm Core:** it is a private repository, so the job points its URL at a read-only
  token before cargo runs: a fine-grained token with **Contents: read** on
  `mornycorp/swarm-core`, stored as this repository's `SWARM_CORE_TOKEN` secret (its owner makes
  it, once).
- **Branch protection on `main`:** no required status checks, since the coordinator's local gate
  is the check before a merge, and `land` cancels a PR's runs when it lands it (a required check
  would block the merge). Require PRs, and allow merge commits.
- Nothing merges while `main`'s CI is red.

## Revision history

| Date | Change | Summary |
|---|---|---|
| {{today}} | major | First version, from swarm-core's template |
