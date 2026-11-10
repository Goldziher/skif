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
version, host, hyperfine version, every target with mean, stddev, median, min and max in ms and the number of runs that exited with an
error, and the checks), and one raw
hyperfine export per target under `raw/`. `run` exits non-zero when an ADR 0001 check fails, after writing the
summary; `--no-enforce` turns that off. A tool that exits with a status above 1 (knip's exit code when it fails or cannot
load a configuration file) is noted under its table and in the knip check, since its time may be that of a partial run.
A corpus repository's `bench-knip-args` in `corpus.toml` are passed to knip, so it reads the files skif reads (npm-cli
sets `--no-gitignore`). hyperfine 1.x and 2.x exports are both read.

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
mise run bench:deadcode   # dead-code precision and recall against an answer key (see "Dead-code precision and recall")
mise run bench:generate -- --packages 500 --out target/bench-generated/demo
```

Directly:

```sh
cargo run --release -p skif-bench -- generate --packages 500 --out DIR [--seed S] [--pm pnpm|npm|yarn|bun] [--catalog] [--mismatch-rate F] [--force]
cargo run --release -p skif-bench -- corpus fetch [--suite bench|parity] [--only NAME,...] [--file corpus.toml] [--dir target/bench-corpus]
cargo run --release -p skif-bench -- run [--skif PATH] [--sizes 50,500,2000] [--graph-files 10000] [--pm pnpm] [--no-corpus] [--warmup 3] [--runs 10] [--skif-arg ARG]... [--no-enforce] [--install]
cargo run --release -p skif-bench -- parity [--skif PATH] [--only NAME,...] [--install] [--recursive] [--repo-knip-config] [--include-test-files] [--check | --write-baseline]
cargo run --release -p skif-bench -- syncpack-diff [--skif PATH] [--only NAME,...] [--no-fixtures] [--no-corpus] [--check | --write-baseline]
cargo run --release -p skif-bench -- deadcode [--skif PATH] [--packages 5] [--seed 1] [--knip] [--json] [--out target/bench-deadcode] [--force]
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
| `namespaceMembers` | `unused-namespace-members` |
| `nsExports`, and `exports` items that carry a `namespace` | `unused-namespace-exports` |
| `nsTypes`, and `types` items that carry a `namespace` | `unused-namespace-types` |
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
times out, skif fails, or knip prints no JSON; the reason names the first error line the install printed, on stderr
or stdout, or knip's error. A repository with `install = false` in `corpus.toml` (rushstack: Rush keeps its lockfiles
under `common/config`) is compared uninstalled, so the tools say themselves whether they can run there: knip has no
Rush support and stops with `Unable to find package.json`, which becomes the skip reason. Without `--install` the run is quick but dominated by that noise. Whether a repository counts as installed is read
from the checkout (the `.skif-bench-installed` marker holds the pinned commit once an install succeeded), not from the
flag: `parity.md` says `yes`, `no` or `partial (n of m)`, each baseline entry records its own `installed`, and
`--check` rejects an entry whose checkout is installed differently from when it was recorded. A repository may set `install-command` (program first; `rules-js` installs with
the pnpm its `MODULE.bazel` pins) and `install-timeout-secs` (`vercel-examples`) in `corpus.toml`.

