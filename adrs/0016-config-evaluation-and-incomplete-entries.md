---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Config Evaluation and Incomplete Entries: A Sound Static Evaluator and Narrowed Unjudged Scopes

## Context and Problem Statement

Plugins read tool configs statically ([ADR 0015](0015-plugin-model-and-plugin-set.md)). Real configs compute values:
they merge a shared base, branch on the environment, spread a helper's result, filter a list of reporters, or call
functions of other modules that change what they are given. A value the evaluator gets wrong is worse than one it does
not know: a wrong entry list reports used files as unused, and `--allow-remove-files` deletes them. A value it does not
know hides where entries lie, but suppressing every finding of a package for one unknown key hides real dead code too.
Published paths in `package.json` raise the same question for packages that are not built yet.

What does the evaluator compute, when is a package's set of entries incomplete, and what is still judged in an
incomplete package?

## Decision Drivers

* A known value is never one the config's JavaScript could change at run time; imprecision is allowed, unsoundness is
  not
* Unknown entries never turn into unused-file findings, and an incomplete package says why
* Only what an unknown config could make an entry stays unjudged; everything else is judged as in a complete package
* Plugins that read lists of names see every branch of a choice, while every other reading stays as it was
* A published path into nothing is reported, not silently treated as an unknown entry

## Considered Options

* A static evaluator that is sound and may be imprecise, with incompleteness scoped per plugin and per cause
* Execute configs in a sandboxed runtime
* Treat any unknown value as "the whole package is unjudged"
* Guess entries from naming conventions alone

## Decision Outcome

Chosen option: "A static evaluator that is sound and may be imprecise, with incompleteness scoped per plugin and per
cause", because it never executes code, never reports a file an unknown config may load, and keeps judging the files,
exports and members no unknown entry can reach.

### The evaluator

* The evaluator in `skif-js-syntax` computes `ConfigValue`s from the config's AST: literals, objects and arrays,
  constants of the same file, imports the evaluator follows, and calls it understands. Anything else is
  `ConfigValue::Unknown`, or an object with an unknown spread or part.
* Soundness rule: a result may be imprecise (unknown where JavaScript would know), but a value the evaluator reports as
  known is never one the config's JavaScript could change. A write through an alias, a call of a function that may
  change a module's value, a re-export or hand-out of such a function, a module imported for its effects, and `this`
  where it may be a module value leave the values they may reach partly unknown.
* Imports the evaluator neither follows nor finds installed are not trusted: a bare specifier that names a workspace
  package or that no `node_modules` directory at or above the config holds (a `tsconfig` alias, say) is project code
  whose effects are unknown. A `define…` helper, a copying merge and a package whose methods keep nothing are trusted
  only for an installed package. In a project with no installed packages every name that is no workspace package
  counts as installed, and under Yarn Plug'n'Play a registry dependency the package or the root declares is installed.
* `mergeConfig`, `defu`, `deepmerge`, `merge` and `Object.assign` are evaluated as merges of their object arguments,
  an unknown argument adding an unknown spread. `undefined` is `null`. `.filter(Boolean)` drops falsy elements of a
  known array.
* `ConfigValue::OneOf` holds two to eight known alternatives, from a ternary or `if` with an unknown test, two returns
  of a function, conditional spreads and `??` over a read that is not definite. `is_unknown` is true for it, so every
  reading treats it as unknown unless it asks for `alternatives`; plugins that read lists of names (reporters, Storybook
  addons, file lists) credit every alternative.
* `dirname(require.resolve('<pkg>/package.json'))` and its `import.meta.resolve` forms are that package's directory
  only for a bare specifier. A bundler alias whose target is a package makes that package a dependency of the config,
  and its imports are not reported.

### Incomplete entries

* A config that evaluates to an unknown value, has an unknown spread, computes its entry keys, or does not parse makes
  its package's entries incomplete. A declarative plugin opts out with `reads_config`. A published path that maps to no
  source file in a build output directory, a gitignored published path and an existing bin stub that loads output
  without a source (`unmapped-published`) do so too, since publishing may expose any file.
