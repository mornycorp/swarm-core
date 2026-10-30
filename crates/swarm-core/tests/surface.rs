//! The surface a consumer's xtask relies on.

#[test]
fn the_version_is_the_manifests() {
    assert_eq!(swarm_core::VERSION, env!("CARGO_PKG_VERSION"));
    assert!(!swarm_core::VERSION.is_empty());
}

#[test]
fn every_command_has_a_usage_line_naming_it() {
    for (command, usage) in swarm_core::COMMANDS {
        assert!(swarm_core::serves(command));
        assert!(usage.starts_with(&format!("cargo xtask {command}")), "{command}: {usage}");
    }
    assert!(!swarm_core::serves("layers"), "a consumer's own command isn't served here");
}

/// A reader's round-trip test calls through the example project
/// instead of writing out every field, and the example's environment is never set.
#[test]
fn the_example_project_writes_every_example() {
    use swarm_core::{Project, event};
    let lines = event::examples(&Project::EXAMPLE).unwrap();
    for instance in event::EXAMPLE_INSTANCES {
        assert!(lines.iter().any(|(i, _)| i == instance), "{instance} writes");
    }
    assert!(Project::EXAMPLE.env("AGENT").is_none() && Project::EXAMPLE.env("TRACKING").is_none());
}

#[test]
fn the_tracking_format_doc_ships_with_the_writer() {
    let doc = swarm_core::event::FORMAT_DOC;
    assert!(doc.contains("## Versions"), "the doc a reader's versions test reads");
    assert!(doc.contains(&format!("| {} |", swarm_core::event::FORMAT)), "the current version has its row");
}
