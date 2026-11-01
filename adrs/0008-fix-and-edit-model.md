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

* `Json { path, op }` — set, insert (with position), remove or rename at a JSON pointer;
  sort the keys of an object;
* `Yaml { path, op }` — the same operations on a YAML document;
* `Text { path, edit }` — a raw `TextEdit`, for the rare change that is not structural;
* `CreateFile { path, contents }` and `DeleteFile { path }`.

Rules never produce text offsets for structural changes; they say *what* should change.

### skif-edit: how to change it without reformatting

`skif-edit` turns `FixOp`s into `TextEdit { range: start..end (bytes), replacement }`:

* JSON through **`jsonc-parser`'s CST**, which keeps every byte it does not touch: whitespace,
  indentation, key order, line endings. An inserted entry copies the indentation and separator
  style of its siblings.
* YAML through **`yamlpatch`**, behind the `yaml` feature (the facade's `yaml-fix`), which edits a
  YAML document in place and keeps comments and layout.

Edits to one file from several fixes are merged and checked for **overlap**; overlapping edits from
different fixes are a conflict, and the later fix (in diagnostic order) is dropped from the plan and
reported as skipped, never silently merged.

### FixPlan: returned, never written

`skif::apply_fixes(workspace, report, selection) -> FixPlan` returns
`Vec<FileChange { path, edits, new_contents }>` plus the list of applied and skipped fixes. It does
not touch the file system. `skif-cli` writes the plan, atomically per file (write to a temporary
sibling, then rename); a host may instead show it as a diff or merge it with its own edits.

### Safety levels

Every `Fix` has a `safety`:

* `Safe` — cannot change behaviour: sorting dependency keys, removing an empty dependency map,
  setting `private: true` on the root.
* `Unsafe` — changes what gets installed or how something resolves: changing a version, replacing a
  specifier with `catalog:` or `workspace:`, removing a dependency.

`--fix` applies `Safe` fixes; `--fix-unsafe` also applies `Unsafe` ones. A host chooses with the
same `selection`.

### Choices belong to the CLI

When a problem has several valid fixes, the rule returns **all of them** as separate `Fix` values.
The CLI offers an interactive selection when stdin is a TTY and a choice exists, and otherwise
applies the first fix only if the rule marked it as the default. No library crate prompts.

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

* The fix round-trip test applies the plan, re-runs, and expects no fixable diagnostics and unchanged bytes outside every edited range.
* Overlapping edits from different fixes are detected; the later fix is dropped from the plan and reported as skipped.
* musl and Windows cross builds of `yamlpatch` are verified before the `yaml` feature ships.

## Pros and Cons of the Options

### Structural FixOps translated to format-preserving text edits and returned as a FixPlan

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Re-serialize with detected indentation (the inherited approach)

* Bad, because it is not format-preserving in general, and impossible for YAML with comments.

### Rules emit text edits directly

* Bad, because every rule would need to re-derive offsets and separator style, and a structural description is what lets a host or reporter explain a fix.

### Write files from `apply_fixes`

* Bad, because it was rejected by [ADR 0003](0003-library-first-embedding.md)

### Pick a default silently when a fix has alternatives

* Bad, because choosing the version to align on is a judgement the user owns; a non-interactive run reports the diagnostic unfixed unless a default was explicit.

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md)
* See the [ADR index](index.md).
