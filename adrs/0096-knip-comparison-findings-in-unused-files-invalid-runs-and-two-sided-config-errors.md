---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0068"]
---

# The knip Comparison: Findings Inside Unused Files, Invalid knip Runs and Configs knip Cannot Load on Both Sides

## Context and Problem Statement

[ADR 0068](0068-what-the-knip-comparison-sets-apart.md) sets apart the knip-only findings skif does not judge
(`not-judged-by-skif`) and the knip-only findings of a package that holds a configuration knip could not load
(`knip-config-error`). Three kinds of difference remained that say nothing about whether skif is right.

1. A tool analyses nothing inside a file it reports as unused. When knip lists a file under `files` and skif does not,
   every export, type and member skif reports in that file is skif-only, and the reverse holds for a file only skif
   reports. The `files` difference is already counted once; the findings inside the file count it again, often many
   times.
2. On n8n, knip loads none of the configurations that import the workspace's own `@n8n/*-config` packages: an install
   without scripts leaves those packages without a built `dist`. Its report is then not a comparison of the two tools,
   but its counts sat next to the others as if they were.
3. `knip-config-error` covers only knip-only findings. A configuration knip could not load also leaves knip's view of
   the package incomplete, so a finding only skif reports there has the same cause.

What else does the comparison set apart, and how is a repository whose knip run is not a valid comparison recorded?

## Decision Drivers

* A difference is counted once, where it arises
* Every count says what the tools did; a repository whose knip run is not valid is visible as such, not dropped
* A known deviation names its cause, and the categories it applies to are listed

## Considered Options

* Set findings inside a file the other tool reports unused apart, mark invalid knip runs in `corpus.toml`, and apply `knip-config-error` to both sides
* Skip a repository whose knip run is not valid
* Leave the differences in the counts

## Decision Outcome

Chosen option: "Set findings inside a file the other tool reports unused apart, mark invalid knip runs in `corpus.toml`, and apply `knip-config-error` to both sides", because each set-apart finding has a cause already counted or recorded, and nothing is hidden: every deviation has its own column in `parity.md`.

**Findings inside unused files.** A finding only skif reports, in a file knip lists under `files`, is the
`in-knip-unused-file` known deviation; a finding only knip reports, in a file skif reports as unused, is
`in-skif-unused-file`. Unused files themselves and the manifest categories (`dependencies`, `devDependencies`,
`optionalPeerDependencies`, `binaries`) stay compared, since they do not depend on analysing the file.

**Invalid knip runs.** A repository whose knip run cannot be a valid comparison sets `knip-invalid` in `corpus.toml`
with the reason. It is still compared and recorded; its status cell in `parity.md` and the `invalid` field of its
baseline entry carry the reason, and its counts are left out of the totals, with a note naming it and the reason.
n8n is the one such repository.

**Configs knip cannot load.** `knip-config-error` applies to a finding only one tool reports in a package that holds
a configuration knip could not load: on knip's side because it ran without that configuration's entries, as ADR 0068
says, and on skif's because knip's view of the package is incomplete.

**Scope of `not-judged-by-skif`.** Until the harness reads the `scope` of an `incomplete-packages` entry
([ADR 0095](0095-untracked-files-member-manifest-errors-and-converging-fixes.md)), `not-judged-by-skif` for `files`,
`exports`, `types` and the member categories is an upper bound: a knip-only finding outside a narrowed scope is set
apart although skif judged it. Only the dependency categories are unjudged in every listed package.

### Consequences

* Good, because a disagreement about a file no longer inflates the counts of every other category.
* Good, because n8n's counts cannot be read as agreement, and its row still shows how the tools behave there.
* Good, because a finding caused by a configuration knip could not load is set apart whichever tool reports it.
* Bad, because a real skif bug inside a file knip reports unused is hidden in `in-knip-unused-file` until the `files`
  difference is resolved.
* Bad, because `knip-invalid` is set by hand; a repository whose knip run breaks later is not marked until someone
  adds the key.
* Bad, because the category set changes, so every baseline entry has to be recorded again before `--check` compares it.

### Confirmation

* The harness unit tests in `tools/bench` classify a skif-only export in a file knip lists unused as
  `in-knip-unused-file`, a knip-only export in a file skif lists unused as `in-skif-unused-file`, keep both tools'
  `files` findings compared, and classify a skif-only finding in a package with a failed knip configuration as
  `knip-config-error`.
* `n8n_is_marked_as_an_invalid_knip_comparison` reads the corpus and finds exactly one repository with `knip-invalid`.
* `parity.md` shows the reason in n8n's status cell and leaves it out of the totals.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Set findings inside a file the other tool reports unused apart, mark invalid knip runs in `corpus.toml`, and apply `knip-config-error` to both sides

* Good, because each set-apart finding has a recorded cause, and the comparison of the rest of the repository stays.
* Bad, because the categories hide real differences inside files the tools disagree on.

### Skip a repository whose knip run is not valid

* Good, because its counts can never be misread.
* Bad, because skif's behaviour on the repository is no longer recorded, and `--check` loses its drift check there.

### Leave the differences in the counts

* Good, because nothing is set apart.
* Bad, because one disagreement about a file shows up as dozens of differences, and n8n's thousands of one-sided
  findings drown the other repositories.

## More Information

* Amends: [ADR 0068](0068-what-the-knip-comparison-sets-apart.md) (`knip-config-error` applies to one-sided findings
  of either tool; the further deviations `in-knip-unused-file` and `in-skif-unused-file`; `knip-invalid`)
* Related decisions: [ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md),
  [ADR 0030](0030-harness-known-deviations-and-comparison-scope.md),
  [ADR 0053](0053-incomplete-packages-in-the-json-report.md),
  [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md),
  [ADR 0080](0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md)
* See the [ADR index](index.md).
