---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0050", "0051", "0052"]
---

# Custom Element Registrations and the taskr Plugin

## Context and Problem Statement

A class registered as a custom element is used by the browser through its tag name: `customElements.define("x-y", Foo)`, Lit's `@customElement("x-y")`, Stencil's `@Component({ tag })`, Catalyst's `@controller` or FAST's `@customElement` and `FASTElement.define()`. No module imports such a class, so `unused-exports` reports it. knip marks it as used: its core walker records the classes passed to `customElements.define` in every file, and the `lit`, `stencil`, `catalyst` and `fast` plugins register a source visitor (`_custom-elements`) that records the classes their decorators or base class register.

skif's plugins report entries, dependencies and scripts, not export usage. For that reason `lit` was not added ([ADR 0050](0050-twenty-one-more-plugins-and-commands-run-from-source.md)), `stencil` does not mark its components ([ADR 0051](0051-thirty-five-more-plugins-frameworks-task-runners-and-options-written-anywhere.md)), and `catalyst` and `fast` were not added ([ADR 0052](0052-thirty-nine-more-plugins-and-plugins-enabled-by-a-package-json-key.md)). Issue #38 lists custom elements as a missing knip convention.

Issue #20 also asks for a `taskr` plugin: next.js references most of its compiled dependencies from `taskfile.js` and from the taskr plugins it loads, and knip 6.44.1 has no such plugin.

How does a plugin mark an export as used, which registrations count, and what does `taskr` read?

## Decision Drivers

* Library crates do no I/O and expose no oxc type ([ADR 0010](0010-js-analysis-on-oxc.md)); analysis stays static
* A plugin is a declarative row where it can be ([ADR 0011](0011-plugin-model.md))
* Follow knip 6.44.1 where it has a counterpart ([ADR 0036](0036-thirty-nine-more-knip-plugins-by-real-world-usage.md))
* A plugin switched off in `[js.plugins]` stops contributing

## Considered Options

* Record registrations in the module facts, and let the enabled plugins name the libraries whose registrations count
* Count every decorated or `define`d class as registered, whatever the library
* Give plugins a source visitor of their own, as knip does

## Decision Outcome

Chosen option: "Record registrations in the module facts, and let the enabled plugins name the libraries whose registrations count", because the parser stays free of tool knowledge, the plugin stays a declarative row, and the result matches knip.

`skif-js-syntax` records on each `Export` the `registrations` of the class behind it:

* `Define`: the class is the second argument of `customElements.define(…)`, also called through `<x>.customElements` (`window.customElements`), through a variable initialised with `customElements`, `<x>.customElements` or `new CustomElementRegistry()`, or with `this` inside a static block of the class.
* `Decorator { module, name }`: a top-level class carries `@name`, `@name(…)` or `@ns.name(…)` whose binding is a static import of `module`.
* `BaseClassDefine { module, name }`: a top-level class extends `name` imported from `module`, directly or as an argument of a mixin call (`extends Mixin(Base)`), and `Class.define(…)` or `Class.defineAsync(…)` is called.

A registration of a local class applies to every export whose local binding is that class (`export { Foo as Bar }`); a decorated `export default class` applies to `default` as well.

`PluginSpec` gains `custom_elements`: the library's module specifiers (a trailing `/` matches every specifier below it), the registering decorator's exported name, and the base class, when there is one. The JS analysis treats an export as used when one of its registrations is `Define`, or matches the library of a plugin enabled for the export's package. A registered export also keeps the exports whose types its signature mentions in use, as knip's `isRegistered` does, and `skif explain` reports it as `registered`.

`Define` always counts, as in knip's core walker. The decorator and base class registrations count only through an enabled plugin, as in knip; skif resolves the plugins per package, where knip registers a plugin's visitor for the whole run once any workspace enables it. A decorated class in a package that does not depend on the library imports it as an unlisted dependency anyway.

The plugins `lit` (enabled by `lit`, `lit-element` or `@lit/reactive-element`; `@customElement` from `lit/decorators.js`, `lit/decorators/…`, the same under `@lit/reactive-element`, and `lit-element`), `catalyst` (`@controller` from `@github/catalyst`) and `fast` (`@customElement` and `FASTElement` from `@microsoft/fast-element`) are added, and `stencil` names `@Component` from `@stencil/core`.

`taskr` has no knip counterpart and follows taskr's own loading rules: enabled by the `taskr` dependency, `{T,t}askfile.js` is an entry, the paths of `package.json#taskr.requires` are entries (a package name there is a dependency), and every `@taskr/*` and `taskr-*` dependency is used, as taskr loads those by name. The table holds 204 plugins.

Where skif differs from knip:

* A local binding that shadows `customElements` is not detected; knip checks the scope.
* Registrations are resolved per package, as stated above.

### Consequences

* Good, because web component libraries no longer report every element class as an unused export.
* Good, because a further library is a row with a `custom_elements` value, not code in the parser.
* Neutral, because `Export` has one more field, defaulted when deserialised.
* Bad, because a class registered in a way none of the three forms covers (a registry passed in as a parameter, a class expression) is still reported.

### Confirmation

* `crates/skif-js-syntax/tests/module_info.rs` covers each registration form and the forms that do not register.
* `crates/skif-js/tests/custom_elements.rs` lints the `custom-elements` fixture with an insta snapshot: registered elements of `lit`, `fast`, `catalyst` and `stencil` are not reported, unregistered ones and a decorator of a local module are, and a decorator stops counting when its plugin is switched off; `crates/skif-js/tests/taskr.rs` lints the `taskr` fixture, whose taskfile, required plugin and `@taskr/*` dependencies count as used.
* `crates/skif-js-plugins/tests/plugins.rs` lists the 204 ids in `ADR_IDS` and tests the library of each custom element plugin and the inputs of `taskr`.
* `scripts/check-plugin-list.sh` fails when the plugin lists or counts in the README files and the website differ from the code.

## Pros and Cons of the Options

### Record registrations in the module facts, and let the enabled plugins name the libraries whose registrations count

* Good, because parsing stays independent of the plugin set and runs once per file.
* Good, because `[js.plugins] lit = false` switches the decorator off, like any plugin.
* Bad, because the module facts carry registrations no enabled plugin needs.

### Count every decorated or `define`d class as registered, whatever the library

* Good, because no plugin is needed.
* Bad, because any decorator imported from any package (`@Injectable` from `@angular/core`) would hide unused classes.

### Give plugins a source visitor of their own, as knip does

* Good, because a plugin could express any rule.
* Bad, because plugins would see oxc types or need a visitor API of their own, against [ADR 0010](0010-js-analysis-on-oxc.md), and every file would be walked once per plugin.
