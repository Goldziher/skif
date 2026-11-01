---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Fix and Edit Model: Structural Operations, Format-Preserving Edits, Returned Plans

## Context and Problem Statement

The inherited fixes read a manifest into `serde_json::Value`, mutate it, and serialize the whole
document back (`json::serialize` with a detected indent and line ending, then `fs::write`). That
preserves indentation and key order (`serde_json` with `preserve_order`), and loses everything
else: a trailing newline style the detector did not pick up, unusual spacing, and any formatting a
project's formatter does not reproduce exactly. It also means two fixes to the same file each
re-serialize it, and nothing can show a user what a fix will change before it happens.

pnpm catalogs live in `pnpm-workspace.yaml`, where re-serialization is worse: comments are the norm
in that file and a YAML round-trip through a data model discards them.

Some fixes have alternatives rather than one answer. Aligning a `version-mismatch` can target the
highest version, the lowest, or a specific one; the inherited rule resolves that with an
`inquire::Select` prompt from inside the rule. That cannot run in a library or a hook ([ADR 0003](0003-library-first-embedding.md)).

## Decision Drivers

* Fixes must not reformat anything they do not change, including YAML comments in `pnpm-workspace.yaml`
* Users and hosts should see what a fix changes before it happens
* Some problems have several valid fixes, and a prompt cannot run in a library or a hook
* Fixes that change what gets installed need a separate safety level

## Considered Options

* Structural FixOps translated to format-preserving text edits and returned as a FixPlan
* Re-serialize with detected indentation (the inherited approach)
* Rules emit text edits directly
* Write files from `apply_fixes`
* Pick a default silently when a fix has alternatives

## Decision Outcome

Chosen option: "Structural FixOps translated to format-preserving text edits and returned as a FixPlan", because describing what should change, editing through a CST that keeps every untouched byte, and returning a plan instead of writing files keeps comments and formatting, allows preview and composition, and makes the cwd-relative write bug structurally impossible.

A fix is **data describing a structural change**, translated to **text edits** by a separate
crate, and **returned as a plan** that the caller writes.

### FixOp: what to change

Rules attach `Fix { title, safety, ops: Vec<FixOp> }` to a diagnostic ([ADR 0007](0007-rule-and-diagnostic-model.md)). `FixOp`, defined
in `skif-core`, is structural and format-agnostic:

* `Json { path, pointer, action }` — at a path of keys and indices, `Set`, `Insert` (key, value and a
  `Start`, `End` or `Sorted` position), `Remove`, `RenameKey` or `SortKeys`;
* `Yaml { path, route, action }` — `Set`, `Remove` and `Append` on a YAML document;
* `Text { path, edit }` — a raw `TextEdit`, for the rare change that is not structural;
* `CreateFile { path, contents }` and `DeleteFile { path }`.

Rules never produce text offsets for structural changes; they say *what* should change. `TextEdit`
offsets are `u32` UTF-8 byte offsets, the same units as `Span` ([ADR 0007](0007-rule-and-diagnostic-model.md)). `FixOp` and the action
enums are `#[non_exhaustive]`, and their values are skif-owned JSON values, not `serde_json` types.

### skif-edit: how to change it without reformatting

`skif-edit` turns `FixOp`s into `TextEdit { range: start..end (bytes), replacement }`:

* JSON through **`jsonc-parser`'s CST**, which keeps every byte it does not touch: whitespace,
  indentation, key order, line endings. An inserted entry copies the indentation and separator
  style of its siblings.
* YAML through **`yamlpatch`**, behind the `yaml` feature (the facade's `yaml-fix`), which edits a
  YAML document in place and keeps comments and layout.

Edits to one file from several fixes are merged and checked for **overlap**. `skif-edit` fails an
overlapping set naming both operations; the facade resolves that by dropping the later fix (in
diagnostic order) from the plan and recording it in `skipped`, never silently merging.

### FixPlan: returned, never written

There is **one change shape** for every kind of fix, defined in `skif-core`:

```rust
pub enum FileChange {
    Modify { path: PathBuf, edits: Vec<TextEdit>, new_contents: String },
    Create { path: PathBuf, contents: String },
    Delete { path: PathBuf },
}

pub struct FixPlan {
    pub changes: Vec<FileChange>,       // sorted by path, one entry per file
    pub applied: Vec<AppliedFix>,       // diagnostic + fix title
    pub skipped: Vec<SkippedFix>,       // diagnostic + reason
}
```

`skif::apply_fixes(workspace, report, selection) -> Result<FixPlan, FixError>` does not touch the file
system. `CreateFile` and `DeleteFile` operations become `Create` and `Delete` changes directly;
`Json`, `Yaml` and `Text` operations on the same file are planned together by `skif-edit` into one
`Modify` carrying both the edits and the resulting contents, so a host can show a diff from either.
A file created or deleted by one fix cannot also be edited by another; that is a conflict like an
overlap. `skif-cli` writes the plan, atomically per file (write to a temporary sibling, then
rename; create parent directories for `Create`), and a host may instead show it as a diff or
merge it with its own edits.

### Fix selection

A `FixSelection` states which fixes go into the plan:

* `safety` — the highest safety level to include (`Safe`, or `Unsafe` which includes both);
* `rules` — optionally only the fixes of these rule IDs;
* `choices` — explicit picks, by diagnostic, of one of its alternative fixes.

When a diagnostic has several fixes, the rule returns **all of them in order**, labelled by what
they do (aligning on the highest version, on the lowest, on a specific one). If configuration
determines the answer, the determined fix comes first and is marked `default`; the version
alignment of `preferred-semver` is determined by `[policy] prefer`, which is `highest` unless set,
and `--select highest|lowest` overrides it for one run ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)). Selection takes, for each
diagnostic, the pick in `choices` if there is one, else the fix marked `default`, provided its
safety is allowed. A diagnostic with several fixes and neither a pick nor a default is skipped with
reason `NeedsChoice` rather than guessed. The interactive CLI turns `NeedsChoice` into a prompt and
builds `choices` from the answers.

