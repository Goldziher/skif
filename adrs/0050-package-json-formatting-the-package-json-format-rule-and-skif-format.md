---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
---

# `package.json` Formatting: The `package-json-format` Rule and `skif format`

## Context and Problem Statement

[ADR 0001](0001-mission-and-scope.md) puts formatting `package.json` out of scope: key order outside dependency maps, indentation and whitespace belong to a formatter. syncpack has a `format` command that a migrating user loses ([#33](https://github.com/Goldziher/skif/issues/33)): `syncpack format` and `syncpack format --check` order the top-level keys (`sortFirst`, `sortPackages`), order the conditions of `exports` (`sortExports`), sort named fields alphabetically (`sortAz`), collapse `bugs` and `repository` to their shorthand (`formatBugs`, `formatRepository`) and write with an indent (`indent`). `skif migrate` drops these keys today with a note ([ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md)), so a repository that ran `syncpack format --check` in CI loses that check by moving to skif.

This record was written from syncpack 15.3.3 at commit 958d306 (`src/visit_formatting.rs`, `src/visit_formatting/format.rs`, `src/rcfile.rs`, `src/disk.rs`, `src/commands/format.rs`); syncpack.dev could not be reached when it was written.

## Decision Drivers

* A syncpack user keeps the `format --check` gate and the configuration that drives it
* skif's fix model stays the one way files change: structural, format-preserving, returned as a plan ([ADR 0008](0008-fix-and-edit-model.md))
* Two skif rules never ask for opposite orders of the same keys
* Library crates do no I/O and output is deterministic ([ADR 0003](0003-library-first-embedding.md))
* Nothing changes for a user who does not opt in

## Considered Options

* A rule, `package-json-format`, with a safe fix, configured by a `[format]` table, plus `skif format [--check]` that runs only that rule
* A standalone `skif format` subcommand with its own pipeline and no rule
* Keep formatting out of scope and point users at a JSON formatter

## Decision Outcome

Chosen option: "A rule, `package-json-format`, with a safe fix, configured by a `[format]` table, plus `skif format [--check]` that runs only that rule", because the rule and fix model ([ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md)) already gives everything `format` needs: a diagnostic per unformatted manifest, a safe fix the host can preview, `[[ignore]]`, `ignore-packages`, `[[overrides]]`, every output format and exit codes 0, 1 and 2 ([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)). A separate pipeline would duplicate each of them; the subcommand is a short name for one configuration of the existing one.

This record supersedes the "Formatting `package.json`" bullet of ADR 0001 for `package.json` files only.

### What is formatted

The rule reads each `package.json` the workspace does not ignore, the root included, and checks, in this order:

1. **Top-level key order**: the keys of `sort-first` first, in that order; the remaining keys in byte order when `sort-packages` is on, else in their current order.
2. **`bugs`** (`format-bugs`): an object whose only key is `url` becomes that string.
3. **`repository`** (`format-repository`): an object whose keys are only `url` and `type` becomes the URL string, with everything up to `github.com/` removed (`git+https://github.com/o/r.git` becomes `o/r.git`, as in syncpack). An object with a `directory`, or with any other key, is left alone.
4. **`exports`** (`sort-exports`): in every object of `exports`, at any depth, the keys of `sort-exports` come first in that order and the others keep their order; `default` is always last. Arrays are not entered.
5. **`sort-az`**: each named field, addressed like a JSON pointer without the leading slash (`scripts`, `a/b`), is sorted one level deep: an object by key, an array of strings by value. An array holding anything but strings is left alone.

The defaults are syncpack's: `sort-first = ["name", "description", "version", "author"]`, `sort-packages = true`, `sort-az = ["bin", "contributors", "dependencies", "devDependencies", "keywords", "peerDependencies", "resolutions", "scripts"]`, `sort-exports = ["types", "node-addons", "node", "browser", "module", "import", "require", "svelte", "development", "production", "script", "default"]`, `format-bugs = false`, `format-repository = false`, no `indent`. An empty list turns its step off.

### How the file is written

**Without `indent`, the layout is kept.** Reordering moves each member with its own text: the bytes between members (commas, newlines, indentation) stay where they were and each moved member keeps its nested layout, so a sorted file differs from the original only in where its members sit. A collapsed `bugs` or `repository` becomes a JSON string. Whitespace is never reported.

**With `indent`** (a non-empty run of spaces or tabs, or `"\t"`), the manifest must be laid out as `JSON.stringify(value, null, indent)` would lay it out, with the file's own line ending (CRLF when the file has one, else LF) and a final newline. A file that differs only in layout is reported too. Scalars keep their source text, so an escape or a number is never rewritten.

The fix replaces the smallest byte range that differs, as one `Text` operation of safety `Safe`. Formatting twice gives the same bytes as formatting once.

### What it does not do

It formats only `package.json` manifests of the workspace: no other JSON file, no `pnpm-workspace.yaml`, no general JSON formatter. It never adds, removes or renames a key except the two shorthand collapses, never reorders arrays outside `sort-az`, and never validates the manifest.

### Differences from syncpack

* **Byte order.** syncpack sorts with a collator (symbols, then digits, then letters, case-insensitively). skif uses byte order, which `unsorted-dependencies` already uses for dependency maps; with two orders the two rules would ask for opposite fixes of the same map.
* **No data is dropped.** syncpack collapses `bugs` to its `url` even when the object also has an `email`, and `repository` to its `url` when it has any key other than `directory`. skif collapses only when the shorthand holds everything the object held.
* **Layout is checked only with `indent`.** syncpack never reports whitespace and re-renders the whole file whenever it writes one. skif keeps the layout unless `indent` is set, and then checks it.

### Rule and command

`package-json-format` is a `manifest` rule, **off by default**, fixable `safe`; enabling it (`[rules] package-json-format = "error"`) makes every lint check formatting and `--fix` apply it. The `[format]` table is root-scoped: it cannot appear in an `[[overrides]]` entry.

`skif format [PATH]` lints with `package-json-format` at `error` and every other rule off, applies its fixes and never runs the install step; `skif format --check` reports and does not write. Both take `--recursive`, `--config`, `--no-config`, `--format` and `--color` as a lint does. The exit codes are a lint's: `--check` exits `1` when a manifest is not formatted, `0` when every one is, and `2` when skif could not run; without `--check` the exit code comes from the lint after writing. Writing is a fix run, so it is refused under a truthy `CI` like `--fix`.

`skif migrate` writes syncpack's `sortFirst`, `sortAz`, `sortExports`, `sortPackages`, `formatBugs`, `formatRepository` and `indent` to `[format]`, and turns `package-json-format` on when the syncpack configuration sets one of them or a script runs `syncpack format`.

### Consequences

* Good, because a syncpack user keeps `format --check` as `skif format --check` (or as the rule in the normal lint) with the same configuration.
* Good, because the format check is suppressed, reported, previewed and fixed like every other rule, and a host gets it through the library with no new API.
* Good, because without `indent` a formatted file keeps every byte of layout, which keeps diffs to the moved lines.
* Bad, because a project that relied on syncpack's collator order sees keys move once when it moves to skif.
* Bad, because with `unsorted-dependencies` also on, an unsorted dependency map is reported by both rules; the two fixes overlap, the later is skipped ([ADR 0008](0008-fix-and-edit-model.md)), and both agree on the result.
* Bad, because the fix replaces a range rather than issuing structural operations, so it conflicts with any other fix in the same span of the file and the next run applies it.

### Confirmation

* `crates/skif-edit/src/package_json.rs` tests each step, the defaults, idempotence (formatting the output again changes nothing) and that scalars keep their source text.
* `crates/skif-edit/tests/fixtures/package-json/` holds manifests whose before-and-after text `crates/skif-edit/tests/package_json_format.rs` snapshots, and a property test there formats generated manifests twice.
* `crates/skif-rules/src/manifest/package_json_format.rs` tests the rule and that its fix leaves no fixable diagnostic.
* `crates/skif-cli/tests/cli.rs` covers `skif format --check` exiting 1 and 0, the refusal under `CI`, and `skif format` writing the fix once.
* `crates/skif-migrate/tests/golden.rs` covers the translation of syncpack's format options and of a `syncpack format` script.

## Pros and Cons of the Options

### A rule, `package-json-format`, with a safe fix, configured by a `[format]` table, plus `skif format [--check]` that runs only that rule

* Good, because suppression, reporting, previews, exit codes and the library API come from the existing model.
* Good, because a team can make formatting part of every lint instead of a second command.
* Bad, because the rule overlaps `unsorted-dependencies` on dependency maps, so both report one unsorted map.

### A standalone `skif format` subcommand with its own pipeline and no rule

* Good, because it mirrors syncpack's command line exactly.
* Bad, because it needs its own reporting, ignores, JSON output and exit codes, and a host could not get the check through the library.

### Keep formatting out of scope and point users at a JSON formatter

* Good, because it keeps ADR 0001 as written.
* Bad, because no general JSON formatter knows `sortFirst`, `sortExports` or the shorthand fields, so a syncpack user loses the check.

## More Information

* Supersedes, in part: [ADR 0001](0001-mission-and-scope.md), the "Formatting `package.json`" bullet of the out-of-scope list
* Related decisions: [ADR 0004](0004-configuration-model.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), [ADR 0028](0028-cli-flag-contract-completion-and-withdrawn-confirmations.md), [ADR 0039](0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md)
* Upstream: [syncpack](https://github.com/JamieMason/syncpack)
* See the [ADR index](index.md).
