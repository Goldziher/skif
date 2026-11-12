---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# `skif migrate`: A One-Shot Translator From sherif, syncpack and knip

## Context and Problem Statement

skif reads one configuration file, `skif.toml` ([ADR 0004](0004-configuration-model-and-layering.md)). Teams that
move to skif come with a sherif ignore list, syncpack version groups and a knip workspace map. Most of those
constructs have a skif equivalent, some only approximately, and some none. A hand rewrite loses the ones that do not
translate without telling anyone, and a migration that changes what is checked must say so, because the old tools
check less than skif does by default.

How does a project carry its sherif, syncpack and knip configuration over to skif, and how do migrations of several
tools, run together or one after the other, combine?

## Decision Drivers

* Moving to skif must not silently lose configuration
* skif keeps reading `skif.toml` only: one parser, one precedence rule
* Nothing in a user's repository is deleted or run
* A project that passed the old tools passes the migrated configuration in the default profile
* Migrating tools one after the other ends with the same checks as migrating them together
* A value the user wrote is never overridden by a value the migration generated
* Configuration content never produces an internal error
* The translation is testable on in-memory text, like every library crate
  ([ADR 0003](0003-library-api-and-embedding.md))

## Considered Options

* A `skif-migrate` crate and a `skif migrate` command that translate once, mark what they generate and report what
  needs a human
* A compatibility reader: skif reads `.syncpackrc`, `knip.json` and the `sherif` key at lint time
* Documentation of the translation only, with the rewrite by hand

## Decision Outcome

Chosen option: "A `skif-migrate` crate and a `skif migrate` command that translate once, mark what they generate and
report what needs a human", because it keeps the loader and the model unchanged, carries over what translates, and
turns every construct that does not into a named note instead of a silent loss.

### A translator, not a reader

* skif never reads sherif, syncpack or knip files while linting. `skif migrate` writes a `skif.toml` and deletes
  nothing.
* Sources:
  * sherif: the `sherif` key of `package.json`;
  * syncpack: `.syncpackrc` and its JSON, YAML, JavaScript and TypeScript variants, `syncpack.config.*`, the
    `syncpack` key;
  * knip: `knip.json(c)`, `.knip.json(c)`, `knip.*`, `knip.config.*`, the `knip` key;
  * the flags of the `package.json` scripts that run each tool, because they change what the tool checked.
* Extraction is static only, through `skif-js-syntax`, as plugin configuration is
  ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)). A value that is not static is not guessed: its key
  is left out and reported as needing a human, and the other keys are still translated.
* Each script command is one run of the old tool, but skif has one configuration. When runs disagree, issue types and
  dependency types are united with a note, `[js] production` is written only when every run uses production mode, and
  a script that names the tool in a command the parser does not recognise is reported.
* Every note is one of: translated one to one, translated approximately (the note says how skif differs), covered by
  skif by default, or needing a human. `--strict` makes the last group exit code 1.

### The command

* `--from auto|sherif|syncpack|knip` (comma separated; `auto` takes every tool the project has, and a project without
  any is a usage error), `--source FILE` (exactly one tool), `--profile exact|full`, `--output FILE` (`-` for
  stdout), `--merge`, `--force`, `--dry-run`, `--format`, `--strict` and `--strict-hints`.
* Without `--merge` or `--force` an existing configuration is not overwritten. `--force` replaces the configuration
  file that exists (`skif.toml` or `.skif.toml`) instead of adding a second one. `--merge` conflicts with `--force`;
  with `--output` it writes the combination there and leaves the existing file as it is.
* The generated text is parsed as a `skif.toml` in full before it is written. Content of an old configuration never
  reaches the error that means a bug in `skif-migrate`: an unknown specifier type is dropped or reported, a match-all
  filter is no filter, a custom type whose name is a built-in token is renamed with a note, and a group whose selector
  cannot be translated is not written, because a group without its selector would match every dependency.

### Profiles and coverage tables

* `exact` (the default) keeps only the checks the old tools ran: every other default-on rule is set to `off`, and the
  JS analysis is disabled when knip is not migrated. `full` keeps skif's defaults. Both write every rule knip enabled
  at knip's severity. The report lists the extra checks either way.
* Which rules a tool runs comes from data, not code: `rule-renames.txt` (sherif), `knip-types.txt` (knip) and
  `tool-rules.txt` (syncpack, with the condition under which it reports a rule, and `none` rows with a reason for the
  rules no migrated tool runs). A test requires every rule id to sit in exactly one table or `none` row, so an added
  rule must be classified.
* A rule one tool ignores is turned off only when no other migrated tool in the same run still runs it.

### Composable steps

* Each `off` the `exact` profile adds, and `[js] enabled = false`, carries the trailing comment
  `# skif migrate: not checked by <tools>`, and the banner records `# skif migrate: sources = [...]`.
* `--merge` edits the existing file in place (`toml_edit`), keeping its comments and order and appending new tables
  at the end. A marked value counts as generated: a later step's unmarked value replaces it, another marked value is
  united with it (the comment names both tools), and it is removed when the new output does not repeat it. A marked
  generated value for a key the existing file does not mention is skipped when the file records its sources. Values
  without the marker, including `off` values from a tool's own configuration, keep priority. The sources line is
  updated or appended.
* List entries are matched by name: a string and a `{ name, reason }` table with that name are one entry, which keeps
  its place, its existing reason, or takes the migrated reason when it had none; two different reasons keep the
  existing one and are reported (`kept-existing`). A table with other keys matches only identical text.
