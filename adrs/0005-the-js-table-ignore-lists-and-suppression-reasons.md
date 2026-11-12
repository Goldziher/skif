---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# The `[js]` Table, Ignore Lists and Suppression Reasons

## Context and Problem Statement

Projects that adopt skif's JS analysis usually come from knip, whose configuration has name lists with globs and regular
expressions, a production marker, per-workspace tables, ignored workspaces and a strict mode. A knip configuration
should translate into `[js]` key by key, and where skif differs the difference must be stated, not discovered.

Every suppression also hides a finding. A suppression nobody needs any more should be reported, a suppression several
roots read should be judged across them, and a team that audits its suppressions needs each one to say why it exists,
in the file, where a check can require it. How does `[js]` look, how are its lists matched, and how are suppressions
judged and explained?

## Decision Drivers

* A knip configuration maps onto `[js]` key by key
* An unused suppression is a finding with the usual severity and suppression machinery
* A setting shared by several roots is stale only when no root used it, and is reported once
* Every suppression can carry a reason, and a run can require one
* One run can isolate workspaces without production mode, and a file can set the mode CI depends on

## Considered Options

* knip's keys in kebab-case with stated deviations, a `stale-js-config` rule judged across roots, `{ name, reason }`
  entries, and `[js] isolate-workspaces` next to `[js] strict`
* Accept knip's JSON configuration as it is
* Reasons as TOML comments only

## Decision Outcome

Chosen option: "knip's keys in kebab-case with stated deviations, a `stale-js-config` rule judged across roots,
`{ name, reason }` entries, and `[js] isolate-workspaces` next to `[js] strict`", because a knip configuration then
translates mechanically, an unused entry is visible and can fail a run, and a reason sits on the entry it explains.

### The table

* `[js]` holds the knip-compatible keys: `enabled`, `entry`, `project`, `ignore`, `ignore-files`,
  `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, `ignore-members`, `ignore-workspaces`,
  `ignore-exports-used-in-file`, `include-entry-exports`, `production`, `strict`, `isolate-workspaces`, `workspaces`
  and `plugins`, with `extend-` forms of the ignore lists ([ADR 0004](0004-configuration-model-and-layering.md)).
* skif's own keys are decided by the records of their topic: the file walk, `discovery` and `include-fixtures`
  ([ADR 0014](0014-files-the-js-analysis-reads-walk-git-index-fixtures.md)); `tsconfig`, `compilers` and `cycles`
  ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)); `use-public-hoisting`
  ([ADR 0012](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md)); `ignore-global-binaries`
  ([ADR 0017](0017-scripts-binaries-and-working-directories.md)); plugin options
  ([ADR 0015](0015-plugin-model-and-plugin-set.md)).
* `enabled` defaults to `true`, so JS analysis runs wherever the `js-analysis` feature is compiled in; `--no-js`
  overrides it.

### Entries of the name lists

* `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, `ignore-members`, `ignore-workspaces` and
  `ignore-files` hold exact names, globs and regular expressions.
* An entry is a regular expression when it is written between slashes with optional flags (`/^@org\/.+$/`, `/^vite/i`;
  `i`, `m` and `s` apply) or when it contains one of `+ \ ( ) | ^`, `.*` or `.+`, or ends in `$`. knip also counts `*`,
  `{` and `$` anywhere; skif keeps `*` and `{` for globs, so `@types/*` and `eslint-*` stay globs. The expression is not
  anchored, as in knip, and is compiled by the Rust `regex` crate, so lookaround and backreferences are errors.
* `*` crosses `/` in the name lists, and stops at `/` in `ignore-workspaces` and in `[js.workspaces]` keys, which match
  directories.
* A trailing `!` marks production mode. In a name list or `ignore-workspaces`, an entry ending in `!` applies in
  production mode only. In `entry` and `project` it means the opposite, as in knip: a normal run reads `pattern!` as
  `pattern`, and a production run keeps only the patterns that end in `!`, or the defaults when none does. A leading `!`
  is a negation.
* An entry that does not compile is a configuration error naming the key and index.
* `ignore-unresolved` and `ignore-dependencies` both silence `unresolved-imports` and `stale-mocks` for a specifier,
  since plugins declare virtual modules through `ignore-dependencies` ([ADR
  0019](0019-dependency-unlisted-and-unresolved-findings.md)).
* `ignore-files` hides files from `unused-files` only.

### Per-workspace tables

* `[js.workspaces."<dir>"]` takes `entry`, `project`, `ignore`, `ignore-files`, `ignore-dependencies`,
  `ignore-binaries`, `ignore-unresolved`, `ignore-members`, `ignore-exports-used-in-file`, `include-entry-exports`,
  `paths` and `plugins`, with the `extend-` forms.
