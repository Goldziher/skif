# skif-core

Core diagnostic, fix and report types for [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter
for JavaScript and TypeScript, with first-class monorepo support.

The crate is pure data: it performs no IO, every type is plain `serde` data, and spans are byte offsets with
line/column information so they can be consumed by editors, SARIF and other tooling.

- `Diagnostic`, `Severity`, `RuleId`, `RuleMeta`: what a rule reports.
- `Fix`, `FixOp`, `TextEdit`, `FileChange`: how a finding can be repaired, plus `apply_text_edits`.
- `Report`, `Summary`: a deterministic, sorted collection of diagnostics.
- `LineIndex`: offset to line/column conversion (Unicode scalar and UTF-16 columns).

Enable the `schema` feature to derive `schemars::JsonSchema` for the data types.
