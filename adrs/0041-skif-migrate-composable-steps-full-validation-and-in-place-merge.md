---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# `skif migrate`: Composable Migration Steps, Full Validation and an In-Place Merge

## Context and Problem Statement

[ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md) made `skif migrate` a one-shot translator. Three of its statements did not survive contact with the code and with the tools it translates. First, the `exact` profile writes `off` for every default rule the migrated tools did not run and disables the JavaScript analysis when knip is not among them, and `--merge` gave an existing value priority over a generated one. The documented way to combine tools, `skif migrate --from sherif` and then `skif migrate --from knip --merge`, therefore produced a configuration in which nothing but sherif was checked. Second, the output was validated with a stripped copy when the configuration model lacked a key; every such key exists now, and the fallback turned real errors into a success followed by a configuration skif rejects. Third, `--merge` was said to lose the existing comments, but it edits the file in place with `toml_edit`.

Real configurations also showed gaps that cannot be fixed with a note: syncpack's `**`, `hosted-git`, `resolutions`, `overrides` and `pnpmOverrides` were dropped or reported as internal errors, a rule sherif ignores but syncpack checks was turned off, flags of one script became global without a word, and knip's issue types were computed by a simplified rule.

## Decision Drivers

* Migrating tools one after the other must end with the same checks as migrating them together
* A value the user wrote must never be overridden by a value the profile generated, and the reverse must be possible
* Configuration content must never produce an "internal bug" error
* A setting that skif can only apply globally must not silently widen a setting that a tool scoped narrowly
* The translation of knip's issue types should follow knip, not an approximation of it

## Considered Options

* Mark the values the `exact` profile adds and record the migrated tools in the file, so a later `--merge` can replace and complete them
* Recompute the profile in `--merge` from the union of the tools the file records
* Drop the advice to combine tools and require every tool in one run

## Decision Outcome

Chosen option: "Mark the values the `exact` profile adds and record the migrated tools in the file", because it composes with the information that exists at merge time (the old tool files of the earlier step are not available then) and keeps `--merge` a plain edit of the user's file.

**Profile values are marked.** Each `off` the `exact` profile adds, and `[js] enabled = false`, carries a trailing comment `# skif migrate: not checked by <tools>`. The banner records `# skif migrate: sources = ["sherif", "knip"]`. In `--merge`, a marked value is not an existing user value: it is replaced by a generated value that is not marked (the new tool checks the rule), united with another marked value (the comment then names both tools), and removed when the generated file does not repeat it (the new tool checks the rule and the rule returns to its default). A marked generated value for a key the existing file does not mention is skipped when the file records its sources, because the earlier tools run that rule. Values without the marker, including `off` values that come from a tool's own configuration, keep priority. The sources line is updated, or appended when the file had none.

**Ignored rules are decided at the end.** A rule one tool ignores (`sherif` `ignoreRule`) is turned off only when no other migrated tool still runs it. Otherwise it stays on and the report says so as a note that needs a human. A rule set to a level by a translated configuration is never also listed among the checks the profile turned off.

**Full validation.** The generated text is parsed as a `skif.toml` in full; the stripped fallback, the list of pending keys and the validation view are deleted. Content of the old configuration never reaches the error that means a bug in `skif-migrate`: a specifier type that skif does not know is dropped or reported (`hosted-git` is `git`), `**` is no filter, a custom type whose name is a built-in token is renamed with a note, and a group that selects only what skif cannot read is not written.

**syncpack scope.** `resolutions`, `overrides` and `pnpmOverrides` map to the dependency types of [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md) and are part of the default `[policy] dependency-types` the migration writes, which is syncpack's default scope. A `policy: catalog` group becomes the global `[catalog] mode = "prefer"` only when it selects every dependency; otherwise it is reported.

**Several runs of a tool.** Each command of a script is one run of the old tool but skif has one configuration. When runs disagree, the issue types and dependency types are united with a note, `[js] production` is written only when every run uses production mode, and a script that names the tool in a command the parser does not recognise is reported.

**knip.** The issue types a run reports follow knip 6's `getIncludedIssueTypes`: flags override the configuration, `--dependencies` covers the catalog types and brings in the dev dependencies outside production mode, production mode leaves out `devDependencies`, `catalog` and `catalogReferences`, and an include of only `nsExports` or `nsTypes` adds the defaults. `classMembers` is not an issue type. Settings knip scopes to a workspace (`ignoreMembers`, `ignoreExportsUsedInFile`) are written to that workspace's table.

**`--force`** replaces the configuration file that exists (`skif.toml` or `.skif.toml`) instead of adding a second one.

### Consequences

* Good, because `--from sherif` followed by `--from knip --merge` and `--from sherif,knip` check the same rules, tested for several orders.
* Good, because `--merge` keeps the comments and the order of the existing file and appends new tables at the end.
* Good, because no content of an old configuration can produce an internal error.
* Bad, because a rule that sherif ignores and that syncpack keeps is only seen when both tools are migrated in the same run; migrated in steps, the first step's `off` is a tool value and wins.
* Bad, because the generated file now carries comments that are part of how it is merged.

### Confirmation

* A golden test migrates three tools in several orders, merging each step into the previous output, and compares the `[rules]` table and `[js] enabled` with a single run; a CLI test does the same with two tools.
* Golden tests cover `**`, `hosted-git`, `delete`, the override sections, renamed custom types, `policy: catalog` with and without selectors, bare tool invocations, disagreeing runs, per-workspace knip settings, knip shortcut flags and production mode, and scripts behind wrappers.
* Merging the output of a migration into itself changes nothing, and the exact merged text of a file with new tables is asserted.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Mark the values the `exact` profile adds and record the migrated tools in the file

* Good, because the merge needs nothing but the two files.
* Good, because the markers show a reader which `off` values are generated.
* Bad, because the format of two comments becomes part of the contract between consecutive runs.

### Recompute the profile in `--merge` from the union of the tools the file records

* Good, because no marker is needed on single values.
* Bad, because which rules a tool runs depends on its configuration and scripts, which are not available when the second step runs.

### Drop the advice to combine tools and require every tool in one run

* Good, because it needs no code.
* Bad, because the advice already exists in the documentation and users follow it; the failure is silent.

## More Information

* Supersedes the paragraphs "Typed target, checked output" (the table of pending keys and the validated copy, and the statement that `--merge` loses the existing comments) and "Profiles" (the `exact` profile as a plain set of `off` values) of [ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md).
* Related decisions: [ADR 0034](0034-js-configuration-parity-with-knip.md), [ADR 0037](0037-override-sections-custom-types-and-ignored-semver-groups.md)
