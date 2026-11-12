---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Configuration Model and Layering

## Context and Problem Statement

The tools skif replaces are configured in a `package.json` key, in JSON, YAML or JavaScript files, and in TypeScript
files that are executed to be read. A `package.json` key turns every manifest fix into a configuration change,
JavaScript configuration requires executing user code ([ADR 0001](0001-mission-scope-and-non-goals.md)), and several
formats need several parsers and a precedence rule between them. The configuration surface is also large and
rule-specific, and options typed as a loose map push validation into every rule and fail at lint time.

Large repositories add a second problem. One repository holds many independent projects, nested projects and legacy
subtrees. An organisation states policy once at the top, each project adds to it, legacy directories turn rules off,
and the JS analysis needs inputs per package. A run inside a project and a recursive run from the top must give that
project the same configuration, and every effective value must be traceable to the file that set it. What is the
configuration file, and how do files combine?

## Decision Drivers

* One format, no executed configuration, and `package.json` stays a manifest
* A misconfiguration fails before any rule runs, with the file, line and key
* Policy written once at the repository root reaches every project below unless the project opts out explicitly
* A project, a package or a directory can add to, lower or turn off what it inherits
* The same project gets the same configuration however a run reaches it
* Workspace-wide analyses stay workspace-wide; only reporting and per-package JS inputs vary per package
* Every effective value has a provenance

## Considered Options

* One TOML format with typed, rule-owned options, cascading files with a `root = true` stop, `extends`, `[[overrides]]`
  and package files
* Nearest file wins, inheritance only through an explicit `extends`
* One root file with per-package sections only
* A `skif` key in `package.json`, or JSON and YAML next to TOML

## Decision Outcome

Chosen option: "One TOML format with typed, rule-owned options, cascading files with a `root = true` stop, `extends`,
`[[overrides]]` and package files", because one parser and one precedence rule fail a misconfiguration at load time,
policy reaches a project that writes its own file by default, and every way out is an explicit, greppable key.

### Format and shape

* A configuration file is `skif.toml` or `.skif.toml`. Both in one directory is `ConfigError::Ambiguous`, naming both.
  There is no `package.json` key and no JSON, YAML or JavaScript configuration.
* Keys are kebab-case. The top-level sections are `[workspace]`, `[lint]`, `[rules]`, `[[ignore]]`, `[policy]`,
  `[[version-groups]]`, `[[semver-groups]]`, `[[custom-types]]`, `[catalog]`, `[workspace-protocol]`,
  `[[dependency-families]]`, `[format]`, `[update]`, `[[update-groups]]`, `[js]` and `[[overrides]]`, plus the
  top-level keys `root`, `extends` and `reset`. An unknown section or key is an error with the file and line.
* `[rules]` maps a rule id to a level (`off`, `info`, `warn`, `error`) or to a table with `level` (or its alias
  `severity`) and the rule's options.
* Selectors (package and dependency names and globs with `!` negation and `$LOCAL`) are compiled once when the
  configuration is resolved; an invalid pattern is a configuration error naming the key.
* A `Config` builder lets a host construct a configuration without a file.

### Options belong to rules

* `skif-config` is rule-agnostic: it knows the shape of a rule entry but no rule's options.
* Each rule owns a `deny_unknown_fields` options struct next to its code. The registry deserializes every configured
  rule's table into it when `Linter::new` runs, and rejects an unknown rule id in `[rules]` or `[[ignore]]` with the
  closest id suggested ([ADR 0008](0008-rule-and-diagnostic-model-rule-ids-and-severities.md)).
* Options are public API: removing or renaming one is a breaking change, so an option is deprecated before it goes.

### Discovery of files

* For a root, skif walks from the root's directory upwards and collects the file of every directory, farthest first.
* The walk stops at the first of: a file with `root = true` (included); the root of the git repository holding the root,
  the nearest directory with a `.git` directory or file (included); outside a repository, the root itself in a
  single-root run and the start directory in a multi-root run. A file above the repository root is never read.
* `root = true` is valid only in a discovered file; in an extended file it is `ConfigError::Scope`.

### `extends`

* `extends` lists files, relative to the declaring file or absolute. Package-name presets are not supported.
* Targets expand depth-first in listed order, each before the file that extends it. A file reached twice (a diamond, or
  an extended file that is also an ancestor) applies once, at its first position.
* A cycle is `ConfigError::ExtendsCycle` naming the chain; the depth limit is 16 files. A missing target names the
  declaring file, line and column.

