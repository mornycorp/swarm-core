<!-- A pinned copy from swarm-core (crates/swarm-core/assets/.claude/skills/README.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`. -->
# Project skills

Skills that every agent working in this repo can use. Claude Code finds them here, in
`.claude/skills/<name>/SKILL.md`; Codex finds the same files through `agents/openai.yaml`. The rest
of `/.claude/` (the agents' worktrees in `.claude/worktrees/`, local settings) is local and
ignored by git.

| Skill | Use |
|---|---|
| [refactoring](refactoring/SKILL.md) | Fowler-style, behaviour-preserving refactorings, one at a time, each verified. **Every ticket ends with one pass of it:** at most three refactorings of the functions and types the branch's diff changed, with no score targets ([AGENTS.md](../../AGENTS.md) §5). |
| [complexity-analyzer](complexity-analyzer/SKILL.md) | Ranks complex Rust, Python and JS/TS functions, and reports design signals. Optional: in a ticket's pass it only suggests candidates among the changed functions; its thresholds are not targets to meet. It needs `uv` and Python 3.11 or later; its tools install to `%LOCALAPPDATA%\refac\tools\complexity-analyzer`, outside the repo. |

Both skills say they run only when invoked by name. AGENTS.md §5 names the refactoring skill, so
that instruction is the invocation.

## Where they come from

**A pinned copy.** Both skills live in [swarm-core](https://github.com/cmaughan/swarm-core)
(`crates/swarm-core/assets/.claude/skills/`), and `cargo xtask swarm-core sync-assets` writes them
here from Swarm Core's pinned revision. The gate's `sync-assets --check`
fails when a file here differs from it: change a skill in Swarm Core, then bump. Each repo may
keep files of its own beside the copy that Swarm Core doesn't ship, such as
`refactoring/references/project-targets.md` (a project's score targets for dedicated
code-health work); the refactoring skill reads it when it is there.

They were first copied on 2026-10-20 from `D:/dev/refac/skills` (commit 9a53a7f), into the swarms that share them. That folder extracts them from
[fcarucci/Swarm](https://github.com/fcarucci/Swarm) by Francesco Carucci, under the Apache
License 2.0: see [LICENSE](LICENSE), [NOTICE](NOTICE) (which must be kept with them) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Python caches and `node_modules` were left
out.

**Local changes** (keep them when refreshing): in `complexity-analyzer/lib/cxmetrics/coupling.py`,
`load_crates` keeps only the manifests that own a discovered file, so excluded or ignored crates
(the skill's test fixtures, nested worktrees) don't count toward coupling; and the line in `refactoring/SKILL.md`'s Selection Heuristics that points at a
repository's own score targets.

**To refresh them,** copy the two skill folders from the upstream over Swarm Core's, leaving
out `__pycache__` and `node_modules` and keeping the local changes above. Note the date and source
here, land it in Swarm Core, and bump both consumers.
