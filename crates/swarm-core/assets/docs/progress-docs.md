<!-- A pinned copy from swarm-core (crates/swarm-core/assets/docs/progress-docs.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`: the gate's
     `sync-assets --check` fails on a copy that differs. -->
# Progress docs standard

> **Last reviewed:** 2026-10-29 · **Last updated:** 2026-10-30 · **Change:** minor (names no consumer) · **Target:** M1

How {{product}} records where it stands and what's next, so any agent can pick up work cold.
Every swarm that shares swarm-core keeps it; it is a pinned copy from swarm-core.

## The documents

| Doc | Kind | Answers | Updated by |
|---|---|---|---|
| [`ROADMAP.md`](../ROADMAP.md) | hand-written, one page | What stage are we at, what are the headline numbers (each the number and its source), what's next (one line an item, linking its gap)? | the coordinator, when a headline number, the stage or the ranking changes |
| [`roadmap.md`](roadmap.md) | hand-written | Milestones (one short sentence each), the gates, and the ranked **Current focus** list (one line an item, linking its gap) | the coordinator, when a milestone or the ranking changes |
| [`gaps.md`](gaps.md) | hand-written | Every known shortfall, ranked, with what's done and left, evidence, numbers, issues and estimates: **the work list, and the only home for detail** | the coordinator, once per batch |
| [`scorecard.md`](scorecard.md) | generated (`cargo xtask scorecard`) from `scorecard/*.toml`, `gaps.md` and the tree | Per area, what is done, partial and missing, with evidence; measured numbers | its checklists' statuses: the PR that changes one; the rows' notes (their evidence) and its Measured block: the coordinator, once per batch ([development.md](development.md#scorecard)) |
| PR **Progress** section | per PR, on GitHub; never in the repo | What this change measured, moved and left open: the source for the three docs above | the PR's author |
| [`owner-directions.md`](owner-directions.md) | hand-written | The owner's standing directions and decisions, dated | the owner, or an agent recording a direction the owner just gave |
| GitHub issues | live | Who is working on what right now (claims), discussion, bugs | everyone; see [the swarm protocol](swarm-protocol.md#picking-work) |
| `log/devlog.md` | local, gitignored | What one machine's sessions did and left open | each agent at the end of a task |

## Rules for the hand-written docs

1. **Header line** directly under the title, updated on every edit:

   ```text
   > **Last reviewed:** YYYY-MM-DD · **Last updated:** YYYY-MM-DD · **Change:** minor|major (what changed) · **Target:** …
   ```

   *Last updated* is the last content change; *Last reviewed* the last time someone checked the
   whole doc against `main`. *Change* is `major` when a milestone or a ranking moves, `minor`
   otherwise, with a one-line summary. *Target* is the milestone the doc measures against.
2. **Revision history** table at the bottom, newest first: `| Date | Change | Summary |`. One row
   per edit; keep the last 10 rows (older ones are in git: ``Older rows: `git log -p <path>`.``).
3. **Measured beats estimated.** Every number says which it is; measured numbers name their source
   (a test, a command) and date.
4. **Hours are agent-hours**: one agent working sequentially, including tests. Give ranges. Mark
   work an agent can't finish alone with **Human:** and what is needed.
5. **Evidence, not adjectives.** Each gap or milestone cites code paths, tests, command ids or
   issue numbers (`#12`).
6. **Close the loop through the PR.** Each PR says what it moved in its **Progress** section
   (template **Default**: measured, moves, still open, notes), and code that closes or narrows a
   gap flips its scorecard row's status in the same PR, with the evidence in Progress: the
   coordinator writes the rows' notes, as it does the gap entries (a note is one line, so two PRs
   writing the same row's note conflict; owner, 2026-10-29). The coordinator folds the batch's
   Progress sections into the gap entries and the scorecard notes in one commit per batch, and touches `ROADMAP.md` and
   `roadmap.md` in that commit only when a headline number, a milestone, a stage or the ranking
   moved (rule 8). The summaries lag `main` by at most one batch, and no PR conflicts on their
   headers and revision rows. The notes stay on the merged PRs; a summary that falls behind is
   caught up from them.
7. **Never inflate.** Something is done only when it has a command or code path, tests, and (for
   UI) a visual check.
8. **One home for each fact** (the owner, 2026-10-19, after the same progress written
   in three docs had drifted 32 PRs apart in another swarm). Detail lives once, in its gap entry in
   `gaps.md` (what's done and left, numbers, issues, estimates) or in the generated
   `scorecard.md`. `ROADMAP.md` keeps the stage line, a headline-numbers table (each cell the
   number and its source, linking the gap or generated doc that holds the detail) and **What's
   next**; `roadmap.md` keeps the milestones (one short sentence each), the gates and **Current
   focus**. What's next and Current focus are the same ranked list, one line an item: its rank,
   its bold name, its gap link and at most a short clause. Every focus item has a gap. Before a
   detail leaves a summary, it is in its gap entry.

## Ids

- Gaps: `G1`, `G2`, … in [`gaps.md`](gaps.md); never reused.
- Milestones: `M1`, `M2`, … in [`roadmap.md`](roadmap.md).
- Issues: `#n` in [{{github.repo}}](https://{{github.host}}/{{github.repo}}/issues).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-30 | minor | Names no consumer: the consumers' names and their issue citations dropped, each rule kept |
| 2026-10-30 | minor | GitLab to GitHub (owner, 2026-10-30): the PR's Progress section, the issues on GitHub |
| 2026-10-30 | minor | swarm-foundation renamed swarm-core: the crate, `cargo xtask swarm-core` (was `foundation`), `docs/swarm-core-assets.md`, and `sc#n` for its issues (was `f#n`) |
| 2026-10-29 | minor | Rule 6 and the table: an MR flips a scorecard row's status and puts the evidence in Progress; the coordinator writes the rows' notes each batch, so two MRs no longer conflict on a row's one-line note (the owner, 2026-10-29) |
| 2026-10-27 | minor | Moved into swarm-core as a pinned asset, the product named by each consumer, issue numbers qualified |
| 2026-10-20 | major | A twin of another swarm's change: one home for each fact (rule 8); `ROADMAP.md` and `roadmap.md` touched only when a headline number, milestone, stage or the ranking moves (rule 6, the table); `owner-directions.md` in the table |
| 2026-10-17 | minor | The scorecard (`cargo xtask scorecard`) |
| 2026-10-17 | major | First version, from another swarm's progress-docs standard (no scorecard or parity yet) |