* The causes are `unparsable-config`, `unknown-config`, `unknown-spread`, `computed-entries`, `unmapped-published`,
  `no-entries` and `unspecified`, reported per package in `incomplete-packages`
  ([ADR 0022](0022-reporting-and-output-formats.md)).
* `Plugin::incomplete_scope` says where the entries an incomplete config hides can lie: `Package` (the default) or
  `Within(globs)`, relative to the package directory. A config that does not parse and a cause with no scope are
  `Package`, and one `Package` among a package's causes makes the whole package unjudged.
* A plugin narrows only for what a shared base can set, because a base can name files only where the tool looks by
  convention. A key the config computes itself, an unknown spread of a value the config computes (`OWN_SPREAD`, told
  apart from the shared `UNKNOWN_SPREAD`), or a config unknown as a whole may name any file and is `Package`. The
  plugins that narrow are `next`, `vite`, `vitest`, `playwright` and `storybook`, each stating its scope in its module.
* A Vitest `test.reporters` name that cannot be known is skipped without widening the scope, since a shared base's
  reporters belong to the shared package; a Playwright reporter list the config computes itself is `Package`; a
  Storybook addon, framework or builder that cannot be known narrows to the Storybook config directory and the
  plugin's project globs. esbuild scripts are read for the `entryPoints` they write (`read_script`) and are unparsable
  only when they write none.

### What an incomplete package still judges

* The files the scope matches are entries of the module graph, including files outside the package's `project` globs
  and files of nested workspace packages the scope matches, so everything an unknown entry may import is counted.
* Files outside the scope are judged by `unused-files`, and their exports, types and members by the export and member
  rules and the unneeded-tag hints. Exports of a module in the scope stay unjudged, also with
  `include-entry-exports`.
* The dependencies of an incomplete package are never judged: an unread base names packages by string, often from a
  package the evaluator does not reach.
* An `import()` or `require()` with a non-static argument may load any file, so it drops the scope and leaves the whole
  package unjudged.

### Published paths

* A published path names nothing when it lies in the package, outside a build output directory, git does not ignore
  it, it is neither a file nor a directory, and no source maps to it. Such a path is no entry, does not make the
  package incomplete, and is reported by `stale-js-config` with the `package-entry` hint.
* A path at or in `bin` is build output not built yet when the package has a build step that may write it: a build,
  prepack or prepublish script, a script running a compiler or bundler, one naming an output in `bin`, a bundler config
  an enabled plugin reads, or a `tsconfig` `outDir` at or inside `bin`. Such a path keeps the package unjudged as
  `unmapped-published`.
* The ignore files that decide whether a path is gitignored count from the root of the git work tree, together with
  the repository's `info/exclude`.

### Specifiers and test entries

* A package-like specifier (`@scope/x`, `pkg/sub`) that resolves to nothing is a dependency of the package it names,
  judged by the dependency rules, not an unresolved import.
* A relative specifier git ignores, and a JSON or foreign file (images, fonts, shell scripts, YAML) imported from a
  script, are not unresolved; a missing stylesheet import is.
* Test files are entries only through a test-runner plugin or `[js] entry`.

### Explaining it

`explain-usage` names each case: an entry of kind `incomplete-scope` for a file in the scope, `entries-incomplete` and
the verdict `not-judged` for a file or export the rules do not judge, and `widened-by` for the computed load that
dropped a scope.

### Consequences

* Good, because no finding rests on a value the config could change, and no file an unknown entry may load is reported
  or deleted.
* Good, because one unknown key does not hide every dead file of a package: the files, exports and members outside
  the narrowed scope are judged.
* Good, because plugins that read lists credit every branch of a choice.
* Good, because a stale published path is reported where it is written instead of silencing the package.
* Bad, because soundness costs precision: a config that calls project code may leave more values unknown than a human
  reader would.
