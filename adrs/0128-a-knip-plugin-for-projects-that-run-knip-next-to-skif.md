---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0115"]
---

# A knip Plugin for Projects That Run knip Next to skif

## Context and Problem Statement

A project that adopts skif often keeps knip for a while: during a migration, to compare the two, or because a part of
the team still runs it. Its knip configuration stays in the repository, as `knip.json`, `knip.ts` or the `knip` key of
`package.json`, and knip is a listed dev dependency that a script runs. skif's plugin table has no row for knip, since
knip has no plugin for itself, so `unused-files` reports `knip.ts` in every mode, although knip loads it. A TypeScript
config that imports a helper or a baseline also leaves that file unreached.

knip does not report its own configuration. The plugin table follows knip
([ADR 0011](0011-plugin-model.md)), but some rows have no knip counterpart: `aws-cdk`, `browser-extension`,
`azure-pipelines`, `sapper`, `taskr` and `bazel`
([ADR 0115](0115-bazel-build-files-and-output-trees-in-the-js-analysis.md)).

Should skif know knip's configuration files?

## Decision Drivers

* A file a tool of the project loads is not reported by `unused-files`
* A project that runs both tools gets no finding from skif that knip itself does not make
* The plugin table keeps one row per tool, enabled by the tool's package

## Considered Options

* A `knip` plugin whose config files are entries
* No plugin: the user adds the config to `entry` or `ignore-files`

## Decision Outcome

Chosen option: "A `knip` plugin whose config files are entries", because the file is loaded by a tool the project
lists, which is what every plugin row says.

The `knip` plugin is enabled by the `knip` dependency. Its config files are the files knip looks for, `knip.json`,
`knip.jsonc`, `.knip.json`, `.knip.jsonc`, `knip.{js,ts,mjs,cjs,mts,cts}` and `knip.config.{js,ts,mjs,cjs,mts,cts}`, and
the `knip` key of `package.json`. The JavaScript and TypeScript ones are entries, not production entries, so what they
import is used, and in production mode they are no project files, as no development entry is. The plugin reads nothing
from the config: knip's own `entry`, `project` and `ignore` lists describe knip's run, not files that are loaded, and
`skif migrate --from knip` is where they are translated
([ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md)). It provides the `knip` and
`knip-bun` binaries. The table holds 207 plugins.

### Consequences

* Good, because a project that runs knip next to skif gets no `unused-files` finding for knip's config, and the files
  the config imports are used.
* Neutral, because the row has no knip counterpart, as `bazel` and `taskr` have none.
* Bad, because a JSON config's contents are not read, so a file it names is not credited; knip loads only the config
  itself.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs`: `knip_config_files_are_entries_when_knip_is_a_dependency`,
  `knip_config_in_package_json_is_a_config`, and `ADR_IDS` lists the 207 ids.
* `scripts/check-plugin-list.sh` passes with `knip` in the plugin lists.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A `knip` plugin whose config files are entries

* Good, because the config of a listed tool is known without configuration.
* Bad, because one more row has no knip counterpart to be checked against.

### No plugin: the user adds the config to `entry` or `ignore-files`

* Good, because the plugin table stays a subset of knip's.
* Bad, because every project that runs both tools has to configure the same exception.
