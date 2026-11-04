---
status: accepted
date: 2026-11-04
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0004", "0016", "0022", "0023"]
---

# Layered Configuration: Cascading Files, Extends, Overrides and Package Files

## Context and Problem Statement

[ADR 0004](0004-configuration-model.md) defines one `skif.toml` per workspace root and rejects hierarchical files for now. [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) lets one run lint many roots, each taking the nearest file, and [ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md) moved the ceiling of that search to the git repository root. The nearest file still replaces everything above it.

Large repositories do not fit that. One repository holds dozens of independent projects: pnpm workspaces, single packages, projects nested in projects and legacy subtrees. An organisation wants to state policy once at the repository root: banned dependencies, one React version, `fail-on-warnings`, the binaries every project may call. Each project wants to add to that without copying the file, legacy directories need rules turned off or capped, and JS analysis needs entries and ignores per package. Today the options are copying the org file into every project or putting everything into one file whose selectors spell out paths relative to each root. A run inside a project and a recursive run from the top could also read different files.

## Decision Drivers

* Policy set once at the repository root reaches every root below it unless a root opts out explicitly
* A project, a package or a directory can add to, lower or turn off what it inherits
* `skif` in a project and `skif -r` from the top give that project the same configuration
* Every effective value is traceable to the file and line that set it
* Workspace-wide analyses (version groups, catalogs) stay workspace-wide; only reporting and per-package JS inputs vary per package
* Loading stays a few file reads per root, shared across parallel roots, through `FileSystem` ([ADR 0003](0003-library-first-embedding.md))
* Every existing single-file configuration keeps parsing and keeps its meaning

## Considered Options

* Cascading files with a `root = true` stop, plus `extends`, `[[overrides]]` and package files
* Nearest file wins; inheritance only through an explicit `extends` (ruff, oxlint, Biome)
* One root file with per-package sections only (knip, ESLint flat config, syncpack)
* Keep ADR 0022 and ADR 0023 as they are

## Decision Outcome

Chosen option: "Cascading files with a `root = true` stop, plus `extends`, `[[overrides]]` and package files", because it is the only option where policy reaches a project that writes its own file by default, while every way out is an explicit, greppable key (`root = true`, `reset`, an override). One model, an ordered list of layers, covers ancestors, presets, overrides, package files and flags.

**Discovery and ceilings.** A configuration file is `skif.toml` or `.skif.toml`; both in one directory is `ConfigError::Ambiguous`, as before. For a root, skif walks from the root's directory upwards and collects the file of every directory, farthest first. The walk stops after the first of these:

* a file with `root = true` (the file is included);
* the root of the git repository holding the root, the nearest directory with a `.git` entry, a directory or the file of a linked worktree (its file is included);
* outside a repository, the root itself in a single-workspace run and the start directory in a multi-root run, which is the behaviour of ADR 0023 for that case.

A file above the repository root is never read. `root = true` is valid only in a file found by discovery; in an extended file it is `ConfigError::Scope`.

**Extends.** `extends = ["../presets/node-cli.toml"]` lists files, relative to the declaring file or absolute. Targets expand depth-first in listed order and each contributes before the file that extends it. A file reached twice (a diamond, or an extended file that is also an ancestor) is applied once at its first position. A cycle is `ConfigError::ExtendsCycle` with the chain, detected on lexically normalised paths because `FileSystem` has no canonicalize, so a cycle through symlinks is only caught by the depth limit of 16 files. A missing target is an error naming the declaring file, line and column. Package-name presets resolved through `node_modules` are out of scope.

**Layer order.** Built-in defaults, then the base bodies of the chain (each preceded by its extends), farthest first, then `[js.workspaces]`, then package files, outermost directory first, then command-line flags. Overrides are collected in the same chain order and apply to the diagnostics and JS inputs they select on top of the base. A scoped statement therefore beats an unscoped one whatever file declares it, and flags beat both.

**Merge rules.**

| Kind | Keys | Rule |
| --- | --- | --- |
| Scalars, enums | `lint.fail-on-warnings`, `workspace.package-manager`, `policy.prefer`, `catalog.mode`, `js.enabled`, `js.production`, ... | the later layer replaces |
| Tables | `[workspace]`, `[lint]`, `[policy]`, `[catalog]`, `[workspace-protocol]`, `[js]` | merge key by key |
| Scalar arrays | `workspace.ignore-packages`, `js.entry`, `js.project`, `js.ignore`, `js.ignore-dependencies`, `js.ignore-binaries`, `catalog.exclude`, `policy.dependency-types` | replace, as in ADR 0004 |
| `extend-` keys | `workspace.extend-ignore-packages`, `js.extend-ignore`, `js.extend-ignore-dependencies`, `js.extend-ignore-binaries`, `catalog.extend-exclude` | append to the inherited list, after the plain key of the same layer |
| `[rules]` | per rule id | a level (a bare string, `level` or `severity`) replaces the level and keeps inherited options; a non-empty options table replaces the options and keeps the level |
| `[js.plugins]` | per plugin | `true` or `false` sets `enabled` and keeps the options; a table replaces the options and sets `enabled` only when it gives it |
| `[[ignore]]`, `[[overrides]]` | arrays of tables | append |
| `[[version-groups]]`, `[[semver-groups]]` | first match wins | the nearer file's entries are put first, so a project group shadows an org group for the same dependency and the org groups still catch the rest |
| `[[dependency-families]]` | named tables | replace the family of the same `name`, append the others |
| `reset` | `version-groups`, `semver-groups`, `ignore`, `dependency-families`, `overrides` | discards what was inherited for the named list before the file applies |

