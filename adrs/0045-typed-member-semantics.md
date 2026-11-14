---
status: proposed
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Typed Member Semantics: What a Typed Read Credits, and the `type-resolved` Deviation

## Context and Problem Statement

With type awareness ([ADR 0043](0043-type-aware-analysis-on-tsrs.md)) the checker can tell which declaration a read
reaches. [ADR 0042](0042-unused-class-members.md) defines what `unused-class-members` judges and credits by name;
[ADR 0018](0018-export-namespace-and-member-rules.md) does the same for enum and namespace members. The comparison
harness ([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)) compares skif with knip, which has no type checker
in its current major version.

What counts as a use of a member when a read is typed, what when it is not, and how does the comparison account for
findings that change because of types?

## Decision Drivers

* A typed read credits exactly the member TypeScript would rename together with it
* Where the checker cannot settle a read, skif credits as it would without types: types never add a finding through
  missing information
* What is judged stays decided by the declarations ([ADR 0042](0042-unused-class-members.md)); only the source of uses
  changes
* Differences from knip caused by types are named, not silent

## Considered Options

* Credit through symbols, root symbols, base types and implemented interfaces; settle-or-credit-by-name; a
  `type-resolved` deviation
* Credit only the exact declaration a read resolves to
* Use types only to add uses, never to drop a name-based one

## Decision Outcome

Chosen option: "Credit through symbols, root symbols, base types and implemented interfaces; settle-or-credit-by-name;
a `type-resolved` deviation", because it credits what TypeScript treats as the same member, never turns missing
type information into a finding and keeps the knip comparison readable.

### What a typed read credits

* The reads skif asks about: property access, element access with a literal key, qualified names, binding patterns
  and destructuring assignments, shorthand properties, indexed access types (`C['m']`) and `'m' in x` with a literal.
  Only reads of names that some judged declaration has as a member are sent to the checker.
* The checker's symbol for the read, through its root symbols (unions, intersections, mapped and synthetic
  properties), credits each member it lands on.
* A read of a member of a base class or of an interface credits the members that override or implement it in the
  judged classes: a call through the interface reaches every implementation.
* A declaration is used as a whole when it escapes so that any member may be read: `keyof typeof E`, `Object.keys(E)`,
  a spread, a value alias passed on (`const X = E; f(X)`). Whole uses from types are only added, never subtracted.

### What is credited by name

* A read whose receiver is `any`, an error type or a value of an unresolved module credits every member of that name,
  as without types. So does every read in a file the checker did not cover, in a Vue, Svelte or Astro template, or in
  a string the framework exemptions of [ADR 0042](0042-unused-class-members.md) already cover.
* A package whose imports do not resolve (dependencies not installed) is `partial`; its unsettled reads are credited
  by name, so missing `node_modules` never produces a finding.

### Comparison

* The harness runs skif twice per repository, with and without types. A finding present only in one mode is
  classified `type-resolved` and listed apart; the knip comparison counts the untyped run, which matches knip's model.
* The baseline header records the mode; the typed-versus-untyped diff is recorded per repository in
  `tools/bench/baselines/type-aware.json`.

### Consequences

* Good, because a member is unused only when no typed read and no unsettled read of its name exists.
* Good, because the agreement numbers with knip stay comparable across releases.
* Bad, because a project with many `any` receivers gains little from types.

### Confirmation

* Fixtures, each with a real `tsconfig.json`: a typed receiver that leaves a same-named member of another class
  unused; two classes with one member name; a union receiver; an override and an interface implementation read
  through the base; an `any` receiver; an enum read through an alias; a namespace member read through `this`; a file
  outside every program; a Vue SFC reading a class member; a BOM and non-ASCII text before a declaration.
* `uninstalled_dependencies_credit_reads_by_name_and_report_partial`.
* `type_facts_are_identical_at_one_and_eight_threads`.
* `mise run parity -- --check` shows drift only in the `type-resolved` category.

## Pros and Cons of the Options

### Credit through symbols, root symbols, base types and interfaces

* Good, because it matches what TypeScript treats as the same member.
* Bad, because the closure over base types and interfaces costs queries.

### Only the exact declaration

* Good, because it is simplest.
* Bad, because members implemented for an interface and overrides called by a base would be reported as unused.

### Types only add uses

* Good, because it can never add a finding.
* Bad, because it fixes nothing: the false positives come from name-based uses that types would not reach.
