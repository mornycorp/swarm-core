//! Every file under `crates/swarm-core/assets/`, at the path it takes in a consumer. Kept in step
//! with the directory by `assets::tests::every_file_under_assets_is_shipped`.

use super::Asset;

/// The assets this revision ships, by path.
pub const ASSETS: &[Asset] = &[
    Asset { path: ".claude/skills/LICENSE", text: include_str!("../../assets/.claude/skills/LICENSE") },
    Asset { path: ".claude/skills/NOTICE", text: include_str!("../../assets/.claude/skills/NOTICE") },
    Asset { path: ".claude/skills/README.md", text: include_str!("../../assets/.claude/skills/README.md") },
    Asset { path: ".claude/skills/THIRD_PARTY_NOTICES.md", text: include_str!("../../assets/.claude/skills/THIRD_PARTY_NOTICES.md") },
    Asset { path: ".claude/skills/complexity-analyzer/.gitattributes", text: include_str!("../../assets/.claude/skills/complexity-analyzer/.gitattributes") },
    Asset { path: ".claude/skills/complexity-analyzer/.gitignore", text: include_str!("../../assets/.claude/skills/complexity-analyzer/.gitignore") },
    Asset { path: ".claude/skills/complexity-analyzer/SKILL.md", text: include_str!("../../assets/.claude/skills/complexity-analyzer/SKILL.md") },
    Asset {
        path: ".claude/skills/complexity-analyzer/agents/openai.yaml",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/agents/openai.yaml"),
    },
    Asset { path: ".claude/skills/complexity-analyzer/bin/bench.py", text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/bench.py") },
    Asset {
        path: ".claude/skills/complexity-analyzer/bin/calibrate.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/calibrate.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/bin/compare_runs.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/compare_runs.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/bin/install_tools.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/install_tools.py"),
    },
    Asset { path: ".claude/skills/complexity-analyzer/bin/metrics", text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/metrics") },
    Asset { path: ".claude/skills/complexity-analyzer/bin/metrics.cmd", text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/metrics.cmd") },
    Asset { path: ".claude/skills/complexity-analyzer/bin/metrics.py", text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/metrics.py") },
    Asset { path: ".claude/skills/complexity-analyzer/bin/selftest", text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/selftest") },
    Asset {
        path: ".claude/skills/complexity-analyzer/bin/selftest.cmd",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/selftest.cmd"),
    },
    Asset { path: ".claude/skills/complexity-analyzer/bin/selftest.py", text: include_str!("../../assets/.claude/skills/complexity-analyzer/bin/selftest.py") },
    Asset {
        path: ".claude/skills/complexity-analyzer/calibration/benchmark.tsv",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/calibration/benchmark.tsv"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/calibration/inputs.md",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/calibration/inputs.md"),
    },
    Asset { path: ".claude/skills/complexity-analyzer/js/analyze.sh", text: include_str!("../../assets/.claude/skills/complexity-analyzer/js/analyze.sh") },
    Asset {
        path: ".claude/skills/complexity-analyzer/js/cx-depcruise.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/js/cx-depcruise.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/js/cx-eslint.config.mjs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/js/cx-eslint.config.mjs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/js/eslint.config.mjs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/js/eslint.config.mjs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/js/strip-directives.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/js/strip-directives.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/__init__.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/__init__.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/core.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/core.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/coupling.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/coupling.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/dup.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/dup.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/funcs.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/funcs.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/jssrc.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/jssrc.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/langtools.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/langtools.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/nodetools.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/nodetools.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/overeng.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/overeng.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/pipeline.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/pipeline.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/pysrc.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/pysrc.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/pytools.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/pytools.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/report.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/report.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/rustsrc.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/rustsrc.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/lib/cxmetrics/scores.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/lib/cxmetrics/scores.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/node-tools/package-lock.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/node-tools/package-lock.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/node-tools/package.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/node-tools/package.json"),
    },
    Asset { path: ".claude/skills/complexity-analyzer/py/analyze.py", text: include_str!("../../assets/.claude/skills/complexity-analyzer/py/analyze.py") },
    Asset { path: ".claude/skills/complexity-analyzer/py/design.py", text: include_str!("../../assets/.claude/skills/complexity-analyzer/py/design.py") },
    Asset { path: ".claude/skills/complexity-analyzer/py/pyast.py", text: include_str!("../../assets/.claude/skills/complexity-analyzer/py/pyast.py") },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/py/clumps.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/py/clumps.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/py/cognitive.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/py/cognitive.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/py/cognitive.radon.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/py/cognitive.radon.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/py/cognitive.rca.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/py/cognitive.rca.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/py/cohesion.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/py/cohesion.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/py/imports.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/py/imports.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/py/params.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/py/params.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample.metrics.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample.metrics.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample.nodup.metrics.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample.nodup.metrics.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/Cargo.toml",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/Cargo.toml"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/app/Cargo.toml",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/app/Cargo.toml"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/app/src/main.rs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/app/src/main.rs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/Cargo.toml",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/Cargo.toml"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/a.rs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/a.rs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/b.rs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/b.rs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/lib.rs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/lib.rs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/util.rs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/src/util.rs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/tests/smoke.rs",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/crates/core/tests/smoke.rs"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/js/a.js",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/js/a.js"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/js/b.js",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/js/b.js"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/package.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/package.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/py/pkg/__init__.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/py/pkg/__init__.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/py/pkg/x.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/py/pkg/x.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/py/pkg/y.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/py/pkg/y.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/py/test_pkg.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/py/test_pkg.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/ts/app.ts",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/ts/app.ts"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/ts/index.ts",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/ts/index.ts"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/ts/lib/foo.ts",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/ts/lib/foo.ts"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/ts/lib/shape.ts",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/ts/lib/shape.ts"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/ts/orphan.ts",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/ts/orphan.ts"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/fixtures/sample/tsconfig.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/fixtures/sample/tsconfig.json"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/test_funcs.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/test_funcs.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/test_parsers.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/test_parsers.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/test_py_analyze.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/test_py_analyze.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/test_py_design.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/test_py_design.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/test_scores.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/test_scores.py"),
    },
    Asset {
        path: ".claude/skills/complexity-analyzer/tests/test_tool_cache.py",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tests/test_tool_cache.py"),
    },
    Asset { path: ".claude/skills/complexity-analyzer/thresholds.json", text: include_str!("../../assets/.claude/skills/complexity-analyzer/thresholds.json") },
    Asset {
        path: ".claude/skills/complexity-analyzer/tool-manifest.json",
        text: include_str!("../../assets/.claude/skills/complexity-analyzer/tool-manifest.json"),
    },
    Asset { path: ".github/ISSUE_TEMPLATE/bug.md", text: include_str!("../../assets/.github/ISSUE_TEMPLATE/bug.md") },
    Asset { path: ".github/ISSUE_TEMPLATE/task.md", text: include_str!("../../assets/.github/ISSUE_TEMPLATE/task.md") },
    Asset { path: ".github/pull_request_template.md", text: include_str!("../../assets/.github/pull_request_template.md") },
    Asset { path: "docs/lessons.md", text: include_str!("../../assets/docs/lessons.md") },
    Asset { path: "docs/progress-docs.md", text: include_str!("../../assets/docs/progress-docs.md") },
    Asset { path: "docs/swarm-protocol.md", text: include_str!("../../assets/docs/swarm-protocol.md") },
    Asset { path: "scripts/github-labels.sh", text: include_str!("../../assets/scripts/github-labels.sh") },
    Asset { path: "scripts/hooks/pre-push", text: include_str!("../../assets/scripts/hooks/pre-push") },
];