* The `!` production marker: in `entry` and `project` of `[js]` and `[js.workspaces."<dir>"]`, `x` and `x!` are one
  entry that keeps the marker when either side has it, because in a production run a plain `x` would exclude the file
  the marked form makes an entry (reported as `replaced-existing`). In the `[js]` ignore lists and their `extend-` keys
  the united entry keeps the marker only when both have it, since `x` applies in every mode. In other lists the marker
  is part of the entry. A plain pattern that only overlaps a marked one is a different entry.
* Merging the output of a migration into itself changes nothing.

### Translation choices

* knip: the issue types follow knip's own `getIncludedIssueTypes` (flags override the configuration, `--dependencies`
  covers the catalog types, production mode leaves out dev dependencies and the catalog types). Settings knip scopes
  to a workspace are written to that workspace's table.
* Comments carry over. In a knip file read statically, the comment on an entry of a knip ignore list becomes the
  entry's `reason` ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)): a comment belongs to an
  element when it sits on the lines directly above it or after it on the line where it ends, tool directives
  (`eslint-disable`, `@ts-ignore`, `biome-ignore` and the like) are not reasons, a list written as a top-level `const`
  is followed into, and entries are paired with comments by string and workspace key, so a spread does not shift
  reasons. A comment above a syncpack group becomes a TOML comment above the translated group; groups are paired by
  position, so a comment on or after a spread is not carried over rather than written above the wrong group.
* knip's `treatConfigHintsAsErrors` becomes `stale-js-config = "warn"`, because skif's configuration hints differ
  from knip's and include entries knip never flags; `--strict-hints` writes `"error"`, and `--no-config-hints` writes
  `"off"`. `unparseable-files` and `stale-js-config` are not kept for knip, which skips unparseable files silently.
* syncpack: the migration writes `[policy] override-reading = "syncpack"`, so override entries are read as syncpack
  reads them ([ADR 0010](0010-version-semver-range-and-catalog-policies.md)); the override sections are part of the
  default dependency types it writes. A `policy: catalog` group becomes `[catalog] mode = "prefer"` only when it
  selects every dependency, and only then are the catalog rules that need it kept. Group comments become TOML
  comments.
* sherif: the policy scope is `["prod", "dev"]`, and the union with syncpack's when both are migrated.
* Rules with nothing to translate are noted (`stale-mocks` is `off` under `exact`). `include-fixtures` is never
  written.

### Consequences

* Good, because a migration is one command and every untranslated construct is named with its key path.
* Good, because the loader, the model and the precedence rules are untouched by migration.
* Good, because `exact` keeps a migrated repository's results comparable with the old tools, and migrating tools in
  steps checks what one run checks.
* Good, because the set of rules a tool runs is data that a test checks against the rule list.
* Bad, because the translation tables follow three moving upstreams and need an update when a tool adds a key.
* Bad, because several constructs only approximate, and the user has to read the notes.
* Bad, because the marker comments are part of the contract between consecutive runs.
* Bad, because a rule sherif ignores and syncpack runs is kept only when both are migrated in the same run; in steps,
  the first step's `off` is a tool value and wins.
* Bad, because a merge can change the text of an existing list entry; the report names it for `entry` and `project`
  only.

### Confirmation

* `crates/skif-migrate/src/tables.rs`: `every_target_rule_exists`, `renames_agree_with_the_retired_ids` and
  `every_rule_is_covered_by_a_tool_or_declared_uncovered`.
* `crates/skif-migrate/tests/golden.rs`: `migrating_the_tools_one_after_the_other_checks_what_one_run_checks`,
  `merging_the_output_into_itself_changes_nothing`, `merging_keeps_the_comments_of_an_existing_file`,
  `merging_adds_new_tables_after_the_existing_ones`, `every_rule_a_tool_does_not_run_is_off_for_that_tool`,
  `config_hints_as_errors_are_warnings_unless_strict_hints_asks_for_error`,
  `syncpack_overrides_are_read_the_way_syncpack_reads_them`,
  `a_catalog_policy_group_does_not_widen_to_every_dependency`,
  `knip_production_leaves_out_dev_dependencies_and_the_catalog` and
  `merging_knip_keeps_its_production_entries_and_reasons_over_the_plain_existing_ones`.
* `crates/skif-migrate/src/merge.rs` unit tests: the marker in `entry`, `project` and the ignore lists in both orders,
  and reasons kept or taken.
* `crates/skif-cli/tests/migrate.rs`: `migrating_one_tool_after_the_other_checks_what_one_command_checks`,
  `force_replaces_the_dot_file_that_exists_instead_of_adding_a_second_one`,
  `a_project_without_any_tool_is_a_usage_error`, `knip_files_are_discovered_and_strict_fails_on_what_needs_a_human`
  and `merge_keeps_one_entry_per_name_with_the_production_marker_and_the_reason`.

## Pros and Cons of the Options

### A `skif-migrate` crate and a `skif migrate` command that translate once, mark what they generate and report what needs a human

* Good, because the cost is paid once per repository, not on every lint run.
* Good, because the library is pure and testable on in-memory text.
* Good, because the merge needs nothing but the two files: the old tool files of an earlier step need not exist.
* Bad, because it adds a crate and a command.

### A compatibility reader: skif reads `.syncpackrc`, `knip.json` and the `sherif` key at lint time

* Good, because nothing needs to be migrated.
* Bad, because skif would carry three more configuration formats and their semantics for ever, and a repository would
  have two sources of truth.
* Bad, because semantics that differ between the tools (the scope of an ignore, severity defaults) could not be
  reported to the user.

### Documentation of the translation only, with the rewrite by hand

* Good, because it needs no code.
* Bad, because a hand rewrite loses constructs without telling anyone.
