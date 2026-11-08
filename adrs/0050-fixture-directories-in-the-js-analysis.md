---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0023"]
---

# Fixture Directories in the JS Analysis: Not Unused by Default, `include-fixtures` to Report Them

## Context and Problem Statement

[ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md) made root discovery skip fixture directories and said that the setting does not affect the JS analysis. Inside one root, the analysis only used the names `fixtures` and `__fixtures__` to stop holding a fixture's imports against the project: `unresolved-imports` and `unlisted-dependencies` are silent there. Every other JS rule judged a fixture file like production code. On the pinned corpus both skif and knip report fixtures as unused files: babel has 17,094 such findings, 96% of its unused files, and next-js, mastra and vscode have the same shape. The files are test data that tests load by path, so every one of those findings is false, and the agreement between the two tools hides it.

Should the JS analysis report files and exports in fixture directories as unused?

## Decision Drivers

* A finding nobody can act on costs more than a missing one: it buries the real ones
* There is one notion of a fixture directory, not one per rule
* A behaviour that hides findings must be visible in the configuration and switchable
* Agreement with knip is a measure, not the goal

## Considered Options

* Do not report files and exports in `fixtures` and `__fixtures__` by default, with `[js] include-fixtures` to report them
* Keep reporting them and document `[js] ignore = ["**/fixtures/**", "**/__fixtures__/**"]`
* Use the `[workspace] discover-ignore` list for the JS analysis too

## Decision Outcome

Chosen option: "Do not report files and exports in `fixtures` and `__fixtures__` by default, with `[js] include-fixtures` to report them", because the default should be right for the repositories that have such directories, which is nearly every large one, and the switch keeps knip's behaviour one line away.

**Which directories.** A file is in a fixture directory when any directory of its path relative to the workspace root is named `fixtures` or `__fixtures__`. The names are `skif_config::FIXTURE_DIRECTORIES`, which are also the first entries of the `discover-ignore` default, so the analysis and root discovery share one list of names. Other names of test data (`fixture`, `testdata`, `__testfixtures__`, `api-fixtures`) are not included: they hold real code in some repositories, and `[js] ignore` is the place for them.

**What is not reported.** In a fixture directory, `unused-files` reports no file, and `unused-exports`, `unused-types`, `unused-namespace-exports`, `unused-namespace-types`, `unused-enum-members` and `unused-namespace-members` report nothing about a module. `unresolved-imports` and `unlisted-dependencies` stay silent there as before. The other rules are unchanged: `duplicate-exports`, `unparseable-files` and `circular-imports` still see a fixture file that an entry reaches.

**What is still analysed.** A fixture file is not an entry and not ignored. When an entry reaches it, it is read and its imports count as uses; when nothing reaches it, it is not read, so a dependency only an unreached fixture imports is still reported by `unused-dependencies` or `unused-dev-dependencies`. Fixtures are often deliberately broken or import packages the project never installs, so crediting their imports would hide real unused dependencies; a package that tests load through a fixture belongs on `ignore-dependencies`.

**`include-fixtures`.** `[js] include-fixtures = true` reports fixture files and their exports like any other file. The default is `false`. The key is root-scoped, like `include-entry-exports` at the top level: it cannot appear in an override or a package file. `skif migrate` does not write it, although knip reports fixtures; the migration guide names the key.

**Amendment of ADR 0023.** `[workspace] discover-ignore` still concerns root locations only and does not change what the JS analysis reads or reports. The sentence of ADR 0023 that fixture directories do not affect the JS analysis now holds for that setting alone: the names `fixtures` and `__fixtures__` silence the reporting rules above inside every root.

### Consequences

* Good, because babel, next-js, mastra and vscode lose tens of thousands of false `unused-files` findings.
* Good, because the analysis and root discovery use one constant for the names.
* Bad, because a dead file in a fixture directory is not reported unless the user asks.
* Bad, because skif now disagrees with knip on every fixture file knip reports; the comparison counts those as the `fixture-file` deviation.
* Neutral, because a project that wants knip's behaviour sets one key.

### Confirmation

* `fixture_dirs_snapshot`, `files_in_fixture_directories_are_not_unused_by_default` and `include_fixtures_reports_fixture_directories_like_any_other` in `crates/skif-js/tests/fixtures.rs` cover the default and the switch.
* `root_discovery_skips_the_fixture_directories_the_js_analysis_knows` and `root_only_js_keys_are_rejected_in_an_override` in `crates/skif-config/tests/config.rs` cover the shared names and the scope of the key.
* `skif schema` equals `schema/skif.schema.json` and lists `include-fixtures`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Do not report files and exports in `fixtures` and `__fixtures__` by default, with `[js] include-fixtures` to report them

* Good, because the default is right for the repositories that have such directories.
* Bad, because it is a change of behaviour that the changelog must name.

### Keep reporting them and document `[js] ignore = ["**/fixtures/**", "**/__fixtures__/**"]`

* Good, because nothing changes.
* Bad, because every user of a large repository writes the same lines, and `ignore` also silences the imports of those files for every rule.

### Use the `[workspace] discover-ignore` list for the JS analysis too

* Good, because one setting would govern both.
* Bad, because its default includes `__mocks__` and `__snapshots__`, which jest loads by convention and whose files can be dead, and because it is read from the start directory of a recursive run, not from each root's configuration.

## More Information

* Amends: [ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md)
* Related decisions: [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md), [ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md), [ADR 0049](0049-js-corrections-from-adjudicating-knip-disagreements.md)
* Issue: [#28](https://github.com/Goldziher/skif/issues/28)
* See the [ADR index](index.md).
