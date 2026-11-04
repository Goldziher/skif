---
status: accepted
date: 2026-11-04
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0001", "0006", "0009", "0010", "0011", "0019"]
---

# Parity and Diff Tasks on a Pinned Corpus: Findings Against knip and syncpack, With Committed Baselines

## Context and Problem Statement

[ADR 0001](0001-mission-and-scope.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md) and [ADR 0011](0011-plugin-model.md) promise `mise run parity` and `mise run diff:syncpack`, so that drift from knip and syncpack is a number and not an assumption. [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md) found that neither task existed and withdrew the promise: parity became "a number reported by `mise run bench`", and nothing was measured. Since then skif has been run on eleven public repositories, and the findings that matter most (unused files, exports and types, unlisted dependencies) are exactly the ones whose agreement with knip is unknown. The speed commitment of ADR 0001 has the same gap: the benchmark prints a pass or fail line and exits 0, and the 10,000-file module graph target is only tracked by a criterion bench.

## Decision Drivers

* A claim of knip-style analysis and syncpack semantics must come with a measured agreement
* The measurement must be reproducible: pinned repositories, pinned tool versions, counts committed
* A regression of agreement or speed must fail a job, but not block pull requests on network access and installs
* Baselines in the repository must not contain names or paths from third-party sources

## Considered Options

* Restore the tasks as a corpus comparison with committed count baselines and a scheduled drift check
* Keep ADR 0019 as it stands and report agreement ad hoc
* Make parity a release gate

## Decision Outcome

Chosen option: "Restore the tasks as a corpus comparison with committed count baselines and a scheduled drift check", because it gives the numbers the earlier records promised at the cost of one scheduled job, and avoids the release gate that ADR 0019 rightly withdrew.

**Corpus.** `tools/bench/corpus.toml` pins every repository to a full commit SHA and names the pinned versions of `knip` and `syncpack` that the comparisons run. Each repository belongs to the `bench` suite (timed with hyperfine), the `parity` suite (compared with knip and syncpack), or both, and records its package manager and why it is there. `skif-bench corpus fetch` fetches a suite shallowly at the pinned commit; a repository may list `sparse` directories where a full checkout is too large. `mise run corpus:fetch` runs it.

**`mise run parity`.** For each `parity` repository `skif-bench parity` runs `skif --format json` and `npx knip@<pinned> --reporter json` in the checkout and maps both reports to a common key, `<file>|<name>` per knip issue type: `dependencies`, `devDependencies`, `optionalPeerDependencies`, `unlisted`, `binaries`, `unresolved`, `files`, `exports`, `types`, `enumMembers`, `namespaceMembers` and `duplicates`. Per repository and issue type it reports how many findings are only skif's, only knip's, and in both. `classMembers` has no skif rule and is not compared. knip needs installed dependencies to resolve imports, so `--install` installs them first with the repository's package manager and no lifecycle scripts; a repository that cannot be installed, or on which either tool fails, is reported as skipped with the reason and not silently dropped. The number is a measure of drift: the tools model some things differently, and the baseline records what the difference is, not that it is zero.

**`mise run diff:syncpack`.** `skif-bench syncpack-diff` runs skif's policy rules and `syncpack@<pinned> json` on every policy fixture and every `parity` repository. A finding is keyed by syncpack's status code (which skif already reports as the diagnostic `detail`, [ADR 0009](0009-version-and-catalog-policies.md)), the `package.json` and the dependency. A fixture's `skif.toml` is translated to a syncpack configuration for the options both tools share (version groups, semver groups, `prefer` and `dependency-types`); a fixture that uses an option without a syncpack equivalent is skipped and counted. The report names the fixtures whose findings differ.

**Baselines and drift.** `tools/bench/baselines/parity.json` and `syncpack.json` hold counts and nothing else: no file, package or symbol names, which stay in the full report under `target/bench-results`. `--write-baseline` records a run, `--check` fails when a count moves by more than a tolerance (10 % of the baseline value or 5, whichever is larger) or when a repository changes status, and a fixture that did not differ before and now does fails regardless of tolerance. A baseline records the tool version and whether dependencies were installed, and a run made another way is rejected. The `corpus` workflow (`.github/workflows/corpus.yml`) runs both checks weekly and on demand; it does not run on pull requests, because it needs the network and installs.

**Speed.** `skif-bench run` exits non-zero when a target check of ADR 0001 fails, after writing its summary. The module graph target gets its own case: a generated workspace of 10,000 TypeScript files in 100 packages, measured as a whole `skif` run, which includes every JS rule and therefore bounds the parse-and-graph time from above. Results are written as `summary.json` and `summary.md`; `mise run bench:publish` writes them to `tools/bench/results` and refreshes the table in `tools/bench/README.md`.

**Not a release gate.** As in ADR 0019, no release depends on parity. The drift check is advisory for maintainers: a failing run is a reason to look, and the baseline is refreshed when a difference is understood and accepted.

### Consequences

* Good, because the agreement with knip and syncpack is measured, versioned and visible in review when it changes.
* Good, because a speed regression against ADR 0001 fails the benchmark run instead of printing a line.
* Good, because the committed files contain counts only.
* Bad, because the comparison is only as good as the key mapping: finding keys are approximations (a duplicate export is keyed by its names, an unused export by file and name), and knip's own results depend on its plugin configuration loading, which differs between a clean checkout and a repository's CI.
* Bad, because `--install` makes the scheduled job slow and dependent on the registries, and a repository whose install fails drops out of the comparison until the cause is fixed.
* Bad, because the pinned tools and repositories need an occasional deliberate bump, with a new baseline.

### Confirmation

* The `skif-bench` unit tests cover the corpus manifest and its suites, the key mapping of both knip and syncpack reports, the `skif.toml` to syncpack translation and the drift check.
* `mise run parity` and `mise run diff:syncpack` print the per-repository tables; `--check` exits non-zero on drift.
* `mise run bench` exits non-zero when a check of ADR 0001 fails.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Restore the tasks as a corpus comparison with committed count baselines and a scheduled drift check

* Good, because it delivers what ADR 0001, ADR 0009, ADR 0010 and ADR 0011 describe.
* Bad, because it adds a job to keep green.

### Keep ADR 0019 as it stands and report agreement ad hoc

* Good, because it needs no work.
* Bad, because agreement is then known only when someone happens to measure it, and no regression is caught.

### Make parity a release gate

* Good, because drift could not ship.
* Bad, because the two tools disagree with skif by design in places, so the gate would need exceptions as large as the baseline, and a network failure would block a release.

## More Information

* Amends: [ADR 0019](0019-reality-reconciliation-crate-list-schema-file-test-tooling-and-consistency.md) (the `parity` and `diff:syncpack` tasks exist again; parity stays outside the release gate), [ADR 0001](0001-mission-and-scope.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md) and [ADR 0011](0011-plugin-model.md) (the tasks they name; the corpus is `tools/bench/corpus.toml`, not `scripts/corpus.toml`)
* Related decisions: [ADR 0015](0015-attribution-and-vendoring.md)
* See the [ADR index](index.md).