The `extend-` keys exist only for ignore-style lists, where accumulating is the common need. Entry and project globs are project-specific and only replace.

**Overrides.** `[[overrides]]` selects by `packages` (names, or directories) and/or `files`; at least one is required and when both are set both must match. An override changes how diagnostics located in the selected packages or files are reported and which JS inputs those packages get. It never changes what a workspace-wide analysis computes: a package that must leave version policy is excluded by a root-level version group with `policy = "ignored"`. A selector that is set but has nothing to match (a diagnostic without a package or a file) does not match.

The package-scoped keys are `rules` (levels only), `max-level`, `ignore` and `js.{entry, project, ignore, ignore-dependencies, ignore-binaries, extend-ignore, extend-ignore-dependencies, extend-ignore-binaries, plugins}`. Everything else is root-scoped and is `ConfigError::Scope` naming the key, the file, line and column, and where the key belongs: `workspace`, `lint`, `policy`, the group tables, `catalog`, `workspace-protocol`, `dependency-families`, `root`, `extends`, `reset`, nested `overrides`, `js.enabled`, `js.production`, `js.include-entry-exports`, `js.workspaces` and rule options. `[lint] max-level` is also allowed in a base body, where it caps a whole root. `js` in an override needs `packages` and cannot be combined with `files`. A rule that is off in the base and on in an override runs; the enabled set is "on in any layer" and its diagnostics outside the override's scope are dropped.

**Package files.** A `skif.toml` in the directory of a workspace member is treated as an `[[overrides]]` block with `packages = ["./"]`, anchored at that directory. It uses the top-level spelling (`[rules]`, `[js]`, `[[ignore]]`, `[lint] max-level`), may hold only package-scoped keys, and its `[[ignore]]` entries may omit every filter because the package is the filter. `root`, `extends` and `[[overrides]]` are not allowed in it. Files in other directories below a root are not configuration; `skif config --show` lists them as not applied.

**Relative paths.** Selection patterns are anchored at the directory of the file that declares them: `packages` and `files` of an override, and the `extends` paths. A package pattern starting with `./` or `../` is a directory; any other is a package name or a `$LOCAL`-style token, and a file pattern is always a path. A `$ROOT/` prefix anchors a pattern at the root being linted instead, which presets need. A path that would leave its anchor never matches. Values are read in the scope they configure: top-level `[js]` globs are relative to the root and per-package `js` globs relative to the package, as before. Existing selectors (`ignore-packages`, `[[ignore]] packages`, group selectors and `[js.workspaces]` keys) stay relative to the root in this version.

**Provenance.** Resolution returns the configuration, the ordered list of sources (path and kind: root, ancestor, extends, explicit) and an origin for every key: each file, line, column and operation (set, append, prepend, reset) that touched it. Keys are dotted paths such as `lint.fail-on-warnings`, `rules.unused-files.level`, `rules.<id>.options` and list entries `version-groups[0]`. Errors carry the file of the declaring layer.

**CLI.** `skif config [PATH] [-r]` prints the result. `--show` prints the effective TOML per root with `# from <file>:<line>` comments, the source chain and the files that were not applied; `--explain <key> [--package <name|dir>]` prints every layer that touched the key and the final value, overrides included; `--format json` prints the same as data. `--config <FILE>` makes that file (and what it extends) the only configuration of every root and skips discovery. `--no-config` reads no file: defaults plus flags. The two conflict. Flags stay the top layer, and `--ignore-package` keeps replacing `ignore-packages` (ADR 0004, ADR 0016). The napi addon mirrors both as `config` and `noConfig`.

**Performance.** One cache per run holds each parsed file and each directory lookup, and is shared by the parallel root loads, so a file is read and parsed once however many roots share it. Anchors are stored as absolute paths, so sibling roots with the same chain compare equal and share one linter.

**Compatibility.** Every existing file parses unchanged and means the same when it is the only file of its chain. skif is unreleased, so two behaviour changes go into 0.1.0: a single-workspace run reads ancestor files up to the git root, and in a multi-root run nearer files merge with their ancestors instead of replacing them. A file that wants the old "this file only" behaviour adds `root = true`. This replaces "the nearest file replaces the rest" of ADR 0022, removes the ADR 0022 consequence that a run inside a project and a recursive run can read different files, and amends the ceiling of ADR 0023: the ceiling stays the git root, but the files between root and ceiling now merge.

