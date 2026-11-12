---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Fixes, Edits and `package.json` Formatting: Structural Operations, Returned Plans and Safety by Move

## Context and Problem Statement

Rules find problems that often have a mechanical repair: a version to align, a dependency map to sort, a stray
lockfile to delete, an enum member to remove. A repair that re-serializes a whole manifest loses formatting the
project's formatter does not reproduce, and a round-trip of `pnpm-workspace.yaml` through a data model loses its
comments. Some problems have several valid repairs, such as aligning on the highest or the lowest version, and a prompt
cannot run inside a library, a hook or an editor ([ADR 0003](0003-library-api-and-embedding.md)). Users coming from
sherif and syncpack expect `--fix` to align versions, users coming from knip expect deletions to be guarded and fix
runs to be narrowed by issue type, and syncpack users expect a `format --check` gate for `package.json`.

How does a rule describe a repair, how is it turned into file changes, which repairs does a plain `--fix` apply, and how
is `package.json` formatting checked?

## Decision Drivers

* A fix never changes a byte it does not have to change, including comments in YAML files
* A user or a host sees what a fix changes before it happens; the library writes nothing
* When a problem has several valid repairs, the choice is the user's or the configuration's, never a silent default
* `--fix` applies what cannot break a build; everything else needs an explicit `--fix-unsafe`, and deleting a file
  needs its own opt-in
* `skif --fix` followed by `skif` leaves nothing `--fix` could still fix
* Formatting is one rule among others, with the same suppression, output and exit codes

## Considered Options

* Structural fix operations, translated to format-preserving text edits and returned as a plan, with safety judged by
  the move a fix makes and formatting as a rule
* Re-serialize the edited document with detected indentation
* Rules emit text edits directly
* Write files from the library
* A separate formatting pipeline with its own command

## Decision Outcome

Chosen option: "Structural fix operations, translated to format-preserving text edits and returned as a plan, with
safety judged by the move a fix makes and formatting as a rule", because editing through a CST keeps every untouched
byte, returning a plan allows preview and composition by any host, judging each move lets `--fix` do what sherif and
syncpack do for the low-risk case while every risky move still needs `--fix-unsafe`, and the rule model already gives
formatting everything it needs.

### The fix model

* A diagnostic carries `Fix { title, safety, ops }`
  ([ADR 0008](0008-rule-and-diagnostic-model-rule-ids-and-severities.md)). `FixOp`, in `skif-core`, is structural and
  format-agnostic: `Json` (`Set`, `Insert`, `Remove`, `RenameKey`, `SortKeys`), `Yaml` (`Set`, `Remove`, `Append`),
  `Text` for the rare change that is not structural, `CreateFile` and `DeleteFile`. The operation and action enums are
  `#[non_exhaustive]`, and their values are skif-owned, not `serde_json` types. Rules say what should change, never at
  which byte offset.
* `skif-edit` turns operations into text edits: JSON through the `jsonc-parser` CST, which keeps whitespace, key order,
  line endings and comments, an inserted entry copying its siblings' layout; YAML through `yamlpatch`, behind the
  facade's `yaml-fix` feature, which keeps comments and layout.
* The operations on one file are planned together. A value fix inside an object that a `SortKeys` of the same object
  moves is rebased onto the moved text, so both apply in one run. Only an overlap that remains in the joint plan, or a
  file that one fix creates or deletes and another edits, is a conflict; the later fix in diagnostic order is then
  skipped with a reason, never merged.
* `skif::apply_fixes` returns `FixPlan { changes, applied, skipped }` and touches no file system. A change is
  `Modify` (with its edits and the new contents), `Create` or `Delete`, one per file, sorted by path. The CLI writes the
  plan atomically per file (a temporary sibling, then a rename), refuses a file that changed after planning, and
  `--dry-run` prints the plan as a diff instead ([ADR 0024](0024-cli-contract.md)). A host may show the plan or merge it
  with its own edits.
* skif runs no formatter after fixing. The edits keep each file's indentation and newline style; a project runs its
  own formatter as after any edit.

### Selection

* `FixSelection { safety, rules, choices, alternatives, remove_files }` states what goes into the plan: the highest
  safety level, optionally only some rules, explicit picks per diagnostic, how a diagnostic with several fixes and no
  pick is handled, and whether files may be deleted.
* A rule returns every alternative fix in order, labelled by what it does. When configuration decides the answer, the
  decided one is marked `default`: `[policy] prefer` (`highest` unless set) or `--select highest|lowest` for one run
  decides the target of a version alignment ([ADR 0010](0010-version-semver-range-and-catalog-policies.md)).
* Without a pick or a default the diagnostic is skipped as `NeedsChoice`. No library crate prompts; the CLI turns
  `NeedsChoice` into a prompt when stdin is a TTY and `--no-prompt` is not given, and builds `choices` from the answers.
