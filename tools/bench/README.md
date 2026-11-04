# skif-bench

Benchmark harness for skif. It is an internal workspace member (`publish = false`). The `skif-bench` command treats the
skif binary as an external command; the crate also exposes its synthetic monorepo generator as a library
(`skif_bench::generate`) so the in-process criterion benches can build workspaces in memory.

## What it measures

`skif-bench run` times full CLI invocations with [hyperfine](https://github.com/sharkdp/hyperfine) (`-N`, no shell,
with `--warmup` and `--runs`) on two kinds of targets:

| Target | Contents | Tools timed |
| --- | --- | --- |
| Generated workspaces | 50, 500 and 2000 packages (`--sizes`), pnpm layout, 5 % issue rate, seed 1 | `skif --no-js`, `sherif`, `syncpack lint` |
| Module graph | 10,000 TypeScript files in 100 packages (`--graph-files`, `--graph-packages`; `generate --source-files-per-package`) | `skif` |
| Pinned corpus | the `bench` repositories of `corpus.toml` | `skif`, `skif --no-js`, `sherif`, `syncpack lint`, `knip --reporter json` |

`sherif`, `syncpack` and `knip` are optional: a tool that is not on `PATH` is skipped with a note. Results go to
`target/bench-results/` (or `--out`): `summary.md` (tables plus the ADR checks), `summary.json` (schema 1: skif
version, host, hyperfine version, every target with mean, stddev, median, min and max in ms, and the checks), and one raw
hyperfine export per target under `raw/`. `run` exits non-zero when an ADR 0001 check fails, after writing the
summary; `--no-enforce` turns that off.

## Targets (ADR 0001)

| Workload | Target |
| --- | --- |
| Manifest and policy rules, 500 packages | < 50 ms wall clock |
| JS analysis on the corpus repositories | at least 5x faster than `knip` on the same tree |
| Parse and build the module graph, 10,000 files | < 2 s |

`summary.md` ends with a pass/fail line for each. The graph case times a whole `skif` run, which parses the files,
builds the graph and runs every JS rule, so it is an upper bound on the parse-and-graph time; the `js_analysis`
criterion bench tracks the in-process part. The `skif` row on the corpus includes JS analysis and the `skif --no-js`
row does not, so the difference is the cost of the analysis; the knip comparison needs the corpus checkouts to have
their dependencies installed.

## Running

```sh
mise run bench            # build release skif, fetch the corpus, run everything
mise run bench:micro      # all criterion benchmarks (see "In-process benchmarks")
mise run bench:corpus     # only fetch the timed corpus repositories
mise run bench:publish    # run, write tools/bench/results and refresh the numbers table below
mise run corpus:fetch     # fetch the parity corpus (all pinned repositories)
mise run parity           # findings parity with knip (see "Parity with knip")
mise run diff:syncpack    # policy diff against syncpack (see "Policy diff against syncpack")
mise run corpus:check     # both of the above against the committed baselines
mise run bench:generate -- --packages 500 --out target/bench-generated/demo
```

Directly:

```sh
cargo run --release -p skif-bench -- generate --packages 500 --out DIR [--seed S] [--pm pnpm|npm|yarn|bun] [--catalog] [--mismatch-rate F] [--force]
cargo run --release -p skif-bench -- corpus fetch [--suite bench|parity] [--only NAME,...] [--file corpus.toml] [--dir target/bench-corpus]
cargo run --release -p skif-bench -- run [--skif PATH] [--sizes 50,500,2000] [--graph-files 10000] [--pm pnpm] [--no-corpus] [--warmup 3] [--runs 10] [--skif-arg ARG]... [--no-enforce]
cargo run --release -p skif-bench -- parity [--skif PATH] [--only NAME,...] [--install] [--recursive] [--repo-knip-config] [--include-test-files] [--check | --write-baseline]
cargo run --release -p skif-bench -- syncpack-diff [--skif PATH] [--only NAME,...] [--no-fixtures] [--no-corpus] [--check | --write-baseline]
```

`generate` is deterministic for a seed: the same options produce byte-identical files. It writes a root
`package.json`, a lockfile stub (and `pnpm-workspace.yaml` for pnpm) so package manager detection works, and
packages under `apps/*`, `packages/*` and `packages/group/*`. Dependencies are drawn from a fixed pool of about 320
real npm names with a per-name specifier (caret, tilde, exact, `npm:` alias, a few `github:`, `file:` and `link:`),
plus `workspace:*` links between packages. `--source-files-per-package N` adds `N` TypeScript modules under `src/` of every package: a chain of modules in which each imports its predecessor, ending in the `src/index.ts` that the manifests name as `main`. `--mismatch-rate` is the probability, per dependency or package, of each
injected issue: a version mismatch, unsorted `dependencies`, an `@types/*` package in `dependencies`, an empty
`optionalDependencies`. The command prints the injected counts as JSON.

Profile with `cargo build --profile profiling` (release plus debug symbols, no stripping).

## Parity with knip

`skif-bench parity` runs, for each repository of the `parity` suite in `corpus.toml`, `skif . --format json` and
`npx knip@<version pinned in corpus.toml> --reporter json` in the checkout, and reduces both reports to finding keys
`<file>|<name>` within a knip issue type:

| knip issue type | skif rule |
| --- | --- |
| `dependencies` | `unused-dependencies` |
| `devDependencies` | `unused-dev-dependencies` |
| `optionalPeerDependencies` | `unused-optional-peer-dependencies` |
| `unlisted` | `unlisted-dependencies` |
| `binaries` | `unlisted-binaries` |
| `unresolved` | `unresolved-imports` |
| `files` | `unused-files` |
| `exports` | `unused-exports` |
| `types` | `unused-types` |
| `enumMembers` | `unused-enum-members` |
| `namespaceMembers` (older knip: `nsExports`, `nsTypes`) | `unused-namespace-members` |
| `duplicates` | `duplicate-exports` |
| `classMembers`, `catalog`, `catalogReferences` | not compared (no skif rule) |

For each repository and issue type the report gives the number of findings only skif reports, only knip reports, and
both report. The keys are approximations (a duplicate export is keyed by its names, an unused export by file and
name), so the numbers measure drift and where the tools model things differently, not correctness. The full report
with a few sample keys per side is `target/bench-results/parity.json` and `parity.md`; samples name files of the
corpus repositories and are never committed.

**Dependencies.** knip resolves imports and detects used binaries through `node_modules`, and without them reports
nearly every import as unresolved or unlisted. `--install` installs each repository first with its package manager and
`--ignore-scripts` (`pnpm install --frozen-lockfile`, `yarn install --immutable --mode=skip-build` or
`--frozen-lockfile`, `npm ci`, `bun install --frozen-lockfile`); the installed state is cached in the checkout. A
repository is skipped, with the reason in the report, when its package manager is not on `PATH`, the install fails or
times out (`install = false` in `corpus.toml` for Rush, which has no lockfile in the tree), skif fails, or knip prints
no JSON. Without `--install` the run is quick but dominated by that noise; the baseline records which way it was made and
`--check` rejects a mismatch. A repository may set `install-command` (program first; `rules-js` installs with
`npx pnpm@9`) and `install-timeout-secs` (`vercel-examples`) in `corpus.toml`.

**Scope.** knip runs with an empty configuration (`--config` to a `{}` file in the scratch directory), so a repository's
own ignores and disabled issue types do not hide findings; `--repo-knip-config` uses the repository's own. skif runs with
`--recursive` on repositories that set `recursive = true` in `corpus.toml` (nested roots that are not workspace members),
or on all with `--recursive`; paths stay relative to the start directory, as in knip's report. Enum and namespace
members are keyed by the member name on both sides. knip's stdout may carry banners around its JSON; the first object
that is a knip report is used.

**Known deviations.** In `files`, a file only knip reports is not counted as a difference when it is outside skif's
project set (`non-code-file`), in a fixture directory (`fixture-file`) or a test file (`test-file`); those are reported
in their own column and section, with the reason, and `--check` ignores them. `--include-test-files` counts the last
two as differences (not usable with a baseline, nor is `--repo-knip-config`). See
[ADR 0030](../../adrs/0030-harness-known-deviations-and-comparison-scope.md).

## Policy diff against syncpack

`skif-bench syncpack-diff` compares skif's policy rules (`version-mismatch`, `semver-range-mismatch`,
`banned-dependency`, `invalid-local-version`, `policy-conflict`, `unresolved-catalog-references` and the catalog rules) with `syncpack@<pinned> json`.
syncpack has no JSON lint reporter, but `json` prints every dependency instance with its status code, and skif's
diagnostic `detail` carries the same status code names (`DiffersToHighestOrLowestSemver`, `IsBanned`, ...). A finding is
keyed by status code, `package.json` path and dependency. Two targets:

- **`fixtures`**: the cases under `fixtures/policy`. The `skif.toml` of each case is translated to a syncpack
  configuration (version groups: banned, pinned, snapped-to, same-range, same-minor, ignored, preferred-semver with
  `prefer`; semver groups with `range`; `[policy] prefer` and `dependency-types`). A case that uses another option
  (catalogs, the workspace protocol, dependency families, rules, ignores) has no syncpack equivalent and is skipped; the
  report counts the skipped cases and names the cases whose findings differ.
- **Every `parity` repository**, with the default skif configuration and an empty syncpack configuration (skif ignores
  syncpack settings, and a legacy `syncpack` key in a `package.json` breaks syncpack 15). syncpack reads
  `--dependency-types prod,dev,local`, which is skif's default scope plus the own-version instance; for a `rush.json`
  repository every `projectFolder` is passed as a `--source`, since syncpack has no Rush support.

**Known deviations.** A one-sided finding that the manifests show to be an intended difference (`satisfied-local-range`,
`missing-local-version`, `floating-specifier`, `shorthand-ranking`, `npm-alias`, `unread-by-syncpack`) is taken out of
the counts, reported per target in its own column and section with the reason, and ignored by `--check`; what stays
one-sided is unexplained. See [ADR 0030](../../adrs/0030-harness-known-deviations-and-comparison-scope.md).