* A key is a directory or a glob relative to the root, or to the start directory under `--config` in a multi-root run
  ([ADR 0004](0004-configuration-model-and-layering.md)). An exact key beats a glob, and the first glob in file order
  wins. The key `"."` is the root package: its lists replace the top-level `entry` and `project` and add to the other
  lists, which apply to every workspace.
* `paths` is a `tsconfig`-style alias map relative to the workspace, applied ahead of the aliases tools declare.
* `plugins` of a workspace apply over `[js.plugins]` and under `[[overrides]]`.
* A key that selects no package is not a load error; `stale-js-config` reports it.

### Ignored workspaces

* `ignore-workspaces` takes JS analysis away from the packages in the matched directories: they contribute no files,
  entries or dependency findings. They stay in the workspace, so the manifest and policy rules still check them, which
  separates the key from `[workspace] ignore-packages`.
* A matched directory also leaves every package above it, as knip's `ignoreWorkspaces` does: a file is not analysed when
  a directory between its package's directory (excluded) and the file matches. A member below the matched directory
  that no glob matches keeps its own analysis. The root directory is never matched.

### Exports used in their own file

`ignore-exports-used-in-file` is `true` or a table of the kinds `variable`, `function`, `class`, `enum`, `namespace`,
`type` and `interface` (`member` accepted). `type` and `interface` are one kind, because skif does not tell a type alias
from an interface. An export is left unreported only when its own file references it. How a use inside the file counts
for members is decided in [ADR 0018](0018-export-namespace-and-member-rules.md).

### Production, strict and isolated modes

* `production` (also `--production`) analyses production entries and dependencies only.
* `isolate-workspaces` (also `--isolate-workspaces`) makes each workspace package use only the dependencies its own
  manifest lists: an import, a binary a script runs and a package a tool config names are not satisfied by a
  containing package or the root, and `use-public-hoisting` is ignored. Manifests above the analysed root still count.
  Nothing else changes: outside production mode every file is analysed, any dependency field satisfies an import, a
  listed `@types` package stands in for a type-only import, and a peer of a listed package stands in for a listing.
* `strict` (also `--strict`) is knip's strict mode: production mode, isolation and knip's direct-dependency rules
  (production code imports from `dependencies` or `peerDependencies`, type-only imports use no package, peers of listed
  packages do not stand in, required peers are checked like `dependencies`). With it on, `production = false` and
  `isolate-workspaces = false` have no effect.
* All three are top-level only. A flag turns a mode on, never off. In the library, `Js::production_mode` and
  `Js::isolated` give the effective modes. How the modes change dependency findings is in
  [ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md).

### `stale-js-config`

* A `js` rule, warning by default, that reports list entries that matched nothing in the run, `[js.workspaces]` keys
  that select no package, and knip's configuration hints: redundant `entry` and `project` patterns, top-level `entry`
  and `project` in a monorepo, `project` extensions skif does not analyse, published entry files that do not exist.
* `[rules] stale-js-config = "error"` is knip's `--treat-config-hints-as-errors`.
* It runs after the other `js` rules, since an entry is stale only when none used it. A list is not judged when every
  rule that consults it is off, and the dependency lists are not judged for a package with unknown entry files.
* The finding points at the configuration file that wrote the entry.
* A file several roots read is judged across them. Each report records every entry the rule judged, used or stale
  (`Report::config_judgements`), and `settle_shared_config` decides over the reports a run shows: an entry is stale only
  when every root that judged it found it stale; a root that could not judge it has no say; a stale entry is reported
  once, by the first root in path order whose report holds it, at the highest severity among the copies; any other
  finding at a shared file is reported once. Each package selected by a glob table has a list of its own.
* The decision is a pure function over final reports, made once and last: after the lint that follows `--fix`, after
  `--workspace`, `--include` and `--exclude` narrowed the reports, and after suppressions. Deciding a decided set again
  changes nothing. The CLI calls it, and an embedder that lints the roots of `discover_targets` itself calls it the same
  way. Baselines record each root's findings before the decision ([ADR 0023](0023-baselines.md)).

### Reasons

