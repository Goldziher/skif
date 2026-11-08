---
status: accepted
date: 2026-11-07
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0012", "0030"]
---

# Incomplete Packages in the JSON Report: Naming What skif Did Not Judge, and a Harness Category for It

## Context and Problem Statement

When a tool config of a package cannot be read (it does not parse, spreads an imported base config, computes its entry
keys) or the package publishes build output that maps to no source file, the package's entry files are not all known.
skif then reports none of its files as unused and none of its dependencies as unused, because it cannot tell an unused
file from one reached through the unknown entries. `skif explain-usage` names the reasons for one file, but a lint run
says nothing: a package skif did not judge looks the same as a clean one. In the knip comparison
([ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md)) every unused file and dependency knip reports in such a
package counts as knip-only, so a deliberate abstention reads as disagreement and hides the real differences (GitHub
issue #27).

How does a lint report name the packages it did not judge, and how does the harness count findings in them?

## Decision Drivers

* A consumer must be able to tell "nothing is unused here" from "this was not checked"
* The JSON contract of [ADR 0012](0012-reporting.md) changes only additively, without a version bump
* Library crates do no I/O and the report data stays deterministic
* The harness must not count an abstention as agreement or as disagreement

## Considered Options

* An additive `incomplete-packages` array in the report, carried on `Report` and recorded by the provider, with a harness category fed by it
* An info diagnostic per incomplete package
* Have the harness recompute incomplete packages itself

## Decision Outcome

Chosen option: "An additive `incomplete-packages` array in the report, carried on `Report` and recorded by the provider, with a harness category fed by it", because it states a fact about the run where scripts already read, does not add findings that count against `--max-warnings` or show up in SARIF, and gives the harness the exact set skif used.

**Report data.** `skif_core::Report` gains `incomplete_packages: Vec<IncompletePackage>`, each with the package `name`
(its display name), its `dir` relative to the workspace root and its `reasons`. A reason is the `IncompleteEntries` of
explain-usage: a `cause` (`unparsable-config`, `unknown-config`, `unknown-spread`, `computed-entries`,
`unmapped-published`, `unspecified`), the `plugin` when a tool config is the cause, and the `file` relative to the
workspace root. The types move from `skif-js` to `skif-core` (re-exported from `skif-js` and the facade), since the
report is plain data and every crate that renders it must see them. A rule provider records a package with
`Emitter::report_incomplete_package`; `skif-lint` collects the records with the diagnostics and builds the report with
`Report::with_incomplete_packages`, which sorts by directory, then name. The JS provider records every analysed package
whose entries are incomplete, whenever it builds the analysis (any JS rule enabled). Packages in `ignore-packages` are
not analysed and are not listed.

**JSON.** The report document gains a top-level `incomplete-packages` array after `diagnostics`, omitted when empty:

```json
"incomplete-packages": [
  { "name": "web", "dir": "packages/web",
    "reasons": [ { "cause": "unknown-spread", "plugin": "vite", "file": "packages/web/vite.config.ts" } ] }
]
```

Keys are kebab-case. `dir` is `.` for the workspace root package; paths use `/` and are relative to the run's base, so in
a multi-root run each root's packages carry the root's prefix and all roots are merged into the one array in the same
order. Under the rule of [ADR 0012](0012-reporting.md) this is an added field, so `version` stays `1`; an `extra`
member of that name is rejected like the other reserved keys. Omitted-when-empty follows the `fix` and `roots` members,
so a run that judged everything produces the same document as before.

**Other formats.** The human renderer prints one dim line before the summary when the array is not empty:
`note: unused files and dependencies were not checked in N packages (a, b): their entry files are not all known;
`skif explain-usage <file>` says why`. SARIF and GitHub annotations do not carry it: neither has a place for a fact that
is not a finding on a line, and turning it into a result would count against code-scanning dashboards.

**Harness.** `skif-bench parity` reads `incomplete-packages` from skif's JSON. A finding only knip reports in `files`,
`dependencies` or `devDependencies` whose owning package (the deepest directory above the path that holds a
`package.json`, the root at the latest) is listed is the `not-judged-by-skif` category. It is recorded like a known
deviation of [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md): taken out of the one-sided counts,
counted in the baseline under `known_deviations`, ignored by `--check`, listed with its reason in the known-deviations
section. `parity.md` gives it its own column, "Not judged by skif", apart from the agreement counts and from the other
known deviations; `parity.json` lists the directories per repository under `not_judged_packages`. Exports, types and
members are still judged in such a package, so knip-only findings there stay differences.

### Consequences

* Good, because a script or editor can say which packages were not checked for unused files and dependencies, and why,
  from the lint run itself.
* Good, because the harness's knip-only counts no longer include skif's deliberate abstentions, and the abstentions
  are still counted and visible.
* Bad, because the committed parity baselines were recorded with those findings as knip-only; the next baseline refresh
  moves them into the new category, and `--check` against an old baseline reports the drop as drift until then.
* Bad, because owner lookup in the harness reads `package.json` presence on disk, an approximation of workspace
  membership; a directory with a `package.json` that is not a workspace package claims the files below it.

### Confirmation

* `skif-core` unit test: the report sorts incomplete packages by directory, serializes them kebab-case and omits the
  key when empty.
* `skif-report` insta snapshot `render__json_incomplete_packages` and a human-renderer test of the note line.
* A CLI test lints a workspace with an unreadable `vite.config.ts` in one package and asserts the JSON
  `incomplete-packages`, that only the other package's orphan file is reported, and the human note; a unit test of
  `roots.rs` checks multi-root prefixing.
* `skif-bench` unit tests cover reading the array, the owner lookup, the split into `not-judged-by-skif` and the
  summary column.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### An additive `incomplete-packages` array in the report, carried on `Report` and recorded by the provider, with a harness category fed by it

* Good, because it is one fact in one place, available to hosts through `Report` and to scripts through the JSON.
* Bad, because the `Emitter` carries a second kind of record besides diagnostics.

### An info diagnostic per incomplete package

* Good, because every format would show it without a new field.
* Bad, because it is not a problem in the repository: it would count in the summary, appear in SARIF, need a rule ID
  (a public contract) and be suppressible, which would hide the fact it exists to state.

### Have the harness recompute incomplete packages itself

* Good, because skif's output would not change.
* Bad, because the harness would have to reimplement plugin evaluation, and its answer could differ from what skif did.

## More Information

* Amends: [ADR 0012](0012-reporting.md) (the report keys), [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md) (a new knip category)
* Related decisions: [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md), [ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md), [ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md)
* See the [ADR index](index.md).
