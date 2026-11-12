//! Test helpers for {{product}}: temp dirs and hostile request lines. It sits outside the layer
//! stack and is **only a `[dev-dependencies]` entry**, never a dependency of production code
//! (docs/app-architecture.md › Layers). It uses no crate of the workspace, so any layer's tests can
//! use it without a dependency cycle.
//!
//! - [`temp_dir`]: where a test keeps config and state (docs/app-architecture.md › Tests touch
//!   nothing real).
//! - [`adversarial_lines`]: what a hostile caller sends a transport.
//!
//! **The conformance run is not here.** It is swarm-core's: `swarm_core_testkit::conformance`
//! (a `[dev-dependencies]` entry, pinned beside `swarm-core`) drives every command through every
//! surface and compares, and so does `adversarial_params`, the hostile params a command is sent. An
//! app implements `conformance::App` once (`apps/{{cli}}/tests/surfaces.rs`) and gets the run's
//! improvements by bumping the pin, instead of keeping a copy of the logic.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![warn(clippy::cognitive_complexity, clippy::too_many_lines)]
#![cfg_attr(test, allow(clippy::cognitive_complexity, clippy::too_many_lines))]

use std::path::PathBuf;

/// A fresh, empty directory for the test called `name`: cleared first, so what an earlier run left
/// behind (the OS reuses process ids) never meets this one. `name` is unique among the tests of one
/// binary, which run in parallel threads of one process.
pub fn temp_dir(name: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("app-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Raw request lines (no trailing newline) that a transport must answer with an error and survive:
/// not JSON, not a request, not UTF-8, deeply nested.
pub fn adversarial_lines() -> Vec<Vec<u8>> {
    let owned = |s: &str| s.as_bytes().to_vec();
    vec![
        owned("{"),
        owned("null"),
        owned("[]"),
        owned("\"app.info\""),
        owned("{\"id\": 1}"),
        owned("{\"method\": \"app.info\"}"),
        owned("{\"id\": null, \"method\": \"app.info\"}"),
        owned("{\"id\": {}, \"method\": \"app.info\"}"),
        owned("{\"id\": 1, \"method\": 5}"),
        owned("{\"id\": 1, \"method\": \"app.info\", \"params\": 5}"),
        owned("{\"id\": 1, \"method\": \"app.info\", \"params\": [1]}"),
        vec![0xff, 0xfe, 0xfd],
        vec![b'{', 0, b'}'],
        "[".repeat(100_000).into_bytes(),
        ["{\"id\": 1, \"method\": \"", &"a".repeat(100_000), "\"}"].concat().into_bytes(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_dir_is_fresh_and_empty() {
        let dir = temp_dir("testkit-fresh").unwrap();
        std::fs::write(dir.join("left-behind"), "x").unwrap();
        let again = temp_dir("testkit-fresh").unwrap();
        assert_eq!(dir, again);
        assert_eq!(std::fs::read_dir(&again).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&again);
    }

    #[test]
    fn the_adversarial_lines_include_one_that_is_not_utf8() {
        assert!(adversarial_lines().iter().any(|l| std::str::from_utf8(l).is_err()));
    }
}
