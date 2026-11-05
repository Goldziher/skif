---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# `[js]` Configuration Parity with knip: Regular Expression Entries, `ignore-unresolved`, Per-Workspace Keys, the Production Marker and `stale-js-config`

## Context and Problem Statement

[ADR 0010](0010-js-analysis-on-oxc.md) put the knip analysis behind a `[js]` table. Real knip configurations use constructs that table did not accept: ignore lists with regular expressions, `ignoreUnresolved`, `ignoreWorkspaces`, a `paths` map and plugin options per workspace, `ignoreExportsUsedInFile`, `ignoreMembers`, `includeEntryExports` per workspace, the `!` production marker on `entry` and `project`, and a `"."` workspace for the root. knip also prints configuration hints for ignore entries that matched nothing. Without these, a knip configuration cannot be translated and a stale entry stays in the file for good.

## Decision Drivers

* A knip configuration should map onto `skif.toml` key by key
* A key skif accepts must do what knip's does, or the difference must be stated
* Configuration that cannot work (a pattern that does not compile, a plugin that does not exist) is an error; configuration that is merely unused is a finding
* Locked records stay unedited

## Considered Options

* Add the knip keys to `[js]` and `[js.workspaces]`, validate the static part when the configuration is checked, and report unused entries through a rule
* Accept the keys but leave unused entries unreported
* Report unused entries as configuration errors

## Decision Outcome

Chosen option: "Add the knip keys to `[js]` and `[js.workspaces]`, validate the static part when the configuration is checked, and report unused entries through a rule", because a knip configuration then maps onto `skif.toml` key by key and an unused entry follows the severity and suppression machinery of every other finding.

**Entries of name lists.** `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, `ignore-members` and `ignore-workspaces` hold exact names, globs (`@types/*`) and regular expressions. An entry is a regular expression when it is written between slashes with optional flags (`/^@org\/.+$/`, `/^vite/i`) or when it contains one of `+ \ ( ) | ^`, `.*` or `.+`, or ends in `$`. knip's own rule is a character test that also counts `*`, `{` and `$` anywhere; that would make `@types/*` a regular expression that matches any name containing `@types`, so skif keeps `*` and `{` for globs. The expression is not anchored, as in knip. An entry that does not compile is a configuration error naming the key and index.

**The production marker.** In a name list or `ignore-workspaces`, an entry ending in `!` applies in production mode only and the marker is removed; this is knip's `filterIsProduction`. In `entry` and `project` the marker has the opposite meaning, as in knip: a normal run treats `pattern!` as `pattern`; a production run keeps only the patterns that end in `!` and excludes the others. When no pattern ends in `!`, a production run falls back to the default globs, with the others excluded.

**`ignore-unresolved`.** Import specifiers that must not be reported as unresolved have their own key, at the top level and per workspace. `ignore-dependencies` still silences `unresolved-imports` for the same specifier, because plugins declare virtual modules through that list; this is kept for compatibility and is not documented as the way to ignore an unresolved import.

**Per-workspace keys.** `[js.workspaces."<dir>"]` gains `paths`, `plugins`, `ignore-unresolved`, `ignore-members`, `ignore-exports-used-in-file` and `include-entry-exports`. The key `"."` is the root workspace: its lists replace `entry` and `project` of the top level and add to the other lists, which already apply to every workspace. `paths` is a `tsconfig`-style map of aliases to target directories relative to the workspace; each target becomes an alias of the resolver, ahead of the aliases tools declare. `plugins` of a workspace apply over `[js.plugins]` and under `[[overrides]]`. A key that names a directory or glob selects the package by the same rule as before (an exact key wins over a glob).

**`ignore-workspaces`.** Globs of workspace directories the JavaScript analysis skips: the packages contribute no files, entries or dependency findings. They stay in the workspace, so manifest and policy rules still check them, which separates the key from `[workspace] ignore-packages`. The root is never skipped.

**`ignore-exports-used-in-file`.** `true`, or a table of the kinds `variable`, `function`, `class`, `enum`, `namespace`, `type` and `interface`, with `member` accepted. skif does not tell a type alias from an interface, so the two keys are one. Members of enums and namespaces read in their own file already count as used, so `member` has no further effect. An export is left unreported only when the same file references it.

**Validation.** Invalid globs and regular expressions, empty `paths` entries and plugin ids that are not in the plugin table are configuration errors raised when the configuration is checked, with the closest plugin id suggested. A `[js.workspaces]` key that selects no package is not an error: whether it does depends on the workspace, and `unmatched-workspace-patterns` already reports unmatched patterns as a warning.

**`stale-js-config`.** A `js` rule, warning by default, reports `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved` and `ignore-workspaces` entries that matched nothing in the run and `[js.workspaces]` keys that select no package. The finding points at the configuration file, or the root `package.json` when the file is unknown. `[rules] stale-js-config = "error"` is knip's `--treat-config-hints-as-errors`. The rule runs after the other `js` rules, because an entry is stale only when none used it. An `ignore-dependencies` or `ignore-unresolved` entry is not judged when a rule that consults the list is off, and the dependency lists are not judged when a package has unknown entry files, because the rules skip such packages.

### Consequences

* Good, because a knip configuration translates key by key, and the places where skif differs (`*` and `{` stay globs, the marker on `entry`, the root `"."`) are stated.
* Good, because an unused ignore entry is visible and can fail a run.
* Bad, because a pattern such as `foo.*` written as a glob is read as a regular expression.
* Bad, because the dependency lists are not judged in a run where a package has unknown entry files.

### Confirmation

* Unit tests of the entry matcher cover slash-delimited and character-test regular expressions, flags, globs, the production marker and the record of matched entries.
* Tests of `skif-js` run a workspace per key: `ignore-unresolved`, `ignore-workspaces`, `paths`, `include-entry-exports` and `ignore-exports-used-in-file` per workspace, `ignore-members`, the production marker on `entry`, and `stale-js-config` with matched and unmatched entries.
* Tests of the configuration cover the new keys, the validation messages and the merge of `extend-ignore-unresolved`.
* `scripts/check-rule-ids.sh` passes with `stale-js-config`; `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Add the knip keys to `[js]` and `[js.workspaces]`, validate the static part when the configuration is checked, and report unused entries through a rule

* Good, because the same pattern language works in every list.
* Good, because unused entries follow the severity and `[[ignore]]` machinery of every other finding.
* Bad, because the rule depends on which other rules ran.

### Accept the keys but leave unused entries unreported

* Good, because it needs no tracking.
* Bad, because stale entries accumulate and hide that a tool changed.

### Report unused entries as configuration errors

* Good, because a stale entry cannot be missed.
* Bad, because whether an entry matches depends on the workspace, so the configuration would fail to load on a checkout that legitimately does not need it.

## More Information

* Amends: [ADR 0010](0010-js-analysis-on-oxc.md)
* See the [ADR index](index.md).