### Precedence and merge

Layers apply in this order, later beating earlier: built-in defaults; the chain of files, farthest first, each after
what it extends; the `[js.workspaces]` table that selects a package; `[[overrides]]`, in chain order; package files,
outermost first; command-line flags. A scoped statement therefore beats an unscoped one whatever file declares it, and
flags beat both. Command-line rule levels also beat the levels of overrides.

* Scalars and enums replace. Tables merge key by key. Scalar arrays replace.
* Command-line list flags replace like scalar arrays: `--ignore-package` replaces `[workspace] ignore-packages`, whose
  appending form in a file is `extend-ignore-packages`. `--ignore-dependency` adds an `[[ignore]]` entry and so
  accumulates.
* `extend-*` keys append to the inherited list, after the plain key of the same layer. They exist only for ignore-style
  lists; entry and project globs only replace.
* In `[rules]`, a level replaces the level and keeps inherited options, and an options table replaces the options and
  keeps the level. In `[js.plugins]`, `true` or `false` sets `enabled` and keeps the options, and a table replaces them.
* `[[ignore]]` and `[[overrides]]` append, in chain order.
* Version, semver and update groups match first-wins, and the nearer file's groups come first, so a project group
  shadows an organisation group and the organisation groups still catch the rest.
* `[[dependency-families]]` and `[[custom-types]]` replace the entry of the same name and append the others.
* `reset = [...]` discards the inherited `version-groups`, `semver-groups`, `ignore`, `dependency-families`,
  `custom-types`, `overrides` or `update-groups` before the file applies.

### Overrides and package files

* `[[overrides]]` selects by `packages` (names, or directories when written `./` or `../`) and/or `files`; at least one
  is required, and both must match when both are set. It changes how diagnostics in the selected scope are reported and
  which JS inputs the selected packages get, never what a workspace-wide analysis computes.
