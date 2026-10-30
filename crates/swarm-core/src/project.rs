//! What a consumer is: its settings, a `const` in its xtask, which every command reads instead of
//! hard-coding a consumer's.
//!
//! A field is added when a module that moves needs it, with each consumer's value. A setting both
//! swarms agree on stays a constant in its module.

/// A consumer of Swarm Core: one repo whose agent swarm runs on it.
#[derive(Debug, Clone, Copy)]
pub struct Project {
    /// Machine name: the machine lock's swarm, state-file prefixes (`app-swarm`).
    pub name: &'static str,
    /// The prefix of its environment variables: `{prefix}_AGENT`, `{prefix}_TRACKING`, … The
    /// variables keep the names they had before the move.
    pub env_prefix: &'static str,
    pub github: GitHub,
    pub branches: Branches,
    pub labels: Labels,
    pub limits: Limits,
    /// The file in a cargo target dir naming the checkout that last used it (`checkout`):
    /// `swarm-checkout.txt`.
    pub checkout_marker: &'static str,
    /// The repo gate lock's file name, in the clone's common git directory (`swarm-gate.lock`):
    /// it keeps the name it had, so a lock taken before the move still holds.
    pub gate_lock_file: &'static str,
    /// The file in the clone's git directory where events wait that couldn't be sent
    /// (`cargo xtask event`), named as before the move so a queue the old xtask left is sent.
    pub tracking_queue: &'static str,
    /// The pull request template a claim's draft pull request starts from, from the root
    /// (`claim`).
    pub pr_template: &'static str,
    /// The docs `cargo xtask` generates: `stack` regenerates one that conflicts (1 in Swarm
    /// Inspector, 3 in another consumer).
    pub generated: &'static [Generated],
    /// The stack's record, in the gate worktree's git dir (`stack`, `land`): `swarm-stack.json`,
    /// named as before the move so a stack made before it still lands.
    pub stack_state: &'static str,
    /// The periodic full gate's checkout, under the primary checkout (`fullgate`).
    pub full_gate_worktree: &'static str,
    /// What `ready` builds the rebased tree's own xtask for, when a rebase changed one of them:
    /// the sources of the checks that run inside xtask, and what builds it.
    pub xtask_rules: &'static [&'static str],
    /// How `prwatch --ready` reports (the coordinator's ready watch).
    pub ready_watch: ReadyWatch,
    /// What the coordinator's gate is called in the note `land` leaves on each PR, after ``Local
    /// gate `<sub>` ``: empty, or ` (GPU)` (for a gate that runs the GPU tests).
    pub gate_tag: &'static str,
    /// The smoke's golden tests' binary, as nextest names it (`ci --smoke`'s `golden` step): a
    /// rerun of a failed golden test runs it alone (`swarm-engine::golden`).
    pub golden_binary: &'static str,
}

/// A doc `cargo xtask` writes, and the command that writes it (and checks it with `--check`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generated {
    pub path: &'static str,
    pub command: &'static str,
}

/// How the coordinator's ready watch (`prwatch --ready`) reports a PR that becomes ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyWatch {
    /// One line per PR newly ready or pushed, as it happens, for as long as it runs (Swarm
    /// Inspector's coordinator runs it under a monitor).
    Stream,
    /// Exit as soon as one is newly ready or pushed, printing them (for a coordinator that runs it in
    /// the background, its exit waking it, and re-arms it). What it reported is kept in `record`, a
    /// file in the clone's git directory.
    Exit { record: &'static str },
}

/// Where its issues and pull requests are.
#[derive(Debug, Clone, Copy)]
pub struct GitHub {
    /// The host `gh` talks to when `GH_HOST` doesn't say: `github.com`, or a GitHub Enterprise
    /// Server's.
    pub host: &'static str,
    /// The repository, `owner/name` (`example/app-swarm`), as `gh --repo` takes it.
    pub repo: &'static str,
}

/// Its branches and remote.
#[derive(Debug, Clone, Copy)]
pub struct Branches {
    pub main: &'static str,
    /// Where the swarm logs its events (the tracking format).
    pub tracking: &'static str,
    pub remote: &'static str,
}

/// The labels the swarm's commands read and set.
#[derive(Debug, Clone, Copy)]
pub struct Labels {
    /// A claimed issue's label.
    pub in_progress: &'static str,
    /// An issue on hold: nobody claims it.
    pub blocked: &'static str,
    /// The issue filed while `main` is red: nothing merges, nothing starts from `main`.
    pub main_red: &'static str,
    /// What a `main-red` issue the full gate files is, besides that (`type::bug`).
    pub bug: &'static str,
}

/// The ages and limits on which the two swarms differ, each with its consumer's value.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// How long the repo's gate lock holds before another agent may take it.
    pub gate_lock_max_age_secs: i64,
}

impl Project {
    /// The name of its environment variable `suffix`: `APP_SWARM_AGENT` for `AGENT`.
    pub fn env_name(&self, suffix: &str) -> String {
        format!("{}_{suffix}", self.env_prefix)
    }

