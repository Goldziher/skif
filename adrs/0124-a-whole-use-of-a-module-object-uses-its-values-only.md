---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0029"]
---

# A Whole Use of a Module Object Uses Its Values Only

## Context and Problem Statement

[ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md) decides that a use of a
namespace as a whole (passed as an argument, spread, indexed with a computed key, destructured with a rest element),
and an `import()` or `require()` of the whole module, marks every export of the module used. That includes its
interfaces and type aliases. None of these uses can reach a type: the module object that `import * as ns`, `import()`
and `require()` give holds the values of the module only, and `typeof ns` is the type of that object, which has no
member for a type alias. A module whose only consumer writes `require("./m") as typeof M` with
`import type * as M from "./m"` therefore never has an unused type reported, though nothing can read one.

Should a whole use of a module object use the module's type exports?

## Decision Drivers

* A finding is reported when nothing can use the export, and kept silent when something can
* The answer is the same for every way of using the module object as a whole
* A type read through the namespace by name (`ns.Type`) stays a use of that type

## Considered Options

* A whole use of a module object uses every value export and no type export
* Keep ADR 0029: a whole use uses every export

## Decision Outcome

Chosen option: "A whole use of a module object uses every value export and no type export", because the object such a
use reaches has no types, so crediting them hides unused types and nothing else.

A namespace binding used as a whole, an `import()` or `require()` whose result is not bound to names, and a
`typeof ns` of a namespace binding use every export of the module that exists at value level: everything except
`interface`s, type aliases and `export type` re-exports, with `export *` followed to the modules it forwards. A named
re-export that is not marked `type` counts as a value, as only its source tells what it is. The members such a
use reads by name (`ns.Type` in a type position) are used as before. A namespace binding that the importing module
exports itself (`export default ns`, `export = ns`) keeps using every export, values and types, since the importers of
that module can read any of them through it. Entry modules, `export * as ns` re-exports and the modules a glob
import matches keep marking every export used, as ADR 0029 decides.

A module used as a whole is not namespace-only, so its unused types are reported by `unused-types`, not by
`unused-namespace-types`. The rest of ADR 0029 is unchanged.

### Consequences

* Good, because the interfaces and type aliases of a module that is only loaded with `require()` or `import()`, or
  only used as a whole through a namespace, are reported when nothing reads them.
* Good, because `explain-usage` lists a whole use as a user of a value export and not of a type export, so the
  evidence agrees with the verdict.
* Bad, because knip does not report these types by default (only under `nsTypes`, which is off by default, and not
  for a spread or an `import()`), so skif reports types that knip does not.
* Neutral, because a namespace that a module exports under another name (`export const shapes = ns`) uses the values
  only; an importer that reads a type through that constant cannot do so in TypeScript either.

### Confirmation

* `crates/skif-js/tests/namespaces_and_components.rs`:
  `a_namespace_used_as_a_whole_uses_every_value_export_and_no_type` (a spread, `Object.keys`, a computed index,
  `typeof` of a type-only namespace import, `require()` and `import()`) and
  `a_whole_namespace_use_keeps_the_types_read_by_name_and_an_exported_namespace_keeps_all`.
* `crates/skif-js/tests/explain_absence.rs`: `a_whole_namespace_use_is_listed_for_values_and_not_for_types`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A whole use of a module object uses every value export and no type export

* Good, because it follows what the use can reach.
* Bad, because skif reports types that knip's default issue types do not.

### Keep ADR 0029: a whole use uses every export

* Good, because nothing changes.
* Bad, because an unused type of a module used only as a whole is never reported.

## More Information

* Amends: [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md) ("Whole uses
  mark every export of the module used")
* See the [ADR index](index.md).
