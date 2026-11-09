---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0034", "0048"]
---

# Reasons on `[js]` Ignore Entries, a `[lint] require-ignore-reasons` Key, and Comments Carried Over by `skif migrate`

## Context and Problem Statement

[ADR 0048](0048-catalog-version-divergence-and-reasons-on-ignores.md) gave `[[ignore]]` an optional `reason` and
`--require-ignore-reasons` to enforce one. The `[js]` ignore lists of [ADR 0034](0034-js-configuration-parity-with-knip.md)
(`ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, `ignore-files`, `ignore-members`,
`ignore-workspaces`, their `extend-` keys and their per-workspace and override forms) are plain string arrays, so an
entry there cannot say why it exists and the flag cannot ask it to. `skif migrate` drops the comment a knip
configuration writes above an entry to explain it (GitHub issue #113):

```ts
export default {
    workspaces: {
        'packages/app': {
            ignoreDependencies: [
                // false positive: loaded by the bundler from a config string
                'some-loader',
            ],
        },
    },
};
```

becomes `ignore-dependencies = ["some-loader"]`. A team that requires every suppression to document itself needs a
custom guard over `knip.ts` for this.

How does an entry of a `[js]` ignore list record its reason, how is one required, and what does `skif migrate` carry
over?

## Decision Drivers

* Every kind of suppression should be able to carry a reason, and one check should require it everywhere
* The common case, an entry without a reason, stays a bare string, and existing files keep loading unchanged
* The reason the original tool's configuration states must survive the migration
* A team should be able to require reasons from the file, not only from every command line that runs skif

## Considered Options

* Accept an inline table `{ name, reason }` in place of a string in every `[js]` ignore list, add `[lint] require-ignore-reasons`, and have `skif migrate` turn comments into reasons
* A separate `[js.ignore-reasons]` map from entry to reason
* TOML comments only

## Decision Outcome

Chosen option: "Accept an inline table `{ name, reason }` in place of a string in every `[js]` ignore list, add `[lint] require-ignore-reasons`, and have `skif migrate` turn comments into reasons", because the reason then sits on the entry it explains, a list may mix both forms, and a comment cannot be enforced (as ADR 0048 found for `[[ignore]]`).

**The table form.** Every `[js]` ignore list (the six above and the three `extend-` keys, at the top level, in
`[js.workspaces."<dir>"]` and in the `js` of an `[[overrides]]` entry) takes `{ name = "...", reason = "..." }` in
place of a string. `name` holds exactly what the string form holds: a name, a glob or a slash-delimited regular
expression, with the `!` production marker; the key is `name` for every list. `reason` is optional, and a blank one is
a configuration error naming the key and index, as for `[[ignore]]`. Any other key in the table is an error that names
it. The model has one type for an entry, `IgnoreItem` (a `name` and an optional `reason`), shared by every list; it
compares with a string by its name, converts from one, and serializes back as a bare string when it has no reason. The
JSON Schema describes an entry as a string or the table. Matching, validation and `stale-js-config` use the name only:
a table entry that matches nothing is reported by its name, as a string entry is.

**Requiring reasons.** `--require-ignore-reasons` fails a run with exit code 2 on the first suppression of a
configuration file without a reason: an `[[ignore]]` entry, also one in an override, or an entry of a `[js]` ignore
list. The message names the entry (``js.workspaces."packages/app".ignore-dependencies[1] `some-loader`: no `reason`,
which --require-ignore-reasons requires``). The check runs on the merged configuration, so the key and position are
those of the merged list, and the entry's name is what finds it in the file that wrote it; a blank `reason` is
rejected while each file is read, so that error names the file and the key written there
(`js.extend-ignore-dependencies[0]`). `[workspace] ignore-packages` and `[js] ignore` hold globs, not suppressions of
a reported finding, and stay plain strings outside the check. `[lint] require-ignore-reasons = true` turns on the same check from the
file; the flag and the key are or-ed, and the message names the flag when both are set. Entries that flags add stay
exempt. The check is `Config::missing_ignore_reason`, so a library host can run it too; the CLI is the one consumer
that fails on it.

**Comments carried over by `skif migrate`.** In a knip configuration file read statically (any of the JSON, JSON with
comments, JavaScript and TypeScript files `skif migrate` reads; the `knip` key of `package.json` is JSON and has no
comments), the comment on an entry of `ignoreDependencies`, `ignoreBinaries`, `ignoreUnresolved`,
`ignoreMembers`, `ignoreFiles` or `ignoreWorkspaces` becomes its `reason`. A comment belongs to an element when it is on
the lines directly above it with no blank line in between, or after it on the line where it ends; a comment on the
line where the previous element ends belongs to that element, one on the line of the opening bracket to none, and tool
directives are not reasons. A directive is one of the words `eslint-disable`, `eslint-disable-next-line`,
`eslint-disable-line`, `eslint-enable`, `eslint-env`, `@ts-ignore`, `@ts-expect-error`, `@ts-nocheck`, `@ts-check`,
`istanbul ignore`, `c8 ignore`, `prettier-ignore`, `biome-ignore` (and its `-all`, `-start`, `-end` forms),
`oxlint-disable`, `oxlint-disable-next-line`, `oxlint-disable-line` and `oxlint-enable` at the start of the comment
and followed by its end, a space or a colon, or the block comment `/* eslint <rule>: ... */`; a reason that starts
with a package name (`// eslint-plugin-foo is loaded by name`, `// @ts-rest/core is pulled in by codegen`) is kept. A
property or element that names a top-level `const` bound to an object or array literal is followed into it, so a list
written as a constant keeps its comments; a spread is not followed. `skif-js-syntax` finds these
comments with oxc (`extract_array_comments`) and returns, for each, the path of keys from the outermost literal to the
array, the element's position, whether it is a spread or follows one, and its string or pattern, without exposing oxc
types. The translation pairs a knip entry with a comment by that string and by the workspace key of the path, so a
spread in the list does not shift reasons onto the wrong entries. Entries without a comment stay plain strings.
syncpack's ignores are groups, which have no `reason`; a comment above a group in a YAML, JSON-with-comments or
JavaScript syncpack configuration is written as a comment above the group's `[[version-groups]]`, `[[semver-groups]]`
or `[[update-groups]]` entry. Groups are objects with no string to pair by, so they are paired by position, and
position is only sound before the first spread: the evaluated list expands `...shared`, so the comment on a spread or
on a group after one is not carried over rather than written above the wrong group. YAML is read line by line for its block style,
without a new dependency. sherif's configuration is a `package.json` key, and JSON has no comments.

### Consequences

* Good, because every `[[ignore]]` entry and every entry of a `[js]` ignore list can state its reason in the file,
  and one flag or key requires it.
* Bad, because `[workspace] ignore-packages` and `[js] ignore` cannot carry a reason, and the check does not cover
  them.
* Bad, because a missing reason is named by its place in the merged list, which differs from its place in the file
  when `extends` or an `extend-` key added entries before it.
* Good, because a migrated knip configuration keeps the explanations its authors wrote and passes
  `--require-ignore-reasons` wherever they wrote one.
* Good, because files without reasons load and serialize exactly as before.
* Bad, because the `[js]` ignore-list fields of the public model change type from `Vec<String>` to
  `Vec<IgnoreItem>`; code that reads them as strings must read `name`.
* Bad, because the YAML comment reader understands block style only, and a comment on a flow-style list (`[a, b]`) is
  not carried over.
* Bad, because a syncpack comment becomes a TOML comment, which `--require-ignore-reasons` does not see.

### Confirmation

* `skif-config` tests parse both forms and mixed lists at every level, round-trip them, reject a wrong key and a blank
  reason by name, describe both forms in the schema, and name the first entry without a reason
  (`the_first_suppression_without_a_reason_is_named_by_its_key`).
* `require_ignore_reasons_covers_the_js_ignore_lists_and_the_lint_key` in `crates/skif-cli/tests/cli.rs` covers the
  flag, the key and a passing run.
* `ignore_entries_with_a_reason_suppress_and_are_judged_like_strings` in `crates/skif-js/tests/js_config.rs` covers
  matching and `stale-js-config`.
* `skif-migrate` golden tests migrate the issue's reproduction to
  `{ name = "some-loader", reason = "false positive: loaded by the bundler from a config string" }`, a trailing
  comment, a JSON-with-comments knip file and a YAML syncpack comment; `skif-js-syntax` and `skif-migrate` unit tests
  cover which comment belongs to which element.

## Pros and Cons of the Options

### Accept an inline table `{ name, reason }` in place of a string in every `[js]` ignore list, add `[lint] require-ignore-reasons`, and have `skif migrate` turn comments into reasons

* Good, because the reason is next to the entry and moves with it.
* Good, because the common case stays a one-word string.
* Bad, because a list now has two forms to read.

### A separate `[js.ignore-reasons]` map from entry to reason

* Good, because the lists keep a single form.
* Bad, because the reason and the entry drift apart: renaming or removing one leaves the other behind, and the same
  entry in two lists has one reason.

### TOML comments only

* Good, because nothing in the model changes.
* Bad, because a comment cannot be required, which is what the issue asks for.

## More Information

* Amends: [ADR 0034](0034-js-configuration-parity-with-knip.md) (the entries of name lists and `ignore-files` may be
  tables), [ADR 0048](0048-catalog-version-divergence-and-reasons-on-ignores.md) (what `--require-ignore-reasons`
  covers, and the `[lint]` key)
* Related decisions: [ADR 0004](0004-configuration-model.md),
  [ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md),
  [ADR 0041](0041-skif-migrate-composable-steps-full-validation-and-in-place-merge.md)
