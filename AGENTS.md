# AGENTS.md: guide for agents working on Swarm Core

This crate is the swarm tooling every consumer's agent swarm runs on. [README.md](README.md) says
what it serves and how a consumer wires it in; the swarm protocol
(`crates/swarm-core/assets/docs/swarm-protocol.md`) says how a swarm works.

- **The rules are the consumers' rules.** No panics outside tests, no `unsafe`, typed errors,
  hostile input treated as hostile, a deadline on every git and gh call, and the code-health limits
  in `clippy.toml`. Every crate root carries the lint headers in `crates/swarm-core/src/lib.rs`.
- **Every consumer must keep working.** A change to a command's name, flags, output or
  environment variables, or to the tracking format, is a change for every swarm. Behaviour that
  only one project wants goes behind `Project` settings or a `Hooks` method whose default keeps the
  others' behaviour unchanged. The tests run each module against two fixture projects that differ
  in every setting (`project::fixtures`).
- **Landing:** a PR here merges once the checks below pass locally; GitHub runs no workflows
  (CI is local, for the coordinator and every worker). A consumer picks a change up with `cargo xtask swarm-core bump`, and its own gate runs the new code.
- **GitHub, through `gh`:** every call to GitHub goes through `src/github.rs`, which reads REST
  and GraphQL answers into one shape for a PR and one for an issue; the commands' decisions are
  pure functions over those shapes, tested without a network. A change to `github.rs` also runs
  its live read test against GitHub: `cargo test -p swarm-core --lib -- --ignored live`.
- **The app skeleton, built for real:** a change to `src/skeleton.rs`, `templates/app/`,
  `src/checks`, the testkit's conformance run, `examples/consumer_xtask.rs` or `docs/new-swarm.md`
  also runs `cargo test -p swarm-core --test app_skeleton -- --ignored` before it merges (minutes;
  it needs the crates registry and `cargo nextest`, and fails without them). A change to the
  skeleton's dependencies refreshes its locks: `SWARM_CORE_REFRESH_LOCKS=1` with the same command.
- **Iterating from a consumer:** put a `[patch."https://github.com/mornycorp/swarm-core.git"]`
  table pointing at your checkout in your *user-level* cargo config, never in a repo. The
  consumers' `cargo xtask layers` refuses a committed one.
- **Assets are templates for every consumer.** A file under `crates/swarm-core/assets/` lands in
  each consumer's repo (`cargo xtask swarm-core sync-assets`): write it for any of them, naming
  none (the test `no_asset_names_a_consumer`), with `{{name}}`
  placeholders for what differs (`assets::render`), and add a file to `src/assets/list.rs` (the
  test `every_file_under_assets_is_shipped` says when the list and the directory differ). The
  skills under `.claude/skills/` are copied as they are, but `add-command`, a template.
- **Checks:** `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
