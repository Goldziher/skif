---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Type-Only Class Fields and Constant Element Keys

## Context and Problem Statement

[ADR 0042](0042-unused-class-members.md) credits a class member when any module reads a property of its name. A
review of the rule's findings on the comparison corpus found two kinds of false positives that need no type
information to avoid:

* Fields that exist only for the type system: nominal brands (`readonly _rgbaBrand: void = undefined`,
  `readonly __brand!: unique symbol`) and `declare` fields that carry a phantom type. Nothing reads them at runtime,
  by design.
* Reads through a constant key: `target[ReactiveFlags.SKIP]` where `enum ReactiveFlags { SKIP = '__v_skip' }` reads
  the member `__v_skip`, as does `x[KEY]` with `const KEY = '__v_skip'` or a property of an `as const` object.

Which fields does the rule leave unjudged, and which element accesses count as reads?

## Decision Drivers

* The rule reports what is dead code, not what exists to shape types
* A read skif can prove statically counts; a key it cannot prove reads nothing, as before
* No name patterns: a convention one project uses is configuration (`ignore-members`), not a default

## Considered Options

* Exempt type-only fields by their declaration; resolve constant keys through scope and the module graph
* Exempt fields by a default name pattern (`/^_.*Brand$/`)
* Leave both to a type-aware member source

## Decision Outcome

Chosen option: "Exempt type-only fields by their declaration; resolve constant keys through scope and the module
graph", because both cases are decided by syntax alone and neither depends on a naming convention.

### Type-only fields

* A `declare` field is not judged.
* A field whose type annotation is `void`, `never`, `undefined` or `unique symbol` and that has no value, the value
  `undefined`, or the definite-assignment form (`!`) is not judged. With any other value it is judged as before.
* The exemption is `type-only-field`; `explain-usage` names it.

### Constant element keys

* An element access `x[K]` reads the string `K` holds when `K` is a top-level `const` initialised with a string, a
  string enum member (`E.M`), or a string property of a `const` object declared `as const` (`O.M`).
* In the same module, `K` is resolved by scope, so a parameter or local of the same name, a `let` or a plain object
  reads nothing.
* From another module, `K` is resolved on the module graph through named, default and namespace imports, re-exports
  and `export *`.
* Not followed: constants nested more than one level, `Object.freeze`, numeric enum members. An assignment through an
  imported key counts as a read.

### Module facts and cache

* `ModuleInfo` records `string_constants` (the constants of exported bindings, by export name), `imported_key_reads`
  and `ClassMember::type_only`. The parse cache schema moves to 11.

### Consequences

* Good, because brands and constant-key reads stop producing findings, without a configuration change.
* Neutral: a project that names its brands differently is covered as long as their type is one of the four.
* Bad, because keys built in other ways (computed strings, nested constants) still read nothing.

### Confirmation

* `type_only_fields_are_not_judged`, `explain_usage_names_the_type_only_exemption`,
  `an_element_access_through_a_constant_key_reads_its_string`,
  `an_element_access_through_a_key_that_is_no_constant_reads_nothing` and
  `an_element_access_through_an_imported_constant_reads_its_string` in `crates/skif-js/tests/class_members.rs`.
* `type_only_fields_are_marked` and `constant_keys_are_read_as_their_strings` in
  `crates/skif-js-syntax/tests/module_info.rs`.

## Pros and Cons of the Options

### By declaration and by resolved constant

* Good, because it holds for every project without configuration.
* Bad, because it adds module facts and a cache schema change.

### A default name pattern

* Good, because it is one line.
* Bad, because it encodes one project's convention and misses brands named otherwise.

### Leave both to types

* Good, because no new syntactic facts.
* Bad, because brands are unread with types too, and untyped projects keep the false positives.