### Safety levels

Every `Fix` has a `safety`:

* `Safe` — cannot change behaviour: sorting dependency keys, removing an empty dependency map,
  setting `private: true` on the root.
* `Unsafe` — changes what gets installed or how something resolves: changing a version, replacing a
  specifier with `catalog:` or `workspace:`, removing a dependency.

`--fix` applies `Safe` fixes; `--fix-unsafe` also applies `Unsafe` ones ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)). A host chooses
with the same `selection`. A fix writes the specifier string a rule chose, not a re-rendered
range, and never rewrites a `catalog:` reference to a version.

### Choices belong to the caller

No library crate prompts. The CLI offers an interactive selection when stdin is a TTY and a choice
exists; otherwise the diagnostic stays reported and unfixed (`NeedsChoice`) unless configuration or
`--select` determined a default.

### Consequences

* Good, because bytes outside an edit are identical after a fix. The fix round-trip test asserts it: apply the
  plan, re-run, expect no fixable diagnostics and unchanged bytes outside every edited range.
* Good, because comments in `pnpm-workspace.yaml` survive catalog fixes.
* Good, because fixes can be previewed, reviewed, and composed by a host, and the cwd-relative write bug is
  structurally impossible because the library writes nothing ([ADR 0003](0003-library-first-embedding.md)).
* Bad, because two editors (JSON CST and YAML) and a structural operation layer are more code than
  `serde_json` plus `fs::write`.
* Bad, because `yamlpatch` brings native code; musl and Windows cross builds must be verified before it ships,
  which is why it sits behind a feature.
* Bad, because dropping a conflicting fix means some problems need a second `--fix` run. That is preferred to
  merging edits whose combined effect nobody reviewed.

### Confirmation

* A fix round-trip test in the facade applies a plan to a fixture workspace, re-runs the linter, and asserts zero fixable diagnostics and byte-identical content outside every edited range; a variant runs the CLI from a directory other than the workspace root and asserts the root-level fix still lands in the root `package.json`.
* `cargo test -p skif-edit` includes the proptest that arbitrary non-overlapping edits applied in any order give the same text, and tests that comments, indentation, key order and line endings (CRLF, BOM) survive each `JsonAction` and `YamlAction`.
* A facade test asserts that two overlapping fixes yield a plan containing the first and a `skipped` entry for the second, and that a `Create` and an edit of the same path are a conflict.
* A facade test table covers `FixSelection`: safety ceiling, rule filter, explicit `choices`, a `default` fix, and `NeedsChoice` when several alternatives exist and neither is present.
* A test asserts that applying a plan never writes `catalog:` references as versions and writes the chosen specifier string verbatim (`1.2` stays `1.2`).
* The `build` job's musl and Windows targets compile `skif-edit` with the `yaml` feature; the feature stays out of the facade's defaults until those jobs are green.

## Pros and Cons of the Options

### Structural FixOps translated to format-preserving text edits and returned as a FixPlan

* Good, because every untouched byte survives, fixes can be previewed and composed by a host, and the cwd-relative write bug is structurally impossible because the library writes nothing.
* Bad, because two editors (JSON CST and YAML) and a structural operation layer are more code than `serde_json` plus `fs::write`, `yamlpatch` brings native code, and a conflicting fix is deferred to a second `--fix` run.

### Re-serialize with detected indentation (the inherited approach)

* Good, because it is a few lines of code and works for any JSON shape.
* Bad, because it is not format-preserving in general, and impossible for YAML with comments.

### Rules emit text edits directly

* Good, because it has one fewer translation layer.
* Bad, because every rule would need to re-derive offsets and separator style, and a structural description is what lets a host or reporter explain a fix.

### Write files from `apply_fixes`

* Good, because callers would get the result of `--fix` in one call.
* Bad, because the library would perform IO the host did not ask for, which [ADR 0003](0003-library-first-embedding.md) rejects, and there would be no preview.

### Pick a default silently when a fix has alternatives

* Good, because `--fix` would always make progress with no flag.
* Bad, because choosing the version to align on is a judgement the user owns; a non-interactive run reports the diagnostic unfixed unless configuration, `--select` or an interactive pick made the choice explicit.

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* See the [ADR index](index.md).
