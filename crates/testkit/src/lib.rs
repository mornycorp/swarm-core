//! Test helpers for swarm-core: temp dirs and git repositories. A dev-dependency only.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]

pub mod git;

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