* `--fix-type <rule|knip-type>` (comma separated or repeatable) limits a run to the fixes of some rules. A knip issue
  type maps through the knip translation table to the rules that report the same findings (`dependencies` also covers
  `devDependencies` and `optionalPeerDependencies`); any other value is a usage error.
* A fix writes the specifier its rule chose verbatim (`1.2` stays `1.2`) and never rewrites a `catalog:` reference to a
  version.

### Safety

* `Safe` means a fix stays within what the current declaration already promises: sorting, removing an empty dependency
  map, setting `private: true` on the root, and a version alignment that is a compatible upgrade to an explicit target.
* A compatible upgrade keeps the shape of the specifier (a plain version, or a `^` or `~` range with the same number of
  parts), carries no prerelease or build suffix, never goes lower, stays inside the window `^` itself uses (same major
  from 1.0.0, same minor under 0.x, same patch under 0.0.x), and keeps the minor of a `~` range that names one. An
  identical plain specifier is always compatible. An alias, a complex range, a comparator, `workspace:`, a path, a git
  URL and a dist-tag never are.
* Under that test, the `version-mismatch` alignments to a pin, to the highest or lowest version in use, to a snap
  target and to a workspace package's own version, and the moves of `catalog-not-used` and `catalog-mismatch` onto an
  existing catalog entry holding a plain version or `^`/`~` range, are `Safe`; every other move of the same rules is
  `Unsafe`. A pin is no exception: the configuration decides where `--fix-unsafe` goes, not whether `--fix` may go
  there.
