---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Configuration Model: One TOML File, Typed Options, Defaults < File < CLI

## Context and Problem Statement

The three tools skif replaces are configured three ways. sherif reads a `sherif` key in the root
`package.json` (the inherited `config_file.rs` adds a separate file on top). syncpack reads
`.syncpackrc` in JSON, YAML or JavaScript, or a `syncpack` key in `package.json`. knip reads
`knip.json`, `knip.jsonc`, `knip.ts` or a `knip` key in `package.json`, and executes the TypeScript
variant.

Every one of those formats costs something skif has decided not to pay: a `package.json` key turns
a manifest into a config file and makes every fix to `package.json` touch configuration too;
JavaScript configuration requires executing user code ([ADR 0001](0001-mission-and-scope.md)); and supporting several formats
for the same data means several parsers and a precedence rule between them.

The configuration surface is also large and rule-specific: version groups with selectors, catalog
modes, JS entry patterns per workspace. Options typed as a loose map would push validation into
every rule and produce errors at lint time rather than at load time.

## Decision Drivers

* Executing JavaScript configuration is ruled out
* A `package.json` key would turn every manifest fix into a configuration change
* Several formats for the same data mean several parsers and a precedence rule
* Rule-specific options should be validated at load time, not lint time

## Considered Options

* One TOML file with typed per-rule options and defaults < file < CLI precedence
* A `skif` key in `package.json`
* JSONC or YAML alongside TOML
* Hierarchical per-package `skif.toml` files
* Untyped rule options that each rule parses itself at lint time

## Decision Outcome

Chosen option: "One TOML file with typed per-rule options and defaults < file < CLI precedence", because one parser and one precedence rule fail a misconfiguration at load time with a pointer to the key, keep `package.json` a manifest, and never require executing user code.

skif reads **one TOML file**: `skif.toml` or `.skif.toml` at the workspace root. If **both exist,
loading fails** with an error naming both paths; skif never guesses which one is meant. There is no
`package.json` key, no JSON or JSONC variant, no JavaScript config, and no compatibility reader for
sherif, syncpack or knip configuration.

The file has these top-level sections:

| Section | Holds |
| --- | --- |
| `[workspace]` | `ignore-packages`, `package-manager` override |
| `[lint]` | global switches |
| `[rules]` | `id = "off" \| "info" \| "warn" \| "error"`, or a table `{ level = "...", ...options }` |
| `[[ignore]]` | suppressions: `rules` × `packages` / `dependencies` |
| `[policy]` | `prefer`, `dependency-types` |
| `[[version-groups]]`, `[[semver-groups]]` | policy groups ([ADR 0009](0009-version-and-catalog-policies.md)) |
| `[catalog]` | `mode = "off" \| "prefer" \| "enforce"`, `min-packages`, `exclude`, `report-unused-entries` |
| `[workspace-protocol]`, `[[dependency-families]]` | policy ([ADR 0009](0009-version-and-catalog-policies.md)) |
| `[js]` | `enabled` (default `true`, effective only when the `js-analysis` feature is compiled in), `entry`, `project`, `ignore`, `ignore-dependencies`, `ignore-binaries`, `production`, `include-entry-exports`, `[js.workspaces."<dir>"]`, `[js.plugins]` ([ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md)) |

Keys are kebab-case, matching rule IDs ([ADR 0007](0007-rule-and-diagnostic-model.md)).

**Config is rule-agnostic; rules own their options.** `skif-config` knows the shape of a rule entry
(a level and, for a table, the remaining keys held as a skif-owned value) but not any rule's
options. Each rule defines its own options struct, deriving `Deserialize` and `JsonSchema` with
`deny_unknown_fields`, next to the rule that reads it (in `skif-rules` or `skif-js`), and the
`Registry` ([ADR 0007](0007-rule-and-diagnostic-model.md)) deserializes every configured rule's table into that struct when the
`Linter` is built. An unknown key or wrong type is an error with the file, line and key path, a
rule receives a typed value it does not re-validate, and adding a rule never touches `skif-config`.
Because only the registry knows which rule IDs exist, it also rejects an unknown rule ID in
`[rules]` or `[[ignore]]` (with a closest-match suggestion), and it does so before any rule runs.
`RuleId` validates its kebab-case shape when deserialized, so a malformed ID fails at parse time.
The configured level maps one to one onto `Severity`: `off` disables the rule, `info`, `warn` and
`error` set the severity ([ADR 0007](0007-rule-and-diagnostic-model.md)).

**Selectors are compiled once.** Every key that names packages or dependencies
(`ignore-packages`, `[[ignore]]`, version and semver groups, `[js]` ignore lists) is a list of
names and globs with `!` negation and the `$LOCAL` marker ([ADR 0009](0009-version-and-catalog-policies.md)). `skif-config` compiles each
list into a `Selector` (a `GlobSet` plus its negations) when the configuration is resolved, and an
invalid pattern is a configuration error naming the key. Rules and discovery only ever call
`Selector::matches`; no glob is compiled per package or per rule. Dependency kinds and specifier
kinds in configuration are enums (`DependencyKind`, defined once and shared with discovery), never
strings, and `package-manager` is the one `PackageManager` enum that discovery and fixes also use.

