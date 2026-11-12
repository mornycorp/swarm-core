<!-- A pinned copy from swarm-core (crates/swarm-core/assets/docs/swarm-protocol.md). Edit it
     there, then `cargo xtask swarm-core bump` and `cargo xtask swarm-core sync-assets`: the gate's
     `sync-assets --check` fails on a copy that differs. -->
# The swarm protocol

> **Last reviewed:** 2026-10-28 · **Last updated:** 2026-11-12 · **Change:** minor (a take-over: `stop` frees a claim, a red `main` doesn't hold it, a local branch ahead is refused; standing rules; a fresh context for a different task; every PR branched from `main`; a timeout repeated alone is proof; the review's checklist gains the robustness traps; the periodic full gate reads again under the machine lock) · **Target:** M1

How a swarm of agents builds {{product}}: the roles, the claim, the tracking timeline, the
gates, the coordinator's loop and the periodic full gate. Every swarm that shares Swarm Core
runs it, on the commands
[swarm-core](https://github.com/mornycorp/swarm-core) serves behind each
repo's `cargo xtask`. This repo's own parts (what each machine needs, its capabilities, its gate's
kinds, what its CI runs) are in [swarm.md](swarm.md), and the rules every agent follows in
[`AGENTS.md`](../AGENTS.md).

Issue numbers name their project: `sc#n` is mornycorp/swarm-core's issue n. This repo's own issues are
[{{github.repo}}](https://{{github.host}}/{{github.repo}}/issues).

## Roles

| Role | Who | Does |
|---|---|---|
| **Owner** | a person | Product decisions, credentials, anything labelled `needs::human`. Approves plans before they become issues. |
| **Coordinator** | one agent session (or the owner) | Merges finished work in order, keeps the issue list stocked with small independent tasks, folds the merged PRs' Progress sections into `docs/gaps.md` once per batch (and `ROADMAP.md` and `docs/roadmap.md` only when a headline number, a milestone or the ranking moves: [progress-docs.md](progress-docs.md) rule 8), releases stale claims. Claims an issue it works on itself. Only the coordinator merges. |
| **Worker** | any number of agent sessions, on any machine | Picks the highest-ranked unclaimed issue it can run, claims it, does it, writes the PR's Progress section, marks the PR ready. Never merges, never pushes to `main`, never edits the progress summaries. |

The parts are connected by GitHub ([{{github.repo}}](https://{{github.host}}/{{github.repo}})),
not by talking to each other:
- issues are the work;
- the `status::in-progress` label plus a draft pull request is a claim;
- a ready pull request with a green local gate is finished work;
- every agent logs its sessions and issue events to this repo's `tracking` branch
  (`cargo xtask event`, [Tracking](#tracking)), which a timeline viewer shows. An instance writes
  only its own file, so pushes never conflict: one that loses a race reads the branch again and
  retries. Each send is one `git cat-file --batch` and one `git fast-import` (no checkout, no
  index, no ref of its own; its push skips hooks), so any worktree of a clone can
  log while it works. A failure queues the event in the clone's git directory, and the next send
  (or `cargo xtask event flush`) sends it.

## Standing rules

The owner's directions and the swarms' lessons that hold for every agent, coordinator or worker
(swarm-core#29):

- **Product first; tooling when it hurts twice** (owner, 2026-10-23). Swarm tooling or process
  work only after the same problem has cost twice, or when it blocks product work. Otherwise the
  swarm's time goes to the product.
- **"Wind down" means finish** (owner, 2026-11-03). Finish the unfinished work of every issue you
  hold, through submit and merge, and claim nothing new. Never park a claimed issue.
- **Decide, don't ask** (owner, 2026-11-05 and 2026-11-08). A call with precedent, in the product
  the project follows or in this protocol, is the coordinator's: it decides, records the decision
  on the issue and reports it afterwards. So is topping workers up with issues it filed. A new
  kind of `unsafe`, FFI or hook still goes to the owner.
- **A heavy job run by hand waits for an idle gate** (owner, 2026-11-08). A measurement or sweep
  that takes much of the machine's memory or GPU starts only while no gate runs (`cargo xtask
  lock show`: the machine gate lock is free), samples rather than sweeps, fits a 15-minute budget
  (longer is the coordinator's call) and frees what it took at once. A 104-minute sweep holding
  19.5 GB failed a gate with os error 1450.
- **A quiet gate log is not a dead gate** (2026-11-03). A gate whose log stops mid-step runs until
  its process ends (its task's notification, its `.rc`): never restack or touch the gate's
  worktree under it.
- **Stopping a background shell on Windows leaves its children** (2026-10-29). Claude Code's
  TaskStop ends the shell, not the cargo, xtask, nextest or watcher scripts it started: kill the
  tree by PID (`taskkill /T /F /PID <pid>`) and check that nothing is left.
- **Long runs go to a log, not under a monitor** (2026-11-04). A Claude Code Monitor stops its
  command at its timeout, 30 minutes at most: a claim that runs the periodic full gate, `ready`
  and `ci` run in the background with their output in a log you read as it grows (Claude Code's
  `run_in_background`, which wakes you when it ends). A watch that never ends (`prwatch`) may run
  under a monitor, armed again when the monitor ends.
- **Squash onto the merge base** ([lessons.md](lessons.md), lesson 15):
  `git reset --soft $(git merge-base HEAD origin/main)`, or reset to `origin/main` only right
  after rebasing onto that same `origin/main`. A squash onto a `main` the branch wasn't rebased
  on reverts what `main` gained since.

## Starting a worker

**How many** (owner, 2026-10-19; measured in
[development.md › How many builds at once](development.md#how-many-builds-at-once)): at most
**three workers per swarm** on the owner's desktop (32 GB; it runs two builds well and fails at
four), plus other machines' own. The coordinator starts a new worker only while
`cargo xtask capacity` shows room for another build, its own gate counted. The two swarms share
that desktop: `capacity` counts every `cargo` and `rustc` on the machine, either swarm's, so it
arbitrates between them.

Each worker is a fresh agent session in its **own working copy** (an extra clone, or a
`git worktree` where the project keeps them, such as `git worktree add ../{{project}}-<name> main`;
[swarm.md](swarm.md) names the project's layout when it has one) with its own cargo target dir. Give it this
instruction, filling in its name, what the machine can do and, when the coordinator starts it,
the coordinator's instance id (so the inspector nests the worker under the coordinator; leave
`--parent` out for a worker the owner starts):

```text
You are a worker agent on {{product}} (repo in the current directory). Read AGENTS.md first,
then docs/swarm-protocol.md and docs/swarm.md. Your name is <name>. Use
CARGO_TARGET_DIR=target/agent-<name> (a separate one for each worktree you use: never share one
between two checkouts).
This machine can run tasks labelled: <the capabilities, docs/swarm.md › Capabilities>.

Log your session with `cargo xtask event session.start --name <name> --role worker
--parent <coordinator instance> --model <its model> --effort <its effort>` (keep the instance id
it prints) and your work as docs/swarm-protocol.md › Tracking says.
Pick work exactly as docs/swarm-protocol.md › Picking work says: the highest-ranked open issue in
docs/roadmap.md (Current focus) and docs/gaps.md that is not labelled status::in-progress, has no
open related pull request, and needs nothing this machine lacks. Claim it in one step:
`cargo xtask claim <issue> --agent <your instance id>` (it refuses an issue someone holds, else
makes the branch, the draft PR, the label, the comment and the claim event, and switches a clean
worktree of its own to the branch). On a machine that runs the full gate (docs/swarm.md ›
Capabilities, full-gate), the claim then runs the periodic full gate of main when it is due,
with your issue already held: let it finish. If main fails it for the change's cause, it files a
main-red issue and claims it for you, and releases your task's claim: work on that instead. Run
the claim in the foreground, or in the background with its output in a log you read, never under a
monitor that stops it at a timeout and never piped through tail or head, so you see its lines as it
prints them.
When it prints `fullgate: <sub> won the race for <commit>`, tell the owner at once: "My turn for
the periodic full gate of main (`<commit>`): the whole `ci`, about <n> min." When it ends, tell
them "Full gate done: passed." or "Full gate done: failed (`<cause>`), filed `main-red` #<n>;
working on that instead." Do the work with tests. Your
gate is `cargo xtask ci --changed` (it becomes the full `cargo xtask ci` when its plan says the
change reaches everything). Before each run that builds, run `cargo xtask capacity`: when it
suggests 0 more builds, iterate with `cargo test -p <crate>` or wait a few minutes and check
again, rather than start another build. On a machine where the coordinator gates, cap every
build's jobs: `CARGO_BUILD_JOBS=<n>`, `<n>` from capacity's `worker builds:` line
(docs/swarm-protocol.md › Parallel agents). Run `cargo xtask ci --changed`, check UI changes with the
snapshot example, fill in the PR description's Progress section, then submit with
`cargo xtask ready <m> --agent <your instance id>`: it refuses a PR whose branch
holds no work, with no commits beyond its base, only its claim commit or an empty diff (
finish the work, or close the PR), then rebases onto the latest
origin/main (`--onto <base>` for a stacked PR; a conflict stops it for you to resolve), runs
`cargo xtask ci --changed` on the rebased tree (with the rebased tree's own xtask, built apart,
when the rebase changed xtask's sources, the workspace manifest, the lock file or `.cargo/`, so
the gate judges the tree by its own rules; when only tests failed, it reruns them once,
each alone, then the steps they skipped, sc#14), then checks main once more:
when it moved during the gate, it rebases again and pushes without a second gate if the PR's own
diff is unchanged, gates again if it changed, and stops before pushing on a conflict, naming the
files (resolve them and run ready again). Then it pushes, marks the PR ready for review (its
checks start) and logs `submit`. Do not merge.
Keep your PR a draft until then: `ready` is what marks it ready, never `gh pr ready` or the button.
From your first submit on, watch your PRs with `cargo xtask prwatch --agent <your instance id>`
(under your harness's monitor, so each line it prints reaches you; else as a background
process), and run it with `--once` whenever you wake. Act on what it says at once: a merged base
means rebasing what is stacked on it; a conflict, failed checks or a new comment means you have
work (docs/swarm-protocol.md › Watching your PRs).
Then keep going (owner, 2026-10-23):
1. Pick the next open, unclaimed issue (docs/swarm-protocol.md › Picking work › The next
   issue). Skip `status::blocked`, `needs::human`, the capabilities this machine lacks and any
   label the project holds back (docs/swarm.md). Take them in the project's order: `sync`
   issues first.
2. If it is a different task from the one you just finished (another area of the code, not a
   follow-on that builds on your last change), don't claim it: end your turn with
   `next: #<n>, a different task: a fresh context` and wait. The coordinator clears your
   context and sends you its brief, or starts a fresh worker for it (owner, 2026-11-08).
   A follow-on in the same code you take in this session: your harness compacts the history as
   it grows, so a long context is no reason to stop.
3. Claim it. If `claim` refuses because someone holds it, pick another.
4. Work it and submit it as above.
5. Repeat until no open, unclaimed issue is left. Then, once your last PR has merged and before
   you remove your last worktree, end the session with `cargo xtask finish --agent <your
   instance id> --note "no unclaimed issues left"` and stop (docs/swarm-protocol.md › Ending a
   session).
```

**Workers that keep going don't need starting**. A coordinator's harness may let it start
sessions only in turns its owner begins, so the coordinator starts a worker only to fill an empty
slot, and redirects running workers by message. `claim` settles races between workers.

**A fresh context for a different task** (owner, 2026-11-08, swarm-core#29). A worker's history
helps a follow-on in the same code and hinders a different task: it costs every turn, and what it
remembers of the last area misleads it in the next (one worker went through four unrelated
areas in one session; another reached about 2,200 messages). So, at a task boundary and never
mid-task:
- **The worker notices it:** its next issue is a different task, so it stops and names it (step 2
  of the instruction above). It can't clear its own context.
- **The coordinator clears it:** it clears the worker's session (the Claude Desktop app's
  `clear_session`, which works in turns the coordinator didn't start, where starting a session
  doesn't) and sends the instruction above with the issue's brief; where its harness can't clear
  one, the worker runs `finish` and the coordinator starts a fresh worker in its slot.
- **A similar follow-on keeps its session:** its history helps, and the harness's
  auto-compaction keeps it in bounds. There is no compaction to call.
- **Nothing lives only in a worker's history:** whatever a task needs is in its issue, its brief
  or the shared notes.

A worker's `finish` removes its worktrees and their `target/agent-*` when it is done. How many
run at once is the rule above: at most three on the owner's desktop, and only while
`cargo xtask capacity` shows room.

## Picking work

Priorities: important infrastructure first, then the current focus, then the long tail.

0. **Read `ROADMAP.md`, then `docs/gaps.md`.** `gaps.md` ranks the work; each entry names its
   issues. Workers don't edit these summaries (`ROADMAP.md`, `docs/roadmap.md`, `docs/gaps.md`):
   when your work moves a milestone or closes a gap, say so in your PR's **Progress** section
   (step 5), and the coordinator updates them once per batch ([progress-docs.md](progress-docs.md)).
1. `docs/roadmap.md` → **Current focus**.
2. `log/devlog.md` → the "Still open" bullets of recent entries.

Skip issues labelled with a capability your machine lacks (`needs::human`, and the project's own:
[swarm.md › Capabilities](swarm.md#capabilities)), and say so: `cargo xtask capabilities` lists
what this machine has, what each probe found and how to fix a missing one; `event session.start`
ends with the missing ones and `claim` warns when the issue needs one (swarm-core#13). Tell the
owner what you skip for want of a capability, with its fix, rather than pass the work over: on
2026-10-28 a session skipped every issue that needed a tool the owner set up in ten minutes once
asked.

**The next issue** (owner, 2026-10-23), for a worker that keeps going: the order is the
project's, and its [swarm.md](swarm.md) gives it when it differs. Every project takes `sync`
issues first. Then a project may take the oldest, skipping `code-health` while the owner's
pause stands, or the highest-ranked.

Then **claim it on GitHub before writing code**
([{{github.repo}}](https://{{github.host}}/{{github.repo}})). Many agents run under one GitHub
account, so the assignee can't tell agents apart: the claim is the label plus the draft pull
request. Always pass `--repo {{github.repo}}` to gh.

1. **Find or create the issue.** Every task is an issue. If the work came from a gap with no
   issue, create one (template **Task**) naming that gap.
2. **Claim it with one command, at the moment you start:**
   `cargo xtask claim <n> --agent <your instance id>`.
   - **Not while `main` is red**: an open `main-red` issue means nothing merges and no
     work starts from `main` until it is green again. The command refuses then, naming the issue;
     wait for it to close, or keep working on a branch you already have.
   - **Not while it is on hold**: an issue labelled `status::blocked` is refused, with
     the comment that says why. Only whoever put the hold on lifts it.
   - It checks, then and there, the `status::in-progress` label, the open pull requests for the
     issue and other agents' live claims on the tracking branch (24 hours, no later stop, release
     or merge). If anyone else holds it, it refuses, naming who and where: pick another issue.
   - Otherwise it makes the whole claim, in this order: the branch `<n>-<slug>`
     from the latest `origin/main` with a claim commit, pushed; a **draft** PR with `Closes #<n>`
     (template **Default**); the label; the comment `claimed: <agent name>, PR #<m>`; the `claim`
     and `work` events; the race check; only then the periodic full gate, when it is due, so the
     issue is held while it runs (below). It prints the branch and PR, and switches the worktree
     it ran in to the branch when that worktree is clean and isn't the primary checkout (`--no-switch` to stay; otherwise it says how). `--slug` names the branch; run it again to
     finish a claim that failed half way. It notes the open needs of the issue it claims
     ([Watching your PRs](#watching-your-mrs)).
   - **Then the periodic full gate** (owner, 2026-10-22): on a machine with the
     `full-gate` capability ([swarm.md › Capabilities](swarm.md#capabilities)), once it holds the
     issue, it runs the full gate of `main` when it is due (`main` moved since the last
     passed one, and that pass is over an hour old; one agent at a time): the whole
     `cargo xtask ci`. Let it finish. When `main` fails it for the change's cause, it files a
     `main-red` issue and claims it for you, and releases your task's claim (its draft PR closed,
     the label off, `release --reason main-red`): work on that instead. `cargo xtask fullgate
     --check` says whether one is due ([The periodic full gate](#the-periodic-full-gate)).
     - **Tell the owner** (owner, 2026-10-22): it takes a whole `ci`'s time (10 to 15
       minutes, say; [swarm.md](swarm.md) gives the project's own) and holds the
       machine gate lock, so don't go quiet. When the claim prints `fullgate: <sub> won
       the race for <commit>`, say at once **"My turn for the periodic full gate of main
       (`<commit>`): the whole `ci`, about <n> min."** When it ends, say **"Full gate done:
       passed."** or **"Full gate done: failed (`<cause>`), filed `main-red` #<n>; working on
       that instead."**
     - **Run the claim where its output reaches you as it prints:** in the foreground, or in the
       background with its output in a log you read; never under a monitor, which stops it at
       its timeout ([Standing rules](#standing-rules)). Never pipe it through `tail` or `head`,
       which hold every line back until the claim ends.
   - Never claim from a listing you made earlier: four claims raced on 2026-10-18. If two still race, the earlier PR keeps it: the command checks again once its
     PR exists, and closes its own if it lost.
   - **Two claims on one branch** (every agent derives the same `<n>-<slug>`): a PR from that
     branch is yours only when no other agent's live claim or standing claim comment names it,
     so a later claim is refused rather than adopting the first's PR. When two claims land
     seconds apart, both on one PR, the first standing `claimed:` comment (GitHub orders them)
     holds the issue. The later claim releases once its own comment is posted: it logs
     `release --reason duplicate` and comments `released: <name>, …`. It leaves the shared PR,
     branch and label to the first, and doesn't switch its worktree. `ready` refuses to push a
     branch whose issue another agent holds by an earlier live claim or claim comment
     (mornycorp/swarm-core#7: three times in one swarm a second claim adopted the first's PR,
     and once its `ready` force-pushed over it).
   - By hand only when the command can't run (no gh): check the label and the open PRs for the
     issue (`gh pr list --repo {{github.repo}} --search "<n> in:title,body"`), push a first commit
     on the branch, `gh pr create --repo {{github.repo}} --draft --title "<summary> (#<n>)"
     --body "Closes #<n>"`, label, comment, `cargo xtask event claim`.
3. **Taking over.** A claim is stale, and may be taken over, only when the coordinator says so
   or it has been idle for 24 hours. The coordinator's say-so is
   `cargo xtask event release --issue <n> --of <the holder's instance> --reason stale` (or a
   `released: <name>, …` comment), with a comment naming who takes it over. A holder that logged
   `stop` on the issue after its last `claim` or `work` holds nothing: its claim is free at once.
   Idle means no event of its holder on the issue, no claim comment and no commit on its branch
   for 24 hours (step 6).
   - `cargo xtask claim <n> --agent <your instance id> --take-over` takes it and keeps its work:
     it adopts the issue's open PR and its branch, and checks out the branch's head in your
     worktree with the commits kept. It logs `release --of <the old instance>` (unless that holder
     is released already), comments `released: <old name>, taken over by …`, and makes the claim:
     the label, `claimed: <your name>, PR #<m>`, `claim` and `work`. It runs no periodic full gate,
     and a red `main` doesn't hold it: the work goes on on a branch that exists, and nothing new
     starts from `main`.
   - A local branch of that name with commits origin's lacks is refused, before anything is
     posted: taking it over would push the old holder's unpushed work, or lose it. Push or drop
     those commits, then take over again.
   - A `status::blocked` label comes off when every issue its hold waits on is closed: those the
     `Needs` line names, and the newest comment saying `Blocked on …` or `Waiting for …`.
     Otherwise it stays, and the takeover says why. A held issue is idle by design: an idle
     claim on one whose cause may remain is taken over only on the coordinator's say-so.
   - It refuses a claim that isn't stale, naming its holder and when it was last active: ask the
     coordinator, or wait.
   - `--force` claims an issue someone else holds on a new branch and PR (a hold lifted by hand,
     with no work to keep). It never adopts a branch with work on it, and says to use
     `--take-over`.
4. **Work in the open.** Push often. Blocked: label `status::blocked` and comment why (a
   `status::blocked` issue is one nobody takes: the claim command refuses it). Needs a
   person: `needs::human`. `cargo xtask ci --changed` warns when another agent holds your
   branch's issue (`cargo xtask claim --check`); the optional `scripts/hooks/pre-push` hook does
   it before each push.
   - **Branch every PR from `main`** (owner, 2026-11-03, swarm-core#29), as `claim` does. Stack
     on another open PR (rebase your branch onto its branch, then `ready --onto <its branch>`)
     only when your change needs that PR's code to build or work, and say in your PR's
     description what code of the other it needs. PRs that merely touch the same files stay on
     `main`: ordering ready PRs is the coordinator's job (`stack` composes each gate's batch),
     and when `stack` stops on a textual conflict, the second PR's author rebases once after the
     first lands. A chain makes each PR wait on every gate of the PRs below it
     ([lessons.md](lessons.md), lesson 13).
5. **Finish.** Run the checks in AGENTS.md §5, keep the technical docs the change touches
   current, then fill in the PR description's **Progress** section (template **Default**): what
   you measured, the gap it moves, what's still open, and anything the coordinator could miss
   from the diff alone. **Then submit with `cargo xtask ready <m> --agent <id>`**,
   in the PR's worktree: it rebases onto the latest `origin/main` (`--onto <base>` for an
   PR stacked on another's branch), stopping on a conflict for you to resolve, runs `cargo xtask
   ci --changed` on the rebased tree, checks `main` once more (a move during the gate: it rebases
   again, pushing without a second gate when the PR's diff is unchanged, and stopping before the
   push on a conflict), pushes, marks the PR ready for review (no GitHub Actions run: CI is
   local) and logs `submit`. Its gate:
   - **Its own sub-agent**: it logs each gate it runs as `ready-<the head's short
     sha>` (`sub.start --name ready`, then `sub.stop` with `rc=0`, or a failure's
     `rc=<n>; cause=<cause>; step=<step>`: the exit code, the cause read from the log as the gate
     reads its own, and the `ci` step it failed at, each left out when unknown, such as
     `rc=101; cause=build; step=clippy`), apart from the coordinator's `gate-*` gates. Its output goes to the terminal and, with its `.rc` record and
     nextest's junit file, to `<target>/gate-logs/ready-<sha>.log`, `.rc` and `.junit.xml`, as
     the coordinator's gate keeps them (sc#14).
   - **A test-run slot** (sc#17): each gate, with its rerun, holds one of the machine's test-run
     slots ([Test runs share the machine](#staying-awake), in Staying awake), waiting for one while
     every slot is taken, whichever swarm holds them, and saying who does. A docs change (the
     project's planner says so) takes none.
   - **A test that failed under load** (sc#14): when the only failure is the test step's and it
     can name the failed tests (1 to 12), it reruns them once, as `gate --rerun-failed` does:
     each alone, then the doctests and the steps the failure skipped, on the same HEAD, as the
     sub-agent `ready-<sha>-rerun`. A pass goes on to check `main` and submit, and the `submit`
     event's note names the tests that passed only alone, for the coordinator. Anything else
     stops `ready` as before: a build, clippy or fmt failure, a failure at another step, a test
     that fails again alone, and a HEAD whose rerun already ran (one a HEAD). `--no-rerun` stops
     at failed tests too.
   - **`--no-ci`** skips the gate and the second look at `main` (a docs-only PR whose gate just
     passed, say), and the `submit` event says so: its note begins `no-ci`. The coordinator's
     smoke runs no tests, trusting your run, so a stack holding a PR whose latest `submit` says
     `no-ci` is gated with `ci --changed` instead (the **changed** kind, [The coordinator's
     loop](#the-coordinators-loop); sc#18). Readied again with its run, the smoke is enough.
   - **The rebased tree's own xtask**: when the commits since this xtask was built touch
     one of the project's `Project::xtask_rules` (xtask's sources, the workspace manifest, the
     lock file, `.cargo/`), it builds the rebased xtask into `<target>/ready-xtask` and gates
     with that. Some checks run inside xtask, so the old rules must not judge the new tree, and
     Windows can't replace a running executable. The choice is made again for a second gate.

   Rebasing just before submitting puts conflicts with you, who know the change, rather
   than in the coordinator's stack, and makes your gate test what will be stacked. By hand: the
   same steps, then `gh pr ready <m> --repo {{github.repo}}`. **Workers do not
   merge:** the coordinator rebases, runs the local gate (a smoke, `cargo xtask ci --smoke`, for a
   change the docs and tooling gates don't take) and merges in dependency order; the
   periodic full gate tests `main` after ([The coordinator's loop](#the-coordinators-loop)). So
   your own `cargo xtask ci --changed` before ready is what proves your change before it merges
   (owner, 2026-10-22): the whole gate for the packages your change affects, their
   dependents and the packages that read a file it changed, doctests included; it becomes the
   full `cargo xtask ci` when its plan says the change reaches everything. The merge closes the
   issue, and the coordinator removes its label (`cargo xtask tidy <n>`). From your first
   submit, watch your PRs with `cargo xtask prwatch --agent <id>` (`--once` on every wake) and act
   at once: a merged base means rebasing what is stacked on it
   ([Watching your PRs](#watching-your-mrs)).
6. **Give up cleanly.** Comment what you learned, remove `status::in-progress`, close the draft
   PR. A claim with no activity for 24 hours (no events on the issue, no claim comment, no new
   commits) may be taken over (step 3).

## Tracking

Each swarm logs to its own timeline, so a timeline viewer can show the swarm at work. Log
your sessions and your work on issues with `cargo xtask event`; each call takes a few seconds (a
fetch and a push to this repo's `tracking` branch) and never fails your task: offline, it queues
the event and sends it with the next.

- **Start of your session:** `cargo xtask event session.start --name <your name> --role worker
  --model <the model you run as, such as claude-opus-5-5> --effort <low|medium|high|xhigh|max>`
  (`--model` and `--effort` when you know them: the owner compares models by them). It prints
  your instance id: keep it and pass `--agent <id>` to every other event this session (or set
  `{{env}}_AGENT`). A session another agent started (the coordinator's workers) adds
  `--parent <the starter's instance id>`, which the starter gives in its brief.
- **A helper inside your session** (a gate, a search, a review) has no file: you log it in yours,
  `sub.start --sub <id unique in your session> --name <what it is>` (`--model <its model>` when it
  runs on another model than yours) and `sub.stop --sub <id>`, and `--sub <id>` on an issue event
  it causes. The helper never runs `cargo xtask event` itself.
- **On an issue:** `claim --issue <n> --mr <m>` when you claim it (`cargo xtask claim` logs it,
  then `work`, for you); `work --issue <n>` when you start or resume work on it, and
  `work --issue <n> --mr <m>` when you pick up a PR sent back (`prwatch` logs it for you, once);
  `submit --issue <n> --mr <m>` when you mark its PR ready; `stop --issue <n> --reason
  handoff|blocked|paused|done` when you stop working on it; `release --issue <n>` when you give it
  up, with `--reason duplicate --note "<who has it>"` when another agent has it (your work so far
  is thrown away), and close the issue on GitHub as a duplicate, with a comment
  `Duplicate of #<original>`.
  The coordinator ending another agent's work logs `release --issue <n> --of <its instance>`.
  Ending a dead agent's session (checked dead, not just quiet), it logs
  `session.stop --of <its instance> --note "<how it knows>"`; each batch it checks the sessions
  presumed dead, silent past a day ([The coordinator's loop](#the-coordinators-loop) step 8).
- **Your session:** `session.pause` and `session.resume` while you wait and when you go on;
  `cargo xtask finish` when you end, which logs `session.stop` last
  ([Ending a session](#ending-a-session)).
- **`--note "<one line>"`** on any event: why you're blocked or stalled, what you hand off.

Both swarms write **version 3** of the tracking format (`session.start`'s `format`;
[the tracking format › Versions](https://github.com/mornycorp/swarm-core/blob/main/docs/tracking-format.md#versions),
swarm-core's). A change to the format, or to `cargo xtask event`, is done only when that
section's rule holds.

## Parallel agents

- Use your own target dir (`CARGO_TARGET_DIR=target/agent-<name>`): **one per checkout or
  worktree**, never shared between two (cargo would count another checkout's build as yours).
  Delete it when you're done. `cargo xtask ci` enforces it: it records the
  checkout in `<target>/{{checkout_marker}}` and refuses a target dir another checkout used last.
- Work in your own clone or `git worktree` on your claimed branch; never on `main`.
- **Helpers in worktrees:** a helper sub-agent on an independent piece of your issue gets a
  worktree, branch and target dir of its own; `cargo xtask capacity` says whether the machine
  takes another build and `--helper <name>` sets one up
  ([Helpers in worktrees](#helpers-in-worktrees)).
- **A gate has priority** where the project turns it on (sc#41, `Hooks::gate_priority`; off by
  default, when `capacity` counts no gate and `ready` never waits): while a gate holds the
  machine-wide gate lock (from before it waits for room until its `ci` ends), `capacity`
  reserves one build for it (a build's cores and memory) unless its build is counted already (a
  busy target dir in its worktree), and says so (`gate:` lines). The gate's own check, from the
  worktree its lock names, doesn't count itself. So build helpers, the periodic full gate's
  claim and a project's own builds that ask `capacity` (`capacity::read_with`) yield to it, and
  `ready` waits before each `ci`, before it takes its test-run slot, while a gate holds the lock
  and there is no room, saying `a gate has priority` (every minute, at most 30, then it goes
  ahead).
- **Cap your builds' jobs where the coordinator gates** (sc#40): on a machine where the
  coordinator gates, workers cap their builds' jobs, so a gate keeps memory for its own. A build
  left alone takes every core and whatever memory it gets, and `gate` sizes its build from the
  memory left: measured 2026-11-06, gates took 11 to 23 minutes with the machine mostly free and
  28 to 44 beside two workers' full-width builds, one down to 3.3 GB free. So set
  `CARGO_BUILD_JOBS=<n>` on every build, test and `ci` you run there, `<n>` the jobs per worker
  build `cargo xtask capacity` suggests (its `worker builds:` line, 4 unless the machine sets
  its own cost of a build): `CARGO_BUILD_JOBS=4 cargo test -p <crate>`. `ready` does it for its
  own builds where the project turns it on (`Hooks::worker_build_jobs`, off by default), on a
  machine with the `full-gate` capability ([swarm.md › Capabilities](swarm.md#capabilities)),
  and says so; a `CARGO_BUILD_JOBS` you set always wins. Tooling can't cap a `cargo test` or
  `cargo build` you run yourself: this rule does. The exception is a measurement window the
  coordinator calls. `gate` keeps sizing its own build from free memory.
- **Leave the owner's checkout alone.** On a machine the owner builds on, the primary checkout
  stays on `main` for the owner's incremental builds: never check out a branch, commit, rebase,
  reset or stash there, and never build in it with your target dir. Work in a worktree; read the
  checkout (`status`, `log`, `fetch`) only. The coordinator gates in a worktree of its own and
  fast-forwards the checkout after each merge when it is on `main` with no tracked changes.
- Edit only the files you own. Shared files get small, surgical edits; re-read before editing
  (AGENTS.md lists this repo's).
- Leave the progress summaries (`docs/gaps.md`, `docs/roadmap.md`, `ROADMAP.md`) to the
  coordinator; report in your PR's **Progress** section instead.
- If someone else's in-progress edit breaks the build, wait and retry; don't fix their files.
- Keep every `Cargo.toml` valid at all times: the workspace's member glob means one broken
  manifest breaks everyone's build. Write a new manifest to a temp file outside the workspace's
  crates, then move it into place.
- **The pinned copies** (this file among them; `docs/swarm-core-assets.md` lists them) are
  Swarm Core's: change them there, then bump (`cargo xtask swarm-core bump`, then
  `cargo xtask swarm-core sync-assets`). The gate's `sync-assets --check` fails on a copy edited
  here.

## Watching your PRs

A worker's PRs need it after it submits them: the coordinator merges a base (often rebased, so
what is stacked on it conflicts), a check fails that the worker's machine lacks (CI is
Linux; a Windows or Mac worker can miss a Linux-only warning), or the coordinator sends it back
with a note. `cargo xtask prwatch --agent <instance>` says so as it
happens:

- It knows a worker's PRs from its own `claim` and `submit` events on the tracking branch (every
  agent shares one GitHub account), less the issues it released.
- It polls every 150 s (`--interval`) with one GitHub call (the open PRs, each with its merge
  state and its head's checks) and prints a line only when something changes: merge state
  (`dirty`: a conflict, `behind`, `clean`, `blocked`), checks that finished, new comments, a PR
  that merged or closed. The first poll only records (a send-back, below, it still logs).
- **A send-back** (the coordinator's `sendback` of your PR, or its gate failing for the change's
  cause, read from the tracking branch) is a line of its own, `#m sent back (<when>): <why>`, and
  `prwatch` logs your `work --issue <n> --mr <m>` for it then, once, so the timeline shows where
  your rework starts (and the time before it as waiting). A comment that only asks a question
  logs nothing. Not running `prwatch`? Log that `work` yourself when you start on the sent-back PR.
- `--once` polls once and lists each PR's status: run it on every wake, so nothing waits on the
  watcher being up. Its last poll is kept in the clone's git directory
  (`prwatch-<instance>.json`), shared by its worktrees.
- **What your issues need**: an issue that waits on another names it on a line that
  starts `Needs`, such as `Needs S4.` or `Needs #265 and #266` (`Depends on` and
  `Blocked by` read the same; only the first clause, up to a `;` or the sentence's end, counts).
  `prwatch` reads those lines in your open claimed issues and says when a needed issue closes or
  gets a PR, often another agent's; `--once` lists the ones still open and their PRs. Two more
  calls a poll, the claimed issues and the needed ones. `claim` notes the open needs of the issue
  it claims. Whoever files dependent issues writes their `Needs` lines in that form. A need in
  another project is named in full, `group/repo#n` (`mornycorp/swarm-core#4`): a change
  here that waits on a Swarm Core PR says so, and `prwatch` and `claim` read that project's issue
  and PRs (two more calls a poll for each such project). A short name (`ns#381` from another project) is no need.
- A Claude Code worker runs it under Monitor (each line wakes it), armed again when the
  Monitor ends at its timeout; others as a background process. Never pipe it through `tail` or `head`: they print nothing until the watch ends, and
  it doesn't (2026-10-25: a merge sat unseen for hours that way).

What each line asks of you:

| Line | Do |
|---|---|
| `#m merged: rebase what is stacked on it` | rebase the next PR of the stack onto `main` at once (`git rebase --onto origin/main <the old base's tip>`), gate, push |
| `merge state: … -> dirty` | a conflict: rebase onto `main`, resolve, gate, push |
| `merge state: … -> behind` | nothing: the coordinator brings it up to `main` at merge |
| `checks: failure` | GitHub runs no workflows, so a check is something outside the swarm (an app, a hand-added workflow): say so on the PR and tell the owner |
| `#m sent back (<when>): <why>; logged \`work …\`` | the coordinator sent it back: rework it (the `work` is logged), gate, push, and `ready` again |
| `n new comments` | read them: a question (answer it), or a send-back's reasons (`prwatch` says `sent back` itself and logs its `work`; if it didn't, log `work --issue <n> --mr <m>` as you start) |
| `#n closed (needed by #m): start what waited on it` | rebase `#m`'s branch onto `main` and build what waited on `#n` |
| `#n (needed by #m) taken: PR !k` | nothing yet; someone works on it. Don't claim it |
| `no open PR or claim left: … cargo xtask finish …` | pick the next issue; or, if none is left, end your session with `finish` |

**A `ready` that failed at its tests** (sc#14) has already rerun them alone, once, unless it says
why it didn't. Don't run `ready` again in the hope that they pass: that repeats the whole gate,
20 to 60 minutes on a busy machine, and adds the load that made them fail. Read its log
(`<target>/gate-logs/ready-<sha>.log`, and `ready-<sha>-rerun.log`): a test that failed alone too
is a real failure, yours to fix, or another change's to file. On the same HEAD, `ready` reruns no
tests alone a second time.

## Ending a session

End a session in one step, after your last PR merges and before you remove your last worktree
(sc#10: a worker that removed its last worktree first had no checkout left to
log its `session.stop` from):

```sh
cargo xtask finish --agent <your instance id> [--note "<why you stop>"]
```

- **Nothing owed.** An open PR of yours, or an issue you claimed that is still open, stops it,
  naming them: finish them or release them first. `--force` ends the session anyway, and the stop
  names what was left.
- **Tools that need no worktree.** It installs this xtask in the clone's git directory
  (`.git/swarm-tools/`; `cargo xtask install-tools` does only that). Run from there, it finds the
  clone by where it sits, so `.git/swarm-tools/xtask event …` (or `finish`) works from anywhere
  with no worktree and no build.
- **Worktrees.** It removes your worktrees on your merged or closed PRs' branches when they are
  clean, the target dirs inside them too, and names a dirty one. Never the main checkout, never
  one whose PR is open, and not the one it runs from (its xtask is running there): it prints the
  `git worktree remove` for that one.
- **Branches, then the stop.** It deletes your merged PRs' local branches, then logs
  `session.stop`: with your `--note`, else what it did.
- `--dry-run` says what it would do and does nothing.
- `prwatch` says when: once your last PR leaves the open list and no claim of yours is open.

**After `ready`, push to the PR only for a line above that asks it, or a fix you found** (sc#9),
with a note on the PR saying what changed. The coordinator gates the commit it stacked, and
`land` merges only that change: a push while it lands (a rebase of your own for `need_rebase`, a
late fix) stops the landing, and the PR waits for the next gate.

## Helpers in worktrees

A worker (or the coordinator) may hand a piece of its issue to a helper sub-agent working in a
worktree of its own, where that speeds it up on its machine (owner, 2026-10-18).

- **When:** independent pieces only: gating one commit while working on the next; disjoint parts
  of one issue (the engine and the UI); research, measurement or snapshot sweeps. A second issue
  is not a helper's job: that is a worker, started with `--parent`, that claims it with
  `cargo xtask claim`.
- **Two kinds** (owner, 2026-10-18):
  - **Build helpers** (`--kind build`, the default) compile and test: a gate, the other half of an
    issue. Each is a build on the machine, so `cargo xtask capacity` must have room for it.
  - **Read helpers** (`--kind read`) build nothing: research, review, reading logs, another
    repo or GitHub. They are cheap and fine to run beside builds, so the build suggestion never refuses
    them; they get no target dir, and a worktree only with `--worktree`, when they will write files
    (notes, a patch); otherwise they read yours. At most 4 open per worker: each is still another
    agent, spending tokens and the shared GitHub account's calls.
- **How many:** `cargo xtask capacity` decides, not a guess. It reports the logical cores, free
  memory, free disk on the target dir's drive and the builds running now (other target dirs in
  the clone's worktrees written in the last 5 minutes, and the `cargo`, `rustc` and
  `cargo-nextest` processes; with no `rustc` and no `cargo-nextest` running anywhere, none, as a
  worker between builds holds no cores), then suggests how many more builds the machine takes:
  about one per 8 free cores (a running build holds 8) and 5 GB of free memory, at most 3, and
  none with under 20 GB of disk free or a figure it couldn't read; each number is printed with
  its basis. A machine unlike the owner's desktop sets its own 8 and 5 with
  `SWARM_CORES_PER_BUILD` and `SWARM_GB_PER_BUILD` (whole numbers, read by both swarms): on
  2026-10-30 a 12-core Mac took one build by the 8, and a gate waited 35 minutes behind a worker
  that was only editing between builds. The 5 GB is measured ([development.md › How many builds at once](development.md#how-many-builds-at-once)): on the
  owner's 32 GB desktop, two gates at once passed and four filled the memory and failed. It
  limits build helpers only. It also suggests none while **every test-run slot is taken** (the
  test runs going now, the asker's own left out: *Test runs share the machine*, in Staying
  awake) or while **wired memory is half the total or more** (macOS: `vm_stat`'s
  "Pages wired down"; on unified memory what the GPU holds is wired), and prints both figures:
  on 2026-11-03 two test runs at once wired about 20 GB of a 24 GB Mac, pushed 8.6 GB to swap,
  and it locked up (sc#17). It reads
  the OS's own tools, no new crates: `/proc/meminfo` (`MemAvailable`), `df` and `ps` on Linux;
  `sysctl`, `vm_stat` (free, inactive, speculative and purgeable pages; wired pages apart), `df`
  and `ps` on macOS; on Windows
  one PowerShell (CIM's available memory, standby cache included, `Get-PSDrive` and
  `Get-Process`), about 3 seconds.
- **Set up:** `cargo xtask capacity --helper <name> --note "<what it does>" [--kind build|read]
  [--worktree] [--role <what it is>] [--agent <your instance>]`, from your own worktree on your
  branch. A read helper's sub-agent is `read-<name>` (so its worker's open ones can be counted),
  with no target dir and no worktree unless `--worktree`; the rest of this item is a build helper. It makes the helper's
  worktree beside yours, named `<your worktree>-<name>` (`../{{project}}-alder-gate` beside
  `../{{project}}-alder`, `.claude/worktrees/alder-gate` beside `.claude/worktrees/alder`),
  its branch `<your branch>-<name>` at your HEAD, its target dir
  `target/agent-<you>-<name>` in that worktree (marked as that worktree's, so no other checkout
  builds there), and logs its `sub.start --sub <name> --name <role> --note "<what>"`. It
  refuses in the clone's main worktree (the owner's checkout: the first entry of
  `git worktree list --porcelain`), on a detached HEAD, when the worktree or branch exists, and
  when the suggestion is 0.
- **Isolation:** a helper builds only in its worktree with its own
  `CARGO_TARGET_DIR=target/agent-<you>-<name>`, never shared (`cargo xtask ci` refuses a target dir
  another checkout used) and never in the owner's checkout.
- **Branches:** the helper commits on its branch; you bring its commits back with a rebase or
  cherry-pick, and only you push the PR branch.
- **Tracking:** the helper never runs `cargo xtask event` itself. You log its `sub.start` (the
  setup does), `--sub <name>` on any issue event it causes, and its `sub.stop --sub <name>` once,
  when it is done.
- **Cleanup:** check `git worktree list` (worktree names are reused), then remove the helper's
  worktree (`git worktree remove`), its target dir and its branch (`git branch -D`).

## The coordinator's loop

On each finished pull request:

1. **Check it:** the worker's local `cargo xtask ci --changed` passed; the PR closes the right issue; its
   **Progress** section is filled in and it leaves the progress summaries alone; nothing outside its scope changed; UI changes have a snapshot.
2. **Stack it** onto `main`: `cargo xtask stack <pr>…`, in the coordinator's worktree,
   for the batch's ready PRs in merge order. Each PR becomes `s-<pr>`, its source branch rebased
   onto `origin/main` or onto the PR below it (only its own commits, when it was stacked on that
   one), and HEAD ends at the top of the stack, ready to gate. It takes the gate lock first
   (`--agent`, or `{{env}}_AGENT`): a rebase is already a write to the worktree. It doesn't take
   the machine-wide lock: only a gate's build does ([Staying awake](#staying-awake)).
   - **A branch that carries merge commits is merged, not rebased:** a rebase would lose the
     conflict resolutions its merges hold, so its base (`origin/main`, or the PR below) is merged
     into it instead.
   - **It records each source branch's pre-rebase tip** (in the worktree's git dir, in the
     project's `Project::stack_state`, here `{{stack_state}}`), so `land` never pushes over
     commits a worker pushed since.
   - **It resolves the progress docs' header and revision-row conflicts itself**: the header takes
     the PR's change note and the later of each date, the rows are the PR's then `main`'s, and the
     table keeps 10 rows.
   - **It regenerates a conflicting generated doc**: a doc `cargo xtask`
     writes (the project's list: one project's `docs/scorecard.md`, another's three) takes the
     base's side, the rebase finishes, and the doc is then regenerated by its own `cargo xtask
     <command>` on the stacked tree and committed on `s-<pr>` as "regenerated <doc> (stack)". The
     record names it (`regenerated`), the gate checks it (`<command> --check`) and `land` pushes
     it with the PR; a regeneration that fails stops the stack like a conflict. (In one swarm, two
     PRs went back to their authors on 2026-10-21 for a generated doc alone.)
   - **Any other conflict aborts the rebase** and names the files: resolve it
     by hand (the shared files AGENTS.md lists conflict most: keep both sides), or send the
     PR back, logging it as a conflict: `sendback --issue <n> --mr <m> --note "cause=conflict: <files>"`
     (a timeline viewer's send-back rate counts it; a gate's failure counts without one).
   - **A stop leaves the last good top**: HEAD goes back to `s-<pr>` of the last PR
     stacked (or `origin/main`, detached, when none), only those PRs are recorded, and its last
     line says so. Never the failing PR's unrebased branch, which a gate once tested as the stack.
   - The issues each PR closes are read from its description (`Closes #n`); a draft, a closed PR
     or one from a fork is refused.
   - **A PR with no work is refused**: rebased, it has no commits of its own,
     only claim commits (`Claim #<n>: …`) or an empty diff. The stack stops at it as at a
     conflict, saying to finish the work or close the PR: send it back to its worker, or close
     it. On 2026-10-27 a PR was un-drafted with only its claim commit, and the ready watch,
     `stack`, a 0.4-minute smoke and `land` merged it, closing its issue with none of its work.
3. **Merge on the local gate:** run `cargo xtask gate --sub gate-<n> --note "local gate: #<m> (#<i>)"`
   on the stacked tree (it refuses a rebase in progress or conflict markers, and a HEAD
   that isn't the top of the stack record, naming both; picks the gate's kind (below),
   waits for capacity with `--wait` unless it is a docs gate, takes the gate lock, logs
   the gate, runs it into `<target>/gate-logs/gate-<n>.log`, writes `rc=<n> <secs>s [cause=<cause>] kind=<kind> (<why>) head=<commit>` to
   `gate-<n>.rc`, keeps its tests' times in `gate-<n>.junit.xml` when it ran tests (
   `cargo xtask gate-times gate-<n> [gate-<m>]` lists the slowest and compares two gates) and
   names a failure's cause), then
   `cargo xtask land <pr>… --sub gate-<n>` lands the stack's PRs in order (it retakes the gate lock first, and not
   the machine-wide lock, as it builds nothing). It refuses
   PRs that aren't the record's next ones, a PR with no work against what it was stacked onto
   (no commits of its own, only claim commits or an empty diff; checked before anything is
   pushed), a gate whose `.rc` isn't a pass, and a stack whose top
   isn't the HEAD that `.rc` names, and lands nothing while a `main-red` issue is open
   (`claim`'s check, one function): it refuses, naming the issues, unless
   every PR it lands closes one of them (`Closes #<n>` in its description: the fix), or
   `--while-red "<why>"` is given, the why going into each merge event's note (on 2026-10-23
   a swarm's `land` merged two PRs while two `main-red` issues were open). For each PR, it
   stops at the first step that fails, naming the PR and why, and leaves the gate open and the
   lock held:
   1. `s-<pr>` is still its recorded rebased tip (or, on a rerun, `land`'s own pushed merge of
      `main` into it, step 4), and the source branch its recorded pre-rebase
      tip (a worker pushed since: restack). A branch that moved lands only when the gate already
      tested its content: a worker who rebased onto the same `main` while
      the gate ran. `land` restacks the worker's commit onto what `stack` put the PR on (`git
      merge-tree`, which touches no checkout or branch of the gate's worktree) and compares its
      tree with the stacked commit's. The same tree, on a base with nothing of `main` the gate
      didn't test, lands: `!N moved since stack (old → new) but its tree is the one gated
      (<how>): landing`, where `<how>` says whether it was restacked or the worker's own commit
      is that tree already, and the stacked commit is pushed over the worker's. A branch a worker
      rebased onto a **newer `main`** lands without a re-gate when three things hold: its own
      commits' patch-id is the stacked commit's, applied onto what the gate tested they are the
      gated tree, and the newer `main` changes none of the PR's files. The stacked commit is then
      pushed over it (`#N moved onto a newer main (old → new) but its patch is the one gated:
      landing the gated commit`), and step 5's check holds what merges to the gated change. A
      changed patch, a conflict, a newer `main` that touches the PR's files, or another tree (a
      new commit) stops, saying which. An older record names
      no base to restack onto, so a moved branch in it stops. And the PR must be open: GitHub
      closes a PR whose base branch is deleted, so one stacked on the PR below it (its base that
      PR's head branch) was closed when that one merged, before `land` retargeted (on 2026-11-03
      a swarm's `land 129 130` merged #129, GitHub closed #130, and the way out was by hand).
      Before pushing anything to it, `land` pushes that base branch back (at what the stack's
      record put the PR on), reopens the PR, retargets it onto `main` and deletes the branch
      again; when a step fails it stops with the exact steps left (`git push`, `gh pr reopen`,
      `gh pr edit --base main`, `git push --delete`, then the `land` to run again). A PR closed
      any other way stops. Either way `s-<pr>` and the record stay until the PR lands;
   2. it force-pushes `s-<pr>` and checks the remote branch is that commit (on
      2026-10-19 a PR merged without its fix, whose push hadn't landed); a push that didn't
      land is tried again, three times in all;
   3. it notes the gate's `CI summary` on the PR (from that log; `--summary` to pass one),
      and refuses a summary that isn't a pass. Then it retargets onto `main` every open
      PR whose base is this PR's source branch (`gh pr edit <n> --base main`), the stack's later
      PRs first, so deleting that branch once it merged closes none of them; a retarget that fails
      stops it before the merge;
   4. it waits until the PR is mergeable; one `behind` `main` is rebased on the server, and a
      branch the server can't rebase (merge commits), or one `stack` merged its base into, gets
      `main` merged in and pushed instead (semi-linear history needs `main`'s head);
   5. it merges with a merge commit at the head it saw, asking again while GitHub refuses (three
      tries), with
      `[skip ci]` in the merge commit when every file the PR changes is Markdown. Before each
      merge call it checks the server's head carries the gated change (the pushed commit, the
      same tree rebased, or the same patch; sc#9), else it stops and merges nothing;
   6. it confirms the state is `merged`, and nothing below runs before that;
   7. it logs `merge --issue <n> --mr <m> --sub gate-<n>` and runs `tidy <n>` for each issue the
      PR closes;
   8. it fast-forwards the owner's checkout when it is on `main` with no tracked changes;
   9. only then it deletes the source branch (on 2026-10-18 a branch deleted after a merge that
      hadn't happened closed its PR).

   Then it closes the gate (`gate --stop`, which drops the lock). A rerun after a failure goes on
   from the PR that stopped (an already pushed branch isn't pushed again). Nothing runs on
   GitHub after a merge: CI is local (owner, 2026-11-05).
   - **A red gate**. **Where the project turns it on** (`Hooks::gate_reruns`, sc#39; `--rerun`
     for one run elsewhere), **the gate reruns once by itself**: in the same invocation, as
     `gate-<n>-rerun`, a failure only at the tests that names its failed tests (each alone, as
     `--rerun-failed` below does), else a transient failure (below) whole, both results printed;
     a pass lands as `gate-<n>-rerun`, and `--no-rerun` turns it off for a run. When it still
     fails, or the project doesn't turn it on, read the failure's cause (the gate prints it, and its `.rc`
     and `sub.stop` name it). A project's own causes (`Hooks::extra_causes`, read from the log
     before the shared ones) are `gpu`, the GPU or the desktop under load, and `python`, no Python
     for its oracle tests; a project without them never sees them.
     - **transient** (`memory`, `link`, `toolchain`, `git-temp`, and a project's `gpu`):
       `--retry-transient` reruns the whole gate once, as `gate-<n>-rerun` (where the project
       turns reruns on, the gate has done that itself). `git-temp` includes git's `fatal:` about one of the
       testkit's temporary repositories (`swarm-test-*`) inside a test: a fixture, not the
       change;
     - **the gate machine's setup** (a project's `python`): a rerun fails the same way, so none is
       offered; fix the machine and gate again. It is not the change's fault;
     - **tests that failed under load** (cause `test`, `git-temp`, `memory` or `gpu`, and the gate failed
       at its `test` step, or the smoke gate's `golden` step): `cargo xtask gate --rerun-failed
       gate-<n>` reruns, on the HEAD gate-<n> tested, each test nextest's summary (for `golden`,
       libtest's `... FAILED` lines, counted by its `test result:`) lists as failed, **alone** (its own nextest run, one
       thread), then the full gate's doctests and the steps the failure skipped (`ci`'s
       `--test-alone`, `--doctests`, `--only`), as the sub-agent `gate-<n>-rerun`. When every one
       passes, its `.rc` is a pass that says so, `rc=0 … head=<the same commit> of=gate-<n>
       rerun=<binary id>::<test>,…`, and `cargo xtask land <pr>… --sub gate-<n>-rerun` lands on
       it, noting the rerun's summary, the tests that passed alone and gate-<n>'s own summary on
       each PR. A test that fails again alone, or any step that fails, leaves no pass: that
       failure is real. It is refused for a build or clippy failure (no flake), a HEAD that
       moved, a gate that passed or is a rerun itself, a failure nextest's summary doesn't name
       exactly (more than 12 tests included), and a second rerun of the same HEAD: gate the stack
       again then. It costs the failed tests and the skipped steps, not a second gate: on
       2026-10-22 a PR's gate failed only a panic_hunt test, which passed alone.
     - **anything else** (`build`, `other`): send the PR back, or fix forward.
     - **Gating the same stack again** (sc#20) is its rerun, flag or none: when the last gate in
       this worktree failed and the next gates the same PRs at the same heads onto the same
       `main` (restacked or not; or the same `--merge` branches at the same commits), `gate`
       starts it as `rerun of gate-<n>: …` itself, so a flake it answers sends no PR back. A new
       push, another PR, a moved `main` or a gate of another stack between makes it a gate of
       its own; so does reusing gate-<n>'s own `--sub`.
   - **Stop on red**. While `main` is red, nothing merges and
     no work starts from `main`. CI is local, for the coordinator and every worker: GitHub runs
     no workflows (owner, 2026-11-05; [swarm.md › What CI runs](swarm.md#what-ci-runs)). The
     periodic full gate files a `main-red` issue when `main` fails it ([The periodic full
     gate](#the-periodic-full-gate)), and the coordinator files one at once when a local gate
     shows `main` itself broken (what broke, the gate's log, the merges since the last green
     gate), so every agent holds; fixes forward or reverts the batch that broke it, through the
     same local gate; and closes the issue when `main` is green again. While one is open, `cargo xtask claim` refuses to start new work (it names the issue), but for the
     coordinator's own `--no-pr` claims and a claim of the `main-red` issue itself, `cargo xtask
     land` refuses to merge but for the fix (the land step above), and `claim
     --check` (`ci --changed`, the pre-push hook) warns that a failure may not be the branch's own.
   - **The gate runs in the coordinator's own worktree** (its target dir inside it; where the
     project keeps it is its own, such as `<the clone>-coord` beside the clone, or
     `.claude/worktrees/gate` inside it), never in the owner's checkout; after each merge the coordinator fast-forwards the
     owner's checkout when it is on `main` with no tracked changes ([Parallel
     agents](#parallel-agents)).
   - **Four kinds of gate** (owner 2026-10-21; the smoke), and a fifth for a PR readied
     without its run (changed): `cargo xtask gate` picks one from
     the stack's change against `origin/main` and `ci --changed`'s plan for it:
     The project's planner (`Hooks::gate_kind`; [swarm.md › The gate's kinds
     here](swarm.md#the-gates-kinds-here)) decides docs, tooling or full; the coordinator runs
     full as the smoke unless `--full` forces it. What each kind takes is the project's:
     - **docs:** only what the planner counts as docs (only Markdown, say; a project
       may take its templates and scripts too); runs `cargo xtask ci --changed --base origin/main`;
     - **tooling:** only what the planner counts as tooling, and the plan builds no
       crate but xtask (no product crate depends on it); runs `ci --changed --base origin/main` too;
     - **smoke** (owner, 2026-10-22): anything else (a product crate,
       the workspace manifest, `.config/nextest.toml`, `clippy.toml`, a new
       top-level file; a planner may judge a `Cargo.lock` change by the crates it reaches), and
       whenever it is unsure (no plan, no change seen); runs `cargo xtask ci
       --smoke`, the project's smoke within the `ci` contract: `fmt --check`, `clippy -D warnings`
       on the workspace, a build of the workspace but xtask (the exe running it), `layers`, the
       golden tests, a short headless run of the project's CLI and, where the project runs them,
       the generated docs' checks ([swarm.md › The gate's kinds
       here](swarm.md#the-gates-kinds-here) lists this repo's steps).
       That is mostly build time (about 2 minutes warm on a Mac, in one project): no full test run. Clippy is in
       it on purpose: a stale `#[expect]` exemption where two PRs meet is a stack's most common
       failure, and no test run catches it. The whole `ci` moved to [the periodic full
       gate](#the-periodic-full-gate) of `main`.
     - **full:** `--full` forces it: the whole `cargo xtask ci`. **Only when the author's own
       run can't speak for the gating machine** (owner, 2026-10-23): its
       `ci --changed` ran on another platform (a Mac or Linux worker: `cfg(windows)` code is then
       only linted, by `clippy-windows`, never tested), its plan left out tests the change
       reaches, or its tests fail there for reasons of their own. Otherwise, however large the
       change, the smoke and the periodic full gate of `main` are enough. The gate's note says
       why (`--note "local gate: #<m>; full: tested only on macOS"`). A project whose gate runs GPU tests has one
       more case, GPU tests run only on another GPU; a full gate still costs a whole `ci` (10 to
       15 minutes, longer with GPU tests) of the shared desktop for what the
       author's run already showed.

     - **changed** (sc#18): the smoke of a stack holding a PR readied without its author's run
       (`ready --no-ci`: the PR's latest `submit` event's note begins `no-ci`) runs `cargo xtask
       ci --changed --base origin/main` instead, the run that `ready` skipped, so a test run
       stands behind every merge. `gate` finds the PRs in the stack record (or the issues of the
       branches `--merge` names) and reads their submits on the tracking branch; its note names
       them (`kind=changed (ready --no-ci: #<m>)`). The docs, tooling and full gates run `ci
       --changed` or more already, and stay. A tracking branch it can't read is a warning, and
       the smoke stays.

     A stack mixing kinds runs the strongest one any of its PRs needs, since the change is the
     whole stack's (a tooling PR stacked with a product one gets the smoke, and its xtask tests
     wait for the full gate). The kind goes in the gate's notes (`capacity=<n> free=<GB>; kind=<kind>
     (<why>)` in `sub.start`, `kind=<kind>` in a failure's and a pass's `sub.stop`) and its `.rc`.
     `--merge <branch>` (repeatable) gates branches merged onto `origin/main`
     instead of the stack. The periodic full gate stays the backstop.
   - **Docs-only merges** (owner 2026-10-16): a PR whose every file
     is Markdown (the progress folds, docs fixes) is merged with `[skip ci]` in its merge commit,
     inert now that GitHub runs no workflows.
     Its gate must pass, and a doc a test reads tests its reader (in one project a
     change to `gaps.md` or the scorecard runs xtask's tests). Its `ci --changed` run is a gate too: `sub.start --sub gate-<n> --name gate
     --note "docs gate: #<m>"`, the run, the merge logged with `--sub gate-<n>` (when it closes an
     issue), then `sub.stop`, so `swarm.lint` finds no merge outside a gate.
   - **Then the label goes**: GitHub closes the issue on the merge but leaves its labels,
     so once the merge is confirmed, `cargo xtask tidy <n>` removes `status::in-progress` from the
     issue it closed (it waits up to 30 s for GitHub to close it; one still open keeps it).
     `cargo xtask land` runs it right after it confirms a merge, so the scheduled merge check
     does too. On 2026-10-20 115 closed issues here, and 101 in another swarm,
     still carried the label, so a board of work in progress was wrong.
4. **Record progress, once per batch:** fold the merged PRs' Progress sections into their gap
   entries in `docs/gaps.md`, the only home for detail, in one commit; touch `ROADMAP.md` and
   `docs/roadmap.md` in that commit only when a headline number, a milestone or the ranking
   moved, one line an item linking its gap ([progress-docs.md](progress-docs.md) rules 6 and 8); then the project's own batch steps, in the same commit: its measurements and its
   generated docs ([swarm.md › Each batch](swarm.md#each-batch)).
   Then `cargo xtask tidy` (no issue) clears the label from any closed issue a merge, a duplicate
   or an abandoned claim left it on (`--dry-run` lists them).
5. **Restock, within capacity:** start a worker only while `cargo xtask capacity` shows room for
   another build, counting its own gate, and at most three per swarm on the owner's desktop. Then: split large issues into tasks one worker can finish in a session; open issues for
   newly unblocked work, with capability labels; update **Current focus**.
6. **Release stale claims:** a draft PR with no new commits for 24 hours, or one you hand to
   another worker sooner (its holder gone quiet, what it waited on landed):
   `release --issue <n> --of <its instance> --reason stale` (step 8), and a comment naming who
   takes it over. That worker's `cargo xtask claim <n> --take-over` keeps the PR and its branch
   ([Picking work](#picking-work), step 3).
7. **Claim its own work:** `cargo xtask claim <n> --agent <its instance> --no-pr` (the label, the
   comment `claimed: coordinator` and the claim event, refused when another agent holds it)
   before starting it, however small. **While it merges,** its claims add `--no-fullgate`.
   **With nothing to merge,** it takes tasks like a worker (owner, 2026-10-22) and claims
   without it, so on a `full-gate` machine the periodic full gate of `main` runs when it is
   due, once the claim holds its issue.
8. **Log to the timeline** with `--role coordinator`: the worker events for its own issues, and
   `merge --issue <n> --mr <m>`, `sendback --issue <n> --mr <m> --note "<why>"`,
   `release --issue <n> --reason stale`, with `--of <instance>` naming whose claim it frees
   (its `claim` event on the tracking branch says which instance).
   - **Workers it starts** get its instance id in their brief and pass `--parent <it>` to their
     `session.start`, so their lanes nest under the coordinator's, with `--model <its model>
     --effort <its effort>`, so the owner can compare models.
   - **Its helpers** (each local gate run, a search, a review) are sub-agents in its own file:
     `sub.start --sub gate-<n> --name gate --note "local gate: #<m> …"` (`--model` when it runs on
     another model) before, and `--sub gate-<n>` on any issue event the helper causes
     (`merge --issue <n> --mr <m> --sub gate-<n>`, `sendback --issue <n> --mr <m> --sub gate-<n>`).
     `cargo xtask gate` logs its own `sub.start` and, on a failure, its `sub.stop` with the
     convention in [tracking-format.md › Gates](https://github.com/mornycorp/swarm-core/blob/main/docs/tracking-format.md#gates): the capacity it started
     with, then `rc=<n>; cause=<cause>` (`memory`, `link`, `toolchain`, `git-temp` and a
     project's `gpu`: transient, a rerun may pass; a project's `python`: the machine's setup;
     `test`, `build`, `other`). A failed gate keeps the lock, as the coordinator
     still owns the worktree. Where the project turns reruns on (`Hooks::gate_reruns`; `--rerun`
     for one run), it reruns once by itself: a failure only at the tests, its failed tests alone
     and the steps they skipped; else a transient failure, the whole gate; as `gate-<n>-rerun`
     either way (`--no-rerun` turns it off for a run). Elsewhere `--retry-transient` reruns a
     transient failure once, as `gate-<n>-rerun`, and `--rerun-failed gate-<n>` reruns only its
     failed tests, alone, and the steps they skipped, as `gate-<n>-rerun` too (step 3, a red gate).
     A gate needn't log `work` on the issue it checks; if it does, the issue stays awaiting merge
     on the Board: a helper's claim, work or stop never moves an issue.
     **A gate stays open through the merges it allows:**
     each is logged `merge --issue <n> --mr <m> --sub gate-<n>`, and only then
     `sub.stop --sub gate-<n> --note "rc=0; merged #<m> …"`, once, so the gate's merges
     sit on its lane (`cargo xtask gate --stop gate-<n> --note "merged #<m> …"` logs it and drops
     the lock). `cargo xtask event` drops a second `sub.stop` of the same gate, and any
     event its agent's file already holds (a re-send).
   - **Its own issues** get `work` right after the claim and on every resume, as a worker's do
     (`cargo xtask claim` logs `claim` and then `work`).
   - **Duplicate work it ends** (two agents on one issue; it stops the later one):
     `release --issue <n> --of <that agent's instance> --reason duplicate --note "<who has it>"`,
     so the inspector counts that agent's work so far as thrown away. Close a duplicate issue on
     GitHub as a duplicate, with a comment `Duplicate of #<original>`.
   - **A dead agent's session** (its machine rebooted or its process killed, so it never logged
     `session.stop`; checked dead, not just quiet: no commits, comments or events, and its owner
     or machine says so): release what it held as above, then
     `session.stop --of <that agent's instance> --note "<how you know>"`, so its lane stops
     reading as live. **Each batch**, list the candidates: `swarm.lint`'s
     `deadNotClosed` findings, or `swarm.instances {now}`'s lanes `dead` with `death.cause`
     `silent` (in session, silent past a day while the swarm went on: the inspector already
     draws them cut where they were last seen); check each as above, then release and close it.

   The writer refuses what the inspector would drop: `--sub` on a session event, `--role`
   anywhere but `session.start`, `--name` anywhere but `session.start` or `sub.start`, `--parent`
   anywhere but `session.start` (or naming the session itself), `--of` anywhere but `release`
   or `session.stop` (or naming the logger itself), `--reason duplicate` anywhere but `release` or
   `session.stop`, `--model` or `--effort` anywhere but `session.start` or `sub.start`, an
   `--effort` other than `low`, `medium`, `high`, `xhigh` or `max`, ids that aren't letters, digits, `-`, `_`, `.`, and a flag given twice.
9. **Keep the two swarms in step** (owner, 2026-10-18; Swarm Core): the swarms
   that share Swarm Core write one tracking format and share one process.
   - **What Swarm Core holds changes there once**: its commands, the tracking
     format (`docs/tracking-format.md`, the contract), this protocol and the other pinned assets
     (`docs/swarm-core-assets.md`). Such a change is a Swarm Core PR, then a bump in each
     consumer (`cargo xtask swarm-core bump`, then `swarm-core sync-assets`); the bump in the
     other repo is its twin issue. A module or document under migration is frozen in both
     consumers: changes that can't wait go to Swarm Core.
   - **A change to one swarm's own process** (its AGENTS.md, its docs/swarm.md, its own xtask
     commands) gets a **twin issue in the other repo** when the other needs it too: labelled
     `sync` (the pinned `scripts/github-labels.sh` makes the label in every repository) and titled
     `[sync] …`, linking the original and saying what to port.
   - Each batch, list the open `sync` issues in both repos (`gh issue list --repo <repo> --label sync`)
     beside the progress fold, restock from ours, and port theirs.
10. **Schedule an architecture review** (owner, 2026-10-23): after the
    first ~20 merges of a new product or a major area, then at each stage gate (alpha, beta,
    release candidate; `docs/roadmap.md`). One swarm's untyped command contract was found by a review
    only after about a week and 100+ commands, and fixing it took seven slices
    and five follow-ups; found at the 20th merge it would have been a few commands' work.
    - **An issue the coordinator files** (template **Task**, titled "Architecture review: <product
      or area> at <milestone>"), taken by one agent like any task.
    - **Read-only:** the review changes no code. It reads the crates, the docs and the merged
      PRs, and ends in one issue per finding (evidence with file paths and counts, a proposal,
      an estimate), each linked from the review's issue, which it then closes. The coordinator
      ranks the findings into `docs/gaps.md` like any other.
    - **It works from the checklist of known traps,** each one a mistake a swarm has already
      paid for ([lessons.md](lessons.md)):
      - untyped contracts at a boundary: command params and results, protocol messages, errors;
      - a consumer (the UI, the CLI, MCP) reaching past the command layer into engine internals;
      - consumers keeping their own mirrors of engine types;
      - persisted data (settings, UI state, caches) without a version;
      - stringly-typed errors, classified by their message;
      - work on the UI thread that should be a job; shared state with no owner;
      - the robustness traps (lessons.md 5–12): a child process started outside the one
        process module, or with no deadline; a second lock taken in a statement that holds
        one; a file read while drawing; defaults saved over state that couldn't be read;
        memory changed before its save succeeded; a secret in a result, a log or an error; a
        discarded `Result`; a test that touches the real config dir;
      - logic the spec or the architecture gives the core (layout, hit-testing, interaction
        state, a pixel-to-model mapping) built in a front end, because the core piece isn't
        there yet;
      - a requirement in the spec (a table row, a bullet) with no code and no open issue, under a
        gap marked done: the review reads the spec line by line against the code, not against
        the gap list.

      A trap it finds that isn't on the list goes on it, and into [lessons.md](lessons.md), with
      the issue that found it.

## The gate daemon

Steps 2 and 3 of [the coordinator's loop](#the-coordinators-loop) are mechanical, and run by hand
they waited on the coordinator's wakes and slipped (owner, 2026-11-07, sc#44): two ready PRs that
conflicted with `main` sat unnoticed for about 8 hours (the ready watch reports only PRs newly
ready), a gate stood idle or waiting for about 45 minutes in a day (restacks while it waited for
room, and the coordinator's wake latency), and the coordinator stacked heads a worker was still
pushing, ran git in the gate worktree mid-gate, killed gates mid-compile (their stale incremental
state broke a later link) and hit stale rerere replays. `cargo xtask gate --daemon` runs them
instead, the same way every time.

**What it does,** each round (every `--interval` seconds, 60 by default):

1. **Pauses** while its off switch is set, a `main-red` issue is open, or GitHub can't say,
   saying so once per change; it goes on when they clear. CI is local, so there is no pipeline
   of `main` to read: a red `main` reaches it as a `main-red` issue, or by its own check (step 7).
2. **Lists the ready PRs** through GitHub: open, not drafts, into `main`, their Progress
   filled in, not a progress fold's (`prwatch --ready`'s test). One GitHub reports as conflicting
   with `main` (`dirty`) is looked at (`git merge-tree`): a conflict `stack` can't resolve gets a
   note, once per head, and waits for its worker, so a stuck ready PR is always flagged; one in
   the generated docs alone, which `stack` regenerates, is gated like any other.
3. **Orders them:** a PR after every open PR whose commits its branch holds, then by the time
   it was first seen ready. One that holds a draft's commits waits (they would land unchecked).
4. **Stacks them** with `stack`'s code, rerere off (`GIT_CONFIG_PARAMETERS='rerere.enabled=false'`
   for the daemon's whole process tree; plain `stack` is unchanged). A conflict drops that PR
   and every PR stacked on it, and stacks the rest. When the PR conflicts with `main` itself
   (`git merge-tree`, no checkout), its worker gets one note per head, "conflicts with main in
   <files> (after #N landed): rebase onto main and run `cargo xtask ready`", and the daemon
   logs `sendback … --note "cause=conflict: <files>"`; one that conflicts only with the PRs below
   it waits for them to land. A PR `stack` refuses for another reason (no work, say) is held
   at that head.
5. **Folds, then gates.** While there is no room for the gate's build (the machine gate lock held
   by another gate, or `capacity` says none), the stack waits, up to `--fold` seconds (120), and
   a PR that becomes ready meanwhile is folded in by restacking, which is free. Then it runs
   `cargo xtask gate --sub gate-<n> --wait --rerun` (numbered on from the gate logs and the
   tracking branch, whichever is higher) as a child,
   and the stack is frozen: **the daemon never kills a gate**, and runs no git in the worktree
   while one runs. A gate that still waits for room after the fold waits itself.
6. **Reads the pushes made during the gate** after it ends, by patch-id, not SHA: a branch whose
   own change is the same patch (a rebase) lands as gated (`land` pushes the gated tree); a real
   change waits, with every PR above it, for the next round, and the PRs below it land.
7. **Lands a pass** (or a pass of the gate's own rerun of its failed tests) with `land`. A
   failure the machine caused (a transient cause the rerun didn't clear, or the gate machine's
   setup) drops nothing and is gated again next round; after three in a row the daemon sets its
   off switch for you. Any other failure is attributed: a build, clippy or fmt error names files,
   a failed test its package and test file, and the PR that changed them is dropped: put back to
   draft (as `prwatch --ready` re-drafts) with the log's excerpt in a note, a `sendback --sub
   gate-<n> --note "cause=<cause>: <evidence>"` logged, not gated again at that head, and the rest
   gated again (`regate of gate-<n>: …`). When no PR or several match, it gates the
   first half of the stack (`bisect of gate-<n>: …`); a stack of one that fails is that PR's.
   The bisection keeps its suspects, the failed gate's PRs, across gates and rounds: a half that
   passes lands, and the culprit is among the rest; one left is gated alone before it is sent
   back, since the others passing proves nothing of it when the failure took two of them: it is
   dropped as above when it fails alone, and when it passes too nobody is sent back (the failure
   was theirs together, or a flake). Several are bisected next, never stacked again with newly
   ready PRs. A suspect that moves or leaves ends the bisection. A
   stack that failed never lands. A `land` that stops at the same PR head twice holds it for you.
   **`main` red on the gating machine** (a GPU driver update, a change that landed by hand) looks
   like the PRs' failure, and with no server CI nothing else says so. Where the project turns it
   on (`Hooks::verify_main`, off by default; a project that runs the daemon should), a stack's first failure at its tests first runs those
   tests alone on `main` (`verify main for gate-<n>`, `gate --test-alone`, no PR: a plain
   gate of `main` is a docs gate of an empty diff, which tests nothing). When one fails there
   too, the daemon sets its off switch with the failed tests, files a `main-red` issue (unless
   one is open) and sends no PR back: fix `main` or the machine, close the issue, then `--resume`. When they pass, the failure is blamed
   as above, and so is one whose tests it can't name plainly. A failed test `main` doesn't have
   (no `fn <name>` there, or nextest's "no tests to run") is new in the stack, so its own: never
   checked on `main` nor read as `main` failing. It costs a build and those tests per
   failed stack. While paused, the daemon says why again every 30 minutes (`still paused: …`), so
   a watch on its log sees it.

Every decision (stacked, dropped, held, landed, paused) is printed with a time; those that change
state are on the timeline: the gate's own `sub.start` and `sub.stop` (its note naming the step,
`daemon gate:`, `regate of gate-<n>:` or `bisect of gate-<n>:`, and the PRs it gates), `land`'s
merges, and a `sendback` for each PR it drops. A PR a failure only sets aside (stacked on the
dropped one, or left out of a bisection's half) gets none: a gate that drops a PR it later lands
in a regate or bisection is not a send-back ([tracking-format.md › Send-backs](https://github.com/mornycorp/swarm-core/blob/main/docs/tracking-format.md#send-backs)), and `prwatch` logs a worker's rework only for a PR a `sendback` names.

**Running it.** On the gating machine, in the gate's worktree, with the gate's environment
(its `CARGO_TARGET_DIR`, `{{env}}_AGENT` the coordinator's instance, and whatever else the
project's gate needs: [swarm.md](swarm.md)), as a background process that outlives the
coordinator's turns:

```sh
cargo xtask gate --daemon --dry-run      # one round: what it would stack and gate; no note, no event, no state saved (safe beside a live daemon)
cargo xtask gate --daemon                # the loop; --once for one round
cargo xtask gate --daemon --status       # the switch, the pause, the PRs it dropped or holds, its latest decisions (saved with each, so a gate shows while it runs)
cargo xtask gate --daemon --stop --note "<why>"   # the off switch: it pauses at its next round
cargo xtask gate --daemon --resume       # clears the switch and the PRs it held
cargo xtask gate --daemon --retry <pr>   # forgets what it kept of one PR (dropped, held or noted), so it is gated again; restart the daemon after
```

`--stop` never interrupts a gate. The switch is read when a round starts, before each gate and
before a pass lands: a gate running when it is set runs to its end, no regate or bisection
starts after it, and a pass after it is closed without landing (its PRs are gated again once
resumed). To end the process, set the switch, wait for `--status` to say `paused`, then stop it.
Its state and switch are in `<target>/gate-daemon/`, with its process id: a second daemon in the
same worktree is refused while the first runs (a `--dry-run` may run beside it). While it runs, the coordinator doesn't
stack, gate or land by hand, and the ready watch and the merge-only scheduled task
([Staying awake](#staying-awake)) stand down: set the switch first to gate by hand.

**What stays with the coordinator:** `main-red` issues and their fixes (the daemon only
pauses), judgment calls (a PR held or dropped twice, a conflict to resolve by hand, a `--full`
gate, a `--while-red` landing), the batch's progress docs (step 4), and keeping the workers fed
(steps 5 to 9). Read `--status` on each wake.

## Staying awake

A coordinator session acts only when something wakes it: the owner's message, a background task
of its own finishing, or a worker it started reporting. Paused, or after its session restarts
(background tasks die with it), nothing polls, and ready PRs wait: three of one worker's sat
overnight on 2026-10-18 for that reason. So (owner, 2026-10-19):

- **While active**, keep a ready-PR watch armed: `cargo xtask prwatch --ready` in the background
  lists the open PRs every 2 minutes (`--interval`) and wakes the coordinator when one is
  ready (not a draft, its Progress filled in, its branch not `progress-*`). How it wakes
  it is the project's (`Project::ready_watch`):
  - **`Stream`**: it prints a line per PR as it becomes ready or is pushed,
    for as long as it runs, so the coordinator runs it under a monitor;
  - **`Exit`**: it exits as soon as one is new or pushed since the record it keeps in the
    clone's git directory, printing them, and after `--max` minutes (240) with nothing new it
    exits saying so; its exit wakes the coordinator, which re-arms it.

  A PR whose
  branch holds no work (no commits of its own, only its claim commit or an empty diff) is
  `ready but empty: #<m> …`, not `ready:`: `stack` and `land` refuse it. **The watch puts it
  back to draft itself**, with a comment saying why and what to do, and says `redrafted: #<m> …`
  (sc#42): a PR is ready only by `cargo xtask ready`, and one marked ready by hand (`gh pr
  ready`, the button) with nothing in it would otherwise reach a gate. It does this once per
  head: a PR marked ready again at the same head stays `ready but empty` and is the
  coordinator's to close or send back; a push is a new head. A PR from a fork is only
  reported, and `--no-redraft` only reports any. A re-drafted PR is no reason for `Exit` to
  exit. Each line names the PR's head commit (`ready: #<m> <sha> <branch> ·
  <title>`), so a PR pushed again is a new line. Re-arm it after
  every merge and after a restart; `--once` lists the ready PRs and exits (with `Exit`, marking
  the new or pushed ones and leaving the record alone).
  The watch runs its own `xtask.exe`, so give it a target dir of its own
  (`CARGO_TARGET_DIR=target/coordinator-watch`): Windows won't let cargo replace a running exe, and a
  watch in the gate's target dir stops every gate from rebuilding xtask (2026-10-20).
- **Always**, a scheduled task (the app's scheduled tasks, every 30 minutes, at the minutes the
  project picks; they survive restarts
  and run on the next launch if the app was closed) starts a fresh, **merge-only** coordinator
  session. It does nothing unless a PR is ready. If one is, it logs its own `session.start`
  (`--role coordinator --parent <the coordinator's instance>`), checks `cargo xtask capacity`,
  gates and merges as step 3 says, starts no new work, and stops. Its prompt is self-contained:
  each run remembers nothing.
- **One coordinator gates at a time.** A coordinator takes the gate lock
  (`cargo xtask lock take --agent <instance>`: in the clone's common git directory, so every
  worktree of the clone shares it; refused while another holds one younger than
  {{gate_lock_max_age}}, the project's `Limits::gate_lock_max_age_secs`, while
  the same holder may take it again, which renews it) *before* touching the worktree (a rebase is already a
  write), and drops it when its merges are done (`cargo xtask gate --stop`, or `lock drop`;
  `lock show` says who holds it, and where). Stacking and landing are commands too (`cargo xtask stack`,
  `land`), so the old helpers in `temp/coordinator/` are retired.
- **One gate builds at a time on the machine, either swarm's**.
  Each swarm's own gate lock lives in its repo, so it never excluded the other swarm's gate. On
  2026-10-21 one gate's links held about 30 GB of the 32 GB desktop.
  - `cargo xtask gate` also takes a **machine-wide lock** outside either repo:
    `%LOCALAPPDATA%\swarm-gate\gate.lock` (else `$XDG_STATE_HOME` or `~/.local/state`;
    `SWARM_GATE_LOCK` names another file, for tests). With no such directory there is no lock:
    the gate warns and goes on.
  - The line is `<swarm> <instance> <time> <worktree>`, the same in every swarm, its time
    ISO-8601 UTC (`2026-10-21T14:00:00Z`). A lock is stale after 3 hours, one value for every
    swarm since the file is shared. A line that can't be read is a lock of nobody's, aged by
    the file's time. The same swarm and instance may take it again, which renews it.
  - It is taken whole: the line is written beside the file, then hard-linked to its name, which
    fails when the file exists, so of two gates that find it free at once, one gets it.
  - It is held only while the gate's `ci` builds and runs, and is dropped after, pass or fail.
    The gate takes the repo's lock first, then the machine's: a gate refused the machine lock
    (or capacity) keeps the repo's lock (`cargo xtask lock drop` when done).
  - With `--wait`, a gate waits for the other holder up to three hours, looking every minute;
    without it, the gate refuses. `stack` and `land` never take it: they build nothing.
  - `lock show` prints both locks. `lock drop` drops this instance's machine hold first, then
    the repo's lock, so a repo lock held by another (and refused) never keeps the machine hold;
    with `--force` it drops any instance's machine hold of this swarm, never the other swarm's.
  - **Only the gates that build take it** (`Kind::builds`): `tooling`, `smoke` and
    `full`. A `docs` gate (about 30 s, xtask alone) runs without it, so it never waits behind the other
    swarm's full gate, and its `sub.start` note ends `; machineLock=skipped`. On 2026-10-22 the
    batch-18 fold waited more than 10 minutes behind the other swarm's full gate.
  - **Nor does a docs gate wait for build capacity**: it builds nothing worth
    room, so it is never refused for capacity or waits for it; its `sub.start` note still records
    the capacity it saw. One rule, `Kind::builds`, decides both the lock and the capacity. A
    gate that builds checks capacity once, when it starts (again for a rerun); without `--wait`
    it refuses when there is no room.
  - With `--wait` a gate waits for room up to two hours, looking every minute.
  - **A gate has priority over workers' builds** where the project turns it on (sc#41,
    `Hooks::gate_priority`, off by default): with `--wait` it waits for room at most
    2 minutes, then goes ahead of the running builds, saying `going ahead with priority`; its
    build jobs follow free memory (below). On 2026-11-07 a gate waited about 25 minutes behind
    six workers' builds (one worker's helpers) with six PRs ready. Only a disk under the floor
    (20 GB, or unread) keeps it waiting, up to two hours, then it refuses: a gate goes ahead of
    builds, never of a full disk. Meanwhile `capacity` reserves a build for it for everyone else
    ([Helpers in worktrees](#helpers-in-worktrees)).
  - The gate runs `ci` with `CARGO_BUILD_JOBS` set from free memory: one job per 1.5 GB, at
    least 2, at most the cores; half the cores when free memory can't be read. A
    `CARGO_BUILD_JOBS` the environment sets wins. A test binary's link took about 1.4 GB.
- **Test runs share the machine** (sc#17): the machine lock kept two gates apart, but a worker's
  `ready` took none, and on 2026-11-03 a `ready` and a `claim`'s periodic full gate of `main`
  tested at once and locked up a 24 GB unified-memory Mac (about 20 GB wired as GPU memory).
  - **Who:** every run of `ci` that builds and tests holds one of the machine's **test-run
    slots** while it runs: a `gate` whose kind builds (`Kind::builds`: not docs; the periodic full
    gate's runs are the gate's, kind full) and `ready`'s `ci --changed` (not for a docs change).
    Dropped when the run ends, pass or fail.
  - **How many:** `SWARM_TEST_RUNS` (a whole number 1 to 8, the machine's own, read by every
    swarm, as they share it), else one per 16 GB of memory (4 GB added for what the machine keeps
    for itself), at least one: one on a 24 GB Mac, two on a 32 GB desktop, where two gates at
    once passed. One figure for the machine, not a project's: the slots are shared.
  - **Where:** `test-runs/slot-<i>` beside the machine lock, in the same state directory
    (`SWARM_GATE_LOCK` moves both). The line is `<swarm> <instance> <pid> <time> <what>
    <worktree>`, the same in every swarm; `what` is the run's sub (`gate-12`, `ready-<sha>`,
    `gate-full-<sha>`). Taken whole as the machine lock is (written beside, hard-linked to a free
    slot's name), only while fewer than the limit are held, and only among slots `0` to
    `limit - 1`.
  - **Left behind:** a slot whose process no longer runs (`ps`, `/proc`, `tasklist`), or older
    than 6 hours, is taken over by the next run: a killed run needs no `lock drop`. A line that
    can't be read is still a slot, aged by its file's time. `lock show` lists the holders.
  - **Waiting:** `ready` waits for a slot (up to three hours), saying who holds them, again when
    that changes; `gate` waits with `--wait`, else refuses (keeping the repo's lock).
  - **The order, so nothing deadlocks:** the repo's gate lock, then the machine lock, then a
    test-run slot, then capacity (which doesn't count the asker's own slot); never another
    order. `ready` takes a slot alone, and nothing waits for a lock while it holds a slot.
  - **The periodic full gate** starts only when `capacity` shows room, so not while every slot
    is taken, and not for a `main` whose tip already passed a full gate within the drift
    interval ([The periodic full gate](#the-periodic-full-gate)).
- **Only `stack`, `gate` and `land` touch the gate's worktree**. A gate may be running there
  at any time: a scheduled run's as well as your own. A `git checkout` there, typed by hand,
  swaps the tree under that gate and voids it. This happened twice on 2026-10-21, once through a
  `;` chain and once through a pipe that read "lock held by …" as a success.
  - The lock records the worktree it was taken in. `cargo xtask claim` won't switch a worktree
    another holder's live lock names, and says why.
  - The coordinator's own changes (progress folds, issues it takes itself) go in a **second
    worktree** of its own, with its own target dir, never the gate's. Where is the project's
    (`<clone>-coord-work` beside the clone, or `.claude/worktrees/gate2` inside it).
  - A step that must be by hand chains on `cargo xtask lock take` itself: `lock take && …`,
    never through a pipe and never after a `;`.
  - **No pipe in an `&&` chain without `pipefail`**. This covers any `&&` chain
    of xtask commands (`lock`, `stack`, `gate`, `land`, `claim`), not only steps by hand.
    - A pipeline's exit status is its last command's: `stack … | tail -3 && gate …` runs `gate`
      even when `stack` was refused, because `tail` succeeded.
    - Run such a chain unfiltered, or start it with `set -o pipefail;`.
    - On 2026-10-22 the coordinator filtered `stack 212 214 && gate && land` to trim SSH noise.
      A scheduled run held the lock, and the chain went on past `stack`'s refusal.
      `gate` and `land` refused on their own checks, so nothing changed.
  - **`lock show` before a stack, gate and land chain:** a scheduled run may already be gating
    the same PRs.
- **"No new work"** (the owner's word): the coordinator only gates, merges, folds progress and
  watches `main`; it files nothing and starts no workers. To hold issues already filed, it labels
  them `status::blocked` with a comment saying why: `cargo xtask claim` refuses a held issue.
  The owner's word lifts the hold; the coordinator takes the label off.
- **Pausing and resuming** (owner, 2026-10-19). When the owner pauses the coordinator, both
  timers go off: it stops its ready watch, **disables** the scheduled merge check, and logs
  `session.pause`. Ready PRs then wait, which is what a pause means. When the owner resumes it,
  its first act is to turn both back on: **re-enable** the scheduled check, **re-arm** the
  ready watch, and log `session.resume`. Then it looks for PRs that came ready while it was
  paused. After a session restart, whose background tasks died with it, the ready watch is
  re-armed the same way; the scheduled check survives a restart and stays as it was.

## The periodic full gate

The owner's direction, 2026-10-22, for both swarms: **unload the
coordinator.** It merges after the [smoke](#the-coordinators-loop) (step 3), and the whole
`cargo xtask ci` moves to a periodic full gate of `main`, run by whichever agent with the
[`full-gate` capability](swarm.md#capabilities) is about to claim a task when `main` has gone
unverified too long. Workers keep testing their own branches, with `cargo xtask ci --changed`
(AGENTS.md §5). `cargo xtask fullgate` (Swarm Core's `fullgate`):

- **What counts as a pass** (sc#15): the periodic gate's own, and **a landed `--full` gate**.
  When the coordinator's `gate --full` (the whole `ci` on the stacked tree) lands and `main` is
  then exactly the tree it tested (every PR of its stack landed onto the `main` it was stacked
  on), `land` closes it with `main=<its commit>` in its stop note, and that commit of `main`
  counts as verified. A stack landed in part, or onto a `main` that moved meanwhile, counts for
  nothing.
- **When** (owner, 2026-10-29, sc#15: "Our periodic hourly gate feels unnecessary with all of
  this testing"):
  - `main` moved since the last pass, by merges no landed full gate verified (a smoke's, a docs
    or tooling gate's, a stack landed in part), and that pass is **more than an hour old**
    (`MIN_AGE`);
  - or `main` is unchanged, but the last pass is older than the **drift interval**: a day by
    default, the project's `Hooks::full_gate_drift_secs`. A driver, a reference build or the
    machine may have moved while `main` didn't, and a `--full` gate may not read what the
    periodic one does (a project's `Hooks::full_gate_env`);
  - or the inputs the gate reads changed since the last pass: the project says why
    (`Hooks::full_gate_inputs_changed`: a driver, a reference build, pinned tools), or
    `{{env}}_FULLGATE_DRIFT="<why>"` on the claim that should run it.

  Never while one is open, nor for a commit of `main` that failed one since its last pass for
  the change's cause: its `main-red` issue covers it until `main` moves; one that failed for the
  machine's is due again an hour later. Within the drift interval an unchanged `main` is never
  gated twice, and a run of `--full` batches leaves nothing to gate.
- **Who:** `cargo xtask claim` runs it, on a machine that can and has room for a build, when
  it is due (the coordinator's claims while it merges pass `--no-fullgate`; a claim while a
  `main-red` issue is open runs none).
- **The claim holds the issue while it runs**: the claim's order is its
  checks; the branch from `origin/main` and the draft PR; the label; the comment; the `claim`
  and `work` events; the race check; **then** the full gate; then the report and the worktree's
  switch (`claim::plan` in Swarm Core). On 2026-10-22 a claim that ran the gate
  first left its issue looking free for 18 minutes, another agent claimed it, and the first
  agent's work was thrown away. The branch is cut before the gate: the gate tests `main`'s
  commit, not the branch, and a red result releases the claim anyway. When `main` is red after
  the gate (it filed `main-red`, or one is open), the task's claim is released: its draft PR
  closed (the branch is kept, for a claim again once `main` is green), the label off, a
  `release --reason main-red` event, and a comment `released: <name>, <why>`. `cargo xtask
  fullgate --check` says whether one is due and
  why: `main`, the last passed full gate's commit and age, whether `main` moved since, any open
  or abandoned start. `cargo xtask fullgate` runs one when it is due.
- **Tell the owner** (owner, 2026-10-22): a whole `ci` takes 10 to 15
  minutes on a cold checkout, longer in a project whose gate runs GPU tests
  ([swarm.md](swarm.md) gives the project's), and holds the desktop's machine gate lock, so the agent whose claim
  runs it says so at once, when the claim prints `fullgate: <sub> won the race for <commit>`:
  **"My turn for the periodic full gate of main (`<commit>`): the whole `ci`, about <n> min."**
  When it ends: **"Full gate done: passed."**, or **"Full gate done: failed (`<cause>`), filed
  `main-red` #<n>; working on that instead."** The agent runs the claim where its output reaches
  it as it prints (the foreground, or a background run to a log it reads; never under a monitor,
  which stops it at its timeout), never piped through `tail` or `head`, which hold every line back
  until the claim ends.
- **One at a time, across agents:** each agent writes its own tracking file, so pushes never
  conflict and the tracking branch gives no mutual exclusion. Two steps do:
  - **The race**: push the start event, read the tracking branch again, and the earliest
    open start goes on (by its time, then its instance id, so every agent picks the same).
  - **The lock**: each agent's own clock writes its start's time, so two agents could
    each see their own start first and both run. The one that goes on must also take
    `refs/fullgate/lock` on the remote by compare-and-swap (`git push
    --force-with-lease=<ref>:<the commit it read>`), and the server grants that to one, whatever
    the clocks say. The ref is outside `refs/heads`, so no branch, PR or workflow sees it.
    - The lock is a commit of the empty tree whose message names its holder: `instance=<id>;
      sub=<sub>; commit=<main's commit>`.
    - It is **never released**. The next gate swaps out a holder whose gate stopped, or whose
      start is three hours old (stale); one whose start it can't see yet is stale three hours
      after it took the lock. A holder that already ran this same `main` keeps it, so `main`
      isn't gated twice.

  The others log a stop, `superseded: <who> began a full gate of <commit> first` (or `<who>'s
  <sub> holds the full gate's lock`), and go on to their tasks. A start with no stop after
  **three hours** is abandoned, and anyone may take over.
- **What:** the whole `ci`, `main`'s own `cargo xtask ci`, on a clean checkout of `origin/main`
  at the commit it recorded: the project's `Project::full_gate_worktree`, `{{full_gate_worktree}}`,
  under the primary checkout (gitignored), shared by the clone's agents (who take turns in it), with its own `target/` so
  its builds stay incremental. The winner then waits for the machine-wide gate lock and
  room for a build, as `gate --wait` does, so it never runs beside either swarm's gate. Holding
  the lock, it **reads again** (sc#36): when another gate passed `main` while it waited (another
  full gate, or a landed `--full` gate), it steps aside, `superseded: <who>'s <sub> verified main
  (<commit>) while this gate waited for the machine`; when `main` moved, it stops,
  `superseded: main moved to <commit> …: gating it instead`, and begins again for the new tip
  (a few times at most); else it runs. It sets
  `CARGO_BUILD_JOBS` as `gate` does, and leaves its log, `.rc` and `.junit.xml` (its tests'
  times) in that checkout's `target/gate-logs/`.
- **Whose code**: the claiming agent's xtask, which may predate `main`,
  only decides that a gate is due, races for it and checks the commit out. It then hands over:
  `cargo run -q -p xtask -- fullgate --run <sub> --commit <sha> --agent <id>` in the checkout, so
  **`main`'s own xtask**, built there, runs the gate (the machine, the attempts, the rerun, the
  pass logged or `main-red` filed). Its output is passed through, and it ends with a
  `fullgate-outcome:` line, a stable format, as the two sides may be built from different
  commits: `passed <commit>`, `machine <cause>`, `red <issue>`, `red open` (one was open
  already), `not-due` or `yielded`. The claimant reads the last such line; a caller built before
  the move reads the `<sub>.outcome` file it still writes beside the log. When `main`'s xtask
  leaves no outcome (it doesn't build, say), the claimant runs the gate with its own code, so a
  broken `main` is still gated and filed. The claimant then claims a `main-red` issue the gate
  filed in its own worktree: the gate only
  files it, so the shared checkout stays detached at the gated commit
  whatever the outcome, and the next gate's checkout detaches it again, cleaned, whatever was left
  there. A change to the full gate takes effect with the commit it gates, not as each agent
  happens to rebuild.
- **Pass:** it logs the stop with the result, then the claim goes on.
- **Fail:**
  - **Tests that failed under load**: when nextest names the failed tests
    and `gate::rerun::plan` finds a rerun path (the failure was at `test`, cause `test`,
    `memory`, `git-temp` or a project's `gpu`, no compile error), it first reruns those tests, each alone, with the
    doctests and the steps the failure skipped, once, as `<sub>-rerun` (`sub.start` note `rerun
    of <sub>: its failed tests alone (<n>); …`). A pass is logged `rc=0; reran <n> failed tests
    alone; …`; a failure goes on as below, with no second rerun. No plan says why
    (`fullgate: no rerun of <sub>'s failed tests alone: …`).
  - **The machine's cause** (`memory`, `link`, `toolchain`, `git-temp`, a project's `gpu`): it
    reruns once, as `<sub>-rerun`, and never files anything for it.
  - **The gate machine's setup** (a project's `python`): no rerun, as it would fail the same way,
    and nothing filed: the machine needs fixing, not `main`.
  - **A real failure** (`test`, `build`, `other`): it opens a **`main-red`** issue, which stops
    new claims and merges (step 3). The issue names the failing commit, the last green one
    (the last passed full gate's) and the merges between them (`git log --first-parent`), the
    cause and the failing tests, and where the log is. The gater claims it and works on it
    instead of its task. An open `main-red` issue stops a second agent filing a duplicate.
    - **A timeout repeated alone is proof** (owner, 2026-11-05, swarm-core#29). A test a deadline
      ended (nextest's `terminate-after`, a test's own timeout) in the run and again in its rerun
      alone is slow, not loaded: the rerun alone already ruled load out. Don't run it again, whole
      and with no limit, "to rule out load". Instead:
      1. **Time one unit** of it (one event, one capture, one input), on this commit and on the
         last green one, and compare.
      2. **Bisect** the merges since the last run that verified it, timing that one unit.
      3. **Fix it**, then measure the whole test once, on the fix.

      A whole uncapped run is worth its time only when the rerun alone **passed**, which points
      at load. On 2026-11-05 a swarm's uncapped run of such a test took 75 minutes, and kept
      `main` red with merges held for about two hours, to show what its two timeouts had shown
      ([lessons.md](lessons.md), lesson 14).
- **The notes** are a convention within tracking format v3, no version bump
  ([tracking-format.md › Gates](https://github.com/mornycorp/swarm-core/blob/main/docs/tracking-format.md#gates)): a full gate is a helper sub-agent
  named `gate`, `sub` `gate-full-<short commit>` (numbered when the same agent gates the same
  commit again):
  - `sub.start --name gate --note "full gate of main; capacity=<n> free=<GB>; kind=full;
    commit=<sha>"`;
  - `sub.stop --note "rc=<n>[; cause=<cause>]; kind=full; commit=<sha>"`; a gate that lost the
    race, or found `main` verified or moved once it held the machine lock,
    `superseded: <why>; kind=full; commit=<sha>`; one that stopped on an error before its
    result, `abandoned: <error>; kind=full; commit=<sha>`. Neither has an `rc`: neither ran.

  "The last passed full gate" is the newest `sub.stop` with `kind=full`, a `commit=` and `rc=0`,
  across every agent's file, or a landed `--full` gate's (sc#15). The coordinator's own `gate
  --full` names no `commit`; `land` closes it with `main=<sha>` only when `main` is the tree it
  tested (`rc=0; kind=full; main=<sha>; merged #<m> …`), and that stop is a pass of that commit.
  A key of its own, so readers that take a `commit=` for the periodic gate, which merges
  nothing, don't misread it. A timeline viewer draws full gates and a main-health band from these
  notes.

## Limits to expect

- **Ready work** caps parallelism: the coordinator's most valuable job is splitting work so more
  of it is independent.
- **CI capacity:** CI is local (owner, 2026-11-05): GitHub runs no workflows, so each machine's
  cores and disk are the limit (`cargo xtask capacity`); workers rely on `cargo xtask ci
  --changed`, the coordinator on its gate.
- **Empty draft PRs:** GitHub refuses a pull request whose branch has no commits of its own.
  Push a first commit before opening it (`claim`'s claim commit).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-12 | minor | The gate daemon: a bisection's last suspect is gated alone before it is sent back, and nobody is when it passes too; the off switch is read before each gate; `--retry <pr>` forgets what it kept of one PR; a second daemon in a worktree is refused; GitHub's conflict flag is only where to look, and a conflict `stack` resolves is gated; `main` failing on the gating machine files a `main-red` issue; a PR it can't put back to draft is held; three gates in a row that don't run set the switch (sc#45) |
| 2026-11-12 | minor | The gate daemon (new): `cargo xtask gate --daemon` runs steps 2 and 3 of the coordinator's loop on the gating machine: pauses on its switch or an open `main-red` issue; flags a ready PR that conflicts with `main` once per head; stacks in dependency order with rerere off, dropping a conflict and what is stacked on it; folds newly ready PRs in while the gate waits for room and never kills a gate; lands what didn't change during the gate; drops the PR a failure points at (back to draft, the log's excerpt in a comment) or bisects, keeping its suspects; where the project turns it on (`Hooks::verify_main`) runs a stack's failed tests alone on `main` before blaming a PR. `--status`, `--stop`, `--resume`; what stays with the coordinator. A daemon gate's failure is a send-back only of what its `sendback` names (sc#44) |
| 2026-11-12 | minor | Where the project turns it on (`Hooks::gate_priority`, off by default: no change), a gate has priority over workers' builds: `gate --wait` waits for room at most 2 minutes, then goes ahead of the running builds (a disk under the floor still waits, then refuses); while a gate holds the machine-wide gate lock, `capacity` reserves a build for it (but for the gate's own check), so helpers and anything asking `capacity` yield, and `ready` waits before each `ci`, before its test-run slot, while there is no room beside it, saying `a gate has priority` (at most 30 minutes) (sc#41) |
| 2026-11-12 | minor | Staying awake and Starting a worker: `prwatch --ready` puts a PR that is ready but holds no work back to draft with a comment, once per head, and says `redrafted:`; `--no-redraft` only reports; a PR from a fork is only reported; a worker keeps its PR a draft until `ready` (sc#42) |
| 2026-11-12 | minor | Parallel agents: on a machine where the coordinator gates, workers cap their builds' jobs (`CARGO_BUILD_JOBS`, `capacity`'s new `worker builds:` line, 4 unless the machine sets its own cost of a build), so a gate keeps memory for its own; a measurement window the coordinator calls is the exception. `ready` sets it on its builds where the project turns it on (`Hooks::worker_build_jobs`, off by default) on a `full-gate` machine, unless `CARGO_BUILD_JOBS` is set; `gate` sizes its own as before (sc#40) |
| 2026-11-12 | minor | Gate reruns, a project's choice (`Hooks::gate_reruns`, off by default; `--rerun`, `--no-rerun` for one run): where on, a failed gate reruns once by itself, a failure only at the tests its failed tests alone and the steps they skipped, else a transient failure the whole gate; as `gate-<n>-rerun`, both results printed (sc#39) |
| 2026-11-12 | minor | The coordinator's loop, `land`'s step 1: a branch a worker rebased onto a newer `main` since `stack` lands without a re-gate when its own commits' patch-id is the stacked commit's, applied onto what the gate tested they are the gated tree, and the newer `main` changes none of the PR's files; the stacked commit is pushed over it. A changed patch, a conflict or a newer `main` touching the PR's files stops, saying which (sc#37) |
| 2026-11-12 | minor | Picking work › Taking over: a holder's own `stop` after its last `claim` or `work` frees its claim at once; a red `main` doesn't hold a take-over, whose work goes on on its branch; a local branch with commits origin's lacks is refused, not kept with a warning (sc#34) |
| 2026-11-12 | minor | Standing rules, a section of their own: product first and tooling when it hurts twice, "wind down" means finish, decide don't ask, a heavy job run by hand waits for an idle gate, a quiet gate log is not a dead gate, stopping a shell on Windows leaves its children, long runs to a log rather than under a monitor (which stops them at its timeout), squash onto the merge base (lessons.md 15). Starting a worker: a different task gets a fresh context; the worker stops and names its next issue, and the coordinator clears its session or starts a fresh worker; a similar follow-on keeps its session. Picking work, step 4: a worker branches every PR from `main`, and stacks on another open PR (`ready --onto`) only when its change needs that PR's code (lessons.md 13). The periodic full gate, a real failure: a timeout repeated in the rerun alone is proof of a slowdown; time one unit against the last green commit, bisect with it, and measure the whole test once on the fix (lessons.md 14) (swarm-core#29) |
| 2026-11-12 | minor | The architecture review's checklist: the robustness traps (lessons.md 5–12: a child process outside the process module or with no deadline, a second lock in one statement, a file read while drawing, defaults saved over unread state, memory changed before its save, a secret in a result, a discarded `Result`, a test in the real config dir); the two lessons the checklist already cited are 16 and 17 (swarm-core#28) |
| 2026-11-12 | minor | The periodic full gate: holding the machine lock, it reads again: it steps aside for a `main` another gate verified while it waited, and begins again for a `main` that moved (sc#36) |
| 2026-11-05 | minor | No GitHub Actions (owner, 2026-11-05): CI is local for the coordinator and every worker; `land` no longer cancels workflow runs, and `main` red comes from the local gates and the periodic full gate |
| 2026-11-04 | minor | Staying awake › Test runs share the machine (new): every `ci` run that builds and tests (a gate that builds, the periodic full gate's, `ready`'s) holds one of the machine's test-run slots, `SWARM_TEST_RUNS` or one per 16 GB; the order repo lock, machine lock, slot, capacity; `capacity` suggests none while every slot is taken or wired memory is half the total (sc#17) |
| 2026-11-04 | minor | The coordinator's loop › A red gate: gating the same stack again (the same PRs at the same heads onto the same `main`) straight after a failure is its rerun, and `gate` notes it `rerun of <sub>:` itself, so a flake it answers sends nothing back (sc#20) |
| 2026-11-04 | minor | Picking work › Taking over: a claim is stale on the coordinator's say-so (`release --of`, or a `released:` comment) or after 24 hours with no events, comments or commits; `claim <n> --take-over` adopts its PR and branch with their commits, logs `release --of` the old holder, comments the release and the claim, and takes `status::blocked` off when everything its hold waits on is closed; `--force` names `--take-over` for a branch with work on it; the coordinator's step 6 hands a stale claim over so (mornycorp/swarm-core#16) |
| 2026-11-04 | minor | The land step: before a PR merges, `land` retargets onto `main` every open PR whose base is its branch, so GitHub deleting that branch closes none; a PR already closed that way gets its base branch pushed back, is reopened, retargeted and the branch deleted again, or `land` stops with the steps; `s-<pr>` stays until the PR lands (mornycorp/swarm-core#19) |
| 2026-11-04 | minor | Picking work › Finish and The coordinator's loop: `ready --no-ci` notes `no-ci` on its `submit`, and the gate runs a stack holding such a PR as the **changed** kind (`ci --changed --base origin/main`) instead of the smoke, its note naming the PRs (sc#18) |
| 2026-11-03 | minor | Watching your PRs and Tracking: `prwatch` says when your PR is sent back (a `sendback`, or a gate failing for the change's cause) and logs your `work --issue <n> --mr <m>` once, so the rework's start is on the timeline; log it yourself if no command did; a question logs nothing (mornycorp/swarm-core#14) |
| 2026-11-02 | minor | A failed `ready` check's `sub.stop` says why, as the gate's does: `rc=<n>; cause=<cause>; step=<the failing ci step>` (was `rc=1`) |
| 2026-10-31 | minor | GitHub CI runs once a day on `main`, not on pushes or PRs (owner, 2026-10-31): the coordinator checks the latest daily run each batch, and a red one is `main` red |
| 2026-10-31 | minor | No refactoring pass at the end of a worker's task, and no **Refactoring** line for the coordinator to check (owner, 2026-10-31); the `refactoring` skill is dropped too, `complexity-analyzer` kept |
| 2026-10-31 | minor | Claims on one branch: a PR there is yours only when no other claim names it; the first standing claim comment holds the issue; `ready` refuses another agent's (mornycorp/swarm-core#7) |
| 2026-10-31 | minor | The architecture review's checklist: spec requirements with no work under a gap marked done (lessons.md 17, then §6) |
| 2026-10-30 | minor | The architecture review's checklist: a front end building logic the core owns (lessons.md 16, then §5) |
| 2026-10-30 | minor | `capacity`: no build runs while no `rustc` or `cargo-nextest` does; `SWARM_CORES_PER_BUILD` and `SWARM_GB_PER_BUILD` set a machine's own costs; macOS free memory counts purgeable pages |
| 2026-10-30 | minor | Names no consumer: the consumers' names and their issue citations dropped, each rule kept |
| 2026-10-30 | major | GitLab to GitHub (owner, 2026-10-30): the claim is a label and a draft PR (`gh`, `--repo`); `ready` marks a PR ready for review, which starts its checks; `prwatch` (was `mrwatch`) reads merge states (`dirty`, `behind`, `clean`, `blocked`), checks and comments; `land` cancels the head's workflow runs and rebases on the server only a PR that is `behind`; PRs are `#n`; duplicates are closed with `Duplicate of #n` |
| 2026-10-30 | minor | swarm-foundation renamed swarm-core: the crate, `cargo xtask swarm-core` (was `foundation`), `docs/swarm-core-assets.md`, and `sc#n` for its issues (was `f#n`) |
| 2026-10-29 | minor | The periodic full gate: a landed `--full` gate whose merges left `main` at its tree is a pass of that commit (`land` adds `main=<sha>` to its stop); due only after merges no landed full gate verified (an hour on), after the drift interval on an unchanged `main` (a day, `Hooks::full_gate_drift_secs`), or when the gate's inputs changed (`Hooks::full_gate_inputs_changed`, `{{env}}_FULLGATE_DRIFT`) (sc#15) |
| 2026-10-29 | minor | Picking work › Finish and Watching your MRs: `ready` keeps its gate's log, `.rc` and junit as `<target>/gate-logs/ready-<sha>.*`, and reruns a test-only failure's tests once, each alone, then the steps they skipped, as `ready-<sha>-rerun`; `submit`'s note names them; `--no-rerun`; don't rerun a failed `ready` for its tests (sc#14) |
| 2026-10-29 | minor | Picking work: `cargo xtask capabilities`, `session.start`'s summary of what the machine lacks and `claim`'s warning; say what you skip for want of one, with its fix (swarm-core#13) |
| 2026-10-28 | minor | Ending a session (new): `cargo xtask finish` checks nothing is owed, installs the tools in `.git/swarm-tools/` (`install-tools`), removes the agent's clean worktrees and merged branches, then logs `session.stop`; `mrwatch` says when; a worker's step 4 and Tracking › Your session end with it (sc#10) |
| 2026-10-28 | minor | What the code does for every consumer (sc#5): every gate kind that builds takes the machine lock (`Kind::builds`, the smoke included), and the lock's file, wait, renewal and `lock drop` order; a project's causes `gpu` and `python`; the full gate's `fullgate-outcome:` hand-over; `ready`'s `ready-<sha>` sub-agent and rebased xtask; `stack`'s merged branches; `land`'s retries, cancelled pipelines and merge of `main`; the helper worktree beside yours. The project's own settings named as its own: the repo lock's age, the stack record and the full gate's checkout (rendered from `Project`), the worktree layout, gate times, the ready watch's mode, the merge check's minutes, the next issue's order, the gate kinds' planner; sync twins labelled `sync` and titled `[sync]` |
| 2026-10-28 | minor | Watching your MRs: after `ready`, push only for a line that asks it; `land` merges only the gated change, and a push while it lands stops it (sc#9) |

Older rows are in the first consumer's history of `docs/swarm.md` and `AGENTS.md`.
