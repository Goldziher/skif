---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0115"]
---

# Bazel Rule Sources Are Used Files, Not Entries

## Context and Problem Statement

[ADR 0115](0115-bazel-build-files-and-output-trees-in-the-js-analysis.md) makes the files a `rules_js` rule builds
from entries: the `entry_point` and `data` of `js_binary`, `js_run_devserver` and `js_test`, and the `srcs` and `data`
of `js_library` and `ts_project`, production entries except those of `js_test`. That keeps them out of
`unused-files`, but an entry's exports are public, so `unused-exports`, `unused-types`, the member rules and
`duplicate-exports` stop judging every file that some `srcs` lists. In a rules_js repository most source files are in
some `ts_project`, so those rules are nearly off. On the pinned angular corpus, the exports both tools report fell
from 787 to 680 and the types from 209 to 172 when the plugin landed. An export such as a helper that only its own
file uses is no longer found.

A `ts_project` compiles its `srcs`; it does not run them or publish their exports to unknown callers. Only the file a
rule runs, its `entry_point`, is a module whose exports a caller outside the analysis may read.

How should the JS analysis treat the files a Bazel rule names?

## Decision Drivers

* A file a `BUILD` file builds from is not `unused-files`, and what it imports is used
* Exports are public only where a caller the analysis cannot see may read them
* The plugin interface keeps entries and files that are only used apart, so other plugins can say the same
* `explain-usage` says why such a file is used and why its exports are judged

## Considered Options

* A build source: a file the module graph starts from that is no entry
* Keep `srcs` and `data` entries (ADR 0115)
* Make `srcs` and `data` entries and turn on `include-entry-exports` for them
* Count `srcs` and `data` in `ignore-files`

## Decision Outcome

Chosen option: "A build source: a file the module graph starts from that is no entry", because it keeps the files
and what they import used while judging their exports as any module's, and the `entry_point` keeps the public
exports a runner gives it.

* `skif-js-plugins` gains `PluginInput::BuildSource` and `PluginInput::ProductionBuildSource`, collected into
  `PluginOutput::build_sources` and its subset `production_build_sources`, as entries and production entries are.
  `skif-js` carries them in `WorkspaceInputs::build_source` and `production_build_source`; production mode uses the
  production ones only.
* A candidate file of a package that its build sources match and that no entry rule matches is a build source. The
  module graph starts from it as from an entry, so it is no `unused-files` finding and its imports are followed, but
  its exports are not public: `unused-exports`, `unused-types`, the namespace and member rules and
  `duplicate-exports` judge it. A file that an entry rule also matches (a `main` or `bin` of `package.json`, an
  `entry` glob, another plugin's entry) stays an entry.
* Build sources reach where plugin entries do: one that matches a file of a workspace package nested below the
  package (a root `BUILD` file naming the sources of a member that has no `BUILD` file) is a build source of that
  package, and one that names a file of another project below the root (a nested root) is handed to that root as
  plugin entry files are, where it is an entry.
* The `bazel` plugin gives the `entry_point` of `js_binary`, `js_run_devserver` and `js_test` as an entry, a
  production entry except for `js_test`. The `data` of those rules and the `srcs` and `data` of `js_library` and
  `ts_project` are build sources, production ones except for `js_test`. This amends the entries paragraph of ADR
  0115; its other paragraphs stand. The plugin reads no `deps`: they name targets, not files.
* `skif explain-usage` reports `build-source` for such a file, starts its chains of imports at build sources too, and
  sets `chain-from-build-source` when the shortest chain starts at one; the text report says "build source" and
  "reached from a build source through". Both JSON fields are left out when false.

### Consequences

* Good, because `unused-exports`, `unused-types`, the member rules and `duplicate-exports` judge the sources of a
  rules_js repository again, as knip does, which has no Bazel plugin.
* Good, because the files a `BUILD` file names and what they import stay used.
* Good, because another plugin can name files that are used but no entries without a change to the analysis.
* Bad, because an export that only another Bazel target reads through a path skif does not resolve (a module name only
  Bazel maps) is reported unused; a `@public` JSDoc tag or `ignore-tags` of `unused-exports` keeps it.
* Neutral, because a `data` file that a runner loads by path, not by import, now has its exports judged; such a file
  rarely exports anything.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs`: `bazel_build_files_credit_linked_dependencies_and_add_entries` (the
  `entry_point` of `js_binary` a production entry, of `js_test` an entry, the `srcs` and `data` of `js_library` and
  of `js_binary` production build sources, the `data` of `js_test` a build source),
  `bazel_build_files_name_nothing_without_rules_js` and
  `bazel_root_labels_from_nested_packages_count_and_output_trees_do_not` (no build sources).
* `crates/skif-js/tests/fp_resolve.rs`: `bazel_sources_are_used_and_their_exports_judged` (the `srcs` of a
  `ts_project` and the file one of them imports are no `unused-files`, an unused export and type of a source are
  `unused-exports` and `unused-types`, the export of a `js_binary` `entry_point` is not reported, and a file nothing
  names is `unused-files`), `a_bazel_source_that_is_also_an_entry_keeps_public_exports` (a source that is the `main`,
  a default entry or an `entry` glob keeps public exports), `bazel_test_data_is_no_production_build_source`,
  `duplicates_and_members_of_a_bazel_source_are_reported` and `a_root_bazel_source_in_a_member_is_a_build_source`.
* `crates/skif-js/tests/explain_absence.rs`: `a_bazel_source_is_a_build_source_whose_exports_are_judged`,
  `the_text_report_names_build_sources` and `a_published_bazel_source_is_an_entry_not_a_build_source`.
* `crates/skif-cli/tests/multi.rs`: `a_bazel_source_of_a_nested_root_is_used_there`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A build source: a file the module graph starts from that is no entry

* Good, because files, imports and exports are each judged as the rule's role says.
* Bad, because the plugin interface, the setup, the entries and the module graph carry one more kind of file.

### Keep `srcs` and `data` entries (ADR 0115)

* Good, because nothing changes.
* Bad, because the export, type and member rules stay nearly off in a rules_js repository.

### Make `srcs` and `data` entries and turn on `include-entry-exports` for them

* Good, because no new kind of input is needed.
* Bad, because `include-entry-exports` is a setting of the package, not of a file; making it per file is the same
  distinction under another name, and `duplicate-exports` and `explain-usage` would still treat the files as entries.

### Count `srcs` and `data` in `ignore-files`

* Good, because the files are not `unused-files` and their exports are judged.
* Bad, because an ignored file is not followed: what only a source imports would be `unused-files`, and the
  dependencies only it imports `unused-dependencies`.

## More Information

* Amends: [ADR 0115](0115-bazel-build-files-and-output-trees-in-the-js-analysis.md) (the entries paragraph of the
  decision outcome)
* rules_js: <https://github.com/aspect-build/rules_js>
* See the [ADR index](index.md).