**Scope.** knip runs with an empty configuration (`--config` to a `{}` file in the scratch directory), so a repository's
own ignores and disabled issue types do not hide findings; `--repo-knip-config` uses the repository's own. skif runs with
`--recursive` on repositories that set `recursive = true` in `corpus.toml` (nested roots that are not workspace members),
or on all with `--recursive`; paths stay relative to the start directory, as in knip's report. A repository's `entry`
list in `corpus.toml` names entry files of the root workspace that neither tool finds by itself (vscode's workbench and
workers, which only its bundler script names); both tools get them after their shared defaults (`[js.workspaces."."]`
in a `--config` file for skif, `workspaces["."].entry` in knip's configuration), so the files only they reach are no
longer reported as unused on both sides. Enum and namespace
members are keyed by the member name on both sides. knip's stdout may carry banners around its JSON; the first object
that is a knip report is used.

**knip errors.** knip prints `ERROR: Error loading <file>` when a tool configuration of the repository does not load
(create-t3-turbo's `metro.config.js` requires an unlisted package; vue-core's `vite.config.ts` files read an undefined
`__DEV__`), runs on without the entry files that configuration names, prints its report and exits with 2. The harness
records those files in `parity.json` (`knip_errors`, with `knip_exit`), their number in the baseline (`tool_errors`)
and in the status cell of `parity.md`, and counts a finding only one tool reports in a package that holds such a file
as the `knip-config-error` known deviation rather than as a difference: on knip's side because it ran without that
configuration's entries, on skif's because knip's view of the package is incomplete.

**Invalid knip runs.** A repository whose knip run cannot be a valid comparison sets `knip-invalid` in `corpus.toml`
with the reason (n8n: its configs import the workspace's own `@n8n/*-config` packages, which have no built `dist`
after an install without scripts, so knip loads none of them). It is still compared and recorded, and its status
cell in `parity.md` (and `invalid` in the baseline) carries the reason, so its counts are not read as agreement.

**Findings inside unused files.** A tool analyses nothing inside a file it reports as unused, so a finding in such a
file follows from the `files` disagreement, which is counted once already. A finding only skif reports in a file knip
lists under `files` is the `in-knip-unused-file` known deviation, and a finding only knip reports in a file skif
reports unused is `in-skif-unused-file`; unused files themselves and the manifest categories (`dependencies`,
`devDependencies`, `optionalPeerDependencies`, `binaries`) stay compared.

**Labels.** `exports`, `types`, `nsExports` and `nsTypes` key the same `<file>|<name>`, and the tools label some
differently: knip calls a `const enum` a type and files an export under `nsExports` when any importer uses a namespace,
skif when all do. A skif finding knip has under another of these labels counts as shared, under knip's label.

**Known deviations.** Findings on one side below a path in a repository's `knip-unread` list are the `knip-unread`
deviation: knip reads no files there, as npm's deny-all `.gitignore` hides its workspaces, so it reports every
dependency there as unused and says nothing about the files. In `files`, a file only knip reports is not counted as a
difference when it is outside skif's project set (`non-code-file`) or a test file or any file below a test directory
(`test-file`). A finding only knip reports below a `fixtures` or `__fixtures__` directory is `fixture-file` in the
issue types skif does not report there: files, exports, types and members unless `[js] include-fixtures` is set, and
unresolved imports and unlisted dependencies at all (other test-data names such as `testdata` or `*-fixtures` are
judged by skif and not counted here). Those are reported in their own column and section, with the reason, and
`--check` ignores them. Most `test-file` findings are test files, which skif's test-runner plugins make entries, or
test helpers and setup files; in next.js thousands are the app files of test fixtures below test directories, which
are not test entries. The harness sets them apart by path, without establishing why skif does not report each one.
`--include-test-files` counts the `files` of `fixture-file` and `test-file` as differences (not usable with a
baseline, nor is `--repo-knip-config`). See
[ADR 0030](../../adrs/0030-harness-known-deviations-and-comparison-scope.md) and
[ADR 0068](../../adrs/0068-what-the-knip-comparison-sets-apart.md).

**Foreign tsconfig paths.** knip resolves every import through the merged tsconfig `paths` of all workspaces; skif
uses the `paths` of the configuration the importing file follows, and reports an import that only another package's
`paths` map as `unlisted-dependencies` with the detail `ForeignTsconfigPaths`. A finding only skif reports in
`unlisted` that carries this detail is the `foreign-tsconfig-paths` known deviation: counted in the known-deviations
column and section, recorded in the baseline and ignored by `--check`. An `unlisted-dependencies` finding without the
detail stays a difference. See [ADR 0075](../../adrs/0075-imports-only-another-packages-tsconfig-paths-map.md).

**Where each tool looks.** knip's workspaces are the root and the members its patterns select (`pnpm-workspace.yaml`
or the `workspaces` of `package.json`); the harness reads them with skif's workspace discovery and records their
number in `parity.json` (`workspace_members`). A project directory is one whose `package.json` has a field besides
`type`, `sideEffects` and `imports`. Four more known deviations follow from these facts, each decided by path and
manifest, not by listing findings:

