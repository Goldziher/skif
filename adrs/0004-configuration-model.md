# 0004 — Configuration Model: One TOML File, Typed Options, Defaults < File < CLI

* Status: Accepted
* Date: 2026-11-01

## Context

The three tools skif replaces are configured three ways. sherif reads a `sherif` key in the root
`package.json` (the inherited `config_file.rs` adds a separate file on top). syncpack reads
`.syncpackrc` in JSON, YAML or JavaScript, or a `syncpack` key in `package.json`. knip reads
`knip.json`, `knip.jsonc`, `knip.ts` or a `knip` key in `package.json`, and executes the TypeScript
variant.

Every one of those formats costs something skif has decided not to pay: a `package.json` key turns
a manifest into a config file and makes every fix to `package.json` touch configuration too;
JavaScript configuration requires executing user code (ADR 0001); and supporting several formats
for the same data means several parsers and a precedence rule between them.

The configuration surface is also large and rule-specific: version groups with selectors, catalog
modes, JS entry patterns per workspace. Options typed as a loose map would push validation into
every rule and produce errors at lint time rather than at load time.

## Decision

skif reads **one TOML file**: `skif.toml` or `.skif.toml` at the workspace root. If **both exist,
loading fails** with an error naming both paths; skif never guesses which one is meant. There is no
`package.json` key, no JSON or JSONC variant, no JavaScript config, and no compatibility reader for
sherif, syncpack or knip configuration.

The file has these top-level sections:

| Section | Holds |
| --- | --- |
| `[workspace]` | `ignore-packages`, `package-manager` override |
| `[lint]` | global switches |
| `[rules]` | `id = "off" \| "warn" \| "error"`, or a table `{ level = "...", ...options }` |
| `[[ignore]]` | suppressions: `rules` × `packages` / `dependencies` |
| `[policy]` | `prefer`, `dependency-types` |
| `[[version-groups]]`, `[[semver-groups]]` | policy groups (ADR 0009) |
| `[catalog]` | `mode = "off" \| "prefer" \| "enforce"`, `min-packages`, `exclude`, `report-unused-entries` |
| `[workspace-protocol]`, `[[dependency-families]]` | policy (ADR 0009) |
| `[js]` | `enabled`, `entry`, `project`, `ignore`, `ignore-dependencies`, `ignore-binaries`, `production`, `include-entry-exports`, `[js.workspaces."<dir>"]`, `[js.plugins]` (ADR 0010, ADR 0011) |

Keys are kebab-case, matching rule IDs (ADR 0007).

**Options are typed per rule.** Each rule declares an options struct deriving `Deserialize` and
`JsonSchema` with `deny_unknown_fields`; the registry deserializes a rule's table into its struct
when configuration is loaded, so an unknown key or wrong type is a load error with the file, line
and key path, and a rule receives a typed value it does not re-validate. An unknown rule ID in
`[rules]` is also a load error.

**Precedence is defaults < file < CLI.** `skif-config` produces a `Config` from built-in defaults,
merges the file over it, then applies `ConfigOverrides` — the typed form of command-line flags such
as `--rule`, `--ignore-package` or `--no-js` — over that. Library hosts build `ConfigOverrides`
directly (ADR 0003). Each layer sets only the keys it contains; arrays replace rather than append.

`skif-config` also exposes a builder so a host can construct a `Config` without a file.

A **JSON Schema** is generated from the same Rust types (`schemars`) and committed as
`schema/skif.schema.json`; a test fails when the committed file differs from the generated one.
`skif schema` prints it and `skif init` writes a commented starter `skif.toml` with a
`#:schema` directive, so editors with Taplo or Even Better TOML validate and complete the file.

## Consequences

Positive:

* One parser, one file name pair, one precedence rule. A misconfiguration fails before any rule
  runs, with a pointer to the key.
* `package.json` stays a manifest; fixes to it never interact with configuration.
* The schema is generated, so it cannot drift from what the loader accepts.

Negative / risks:

* Users migrating from sherif, syncpack or knip rewrite their configuration by hand. A migration
  guide in the documentation is the mitigation; an automatic converter is not planned.
* TOML is awkward for deeply nested data. Version groups are arrays of tables, which TOML handles
  well; anything that would need nesting beyond that is a sign the option is badly designed.
* `deny_unknown_fields` makes removing or renaming an option a breaking change. Options are public
  API and are deprecated before removal.

## Alternatives considered

* **A `skif` key in `package.json`:** rejected — the inherited behaviour; mixes configuration with a
  file skif edits, and JSON has no comments.
* **JSONC or YAML alongside TOML:** rejected — two loaders and a precedence question for no new
  capability. The sibling tools (poly, voom, adrlint) all use TOML.
* **Hierarchical per-package `skif.toml` files:** rejected for now — per-package differences are
  expressed with `[[ignore]]`, group selectors and `[js.workspaces."<dir>"]`; a nested file would
  add a discovery walk and a merge order with no case the selectors cannot express.
* **Untyped `HashMap<String, toml::Value>` options per rule:** rejected — validation would move into
  each rule and errors would surface late and inconsistently.