* An override allows only the package-scoped keys: `rules` (levels only), `max-level`, `ignore` and the package subset
  of `js` (`entry`, `project`, `ignore`, `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, their `extend-`
  forms and `plugins`). `js` requires `packages` and no `files`.
* Every other key is root-scoped and is `ConfigError::Scope` in an override or a package file, naming the key, its
  position and where it belongs. Among them are `[format]`, `[update]` and the update groups, `[workspace]` (including
  `discover-ignore`), and the `[js]` modes and analysis-wide keys such as `production`, `strict`, `isolate-workspaces`,
  `include-fixtures`, `use-public-hoisting`, `compilers` and `workspaces`.
* A rule that is off in the base and on in an override runs, and its findings outside the override's scope are dropped.
* A `skif.toml` in a workspace member's directory is a package file: an implicit override with `packages = ["./"]`,
  written with the top-level spelling and holding package-scoped keys only. `root`, `extends` and `[[overrides]]` are
  not allowed in it. A file in any other directory below a root is not configuration.

### Anchors

* Selection patterns are anchored at the declaring file's directory: the `packages` and `files` of an override and the
  `extends` paths. A `$ROOT/` prefix anchors a pattern at the root being linted, which presets need. A path that would
  leave its anchor never matches.
* Values are read in the scope they configure: top-level `[js]` globs are relative to the root, per-package `js` globs
  to the package. `ignore-packages`, `[[ignore]] packages`, group selectors and `[js.workspaces]` keys are relative to
  the root.
* Under `--config` in a multi-root run, the `[js.workspaces]` keys and `ignore-workspaces` globs of that file are
  relative to the start directory instead, so one explicit file configures every root of the run by its directory:
  `"."` is the root at the start directory and no other, and `"tools/sub"` the root there. The anchor is a fact of root
  discovery (`Workspace::workspace_keys_anchor`). An exact key beats a glob, and a glob never selects the root at the
  start directory. Files found by discovery keep their keys relative to each root that reads them.

### Explicit configuration and provenance

* `--config <FILE>` makes that file and what it extends the only configuration of every root and skips discovery,
  including nested roots' own files; `--no-config` reads no file. The two conflict.
* Resolution returns the configuration, the ordered list of sources and the origin of every key: each file, line,
  column and operation (set, append, prepend, reset) that touched it.
* `skif config [PATH] [-r] --show` prints the effective TOML per root with origins, the source chain and the files that
  were not applied; `--explain <key> [--package <name|dir>]` prints every layer that touched a key.
* One cache per run holds each parsed file and directory lookup, shared by the parallel root loads, so a file is read
  once however many roots share it, and sibling roots with equal configuration share one linter.

### Schema and starter file

* A JSON Schema is generated from the Rust types and committed as `schema/skif.schema.json`
  ([ADR 0002](0002-crates-publishing-and-repository-consistency-checks.md)). `skif schema` prints it.
* `skif init` writes a commented starter file with a `#:schema` directive, so TOML editors validate and complete it.

### Consequences

* Good, because one parser and one precedence rule fail a misconfiguration before any rule runs.
* Good, because `package.json` stays a manifest and the schema cannot drift from what the loader accepts.
* Good, because policy is written once and reaches every root, including roots with their own file.
* Good, because every way out is explicit (`root = true`, `reset`, `max-level`, an override), so an audit can grep for
  it.
* Good, because `skif` in a project and `skif -r` from the top agree, and `skif config` shows why.
* Bad, because a file's meaning depends on the files above it; `skif config` is the mitigation.
* Bad, because there are two path bases (selection at the declaring file, values in their scope), and the keys of an
  explicit file depend on the directory a recursive run starts at.
* Bad, because a package that needs a version policy of its own writes it in a root-scoped table; overrides cannot
  change workspace-wide analysis.
* Bad, because `load_config` alone does not catch a misspelled rule option; a host builds the `Linter` for that check.

### Confirmation

* `crates/skif-config/tests/layers.rs` covers each merge rule (`scalars_replace_and_unset_keys_are_inherited`,
  `extend_keys_append_to_the_inherited_list`, `the_nearer_file_s_groups_come_first`,
  `reset_discards_the_inherited_lists`), the ceilings
  (`ancestors_up_to_the_git_directory_contribute_and_nothing_above_it`, `a_git_file_is_a_repository_marker_too`,
  `root_true_ends_the_search_at_that_file`), `extends` (`a_cycle_is_an_error_naming_the_files`,
  `a_diamond_applies_the_shared_file_once`, `extends_is_limited_to_sixteen_files_deep`), scope errors
  (`root_scoped_keys_in_an_override_are_scope_errors_with_a_position`), anchors
  (`override_patterns_are_anchored_at_the_declaring_file`, `root_anchors_follow_the_linted_root_not_the_declaring_file`)
  and provenance (`origins_record_the_file_line_and_operation`).
* `crates/skif-config/tests/config.rs` covers `both_files_is_ambiguous`, `unknown_key_reports_path_and_line`,
  `invalid_rule_suggests_closest_id`, `cli_overrides_win_over_file_and_defaults`, `builder_matches_equivalent_toml` and
  `root_only_js_keys_are_rejected_in_an_override`.
* `crates/skif/tests/inheritance.rs` covers `a_project_has_the_same_configuration_whichever_way_it_is_reached`,
  `each_config_file_is_read_once_per_run`, `a_package_file_becomes_an_override_scoped_to_its_package` and
  `package_files_apply_after_the_overrides_of_the_chain_outermost_first`.
* `crates/skif-cli/tests/multi.rs` covers `an_explicit_config_keys_workspaces_by_their_directory_below_the_start`.
* `scripts/check-schema.sh` fails when `schema/skif.schema.json` differs from `skif schema`.

## Pros and Cons of the Options

### One TOML format with typed, rule-owned options, cascading files with a `root = true` stop, `extends`, `[[overrides]]` and package files

* Good, because inheritance is the default and each exception is explicit and local.
* Good, because each mechanism covers a case the others cannot: ancestors (organisation to project), `extends` (shared
  presets), overrides (subtrees without adding files), package files (ownership next to the code).
* Bad, because it is the largest surface; the git ceiling, TOML-only files and provenance output keep the cascade
  predictable.

### Nearest file wins, inheritance only through an explicit `extends`

* Good, because each file is self-describing, as in ruff, oxlint and Biome.
* Bad, because a project that writes its own file silently drops organisation policy unless it remembers to extend it,
  and nothing reports that.

### One root file with per-package sections only

* Good, because there is one file to read, as in knip and ESLint's flat config.
* Bad, because a repository of many projects with different owners puts every team in one file, and sections cannot
  express a nested project that must not inherit.

### A `skif` key in `package.json`, or JSON and YAML next to TOML

* Good, because it matches what syncpack and knip accept.
* Bad, because a `package.json` key makes every manifest fix a configuration change, and several formats mean several
  parsers and a precedence rule between them.