- `knip-unjudged-manifest`: an unused dependency or a script binary only skif reports, on a `package.json` that is not
  one of knip's workspaces and that knip reports nothing on. knip judges its workspaces' manifests only; skif's
  recursive run lints a nested project as a root of its own.
- `non-member-project`: an unused file only knip reports, or an unlisted dependency only knip reports that the file's
  own manifest lists, below a project directory that is not one of knip's workspaces. knip counts those files to an
  enclosing workspace without their own manifest's entries and dependencies; skif's analysis of the enclosing
  workspace skips the directory.
- `skipped-directory`: a finding only knip reports in a file below a directory skif's file discovery skips by name
  (`node_modules`, `dist`, `.next`, `.vercel` and the other build output names, `out`, `target` and `coverage`
  directly in a package directory, and hidden directories other than tool ones such as `.github`), unless the
  directory is a workspace member.
- `published-source`: an unused TypeScript file only knip reports whose path its package publishes with a built
  extension (`main`, `module`, `types`, `bin`, an `exports` leaf with `*` patterns, as `.js`, `.mjs`, `.cjs`, `.jsx` or
  a `.d.ts` variant). skif makes the source beside a published path an entry; knip maps a published path to source
  only through a tsconfig `outDir`. Paths mapped through a build directory (`out/main.js` to `src/main.ts`) stay
  compared.

`non-member-project` and `published-source` check where a finding is, not why skif has no finding there. Unlisted
dependencies a `declare module` satisfies, and npm's `lib/commands` files that skif reaches through a computed
`require` in `lib/npm.js`, stay compared: the harness cannot check them from its data. See
[ADR 0080](../../adrs/0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md).

