---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0065"]
---

# Exports Outside a Narrowed Incomplete Scope Are Judged, and esbuild Build Scripts Are Read

## Context and Problem Statement

[ADR 0065](0065-narrowed-scope-for-incomplete-entries.md) lets `unused-files` judge an incomplete package outside the
narrowed scope of its configs, but keeps `unused-dependencies`, `unused-dev-dependencies`, `unused-exports`,
`unused-types`, the namespace export rules and the member rules silent in the whole package, "since an unknown entry
may use any of them". knip reports there, so these packages are where skif cannot replace it (GitHub issue #101). On
the pinned corpus 80 of n8n's 112 packages are not judged, 38 of vscode's 139 and 37 of next-js's 671; knip finds
1942 unused exports and 1245 unused types in n8n's, and 1015 exports, 715 types and 442 members in vscode's.

Two causes dominate. Most n8n packages merge a shared Vitest base, so their scope is narrowed, yet their exports are
not judged. 30 vscode extensions build with an `esbuild.mts` that exports nothing and calls a shared helper:

```ts
const srcDir = path.join(import.meta.dirname, 'src');
run({ platform: 'node', entryPoints: { main: path.join(srcDir, 'main.ts') }, srcDir }, process.argv);
```

The evaluator finds no exported config, so the config counts as unparsable and the package is unjudged as a whole.
What else can be judged without reporting what an unknown entry uses?

## Decision Drivers

* A finding that depends on something the analysis could not read is a false positive
* A false unused dependency is the costliest finding: it is removed and breaks a build
* Narrow by what the analysis already establishes, not by a new guess of where tools look
* Read the config shapes behind the most packages not judged

## Considered Options

* Judge the exports outside a narrowed scope, read esbuild build scripts, and keep dependencies unjudged
* Also judge dependencies a narrowed package's configs cannot name
* Treat every file of a package unjudged as a whole as an entry

## Decision Outcome

Chosen option: "Judge the exports outside a narrowed scope, read esbuild build scripts, and keep dependencies
unjudged", because both narrowings follow from facts the analysis has, and the dependency narrowing would rest on
string matching against configs skif often cannot read.

**Exports outside the scope.** Every project file in the narrowed scope of an incomplete package is an entry of the
module graph (ADR 0065), so every import an unknown entry can make is already counted. `JsAnalysis::judges_exports_of`
is true for a module of a complete package, false for a module of a package unjudged as a whole (no scope, an
unparsable config, `unmapped-published`, or a computed `import()` that widened the scope), and otherwise true exactly
when the module lies outside the scope. `unused-exports`, `unused-types`, `unused-namespace-exports`,
`unused-namespace-types`, `unused-enum-members` and `unused-namespace-members` use it through
`reports_exports_of`. A module in the scope may itself be an unknown entry whose exports the tool reads, so its
exports stay unjudged even with `include-entry-exports`. `explain-usage` sets `entries-incomplete` and the verdict
`not-judged` only for an export the rules do not judge.

**esbuild build scripts.** esbuild has no config file format, so `esbuild.config.*` and `esbuild.*` are scripts that
call its API. A new `Plugin::read_script` hook gives the value of a config that parses but exports nothing
recognised; the default keeps it unparsable. The esbuild plugin returns one build per `entryPoints` property the
script writes anywhere (`extract_property_values`, evaluated against the script's top-level bindings), so
`entryPoints` of a top-level `run({ ... })` and of a `build({ ... })` inside a function are both read; a computed
value leaves the entries incomplete (`computed-entries`), and a script that writes no `entryPoints` stays
unparsable.

**Dependencies stay unjudged** in every incomplete package. An unread base config names packages by string
(`environment: 'jsdom'`, coverage provider `'v8'` for `@vitest/coverage-v8`, reporters, setup modules), often from an
npm or workspace package the evaluator does not reach, and those packages are declared by the consuming package.

**Reporting.** The JSON report does not change: a narrowed package is listed under `incomplete-packages` as before.

### Consequences

* Good, because on the corpus at least 938 exports, 581 types and 11 members of n8n and 128 exports and 87 types of
  mastra that knip reports are now judged (up to 1841, 1199 and 16 in n8n when every Vite or Vitest package with an
  unknown base is narrowed).
* Good, because up to 30 vscode extensions become fully judged; knip reports 52 dependencies, 269 files, 177 exports,
  107 types and 8 members there.
* Good, because the dependency rules keep their current, sound behaviour.
* Bad, because an alias a shared base declares (`resolve.alias` in an unknown spread) is unknown: an export used only
  through it is reported, as ADR 0065 already accepts for files.
* Bad, because an esbuild script that passes options built elsewhere (`build(options)`) next to one that writes
  `entryPoints` loses the entries of the former.
* Bad, because the knip harness of ADR 0068 still sets apart every knip-only export, type and member of a listed
  package as `not-judged-by-skif`, so the comparison does not show the gain until the report or the harness tells a
  narrowed package from one unjudged as a whole.

### Confirmation

* `crates/skif-js/tests/fixtures.rs` `exports_outside_a_narrowed_incomplete_scope_are_judged` lints the
  `incomplete-judged` fixture: the unused export, type and enum member outside the scope are reported, the export of
  the scope's setup module and of a package with a config unknown as a whole are not. Before this change it reports
  none.
* `crates/skif-js/tests/explain_absence.rs` `an_export_outside_the_narrowed_scope_is_judged` checks the verdicts.
* `crates/skif-js-plugins/tests/fp_config.rs` `esbuild_build_scripts_name_their_entry_points` reads the helper-call and
  in-function shapes and keeps computed and absent `entryPoints` incomplete.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Judge the exports outside a narrowed scope, read esbuild build scripts, and keep dependencies unjudged

* Good, because each narrowing is bounded by what the graph and the script already say.
* Bad, because n8n's packages whose Vitest config is a factory call stay unjudged as a whole.

### Also judge dependencies a narrowed package's configs cannot name

A dependency would be judged when no config of a reason, nor any module it imports, mentions its name or a short
form of it, and it is not a package of the tool.

* Good, because about 50 more knip findings in narrowed packages would be judged.
* Bad, because a base imported from an installed package is not read, and short names follow per-tool conventions,
  so a miss reports a dependency the tool loads.

### Treat every file of a package unjudged as a whole as an entry

* Good, because exports and dependencies of such packages could be judged as well.
* Bad, because dead files would become entries, and their imports would surface as unlisted dependencies and
  unresolved imports that knip does not report.

## More Information

* Amends: [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md) (exports, types and members of a narrowed package
  are judged outside the scope; an esbuild script with `entryPoints` is not unparsable)
* Related decisions: [ADR 0053](0053-incomplete-packages-in-the-json-report.md),
  [ADR 0068](0068-what-the-knip-comparison-sets-apart.md),
  [ADR 0069](0069-a-published-path-into-nothing-is-reported-not-an-incomplete-entry.md)
* See the [ADR index](index.md).
