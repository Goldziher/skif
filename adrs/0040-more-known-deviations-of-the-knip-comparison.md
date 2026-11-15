---
status: accepted
superseded-in-part-by: ["0051", "NNNN"]
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# More Known Deviations of the knip Comparison

## Context and Problem Statement

The comparison harness of [ADR 0021](0021-knip-and-syncpack-comparison-harness.md) counts a one-sided finding as a
known deviation only when a mechanical test on the finding key, the manifests or an import scan explains it. The corpus
still counted as differences many findings with a known cause that no such test covered: binaries Rush or the shell
provides, `declare module` and `data:` specifiers, imports through the importer's own `paths`, enum members of an enum
used as a whole, files a manifest publishes as they are, the sources of Bazel rules and of a published `bin` stub, the
knip configuration the harness replaces, projects the corpus does not install, and an unresolved import knip reports
where skif reports a stale mock. They drowned the differences that need a fix. Which of them does the harness set
apart, and from what does it decide them: from the checkout, or from a new detail in skif's JSON report?

## Decision Drivers

* A deviation is decided by a test the harness runs on data it reads, never by listing findings
* skif's report carries what users need, not what the harness needs to explain knip
* A table skif uses (its global binaries, its shell builtins) is read from skif's crate, not copied
* A deviation never hides a finding of the other side, and the counts of the agreement table stay readable with the
  deviations set apart
* The decisions of skif's own records stand: a whole use of an enum credits every member
  ([ADR 0018](0018-export-namespace-and-member-rules.md)), and `paths` resolve before packages (ADR 0038)

## Considered Options

* Decide each deviation in the harness, from the files, manifests, TypeScript configurations and an import scan of the
  checkout with skif's parser
* Add a detail to skif's JSON report for each case and key the deviations on it
* Keep the findings counted as differences and explain them in the README only

## Decision Outcome

Chosen option: "Decide each deviation in the harness", because each case can be told from the checkout with the parser
and the resolver rules skif already follows, and none needs a report field users would read.

### Deviations added

* `rush-provided-command`: `rush`, `rushx` or `rush-pnpm` only knip reports as an unlisted binary in a repository with
  a `rush.json`.
* `shell-builtin` and `global-binary`: a binary only knip reports that skif's script analysis takes for a shell builtin
  or a system or machine tool; the harness reads `IGNORED_BINARIES`, `is_shell_builtin` and `is_machine_binary` from
  `skif-js-plugins`.
* `data-uri`, `ambient-module` and `own-tsconfig-paths`: an import only knip reports in `unlisted` that is a `data:`
  specifier, that a `declare module` of a script file of the checkout declares (matched as skif matches it), or that
  the `paths` of the importer's own configuration (nearest `tsconfig.json` or `jsconfig.json`, its `extends` chain,
  `baseUrl` and `${configDir}`) map to a file of the checkout outside `node_modules`.
* `replaced-knip-config`: an unused file only knip reports that is a knip configuration at the root of one of its
  workspaces, while knip ran with the harness's configuration instead of the repository's.
* `whole-enum-use`: an enum member only knip reports whose enum is used as a whole value in its file or through an
  import, found with the import scan of `imported-by-knip-unused-file` and the use flags skif's parser records. ADR
  0018 stands; the enum-member recall is read with these set apart.
* `published-file`: an export or type of a declaration file, or an unused JavaScript file and its exports, only knip
  reports, that the project's manifest publishes as it is through a published field, `bin`, `exports` or `files`.
* `paths-before-packages`: a dependency only skif reports unused, or a file only knip reports unused, where an import
  of the dependency's package maps through the importer's `paths` to that file of the same workspace.
* `uninstalled-project`: an install-dependent finding (dependencies, binaries, unlisted, unresolved) one tool reports
  in a project with no installed packages at or above it, in a checkout the harness installed.

### Deviations extended

* `published-source`: the sources a published `node` `bin` stub loads, and the files whose exports a published source
  re-exports.
