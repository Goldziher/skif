---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Thirty-Nine More knip Plugins, Chosen by Real-World Usage

## Context and Problem Statement

The plugin table holds 63 plugins ([ADR 0011](0011-plugin-model.md), [ADR 0025](0025-plugin-set-extension-thirty-five-more-tools.md), [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md)); knip 6.44.1 ships about 206. A repository that uses a tool without a plugin gets false `unused-files`, `unused-dependencies` and `unlisted-dependencies` findings for what the tool's config references: its entry files, its plugins and presets, its test files.

Which of the missing plugins are added, how are they chosen, and where does skif deliberately not follow knip's implementation?

## Decision Drivers

* A missing plugin costs the most where many repositories use the tool, so the npm downloads of the enabling package decide the order
* A plugin follows knip's enablers, config files, entry and production patterns and config-to-dependency resolution
* Configs are read statically and never executed ([ADR 0010](0010-js-analysis-on-oxc.md)); knip loads them, so some inputs are approximated
* Every plugin has a fixture test, and the plugin lists in the documentation are checked against the code

## Considered Options

* Add the 39 missing plugins with the widest use, in the declarative table plus a small static resolver where the config names files or packages
* Port all missing knip plugins
* Add a user-declared tool (`[[js.tools]]`) instead of more plugins

## Decision Outcome

Chosen option: "Add the 39 missing plugins with the widest use", because it removes the false findings of the most-used tools and keeps each plugin a row of the table with a short resolver.

The added plugins are `rsbuild`, `rslib`, `rolldown`, `unbuild`, `unocss`, `stryker`, `typedoc`, `react-router`, `remix`, `electron-vite`, `gatsby`, `svgo`, `svgr`, `swc`, `sentry`, `netlify`, `vercel`, `msw`, `webdriver-io`, `nyc`, `c8`, `ava`, `jasmine`, `i18next-parser`, `dotenv`, `size-limit`, `tsd`, `knex`, `nest`, `nitro`, `oclif`, `panda-css`, `plop`, `preconstruct`, `remark`, `sanity`, `vike`, `biome` and `lingui`. The table holds 102 plugins.

Requested tools that need no new plugin or do not exist upstream:

* `rspack`, `graphql-codegen`, `release-it`, `semantic-release`, `nx`, `prisma`, `wrangler`, `cypress`, `mocha`, `nodemon`, `pm2` and `simple-git-hooks` are already in the table.
* The test runner of Node.js is the `node` plugin ([ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md)); `syncpack` exists too.
* knip has no plugin for `lerna`, `tap`, `knip` itself or `i18next` globs (it has `i18next-parser`), so none is added; a tool without an upstream row would have no reference behaviour to follow.

Where skif differs from knip:

* `webdriver-io` reads the `config` export of `wdio.conf.*` also when it is `export const config = …` or `exports.config = …`, which the static extraction of default exports does not cover; the plugin rewrites the declaration to a default export before extracting it.
* `react-router` does not load `routes.ts`. The route modules are the string literals of that file that name a source file, relative to the app directory.
* `stryker`, `typedoc`, `panda-css`, `knex` and `gatsby` report packages that are loaded on demand (test runners, plugins, themes, presets, database drivers) as optional dependencies, which knip defers (`toDeferResolve`); they count as used and are never listed as missing.
* `rsbuild`, `rslib` and `rolldown` collect `source.entry`, `entry` and `input` from the whole config. The `rspack` section of an `rsbuild` config is read as an `rspack` config object; a function is not called.
* For `tsd`, `jasmine`, `ava`, `msw` and `oclif` the default entry patterns apply also when the config names others; knip replaces the defaults. The result is a superset, so it can hide an unused file but never reports a used one.
* `c8` and `dotenv` only provide their binaries; the script parser follows the command they wrap.

The user-declared tool (`[[js.tools]]`) is deferred: it needs a configuration model in `skif-config` that another change owns.

### Consequences

* Good, because the false findings of 39 more tools disappear without running any tool.
* Good, because every plugin is a row plus at most one short resolver, checked by one fixture test.
* Bad, because the static approximations above diverge from knip on configs that compute entries or call functions; a config that does so shows as an unused file until the tool gets a resolver for the form.
* Bad, because about 100 further knip plugins remain, among them `parcel`, `wxt`, `tauri` and `serverless-framework`.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs` lists the 102 ids in `ADR_IDS`, has a fixture test for each new plugin and a table test for the declarative ones.
* `scripts/check-plugin-list.sh` fails when the plugin lists or counts in the README files, the website and the changelog differ from the code.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Add the 39 missing plugins with the widest use

* Good, because the tools covered are the ones most repositories use.
* Bad, because the long tail stays uncovered.

### Port all missing knip plugins

* Good, because it reaches knip's coverage.
* Bad, because plugins of tools with few users cost review and tests with little effect, and several read the config by running it.

### Add a user-declared tool instead of more plugins

* Good, because a repository could cover its own tools.
* Bad, because every user would repeat what a plugin does once, and it needs a new configuration key first.

## More Information

* Extends: [ADR 0011](0011-plugin-model.md), [ADR 0025](0025-plugin-set-extension-thirty-five-more-tools.md), [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md)
* See the [ADR index](index.md).
