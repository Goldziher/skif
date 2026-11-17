---
status: accepted
date: 2026-11-17
decision-makers: ["Na'aman Hirschfeld"]
---

# Configuration, Suppressions, Baselines and Migration

## Context and Problem Statement

The tools skif replaces are configured in a `package.json` key, in JSON, YAML or JavaScript files, and in TypeScript
files that are executed to be read. A `package.json` key turns every manifest fix into a configuration change,
JavaScript configuration requires executing user code ([ADR 0001](0001-mission-scope-and-non-goals.md)), and options
typed as a loose map fail at lint time.

Large repositories hold many projects, nested projects and legacy subtrees: an organisation states policy once at the
top, each project adds to it, legacy directories turn rules off, and a run inside a project and a recursive run from the
top must agree. Every suppression hides a finding, so a stale one should be reported and each should be able to say
why it exists. A team adopting skif on an existing repository needs to record today's findings and fail only on new
ones, and a team arriving from sherif, syncpack or knip needs its configuration carried over without silent loss. What
is the configuration, how do files combine, and how are suppressions, baselines and migration built on it?

## Decision Drivers

* One format, no executed configuration, and `package.json` stays a manifest
* A misconfiguration fails before any rule runs, with the file, line and key
* Policy written once reaches every project below unless a project opts out explicitly, and every effective value has a
  provenance
* A knip configuration maps onto `[js]` key by key, with deviations stated
* Baselines and migrations are ordinary configuration, so lint runs need no second mechanism

## Considered Options

* One TOML format with rule-owned typed options, cascading files, `[[ignore]]` suppressions with reasons, baselines as
  extended `[[ignore]]` files, and a one-shot migration translator
* Nearest file wins, inheritance only through `extends`, rejected because policy written at the top would stop reaching a
  project as soon as it writes its own file
* A `skif` key in `package.json`, or JSON and YAML next to TOML, rejected because it needs several parsers and a
  precedence rule between them
* A compatibility reader for `.syncpackrc`, `knip.json` and the `sherif` key at lint time, rejected because it keeps
  several formats alive and loses the untranslatable silently

## Decision Outcome

Chosen option: "One TOML format with rule-owned typed options, cascading files, `[[ignore]]` suppressions with reasons,
baselines as extended `[[ignore]]` files, and a one-shot migration translator", because one parser and one precedence
rule fail a misconfiguration at load time, policy reaches a project that writes its own file, every way out is an
explicit key, and baselines and migrations reuse the same model instead of adding one.

### Format and options

* A configuration file is `skif.toml` or `.skif.toml`; both in one directory is an error naming both. Keys are
  kebab-case, and an unknown section or key is an error with file and line. The full key set is the JSON Schema,
  generated from the Rust types, committed as `schema/skif.schema.json` and printed by `skif schema`; `skif init` writes
  a commented starter file with a `#:schema` directive.
* `[rules]` maps a rule id to a level (`off`, `info`, `warn`, `error`) or to a table with the level and the rule's
  options. `skif-config` is rule-agnostic: each rule owns a `deny_unknown_fields` options struct next to its code, and
  `Linter::new` deserializes every configured rule's table into it and rejects an unknown rule id with the closest id
  suggested. Options are public API.
* Selectors (names and globs with `!` negation and `$LOCAL`) are compiled once when the configuration is resolved. A
  builder lets a host construct a configuration without a file.

### The cascade

* For a root, skif collects the configuration file of every directory from the root upwards, farthest first. The walk
  stops at the first file with `root = true` (included) or at the root of the git repository holding the root (the
  nearest directory with a `.git` directory or file, included); outside a repository it stops at the root, or at the
  start directory of a multi-root run. A file above the repository root is never read.
* `extends` lists files, relative to the declaring file or absolute; package-name presets are not supported. Targets
  expand depth-first, each before the file that extends it, and a file reached twice applies once. A cycle is an error
  naming the chain, and the depth is limited.
* Layers apply in this order, later beating earlier: built-in defaults; the chain of files, farthest first, each after
  what it extends; the `[js.workspaces]` table that selects a package; `[[overrides]]` in chain order; package files,
  outermost first; command-line flags. A scoped statement beats an unscoped one whatever file declares it.
* Scalars replace, tables merge key by key, scalar arrays replace. `extend-*` keys append to the inherited ignore-style
  list. `[[ignore]]` and `[[overrides]]` append. Version, semver and update groups match first-wins with the nearer
  file's groups first, so a project group shadows an organisation group. `reset = [...]` discards named inherited lists.
