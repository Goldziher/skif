---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0034"]
---

# A Whole Use of an Enum in Its Own File Uses Every Member

## Context and Problem Statement

[ADR 0034](0034-js-configuration-parity-with-knip.md) states that the members of enums and namespaces read in their
own file count as used. skif counts a member read by name there (`E.A`, `E["A"]`, `const { A } = E`, the type `E.A`)
and, as knip does, nothing else: a use of the whole enum in the file that declares it credits no member. Such a use can
read any member. `Object.values(E)` and `Object.keys(E)` list them, `E[key]` with a computed key, a spread and
`for (k in E)` read whichever the key names, and `E` passed, stored or returned hands every member to code that may
read any of them. In a type, `typeof E` (also `keyof typeof E`) is the type of the enum object, and the template literal
type `` `${E}` `` spells out the value of every member. A file that declares an enum, derives a type of its member
values with `` `${E}` `` and exports only that type therefore gets every member reported, though removing one changes
the type.

A use of an enum as a whole from another file (`import { E }` with `Object.values(E)`, `keyof typeof E` or
`` `${E}` ``) already uses every member: the import of `E` is used in a way that cannot be reduced to its members.

Should a whole use of an enum in its own file use every member?

## Decision Drivers

* A finding is reported when nothing can use the member, and kept silent when something can
* A whole use counts the same in the declaring file as in another file
* A member read by name stays a use of that member only

## Considered Options

* A whole use of an enum in its own file uses every member
* Keep knip's behaviour: only members read by name in the file count

## Decision Outcome

Chosen option: "A whole use of an enum in its own file uses every member", because each such use can reach any member,
so a member it reaches is not unused.

In the file that declares an enum export, these use every member of it: a reference to the enum that is no read of a
member by name, as a value (`Object.values(E)`, `Object.keys(E)`, `E[key]` with a key that is not a string literal, a
spread, `for (k in E)`, an argument, a variable initialiser, a return value, `export default E`), `typeof E` (also in
`keyof typeof E`) and `E` interpolated in a template literal type (`` `${E}` ``, `` `on-${E}` ``). `export { E }`
publishes the enum and reads none of it. A type annotation `x: E` names the enum type, a union of the members, and
reads no member: a value of the type is one of the members, which the code that produces it names. `E.A`,
`E["A"]`, `const { A } = E`, the type `E.A`, `typeof E.A` and `` `${E.A}` `` read `A` only. References are matched by
name, as the reads by name are, so a parameter or local named like the enum counts as a use of it. Namespaces keep
ADR 0034: only their members read by name in the file count.

### Consequences

* Good, because an enum whose members a file lists, iterates or turns into a type has none of them reported.
* Good, because a whole use counts the same whether it is in the declaring file or in an importer.
* Bad, because for an `export enum` with a whole use in its own file skif reports fewer members than knip, which counts
  only the members read by name there; knip's findings in that case are false positives.
* Neutral, because a whole use in another file still also counts a type annotation `x: E` there as a use of every
  member, while in the declaring file it counts none; this record does not change the importer's side.

### Confirmation

* `crates/skif-js-syntax/tests/fp_graph.rs`: `members_read_by_name_in_the_declaring_file_are_recorded`,
  `an_enum_used_as_a_whole_in_its_own_file_is_flagged_and_one_read_by_name_or_annotated_is_not` and
  `a_namespace_used_as_a_whole_in_its_own_file_is_not_flagged`.
* `crates/skif-js/tests/knip_parity.rs`: `a_member_read_in_its_own_file_counts_and_a_whole_use_there_uses_every_member`
  and `a_whole_use_of_an_imported_enum_uses_every_member`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A whole use of an enum in its own file uses every member

* Good, because it follows what the use can reach.
* Bad, because skif reports fewer enum members than knip.

### Keep knip's behaviour: only members read by name in the file count

* Good, because skif and knip agree.
* Bad, because the members of an enum used only as a whole in its own file are all reported, though removing one
  changes what the file computes.

## More Information

* Amends: [ADR 0034](0034-js-configuration-parity-with-knip.md) ("Members of enums and namespaces read in their own
  file already count as used")
* See the [ADR index](index.md).
