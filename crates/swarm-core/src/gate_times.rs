//! `cargo xtask gate-times`: one gate's slowest tests, or two gates'
//! compared (the slowest in the second, and the biggest changes), for finding the gate's long poles
//! and what moved.
//!
//! A gate is named by its sub id, found in `<target>/gate-logs/` ([`LOG_DIR`]; `--dir` for
//! another), or by a file. Its times come from the junit file the gate keeps beside its log
//! (`<sub>.junit.xml`), else from the nextest lines in its log (`PASS [  12.3s] (n/m) <binary>
//! <test>`), as older gates left only their logs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Project, capacity};

const USAGE: &str = "usage: cargo xtask gate-times <gate> [<gate>] [--top <n>] [--dir <gate-logs dir>]
  <gate>: a sub id in <target>/gate-logs (its .junit.xml, else its .log), or a junit or log file";

/// The directory in the target dir where the gates keep their logs and junit files (the consumers'
/// `land::GATE_LOG_DIR` and `gate::LOG_DIR`).
pub const LOG_DIR: &str = "gate-logs";

/// How many tests each list shows by default.
const TOP: usize = 15;

/// Each test's time in seconds, by `<binary> <test>` as nextest names it.
pub type Times = BTreeMap<String, f64>;

/// One attribute's value in an XML start tag, entities decoded.
fn attr(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let len = tag.get(start..)?.find('"')?;
    let raw = tag.get(start..start + len)?;
    Some(raw.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}

/// The times in a nextest junit file: each `testcase`'s `classname` (the binary), `name` and `time`.
pub fn from_junit(xml: &str) -> Times {
    let mut out = Times::new();
    for chunk in xml.split("<testcase ").skip(1) {
        let tag = format!(" {}", chunk.split_once('>').map_or(chunk, |(t, _)| t));
        let (Some(binary), Some(name), Some(time)) = (attr(&tag, "classname"), attr(&tag, "name"), attr(&tag, "time")) else { continue };
        if let Ok(secs) = time.parse::<f64>()
            && secs.is_finite()
        {
            out.insert(format!("{binary} {name}"), secs);
        }
    }
    out
}

/// The times in a gate's log: nextest's final line for each test (`PASS`, `FAIL`, `LEAK`, …;
/// not the `SLOW` notices while it runs), `<STATUS> [ <secs>s] [(n/m)] <binary> <test>`.
pub fn from_log(log: &str) -> Times {
    let mut out = Times::new();
    for l in log.lines() {
        let Some((status, rest)) = l.trim_start().split_once(" [") else { continue };
        if status.is_empty() || status == "SLOW" || !status.chars().all(|c| c.is_ascii_uppercase() || c == '-') {
            continue;
        }
        let Some((secs, after)) = rest.split_once(']') else { continue };
        let Some(Ok(secs)) = secs.trim().strip_suffix('s').map(str::parse::<f64>) else { continue };
        let after = after.trim_start();
        let name = if after.starts_with('(') { after.split_once(')').map_or("", |(_, n)| n) } else { after }.trim();
        if secs.is_finite() && name.contains(' ') {
            out.insert(name.to_owned(), secs);
        }
    }
    out
}

/// The file a gate's times come from: `spec` itself when it is a file, else `<dir>/<spec>.junit.xml`
/// or `<dir>/<spec>.log`.
pub fn find(spec: &str, dir: &Path) -> Result<PathBuf, String> {
    let as_file = PathBuf::from(spec);
    if as_file.is_file() {
        return Ok(as_file);
    }
    ["junit.xml", "log"]
        .iter()
        .map(|ext| dir.join(format!("{spec}.{ext}")))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("gate-times: no file `{spec}`, nor {spec}.junit.xml or {spec}.log in {}", dir.display()))
}

/// A gate's times from its file (junit when it looks like XML, else a log).
pub fn load(path: &Path) -> Result<Times, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let times = if text.trim_start().starts_with('<') { from_junit(&text) } else { from_log(&text) };
    if times.is_empty() {
        return Err(format!("gate-times: no test times in {}", path.display()));
    }
    Ok(times)
}

/// The `n` slowest tests, slowest first.
fn slowest(t: &Times, n: usize) -> Vec<(&str, f64)> {
    let mut v: Vec<(&str, f64)> = t.iter().map(|(k, s)| (k.as_str(), *s)).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    v.truncate(n);
    v
}

/// The `n` tests in both whose time changed most, either way, biggest first: name, before, after.
fn biggest_changes<'a>(a: &Times, b: &'a Times, n: usize) -> Vec<(&'a str, f64, f64)> {
    let mut moved: Vec<(&str, f64, f64)> = b.iter().filter_map(|(k, after)| a.get(k).map(|before| (k.as_str(), *before, *after))).collect();
    moved.sort_by(|x, y| (y.2 - y.1).abs().total_cmp(&(x.2 - x.1).abs()).then(x.0.cmp(y.0)));
    moved.truncate(n);
    moved
}

fn headline(label: &str, t: &Times) -> String {
    format!("{label}: {} tests, {:.1} s summed\n", t.len(), t.values().sum::<f64>())
}

