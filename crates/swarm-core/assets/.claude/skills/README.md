<!-- A pinned copy from swarm-core (crates/swarm-core/assets/.claude/skills/README.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`. -->
# Project skills

Skills that every agent working in this repo can use. Claude Code finds them here, in
`.claude/skills/<name>/SKILL.md`; Codex finds the same files through `agents/openai.yaml`. The rest
of `/.claude/` (the agents' worktrees in `.claude/worktrees/`, local settings) is local and
ignored by git.

| Skill | Use |
|---|---|
| [complexity-analyzer](complexity-analyzer/SKILL.md) | Ranks complex Rust, Python and JS/TS functions, and reports design signals. Optional, run when asked; its thresholds are not targets to meet. It needs `uv` and Python 3.11 or later; its tools install to `%LOCALAPPDATA%\refac\tools\complexity-analyzer`, outside the repo. |

It runs only when invoked by name. The `refactoring` skill that shipped beside it, and the
refactoring pass at the end of each ticket, were dropped on 2026-10-31 (the owner's direction).

## Where it comes from

**A pinned copy.** The skill lives in [swarm-core](https://github.com/mornycorp/swarm-core)
(`crates/swarm-core/assets/.claude/skills/`), and `cargo xtask swarm-core sync-assets` writes it
here from Swarm Core's pinned revision. The gate's `sync-assets --check` fails when a file here
differs from it: change the skill in Swarm Core, then bump. A repo may keep files of its own
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
