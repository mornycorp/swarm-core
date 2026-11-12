# The `ci` contract

> **Last updated:** 2026-10-30 · **Change:** minor (GitLab to GitHub)

Swarm Core's gate never builds or tests a consumer itself. It runs the consumer's own
`cargo xtask ci`, as that consumer's xtask executable, and reads what it leaves behind. This page
is everything the two sides agree on. Each consumer's `ci` must accept every form below and
behave as described. Swarm Core must call nothing else.

`crates/swarm-core/tests/ci_contract.rs` checks Swarm Core's side against a fake consumer,
`crates/swarm-core/fake/xtask.rs`. The fake accepts exactly these forms and refuses
anything else with exit code 2.

## Who calls it

| Caller | Command line | Where |
|---|---|---|
| `gate`, docs and tooling kinds | `ci --changed --base origin/main` | the coordinator's worktree |
| `gate`, smoke kind | `ci --smoke` | the coordinator's worktree |
| `gate --full`, the periodic full gate | `ci` | the coordinator's worktree; the full gate's checkout |
| `gate --rerun-failed`, the full gate's rerun | the kind's arguments, then `--test-alone "<binary id> <test>"`… `[--doctests]` `--only <step>,<step>…` | as the run it reruns |
| `ready` | `ci --changed --base <base>` | the PR's worktree |
| `ready`'s rerun of failed tests (sc#14) | `ci --changed --base <base> --test-alone "<binary id> <test>"… --doctests --only <step>,<step>…` | the PR's worktree |
| a consumer's own planning (not Swarm Core) | `ci --changed --base <ref> --plan [--head <ref>]` | anywhere |

## The arguments

- **`--changed --base <ref>`:** run the gate's steps only for the packages that the change since
  the merge base with `<ref>` affects, plus their dependents and the packages that read a changed
  file. A `Cargo.lock` change reaches only the packages whose resolved dependencies include a
  package whose lock entry changed (`changed::lock`, `changed::lock_for`); a lockfile that can't
  be read plans everything. When the plan reaches everything (the workspace manifest, the lint or
  test settings, an unreadable lockfile), run everything.
- **`--plan`:** with `--changed`, print the plan and run nothing.
- **`--head <ref>`:** with `--plan`, plan a pushed ref instead of the worktree.
- **`--smoke`:** the coordinator's smoke. That is the build, fmt, clippy, layers, the generated
  docs' checks, the golden tests (a step named `golden`, run with `cargo test`) and a short
  headless run. There is no full test run.
- **No arguments:** the whole gate: every step.
- **`--test-alone "<binary id> <test>"`** (repeatable): before anything else, run each named test
  alone: its own nextest run, one thread, the `ci` profile. That is
  `gate::rerun::Test::nextest_args`. A filter that matches no test fails. The test step that runs
  them is named `alone <binary id> <test>`.
- **`--doctests`:** then the workspace's doctests, which nextest doesn't run.
- **`--only <step>,<step>…`:** then only these steps of the gate the other arguments name. The step
  names are the ones the consumer's `CI summary` prints.

## What it leaves

- **Its exit code:** 0 when every step passed. Anything else is a failure, and the gate reads why
  from the log.
- **Its output** goes to stdout and stderr; the gate writes both to its log. The log ends with the
  `CI summary` block:

  ```text
  CI summary: all <n> steps passed (<step>, <step>, …) in <m> min
  ```

  or, on a failure:

  ```text
  CI summary:
    ok    <step>
    FAIL  <step>: <what failed>
    skip  <step>
  ```

  `land` lands only a gate whose last `CI summary` says `all … steps passed`. A rerun's pass must
  name every test it ran alone (`alone <binary id> <test>`) on that line. `gate --rerun-failed`
  reruns only a failure at the gate's test step (`test`; the smoke's `golden`) and the steps marked
  `skip` after it.
- **nextest's lines** for the test step: its status lines (`FAIL [ 1.2s] (n/m) <binary id>
  <test>`, `TIMEOUT`, `SIG…`, `… + LEAK`), and its `Summary [ …] N tests run: a passed (…), b
  failed, c timed out, d skipped` line followed by the failures again. A rerun takes the tests
  that summary lists, and only when they are as many as it counts.
- **libtest's lines** for `golden`: `test result: FAILED. a passed; b failed; …`, and the last
  `failures:` list with one name per line.
- **nextest's junit file** at `target/nextest/ci/junit.xml` under the workspace root, whatever
  `CARGO_TARGET_DIR` says. That is the `ci` profile's `junit.path`. The gate and `ready` keep it as
  `<target>/gate-logs/<sub>.junit.xml` when it was written during the run.

## What the gate gives it

- **The working directory:** the tree to gate.
- **`CARGO_BUILD_JOBS`:** from the free memory the gate saw, unless the environment sets it.
- **The project's variables:** [`Hooks::around_ci`](../crates/swarm-core/src/lib.rs) may add more
  (a Python interpreter's path, a GPU lease's mode), and so may `Hooks::full_gate_env` for the
  periodic full gate (the path to test data the full gate reads).