## Baselines and the drift check

`baselines/parity.json` and `baselines/syncpack.json` hold the counts of a run (skif only, tool only, both, per issue
type or status code and repository, plus the case summary of the fixtures) and the tool version and install mode they
were recorded with. They contain no file, package or symbol names. After a deliberate change, or a bump of a pin in
`corpus.toml`, refresh them:

```sh
mise run parity -- --install --write-baseline
mise run diff:syncpack -- --write-baseline
```

`--check` compares a run with the baseline and exits non-zero when a count moves by more than
`--tolerance-percent` (default 10) of its baseline value or `--tolerance-abs` (default 5), whichever is larger, when a
repository changes between compared and skipped, or when a fixture that agreed before now differs. A run narrowed with
`--only` is checked against those entries alone, and `--write-baseline --only` updates only them. The scheduled
`corpus` workflow (`.github/workflows/corpus.yml`, weekly and on demand, never on pull requests) runs both checks.
The decision is [ADR 0026](../../adrs/0026-parity-and-diff-tasks-on-a-pinned-corpus.md).

## Measured numbers

`mise run bench:publish` writes `tools/bench/results/summary.{json,md}` and rewrites this section with
`refresh-numbers.mjs`. Until it has been run on a reference machine the section holds placeholders.

<!-- bench:numbers:start -->
Not measured yet: run `mise run bench:publish`.
<!-- bench:numbers:end -->

## In-process benchmarks

`mise run bench:micro` runs every criterion benchmark (`cargo bench --workspace`). They run in one process on a
`MemoryFs`, so they measure the library, not process start-up or disk. Run one bench with
`cargo bench -p <crate> --bench <name> -- <filter>`; `--features fix` is needed for the `skif` `fix` bench
(`cargo bench -p skif --features fix --bench fix`).

Generated workspaces use seed 1, pnpm layout and a 5 % issue rate, the same as the hyperfine targets.

| Bench target | Benchmark id | Measures |
| --- | --- | --- |
| `skif` / `pipeline` | `manifest/parse_500` | `Manifest::parse` over 500 generated manifests |
| | `discover/{50,500,2000}` | `Workspace::discover_with_fs` |
| | `policy/{plain,semver_groups,same_range}/{500,2000}` | only the policy provider (version, semver and `same-range` groups) |
| | `target/manifest_policy_{50,500,2000}` | `Linter::run` with the manifest, policy and catalog providers; `manifest_policy_500` is the ADR 0001 budget (< 50 ms) |
| | `suppressions/ignore_entries/{0,10,50}` | a 500-package run with that many non-matching `[[ignore]]` entries |
| `skif` / `fix` (feature `fix`) | `fix/apply_fixes/{1,10,100}` | `apply_fixes` planning N fixes against one 200-dependency `package.json` |
| `skif-core` / `core` | `Report/new_{10k,50k}` | `Report::new` (sort and summary) over that many diagnostics |
| `skif-core` / `core` | `LineIndex/*` | line and column lookup |
| `skif-js-syntax` / `parse_module` | `parse_module/corpus/{full,facts_only}` | `parse_module` over the files in `benches/corpus` (TS, TSX, JS); `facts_only` turns off in-file references and member tracking |
| | `parse_module/file/<name>` | one corpus file at a time |
| `skif-js` / `analysis` | `js_analysis/files/{210,2100}` | the JS provider (module graph and all JS rules) over 10 and 100 generated packages of 21 files each |
| `skif-specifier`, `skif-edit` | see the bench sources | specifier parsing and format-preserving JSON edits |

The `target/` group covers the ADR 0001 manifest and policy budget. The JS analysis speed target (at least 5x faster than
`knip`) is only measurable end to end with hyperfine on the corpus; `js_analysis` tracks regressions of the in-process
part.

## Caveats

- **Warm cache.** hyperfine warms up before measuring, so numbers are warm file-cache, not cold start. Compare
  cold runs with `hyperfine --prepare` and a cache purge by hand if that matters.
- **Comparison tools are measured as installed.** Versions are whatever is on `PATH`, and their output formats and
  defaults differ; `sherif` and `syncpack` only check manifests, `knip` analyses code. The summary records only the
  hyperfine version, so note tool versions when sharing numbers.
- **knip needs installed dependencies** to give realistic results; the corpus is cloned, not installed, so run
  `pnpm install` (or the matching package manager) in a corpus checkout before timing knip, or use
  `parity --install` for the findings comparison.
- Non-zero exits are ignored (`--ignore-failure`): linters exit 1 when they find something.
- The skif command line is `skif . --format json --no-install [--no-js]`; the working directory is the target.
- Wall-clock times include process start-up. For sub-10 ms runs hyperfine warns about short commands; trust the
  mean over many `--runs`.
- The corpus pins live in `corpus.toml` (full commit SHAs, the knip and syncpack versions, the suites of each
  repository). Repositories are fetched shallow with `git fetch --depth 1 origin <sha>` (plus a cone sparse checkout
  where `sparse` is set), so `fetch` is idempotent and cheap to repeat. Bump a pin deliberately and refresh the
  baselines in the same change.