* Every `[js]` ignore list (at the top level, in `[js.workspaces]` and in an override's `js`), every `[[ignore]]` entry
  (also in overrides and package files) and every rule `allow` list takes an inline table `{ name, reason }` in place
  of a string. `name` is exactly what the string form holds, the production marker included. A list may mix both forms.
* A blank `reason` or an unknown key in the table is an error naming the key and index in the file that wrote it.
* Matching, validation and `stale-js-config` use the name only. An entry without a reason is written back as a bare
  string. The model has one type for an entry, `IgnoreItem`.
* `--require-ignore-reasons` or `[lint] require-ignore-reasons = true` (or-ed) fails the run with exit code 2 on the
  first suppression of a configuration file without a reason, named by its key and position in the merged list. Entries
  added by flags are exempt. `[workspace] ignore-packages` and `[js] ignore` select inputs rather than suppress a
  finding, stay plain globs and are not checked. The check is `Config::missing_ignore_reason`, so a host can run it too.
* `skif migrate` carries the comments of a knip configuration over as reasons ([ADR 0025](0025-skif-migrate.md)).

### Consequences

* Good, because a knip configuration translates key by key, and the deviations (`*` and `{` stay globs, the meaning of
  the marker on `entry`, the root key `"."`) are stated.
* Good, because an unused suppression is visible, can fail a run, and means that no root of the run used it.
* Good, because one default run with `isolate-workspaces` checks every package, tests and tool configs included, against
  its own manifest, so a project needs no second strict run whose configuration hints disagree with the first.
* Good, because each suppression can state why it exists, and one key requires it.
* Bad, because a pattern such as `foo.*` meant as a glob is read as a regular expression.
* Bad, because the dependency lists are not judged in a run where a package has unknown entry files.
* Bad, because a glob in `ignore-workspaces` also hides the root's files in the directories it names.
* Bad, because a host that lints roots itself must call `settle_shared_config`, or it reports a shared file's stale
  entry once per root.
* Bad, because a missing reason is named by its place in the merged list, which differs from its place in the file when
  `extends` or an `extend-` key added entries before it.

### Confirmation

* `crates/skif-js/tests/js_config.rs`: `ignore_dependencies_take_regular_expressions`,
  `the_production_marker_limits_an_ignore_entry_to_production`,
  `the_production_marker_selects_entries_in_a_production_run`, `a_star_in_ignore_workspaces_does_not_cross_a_slash`,
  `ignore_workspaces_takes_the_matched_directories_out_of_the_packages_above`,
  `the_root_workspace_key_configures_the_root_package`, `paths_resolve_aliases_for_one_workspace`,
  `exports_used_in_their_own_file_can_be_ignored`, `stale_js_config_reports_entries_that_matched_nothing`,
  `stale_js_config_does_not_judge_a_list_whose_rules_are_off` and
  `ignore_entries_with_a_reason_suppress_and_are_judged_like_strings`.
* `crates/skif/src/shared_config.rs` unit tests: `a_root_that_could_not_judge_an_entry_does_not_hide_it`,
  `the_kept_copy_takes_the_highest_severity_and_the_summary_counts_it`, `each_package_of_a_glob_table_has_its_own_list`
  and `deciding_decided_reports_again_changes_nothing`.
* `crates/skif-cli/tests/multi.rs`: `a_stale_entry_of_an_explicit_config_is_reported_once_for_the_run` and
  `a_discovered_file_several_roots_inherit_is_judged_across_them`.
* `crates/skif-js/tests/isolate_workspaces.rs`, and in `crates/skif-cli/tests/filters.rs`
  `isolate_workspaces_reports_a_members_use_of_an_ancestor_only_dev_dependency`, `isolate_workspaces_is_not_strict_mode`
  and `strict_and_isolate_workspaces_in_the_configuration_equal_their_flags`.
* `crates/skif-config/tests/config.rs`: `js_ignore_lists_take_strings_and_tables_with_a_reason`,
  `a_js_ignore_table_with_a_wrong_key_names_it`, `the_first_suppression_without_a_reason_is_named_by_its_key` and
  `strict_implies_production_and_isolation_and_isolation_implies_neither`.
* `crates/skif-cli/tests/cli.rs`: `require_ignore_reasons_covers_the_js_ignore_lists_and_the_lint_key`.

## Pros and Cons of the Options

### knip's keys in kebab-case with stated deviations, a `stale-js-config` rule judged across roots, `{ name, reason }` entries, and `[js] isolate-workspaces` next to `[js] strict`

* Good, because translation is mechanical and every deviation is written down.
* Good, because reasons and staleness use the machinery every other key and finding uses.
* Bad, because the public model's list fields are `IgnoreItem`s, which code reading them as strings must read by `name`.

### Accept knip's JSON configuration as it is

* Good, because no translation is needed.
* Bad, because it is a second format with its own precedence, and knip's TypeScript configurations need execution
  ([ADR 0001](0001-mission-scope-and-non-goals.md)).

### Reasons as TOML comments only

* Good, because it needs no schema change.
* Bad, because a comment cannot be required or checked, and it is lost when a tool rewrites the file.