* Bad, because the dependencies of an incomplete package are never judged.
* Bad, because a narrowed scope that is wrong costs more because exports outside it are judged, so narrowing is kept
  to what a shared base can set.

### Confirmation

* `crates/skif-js-syntax/tests/config_functions.rs`: `merge_config_merges_the_known_objects_and_marks_the_imported_one`,
  `an_undecided_condition_between_known_values_is_one_of_them`,
  `a_spread_of_one_of_several_values_adds_the_keys_of_each`, `an_array_spreads_the_elements_of_every_branch`,
  `filter_boolean_keeps_what_may_be_truthy`, `only_a_bare_manifest_directory_is_a_package`,
  `an_unknown_spread_the_config_computes_itself_is_its_own` and
  `a_call_not_followed_through_its_parameters_may_reach_all_of_its_argument`; for soundness,
  `a_change_through_an_alias_a_call_or_a_closure_reaches_the_original`,
  `a_call_that_is_not_known_may_change_its_argument_in_any_position`,
  `a_define_helper_of_a_project_module_may_change_what_it_is_given` and
  `a_define_helper_or_merge_the_evaluator_cannot_follow_is_unknown`.
* `crates/skif-js-syntax/tests/config_imports.rs`: `imports_are_unknown_without_a_loader`,
  `a_define_helper_of_an_alias_the_loader_does_not_follow_is_unknown`,
  `a_call_of_an_alias_the_loader_does_not_follow_changes_the_imported_values`,
  `a_factory_that_changes_a_binding_of_its_module_leaves_the_imported_values_open`,
  `a_barrel_that_exports_an_imported_changing_function_changes_the_imported_values`,
  `a_write_through_this_that_may_reach_a_value_of_a_module_changes_the_imported_values` and
  `a_reexport_of_a_file_that_holds_no_code_changes_nothing`.
* `crates/skif-js/src/config_modules.rs`: `a_package_is_installed_where_a_node_modules_directory_holds_it`,
  `under_plug_n_play_a_declared_dependency_is_installed` and
  `without_node_modules_every_name_of_no_workspace_package_is_installed`.
* `crates/skif-js-plugins/tests/fp_config.rs`: `incomplete_configs_are_scoped_to_what_the_tool_could_make_an_entry`,
  `computed_unparsable_and_unscoped_configs_stay_package_wide` and `esbuild_build_scripts_name_their_entry_points`.
* `crates/skif-js/tests/fixtures.rs`: `incomplete_configs_narrow_what_is_not_judged`,
  `exports_outside_a_narrowed_incomplete_scope_are_judged`,
  `exports_in_a_narrowed_scope_stay_unjudged_with_include_entry_exports`,
  `a_scope_file_outside_the_project_globs_still_imports` and `a_root_scope_counts_the_imports_of_nested_packages`.
* `crates/skif-js/tests/explain_absence.rs`: `an_export_outside_the_narrowed_scope_is_judged`,
  `a_computed_import_keeps_the_package_unjudged` and `incomplete_packages_carry_the_scope_left_unjudged`.
* `crates/skif-js/tests/config_hints.rs`: `published_paths_that_name_no_file` and
  `a_bin_path_a_compiling_command_may_write_keeps_the_package_unjudged`.

## Pros and Cons of the Options

### A static evaluator that is sound and may be imprecise, with incompleteness scoped per plugin and per cause

* Good, because findings are never based on a guess and most of an incomplete package is still judged.
* Bad, because each plugin's scope is knowledge to maintain, and soundness leaves some readable configs unknown.

### Execute configs in a sandboxed runtime

* Good, because every computed value would be exact.
* Bad, because it needs a JavaScript runtime and runs untrusted code
  ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)).

### Treat any unknown value as "the whole package is unjudged"

* Good, because it is simple and never wrong.
* Bad, because a shared base config with one computed key hides every dead file of every package that uses it.

### Guess entries from naming conventions alone

* Good, because it needs no evaluator.
* Bad, because a config that names a file outside the conventions gets it reported as unused.
