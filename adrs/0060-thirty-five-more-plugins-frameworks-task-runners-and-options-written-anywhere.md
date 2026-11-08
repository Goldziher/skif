---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0059"]
---

# Thirty-Five More Plugins: Frameworks, Task Runners and Options Written Anywhere in a Config

## Context and Problem Statement

The plugin table holds 127 plugins ([ADR 0059](0059-twenty-one-more-plugins-and-commands-run-from-source.md)). Issue #40 lists the next tier of knip plugins: frameworks and app platforms (`tauri`, `capacitor`, `convex`, `sst`, `payload`, `qwik`, `quasar`, `wxt`, `stencil`, `eleventy`), test and code generation tools (`cucumber`, `rstest`, `playwright-ct`, `relay`, `orval`, `openapi-ts`, `buf`), linters and release tools (`xo`, `textlint`, `danger`, `changelogen`) and task runners and CI (`taskfile`, `mise`, `moonrepo`, `wireit`, `travis`, `nano-staged`), plus smaller ones.

Several of these read options that are not part of what the config exports: SST handlers sit inside `run()` and in the modules it imports, Stencil exports `config` by name, Quasar wraps its config in `configure(…)`, and orval nests `path` deep inside each target. knip reads them with `collectPropertyValues`, which visits every object literal of the file.

Which plugins are added, how are options written anywhere in a file read without running it, and where does skif deliberately not follow knip?

## Decision Drivers

* A plugin follows knip 6.44.1's enablers, config files, entry and production patterns and config-to-dependency resolution ([ADR 0036](0036-thirty-nine-more-knip-plugins-by-real-world-usage.md))
* Configs and sources are read statically and never executed ([ADR 0010](0010-js-analysis-on-oxc.md))
* A mechanism shared by several plugins is better than a parser per plugin ([ADR 0059](0059-twenty-one-more-plugins-and-commands-run-from-source.md))
* Every plugin has a fixture test, and the plugin lists in the documentation are checked against the code

## Considered Options

* Add the plugins, with one shared extraction of the values of a property wherever it is written
* Add only the plugins whose options are part of the exported config
* Evaluate the config's function bodies further so that the exported value carries the options

## Decision Outcome

Chosen option: "Add the plugins, with one shared extraction of the values of a property wherever it is written", because it mirrors the knip helper those plugins use and needs no change to the static evaluator.

The added plugins are `tauri`, `capacitor`, `convex`, `payload`, `wxt`, `hardhat`, `temporal`, `react-email`, `openapi-ts`, `danger`, `changelogen`, `purgecss`, `fumadocs`, `nuxtjs-i18n`, `playwright-ct`, `sst`, `qwik`, `quasar`, `stencil`, `eleventy`, `cucumber`, `relay`, `orval`, `vite-plugin-pwa`, `xo`, `textlint`, `rstest`, `sveltejs-package`, `taskfile`, `mise`, `moonrepo`, `wireit`, `travis`, `buf` and `nano-staged`. The table holds 162 plugins.

`skif-js-syntax` gains `extract_property_values`: the statically evaluated value of every property with a given key in every object literal of a file, in source order, as knip's `collectPropertyValues`. `sst`, `quasar`, `stencil`, `orval` and `vite-plugin-pwa` (for `nuxt.config.*`) use it; `payload`, `qwik` and `vite-plugin-pwa` (for `vite.config.*`) use `extract_imported_calls` of [ADR 0059](0059-twenty-one-more-plugins-and-commands-run-from-source.md).

Two driver changes support them:

* A plugin that reads the source rather than the exported value (`reads_config` is false) is also called for a config whose export is not understood, with a null value, so Stencil's `export const config` is read.
* A config path a script passes (`--config`, `buf generate --template`) is a file whenever that file exists, not only when its extension is a source one; only a relative path without `..` is probed, so nothing outside the package is read; `buf` adds `--template` to its config flags.

Not added:

* `starlight` was added by [ADR 0059](0059-twenty-one-more-plugins-and-commands-run-from-source.md).
* `unplugin-vue-i18n` has an enabler and nothing else in knip: no config, entry or resolver, so a row would change nothing.

Where skif differs from knip:

* `sst` also reads the handlers of the modules `sst.config.ts` imports by relative path, statically or with `await import("./infra/…")`, transitively, resolving a `.js`, `.mjs` or `.cjs` specifier to its `.ts`/`.tsx`, `.mts` or `.cts` source as TypeScript does; knip reads the config and the stacks of SST 2's `.stack(…)`, which skif does not read. A handler is a module and an export (`src/api.handler`): the export is cut off for `.route(path, "src/api.handler")` too, where knip resolves the whole string.
* `qwik` reads `srcDir` and `routesDir` from `qwikVite` and `qwikCity` imported from their packages, and does not compile `.mdx` routes; `stencil` does not mark custom elements and component style URLs as used and does not compile Sass.
* `quasar` reads the first `sourceFiles` object and the first `boot` and `css` arrays written anywhere in the config, where knip reads the top-level object of the export.
* `eleventy` reads `dir` and `templateFormats` from the value the config returns; passthrough copies, which knip collects by calling the config function with a stub, are not read.
* `rstest` does not apply `exclude` to `include` (more test files are entries, not fewer), does not read `rs.mock(import(…))` calls, and credits the coverage provider of every config it reads, optionally.
* `react-email` credits both preview server packages, `@react-email/ui` and `@react-email/preview-server`, optionally, as the installed major version is not known; knip picks one by the declared version.
* `payload` keeps migrations and the generated types from being reported through `ignore`; knip removes them from the project files.
* `taskfile` skips commands and includes holding a `{{…}}` template. `mise` runs a task in its `dir` (`{{config_root}}` expanded) and skips templated commands, as knip does, but does not track whether `node_modules/.bin` is on the task's `PATH`, and does not read file tasks.
* `moonrepo` reads `$workspaceRoot` and `$projectRoot` as the workspace, as the plugin is enabled at the root only.
* `textlint` prefers a candidate package declared by the workspace or an ancestor, and names a scoped rule `@scope/textlint-rule-<name>`.

### Consequences

* Good, because the false findings of 35 more tools disappear without running any tool, among them the `infra/*` modules and handlers of SST apps.
* Good, because the property extraction serves every plugin whose options sit inside calls or function bodies.
* Neutral, because a property found anywhere may come from an unrelated object with the same key; the plugins that use it read keys specific to their tool, as knip does.
* Bad, because options computed at run time (a handler built from a variable, an Eleventy config that only calls `eleventyConfig.*` methods) are not seen.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs` lists the 162 ids in `ADR_IDS` and has a fixture test for each new plugin.
* `crates/skif-js-syntax/tests/property_values.rs` covers nested properties, arrays, quoted keys, methods and unparsable files.
* `scripts/check-plugin-list.sh` fails when the plugin lists or counts in the README files and the website differ from the code.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Add the plugins, with one shared extraction of the values of a property wherever it is written

* Good, because it covers every listed tool but one that has nothing to port.
* Bad, because a key is matched wherever it appears, not where the tool reads it.

### Add only the plugins whose options are part of the exported config

* Good, because it needs no change outside the plugin crate.
* Bad, because `sst`, `stencil`, `quasar` and `orval` would be rows that find nothing.

### Evaluate the config's function bodies further so that the exported value carries the options

* Good, because each option would be read where the tool reads it.
* Bad, because SST's `run()` creates resources with side effects rather than returning them, and Stencil exports a named binding; no evaluation of the export reaches them.
