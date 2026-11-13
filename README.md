# Swarm tracking

The swarm's timeline: every agent instance (worker or coordinator) logs its sessions and
its work on issues with `cargo xtask event` (AGENTS.md › Tracking). This branch is never merged.

- `tracking/<instance>/work.jsonl`: one agent instance's events, one JSON object a line, oldest
  first. Only that instance writes it.
- Every event: `t` (UTC, `YYYY-MM-DDTHH:MM:SSZ`, the agent's clock) and `event`.
- `session.start`: `name` (the agent's stable name, such as `claude-mac` or `coordinator`),
  `role` (`worker` or `coordinator`), `machine`, and `parent`: the instance id of the agent that
  started this session (a session the coordinator starts); none for a top-level agent. A tracker
  nests an instance under its parent. Optional `model` (the model running the session, such as
  `claude-opus-5-5`) and `effort` (`low`, `medium`, `high`, `xhigh` or `max`), for comparing
  models; a tracker shows `unknown` where they are missing. `format`: the version of this format
  the writer wrote (a whole number; none means 1). A change to the format bumps it.
- `sub.start` (`sub`, `name`?, `model`?, `effort`?), `sub.stop` (`sub`): a helper sub-agent inside this instance's
  session (a search, a review, a gate) starts and ends. Sub-agents have no file of their own:
  their parent logs them here, and any issue event may carry `sub` to say the sub-agent did it.
  `sub` is unique within the file.
- `session.pause`, `session.resume`, `session.stop` (`reason`?, `of`?): the instance pauses,
  resumes, ends; `reason` `duplicate`: because another agent has its issue. `of`: the dead
  instance whose session this stop ends (the coordinator closing a dead agent's session).
- `claim` (`issue`, `mr`): it takes an issue. `work` (`issue`, `mr`?): it starts or resumes work
  on it; with `mr`, it picks up that PR sent back (`sendback`, or a failed gate naming it): its
  rework starts here, and the time since the send-back was waiting.
- `submit` (`issue`, `mr`): it marks the PR ready for merge.
- `stop` (`issue`, `reason`: handoff, blocked, paused or done): it stops working on the issue.
- `release` (`issue`, `reason`?, `of`?): it gives up the issue (lost a claim race, gave up), or the
  coordinator released a stale claim. `reason` `duplicate`: another agent has the issue, so the
  work so far is thrown away. `of`: the instance whose claim or work this release ends, when it
  isn't the logger's own (the coordinator stopping another agent's work).
- `merge`, `sendback` (`issue`, `mr`): the coordinator merged a PR or sent it back.
- Any event may carry a one-line `note`.