* `[[overrides]]` selects by `packages` and/or `files` and changes only how diagnostics in its scope are reported and
  which JS inputs the selected packages get, never what a workspace-wide analysis computes. It allows only
  package-scoped keys; any root-scoped key is a scope error naming the key and where it belongs. A rule off in the base
  and on in an override runs, and its findings outside the override's scope are dropped.
* A `skif.toml` in a workspace member's directory is a package file: an implicit override for that package, holding
  package-scoped keys only. A file in any other directory below a root is not configuration.
* There are two anchors. Selection patterns (override `packages` and `files`, `extends` paths) are relative to the
  declaring file, with `$ROOT/` anchoring at the linted root; values are read in the scope they configure (top-level
  `[js]` globs relative to the root, per-package globs to the package). Under `--config` in a multi-root run, that
  file's `[js.workspaces]` keys and `ignore-workspaces` globs are relative to the start directory, so one file
  configures every root by its directory.
* `--config <FILE>` makes that file and what it extends the only configuration of every root; `--no-config` reads none.
* Resolution returns the configuration, the ordered sources and the origin of every key (file, line, column and
  operation). `skif config --show` prints the effective TOML per root with origins, and `--explain <key>` prints every
  layer that touched a key. One cache per run reads each file once however many roots share it.

### The `[js]` table

* `[js]` holds knip's keys in kebab-case, top level and per workspace in `[js.workspaces."<dir>"]` (the key `"."` is the
  root package), plus skif's own keys owned by the records of their topic.
* `enabled` defaults to `true`, so JS analysis runs wherever the `js-analysis` feature is compiled in; `--no-js` turns it
  off.
* `type-aware` turns type-aware analysis on, at the top level or per workspace and never in an override; its default
  and switches are in [ADR 0010](0010-type-aware-analysis-on-tsrs.md).
