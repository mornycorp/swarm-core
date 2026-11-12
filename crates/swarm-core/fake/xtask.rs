//! A fake consumer's `cargo xtask ci`, for the tests of the `ci` contract (docs/ci-contract.md):
//! it accepts exactly the contract's command lines, refusing anything else with exit code 2, and
//! behaves as a consumer's `ci` must, as far as the gate reads it.
//!
//! - `FAKE_CI_RECORD=<file>`: each command line it is given is appended there, one a line, its
//!   arguments joined by `|`.
//! - `FAKE_CI_FAIL="<binary id> <test>"`: that test fails in the test step (nextest's lines, its
//!   summary, the junit file), and passes alone (`--test-alone`) unless `FAKE_CI_FAIL_ALONE` is
//!   set too.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::Write;
use std::process::ExitCode;

/// A command line, as the contract has it.
#[derive(Debug, Default)]
struct Ci {
    changed: Option<String>,
    plan: bool,
    head: Option<String>,
    smoke: bool,
    alone: Vec<String>,
    doctests: bool,
    only: Option<Vec<String>>,
}

fn parse(args: &[String]) -> Result<Ci, String> {
    let mut it = args.iter();
    if it.next().map(String::as_str) != Some("ci") {
        return Err("not `ci`".into());
    }
    let mut c = Ci::default();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--changed" => c.changed = Some(String::new()),
            "--base" => match &mut c.changed {
                Some(base) if base.is_empty() => *base = value()?,
                _ => return Err("--base goes with --changed, once".into()),
            },
            "--plan" => c.plan = true,
            "--head" => c.head = Some(value()?),
            "--smoke" => c.smoke = true,
            "--test-alone" => c.alone.push(value()?),
            "--doctests" => c.doctests = true,
            "--only" => c.only = Some(value()?.split(',').map(str::to_owned).filter(|s| !s.is_empty()).collect()),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    check(&c)?;
    Ok(c)
}

/// The contract's rules between the arguments.
fn check(c: &Ci) -> Result<(), String> {
    if c.changed.as_deref() == Some("") {
        return Err("--changed needs --base <ref>".into());
    }
    if (c.plan || c.head.is_some()) && c.changed.is_none() {
        return Err("--plan and --head go with --changed".into());
    }
    if c.head.is_some() && !c.plan {
        return Err("--head goes with --plan".into());
    }
    if c.smoke && c.changed.is_some() {
        return Err("--smoke and --changed are two gates".into());
    }
    let rerun = !c.alone.is_empty() || c.doctests;
    if rerun && c.only.is_none() {
        return Err("--test-alone and --doctests come with --only".into());
    }
    if c.alone.iter().any(|t| t.split(' ').count() != 2 || t.starts_with('-')) {
        return Err("--test-alone takes `<binary id> <test>`".into());
    }
    Ok(())
}

/// The steps a command line runs, in order.
fn steps(c: &Ci) -> Vec<String> {
    let gate: &[&str] = if c.smoke {
        &["fmt", "clippy", "build", "layers", "golden", "cli"]
    } else if c.changed.is_some() {
        &["fmt", "clippy", "test", "layers"]
    } else {
        &["fmt", "clippy", "test", "layers", "wasm", "conformance"]
    };
    match &c.only {
        None => gate.iter().map(|s| (*s).to_owned()).collect(),
        Some(only) => {
            let mut s: Vec<String> = c.alone.iter().map(|t| format!("alone {t}")).collect();
            if c.doctests {
                s.push("doctests".into());
            }
            s.extend(only.iter().cloned());
            s
        }
    }
}

/// The test step's output: nextest's lines, its summary, and the junit file, failing `fail`.
fn test_step(fail: Option<&str>) -> bool {
    let _ = std::fs::create_dir_all("target/nextest/ci").and_then(|()| std::fs::write("target/nextest/ci/junit.xml", "<testsuites/>"));
    println!("    Starting 3 tests across 2 binaries");
    println!("        PASS [   0.1s] (1/3) fake::a one");
    let Some(f) = fail else {
        println!("     Summary [   0.3s] 3 tests run: 3 passed, 0 skipped");
        return true;
    };
    println!("        FAIL [   0.2s] (2/3) {f}");
    println!("     Summary [   0.3s] 3 tests run: 2 passed, 1 failed, 0 skipped");
    println!("        FAIL [   0.2s] (2/3) {f}");
    println!("error: test run failed");
    false
}

/// Run one step: whether it passed.
fn run_step(step: &str, fail: Option<&str>, fail_alone: bool) -> bool {
    println!("=== ci: {step} ===");
    match step.strip_prefix("alone ") {
        Some(t) => !(fail_alone && Some(t) == fail),
        None if step == "test" => test_step(fail),
        None => true,
    }
}

fn record(args: &[String]) {
    let Ok(path) = std::env::var("FAKE_CI_RECORD") else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", args.join("|"));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    record(&args);
    let c = match parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fake-xtask: {e}");
            return ExitCode::from(2);
        }
    };
    if c.plan {
        println!("plan: --changed against {}", c.changed.unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    let fail = std::env::var("FAKE_CI_FAIL").ok();
    let fail_alone = std::env::var_os("FAKE_CI_FAIL_ALONE").is_some();
    let steps = steps(&c);
    let mut lines = Vec::new();
    let mut failed = None;
    for s in &steps {
        if failed.is_some() {
            lines.push(format!("  skip  {s}"));
        } else if run_step(s, fail.as_deref(), fail_alone) {
            lines.push(format!("  ok    {s}"));
        } else {
            lines.push(format!("  FAIL  {s}: it failed"));
            failed = Some(s.clone());
        }
    }
    println!();
    if failed.is_some() {
        println!("CI summary:");
        for l in lines {
            println!("{l}");
        }
        return ExitCode::from(100);
    }
    println!("CI summary: all {} steps passed ({}) in 0.0 min", steps.len(), steps.join(", "));
    ExitCode::SUCCESS
}
