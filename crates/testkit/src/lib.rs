//! Test helpers for swarm-core and the apps built on it. A dev-dependency only.
//!
//! - [`temp_dir`] and [`git`]: temp dirs and git repositories.
//! - [`conformance`]: the app architecture's conformance run (docs/app-architecture.md › Every
//!   action is a command): an app implements [`conformance::App`], and one call proves every
//!   command behaves the same on every surface. An app gets the run's improvements by bumping
//!   its pin of this crate.
//! - [`process`]: a program run to a deadline, for an app's CLI surface.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

pub mod conformance;
pub mod git;
pub mod process;

use std::path::PathBuf;

/// Every temp path in a test comes from here: it is cleared first, so a file a failed earlier run
/// left behind (with the same process id, which the OS reuses) never meets this run. `name` must
/// be unique among the tests of one binary: tests run in parallel threads of one process, and a
/// second call with the same name clears the first one's directory. The `swarm-test-` prefix is
/// what the consumers' gates know a test's temporary repository by.
#[allow(clippy::panic)]
pub fn temp_dir(name: &str) -> PathBuf {
    match try_temp_dir(name) {
        Ok(dir) => dir,
        Err(e) => panic!("{e}"),
    }
}

/// [`temp_dir`] as a `Result`.
pub fn try_temp_dir(name: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("swarm-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}
