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
- **Notes:** <!-- anything the coordinator could miss from the diff alone: caveats, surprises, decisions taken -->

## Checklist (AGENTS.md › Before you finish a task)

- [ ] `cargo xtask ci --changed` passes locally{{mr_ci}} (fmt, clippy, tests and doctests, layers, wasm{{mr_ci_steps}}, for what the change affects; the full run when its plan says everything)
- [ ] Tests for the change (and a regression test for any crash fix)
- [ ] {{mr_docs}}
- [ ] **Progress** section above filled in
- [ ] A UI change: a test checks what its views draw against their commands' answers, and **Screenshots** below has before and after (docs/app-architecture.md › Verify visually)
- [ ] `log/devlog.md` entry written
- [ ] Events logged (`cargo xtask event`: claim, work, submit)

## Screenshots

<!-- For a UI change: before and after, the same state in each (docs/app-architecture.md › Verify
     visually). From `cargo run -p {{ui_crate}} --example snapshot` where the app has one, or the
     app run with its control socket (`--socket`), driven with `ui.set` and the commands whose views
     changed, and its window captured. Say which commands set the state up, so a reviewer can make
     the same picture. Write "No UI change" otherwise. -->

| Before | After |
|---|---|
|  |  |