**Not judged by skif.** skif lists under `incomplete-packages` in its JSON report the packages whose entry files it
cannot all know (a tool config it cannot read), and does not report their unused dependencies, exports, types and
members, nor the files the config could make entries. A finding only knip reports in `files`, `dependencies`,
`devDependencies`, `exports`, `types`, `nsExports`, `nsTypes`, `enumMembers` or `namespaceMembers` whose package (the
deepest directory above it with a `package.json`) is one of those is counted as `not-judged-by-skif`: in its own
column of `parity.md`, apart from the agreement counts and the other known deviations, and ignored by `--check`.
`unlisted`, `unresolved`, `binaries` and `duplicates` are judged in every package and stay compared. For `files` the
count is an upper bound: skif judges the files outside a narrowed scope
([ADR 0065](../../adrs/0065-narrowed-scope-for-incomplete-entries.md)), but its JSON does not say which packages are
narrowed. `parity.json` lists the directories per repository under `not_judged_packages`. See
[ADR 0053](../../adrs/0053-incomplete-packages-in-the-json-report.md) and
[ADR 0068](../../adrs/0068-what-the-knip-comparison-sets-apart.md).

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
`--only` is checked against those entries alone, and `--write-baseline --only` updates only them, so the baseline
can be refreshed one repository at a time (install, run, then delete the checkout's `node_modules`) where the disk
does not hold every install at once. In the committed `baselines/parity.json` the entries of create-t3-turbo,
vue-core, tanstack-query, jest, babel, cal-com, mastra, vscode, npm-cli and rushstack were recorded on 2026-11-08 with
the categories of [ADR 0068](../../adrs/0068-what-the-knip-comparison-sets-apart.md). The entries of n8n, angular,
opencode and rules-js (whose installs fetch tarballs from `codeload.github.com`) and of vercel-examples and next-js
(whose installs did not fit the disk) predate them, and the per-repository `entry` lists and recursive runs of
[#27](https://github.com/Goldziher/skif/issues/27), so they still need a
`mise run parity -- --install --write-baseline --only <name>` refresh before `--check` means anything for them. Every
entry predates the categories of
[ADR 0080](../../adrs/0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md), so
all of them need that refresh. The
scheduled `corpus` workflow (`.github/workflows/corpus.yml`, weekly and on demand, never on pull requests) runs both
checks. The decision is [ADR 0026](../../adrs/0026-parity-and-diff-tasks-on-a-pinned-corpus.md).

## Measured numbers

`mise run bench:publish` writes `tools/bench/results/summary.{json,md}` and rewrites this section with
`refresh-numbers.mjs`. These numbers come from skif at commit `bbb0971` (0.1.1 with the manifest-check work of
[#29](https://github.com/Goldziher/skif/issues/29)) on a 4-vCPU Linux VM (Intel Xeon at 2.10 GHz, 15 GB RAM, Linux
6.18) with no other work running (load average 1.5 to 2.9 from the VM itself), using hyperfine 2.0.0 with 15 measured
runs and sherif 1.13.0, syncpack 15.3.3 and knip 6.44.1 through their npm launchers on Node 22. The corpus was installed
first (`npm-cli` with npm 11, whose lockfile npm 10 rejects), so knip ran on every repository. Read the ratios rather
than the absolute times, which depend on the machine.

knip's error lines below are not failed runs: on create-t3-turbo and vue-core knip finishes its analysis and prints its
whole report, then exits with 2 because some of the repository's tool configurations do not load (create-t3-turbo:
`apps/expo/metro.config.js` requires the unlisted `metro-cache`, and `packages/db/drizzle.config.ts` throws without
`POSTGRES_URL`; vue-core: two `vite.config.ts` files read an undefined `__DEV__`). Setting the missing variables on
create-t3-turbo changes neither its findings nor its time. On npm-cli knip runs with `--no-gitignore` (the repository's
`bench-knip-args` in `corpus.toml`): npm's deny-all `.gitignore` otherwise hides the workspaces from knip, which then
analyses only the root package. At 2,000 packages skif is 1.3 times faster than sherif: 102 ms against 130 ms (median)
here, 101 ms against 125 to 128 ms in two separate 30-run comparisons, while skif reports about 24,700 diagnostics
there and sherif 638.

<!-- bench:numbers:start -->
Measured with hyperfine 2.0.0, 3 warmup and 15 measured runs on linux x86_64; skif 0.1.1.

**generated: 50 packages (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif --no-js | 6.3 | 6.3 | 1.0x |
| sherif | 40.4 | 38.8 | 6.4x |
| syncpack lint | 46.8 | 46.3 | 7.4x |

**generated: 500 packages (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif --no-js | 27.1 | 26.9 | 1.0x |
| sherif | 59.0 | 58.8 | 2.2x |
| syncpack lint | 155 | 156 | 5.7x |

**generated: 2000 packages (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif --no-js | 99.4 | 102 | 1.0x |
| sherif | 131 | 130 | 1.3x |
| syncpack lint | 649 | 655 | 6.5x |

**graph: 10000 files in 100 packages**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 276 | 271 | 1.0x |

**corpus: create-t3-turbo (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 33.2 | 31.9 | 1.0x |
| skif --no-js | 4.9 | 4.6 | 0.1x |
| sherif | 35.4 | 36.0 | 7.2x |
| syncpack lint | 41.1 | 41.0 | 8.4x |
| knip --reporter json | 2011 | 2011 | 60.7x |

knip --reporter json exited with an error in 15 of 15 runs, so its time may be that of a partial run.

**corpus: vue-core (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 62.7 | 61.9 | 1.0x |
| skif --no-js | 5.5 | 5.5 | 0.1x |
| sherif | 36.7 | 35.6 | 6.7x |
| syncpack lint | 42.7 | 42.5 | 7.8x |
| knip --reporter json | 1688 | 1657 | 26.9x |

knip --reporter json exited with an error in 15 of 15 runs, so its time may be that of a partial run.

**corpus: tanstack-query (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 375 | 375 | 1.0x |
| skif --no-js | 12.2 | 11.1 | 0.0x |
| sherif | 43.2 | 42.0 | 3.6x |
| syncpack lint | 111 | 106 | 9.2x |
| knip --reporter json | 4563 | 4527 | 12.2x |

**corpus: babel (yarn)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 884 | 884 | 1.0x |
| skif --no-js | 9.2 | 9.4 | 0.0x |
| sherif | 37.5 | 37.3 | 4.1x |
| syncpack lint | 277 | 280 | 30.2x |
| knip --reporter json | 7181 | 7217 | 8.1x |

**corpus: npm-cli (npm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 99.8 | 98.6 | 1.0x |
| skif --no-js | 5.3 | 5.3 | 0.1x |
| sherif | 38.0 | 38.0 | 7.2x |
| syncpack lint | 79.1 | 77.2 | 14.9x |
| knip --reporter json | 960 | 968 | 9.6x |

| ADR 0001 target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 27.1 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 275.8 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (measured 60.7x; knip exited with an error in 15 runs) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (measured 26.9x; knip exited with an error in 15 runs) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 12.2x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 8.1x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 9.6x) | pass |

<!-- bench:numbers:end -->

## Seeded defects

`skif-bench generate --packages 500 --seed 1` injects known defects, so unlike the corpus this has a right answer.
Run `skif --no-js`, `sherif` and `syncpack lint` on the output and compare with the files:

| Defect | Actual (from the files) | skif | sherif | syncpack |
| --- | ---: | ---: | ---: | ---: |
| Dependencies with mismatched versions | 200 | 200 | 200 | 200 |
| Unsorted dependency maps | 25 | 25 | 25 | not checked |
| Type packages in `dependencies` | 24 | 24 | 24 | not checked |
| Empty dependency objects | 31 | 31 | 31 | not checked |

The generator reports 347 mismatched entries; the tools count every instance off the aligned version, which is 4,568
for both skif and syncpack, as the same set. It reports 30 empty objects because it counts `optionalDependencies`; the
root also has an empty `devDependencies`. This covers only the defect classes the generator injects.

## Dead-code precision and recall

Agreement with knip on the corpus is not precision: either tool can be wrong where they agree or differ. `skif-bench
deadcode` measures both tools against a right answer instead. It generates a pnpm workspace of `--packages` packages
(default 5) with seeded dead code, deterministic for `--seed`, writes it to `<out>/repo` and the answer key, every
expected finding as a knip issue type and a parity key, to `<out>/answer-key.json`. It then runs
`skif . --format json --no-install` and, with `--knip`, the knip pinned in `corpus.toml` with an empty configuration (as
`parity` does; needs `npx` and the npm registry). Both reports are reduced to the parity keys, and each tool gets
precision (seeded findings among those reported) and recall (reported findings among those seeded) per issue type.
`nsExports` and `nsTypes` count as `exports` and `types`. A finding in a type the key does not seed (`unlisted`,
`unresolved`, `binaries`, ...) is false by construction. The report goes to stdout (`--json` for JSON) and to
`<out>/deadcode.{md,json}`, with every false finding and miss listed. A rerun replaces `<out>/repo` only when `<out>`
holds the `answer-key.json` of an earlier run; `--force` replaces it regardless.

Every package holds the dead code: unused dependencies and devDependencies (plain runtime libraries with no knip plugin
or skif tool mapping), unused files (one standalone, one that imports live code), unused exports and types in a
module the entry imports, and an unused export behind a re-export chain. Next to it sits live code reached only
through a pattern that has caused false findings on real repositories. A false finding that names that code is
attributed to its pattern:

| Pattern | Live code |
| --- | --- |
| `namespace-member` | exports read as `ns.member` through `import * as ns` |
| `namespace-bare` | a namespace import passed as a value, which uses every export |
| `ts-namespace` | members of an exported TypeScript `namespace` |
| `declare-module` | an import of a virtual module declared in a `.d.ts` |
| `test-mock` | a manual mock in `__mocks__` for a `vi.mock()` call |
| `typeof-import` | an export read only through `typeof import("./file")` |
| `dynamic-import` | a module loaded with `await import()` |
| `re-export-chain` | exports reached through `export *` and `export { } from` |
| `import-meta-glob` | modules loaded with `import.meta.glob` |
| `script-entry` | a file run by a `package.json` script |
| `tsconfig-paths` | an import through a tsconfig `paths` alias |
| `fixture-dir` | a file in a `fixtures` directory (test data) |
| `optional-dependency` | an optional dependency loaded with `import()` in a `try` |
| `subpath-import` | a `#` subpath import mapped by the `imports` field |
| `vercel-json-command` | a devDependency whose binary only the `buildCommand` of `vercel.json` runs |

The workspace is not installed, so nothing depends on `node_modules`: the only `package.json` script runs `node`, the
`vercel.json` command runs a listed devDependency named like its binary, and every import of a dependency names a
listed package. The `skif-bench` unit tests run skif in process on the same workspace (two seeds,
no binary, network or Node) and fail when it misses a seeded finding or reports a false one. CI exercises neither the `skif` binary invocation nor the knip side: `skif-bench`
cannot depend on the `skif` binary in its tests, and knip needs Node and the npm registry. Run `deadcode` with `--knip`
by hand to check them.

Default run (`--packages 5 --seed 1`: 33 seeded findings, 120 live items). skif was measured at the current source
(after 0.1.1) on 2026-11-08; knip 6.44.1 on 2026-11-07:

| Issue type | Seeded | skif precision | skif recall | knip precision | knip recall |
| --- | ---: | ---: | ---: | ---: | ---: |
| `dependencies` | 4 | 1.000 | 1.000 | 1.000 | 1.000 |
| `devDependencies` | 5 | 1.000 | 1.000 | 1.000 | 1.000 |
| `exports` | 15 | 1.000 | 1.000 | 0.750 | 1.000 |
| `files` | 6 | 1.000 | 1.000 | 0.545 | 1.000 |
| `types` | 3 | 1.000 | 1.000 | 1.000 | 1.000 |
| All | 33 | 1.000 | 1.000 | 0.767 | 1.000 |

Both tools find every seeded finding, and skif reports nothing else: it does not report files in `fixtures`
directories (ADR 0050) and credits the export `typeof import("./typed")["settings"]` names. knip's false findings are
those five exports (`typeof-import`) and the five fixture files (`fixture-dir`). The patterns are synthetic and cover
only these cases, so the numbers say which known traps a tool falls into, not how precise it is on a real repository.

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
- **knip needs installed dependencies** to give realistic results: without them it fails to load tool configurations
  that import the repository's own packages (create-t3-turbo's `@acme/tsconfig`), and the time is that of a partial
  run. `run --install` installs each corpus repository first, as `parity --install` does (`mise run bench:publish`
  passes it); a repository whose install fails is timed without knip. Installing changes the timings of every tool,
  not only knip's (skif, sherif and syncpack then run on a tree with `node_modules`), so compare only runs made the
  same way. Installed, knip still reports two errors on create-t3-turbo that are the repository's own:
  `apps/expo/metro.config.js` requires `metro-cache`, which it does not list, and `packages/db/drizzle.config.ts`
  throws without a `POSTGRES_URL`.
- Non-zero exits are ignored (`--ignore-failure`): linters exit 1 when they find something.
- The skif command line is `skif . --format json --no-install [--no-js]`; the working directory is the target.
- Wall-clock times include process start-up. For sub-10 ms runs hyperfine warns about short commands; trust the
  mean over many `--runs`.
- The corpus pins live in `corpus.toml` (full commit SHAs, the knip and syncpack versions, the suites of each
  repository). Repositories are fetched shallow with `git fetch --depth 1 origin <sha>` (plus a cone sparse checkout
  where `sparse` is set), so `fetch` is idempotent and cheap to repeat. Bump a pin deliberately and refresh the
  baselines in the same change.
