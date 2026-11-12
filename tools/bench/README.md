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

`sherif`, `syncpack` and `knip` are the versions pinned under `[tools]` in `corpus.toml`, where their versions are
declared. `tools/package.json` and `tools/package-lock.json` repeat them and pin every package they depend on; the
harness refuses to load `corpus.toml` when the lock's root dependencies or its installed `node_modules/<tool>` versions
differ from `[tools]`. To bump a tool, change `[tools]` and `tools/package.json`, then run `npm install
--package-lock-only --ignore-scripts` in `tools/`. `run` copies the two files into `target/bench-generated/tools`,
installs them there once with `npm ci --ignore-scripts` and times the executables in its `node_modules/.bin` directly,
so no `npx` resolution is timed and nothing on `PATH` is used. A tool that cannot be installed (no `npm`, a failed
install), or knip on a corpus repository whose install failed, is left out and recorded with the reason under
`skipped_tools`. Results go to `target/bench-results/` (or `--out`): `summary.md` (tables plus the ADR checks),
`summary.json` (schema 5: skif version, the commit of the skif source it was built from with a `-dirty` flag
(`--skif-source`, else the harness's own checkout) and the harness's commit, the machine record and the one line
`machine_text` that `summary.md` and the published tables print, the toolchain, the load average before and after the
run, hyperfine version, each comparison tool's `--version`, the corpus repositories of an `--only` run, every target
with mean, stddev, median, min and max in ms and the number of runs that exited with an error, and for an installed
corpus repository `installed_with`, the package manager and the `--version` it reports in the checkout (where corepack
resolves the repository's `packageManager`) or the repository's `install-command`, its `declared_knip_failure` and, on a
knip row that failed that way, `failure` (the reason and the error line), the skipped tools and the checks, each `pass`,
`fail`, `invalid` or `not-measured`), and one raw hyperfine export per target under `raw/`. The machine record holds the
operating system (`PRETTY_NAME` of `/etc/os-release`, `sw_vers` on macOS), the kernel (`uname -sr`), the architecture,
the CPU model (`/proc/cpuinfo`, `lscpu` for Arm, `sysctl` on macOS), the logical CPUs, those the process may use and the
physical cores, the total memory, the cgroup CPU limit (the smallest `cpu.max` of the process's cgroup and its
ancestors), the hypervisor, and in a GitHub Actions job the runner image (`ImageOS`, `ImageVersion`), `RUNNER_OS`,
`RUNNER_ARCH`, `RUNNER_ENVIRONMENT`, `RUNNER_NAME`, the run attempt (`GITHUB_RUN_ATTEMPT`) and the URL of that attempt;
the toolchain holds the `--version` of `rustc`, `node`, `npm`, `pnpm`, `yarn` and `bun` on `PATH`, probed in a directory
without a `package.json` (so a corepack shim reports its default, not a corpus repository's version). A fact that cannot
be read is `null`. `--only NAME,...` times only those repositories of the bench corpus. `run` exits non-zero when an ADR
0001 check fails, after writing the summary; `--no-enforce` turns that off. A status of 1 is a complete run that
reported findings; a run that exits with a status above 1 (knip's exit code when it fails or cannot load a configuration
file) or by a signal is an error, since its time may be that of a partial run. Such a tool is run once more outside
hyperfine, the end of its stderr goes to the log and its error line to `error_output` in the summary and under its
table, and an ADR 0001 check that compares it is `invalid` instead of `pass` or `FAIL`; only `FAIL` makes `run` exit
non-zero. A bench repository on which knip is known to fail declares it in `corpus.toml` as `bench-knip-failure`, a
`matches` text and a `reason` (create-t3-turbo's Expo Metro config and vue-core's SFC playground Vite config do not
load). skif, sherif and syncpack are still timed there; when knip errors and that run's stderr or error line holds
`matches`, its row is marked with `failure`, has no ratio to skif, shows as `fails: <reason>` in `summary.md` and the
published tables, and its check is `not-measured` with the reason. Its times stay in `summary.json` and `raw/` but are
compared with nothing. A declared failure that did not happen, because knip ran without an error or failed with an error
that does not hold `matches`, makes the check `invalid` and names the declaration as stale, so a changed failure is
noticed and the declaration updated or removed; an error no declaration covers is `invalid`. Loading `corpus.toml` fails
when `matches` or `reason` is empty or the repository is not in the bench suite. A corpus repository's `bench-knip-args`
in `corpus.toml` are passed to knip, so it reads the files skif reads (npm-cli sets `--no-gitignore`), and skif runs
with `--recursive` where the repository sets `recursive` (babel), as `parity` runs it. hyperfine 1.x and 2.x exports are
both read.

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
mise run bench:import -- <run-id>  # import a bench workflow run into tools/bench/results and refresh the speed tables
mise run parity:publish   # refresh the generated agreement tables from baselines/parity.json
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
cargo run --release -p skif-bench -- run [--skif PATH] [--skif-source DIR] [--sizes 50,500,2000] [--graph-files 10000] [--pm pnpm] [--no-corpus | --only NAME,...] [--warmup 3] [--runs 15] [--skif-arg ARG]... [--no-enforce] [--install]
cargo run --release -p skif-bench -- parity [--skif PATH] [--only NAME,...] [--install] [--recursive] [--repo-knip-config] [--include-test-files] [--check | --write-baseline]
cargo run --release -p skif-bench -- syncpack-diff [--skif PATH] [--only NAME,...] [--no-fixtures] [--no-corpus] [--check | --write-baseline]
cargo run --release -p skif-bench -- deadcode [--skif PATH] [--packages 5] [--seed 1] [--knip] [--json] [--out target/bench-deadcode] [--force]
```

`corpus fetch`, `run`, `parity` and `syncpack-diff` lock each corpus checkout (`<dir>/<name>.lock`, the operating
system's file lock, holding the process id) from the fetch to the end of their pass over it, so two runs never
install into, write into or time the same checkout at once: a second run stops at once and names the lock file and,
except on Windows, where the lock keeps other processes from reading the file, the process id it holds. A run that
dies releases its locks with the process, so there is no stale lock to remove. The corpus directory must be writable:
a lock file that cannot be created there is an error that says so.

`generate` is deterministic for a seed: the same options produce byte-identical files. It writes a root `package.json`,
a lockfile stub (and `pnpm-workspace.yaml` for pnpm) so package manager detection works, and packages under `apps/*`,
`packages/*` and `packages/group/*`. Dependencies are drawn from a fixed pool of about 320 real npm names with a
per-name specifier (caret, tilde, exact, `npm:` alias, a few `github:`, `file:` and `link:`), plus `workspace:*` links
between packages. `--source-files-per-package N` adds `N` TypeScript modules under `src/` of every package: a chain of
modules in which each imports its predecessor, ending in the `src/index.ts` that the manifests name as `main`.
`--mismatch-rate` is the probability, per dependency or package, of each injected issue: a version mismatch, unsorted
`dependencies`, an `@types/*` package in `dependencies`, an empty `optionalDependencies`. The command prints the
injected counts as JSON.

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
`--frozen-lockfile`, `npm ci`, `bun install --frozen-lockfile`); the installed state is cached in the checkout. When the
package manager is not on `PATH` or the install or build fails or times out, the repository is still compared on what
its checkout holds, which counts as not installed, and `parity.json` records why under `install_failure`: the first
error line the step printed, on stderr or stdout, or its time limit. `--check` then rejects the entry if it was recorded
installed. A repository is skipped, with the reason in the report, when skif fails or knip prints no JSON. A repository
with `install = false` in `corpus.toml` is compared uninstalled, so the tools say themselves whether they can run there.
rushstack installs with `node common/scripts/install-run-rush.js install --bypass-policy`, which links each project's
`node_modules`. knip has no Rush support and stops with `Unable to find package.json` without a root manifest, so
rushstack sets `knip-root = "rush"`: while knip runs, the checkout gets a root `package.json` with the `projectFolder`
of every `rush.json` project as its `workspaces`, so knip reads the installed `node_modules` and both reports share one
root. The manifest carries a `//` mark and is removed after the knip run; one an interrupted run leaves behind is
removed when the checkout is next fetched, so skif and the syncpack diff never read it. It makes the Rush root a
workspace for knip, which skif does not analyse, so knip's findings in root files outside every project
(`.prettierrc.js`, `common/scripts`) are the `rush-root-file` known deviation. A repository may set `build-command`
(program first) in `corpus.toml`, run after a successful install within `build-timeout-secs` (the install's time limit
when unset): n8n builds the `@n8n/*-config` packages and `@n8n/constants` with their workspace dependencies, whose built
`dist` its tool configurations import. The install marker records the install and build commands, so a checkout
installed with other commands is installed again. Without `--install` the run is quick but dominated by knip's
unresolved and unlisted imports of the uninstalled packages. Whether a repository counts as installed is read from the
checkout (the `.skif-bench-installed` marker holds the pinned commit once an install succeeded), not from the flag:
`parity.md` says `yes`, `no` or `partial (n of m)`, each baseline entry records its own `installed`, and `--check`
rejects an entry whose checkout is installed differently from when it was recorded. A repository may set
`install-command` (program first; `rules-js` installs with the pnpm its `MODULE.bazel` pins, `opencode` with the bun of
`mise.lock` through `mise x bun@1.3.14`, so neither depends on the package manager on `PATH`) and `install-timeout-secs`
(`vercel-examples`) in `corpus.toml`.

**Scope.** knip runs with an empty configuration (`--config` to a `{}` file in the scratch directory), so a repository's
own ignores and disabled issue types do not hide findings; `--repo-knip-config` uses the repository's own. skif runs
with `--recursive` on repositories that set `recursive = true` in `corpus.toml` (nested roots that are not workspace
members), or on all with `--recursive`; paths stay relative to the start directory, as in knip's report. A repository's
`entry` list in `corpus.toml` names entry files of the root workspace that neither tool finds by itself (vscode's
workbench and workers, which only its bundler script names); both tools get them after their shared defaults
(`[js.workspaces."."]` in a `--config` file for skif, `workspaces["."].entry` in knip's configuration), so the files
only they reach are no longer reported as unused on both sides. Enum and namespace members are keyed by the member name
on both sides. knip's stdout may carry banners around its JSON; the first object that is a knip report is used.

**knip errors.** knip prints `ERROR: Error loading <file>` when a tool configuration of the repository does not load
(create-t3-turbo's `metro.config.js` requires an unlisted package; vue-core's `vite.config.ts` files read an undefined
`__DEV__`), runs on without the entry files that configuration names, prints its report and exits with 2. The harness
records those files in `parity.json` (`knip_errors`, with `knip_exit`), their number in the baseline (`tool_errors`)
and in the status cell of `parity.md`, and counts a finding only one tool reports in a package that holds such a file
as the `knip-config-error` known deviation rather than as a difference: on knip's side because it ran without that
configuration's entries, on skif's because knip's view of the package is incomplete.

**Invalid knip runs.** A repository whose knip run cannot be a valid comparison sets `knip-invalid` in `corpus.toml`
with the reason. It is still compared and recorded, and its status cell in `parity.md` (and `invalid` in the
baseline) carries the reason, so its counts are not read as agreement. No repository is marked at present.

**Findings inside unused files.** A tool analyses nothing inside a file it reports as unused, so a finding in such a
file follows from the `files` disagreement, which is counted once already. A finding only skif reports in a file knip
lists under `files` is the `in-knip-unused-file` known deviation, and a finding only knip reports in a file skif
reports unused is `in-skif-unused-file`; unused files themselves and the manifest categories (`dependencies`,
`devDependencies`, `optionalPeerDependencies`, `binaries`) stay compared. A dependency only knip reports unused is
`used-in-knip-unused-file` when a file of the same workspace that only knip lists under `files` imports the package
(an `@types/` package when it imports the runtime package); a file both tools report unused credits the package on
neither side, so its dependencies stay compared: the harness parses those files with skif's parser
(`skif-js-syntax`) for their bare imports, requires, re-exports and dynamic imports. knip does not analyse such a
file, so it never sees the import skif credits; the harness does not check that this import is the one skif counts.
The mirror, a dependency only skif reports unused that a file of the same workspace only skif reports unused
imports, is `used-in-skif-unused-file`, from the same import scan.

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
- `non-member-project`: a finding only knip reports in a file (an unused file, export, type or member, a duplicate, an
  unresolved import, or an unlisted dependency that the file's own manifest lists) below a project directory that is
  not one of knip's workspaces. knip counts those files to an enclosing workspace without their own manifest's entries
  and dependencies; skif's analysis of the enclosing workspace skips the directory. When a recursive skif run lints
  the project as a package of one of its roots (the `packages` of the JSON `roots`), its exports, types, members and
  duplicates stay compared, since skif judges them there.
- `skipped-directory`: a finding only knip reports in a file below a directory skif's file discovery skips by name
  (`node_modules`, `dist`, `.next`, `.vercel` and the other build output names, `out`, `target` and `coverage`
  directly in a package directory, and hidden directories other than tool ones such as `.github`), unless the
  directory is a workspace member.
- `published-source`: an unused TypeScript file only knip reports whose path its package publishes with a built
  extension (`main`, `module`, `types`, `bin`, an `exports` leaf with `*` patterns, as `.js`, `.mjs`, `.cjs`, `.jsx` or
  a `.d.ts` variant) or without one (`./out/main`). A published path below a build directory (`dist`, `lib`, `out`,
  `esm` and the others of `skif_js::is_out_dir`, also with a flavour suffix such as `lib-commonjs`) also names the
  same path below `src`, as skif maps it: `./lib-commonjs/*.js` publishes every `src/**/*.ts`. A published path
  without `*` names one source only, the first of skif's candidates that exists (`src/x.ts` before `lib/x.ts` for
  `./lib/x.js`, `.ts` before `.tsx`). In an n8n node package the files of `n8n.nodes` and `n8n.credentials` are
  published too, a path below a build directory naming the path without it (`dist/nodes/A/A.node.js` names
  `nodes/A/A.node.ts`), as skif reads them (`skif_js::n8n_load_paths`). A file such a source imports by relative path
  (resolved with its extension, a TypeScript twin of a `.js` one, or the `index` file of a directory), directly or
  through other files only knip reports unused, is `published-source` too. skif makes the source of a published path
  an entry and uses what it imports; knip maps a published path to source only through a tsconfig `outDir` and does
  not read the n8n fields. The exports, types and members of these files stay compared.

`non-member-project` and `published-source` check where a finding is, not why skif has no finding there. Unlisted
dependencies a `declare module` satisfies, and npm's `lib/commands` files that skif reaches through a computed
`require` in `lib/npm.js`, stay compared: the harness cannot check them from its data. See
[ADR 0080](../../adrs/0080-knip-workspaces-skipped-directories-and-published-sources-in-the-knip-comparison.md)
and [ADR 0100](../../adrs/0100-knip-comparison-n8n-node-files-and-what-published-sources-import.md).

**Not judged by skif.** skif lists under `incomplete-packages` in its JSON report the packages whose entry files it
cannot all know (a tool config it cannot read), and does not report their unused dependencies, exports, types and
members, nor the files the config could make entries. A finding only knip reports in `dependencies` or
`devDependencies` whose package (the deepest directory above it with a `package.json`) is one of those is counted as
`not-judged-by-skif`, and so is one in `files`, `exports`, `types`, `nsExports`, `nsTypes`, `enumMembers` or
`namespaceMembers` in a file skif left unjudged: any file of the package when its entry's `scope` is `"package"`, only
a file its globs match (relative to the package directory, matched as skif matches them) when the scope is
`{"within": [...]}` ([ADR 0065](../../adrs/0065-narrowed-scope-for-incomplete-entries.md),
[ADR 0081](../../adrs/0081-exports-outside-a-narrowed-incomplete-scope-are-judged.md)); a package listed twice gets
the union of its scopes. These are counted in their own column of `parity.md`, apart from the agreement counts and the
other known deviations, and ignored by `--check`. `unlisted`, `unresolved`, `binaries` and `duplicates` are judged in
every package and stay compared. `parity.json` lists the directories per repository under `not_judged_packages`, and
the globs of the narrowed ones under `narrowed_packages`. See
[ADR 0053](../../adrs/0053-incomplete-packages-in-the-json-report.md),
[ADR 0068](../../adrs/0068-what-the-knip-comparison-sets-apart.md) and
[ADR 0099](../../adrs/0099-knip-comparison-narrowed-scopes-rush-installs-and-dependencies-of-unused-files.md).

## Policy diff against syncpack

`skif-bench syncpack-diff` compares skif's policy rules (`version-mismatch`, `semver-range-mismatch`,
`banned-dependency`, `invalid-local-version`, `policy-conflict`, `unresolved-catalog-references` and the catalog rules)
with `syncpack@<pinned> json`. syncpack has no JSON lint reporter, but `json` prints every dependency instance with its
status code, and skif's diagnostic `detail` carries the same status code names (`DiffersToHighestOrLowestSemver`,
`IsBanned`, ...). A finding is keyed by status code, `package.json` path and dependency. Two targets:

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
type or status code and repository, plus the case summary of the fixtures) and the tool version they were recorded
with. They contain no file, package or symbol names. `baselines/parity.json` also records the set of compared
categories and known deviations (`category_set`) and, per repository, whether its checkout was installed, whether
skif ran recursively and the skif source commit (`skif_rev`, informative only). After a deliberate change, or a bump
of a pin in `corpus.toml`, refresh them:

```sh
mise run parity -- --install --write-baseline
mise run diff:syncpack -- --write-baseline
```

`--check` compares a run with the baseline and exits non-zero when a count moves by more than
`--tolerance-percent` (default 10) of its baseline value or `--tolerance-abs` (default 5), whichever is larger, when a
repository changes between compared and skipped, or when a fixture that agreed before now differs. It refuses, with
one message instead of a list of drifts, a baseline recorded with another tool version or another category set (an
older harness, or one that sets apart other findings), and it refuses a repository whose checkout is installed
differently, or whose skif ran with another `--recursive` mode, from when it was recorded. A run narrowed with
`--only` is checked against those entries alone, and `--write-baseline --only` updates only them, so the baseline can
be refreshed one repository at a time (install, run, then delete the checkout's `node_modules`) where the disk does
not hold every install at once; entries recorded with another tool version or category set are dropped on write, not
merged. The scheduled `corpus` workflow (`.github/workflows/corpus.yml`, weekly and on demand, never on pull requests)
runs both checks and writes the parity table and any drift to the job summary. The decision is
[ADR 0026](../../adrs/0026-parity-and-diff-tasks-on-a-pinned-corpus.md).

## Measured numbers

The published numbers come from the `bench` workflow (`.github/workflows/bench.yml`), which runs only when dispatched,
on a pinned GitHub-hosted runner image ([ADR 0113](../../adrs/0113-speed-benchmarks-on-a-pinned-github-hosted-runner.md)):

```sh
gh workflow run bench.yml -f ref=v0.3.0 -f runner=ubuntu-24.04   # also: -f runs=15 -f warmup=3 -f only=babel
gh run list --workflow bench.yml --limit 1                       # the run id
mise run bench:import -- <run-id>
```

`ref` is the commit, tag or branch of skif to measure (default: the dispatched ref, which is also where the harness,
`corpus.toml` and the tool versions come from); `runner` is `ubuntu-24.04` or `ubuntu-24.04-arm`. The workflow checks
the harness out at the workspace root, so its `mise.toml` applies to every step, and the measured skif into `measured/`
(untracked, so the harness commit is not `-dirty`). It builds skif and the harness with the `rust-version` of their own
`Cargo.toml` (a minor release, so its latest patch; the summary records the exact `rustc`) and `--locked`, installs Node
and hyperfine from `mise.lock` with `MISE_LOCKED=1`, and a pinned corepack whose pnpm and yarn shims install the corpus
(each repository's `packageManager` selects the version; `COREPACK_DEFAULT_TO_LATEST=0` keeps a repository without one
on corepack's default), times everything with `run --install --no-enforce`, writes the corpus and the generated
workspaces to the runner's temporary directory, writes `summary.md` to the job summary and, once there is a summary,
uploads `summary.{json,md}` and `raw/` as the artifact `bench-<commit>-<runner>`. Runs on one runner image queue rather
than overlap (a concurrency group per image); GitHub keeps one pending run per group, so a dispatch made while another
waits cancels the waiting one. The harness fails before timing anything when a corpus repository is not pinned to a full
commit SHA or a comparison tool not to an exact version.

`mise run bench:import -- <run-id>` (`scripts/bench-import.sh`, needs `gh`, `git`, `jq` and Node) imports only a
successful `workflow_dispatch` run of `bench` whose commit is on `origin/main` (it runs `git fetch origin main` first)
and whose summary names that commit as the harness's; it refuses any other run, naming its branch. It downloads that
artifact, refreshes the block below, and the speed tables of the root `README.md` and of the comparison guide, with
`refresh-numbers.mjs --bench`, then puts the summary into `tools/bench/results` for a commit made locally. The first
line of each block names the skif version, source commit and the `rustc` that built it, the comparison tool versions,
the harness commit when it differs, the runner image and the machine, its load, the date and the workflow run attempt.
`refresh-numbers.mjs` writes nothing and exits non-zero when a comparison tool was not timed, a corpus repository has no
row for one of the tools, the run was narrowed with `only`, a tool exited with an error in a measured run that is not a
declared knip failure the run matched, an ADR 0001 check is `invalid`, the summary is of an older schema, it records no
GitHub-hosted runner or no workflow run, or the measured or the harness checkout had local changes (`-dirty`); the
import then changes nothing either. `mise run parity:publish` (`refresh-numbers.mjs --parity`) writes the agreement
tables from `baselines/parity.json` the same way, and refuses a baseline recorded by an older harness. `node --test
tools/bench/refresh-numbers.test.mjs` tests both. Measure the release commit after `release-bump`, or the skif version
names the previous release. Without flags, `refresh-numbers.mjs` refreshes both and leaves the speed tables as they are
while `tools/bench/results` holds no imported summary.

GitHub-hosted runners are shared virtual machines: a neighbour's load moves the times of a run by several percent, and
two runs of the same image may land on different CPU models (the summary names the one it got). The tables show
medians, which one slow run does not move, over 15 runs after 3 warmup runs; compare runs on the same runner image and
CPU model, and read the ratios against knip, sherif and syncpack, which share the machine and its noise, rather than
the absolute times. `mise run bench` stays for local measurement while developing; its numbers are not published.

A summary in which a tool exited with an error is not published, unless the error is a knip failure `corpus.toml`
declares and the run matched, which is published as `fails: <reason>` without a ratio: knip exits with 2 when some of
the repository's tool configurations do not load, and its time is then that of a partial analysis. The run's log and
`error_output` name the cause, which is fixed or declared in `corpus.toml` before measuring again. On npm-cli knip runs
with `--no-gitignore` (the repository's `bench-knip-args` in `corpus.toml`): npm's deny-all `.gitignore` otherwise hides
the workspaces from knip, which then analyses only the root package.

<!-- bench:numbers:start -->
Medians of 15 runs after 3 warmup runs (hyperfine 1.20.0), skif 0.3.0 at `150634614dea` built with rustc 1.99.1 (58cf8f9c3 2026-10-13) against knip 6.44.1, sherif 1.13.0, syncpack 15.3.3, measured on 2026-11-12 on a GitHub-hosted runner, image ubuntu24 20261101.360.1, Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, AMD EPYC 9V45 96-Core Processor, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft, load average 3.76 / 2.39 / 1.02 before and 1.72 / 1.95 / 1.12 after the run ([workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43136971156/attempts/1)).

**generated: 50 packages (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif --no-js | 3.6 | 3.6 | - |
| sherif | 23.0 | 22.4 | 6.5x |
| syncpack lint | 30.1 | 29.8 | 8.4x |

**generated: 500 packages (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif --no-js | 18.5 | 18.3 | - |
| sherif | 37.8 | 38.0 | 2.0x |
| syncpack lint | 110 | 109 | 6.0x |

**generated: 2000 packages (pnpm)**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif --no-js | 81.5 | 81.3 | - |
| sherif | 94.0 | 93.9 | 1.2x |
| syncpack lint | 498 | 495 | 6.1x |

**graph: 10000 files in 100 packages**

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 156 | 156 | - |

**corpus: create-t3-turbo (pnpm)**

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 25.1 | 23.0 | - |
| skif --no-js | 2.9 | 3.0 | - |
| sherif | 25.6 | 22.3 | 8.7x |
| syncpack lint | 26.3 | 26.1 | 8.9x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - |

**corpus: vue-core (pnpm)**

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 43.1 | 42.9 | - |
| skif --no-js | 3.1 | 3.1 | - |
| sherif | 22.7 | 22.6 | 7.3x |
| syncpack lint | 28.1 | 27.7 | 9.0x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - |

**corpus: tanstack-query (pnpm)**

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 281 | 280 | - |
| skif --no-js | 8.9 | 8.9 | - |
| sherif | 33.6 | 34.6 | 3.8x |
| syncpack lint | 85.7 | 85.6 | 9.7x |
| knip --reporter json | 2541 | 2533 | 9.0x |

**corpus: babel (yarn)**

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 604 | 603 | - |
| skif --no-js | 12.2 | 12.1 | - |
| sherif | 25.3 | 24.8 | 2.1x |
| syncpack lint | 251 | 251 | 20.6x |
| knip --reporter json | 3677 | 3675 | 6.1x |

**corpus: npm-cli (npm)**

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Median (ms) | vs skif |
| --- | ---: | ---: | ---: |
| skif | 63.2 | 64.0 | - |
| skif --no-js | 4.6 | 4.6 | - |
| sherif | 21.8 | 21.6 | 4.7x |
| syncpack lint | 55.9 | 55.8 | 12.1x |
| knip --reporter json | 483 | 481 | 7.6x |

| ADR 0001 target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 18.5 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 155.7 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 9.0x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 6.1x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 7.6x) | pass |
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
`vercel.json` command runs a listed devDependency named like its binary, and every import of a dependency names a listed
package. The `skif-bench` unit tests run skif in process on the same workspace (two seeds, no binary, network or Node)
and fail when it misses a seeded finding or reports a false one. CI exercises neither the `skif` binary invocation nor
the knip side: `skif-bench` cannot depend on the `skif` binary in its tests, and knip needs Node and the npm registry.
Run `deadcode` with `--knip` by hand to check them.

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

The `target/` group covers the ADR 0001 manifest and policy budget. The JS analysis speed target (at least 5x faster
than `knip`) is only measurable end to end with hyperfine on the corpus; `js_analysis` tracks regressions of the
in-process part.

## Caveats

- **Warm cache.** hyperfine warms up before measuring, so numbers are warm file-cache, not cold start. Compare
  cold runs with `hyperfine --prepare` and a cache purge by hand if that matters.
- **Comparison tools are the pinned versions.** `run` installs the `[tools]` versions of `corpus.toml`, with the
  dependencies `tools/package-lock.json` locks, and records each one's `--version` in the summary. Their output formats
  and defaults differ; `sherif` and `syncpack` only check manifests, `knip` analyses code.
- **knip needs installed dependencies** to give realistic results: without them it fails to load tool configurations
  that import the repository's own packages (create-t3-turbo's `@acme/tsconfig`), and the time is that of a partial
  run. `run --install` installs each corpus repository first, as `parity --install` does (the `bench` workflow
  passes it); a repository whose install fails is timed without knip. Installing changes the timings of every tool,
  not only knip's (skif, sherif and syncpack then run on a tree with `node_modules`), so compare only runs made the
  same way. Where knip fails even installed, for reasons that are the repository's own, the repository declares it
  with `bench-knip-failure` and knip's row is published as failing.
- Non-zero exits are ignored (`--ignore-failure`): linters exit 1 when they find something.
- The skif command line is `skif . --format json --no-install [--no-js] [--recursive]`; the working directory is the
  target.
- Wall-clock times include process start-up. For sub-10 ms runs hyperfine warns about short commands; trust the
  mean over many `--runs`.
- The corpus pins live in `corpus.toml` (full commit SHAs, the knip, syncpack and sherif versions, the suites of each
  repository). Repositories are fetched shallow with `git fetch --depth 1 origin <sha>` (plus a cone sparse checkout
  where `sparse` is set), so `fetch` is idempotent and cheap to repeat. Bump a pin deliberately and refresh the
  baselines in the same change.