/// The report: `a`'s slowest tests alone, or with `b`, `b`'s slowest beside their time in `a` and
/// the `top` biggest changes between them (tests in both).
pub fn report(a: (&str, &Times), b: Option<(&str, &Times)>, top: usize) -> String {
    let mut s = headline(a.0, a.1);
    let Some(b) = b else {
        s.push_str(&format!("\nslowest {top}:\n"));
        for (name, secs) in slowest(a.1, top) {
            s.push_str(&format!("{secs:>9.1}  {name}\n"));
        }
        return s;
    };
    s.push_str(&comparison(a, b, top));
    s
}

/// The report's part for two gates: `b`'s headline, its slowest beside their time in `a`, the
/// biggest changes, and how many tests only one of them ran.
fn comparison(a: (&str, &Times), b: (&str, &Times), top: usize) -> String {
    let mut s = headline(b.0, b.1);
    let was = |name: &str| a.1.get(name).map_or_else(|| "-".to_owned(), |x| format!("{x:.1}"));
    s.push_str(&format!("\nslowest {top} in {}:   (time in {})\n", b.0, a.0));
    for (name, secs) in slowest(b.1, top) {
        s.push_str(&format!("{secs:>9.1} {:>9}  {name}\n", was(name)));
    }
    s.push_str(&format!("\nbiggest {top} changes ({} to {}):\n", a.0, b.0));
    for (name, before, after) in biggest_changes(a.1, b.1, top) {
        s.push_str(&format!("{:>+9.1}  {before:>8.1} to {after:>8.1}  {name}\n", after - before));
    }
    let only = |x: &Times, y: &Times| x.keys().filter(|k| !y.contains_key(*k)).count();
    s.push_str(&format!("\nonly in {}: {} tests; only in {}: {} tests\n", a.0, only(a.1, b.1), b.0, only(b.1, a.1)));
    s
}

/// The gates named, the list length and the `--dir` given; `None` for `--help`.
type Opts<'a> = (Vec<&'a str>, usize, Option<PathBuf>);

fn parse_args<'a>(args: &[&'a str]) -> Result<Option<Opts<'a>>, String> {
    let (mut gates, mut top, mut dir) = (Vec::new(), TOP, None);
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--top" => top = it.next().and_then(|n| n.parse().ok()).filter(|n| *n > 0).ok_or_else(|| format!("gate-times: --top needs a count\n{USAGE}"))?,
            "--dir" => dir = Some(PathBuf::from(it.next().ok_or_else(|| format!("gate-times: --dir needs a path\n{USAGE}"))?)),
            "-h" | "--help" => return Ok(None),
            other if other.starts_with("--") => return Err(format!("gate-times: unknown argument `{other}`\n{USAGE}")),
            other => gates.push(other),
        }
    }
    if gates.is_empty() || gates.len() > 2 {
        return Err(format!("gate-times: name one gate or two\n{USAGE}"));
    }
    Ok(Some((gates, top, dir)))
}