**Loading goes through the `FileSystem`** ([ADR 0003](0003-library-first-embedding.md)). `load_config(root, fs, overrides)` looks
for `skif.toml` and `.skif.toml` in the workspace root through `fs`, never through `std::fs`, so a
host with unsaved buffers configures skif the same way the CLI does. The **workspace root** is not
assumed to be the current directory: `skif-workspace` finds the nearest ancestor of the start path
that declares a workspace (`workspaces` in `package.json`, or `pnpm-workspace.yaml`), and the
configuration file is looked up in that directory only. Parse errors carry the file, line and
column computed with the same `LineIndex` that spans use.

**Precedence is defaults < file < CLI.** `skif-config` produces a `Config` from built-in defaults,
merges the file over it, then applies `ConfigOverrides` — the typed form of command-line flags such
as `--rule`, `--ignore-package` or `--no-js` ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)) — over that. Library hosts build `ConfigOverrides`
directly ([ADR 0003](0003-library-first-embedding.md)). Each layer sets only the keys it contains; arrays replace rather than append.

`skif-config` also exposes a builder so a host can construct a `Config` without a file.

`[js] enabled` defaults to `true`: JS analysis runs whenever the `js-analysis` feature is compiled
in, so `skif` in CI checks everything. A feature that is not compiled in simply has nothing to
enable, which is why the default is harmless in 0.1.0 and in hosts that leave the feature off. The
speed-sensitive caller is the git hook, which passes `--no-js` explicitly rather than relying on
the default ([ADR 0014](0014-git-hook-integration.md)); `--no-js` overrides the file.

A **JSON Schema** is generated from the same Rust types (`schemars`) and committed as
`schema/skif.schema.json`; a test fails when the committed file differs from the generated one.
`skif schema` prints it and `skif init` writes a commented starter `skif.toml` with a
`#:schema` directive, so editors with Taplo or Even Better TOML validate and complete the file.

### Consequences

* Good, because one parser, one file name pair, one precedence rule. A misconfiguration fails before any rule
  runs, with a pointer to the key.
* Good, because `package.json` stays a manifest; fixes to it never interact with configuration.
* Good, because the schema is generated, so it cannot drift from what the loader accepts.
* Bad, because users migrating from sherif, syncpack or knip rewrite their configuration by hand. A migration
  guide in the documentation is the mitigation; an automatic converter is not planned.
* Bad, because TOML is awkward for deeply nested data. Version groups are arrays of tables, which TOML handles
  well; anything that would need nesting beyond that is a sign the option is badly designed.
* Bad, because option validation moved from the loader to the registry, so `load_config` alone does
  not catch a misspelled option; a host must build the `Linter` (which the CLI always does before running a rule) to get that check.
* Bad, because `deny_unknown_fields` makes removing or renaming an option a breaking change. Options are public
  API and are deprecated before removal.

### Confirmation

* `cargo test -p skif-config` includes a schema freshness test that fails when the committed `schema/skif.schema.json` differs from the schema generated from the Rust types.
* `cargo test -p skif-lint` includes registry tests: an unknown option key, a wrong option type and an unknown rule ID in `[rules]` or `[[ignore]]` each fail `Linter::new` with the file, line and key path, and a rule ID with uppercase letters fails at parse time.
* `cargo test -p skif-config` includes tests that both `skif.toml` and `.skif.toml` present is an error naming both paths, that loading works against `MemoryFs` with no file on disk, and that an invalid selector pattern is a configuration error naming the key.
* A `skif-config` benchmark (`mise run bench`) matches 500 packages against five selectors; the compiled `Selector` is held to a budget in the tens of microseconds, against the roughly 20 ms that compiling per call cost.
* A CLI test for `--no-js` checks that it overrides `[js] enabled = true`, and a `.pre-commit-hooks.yaml` test asserts the default hook command line contains `--no-js` ([ADR 0014](0014-git-hook-integration.md)).

## Pros and Cons of the Options

### One TOML file with typed per-rule options and defaults < file < CLI precedence

* Good, because one parser, one file name pair and one precedence rule; a misconfiguration fails before any rule runs, with a pointer to the key.
* Good, because rules own their option types, so a new rule needs no change in `skif-config`, and the schema is generated from the same types and cannot drift.
* Bad, because migrating users rewrite configuration by hand, TOML is awkward for deep nesting, and `deny_unknown_fields` makes renaming an option a breaking change.

### A `skif` key in `package.json`

* Good, because it needs no extra file and is how sherif and syncpack were configured.
* Bad, because it mixes configuration with a file skif edits, so every manifest fix may touch configuration, and JSON has no comments.

### JSONC or YAML alongside TOML

* Good, because users could keep the format they already use for syncpack or knip.
* Bad, because it needs two loaders and a precedence question for no new capability. The sibling tools (poly, voom, adrlint) all use TOML.

### Hierarchical per-package `skif.toml` files

* Good, because a package could carry its own exceptions next to its code.
* Bad, because per-package differences are expressed with `[[ignore]]`, group selectors and `[js.workspaces."<dir>"]`; a nested file would add a discovery walk and a merge order with no case the selectors cannot express. (rejected for now)

### Untyped rule options that each rule parses itself at lint time

* Good, because neither the config crate nor the registry needs to know anything about options.
* Bad, because validation would move into each rule and errors would surface at lint time, late and inconsistently; the chosen design keeps the raw table in config but has the registry validate it into the rule's own type before any rule runs.

## More Information

* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0003](0003-library-first-embedding.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md), [ADR 0014](0014-git-hook-integration.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)
* See the [ADR index](index.md).
