## What

<!-- One paragraph: what changed and why. -->

Closes #<!-- issue number; the merge closes it -->

## Progress

<!-- A note for the coordinator, read at merge; it never lands in the repo. The coordinator updates
     docs/gaps.md from it once per batch, and ROADMAP.md and docs/roadmap.md when a headline
     number, a milestone or the ranking moves (docs/progress-docs.md rules 6 and 8). -->

- **Measured:** <!-- numbers, each with its source (test, command, report) and date; or "nothing measured" -->
- **Moves:** <!-- {{mr_moves}}; or "none" -->
- **Still open:** <!-- what this leaves undone, with issue numbers if filed -->
- **Refactoring:** <!-- the refactoring pass (AGENTS.md §5): refactorings applied, by name; or "none", or "none: docs only" -->
- **Notes:** <!-- anything the coordinator could miss from the diff alone: caveats, surprises, decisions taken -->

## Checklist (AGENTS.md › Before you finish a task)

- [ ] `cargo xtask ci --changed` passes locally{{mr_ci}} (fmt, clippy, tests and doctests, layers, wasm{{mr_ci_steps}}, for what the change affects; the full run when its plan says everything)
- [ ] Tests for the change (and a regression test for any crash fix)
- [ ] One refactoring pass with the refactoring skill (AGENTS.md §5), recorded in **Progress**
- [ ] {{mr_docs}}
- [ ] **Progress** section above filled in
- [ ] UI changes checked visually (snapshot PNG attached below)
- [ ] `log/devlog.md` entry written
- [ ] Events logged (`cargo xtask event`: claim, work, submit)

## Screenshots

<!-- Before / after from `cargo run -p {{ui_crate}} --example snapshot`, for UI changes. -->