* `non-member-project`: the re-exports of a nested root's default entries; on skif's side, an export, type or duplicate
  of a file of knip's workspaces that a file below a non-member project imports.
* `skipped-directory`: on skif's side, the same for importers below a directory skif's walk skips.
* `bazel-build-file`: the unused files the `srcs`, `data` and `entry_point` literals of `rules_js` rules name in their
  `BUILD` package, and the files they reach through files only knip reports unused.
* `imported-by-knip-unused-file`: imports through the importer's own `paths` and by a workspace package's name.

### Agreement

* An `unresolved` finding only knip reports whose file and specifier match a `stale-mocks` finding of skif (the
  `jest.requireActual` call it names) counts as shared: both tools report the same missing module.

### Not a root install for turborepo-examples

The corpus install leaves the turborepo-examples root uninstalled because its workspaces lie outside the sparse
checkout. `uninstalled-project` covers it and any other such layout; changing the install command would install
packages the harness does not compare.

### Consequences

* Good, because the differences left in the corpus are the ones that need a fix in skif or a record of their own.
* Good, because skif's report and crates are unchanged, apart from the harness reading two tables of `skif-js-plugins`.
* Bad, because the harness repeats parts of skif's resolution (`paths`, workspace packages, Bazel rule literals,
  `bin` stubs) and can drift from them; the unit tests pin each rule.
* Bad, because the import scans parse every code file of a checkout once more, which lengthens a parity run.
* Bad, because enum and namespace members whose only users lie in a non-member project or a skipped directory, a
  directory listed in `files`, the excludes of a Bazel `glob`, and a namespace used as a whole that never names the
  enum stay counted as differences.

### Confirmation

* `tools/bench/src/parity.rs`: the unit tests
  `an_unresolved_import_skif_reports_as_a_stale_mock_on_the_same_file_and_specifier_is_agreement`,
  `binaries_rush_provides_shell_builtins_and_skif_s_global_binaries_split_off`, `unlisted_data_uris_split_off`,
  `the_knip_config_the_harness_replaces_splits_off_at_a_workspace_root`,
  `tsconfig_paths_map_a_specifier_through_the_pattern_typescript_picks`,
  `unlisted_imports_a_declare_module_or_the_importer_s_own_tsconfig_paths_explain_split_off`,
  `dependencies_and_files_of_imports_tsconfig_paths_map_before_the_package_split_off`,
  `exports_files_only_knip_lists_unused_import_through_tsconfig_paths_and_workspace_packages_split_off`,
  `members_of_an_enum_used_as_a_whole_split_off`,
  `exports_a_published_source_or_a_nested_root_s_default_entry_re_exports_split_off`,
  `declaration_and_javascript_files_a_manifest_publishes_as_they_are_split_off`,
  `files_bazel_rules_build_from_and_what_they_import_split_off`,
  `the_sources_a_published_node_bin_stub_loads_are_published_sources`,
  `exports_only_skif_reports_whose_users_skif_does_not_read_split_off_on_skif_side` and
  `install_dependent_findings_of_a_project_without_installed_packages_split_off_on_both_sides`.
* The known-deviations section of `target/bench-results/parity.md` lists every id with its reason, rendered from
  `KNIP_DEVIATIONS`.

## Pros and Cons of the Options

### Decide each deviation in the harness

* Good, because skif's report stays what users read.
* Good, because a deviation is checked against the checkout, not taken from the tool it explains.
* Bad, because the harness keeps its own copy of some resolution rules.

### Add a detail to skif's JSON report for each case

* Good, because the harness would follow skif's own resolution exactly.
* Bad, because most cases are about why knip reports a finding, which skif's report has no reason to say.
* Bad, because every detail is a public report field to keep stable.

### Keep the findings counted as differences

* Good, because nothing new can drift.
* Bad, because the differences that need a fix stay hidden among those with a known cause.
