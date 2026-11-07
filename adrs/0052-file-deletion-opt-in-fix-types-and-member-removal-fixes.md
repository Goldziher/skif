---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
---

# File Deletion Opt-In, Fix Types and Member Removal Fixes

## Context and Problem Statement

[ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) defines `--fix-unsafe` as "apply `Safe` and `Unsafe` fixes". The fix of `unused-files` deletes a file, so `skif --fix-unsafe` deletes every file it believes unreachable together with every other unsafe edit. knip, whose JS rules skif follows ([ADR 0010](0010-js-analysis-on-oxc.md)), guards deletions with a separate `--allow-remove-files` and lets a fix run be narrowed with `--fix-type` ([#37](https://github.com/Goldziher/skif/issues/37)). A user migrating from knip ([ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md)) loses both guards. knip also removes unused enum members, namespace members, catalog entries and `export default <expression>`, which skif reports without a fix, and it runs the project's formatter on the changed files.

## Decision Drivers

* Deleting a file is harder to review and undo than an edit inside it; a fix run must not do it unless asked
* The library plans fixes for hosts other than the CLI ([ADR 0003](0003-library-first-embedding.md)), so the guard belongs in `FixSelection`, not only in a flag
* A knip user should be able to say what they said to knip: `--fix-type exports,types`
* skif's own process spawns nothing but the package manager's install step after a fix ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md))

## Considered Options

* A separate opt-in for deletions in `FixSelection` and the CLI, `--fix-type` over rule ids and knip issue types, and unsafe removal fixes for the remaining knip types
* Keep deletions in `--fix-unsafe` and document them
* Make the `unused-files` fix a third safety level

## Decision Outcome

Chosen option: "A separate opt-in for deletions in `FixSelection` and the CLI, `--fix-type` over rule ids and knip issue types, and unsafe removal fixes for the remaining knip types", because it matches knip's guard, keeps `FixSafety` at two levels, and puts the guard where every host goes through it.

### Deleting files

`FixSelection` gains `remove_files`, off by default. A picked fix with a `DeleteFile` operation is skipped with `SkipReason::RemovesFiles` unless it is set, whatever its safety. The CLI sets it with `--allow-remove-files`, which needs `--fix` or `--fix-unsafe`; a skipped deletion is reported with the reason `removes-files` in the human report and in the JSON `fix.skipped` list. `--fix-unsafe` therefore means "apply `Safe` and `Unsafe` fixes that edit files in place".

### Fix types

`--fix-type <TYPE>`, comma separated or repeatable, needs `--fix` or `--fix-unsafe` and limits the run to the fixes of some rules (`FixSelection::rules`). A value is a registered rule id or a knip issue type, mapped through skif-migrate's knip table to the rules that report the same findings; `dependencies` also covers `devDependencies` and `optionalPeerDependencies`, as knip's dependency fixer does. Any other value is a `usage` error. Alternatives of the rules left out are not put to the user.

### New unsafe fixes

* `unused-enum-members` and `unused-namespace-members` delete the member: its line when it stands alone on it, else the member with its comma. A member whose name another member of the enum or namespace mentions, or a namespace member declared with siblings in one statement, has no fix.
* `unused-catalog-entries` removes the entry from the file that declares the catalog, `pnpm-workspace.yaml`, `.yarnrc.yml` or the root `package.json`, through the format-preserving editors ([ADR 0008](0008-fix-and-edit-model.md)).
* `unused-exports` removes `export default <expression>` with its expression; the title says so, because side effects of the expression go too.

### No formatter run

skif does not run a formatter after fixing. Detecting and spawning prettier, biome or oxfmt would be a second process besides the install step, run with the project's own configuration and failure modes; the fixes already keep the file's indentation and newline style. Users run their formatter after `skif --fix`, as for any other edit.

### Consequences

* Good, because `--fix-unsafe` no longer deletes files unasked, and an embedding host gets the same guard by default.
* Good, because a knip user's `knip --fix --fix-type exports --allow-remove-files` has a one-to-one skif command.
* Bad, because a host that relied on `apply_fixes` deleting files must now set `remove_files`.
* Bad, because a removed `export default` expression or member can leave a blank line or a now-unused import, which the next lint run or the formatter cleans up.

### Confirmation

* `create_and_delete_become_changes_only_when_files_may_be_removed` in `crates/skif/tests/fix.rs` checks the default skip and the opt-in.
* `fix_unsafe_deletes_files_only_with_allow_remove_files`, `fix_type_limits_the_fixes_to_some_rules` and `fix_type_and_allow_remove_files_are_usage_errors_without_fix` in `crates/skif-cli/tests/cli.rs` run the binary on the `autofix` fixture and snapshot the changed files.
* `member_removal_ranges_keep_the_layout` in `crates/skif-js-syntax/tests/module_info.rs` checks the member ranges; the `unused-entries-reported` conformance case carries the expected fixed `pnpm-workspace.yaml`.

## Pros and Cons of the Options

### A separate opt-in for deletions in `FixSelection` and the CLI, `--fix-type` over rule ids and knip issue types, and unsafe removal fixes for the remaining knip types

* Good, because it matches knip and needs no new safety level.
* Bad, because it adds two flags to the CLI contract.

### Keep deletions in `--fix-unsafe` and document them

* Good, because nothing changes.
* Bad, because one flag keeps meaning both "edit" and "delete", which knip users do not expect.

### Make the `unused-files` fix a third safety level

* Good, because the level would be visible per fix.
* Bad, because `FixSafety` is part of the facade, JSON and SARIF, as [ADR 0051](0051-safe-version-alignment-fixes.md) found for version alignment.

## More Information

* Amends [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) (flag table, `--fix-unsafe`) and [ADR 0028](0028-cli-flag-contract-completion-and-withdrawn-confirmations.md) (flag contract); the index records the effective statements.
* Upstream: [knip auto-fix](https://knip.dev/features/auto-fix)
* See the [ADR index](index.md).
