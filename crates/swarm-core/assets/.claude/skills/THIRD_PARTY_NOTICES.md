# Third-party software

These extracted skills do not vendor third-party code. The analyzer can install
or use the components below; each keeps its own licence. Package versions and
download hashes remain in `skills/complexity-analyzer/tool-manifest.json` and
`skills/complexity-analyzer/node-tools/package-lock.json`.

| Component | Licence | How it is used |
| --- | --- | --- |
| [rust-code-analysis](https://github.com/mozilla/rust-code-analysis) | MPL-2.0 | Downloaded by the analyzer installer with SHA256 verification. |
| ESLint, eslint-plugin-sonarjs, @typescript-eslint/parser, TypeScript, dependency-cruiser, jscpd, knip | Each package's own licence | Installed with npm from the committed lockfile for JS/TS metrics and duplication detection. |
| tree-sitter, tree-sitter-rust, radon, vulture | Each package's own licence | Pinned Python dependencies provided by uv or an optional analyzer venv. |
| Clippy | MIT or Apache-2.0 | Used from the target project's Rust toolchain. |
| pytest | MIT | Used to run the retained analyzer tests. |

Original Swarm attribution and licensing are preserved in `NOTICE` and `LICENSE`.