    /// Its environment variable `suffix`, when set and valid Unicode.
    pub fn env(&self, suffix: &str) -> Option<String> {
        std::env::var(self.env_name(suffix)).ok()
    }

    /// The generated doc at `path` (as git names it, from the root), if it is one.
    pub fn generated(&self, path: &str) -> Option<&'static Generated> {
        self.generated.iter().find(|g| g.path == path)
    }

    /// A complete project for a consumer's tests that only need one to call through, such as a
    /// reader's round-trip test of `event::examples(&Project::EXAMPLE)`. Swarm Core fills in
    /// each field it adds, so such a test doesn't break on a bump that adds one. No environment variable of its prefix is ever set.
    pub const EXAMPLE: Project = Project {
        name: "example",
        env_prefix: "SWARM_CORE_EXAMPLE",
        github: GitHub { host: "github.invalid", repo: "example/example" },
        branches: Branches { main: "main", tracking: "tracking", remote: "origin" },
        labels: Labels { in_progress: "status::in-progress", blocked: "status::blocked", main_red: "main-red", bug: "type::bug" },
        limits: Limits { gate_lock_max_age_secs: 2 * 3600 },
        checkout_marker: "example-checkout.txt",
        gate_lock_file: "example-gate.lock",
        tracking_queue: "example-tracking-queue.jsonl",
        pr_template: ".github/pull_request_template.md",
        generated: &[],
        stack_state: "example-stack.json",
        full_gate_worktree: ".claude/worktrees/fullgate",
        xtask_rules: &["xtask", "Cargo.toml", "Cargo.lock", ".cargo"],
        ready_watch: ReadyWatch::Stream,
        gate_tag: "",
        golden_binary: "swarm-engine::golden",
    };
}

/// Two consumers' settings, for Swarm Core's own tests: a first project, and a second
/// project that differs in every setting a consumer can pick (a GPU gate, a longer gate lock,
/// more generated docs, the ready watch that exits). Each module's tests run against both, so a
/// setting one of them depends on can't drift unseen.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub const APP_SWARM: Project = Project {
        name: "app-swarm",
        env_prefix: "APP_SWARM",
        github: GitHub { host: "github.com", repo: "example/app-swarm" },
        branches: Branches { main: "main", tracking: "tracking", remote: "origin" },
        labels: Labels { in_progress: "status::in-progress", blocked: "status::blocked", main_red: "main-red", bug: "type::bug" },
        limits: Limits { gate_lock_max_age_secs: 2 * 3600 },
        checkout_marker: "swarm-checkout.txt",
        gate_lock_file: "swarm-gate.lock",
        tracking_queue: "swarm-tracking-queue.jsonl",
        pr_template: ".github/pull_request_template.md",
        generated: &[Generated { path: "docs/scorecard.md", command: "scorecard" }],
        stack_state: "swarm-stack.json",
        full_gate_worktree: ".claude/worktrees/fullgate",
        xtask_rules: &["xtask", "Cargo.toml", "Cargo.lock", ".cargo"],
        ready_watch: ReadyWatch::Stream,
        gate_tag: "",
        golden_binary: "swarm-engine::golden",
    };

    pub const GPU_SWARM: Project = Project {
        name: "gpu-swarm",
        env_prefix: "GPU_SWARM",
        github: GitHub { host: "github.example.com", repo: "example/gpu-swarm" },
        branches: Branches { main: "main", tracking: "tracking", remote: "origin" },
        labels: Labels { in_progress: "status::in-progress", blocked: "status::blocked", main_red: "main-red", bug: "type::bug" },
        limits: Limits { gate_lock_max_age_secs: 3 * 3600 },
        checkout_marker: "gpu-checkout.txt",
        gate_lock_file: "gpu-gate.lock",
        tracking_queue: "gpu-tracking-queue.jsonl",
        pr_template: ".github/pull_request_template.md",
        generated: &[
            Generated { path: "docs/commands.md", command: "commands" },
            Generated { path: "docs/parity-checklist.md", command: "parity" },
            Generated { path: "docs/scorecard.md", command: "scorecard" },
        ],
        stack_state: "gpu-stack.json",
        full_gate_worktree: ".claude/worktrees/fullgate",
        xtask_rules: &["xtask", "Cargo.toml", "Cargo.lock", ".cargo"],
        ready_watch: ReadyWatch::Exit { record: "gpu-prwatch-ready.txt" },
        gate_tag: " (GPU)",
        golden_binary: "gpu-engine::golden",
    };

    pub const BOTH: [Project; 2] = [APP_SWARM, GPU_SWARM];
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;

    #[test]
    fn environment_variables_keep_their_names() {
        assert_eq!(APP_SWARM.env_name("AGENT"), "APP_SWARM_AGENT");
        assert_eq!(GPU_SWARM.env_name("TRACKING"), "GPU_SWARM_TRACKING");
        for p in BOTH {
            assert!(crate::github::check_repo(p.github.repo).is_ok() && p.limits.gate_lock_max_age_secs > 0);
        }
    }
}