* The name lists (`ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, `ignore-members`, `ignore-workspaces`,
  `ignore-files`) take exact names, globs and regular expressions, with these stated deviations from knip: an entry is a
  regular expression when written between slashes or when it holds regex-only syntax, but `*` and `{` keep it a glob, so
  `@types/*` stays a glob; expressions are compiled by the Rust `regex` crate, so lookaround is an error.
* A trailing `!` is the production marker. In a name list an entry ending in `!` applies in production mode only; in
  `entry` and `project` it means what knip means: a production run keeps only the marked patterns, or the defaults when
  none is marked. A leading `!` is a negation.
* `production` analyses production entries and dependencies only. `isolate-workspaces` makes each workspace package use
  only the dependencies its own manifest lists, without production mode. `strict` is knip's strict mode: production,
  isolation and knip's direct-dependency rules. All three are top level only, and a flag turns a mode on, never off.
* `ignore-workspaces` takes JS analysis away from the matched directories and the packages above them, as knip does;
  the manifest and policy rules still check those packages, which separates it from `[workspace] ignore-packages`.

### The `[cache]` table

* `[cache]` holds `enabled`, `location` and `max-size` for the cache store
  ([ADR 0007](0007-js-analysis-core-on-oxc.md)). It is read only from the root configuration; in a file reached through
  `extends`, a shared file above the root or the configuration of a nested root it is ignored with a warning, because
  one store serves the whole run and a shared file would set it for every project below.
* `location` is relative to the configuration file. Flags and the environment beat the table.
* No cache setting enters any cache key, so changing one never invalidates an entry.

### Suppressions and reasons

* `[[ignore]]` entries match by `rules`, `packages`, `dependencies` (a name or `name@specifier`) and `files`; they are
  applied centrally by the emitter ([ADR 0005](0005-rules-diagnostics-and-fixes.md)).
* Every `[js]` ignore list, every `[[ignore]]` entry and every rule `allow` list accepts `{ name, reason }` in place of a
  string. Matching uses the name only. A blank reason or an unknown key is an error naming the key and index.
* `--require-ignore-reasons` or `[lint] require-ignore-reasons = true` fails the run with exit code 2 on the first
  suppression from a configuration file without a reason. Lists that select inputs rather than suppress a finding
  (`[workspace] ignore-packages`, `[js] ignore`) are not checked. A host runs the same check through `Config`.
* `stale-js-config` (a `js` rule, warning by default) reports list entries that matched nothing, `[js.workspaces]` keys
  that select no package, and knip's configuration hints. It runs after the other JS rules, and a list is not judged
  when every rule that consults it is off.
* A file several roots read is judged across them. Each report records every entry it judged, and
  `settle_shared_config` decides over the final reports of a run: an entry is stale only when every root that judged it
  found it stale, and it is reported once. The decision is a pure function, made last, after filters, the post-fix
  re-lint and suppressions; the CLI calls it, and an embedder that lints roots itself calls it too.

### Baselines

* A baseline is a file of `[[ignore]]` entries only (`skif.baseline.toml` in the workspace root, or `--file`), reached
  through the root configuration's `extends`. It has no key or lint flag of its own, and the lint run applies it with
  the matching code it already has. Any other key in it is an error, which is what lets the commands rewrite it whole.
* An entry silences what its finding is about and no more. A finding of a version-scoped rule records
  `name@specifier`, so a third version fails again; any other dependency finding records the name; a finding about a
  file other than a manifest records its package and file. Recorded values are escaped so they match literally.
* `skif baseline write` lints and adds an entry for every unmatched error or warning (an `info` finding is never
  recorded), and adds the baseline to `extends`; running it again changes nothing. `prune` removes entries that match no finding. Both take the lint flags
  that change findings and fail when the configuration does not apply the baseline. Findings that would need a
  workspace-wide entry are listed as unrecorded and still reported.
* `check` lints nothing: it fails when the baseline has an entry an earlier version lacks, read with `git show` at
  `--base <REF>` or from `--base-file`. A baseline may shrink freely and grows only through a reviewed change.
* Every command resolves roots as a lint run does, with one baseline per root, and records findings before the
  cross-root settling.

### Migration

* `skif migrate` is a one-shot translator, never a reader: skif never reads sherif, syncpack or knip files while
  linting. It reads their configuration files, `package.json` keys and the flags of the scripts that run each tool,
  writes a `skif.toml`, and deletes nothing. It replaces an existing configuration only with `--force` and edits it
  only with `--merge`.
* Extraction is static through `skif-js-syntax`, as plugin configuration is
  ([ADR 0008](0008-plugins-config-evaluation-and-commands.md)). A value that is not static is left out and reported, and
  the generated text is parsed as a `skif.toml` before it is written.
* Profiles: `exact` (the default) keeps only the checks the old tools ran and turns every other default-on rule off;
  `full` keeps skif's defaults. Which rules each tool runs is data in the crate's coverage tables, and every rule id must
  be classified there.
* Every generated `off` carries a `# skif migrate: not checked by <tools>` marker and the banner records the sources, so
  `--merge` composes steps: a marked value counts as generated and yields to a value the user wrote, and migrating tools
  one after the other checks what migrating them together checks. Merging a migration's output into itself changes
  nothing.
* Every construct gets a note classified as translated one to one, translated approximately (saying how skif differs),
  covered by skif by default, or needing a human; `--strict` fails on the last. Comments on knip ignore entries become
  their `reason`.

### Consequences

* Good, because one parser and one precedence rule fail a misconfiguration before any rule runs, and the schema cannot
  drift from the loader.
* Good, because policy is written once and reaches every root, every way out is an explicit key, and `skif` in a project
  and `skif -r` from the top agree, with `skif config` showing why.
* Good, because a knip configuration translates key by key, an unused suppression is visible, and each can state why it
  exists.
* Good, because a baseline is plain configuration with reasons, exact for versions, and `check --base` lets it shrink
  and never grow unseen.
* Good, because migration leaves the loader untouched and names every construct it could not translate.
* Bad, because a file's meaning depends on the files above it, and there are two path anchors.
* Bad, because a pattern such as `foo.*` meant as a glob is read as a regular expression.
* Bad, because a host that lints roots itself must call `settle_shared_config`, and comments inside a baseline file are
  lost on the next write; the migration tables follow three moving upstreams.

### Confirmation

* `crates/skif-config/tests/layers.rs` and `crates/skif-config/tests/config.rs` cover each merge rule, the cascade
  ceilings, `extends`, scope errors, anchors, provenance, the reason tables and the `type-aware` default and flags.
* `crates/skif/tests/inheritance.rs` and `crates/skif-cli/tests/multi.rs` cover equal configuration however a project is
  reached and the judging of shared files across roots; `crates/skif/src/shared_config.rs` covers the settling.
* `crates/skif-js/tests/js_config.rs` and `crates/skif-js/tests/isolate_workspaces.rs` cover the name lists, the
  production marker, per-workspace tables, `stale-js-config` and the modes.
* `crates/skif-cli/tests/baseline.rs` and `crates/skif-lint/src/ignore.rs` cover writing, pruning, checking and matching
  baselines.
* `crates/skif-migrate/src/tables.rs` requires every rule to be classified, and `crates/skif-migrate/tests/golden.rs`,
  `crates/skif-migrate/src/merge.rs` and `crates/skif-cli/tests/migrate.rs` cover composition and merging.
* `scripts/check-schema.sh` fails when `schema/skif.schema.json` differs from `skif schema`.