### Consequences

* Good, because policy is written once and reaches every root, including those with their own file.
* Good, because every way out is an explicit key, so an audit can grep for `root = true`, `reset` and `max-level`.
* Good, because `skif` in a project and `skif -r` at the top agree.
* Good, because `--show` and `--explain` make the layers inspectable.
* Bad, because a file's meaning depends on files above it; `skif config` is the mitigation.
* Bad, because two path bases exist (selection anchored at the declaring file, values relative to their scope) and the older selectors are still root-relative.
* Bad, because `extend-` keys and `reset` add public schema surface under `deny_unknown_fields`.
* Bad, because a package that wants a version policy of its own must still write it in a root-scoped table; overrides and package files cannot change workspace-wide analysis.

### Confirmation

* `skif-config` tests cover each merge rule, group order and `reset`, `extend-` keys, extends cycles, diamonds and missing targets, scope errors in overrides, `root = true` and the `.git` ceiling as a directory and as a file, and the origins of every key kind.
* `skif` tests cover the effective configuration per root of a multi-project tree, sibling roots sharing one configuration, and one read per file per run.
* `skif-lint` and `skif-js` tests cover scoped levels, `max-level`, scoped ignores and per-package JS inputs.
* CLI tests cover `skif config --show` and `--explain`, `--config` and `--no-config`.
* `skif schema` equals `schema/skif.schema.json` and lists `root`, `extends`, `reset`, `overrides`, `lint.max-level` and the `extend-` keys.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Cascading files with a `root = true` stop, plus `extends`, `[[overrides]]` and package files

* Good, because inheritance is the default and each exception is explicit and local.
* Good, because each mechanism covers a case the others cannot: ancestors (org to project), `extends` (shared presets, skipping a parent), overrides (subtrees without adding files), package files (ownership next to the code).
* Bad, because it is the largest surface. ESLint's lookup went the other way, from a cwd-based search to one starting at each file's directory that uses the first file found and does not merge parents. The failure modes of implicit cascading in tools that evaluate JavaScript or resolve plugins relative to the config file are avoided here by the git ceiling, TOML-only files and provenance output, not by removing the cascade.

### Nearest file wins; inheritance only through an explicit `extends`

* Good, because each file is self-describing. Ruff uses the closest configuration file and ignores parent files unless it `extend`s them; oxlint uses the nearest file and does not merge configs unless `extends` is given; Biome v2 nested configs do not inherit unless they set `"extends": "//"`.
* Good, because the result for a file depends on one file and its explicit references.
* Bad, because a project that writes its own `skif.toml` (for example with `skif init`) silently drops org policy unless it remembers to extend, and nothing reports it. That is the main driver of this record. `skif init` writing an `extends` line would reduce the risk but not remove it.

Honest comparison: this option has the better local reasoning and the smaller surprise when a file is added; the chosen one has the better default for org policy and pays for it with a dependency on parent files. The mitigation for the chosen option is tooling (`skif config`), for this option it is discipline.

### One root file with per-package sections only

* Good, because there is one file to read. Knip takes a single root configuration with a `workspaces` map keyed by directory patterns, and ESLint flat config is one array of objects with `files` and `ignores`.
* Bad, because a repository of dozens of projects with different owners puts every team in one file, and sections cannot express a nested project that must not inherit.

### Keep ADR 0022 and ADR 0023 as they are

* Good, because it needs no work.
* Bad, because policy must be copied into every project and the start directory changes the result.

## More Information

* Amends: [ADR 0004](0004-configuration-model.md) (hierarchical files are no longer rejected and precedence gains the extends, override and package-file layers), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md) (the `config` subcommand, `--config`, `--no-config`), [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) (nearest file wins), [ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md) (the configuration ceiling now bounds a merged chain)
* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md)
* Prior art, as documented when this record was written:
  * Biome, [Big projects](https://biomejs.dev/guides/big-projects/): nested configurations set `"root": false` and inherit only through `"extends": "//"`.
  * ruff, [Configuration](https://docs.astral.sh/ruff/configuration/): the closest configuration file is used, parents are ignored, `extend` inherits explicitly.
  * oxlint, [Nested config](https://oxc.rs/docs/guide/usage/linter/nested-config.html): the nearest file is used, configs are not merged automatically, `extends` is explicit and `--config` disables nested lookup.
  * ESLint, [Configuration files](https://eslint.org/docs/latest/use/configure/configuration-files): the lookup starts in the directory of the linted file and goes up to the first `eslint.config.*`; merging happens between objects of one file.
  * knip, [Monorepos and workspaces](https://knip.dev/features/monorepos-and-workspaces): one root configuration with a `workspaces` map.
* See the [ADR index](index.md).
