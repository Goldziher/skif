---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0029"]
---

# Namespace Reads Through Test-Runner Spies and Imports Awaited Together

## Context and Problem Statement

[ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md) counts a namespace
passed as an argument as a use of the whole module, so every export of the module is used. Two common forms of test
code fall under that rule although they read one member each. `vi.spyOn(ns, "a")`, `jest.spyOn(ns, "a")` and
Jasmine's `spyOn(ns, "a")` pass the namespace to replace the member `a` and nothing else. And
`const [{ a }, ns] = await Promise.all([import("./x"), import("./y")])` binds each `import()` to an element of the
array pattern, which the analysis did not see, so both modules were used as a whole. In the knip comparison on the
corpus (mastra, opencode, n8n, cal-com), knip reports the other exports of these modules and skif did not.

How does the analysis count these two forms?

## Decision Drivers

* A use of one member must not make every export of a module used
* A form whose member is not known statically stays a use as a whole
* The rule must stay a syntactic one: no type information, no knowledge of a test runner's runtime

## Considered Options

* Read the spied member and the array-pattern element as member reads
* Keep both forms as uses of the whole module

## Decision Outcome

Chosen option: "Read the spied member and the array-pattern element as member reads", because both forms name the
member they use in the source, as `ns.a` does.

**Spies.** A call `vi.spyOn(ns, "a")`, `jest.spyOn(ns, "a")` or `spyOn(ns, "a")` whose first argument is an
identifier and whose second is a string literal (or a template literal without expressions) reads member `a` of
that identifier. Any other argument list, a computed member name, or another object's `spyOn` method is still a use
as a whole.

**Imports awaited together.** In `const [p0, p1, …] = await Promise.all([e0, e1, …])` (with or without `await`),
each element `eN` that is an `import("…")` takes the bindings of the pattern element `pN`, read as the pattern of
`const pN = await import("…")` is: an identifier binds the namespace and its member reads count, an object pattern
binds the names it destructures. An import with no pattern element at its position (a hole, a shorter pattern, an
element behind a rest element) and every element after a spread element in the array stay uses as a whole.

A module that only such reads reach is namespace-only as before, so its unused exports are reported by
`unused-namespace-exports` and `unused-namespace-types`.

### Consequences

* Good, because spying on one member no longer hides the module's other unused exports.
* Good, because test helpers that load several modules at once are judged like separate `await import()` calls.
* Bad, because a spy on a member that does not exist names a member no module has; it reads nothing then, as
  `ns.missing` does.
* Bad, because the parse output changes, so the module cache schema moves.

### Confirmation

* `skif-js-syntax` tests `a_spy_on_a_namespace_member_reads_that_member`,
  `imports_awaited_together_bind_the_element_of_the_array_pattern_at_their_position` and
  `imports_awaited_together_after_a_spread_are_used_as_a_whole`.
* `skif-js` tests `a_spy_on_a_namespace_uses_only_the_member_it_spies_on` and
  `imports_awaited_together_use_only_what_their_bindings_read`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read the spied member and the array-pattern element as member reads

* Good, because the result matches how the code uses the module.
* Bad, because `spyOn` is recognised by name, so a project's own `spyOn(ns, "a")` that reads the whole object is
  counted as a read of `a`.

### Keep both forms as uses of the whole module

* Good, because nothing changes.
* Bad, because every module spied on in a test, or loaded through `Promise.all`, never has an unused export reported.

## More Information

* Amends: [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md) (what counts
  as a use of a namespace as a whole)
* Related decisions: [ADR 0068](0068-what-the-knip-comparison-sets-apart.md)
* See the [ADR index](index.md).
