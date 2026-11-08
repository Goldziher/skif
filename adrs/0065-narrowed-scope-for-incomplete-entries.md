---
status: accepted
date: 2026-11-08
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0035", "0053"]
---

# Narrowed Scope for Incomplete Entries: Only What an Unreadable Config Could Make an Entry Stays Unjudged

## Context and Problem Statement

[ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md) marks the entries
of a package incomplete when a tool config evaluates to an unknown value, spreads or merges in an unknown base, or
computes its entry keys, and [ADR 0053](0053-incomplete-packages-in-the-json-report.md) names such packages in the
report. skif then reports none of the package's files as unused. On the pinned corpus that hides real dead files:
cal-com's `apps/web/lib/cropImage.ts` and `lib/ensureArray.ts` sit behind a `next.config.ts` that exports
`(phase) => plugins.reduce(...)`, and n8n's `useParentScroll.ts` behind a Vite config that merges an imported base.
80 of n8n's 98 packages and 6 of cal-com's 114 are incomplete (GitHub issue #23).

An unreadable config cannot name any file, though: Next.js takes its entries from its router, a test runner's config
names tests and setup modules, and a base config shared between packages names no file of one of them. How much of an
incomplete package should stay unjudged?

## Decision Drivers

* A finding that depends on something the analysis could not read is a false positive
* A dead file should not be hidden because an unrelated config is unreadable
* The new findings must be mostly right: at least about 80% on the corpus
* When skif cannot bound what a config hides, it keeps the whole package unjudged

## Considered Options

* A narrowed scope per plugin, whose files are treated as entries
* Keep the package-wide suppression
* Report every unreached file and leave the unknown entries to the user

## Decision Outcome

Chosen option: "A narrowed scope per plugin, whose files are treated as entries", because it reports the dead files
the issue names, keeps the new findings above the bar on both corpora, and falls back to the old behaviour wherever a
plugin cannot say where its entries lie.

**The scope.** `Plugin::incomplete_scope(cause, config)` returns where the entries an incomplete config hides can lie:
`IncompleteScope::Package` (the default) or `IncompleteScope::Within(globs)`, relative to the workspace directory. It
is asked for an unknown value, an unknown spread and computed entry keys; a config that does not parse is always
`Package`. Each reason carries its scope, and the scopes of a package's reasons are joined: one `Package` makes the
whole package unjudged. A published entry that maps to no source file (`unmapped-published`) and an input source that
gives no reason stay `Package`, since publishing may expose any file.

| Plugin | Scope, besides the plugin's own entry globs |
| --- | --- |
| `next` | `{,src/}{app,pages}/**` and `{,src/}{instrumentation,instrumentation-client,middleware,proxy}.*`, since an unknown `pageExtensions` changes which router files are routes |
| `vite` | `{,src/}{index,main}.*`, the conventional build input a shared base may set; `Package` when the config computes `root`, `build.rollupOptions.input` or `build.lib.entry` itself or is unknown as a whole |
| `vitest` | test files, test and fixture directories, files named after `setup` or `teardown`, and `*.{test,spec,bench,e2e,integration}*`; `Package` when the config computes `root`, `test.include`, `test.setupFiles` or `test.globalSetup` itself (also in an inline project) or is unknown as a whole |
| `playwright` | the same test conventions; `Package` when the config is unknown as a whole |

A config's own computed key is told from one a spread may replace by evaluating the key with the unknown spreads
removed. A spread or merged base is shared between packages, so it can only name files where the tool looks by
convention; a key the config computes itself, or a config unknown as a whole (a factory called with the package's own
options, such as `include: ['src/**/*.check.ts']`), may name any file of the package, so both are `Package`.

**Scoped files are entries.** The project files of the package that match the scope are entry files of the module
graph, so the files they import are not reported either; every other file of the package is judged by `unused-files`
as in a complete package. Unused dependencies, exports, types and members of an incomplete package are still not
judged, since an unknown entry may use any of them.

**Computed loads widen the scope.** A module of the package with an `import()` or `require()` whose argument is not
static may load any file of it, which the scope does not bound. The scope is then dropped and the package is unjudged
as a whole, as before.

**Reporting.** The package stays in `incomplete-packages` with the same reasons; the JSON report does not change. The
human note reads "unused files and dependencies were not fully checked". `explain-usage` names the case: a file in the
scope is an entry of kind `incomplete-scope` with the scope and the reasons, an unused file outside it carries the
scope in `incomplete-scope`, and a package unjudged as a whole keeps the absence `entries-incomplete`, with
`widened-by` naming the computed load when one dropped the scope.

### Consequences

* Good, because n8n's `unused-files` findings go from 43 to 123 and cal-com's from 282 to 377, with the three dead
  files of the issue among them; on samples of the new findings, 67 of n8n's 80 and 90 of cal-com's 95 are dead.
* Good, because a plugin without a scope keeps the old, sound behaviour.
* Bad, because the scope is a judgement of where a tool looks, not a proof: a base config that names an unconventional
  file of the package makes it a false positive.
* Bad, because the false positives left are gaps that a complete package has too, now visible in packages that were
  hidden: files loaded by URL from `public/`, `postcss` configs of a Vite package that does not list `postcss`, alias
  targets that only dependencies import, and build output of another package imported by its `dist` path.
* Bad, because `incomplete-packages` lists a narrowed package like a package-wide one, so the knip harness of ADR 0053
  still counts every knip-only file there as not judged.

### Confirmation

* `crates/skif-js/tests/fixtures.rs` `incomplete_configs_narrow_what_is_not_judged` lints the `incomplete-scopes`
  fixture (one package per scoped plugin, one with a computed `import()`, one with an unparsable config) and snapshots
  the diagnostics; before this change it reports no unused file.
* `crates/skif-js-plugins/tests/fp_config.rs` checks the scope of each plugin and the cases that stay `Package`.
* `crates/skif-js/tests/explain_absence.rs` checks the three `explain-usage` cases.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A narrowed scope per plugin, whose files are treated as entries

* Good, because dead files outside what a tool reads are reported.
* Good, because treating the scope as entries keeps the files they import from being reported.
* Bad, because each plugin needs its own judgement of where it looks.

### Keep the package-wide suppression

* Good, because it never reports a file an unknown entry reaches.
* Bad, because one unreadable test config hides every dead file of its package, most of n8n.

### Report every unreached file and leave the unknown entries to the user

* Good, because nothing is hidden.
* Bad, because the entries a framework config does hide (Next.js routes under a custom extension, a test setup
  module) become false positives.

## More Information

* Amends: [ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md) (an
  unreadable config no longer hides every unused file of its package),
  [ADR 0053](0053-incomplete-packages-in-the-json-report.md) (the human note and the meaning of a listed package)
* Related decisions: [ADR 0011](0011-plugin-model.md), [ADR 0044](0044-monorepo-js-corrections-builtins-html-entries-referenced-paths-extglob-helpers-and-stylesheets.md)
* See the [ADR index](index.md).