/// `cargo xtask gate-times <gate> [<gate>] [--top <n>] [--dir <gate-logs dir>]`.
pub fn run(_project: &Project, root: &Path, args: &[&str]) -> Result<(), String> {
    let Some((gates, top, dir)) = parse_args(args)? else {
        println!("{USAGE}");
        return Ok(());
    };
    let dir = match dir {
        Some(d) => d,
        None => capacity::our_target(root)?.join(LOG_DIR),
    };
    let mut loaded = Vec::new();
    for g in &gates {
        let path = find(g, &dir)?;
        let label = Path::new(g).file_name().and_then(|n| n.to_str()).unwrap_or(g);
        println!("{label}: {}", path.display());
        loaded.push((label, load(&path)?));
    }
    let mut it = loaded.iter().map(|(g, t)| (*g, t));
    let first = it.next().ok_or("gate-times: no gate")?;
    print!("\n{}", report(first, it.next(), top));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const JUNIT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="3" time="596.990">
    <testsuite name="xtask::bin/xtask" tests="2">
        <testcase name="land::tests::a_stack_lands" classname="xtask::bin/xtask" timestamp="2026-10-21T12:56:06.218+01:00" time="168.200"/>
        <testcase name="gate::tests::a&amp;b" classname="xtask::bin/xtask" timestamp="2026-10-21T12:56:06.218+01:00" time="0.500">
            <failure message="x"/>
        </testcase>
    </testsuite>
    <testsuite name="gpu::bin/gpu" tests="1">
        <testcase name="logging::tests::rotation" classname="gpu::bin/gpu" time="0.747"/>
        <testcase name="no_time" classname="gpu::bin/gpu"/>
    </testsuite>
</testsuites>"#;

    #[test]
    fn junit_gives_each_tests_time_by_binary_and_name() {
        let t = from_junit(JUNIT);
        assert_eq!(t.len(), 3, "{t:?}");
        assert_eq!(t["xtask::bin/xtask land::tests::a_stack_lands"], 168.2);
        assert_eq!(t["xtask::bin/xtask gate::tests::a&b"], 0.5, "entities decoded");
        assert_eq!(t["gpu::bin/gpu logging::tests::rotation"], 0.747);
        assert!(from_junit("<testcase name=\"x\" classname=\"y\" time=\"NaN\"/>").is_empty());
    }

    /// The nextest lines of a real gate log (a GPU swarm's gate), the old scripts' and `gate`'s alike.
    #[test]
    fn a_log_gives_the_final_line_of_each_test() {
        let log = "   Compiling xtask\n\
            \x20       PASS [   0.041s] (   1/1692) gpu::control_methods both_tables\n\
            \x20       SLOW [> 60.000s] (─────────) xtask::bin/xtask land::tests::a_stack_lands\n\
            \x20       PASS [ 168.200s] (1500/1692) xtask::bin/xtask land::tests::a_stack_lands\n\
            \x20       LEAK [   2.5s] (1501/1692) gpu-engine::jobs a_job\n\
            \x20       FAIL [  59.000s] (1502/1692) xtask::bin/xtask stack::tests::rebases\n\
            \x20 FAIL  test: cargo nextest run failed\n\
            error [E0308]: not a test\n";
        let t = from_log(log);
        assert_eq!(
            t.into_iter().collect::<Vec<_>>(),
            [
                ("gpu-engine::jobs a_job".to_owned(), 2.5),
                ("gpu::control_methods both_tables".to_owned(), 0.041),
                ("xtask::bin/xtask land::tests::a_stack_lands".to_owned(), 168.2),
                ("xtask::bin/xtask stack::tests::rebases".to_owned(), 59.0),
            ]
        );
    }

    #[test]
    fn two_gates_compare_by_their_slowest_and_their_biggest_changes() {
        let times = |v: &[(&str, f64)]| v.iter().map(|(k, s)| ((*k).to_owned(), *s)).collect::<Times>();
        let a = times(&[("x a", 10.0), ("x b", 100.0), ("x c", 1.0), ("x gone", 5.0)]);
        let b = times(&[("x a", 12.0), ("x b", 20.0), ("x c", 31.0), ("x new", 7.0)]);
        let r = report(("gate-1", &a), Some(("gate-2", &b)), 2);
        assert_eq!(
            r,
            "gate-1: 4 tests, 116.0 s summed\n\
             gate-2: 4 tests, 70.0 s summed\n\
             \n\
             slowest 2 in gate-2:   (time in gate-1)\n\
             \x20    31.0       1.0  x c\n\
             \x20    20.0     100.0  x b\n\
             \n\
             biggest 2 changes (gate-1 to gate-2):\n\
             \x20   -80.0     100.0 to     20.0  x b\n\
             \x20   +30.0       1.0 to     31.0  x c\n\
             \n\
             only in gate-1: 1 tests; only in gate-2: 1 tests\n"
        );
        let one = report(("gate-1", &a), None, 1);
        assert_eq!(one, "gate-1: 4 tests, 116.0 s summed\n\nslowest 1:\n    100.0  x b\n");
    }

    #[test]
    fn arguments_are_checked() {
        assert_eq!(parse_args(&["gate-1", "gate-2", "--top", "5", "--dir", "d"]), Ok(Some((vec!["gate-1", "gate-2"], 5, Some(PathBuf::from("d"))))));
        assert_eq!(parse_args(&["gate-1"]), Ok(Some((vec!["gate-1"], TOP, None))));
        assert_eq!(parse_args(&["--help"]), Ok(None));
        for (bad, why) in [
            (&[][..], "one gate or two"),
            (&["a", "b", "c"][..], "one gate or two"),
            (&["a", "--top", "0"][..], "a count"),
            (&["a", "--dir"][..], "a path"),
            (&["a", "--fast"][..], "unknown"),
        ] {
            assert!(parse_args(bad).unwrap_err().contains(why), "{bad:?}");
        }
    }

    #[test]
    fn a_gate_is_found_by_its_junit_then_its_log_or_as_a_file() {
        let dir = swarm_core_testkit::temp_dir("swarm-core-gate_times-find");
        std::fs::write(dir.join("gate-1.log"), "        PASS [   1.5s] (1/1) a::b t\n").unwrap();
        assert_eq!(find("gate-1", &dir).unwrap(), dir.join("gate-1.log"));
        std::fs::write(dir.join("gate-1.junit.xml"), JUNIT).unwrap();
        assert_eq!(find("gate-1", &dir).unwrap(), dir.join("gate-1.junit.xml"), "the junit file first");
        assert_eq!(load(&dir.join("gate-1.junit.xml")).unwrap().len(), 3);
        assert_eq!(load(&dir.join("gate-1.log")).unwrap()["a::b t"], 1.5);
        let file = dir.join("gate-1.log");
        assert_eq!(find(file.to_str().unwrap(), Path::new("elsewhere")).unwrap(), file);
        assert!(find("gate-2", &dir).unwrap_err().contains("gate-2.junit.xml"));
        std::fs::write(dir.join("empty.log"), "no tests here\n").unwrap();
        assert!(load(&dir.join("empty.log")).unwrap_err().contains("no test times"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
