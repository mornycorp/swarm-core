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
    Asset { path: ".claude/skills/refactoring/SKILL.md", text: include_str!("../../assets/.claude/skills/refactoring/SKILL.md") },
    Asset { path: ".claude/skills/refactoring/agents/openai.yaml", text: include_str!("../../assets/.claude/skills/refactoring/agents/openai.yaml") },
    Asset {
        path: ".claude/skills/refactoring/languages/cpp/constructor-complete-init.md",
        text: include_str!("../../assets/.claude/skills/refactoring/languages/cpp/constructor-complete-init.md"),
    },
    Asset { path: ".claude/skills/refactoring/languages/cpp/raii.md", text: include_str!("../../assets/.claude/skills/refactoring/languages/cpp/raii.md") },
    Asset {
        path: ".claude/skills/refactoring/languages/rust/macro-restraint.md",
        text: include_str!("../../assets/.claude/skills/refactoring/languages/rust/macro-restraint.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/catalog-index.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/catalog-index.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/change-function-declaration.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/change-function-declaration.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/change-reference-to-value.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/change-reference-to-value.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/change-value-to-reference.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/change-value-to-reference.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/collapse-hierarchy.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/collapse-hierarchy.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/combine-functions-into-class.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/combine-functions-into-class.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/combine-functions-into-transform.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/combine-functions-into-transform.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/consolidate-conditional-expression.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/consolidate-conditional-expression.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/decompose-conditional.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/decompose-conditional.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/encapsulate-collection.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/encapsulate-collection.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/encapsulate-record.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/encapsulate-record.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/encapsulate-variable.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/encapsulate-variable.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/extract-class.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/extract-class.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/extract-function.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/extract-function.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/extract-superclass.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/extract-superclass.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/extract-variable.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/extract-variable.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/hide-delegate.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/hide-delegate.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/inline-class.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/inline-class.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/inline-function.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/inline-function.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/inline-variable.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/inline-variable.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/introduce-assertion.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/introduce-assertion.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/introduce-parameter-object.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/introduce-parameter-object.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/introduce-special-case.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/introduce-special-case.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/move-field.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/move-field.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/move-function.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/move-function.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/move-statements-into-function.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/move-statements-into-function.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/move-statements-to-callers.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/move-statements-to-callers.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/parameterize-function.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/parameterize-function.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/preserve-whole-object.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/preserve-whole-object.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/pull-up-constructor-body.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/pull-up-constructor-body.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/pull-up-field.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/pull-up-field.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/pull-up-method.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/pull-up-method.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/push-down-field.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/push-down-field.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/push-down-method.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/push-down-method.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/remove-dead-code.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/remove-dead-code.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/remove-flag-argument.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/remove-flag-argument.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/remove-middle-man.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/remove-middle-man.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/remove-setting-method.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/remove-setting-method.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/remove-subclass.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/remove-subclass.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/rename-field.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/rename-field.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/rename-variable.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/rename-variable.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-command-with-function.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-command-with-function.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-conditional-with-polymorphism.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-conditional-with-polymorphism.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-constructor-with-factory-function.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-constructor-with-factory-function.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-control-flag-with-break.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-control-flag-with-break.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-derived-variable-with-query.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-derived-variable-with-query.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-error-code-with-exception.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-error-code-with-exception.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-exception-with-precheck.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-exception-with-precheck.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-function-with-command.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-function-with-command.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-inline-code-with-function-call.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-inline-code-with-function-call.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-loop-with-pipeline.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-loop-with-pipeline.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-magic-literal.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-magic-literal.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-nested-conditional-with-guard-clauses.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-nested-conditional-with-guard-clauses.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-parameter-with-query.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-parameter-with-query.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-primitive-with-object.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-primitive-with-object.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-query-with-parameter.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-query-with-parameter.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-subclass-with-delegate.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-subclass-with-delegate.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-superclass-with-delegate.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-superclass-with-delegate.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-temp-with-query.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-temp-with-query.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/replace-type-code-with-subclasses.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/replace-type-code-with-subclasses.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/return-modified-value.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/return-modified-value.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/separate-query-from-modifier.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/separate-query-from-modifier.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/slide-statements.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/slide-statements.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/split-loop.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/split-loop.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/split-phase.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/split-phase.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/split-variable.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/split-variable.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/refactorings/substitute-algorithm.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/refactorings/substitute-algorithm.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/scope-selection.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/scope-selection.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/suggest-mode.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/suggest-mode.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/references/verification-gates.md",
        text: include_str!("../../assets/.claude/skills/refactoring/references/verification-gates.md"),
    },
    Asset {
        path: ".claude/skills/refactoring/scripts/generate_refactoring_contexts.py",
        text: include_str!("../../assets/.claude/skills/refactoring/scripts/generate_refactoring_contexts.py"),
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