* Always `Unsafe`: `catalog-missing-entry` (skif chooses the new entry's version), `banned-dependency`,
  `semver-range-mismatch`, `policy-conflict`, `types-in-dependencies` (a production install may still need the types),
  `missing-package-manifest` (a new member is installed and locked), `unmatched-workspace-patterns` (the pattern may
  record a directory still to come), every removal of code or configuration, and any downgrade, including every
  alignment under `prefer = "lowest"`. `--select` decides which alternative is taken, never its safety.
* `--fix` applies `Safe` fixes. `--fix-unsafe` applies `Safe` and `Unsafe` fixes that edit files in place.
* A fix with a `DeleteFile` operation is skipped as `removes-files` unless `FixSelection::remove_files` is set
  (`--allow-remove-files`), whatever its safety. `--allow-remove-files` and `--fix-type` need `--fix` or
  `--fix-unsafe`. A fix run is refused under a truthy `CI`; a dry run is not.
* Safety is per finding, so a rule can be `Fixable::Sometimes`, and the safety of each fix is in the output
  (`fixes[].safety` in JSON, `fix (safe)` in the human report). No third safety level exists.

### Removal fixes

All of these are `Unsafe`:

* `unused-enum-members` and `unused-namespace-members` delete the member: a member alone on its line goes with that
  line, the comment lines directly above it and a trailing comment; otherwise the member goes with its comma. There is
  no fix when another member mentions the member's name, when a later enum member has no initializer and would be
  renumbered, when the member shares its line with an adjacent comment, or when a namespace member is declared with
  siblings in one statement.
* `unused-catalog-entries` removes the entry from the file that declares the catalog, and the catalog or the
  `catalogs` key when it empties, as one shared operation. An entry an `[[ignore]]` entry suppresses keeps its catalog.
* `unused-exports` removes an `export default <expression>` only when evaluating the expression has no effect (a
  literal, an identifier, a function, a class that runs no code, or an array or object of those).

### `package-json-format` and `skif format`

* `package-json-format` is a `manifest` rule, off by default, configured by a root-only `[format]` table (it cannot
  appear in an override). It checks each `package.json` the workspace does not ignore, in this order: top-level key
  order (`sort-first`, then the rest in byte order when `sort-packages` is on), the `bugs` shorthand (`format-bugs`),
  the `repository` shorthand (`format-repository`), the order of `exports` conditions at every depth
  (`sort-exports`, `default` always last), and one-level sorting of the fields `sort-az` names.
* The defaults are syncpack's, except `sort-exports = ["types", "default"]`: the order of conditions is resolution
  behaviour, and only moving `types` first and `default` last cannot change which file an import resolves to. A
  configured list that reorders other conditions is applied with an `Unsafe` fix. An empty list turns its step off.
* skif departs from syncpack where syncpack would lose or change data: keys sort in byte order, the order
  `unsorted-dependencies` uses, so the two rules never ask for opposite orders; a shorthand collapse happens only when
  the shorthand holds everything the object held; an array holding anything but strings or a negated glob is not
  sorted; nothing is reordered across a comment.
* Without `indent` the layout is kept: members move with their own text and whitespace is never reported. With
  `indent`, the file must be laid out as `JSON.stringify` would with that indent, the file's own line ending and a
  final newline; a manifest that is not strict JSON keeps its layout. Scalars keep their source text.
* The fix is one minimal `Text` replacement. Formatting twice gives the bytes of formatting once.
* `skif format [PATH]` runs that rule alone at `error`, applies its fixes and never runs the install step; `--check`
  reports without writing and exits 1 when a manifest is unformatted, and `--unsafe` also writes the unsafe reorder. It
  takes `--recursive`, `--config`, `--no-config`, `--color` and a single `--format FORMAT`; the repeatable
  `FORMAT=FILE` and `--output` of a lint run are not offered, since the command reports one rule. Writing is a fix run
  and is refused under a truthy `CI`.

### Consequences

* Good, because bytes outside an edit are identical after a fix, and comments in `pnpm-workspace.yaml` and
  `.yarnrc.yml` survive.
* Good, because a host previews, reviews or composes fixes, and the library cannot write to a wrong directory because
  it writes nothing.
* Good, because `skif --fix` aligns a workspace within each major as `sherif --fix` and `syncpack fix` do, while a new
  major, a downgrade or a narrower range still asks for `--fix-unsafe`, which neither tool separates.
* Good, because `--fix-unsafe` never deletes a file unasked, in the CLI or in a host, and a knip user's
  `--fix-type exports --allow-remove-files` has a one-to-one skif form.
* Good, because a syncpack user keeps the format gate, and the format check is suppressed, reported and fixed like any
  rule.
* Bad, because two editors and an operation layer are more code than re-serializing, and `yamlpatch` brings native
  code.
* Bad, because a skipped conflicting fix needs a second `--fix` run, and the formatter's range replacement conflicts
  with any other fix in its span until the next run.
* Bad, because one dependency can end half aligned after `--fix`, and a reader learns a fix's safety per finding.
* Bad, because a removed member or export can leave a blank line or a now-unused import for the formatter or the next
  run to clean up.
* Bad, because a project used to syncpack's collator order or its default `sortExports` sees keys move once.

### Confirmation

* `crates/skif/tests/fix.rs`: `overlapping_fix_is_skipped_and_the_earlier_one_wins`,
  `a_value_fix_and_a_sort_of_the_same_object_both_apply` and
  `create_and_delete_become_changes_only_when_files_may_be_removed`.
* `crates/skif-rules/src/policy/specifiers.rs`: `only_compatible_upgrades_of_the_same_shape_are_safe_alignments`;
  `crates/skif-rules/src/policy/report.rs`: `a_pin_is_safe_only_as_a_compatible_upgrade`.
* `crates/skif-cli/tests/cli.rs`: `fix_applies_compatible_version_alignments_and_leaves_the_rest`,
  `one_fix_run_aligns_and_sorts_the_same_dependencies`, `fix_unsafe_deletes_files_only_with_allow_remove_files`,
  `fix_type_limits_the_fixes_to_some_rules`, `fix_type_and_allow_remove_files_are_usage_errors_without_fix`,
  `fix_is_refused_only_when_ci_is_truthy`, `format_check_exits_one_and_format_writes_the_fix` and
  `format_writes_an_unsafe_exports_reorder_only_with_unsafe`.
* `crates/skif-cli/src/write.rs`: `a_file_that_changed_after_planning_is_refused`.
* `crates/skif-js-syntax/tests/module_info.rs`: `member_removal_ranges_keep_the_layout`,
  `enum_member_removal_never_renumbers_a_later_member`, `member_removal_takes_the_comments_of_the_member` and
  `default_expression_removal_keeps_side_effects`.
* `crates/skif-edit/src/package_json.rs` tests each formatting step, the defaults and idempotence, and
  `crates/skif-edit/tests/package_json_format.rs` snapshots the manifests under
  `crates/skif-edit/tests/fixtures/package-json/`.
* The conformance fixtures under `fixtures/policy` mark each diagnostic whose fix is `Safe` in `expected.json`, and
  `crates/skif/tests/policy_conformance.rs` compares it.

## Pros and Cons of the Options

### Structural fix operations, translated to format-preserving text edits and returned as a plan, with safety judged by the move a fix makes and formatting as a rule

* Good, because every untouched byte survives, fixes can be previewed and composed, and the safety line follows what
  each specifier promises.
* Bad, because it is the most code, and safety varies per finding of one rule.

### Re-serialize the edited document with detected indentation

* Good, because it is a few lines of code and works for any JSON shape.
* Bad, because it is not format-preserving in general and impossible for YAML with comments.

### Rules emit text edits directly

* Good, because it has one translation layer fewer.
* Bad, because every rule would re-derive offsets and separator style, and a host could not explain a fix.

### Write files from the library

* Good, because a caller would get the result of a fix run in one call.
* Bad, because the library would perform I/O the host did not ask for, and nothing could be previewed.

### A separate formatting pipeline with its own command

* Good, because formatting would not depend on the rule model.
* Bad, because suppression, overrides, output formats and exit codes would be built twice.
