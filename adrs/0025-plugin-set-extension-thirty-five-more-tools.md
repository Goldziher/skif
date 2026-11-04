---
status: accepted
date: 2026-11-04
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0011", "0022"]
---

# Plugin Set Extension: Thirty-Five More Tools

## Context and Problem Statement

[ADR 0011](0011-plugin-model.md) lists 21 plugins and [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) records a table of 25. A run over eleven public repositories showed that the false positives of the JS rules concentrate where a tool has a config file or a package that skif does not know: `tsdown.config.*` files reported as unused in every package of one repository, `oxlint.config.*` and the shared lint packages they name reported as unused dependencies, `@angular/build` and `@angular-devkit/build-angular` reported as unused although `angular.json` names them as builders, and Karma, esbuild, Svelte, Nuxt, Astro, Drizzle and `ts-node` setups in the same state. Each missing plugin turns a correct project into a list of findings that the user has to suppress by hand.

## Decision Drivers

* The default severity of the dependency rules is an error ([ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md)), so a missing plugin breaks CI for a correct project
* Configs are read statically and never executed ([ADR 0010](0010-js-analysis-on-oxc.md)); a plugin that needs execution to be right reports an incomplete result instead of guessing
* Every plugin follows the model of [ADR 0011](0011-plugin-model.md): a declarative row, an optional resolver, a fixture that fails without it
* The tools were chosen by how often their files and packages appeared among the findings, not by popularity

## Considered Options

* Add the 35 tools as plugins in the existing table
* Add only the first batch of the corpus (about ten) and defer the rest
* Make the table extensible by users through `skif.toml` rows
* Keep 25 plugins and document `ignore` and `ignore-dependencies` for the rest

## Decision Outcome

Chosen option: "Add the 35 tools as plugins in the existing table", because each plugin is small, uses only the existing `PluginSpec` and `PluginInput` vocabulary, and removes a whole class of findings for every user of the tool. The table holds 60 plugins. In table order the added 35 are `tsdown`, `oxlint`, `angular`, `karma`, `esbuild`, `svelte`, `nuxt`, `astro`, `drizzle`, `ts-node`, `tsx`, `nx`, `cypress`, `mocha`, `sveltekit`, `prisma`, `graphql-codegen`, `docusaurus`, `vitepress`, `wrangler`, `expo`, `react-native`, `metro`, `release-it`, `semantic-release`, `lefthook`, `simple-git-hooks`, `stylelint`, `markdownlint`, `cspell`, `nodemon`, `pm2`, `pnpm`, `yarn` and `syncpack`.

The first 25 are unchanged. The list in [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) is the previous set, not the current one; the table in `registry.rs` is the current one, and `scripts/check-plugin-list.sh` fails when the README or the website list differs from it.

**What each plugin does.** The plugins that only declare a row are `svelte`, `ts-node`, `vitepress`, `metro` and `syncpack`. `esbuild` and `tsdown` share the entry resolver of `tsup`. The others read their config for the entries, dependencies, config files and scripts it names: `angular` reads the builders, options and Karma settings of every target in `angular.json`; `oxlint`, `stylelint`, `markdownlint`, `cspell` and `semantic-release` follow `extends`, plugin and rule lists to packages and to further config files; `nx`, `release-it`, `lefthook`, `simple-git-hooks`, `nodemon`, `prisma` and `mocha` pass the commands they hold to the script parser. `pnpm` and `yarn` are enabled by their files and credit the peer dependencies of `packageExtensions`.

**Reference and licence.** Behaviour is modelled on the plugin of the same name in knip at commit `da07b6e9887f9a1eb08545da29755b7f9d309bca`, the commit the existing rows record; the implementation is independent and nothing is copied.

**Known limits.** A config that is a function whose body only calls a method of its argument (Karma's `config.set({...})`) evaluates to an unknown value, so `karma` credits nothing from such a file; the object form and the Angular builder path are read. `docusaurus` and `nuxt` do not tell the dependency rules to ignore their virtual modules (`@generated/*`, `#imports`); a project lists them in `ignore-dependencies`. The `node` plugin of knip (`--import`, `--require` and `--test` flags) is not added here; the script parser covers the common forms.

### Consequences

* Good, because the findings of the corpus that came from missing plugins disappear without a user setting.
* Good, because the table stays declarative and the three shared helpers (`entry_inputs`, `config_ref`, `script_inputs`) keep the new plugins short.
* Bad, because every plugin is knowledge that follows its tool's releases; `knip_commit` per row records the commit it was last checked against.
* Bad, because enabling more plugins by dependency name means more projects get plugin entries, so a wrong pattern now affects more projects.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs` holds one fixture per added plugin that fails when the plugin is missing, and `ADR_IDS` lists the 60 ids in table order.
* The array in `registry.rs` has a fixed length, so adding a plugin changes the length in the same change.
* `scripts/check-plugin-list.sh` compares the list in the README and in the website configuration reference with the plugin ids in `crates/skif-js-plugins/src/plugins`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Add the 35 tools as plugins in the existing table

* Good, because it covers every tool that produced findings in the corpus and the common neighbours of those tools.
* Good, because each plugin is independent: a wrong one is fixed or switched off with `[js.plugins]` without touching the others.
* Bad, because it is a large change to review at once.

### Add only the first batch of the corpus (about ten) and defer the rest

* Good, because it is a smaller change.
* Bad, because the deferred tools produce the same kind of finding, and the table is amended again for each batch.

### Make the table extensible by users through `skif.toml` rows

* Good, because users could cover private tools.
* Bad, because a row without a resolver cannot express most of the above (aliases, builders, nested configs), and a second description format competes with the Rust table.

### Keep 25 plugins and document `ignore` and `ignore-dependencies` for the rest

* Good, because it needs no work.
* Bad, because the default run reports errors for a correct project, which is the problem this record exists to remove.

## More Information

* Amends: [ADR 0011](0011-plugin-model.md) (the set of plugins) and [ADR 0022](0022-multi-root-analysis-single-package-mode-and-the-plugin-set.md) (the table of 25).
* Related decisions: [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md)
* See the [ADR index](index.md).
