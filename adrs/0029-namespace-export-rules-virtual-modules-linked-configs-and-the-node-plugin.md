---
status: accepted
date: 2026-11-04
decision-makers: ["Na'aman Hirschfeld"]
---

# Namespace Export Rules, Virtual Modules, Linked Configs and the node Plugin

## Context and Problem Statement

[ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md) withdrew `unused-namespace-exports` and `unused-namespace-types` because reporting them needed the member usage of a namespace per consumer, which the module graph did not track. It does now: every `import * as ns` records the members read through `ns` and whether `ns` is used as a whole, and `export * as ns` and `import * as ns; export { ns }` are re-export edges that forward only the members read through them. Two knip issue types, `nsExports` and `nsTypes`, therefore have a precise counterpart again.

Separately, the plugin vocabulary of [ADR 0011](0011-plugin-model.md) cannot say that a specifier is resolved by the tool at build time (Docusaurus `@generated/*`, Nuxt `#imports`) or that a config file of one tool belongs to another (an Angular target's `tsConfig`, the Babel config of an Expo project), and the Node.js runtime has no plugin although its test runner finds files by convention.

## Decision Drivers

* The rule list in the records must equal the rule list in the registry
* A report must not duplicate another rule's report
* Whole-namespace uses must never produce findings
* A tool's virtual modules are not dependencies and must not be reported as unlisted or unresolved
* A config file must be read by the plugin that understands it

## Considered Options

* Restore both rules, move the findings of namespace-only modules to them, extend the plugin vocabulary and add a `node` plugin
* Restore both rules and report the same exports under both `unused-exports` and the namespace rule
* Keep the rules withdrawn

## Decision Outcome

Chosen option: "Restore both rules, move the findings of namespace-only modules to them, extend the plugin vocabulary and add a `node` plugin", because it gives every export exactly one rule, keeps the whole-namespace case silent and removes false findings on the virtual modules of common frameworks.

**Rules.** `unused-namespace-exports` (values) and `unused-namespace-types` (interfaces and type aliases) are registered again, in category `js`, with default severity `Warning` and fixability `Unsafe`, like `unused-exports` and `unused-types`. The JS rules are the fourteen that [ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md) lists as corrected by the index, plus these two: sixteen.

**Semantics.** A module is *namespace-only* when it is reached through `import * as ns` or `export * as ns` with members read as `ns.member` (also `ns['member']`, `const { member } = ns`, and `ns.Type` in a type position), by one or more such consumers, and by nothing else: no named or default import, no `import()` or `require()` of the whole module, no use of the namespace as a whole (passed as an argument, spread, indexed with a computed key, destructured with a rest element). Whole uses mark every export of the module used. For a namespace-only module, an export that no consumer reads is reported by `unused-namespace-exports` or `unused-namespace-types` and not by `unused-exports` or `unused-types`; every other module keeps its findings under the latter. The same exemptions apply as for `unused-exports`: `@public`, `@beta` and `@alpha` tags, `ignore-tags`, entry files, exports used in the signature of a used export, and the `[js] include-entry-exports` setting. `export * as ns from './m'` and `import * as ns from './m'; export { ns }` forward only the members read through `ns` by the module's consumers, and the whole of `./m` when `ns` is used as a whole or not read at all. `import { x } from './m'; export { x }` is a named re-export of `x`, as in TypeScript.

**Plugin vocabulary.** `PluginInput::VirtualModule(pattern)` names a specifier pattern the tool resolves at build time; it is a glob over the specifier or the package name and is never reported as unlisted, unused or unresolved. `PluginInput::LinkedConfig { plugin, path }` names a config file (or a glob for it) that another plugin reads with its own resolver; the file counts as a config file of the plugin that links it. `Enabler::Always` enables a plugin in every workspace.

**Plugins.** `docusaurus` declares `@generated/*`, the `@docusaurus/core` client API, `@theme/*`, `@theme-init/*` and `@theme-original/*` (with the classic theme or preset) as virtual modules, the alias `@site` and the Babel config as linked. `nuxt` declares `#app`, `#app/*`, `#build/*`, `#components`, `#imports`, `#internal/*` and `#spa-template` as virtual modules and the aliases `~`, `@`, `~~` and `@@`. `angular` links the `tsConfig` of its targets to `typescript` and the `karmaConfig` to `karma`. `expo` and `metro` link the Babel config. The new `node` plugin, enabled everywhere, makes `server.js` a production entry and, when a script runs `node --test`, the test files the runner finds by convention. The plugin table holds 61 plugins.

**Single-file components.** The `.vue`, `.svelte` and `.astro` analysis reads every code block, the imports of style blocks and `src` attributes, and the identifiers of the markup, so an import used only from a template counts as used.

### Consequences

* Good, because both knip issue types have a rule again and every unused export is reported by one rule.
* Good, because the severity of the namespace findings can be set apart from the others.
* Good, because Docusaurus and Nuxt projects stop reporting their generated modules.
* Bad, because a user who raised `unused-exports` to an error must raise the two new rules as well to keep reporting the exports of namespace-only modules.
* Bad, because the markup scan matches identifiers by name, so a template local that shadows an import hides an unused import.

### Confirmation

* `docs/rules.md` and `rule-ids.txt` list `unused-namespace-exports` and `unused-namespace-types`; `scripts/check-rule-ids.sh` passes.
* `skif rules --format json` reports both with default severity `warning`.
* `crates/skif-js/tests` holds fixtures for a namespace-only module, a namespace used as a whole, a namespace re-exported through a local binding and through `export * as`, and the single-file component templates.
* `crates/skif-js-plugins/tests/plugins.rs` holds the fixtures of the virtual modules, the linked configs and the `node` plugin, and `ADR_IDS` lists 61 ids.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Restore both rules, move the findings of namespace-only modules to them, extend the plugin vocabulary and add a `node` plugin

* Good, because there is one report per export and the whole-namespace case stays silent.
* Bad, because `unused-exports` loses findings that move to the new rules.

### Restore both rules and report the same exports under both

* Good, because `unused-exports` keeps every finding.
* Bad, because one export appears twice and a fix is offered twice.

### Keep the rules withdrawn

* Good, because it needs no work.
* Bad, because the namespace case of knip has no counterpart.

## More Information

* Amends: [ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md), [ADR 0025](0025-plugin-set-extension-thirty-five-more-tools.md)
* Related decisions: [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md)
* See the [ADR index](index.md).
