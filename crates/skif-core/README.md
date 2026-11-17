# skif-core

Core diagnostic, fix and report types for [skif](https://github.com/polylint/skif), a dependency and package-manager linter
for JavaScript and TypeScript, with first-class monorepo support.

The data types are plain `serde` data, and spans are byte offsets with line/column information so they can be
consumed by editors, SARIF and other tooling. Only the `fs` module touches the disk.

- `Diagnostic`, `Severity`, `RuleId`, `RuleMeta`: what a rule reports.
- `Fix`, `FixOp`, `TextEdit`, `FileChange`: how a finding can be repaired, plus `apply_text_edits`.
- `Report`, `Summary`: a deterministic, sorted collection of diagnostics.
- `LineIndex`: offset to line/column conversion (Unicode scalar and UTF-16 columns).
- `FileSystem`: the read-only file access every skif library goes through, with `OsFs` (disk), `MemoryFs` (tests)
  and `BufferFs` (an editor's unsaved buffers over another file system).
- `CacheStore`, `CacheNamespace`, `KeyBuilder`, `seal` and `open`, `CacheCounters`, `MemoryStore`: the cache store a host
  gives the linter, with the checked envelope skif wraps each value in; a store only moves opaque bytes.

Enable the `schema` feature to derive `schemars::JsonSchema` for the data types.
