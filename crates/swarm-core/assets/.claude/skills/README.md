<!-- A pinned copy from swarm-core (crates/swarm-core/assets/.claude/skills/README.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`. -->
# Project skills

Skills that every agent working in this repo can use. Claude Code finds them here, in
`.claude/skills/<name>/SKILL.md`; Codex finds the same files through `agents/openai.yaml`. The rest
of `/.claude/` (the agents' worktrees in `.claude/worktrees/`, local settings) is local and
ignored by git.

| Skill | Use |
|---|---|
| [add-command](add-command/SKILL.md) | **Adding a command to the app, the one way every command is added:** name it by the id rule, write the typed params and result, register it through the typed adapter with honest effects, write `run`, test it (a bad param naming its key, a disabled state, the conformance run), regenerate the docs, run `cargo xtask layers` and `cargo xtask ci --changed`, try it from `{{cli}} run`. Each step ends in a check. It says what it never does: a surface-specific action, a transport edit to expose one command, a loosened allow-list. The app's own paths are in [docs/architecture.md](../../docs/architecture.md#adding-a-command). Used implicitly: it is the answer to "add a command that ...". |
| [complexity-analyzer](complexity-analyzer/SKILL.md) | Ranks complex Rust, Python and JS/TS functions, and reports design signals. Optional, run when asked; its thresholds are not targets to meet. It needs `uv` and Python 3.11 or later; its tools install to `%LOCALAPPDATA%\refac\tools\complexity-analyzer`, outside the repo. |

`complexity-analyzer` runs only when invoked by name. `add-command` is the other way: it runs
whenever a task adds a command (AGENTS.md §5 names it), and it is the only skill with placeholders
(this README has them too): the app's CLI, `{{cli}}`, and its environment prefix, `{{env}}`, so
each repo's copy carries its own commands. The `refactoring` skill that shipped beside them, and
the refactoring pass at the end of each ticket, were dropped on 2026-10-31 (the owner's direction).

## Where they come from

**A pinned copy.** The skills live in [swarm-core](https://github.com/mornycorp/swarm-core)
(`crates/swarm-core/assets/.claude/skills/`), and `cargo xtask swarm-core sync-assets` writes them
here from Swarm Core's pinned revision. The gate's `sync-assets --check` fails when a file here
differs from it: change a skill in Swarm Core, then bump. A repo may keep files of its own
beside the copy that Swarm Core doesn't ship.

It was first copied on 2026-10-20 from `D:/dev/refac/skills` (commit 9a53a7f), into the swarms that
share it. That folder extracts it from [fcarucci/Swarm](https://github.com/fcarucci/Swarm) by
Francesco Carucci, under the Apache License 2.0: see [LICENSE](LICENSE), [NOTICE](NOTICE) (which
must be kept with it) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Python caches and
`node_modules` were left out.

**Local changes** (keep them when refreshing): in `complexity-analyzer/lib/cxmetrics/coupling.py`,
`load_crates` keeps only the manifests that own a discovered file, so excluded or ignored crates
(the skill's test fixtures, nested worktrees) don't count toward coupling.

**To refresh it,** copy the `complexity-analyzer` folder from the upstream over Swarm Core's,
leaving out `__pycache__` and `node_modules` and keeping the local change above. Note the date and
source here, land it in Swarm Core, and bump every consumer.
