//! What this machine can run (swarm-core#13): the project's capabilities, each the issue label
//! that needs it, probed by its [`crate::Hooks::capabilities`]. A worker whose machine lacks one
//! skipped every issue that needs it, and nobody saw it: on 2026-10-28 a session skipped every
//! `requires-reference` issue for want of a tool the owner set up in ten minutes once asked.
//!
//! - `cargo xtask capabilities` prints each, what it found, and for a missing one how to fix it.
//! - `cargo xtask event session.start` ends with the missing ones, so a session says at its start
//!   what it can't take.
//! - `claim` warns when the issue is labelled with one this machine lacks.
//!
//! A project with none (the default) prints nothing.

use std::path::Path;

use crate::Hooks;

/// The usage line, as [`crate::COMMANDS`] lists it.
pub const USAGE: &str = "cargo xtask capabilities: what this machine can run, by the issue label that needs it, and how to fix a missing one";

/// One capability: the label of the issues that need it, whether this machine has it, what the
/// probe found, and how to get it when missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    pub label: &'static str,
    pub have: bool,
    pub detail: String,
    pub fix: String,
}

impl Capability {
    fn line(&self) -> String {
        if self.have { format!("  {}: yes ({})", self.label, self.detail) } else { format!("  {}: NO ({}); fix: {}", self.label, self.detail, self.fix) }
    }
}

/// `cargo xtask capabilities` ([`USAGE`]).
pub fn run(hooks: &dyn Hooks, root: &Path, args: &[&str]) -> Result<(), String> {
    if let Some(a) = args.first() {
        return Err(format!("capabilities: unknown argument `{a}`\nusage: {USAGE}"));
    }
    let caps = hooks.capabilities(root);
    if caps.is_empty() {
        println!("capabilities: the project names none");
        return Ok(());
    }
    println!("capabilities of this machine:");
    for c in &caps {
        println!("{}", c.line());
    }
    Ok(())
}

/// What a session's start says: the missing capabilities and their fixes, or that it has them
/// all; nothing for a project that names none.
pub fn summary(caps: &[Capability]) -> Option<String> {
    if caps.is_empty() {
        return None;
    }
    let missing: Vec<&Capability> = caps.iter().filter(|c| !c.have).collect();
    if missing.is_empty() {
        let all: Vec<&str> = caps.iter().map(|c| c.label).collect();
        return Some(format!("capabilities: this machine runs all of {}", all.join(", ")));
    }
    let mut out = format!(
        "capabilities: this machine can't run issues labelled {} (`cargo xtask capabilities`); tell the owner, with the fix:",
        missing.iter().map(|c| c.label).collect::<Vec<_>>().join(", ")
    );
    for c in missing {
        out.push('\n');
        out.push_str(&c.line());
    }
    Some(out)
}

/// What a claim of an issue labelled `labels` says: the capabilities it needs that this machine
/// lacks, and their fixes; `None` when it has them all.
pub fn lacking(caps: &[Capability], issue: u64, labels: &[String]) -> Option<String> {
    let lacking: Vec<&Capability> = caps.iter().filter(|c| !c.have && labels.iter().any(|l| l == c.label)).collect();
    if lacking.is_empty() {
        return None;
    }
    let mut out = format!(
        "warning: #{issue} needs {}, which this machine lacks: its checks can't run here. Release it unless the work needs none of it, and tell the owner:",
        lacking.iter().map(|c| c.label).collect::<Vec<_>>().join(", ")
    );
    for c in lacking {
        out.push('\n');
        out.push_str(&c.line());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(label: &'static str, have: bool) -> Capability {
        Capability { label, have, detail: format!("{label} probed"), fix: format!("get {label}") }
    }

    #[test]
    fn a_session_says_what_it_cant_run_and_how_to_fix_it() {
        assert_eq!(summary(&[]), None, "a project with none says nothing");
        let all = summary(&[cap("requires-gpu", true), cap("requires-reference", true)]).unwrap();
        assert_eq!(all, "capabilities: this machine runs all of requires-gpu, requires-reference");
        let some = summary(&[cap("requires-gpu", true), cap("requires-reference", false)]).unwrap();
        assert!(some.starts_with("capabilities: this machine can't run issues labelled requires-reference"), "{some}");
        assert!(some.contains("requires-reference: NO (requires-reference probed); fix: get requires-reference") && !some.contains("requires-gpu:"), "{some}");
    }

    #[test]
    fn a_claim_warns_only_for_the_labels_it_lacks() {
        let caps = [cap("requires-gpu", true), cap("requires-reference", false)];
        let labels = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(lacking(&caps, 7, &labels(&["requires-gpu", "type::task"])), None);
        assert_eq!(lacking(&caps, 7, &labels(&[])), None);
        let w = lacking(&caps, 7, &labels(&["requires-reference", "requires-gpu"])).unwrap();
        assert!(w.starts_with("warning: #7 needs requires-reference, which this machine lacks") && w.contains("fix: get requires-reference"), "{w}");
    }

    #[test]
    fn the_command_takes_no_arguments() {
        assert!(run(&crate::Defaults, Path::new("."), &["--all"]).unwrap_err().contains("unknown argument"));
        assert_eq!(run(&crate::Defaults, Path::new("."), &[]), Ok(()));
    }
}
