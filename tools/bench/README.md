# skif-bench

Benchmark harness for skif. It is an internal workspace member (`publish = false`). The `skif-bench` command treats the
skif binary as an external command; the crate also exposes its synthetic monorepo generator as a library
(`skif_bench::generate`) so the in-process criterion benches can build workspaces in memory.

## What it measures

`skif-bench run` times full CLI invocations with [hyperfine](https://github.com/sharkdp/hyperfine) (`-N`, no shell,
with `--warmup` and `--runs`) on three kinds of targets:

| Target | Contents | Tools timed |
| --- | --- | --- |
| Generated workspaces | 50, 500 and 2000 packages (`--sizes`), pnpm layout, 5 % issue rate, seed 1 | `skif --no-js`, `sherif`, `syncpack lint` |
| Module graph | 10,000 TypeScript files in 100 packages (`--graph-files`, `--graph-packages`; `generate --source-files-per-package`) | `skif` |
| Pinned corpus | the `bench` repositories of `corpus.toml` | `skif`, `skif --no-js`, `sherif`, `syncpack lint`, `knip --reporter json` |

`sherif`, `syncpack` and `knip` are the versions pinned under `[tools]` in `corpus.toml`. `tools/package.json` and
`tools/package-lock.json` repeat them and pin every package they depend on; the harness refuses to load `corpus.toml`
when the lock's root dependencies or its installed `node_modules/<tool>` versions differ from `[tools]`. The pins stay
exact, at the latest releases: `skif-bench tools update` looks each tool up on the npm registry (`npm view <tool>
version`), rewrites `[tools]` and `tools/package.json` for every tool that is behind and runs `npm install
--package-lock-only --ignore-scripts --no-audit --no-fund` in `tools/`, printing `old -> new` per tool (a failed relock
restores all three files); `skif-bench tools check` exits non-zero and lists each tool that is `pinned X, latest Y` (a
registry failure is an error, not a pass). `run` copies the two files into `target/bench-generated/tools`,
installs them there once with `npm ci --ignore-scripts` (skipped when that directory already holds the byte-identical
lock and every executable) and times the executables in its `node_modules/.bin` directly, so no `npx` resolution is
timed and nothing on `PATH` is used. `parity`, `syncpack-diff` and `deadcode --knip` install and run the same locked
tools from `--tools-dir` (default `target/bench-generated/tools`, the directory `run` uses), so every command compares
against the same dependency versions; they fail when the tools cannot be installed. A tool that cannot be installed (no `npm`, a failed
install), or knip on a corpus repository whose install failed, is left out and recorded with the reason under
`skipped_tools`. Results go to `target/bench-results/` (or `--out`): `summary.md` (tables plus the ADR checks),
`summary.json` (schema 9: skif version, the commit of the skif source it was built from with a `-dirty` flag
(`--skif-source`, else the harness's own checkout) and the harness's commit, the machine record and the one line
`machine_text` that `summary.md` and the published tables print, the toolchain, the load average before and after the
run, hyperfine version, each comparison tool's `--version`, the corpus repositories of an `--only` run, every target
with its warmup and measured runs and, per tool, mean, stddev, median, min and max in ms and the number of runs that
exited with an error, on the row of the whole skif run `peak_rss_bytes`, and for an installed
corpus repository `installed_with`, the package manager and the `--version` it reports in the checkout (where corepack
resolves the repository's `packageManager`) or the repository's `install-command`, its `declared_knip_failure` and, on a
knip row that failed that way, `failure` (the reason, the error line and `timed`), the skipped tools, and the ADR 0001
checks, each `pass`, `fail`, `invalid` or `not-measured`), and one raw hyperfine export per target under `raw/`. The machine record holds the
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
when `matches` or `reason` is empty or the repository is not in the bench suite. A declaration with `timed = true` is
an error knip exits with after printing its whole report, because a tool configuration of the repository does not load
and knip goes without the entries it names (next-js's root `jest.config.js`, which requires the unbuilt `next/jest`;
two n8n Vite configurations): a run that matches it keeps its times and its ratio to skif, its check is `pass` or `FAIL`
as for a clean run with the reason beside it, and `summary.md` and the published tables give its time with the reason.
Such a time is that of a run without those entries. A corpus
repository's `bench-knip-args` in `corpus.toml` are passed to knip, so it reads the files skif reads (npm-cli sets
`--no-gitignore`). Every compared row is timed on the scope its comparison tool reads
([ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md)): `skif --no-js` against sherif and
syncpack, and the full `skif` against knip, run without `--recursive`, because a recursive run lints the nested roots
that are not workspace members, which none of those tools reads (next-js has 403). The full row would use
`--recursive` only where knip itself is configured to read such nested projects; no repository does. Where a repository
sets `recursive` (babel, vscode, next-js; `parity` runs it recursively), the run adds an informational
`skif --recursive` row: it has no ratio and no peak RSS, is in no ADR 0001 check and enters no compact table
or claim. A repository that sets `bench-knip-config = "parity"` (angular, vscode, n8n, next-js) times knip with
the configuration of the knip comparison instead of its own (`--config` to a file in `<generated-dir>/configs/<name>`
that is empty apart from the repository's `entry` files) and skif with a `--config` that adds the same `entry` files,
so both read what `parity` compares: n8n's own `knip.ts` checks dependencies only, and vscode's workbench is reached
only from the entries its bundler script names. A bench repository that sets `entry` has to set it. `bench-max-runs`
and `bench-max-warmup` cap hyperfine's measured and warmup runs on a repository below a higher `--runs` and `--warmup`;
the four large repositories run 5 times after 1 warmup run, since knip takes tens of seconds on each. `summary.md` and
the published tables name the run counts of a capped target, and an error count is out of the target's own runs.
`bench-knip-config`, `bench-max-runs` and `bench-max-warmup` are rejected outside the bench suite, and so is an unknown
configuration or a cap of 0 runs. hyperfine 1.x and 2.x exports are both read.

The whole run (`skif`) of each corpus and module graph target then runs once more under `/usr/bin/time` (`-v`, GNU
time, on Linux; `-l` on macOS) for its peak RSS; where neither is available the RSS is `null`. `summary.md` and the
published tables show it per target.

## Running

```sh
mise run bench            # build release skif, fetch the corpus, run everything
mise run bench:local      # as CI runs it: --install --no-enforce (numbers are not publishable)
mise run bench:tools:update  # update the pinned sherif, syncpack and knip (bench:tools:check verifies them)
mise run profile -- <repo-dir> [skif args...]  # samply profile plus SKIF_TIMINGS of one run (see "Profiling")
mise run bench:micro      # all criterion benchmarks (see "In-process benchmarks")
mise run bench:corpus     # only fetch the timed corpus repositories
mise run bench:import -- <run-id>  # import a bench workflow run into tools/bench/results and refresh the speed numbers
mise run parity:publish   # refresh the generated agreement table from baselines/parity.json
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
cargo run --release -p skif-bench -- tools update|check [--file corpus.toml]
```

`corpus fetch`, `run`, `parity`, `syncpack-diff` and `cache-diff` lock each corpus checkout (`<dir>/<name>.lock`, the
operating system's file lock, holding the process id) from the fetch to the end of their pass over it, so two runs never
install into, write into or time the same checkout at once: a second run stops at once and names the lock file and,
except on Windows, where the lock keeps other processes from reading the file, the process id it holds. A run that
dies releases its locks with the process, so there is no stale lock to remove. The corpus directory must be writable:
a lock file that cannot be created there is an error that says so. A cached checkout that is not a repository of its
own (a `.git` a cache restored empty, where git would act on the enclosing skif checkout) is re-initialised, and a
cached `origin` is reset to the URL in `corpus.toml`.

`generate` is deterministic for a seed: the same options produce byte-identical files. It writes a root `package.json`,
a lockfile stub (and `pnpm-workspace.yaml` for pnpm) so package manager detection works, and packages under `apps/*`,
`packages/*` and `packages/group/*`. Dependencies are drawn from a fixed pool of about 320 real npm names with a
per-name specifier (caret, tilde, exact, `npm:` alias, a few `github:`, `file:` and `link:`), plus `workspace:*` links
between packages. `--source-files-per-package N` adds `N` TypeScript modules under `src/` of every package: a chain of
modules in which each imports its predecessor, ending in the `src/index.ts` that the manifests name as `main`.
`--mismatch-rate` is the probability, per dependency or package, of each injected issue: a version mismatch, unsorted
`dependencies`, an `@types/*` package in `dependencies`, an empty `optionalDependencies`. The command prints the
injected counts as JSON.

## Profiling

`mise run profile -- <repo-dir> [skif args...]` builds `skif` with `cargo build --profile profiling` (release plus debug
symbols, no stripping), then records one run on `<repo-dir>` with [samply](https://github.com/mstange/samply)
(pinned in `mise.toml`) as `skif <repo-dir> --format json --no-install [args]`. It writes `reports/profile-<name>-<timestamp>.json.gz` (open it with
`samply load`) and, from `SKIF_TIMINGS=1`, `reports/timings-<name>-<timestamp>.txt`; `reports/` is untracked. Run it on
a corpus checkout in `target/bench-corpus` after `mise run bench:corpus`.

`SKIF_TIMINGS=1` prints one `skif timing: ...` line per phase to stderr. In the JS analysis of each root:

- `js <phase> <root>: N ms` for `git index`, `setup (plugins, configs)`, `discover`, `entries`, `resolver`,
  `graph (read, parse, resolve)`, `reached sources`, `config hints`, `exports`, `dependencies`, `ambient modules`,
  and `judged patterns`.
- `js rule <rule-id> <root> (summed over threads): N ms`, one line per enabled rule in rule-id order, for the time of
  its check inside the `provider js` phase.

Timings are off by default and cost nothing then.

## Parity with knip

`skif-bench parity` runs, for each repository of the `parity` suite in `corpus.toml`, `skif . --format json` and
the locked knip (`<tools-dir>/node_modules/.bin/knip --reporter json`, at the version pinned in `corpus.toml`) in the
checkout, and reduces both reports to finding keys
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
with sample keys per side is `target/bench-results/parity.json` and `parity.md`: five per category, known deviation
and side, or as many as `--samples <N>` asks for, and every one-sided finding with `--samples all`, which lists all the
files only one tool reports in a repository. Samples name files of the corpus repositories and are never committed.

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
`dist` its tool configurations import, then runs `@n8n/db`'s `gen:migration-index`, which writes the gitignored
migration indexes `src/index.ts` imports, so both tools see the migrations they list. The install marker records the install and build commands, so a checkout
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
configuration's entries, on skif's because knip's view of the package is incomplete. The knock-on effect in other
packages stays compared: a file of another package that only the failed configuration's entries reach is a difference.
The harness evaluates no configuration, so it cannot tell which files a failed one makes entries, and skif's report does
not say which entry reaches a file. In next-js, knip cannot load the root `jest.config.js` (it requires the built
`next/jest`), whose `roots` include `packages/next/src` and `packages/font/src`, so the `packages/next` files that only
root tests and `packages/font` tests reach are knip-only unused files in the counts.

**Invalid knip runs.** A repository whose knip run cannot be a valid comparison sets `knip-invalid` in `corpus.toml`
with the reason. It is still compared and recorded, and its status cell in `parity.md` (and `invalid` in the
baseline) carries the reason, so its counts are not read as agreement. No repository is marked at present.

**Findings inside unused files.** A tool analyses nothing inside a file it reports as unused, so a finding in such a
file follows from the `files` disagreement, which is counted once already. A finding only skif reports in a file knip
lists under `files` is the `in-knip-unused-file` known deviation, and a finding only knip reports in a file skif
reports unused is `in-skif-unused-file`; unused files themselves and the manifest categories (`dependencies`,
`devDependencies`, `optionalPeerDependencies`, `binaries`) stay compared. A dependency only knip reports unused is
`used-in-knip-unused-file` when a file of the same workspace that only knip lists under `files` imports the package
(an `@types/` package when it imports the runtime package), and when the installed manifest of a package so imported
names it in `peerDependencies`, as both tools credit the peers of a used package; a file both tools report unused
credits the package on neither side, so its dependencies stay compared: the harness parses those files with skif's parser
(`skif-js-syntax`) for their bare imports, requires, re-exports and dynamic imports. knip does not analyse such a
file, so it never sees the import skif credits; the harness does not check that this import is the one skif counts.
The mirror, a dependency only skif reports unused that a file of the same workspace only skif reports unused
imports, is `used-in-skif-unused-file`, from the same import scan. The same holds inside the files such a file
imports: an export, type, namespace export or duplicate only knip reports is `imported-by-knip-unused-file` when a
file only knip lists under `files` imports it by relative path, by a specifier the `paths` of its own TypeScript
configuration map (the nearest `tsconfig.json` or `jsconfig.json` with its `extends` and `baseUrl`, `@/lib/api`), or
by the name of a workspace package (mapped to the source of what its manifest publishes), by name, through a
namespace whose members it reads, or as a whole module (a namespace used as a whole, `import()`, `require`), directly
or through the `export … from` and `export *` re-exports of the files in between (a star re-export forwards only the
names the re-exporting file does not export itself). The mirror on skif's side is `imported-by-skif-unused-file`. An
enum or namespace member only knip reports is `imported-by-knip-unused-file` too when such a file reads it
(`BrevoNode.INTERCEPTORS`) or uses its export as a whole; on skif's side members stay compared.

**Labels.** `exports`, `types`, `nsExports` and `nsTypes` key the same `<file>|<name>`, and the tools label some
differently: knip calls a `const enum` a type and files an export under `nsExports` when any importer uses a namespace,
skif when all do. A skif finding knip has under another of these labels counts as shared, under knip's label. In the
same way an import only skif reports in `unlisted` that knip reports in `unresolved` on the same file, or the
reverse, counts as shared under knip's category and key: a tsconfig `types` entry or `importHelpers` is a package no
manifest lists for skif and a module knip cannot resolve. The two keys pair when they name the same package, a
specifier with a subpath (`vite/client`) pairing with its package (`vite`). An import only knip reports in
`unresolved` that skif reports as `stale-mocks` on the same file and specifier (`jest.requireActual('./x')` of a
module that does not exist, which skif reports under its own rule, ADR 0009) counts as shared too.

**Signature types.** skif counts an export that the signature of a used export of the same file mentions as used ([ADR
0009](../../adrs/0009-what-counts-as-a-use.md)):
`export interface Props` that `export function Widget(p: Props)` names. knip has no such rule. An export or type only
knip reports is the `signature-type` known deviation when skif's parser (`referenced_in`) finds it in the signature of
another export of the file that skif does not report as unused, directly or through exports in such signatures; the
harness parses only the files with such findings and takes an export skif does not report for a used one.

**Known deviations.** Findings on one side below a path in a repository's `knip-unread` list are the `knip-unread`
deviation: knip reads no files there, as npm's deny-all `.gitignore` hides its workspaces, so it reports every
dependency there as unused and says nothing about the files. In `files`, a file only knip reports is not counted as a
difference when it is outside skif's project set (`non-code-file`), a declaration file `.d.ts`, which skif never reports
unused (`declaration-file`; a `.d.mts` or `.d.cts` file stays compared), or a test file or any file below a test
directory (`test-file`). A finding only knip reports below a `fixtures` or `__fixtures__` directory is `fixture-file` in
the issue types skif does not report there: files, exports, types and members unless `[js] include-fixtures` is set, and
unresolved imports and unlisted dependencies at all (other test-data names such as `testdata` or `*-fixtures` are judged
by skif and not counted here). Those are reported in their own column and section, with the reason, and `--check`
ignores them. Most `test-file` findings are test files, which skif's test-runner plugins make entries, or test helpers
and setup files; in next.js thousands are the app files of test fixtures below test directories, which are not test
entries. The harness sets them apart by path, without establishing why skif does not report each one.
`--include-test-files` counts the `files` of `fixture-file` and `test-file` as differences (not usable with a baseline,
nor is `--repo-knip-config`). See [ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md).

**Foreign tsconfig paths.** knip resolves every import through the merged tsconfig `paths` of all workspaces; skif
uses the `paths` of the configuration the importing file follows, and reports an import that only another package's
`paths` map as `unlisted-dependencies` with the detail `ForeignTsconfigPaths`. A finding only skif reports in
`unlisted` that carries this detail is the `foreign-tsconfig-paths` known deviation: counted in the known-deviations
column and section, recorded in the baseline and ignored by `--check`. An `unlisted-dependencies` finding without the
detail stays a difference. The mirror on knip's side is `own-tsconfig-paths`: an import only knip reports in
`unlisted` that the `paths` of the importing file's own configuration (the nearest `tsconfig.json` or `jsconfig.json`
at or above it, with its `extends` chain, `baseUrl` and `${configDir}`) map to a file of the checkout outside
`node_modules`, which skif resolves to that source. An unlisted import a `declare module` of a script file of the
checkout declares (the specifier, its package, or a pattern with one `*`, as skif matches them) is `ambient-module`,
and a `data:` specifier is `data-uri`; skif takes neither for a dependency. See
[ADR 0009](../../adrs/0009-what-counts-as-a-use.md) and
[ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md).

**Where each tool looks.** knip's workspaces are the root and the members its patterns select (`pnpm-workspace.yaml`
or the `workspaces` of `package.json`); the harness reads them with skif's workspace discovery and records their
number in `parity.json` (`workspace_members`). A project directory is one whose `package.json` has a field besides
`type`, `sideEffects` and `imports`. Five more known deviations follow from these facts, each decided by path and
manifest, not by listing findings:

- `knip-unjudged-manifest`: an unused dependency or a script binary only skif reports, on a `package.json` that is not
  one of knip's workspaces and that knip reports nothing on. knip judges its workspaces' manifests only; skif's
  recursive run lints a nested project as a root of its own.
- `non-member-project`: a finding only knip reports in a file (an unused file, export, type or member, a duplicate, an
  unresolved import, or an unlisted dependency that the file's own manifest lists) below a project directory that is
  not one of knip's workspaces. knip counts those files to an enclosing workspace without their own manifest's entries
  and dependencies; skif's analysis of the enclosing workspace skips the directory. When a recursive skif run lints
  the project as a package of one of its roots (the `packages` of the JSON `roots`), its exports, types, members and
  duplicates stay compared, since skif judges them there, except the exports and types of the project's default entry
  files (`index`, `cli` or `main` at the top or in `src`) and of the entry files of skif's plugins there: they are
  public in skif's run of the project, and knip reads them without the project's entries; the same holds for the
  exports those entries re-export through `export … from` and `export *`, followed through the files in between.
  knip runs no plugin in a project that is not one of its workspaces, so the harness runs skif's plugin resolver
  (`skif_js_plugins::resolve_plugins`) on the project's manifest and files and takes the entry globs and code
  configuration files of the plugins it enables: the GraphQL Code Generator outputs a `codegen.ts` names (next.js
  `examples/with-typescript-graphql/lib/gql/graphql.ts`), or Mastra's `src/mastra/tools` and `src/mastra/workflows`
  in a template. On skif's side, an unused file, an unlisted
  dependency or an unlisted binary only skif reports below such a project is `non-member-project` too: the recursive
  run lints the project with its own entry files, plugins, configs and manifest (a package its `.eslintrc.json` or
  `project.json` names), which knip does not read there. A binary on a `package.json` that knip reports anything on
  stays compared. So is an export, type or duplicate only skif reports in a file of knip's workspaces that a file below
  such a project imports (the import scan of `imported-by-knip-unused-file`), and an enum or namespace member only skif
  reports there that such a file reads (`Phase.Start`, `api.Phase.Start`, through the re-exports in between) or whose
  enum or namespace it uses as a whole: skif's analysis of the workspace does not read the importer, and knip does.
- `skipped-directory`: a finding only knip reports in a file below a directory skif's file discovery skips by name
  (`node_modules`, `dist`, `.next`, `.vercel` and the other build output names, `out`, `target` and `coverage`
  directly in a package directory, and hidden directories other than tool ones such as `.github`), unless the
  directory is a workspace member. On skif's side, an export, type or duplicate only skif reports in a file of knip's
  workspaces that a file below such a directory imports (`.devcontainer/*.test.mjs`), and an enum or namespace member
  such a file uses as `non-member-project` describes, is `skipped-directory` too; the harness scans the hidden
  directories for such importers, none of the build output ones.
- `published-source`: an unused TypeScript file only knip reports whose path its package publishes with a built
  extension (`main`, `module`, `types`, `bin`, an `exports` leaf with `*` patterns, as `.js`, `.mjs`, `.cjs`, `.jsx` or
  a `.d.ts` variant) or without one (`./out/main`). A published path below a build directory (`dist`, `lib`, `out`,
  `esm` and the others of `skif_js::is_out_dir`, also with a flavour suffix such as `lib-commonjs`) also names the same
  path below `src`, as skif maps it: `./lib-commonjs/*.js` publishes every `src/**/*.ts`. A published path without `*`
  names one source only, the first of skif's candidates that exists (`src/x.ts` before `lib/x.ts` for `./lib/x.js`,
  `.ts` before `.tsx`), after mapping it through the package's build directories as skif's TypeScript plugin reads them:
  the path below the `outDir` of a `tsconfig.json`, `tsconfig.*.json` or `jsconfig.json` in the package directory is the
  same path below its `rootDir` (without one, the directory its `include` and `files` share), so `./dist/esm/index.js`
  with a `tsconfig.build.esm.json` of `outDir: "dist/esm"` and `rootDir: "src"` names `src/index.ts`; a configuration
  extended from elsewhere is not read. In an n8n node package the files of `n8n.nodes` and `n8n.credentials` are
  published too, a path below a build directory naming the path without it (`dist/nodes/A/A.node.js` names
  `nodes/A/A.node.ts`), as skif reads them (`skif_js::n8n_load_paths`). A file such a source imports by relative path
  (resolved with its extension, a TypeScript twin of a `.js` one, or the `index` file of a directory), directly or
  through other files only knip reports unused, is `published-source` too. skif makes the source of a published path an
  entry and uses what it imports; knip maps a published path to source only through a tsconfig `outDir` and does not
  read the n8n fields. A published `bin` stub without a code extension whose first line is a `#!` line naming `node`
  loads build output through its relative `require`s and imports (`require('../lib-commonjs/start.js')`), which maps
  to source the same way, as skif reads such a stub: that source and the files it reaches are `published-source` too.
  So are the sources a JavaScript or declaration file the manifest publishes as it is (`published-file`, named without
  `*`) loads by a relative specifier into build output, and what they reach: next.js publishes `client.js`
  (`module.exports = require('./dist/client/index')`) and `client.d.ts` (`export * from './dist/client/index'`),
  which name `src/client/index.tsx`. skif makes the published file an entry and resolves a specifier into output that
  was never built to its source; a file inside build output maps nothing.
  An export or type (also as a namespace export or type) only knip reports in a published source, or in a file whose
  exports a published source, or a declaration or JavaScript file the manifest publishes as it is, re-exports through
  `export … from` and `export *` (followed through the files in between), is `published-source` as well: an entry's
  exports are public in skif. A specifier of a workspace package's subpath into build output that was never built
  resolves to the source it is built from, so next.js's `server.d.ts`, which re-exports `next/dist/server/web/types`,
  credits `src/server/web/types.ts`. Enum and namespace members of these
  files, and the exports of the other files they reach, stay compared.
- `published-file`: an export or type only knip reports in a declaration file, or an unused file, export or type of a
  JavaScript file (`.js`, `.mjs`, `.cjs`, `.jsx`), that the manifest of its project publishes as it is: a published
  field, `bin` or `exports` leaf names its path (`*` matching any part of it), or an entry of `files` names it or
  matches it with `*` (next.js publishes `cache.d.ts` and `navigation.js` so). A directory listed in `files` is not
  taken to publish its files.
- `entry-module-duplicate`: a duplicate export only knip reports (`export const x …; export default x`) in a file skif
  makes an entry from the manifest and the defaults alone: a default entry file of its project (`index`, `cli` or
  `main` at the top or in `src`, matched as skif matches them) or a published source as in `published-source`.
  skif's `duplicate-exports` skips entry modules, whose names are all public API. Duplicates in entries that only
  skif's plugins or the configuration make stay compared.

Three known deviations on skif's side follow from how knip reads a file:

- `knip-dot-ignore`: a finding only skif reports (anything but a manifest finding) in a file that a `./` pattern of a
  `.gitignore` below the checkout root hides from knip. git matches no path with `./storage/`, since no path has a `.`
  segment; knip joins every pattern of a nested `.gitignore` to its directory, which drops the `./`, and ignores every
  file and directory named `storage` at any depth below it (mastra's `packages/core/.gitignore` hides
  `src/storage/**`). knip neither reports such a file unused nor analyses its exports or imports, so on knip's side
  a dependency of the same workspace such a file imports (with its peers), and an export or type such a file
  imports, are set apart as well: the import scans of `used-in-knip-unused-file` and `imported-by-knip-unused-file`
  over the hidden files skif does not report unused (mastra's `lru-cache`, `posthog-node` and `rollup`). The harness
  reads the `.gitignore` files its file scan reaches and takes only patterns without wildcards.
- `knip-node-modules-name`: a finding only skif reports (anything but a manifest finding) in a file whose path holds
  `node_modules` inside a name, below no `node_modules` directory (rules_js's `node_modules_in_sandbox.test.mjs`):
  knip skips every path that contains the text (`isInNodeModules` in its `util/path.js`).
- `re-export-alias`: a duplicate export only skif reports whose aliases (`export const b = a`) each name an export of
  the group initialised with an imported binding (`export const a = importedA`), no name being `default`. knip takes
  `export const a = importedA` for a re-export of the import and records no alias of a re-export, so it sees no
  duplicate; skif counts every name that holds the one value. The harness reads the aliases with skif's parser.

Two more follow from what knip makes an entry and how it credits a module object:

- `tsconfig-declaration-file`: knip makes every declaration file (`.d.ts`, `.d.mts`, `.d.cts`) that the
  `tsconfig.json` of one of its workspaces compiles a production entry, whose exports are public and whose imports it
  reads: the paths `files` lists and the files the `include` globs match (everything below the configuration's
  directory when neither is set), less the `exclude` globs, `node_modules`, the `outDir` and hidden directories, read
  through `extends` with each list relative to the configuration that sets it. skif judges a module declaration file
  like any module and makes it an entry only when something else does (ADR 0007). Set apart are the exports,
  types and members only skif reports in such a file (vscode's `extensions/git/src/api/git.d.ts`, cal-com's
  `packages/types/*.d.ts`); the unlisted dependencies and unresolved imports only knip reports in one that no code file
  imports (cal-com's `packages/types/AppGetServerSideProps.d.ts` and `next-auth`), as the import scan of
  `imported-by-knip-unused-file` resolves imports, a declaration file also named by its stem with or without a built
  extension when no source file takes the specifier; and the unused files only skif reports that such a file reaches by
  relative imports, directly or through other files only skif reports unused (the files angular's
  `*_isolated.golden.d.ts` compliance files import). An export or type only skif reports that such a file re-exports
  through `export … from` and `export *` is set apart too (next.js's `module.compiled.d.ts`, `export * from
  './module'`, makes the route modules' types public for knip).
- `module-object-types`: a type only skif reports in a file that an import in a code file of the checkout uses as a
  whole module object (`import()`, `require`, a namespace used as a whole), directly or through `export … from` and
  `export *` re-exports. knip records such a use as opaque and credits every export, types included; skif's whole use
  counts the module's values only (ADR 0009). cal-com's `crm.apps.generated.ts` maps each app to
  `import("./<app>/lib/CrmService")`, so the `CrmService` types are skif's only. Only the files that hold `import(`,
  `require(` or `* as ` are parsed.

More follow from what knip reads no use in, where skif reads one with its parser or plugins:

- `heft-config`: a dependency only knip reports unused that skif's `heft` plugin credits (ADR 0008): a project's
  `config/heft.json`, `config/rig.json` or `config/jest.config.json`, or a rig's profile (`profiles/<profile>/` of the
  rig itself, or of the installed rig a `config/rig.json` names) names it, or it is the peer of a package so named.
  knip has no Heft plugin. The harness runs skif's plugin resolver (`skif-js-plugins`) for each project that owns such
  a finding and takes what the `heft` plugin references, and what any plugin references from a rig profile's file
  (the `tslib` an `importHelpers` of a profile's `tsconfig-base.json` needs).
- `type-asserted-require`: an export or namespace export only knip reports in a file that a code file loads with a
  `require` whose result an `as` asserts a type of (`require('./x') as typeof import('./x')`, next.js's
  `stream-ops.ts`; the specifier may be a template literal without a substitution), or that such a file re-exports.
  knip does not follow a `require` wrapped in `as`. The harness finds such a `require` in the text and resolves it as
  the import scan of `imported-by-knip-unused-file` does.
- `module-object-values`: an export or namespace export only knip reports in a file that an import uses as a whole
  module object, as skif's parser reads it (n8n's `methods = { listSearch, loadOptions }`, a namespace spread or
  passed to `vi.mocked`, a `require` a callback returns, a named import of a namespace re-export that escapes:
  `export * as listSearch from './listSearch'` in `methods/index.ts`), or that such a file re-exports. skif credits
  every value export of such a module (ADR 0009); knip does not take these uses for whole ones.
- `commonjs-default-import`: an export or namespace export only knip reports in a CommonJS file (`module.exports`, no
  `default` export) that a default import reads as a property (`import routing from './ci-routing.cjs'` and then
  `const { a } = routing` in mastra's `.github/scripts`). skif binds the default import to `module.exports`; knip
  takes it for a use of `default`.
- `template-literal-import`: an unused file only knip reports that a `require` or `import()` of a template literal
  with a relative start matches (npm's ``require(`./commands/${c}`)`` in `lib/npm.js`), or that such a file reaches by
  relative imports through other files only knip reports unused. skif reads each substitution as a `*` within one
  path segment and matches the glob as its graph does; knip resolves no computed specifier.
- `installed-main`: an unused file only knip reports that the installed file the `main` of its project names below
  `node_modules` imports by relative path (`node_modules/expo/AppEntry.js` imports `../../App` in an Expo app), or that
  such a file reaches. skif makes the `main` an entry; knip reads nothing in `node_modules`.
- `type-referenced-members`: an enum or namespace member only skif reports whose enum, or namespace with exported
  members (a type merged with a namespace, vscode's `CompareResult`), a code file imports by name from the member's
  file without an alias and refers to as a type (`code: CharCode`, `typeof E`). knip skips the member findings of an
  export an importer refers to so while `nsTypes` is not reported (`getMemberIssues` in its `graph/analyze.js`);
  for skif a type reference reads no member. The harness reads the type references with oxc.
- `raw-glob-read`: an unused file only skif reports that an `import.meta.glob` reading the matched files as text
  matches (`import.meta.glob('../../**/*.vue', { query: '?raw' })` in n8n's `editor-ui`), or that such a file reaches.
  knip counts the glob as a use; skif reads the files' text and keeps none of them in use.
- `typescript-twin`: an unused JavaScript file only skif reports that lies beside a TypeScript file of the same stem
  and that a code file names with its extension (`require('../sum.js')` beside `sum.ts` in jest's
  `examples/typescript`). skif resolves the specifier in TypeScript's order, to the twin; knip to the file it names.

More follow from how knip reads a module where skif reads it otherwise. The harness reads the code with oxc, as knip
reads its own oxc tree, and with skif's parser for the imports; each credit is followed through the `export … from`
and `export *` re-exports of the files in between:

- `opaque-dynamic-import`: an export or type only skif reports in a file that an `import()` with a literal specifier
  loads which knip binds to no name (a bare `import('./x')`, one not awaited in a declarator, `vi.mock(import('./x'),
  …)` with a factory that is not a plain object literal). knip records it as opaque and credits every export, types
  included; skif reads the members the code takes from it (mastra's templates, cal-com's `trpc/react`, n8n's
  codemirror utils).
- `dynamic-import-default`: a `default` export only skif reports in a file that an `import()` knip binds to one name
  loads (`const m = await import('./x')`, `import('./x').then(m => …)`, one name of an awaited `Promise.all`). knip
  records an import of `default`; skif reads the members taken from the name (vscode's tsserver plugins).
- `jest-require-actual`: an export or type only skif reports in a file a `jest.requireActual` loads (also through the
  `jest` of `@jest/globals`). knip's Jest plugin records it as opaque; skif reads what the result is used for
  (next.js's `turbopack-utils`).
- `spied-namespace`: an export or type only skif reports in a file whose namespace imports only spy calls read
  (`vi.spyOn(ns, 'a')`, `jest.spyOn`, `spyOn`), no namespace import of it being read through a member elsewhere. skif
  reads a spy as a use of `a`; knip sees a reference of `ns` as a whole and, with `nsExports` and `nsTypes` off, skips
  the exports of a module whose namespace imports are all referenced so (`hasStrictlyNsReferences` in its
  `graph/analyze.js`): cal-com's `email-manager`, mastra's `ast-edit`, n8n's `nodeTypesUtils` and `GenericFunctions`.
- `enumerated-namespace`: an export or type only skif reports in a file whose namespace import's name an array literal
  holds or `Object.keys` (`values`, `entries`, `getOwnPropertyNames`) takes. knip marks the namespace opaque without
  checking for a shadowing name: vscode's `git.ts` passes a `path` parameter in `['ls-tree', path]` beside an
  `import * as path`.
- `namespace-re-export`: an export or namespace export only skif reports that its file declares as `export * as X
  from` (of any file, the file itself included). knip records it as a re-export of the source module and never lists
  it among the file's own exports (opencode's namespaces, vscode's `icons`).
- `typescript-commonjs-export`: an export or namespace export only skif reports that a `CommonJS` assignment
  (`exports.x = …`, `module.exports.x = …`, `module.exports = { a, b }`) of a file that is no JavaScript file
  exports. knip reads `CommonJS` exports in `.js`, `.mjs`, `.cjs` and `.jsx` files only (next.js's `join-function.ts`).
- `node-test-entry`: an export or type only skif reports in a file knip's `node` plugin makes an entry: the plugin is
  on in every workspace when a root script runs `node --test` (also `nub --test`), else in a workspace whose own
  script does, and matches `**/*{.,-,_}test.<ext>`, `**/test-*.<ext>`, `**/test.<ext>` and `**/test/**/*.<ext>`
  (n8n's test helpers and setup files).
- `ng-package-entry`: an export or type only knip reports that an entry point an `ng_package` of a `rules_js`
  repository names re-exports, or that lies in one: the `index.ts` of each Bazel package its `deps` name inside the
  `BUILD` file's npm package, as skif's Bazel plugin makes them entries (angular's packages, whose manifest `exports`
  list only schematics). knip has no Bazel plugin.
- `import-type-qualifier`: a type or export only knip reports that a qualified import type names
  (`import('./x').Name`, also `.Name.Member`). skif's parser reads a use of `Name`; knip records an import of the
  module for its side effects (vscode's `electron-browser` types).

More follow from how knip reads a dependency, an import or a command where skif reads it otherwise. Each one mirrors
a rule of knip 6 and is decided from the files the harness reads (skif's parser, the manifests and installed
manifests, git's ignore rules and the `BUILD` outputs), not from a list of findings:

- `module-augmentation`: an unlisted dependency only knip reports that a `declare module 'x'` of a module file
  augments (jest's `expect`). knip records the augmentation as an import; skif does not.
- `jest-mock-specifier`: an unlisted dependency only knip reports that the first argument of a `jest.mock`,
  `jest.doMock`, `jest.unmock`, `jest.dontMock`, `jest.requireActual`, `jest.requireMock` or
  `jest.createMockFromModule` names (cal-com's `googleapis`). knip's Jest visitor records the specifier; skif reads a
  mock as no use of a dependency.
- `nextra-resolve-alias`: an unlisted dependency or unresolved import only knip reports in a `next.config.*` that
  imports `nextra` and does not hold the specifier (cal-com's docs). knip's Next.js plugin reads the
  `turbopack.resolveAlias` nextra adds.
- `expo-updates`: `expo-updates` unlisted only by knip at an Expo app configuration (`app.json`, `app.config.*`).
  knip's Expo plugin names it for every app that does not set `updates.enabled` to `false`.
- `shared-config-reference`: an unresolved import of a package only knip reports in a file that does not hold the
  specifier and imports a package whose manifest lists it (n8n's `@n8n/stylelint-config`); knip executes the
  configuration and records what the shared one loads.
- `shell-template-directory`: an unresolved relative import only knip reports that a Bun shell template
  (`` $` ``) of the file runs and that resolves from the file's project directory, where the command runs, and not
  from the file's own (opencode's scripts).
- `nested-vitest-setup`: an unresolved import only knip reports in a Vitest or Vite configuration that a nested
  configuration below it quotes and resolves from its own directory (mastra's packages).
- `gitignored-import`: an unresolved import only knip reports of a relative file that does not exist and that git
  ignores (cal-com's generated `packages/kysely/types.ts`). skif's resolver takes such a target for generated code.
- `passthrough-command`: an unlisted binary only knip reports that a command skif passes through names (`watch`).
- `bun-file-argument`: an unlisted binary only knip reports that is a file argument of `bun` (opencode's
  `imports.mjs`).
- `command-directory`: an unlisted binary only knip reports of a command that runs in another directory: after `cd`,
  with `pnpm -C`, `--dir` or `--dir=`, or under a workflow step's `working-directory`, or one whose directory is
  computed (`$…`, absolute or above the root). knip reads a step's `working-directory` and no job or workflow
  `defaults`; skif reads both and resolves the binary where the command runs.
- `optional-peer-host`: an unlisted dependency only skif reports that a workspace package at or above the file names
  in `peerDependencies` with `peerDependenciesMeta.<name>.optional` true (n8n's `@vitest/coverage-v8`, cal-com's
  `jsdom`). knip's dependency deputy counts a host's peers as present.
- `required-child-process`: an unlisted binary only skif reports in a code file that loads `child_process` with
  `require` and imports it statically nowhere (cal-com's `sentry-cli`, vscode's `sqlite3`). knip reads the commands of
  `child_process` from static imports only.

These widen deviations above: `non-member-project` also covers an unlisted dependency in a file inside one of knip's
members, or that a manifest between the file and its workspace lists; `data-uri` also covers the name knip cuts a
`data:` specifier to (`data`); `own-tsconfig-paths` also covers a `paths` target that does not exist and that git
ignores (vscode's `.build/typings`); `heft-config` also covers an unresolved import of a rig profile; and
`bazel-build-file` also covers an unresolved import of a file a `BUILD` file's `outs`, `out` or `stdout` produces.

`non-member-project`, `published-source`, `published-file` and `entry-module-duplicate` check where a finding is, not
why skif has no finding there. See
[ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md).

**Bazel.** skif's Bazel plugin reads the `BUILD.bazel` and `BUILD` files of a repository that uses `rules_js`; knip
has no Bazel plugin. An unlisted dependency only skif reports at a `BUILD` file (a package a label names that no
manifest lists) is the `bazel-build-file` known deviation, and so is a dependency only knip reports unused that a label
of a `BUILD` file in the package's directory names: a `node_modules` target of the package's Bazel package
(`":node_modules/zod"`, `"//:node_modules/typescript"` for the root package) or the `package_json.bzl` of another
repository (`"@npm//packages/app:esbuild/package_json.bzl"`). skif credits a peer dependency of a used package, so a
dependency the package lists that the installed `package.json` of such a credited dependency (the nearest
`node_modules/<name>` at or above the package) names in `peerDependencies`, optional or not, is `bazel-build-file` too,
and so are its own peers in turn; an `@types/` dependency counts when the package it types is such a peer. The harness
scans the string literals of every `BUILD`
file below a directory skif's file discovery enters (a `coverage`, `out` or `target` directory is skipped only
directly in the root or a package directory), in a checkout whose root holds `MODULE.bazel`, `REPO.bazel`,
`WORKSPACE.bazel` or `WORKSPACE`, resolving each label's Bazel package from the nearest of these root files that names
`aspect_rules_js`, as skif does. An unused file only knip reports is `bazel-build-file` too when the `srcs` or `data`
of a `js_library`, `ts_project` or `ng_project`, or the `entry_point` or `data` of a `js_binary`, `js_run_devserver`
or `js_test`, name it in the `BUILD` file's package (a literal of a top-level call, `"src/a.ts"` or `":src/a.ts"`, not
one inside a nested `select`, as skif's plugin reads them), when the `srcs` or `data` of a test target (`js_test`, or
any of these rules with `testonly = True`) add a `glob(...)` (also `native.glob`) whose include patterns match it below
the `BUILD` file's directory (its `exclude` patterns are not applied, as skif applies none,
[ADR 0011](../../adrs/0011-nx-lerna-and-bazel.md)), or when such a file reaches it by relative
imports through other files only knip reports unused. The `glob` of a target that is no test names nothing. Their exports stay compared: skif judges the exports of `srcs` and `data` files;
only an `entry_point` is an entry whose exports are public.

**Not judged by skif.** skif lists under `incomplete-packages` in its JSON report the packages whose entry files it
cannot all know (a tool config it cannot read), and does not report their unused dependencies, exports, types and
members, nor the files the config could make entries. A finding only knip reports in `dependencies` or
`devDependencies` whose package (the deepest directory above it with a `package.json`) is one of those is counted as
`not-judged-by-skif`, and so is one in `files`, `exports`, `types`, `nsExports`, `nsTypes`, `enumMembers` or
`namespaceMembers` in a file skif left unjudged: any file of the package when its entry's `scope` is `"package"`, only
a file its globs match (relative to the package directory, matched as skif matches them) when the scope is
`{"within": [...]}` ([ADR 0008](../../adrs/0008-plugins-config-evaluation-and-commands.md)); a package listed twice gets
the union of its scopes. A narrowed scope also covers the files of the packages nested below its package that its
globs match relative to that package (a root scope of `packages/*/e2e/**` reaches `packages/web/e2e/run.ts`), as skif
makes each of them an entry; the dependencies of a nested package are not judged only when skif lists that package
itself. These are counted in their own column of `parity.md`, apart from the agreement counts and the
other known deviations, and ignored by `--check`. `unlisted`, `unresolved`, `binaries` and `duplicates` are judged in
every package and stay compared. `parity.json` lists the directories per repository under `not_judged_packages`, and
the globs of the narrowed ones under `narrowed_packages`. See
[ADR 0012](../../adrs/0012-cli-contract-and-reporting.md) and [ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md).

**More layouts (ADR 0014).** These known deviations are decided from the finding key, the manifests, the
configuration files and an import scan of the checkout's code files (every directory but the build output and
`node_modules` ones, hidden directories included), with skif's parser:

- `rush-provided-command`: `rush`, `rushx` or `rush-pnpm` only knip reports as an unlisted binary in a repository with
  a `rush.json`; Rush provides them to every script (ADR 0004).
- `shell-builtin` and `global-binary`: a binary only knip reports that is a shell builtin (`read`, `ulimit`), or a
  system or machine tool on skif's list (`IGNORED_BINARIES` and `MACHINE_BINARIES` of `skif-js-plugins`, which the
  harness reads rather than copies, ADR 0008).
- `replaced-knip-config`: an unused file only knip reports that is a knip configuration (`knip.ts`,
  `knip.config.js`, ...) at the root of one of its workspaces, while knip ran with the harness's empty configuration;
  skif's knip plugin makes it an entry.
- `whole-enum-use`: an enum member only knip reports whose enum skif's parser finds used as a whole value, which ADR
  0009 counts as a use of every member: in its own file (`Object.keys(E)`, `E[key]`, `typeof E`), or in a code file
  of the checkout through a named import used so (`@IsEnum(E)`, `{ enum: E }`), a namespace member used so
  (`{ KeyCode: ns.KeyCode }`), a namespace used as a whole, `import()` or `require`, directly or through re-exports,
  resolved as the import scan of `imported-by-knip-unused-file` resolves them. Only the files that name one of the
  enums are parsed. The enum-member counts of the agreement table are read with these set apart.
- `paths-before-packages`: a dependency only skif reports unused, or a file only knip reports unused, where a file of
  the workspace imports the dependency's package by a specifier its configuration's `paths` map to that file of the
  same workspace, which exists: skif resolves through `paths` before packages, as TypeScript does, and counts the
  import for the file (ADR 0007); knip credits the dependency. A mapping into another member or into `node_modules`
  stays compared.
- `uninstalled-project`: a dependency, binary, unlisted dependency or unresolved import one tool reports in a project
  without installed packages at or above it (no `node_modules` directory or `.pnp.cjs`), in a checkout the harness
  installed: the turborepo-examples root, which its install command leaves out because its workspaces lie outside the
  sparse checkout.

**Counted, explained.** Some differences have a known cause but no mechanical test the harness can run cheaply, so they
stay counted as differences. knip has no esbuild plugin, so the `entryPoints` of an esbuild script and the files they
reach are unused files and exports for knip only. Exports of a nested root's plugin entries (a next.js page's
`getStaticProps`) stay compared too: only the default entries of a nested root and what they re-export are set apart.
Enum and namespace members whose only users lie in a non-member project or a skipped directory stay compared: their
keys name the member, not the export an import reads.

## Cache diff

`skif-bench cache-diff` is the correctness gate of skif's cache ([ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md),
"Cache rows and the cache-diff gate"). For every repository of the `bench` suite it
runs the binary under test with `. --format json --no-install` (plus `--recursive` where the repository sets it):

1. without the cache, then with an empty store (cold, writes it), then again on that store (warm);
2. for each of `--edits` seeded source edits, applied one at a time: once without the cache and once with the store the
   earlier runs left, on the same edited tree;
3. once more with the store after the last edit is restored.

Each cached run is compared with the uncached run of the same tree: the whole JSON documents and exit codes, except the
`skif` version stamp and the `cache` and `timing` sections (counters a cached run may add). Each comparison prints
`identical`, `DIFFERENT` with the sections that differ and the findings only one side reports (`+` the cached run, `-`
the uncached one), or `FAILED: <reason>` (a run timed out or printed no report, the cold
run wrote no store, the warm run shows no use of the cache, the uncached run counts cache hits, or no file accepts an
edit kind). Each edit line also says whether the edit changed the uncached report. The exit code is non-zero when any
comparison is not identical or none was made.

Identical reports prove nothing when no run used a cache, so the warm run must show that it did. The gate sums every
`hits` counter below the report's `cache` section (the counters are per namespace) and fails the warm comparison on a
sum of zero. A report without the counter fails too when the binary's cache is on by default; from a binary with the
opt-in cache, which reports no counters, the gate accepts the store the cold run wrote and says so on the warm line.
`--require-counters` fails that case as well.

```sh
mise run cache:diff [--only babel] [--edits 5] [--control env] [--require-counters]
cargo run --release -p skif-bench -- cache-diff [--skif target/release/skif] [--only babel,vue-core] [--edits 5] [--seed 1] [--control flags|env] [--require-counters] [--corpus-dir DIR] [--scratch target/bench-cache-diff] [--timeout-secs 900]
```

It probes `skif --help` and prints which cache command line it used: the opt-in one (no flag uncached,
`--cache --cache-location DIR` cached) or, when the help offers `--no-cache`, the default-on one (`--no-cache` uncached,
`--cache-location DIR` cached). With `--control env` it passes no cache flag and sets `SKIF_CACHE=0` uncached and
`SKIF_CACHE=1` with `SKIF_CACHE_DIR=DIR` cached, which only a default-on binary reads; it refuses a binary with the
opt-in cache. Either way `SKIF_CACHE`, `SKIF_CACHE_DIR` and `SKIF_CACHE_MAX_SIZE` of the caller's environment never
reach a run. The stores live under `<scratch>/store/<repo>`, outside the checkout, and the
captured output beside them. The checkouts are used as they are, installed or not.

The edits are planned from `--seed` and the repository name, so a run is repeatable. They change tracked `.ts`, `.tsx`,
`.mts`, `.cts`, `.js`, `.jsx`, `.mjs` and `.cjs` files (not declaration files, nothing over 1 MiB) and rotate through
five kinds: append a comment, drop the `export` keyword of the first exported declaration, add an unused class property
or enum member, add a side-effect import of a sibling file, and append a `declare global` block (TypeScript modules).
Only a file without uncommitted changes is edited. Before the edit the gate writes a journal, `<name>.edit` beside the
checkout, naming the file; after the two runs it restores the file with `git checkout -- <path>`, checks it byte for
byte against the original (writing the original bytes back and failing when it differs) and removes the journal. An
error between edit and restore restores the file too. A run that is killed (Ctrl-C, a cancelled job) cannot restore in
process; the next skif-bench command that locks the checkout restores the file the journal names, and `cache-diff`
refuses to edit while a journal is there.

After the comparisons it prints, per repository, the wall time of the uncached, cold, warm and first-edit
cached runs and the store size after the cold run. These are single runs, labelled informational: they answer roughly
what a warm cache saves, and are not a benchmark or a published row.

The gate is run by hand before merging a change to the cache; no CI workflow runs it yet.

## Policy diff against syncpack

`skif-bench syncpack-diff` compares skif's policy rules (`version-mismatch`, `semver-range-mismatch`,
`banned-dependency`, `invalid-local-version`, `policy-conflict`, `unresolved-catalog-references` and the catalog rules)
with the locked syncpack (`<tools-dir>/node_modules/.bin/syncpack json`). syncpack has no JSON lint reporter, but `json` prints every dependency instance with its
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
one-sided is unexplained. See [ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md).

## Baselines and the drift check

`baselines/parity.json` and `baselines/syncpack.json` hold the counts of a run (skif only, tool only, both, per issue
type or status code and repository, plus the case summary of the fixtures) and the tool version they were recorded
with. They contain no file, package or symbol names. `baselines/parity.json` also records the set of compared categories and known
deviations (`category_set`) and, per repository, whether its checkout was installed, whether skif ran recursively and
the skif source commit (`skif_rev`, informative only). After a deliberate change, or a
bump of a pin in `corpus.toml` (`skif-bench tools update` changes the pins but re-records no baseline), refresh them:

```sh
mise run parity -- --install --write-baseline
mise run diff:syncpack -- --write-baseline
```

Or record them on CI, where every package manager is installed: `gh workflow run corpus.yml -f record=true` (optionally
`-f only=NAME,...`) uploads the rewritten files as the `corpus-baselines` artifact, to review and commit.
`scripts/check-baseline-tool-versions.sh` (in `mise run consistency` and CI) fails when `parity.json` or
`syncpack.json` records another knip or syncpack version than `[tools]` pins, so a pin bump without a re-record is
caught.

`--check` compares a run with the baseline and exits non-zero when a count moves by more than
`--tolerance-percent` (default 10) of its baseline value or `--tolerance-abs` (default 5), whichever is larger, when a
repository changes between compared and skipped, or when a fixture that agreed before now differs. It refuses, with
one message instead of a list of drifts, a baseline recorded with another tool version or another
category set (an older harness, or one that sets apart other findings, such as the one that ran skif in two analysis
modes and set `type-resolved` apart). It refuses a repository whose checkout is installed
differently, or whose skif ran with another `--recursive` mode, from when it was recorded. A run narrowed with
`--only` is checked against those entries alone, and `--write-baseline --only` updates only them, so the baseline can
be refreshed one repository at a time (install, run, then delete the checkout's `node_modules`) where the disk does
not hold every install at once; entries recorded with another tool version or category set are dropped on write, not
merged. The scheduled `corpus` workflow (`.github/workflows/corpus.yml`, weekly and on demand, never on pull requests)
runs both checks (or, with `record`, records), writes the tables and any drift to the job summary and uploads the full
reports as `corpus-reports`. It runs `skif-bench tools check` first but fails the job for a stale pin only after both
checks, so a new upstream release does not hide drift against the pinned one. The decision is
[ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md).

## Measured numbers

The published numbers are on the [benchmarks page](https://polylint.github.io/skif/guides/benchmarks/), with the
methodology; the README and the comparison guide carry headline claims generated from the same run. They come from the
`bench` workflow (`.github/workflows/bench.yml`), which runs only when dispatched, on a pinned GitHub-hosted runner
image ([ADR 0014](../../adrs/0014-benchmarks-and-the-comparison-harness.md)):

```sh
gh workflow run bench.yml -f ref=v0.7.0 -f runner=ubuntu-24.04   # also: -f runs=15 -f warmup=3 -f only=babel
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
commit SHA or a comparison tool not to an exact version, and `skif-bench tools check` fails the run before that when a
pinned tool is behind its latest npm release.

`mise run bench:import -- <run-id>` (`scripts/bench-import.sh`, needs `gh`, `git`, `jq` and Node) imports only a
successful `workflow_dispatch` run of `bench` whose commit is on `origin/main` (it runs `git fetch origin main` first)
and whose summary names that commit as the harness's; it refuses any other run, naming its branch. It downloads that
artifact, refreshes the speed tables of the benchmarks page (`website/src/content/docs/guides/benchmarks.mdx`) and the
speed claims (`bench:claims`) of the comparison guide and the README, where each claim links to the benchmarks page,
with `refresh-numbers.mjs --bench`,
then puts the summary into `tools/bench/results` for a commit made locally. The claims give, for each tool, the range of
the ratios of its median to skif's over the targets it was timed on without a failure, with the median ratio from three
targets on, each rounded down to one decimal; a knip that fails as declared is left out and named, and one that exits
with a declared error after its report is counted and named. The first line of the
speed tables names the skif version, source commit and the `rustc` that built it, the comparison tool versions, the
harness commit when it differs, the runner image and the machine, its load, the date and the workflow run attempt.
`refresh-numbers.mjs` writes nothing and exits non-zero when a comparison tool was not timed, a corpus repository has no
row for one of the tools, the run was narrowed with `only`, a tool exited with an error in a measured run that is not a
declared knip failure the run matched, an ADR 0001 check is `invalid`, the summary is of an older schema, its tool
versions differ from the `[tools]` pins of `corpus.toml`, it records no GitHub-hosted runner or no workflow run, or the
measured or the harness checkout had local changes (`-dirty`); the import then changes nothing either.
`mise run parity:publish` (`refresh-numbers.mjs --parity`) writes the agreement table of the benchmarks page from
`baselines/parity.json` the same way, and refuses a baseline recorded by an older harness.
`node --test tools/bench/refresh-numbers.test.mjs` tests both. Measure the release commit after `release-bump`,
or the skif version names the previous release. Without flags, `refresh-numbers.mjs` refreshes both and leaves the speed
blocks as they are while `tools/bench/results` holds no imported summary.

GitHub-hosted runners are shared virtual machines: a neighbour's load moves the times of a run by several percent, and
two runs of the same image may land on different CPU models (the summary names the one it got). The tables show
medians, which one slow run does not move, over 15 runs after 3 warmup runs (5 runs after 1 warmup run on the
repositories whose `bench-max-runs` and `bench-max-warmup` cap them); compare runs on the same runner image and
CPU model, and read the ratios against knip, sherif and syncpack, which share the machine and its noise, rather than
the absolute times. `mise run bench` stays for local measurement while developing; its numbers are not published.

A summary in which a tool exited with an error is not published, unless the error is a knip failure `corpus.toml`
declares and the run matched, which is published as `fails: <reason>` without a ratio: knip exits with 2 when some of
the repository's tool configurations do not load, and its time is then that of a partial analysis. A declared failure
with `timed = true` is published with its time, its ratio and the reason, since knip printed its whole report before
the error. The run's log and
`error_output` name the cause, which is fixed or declared in `corpus.toml` before measuring again. On npm-cli knip runs
with `--no-gitignore` (the repository's `bench-knip-args` in `corpus.toml`): npm's deny-all `.gitignore` otherwise hides
the workspaces from knip, which then analyses only the root package.

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
`skif . --format json --no-install` and, with `--knip`, the knip
pinned in `corpus.toml` with an empty configuration (as `parity` does; the locked knip is installed with npm, which
needs the npm registry). The reports are reduced to the parity keys, plus skif's `unused-class-members` findings as
`classMembers` keys `<file>|<Class>.<member>` (knip 6 has no class-member check), and each run gets precision (seeded
findings among those reported) and recall (reported findings among those seeded) per issue type, for the member types
(`classMembers`, `enumMembers`, `namespaceMembers`), for the rest (`base`, the whole key before the member cases were
added) and overall. `nsExports` and `nsTypes` count as `exports` and `types`. A finding in a type the key does not seed
(`unlisted`, `unresolved`, `binaries`, ...) is false by construction. The report goes to stdout (`--json` for
JSON) and to `<out>/deadcode.{md,json}`, with every false finding and miss listed. A rerun replaces `<out>/repo` only when `<out>`
holds the `answer-key.json` of an earlier run; `--force` replaces it regardless.

Every package holds the dead code: unused dependencies and devDependencies (plain runtime libraries with no knip plugin
or skif tool mapping), unused files (one standalone, one that imports live code), unused exports and types in a
module the entry imports, an unused export behind a re-export chain, and unused members: `Ledger.archive`,
`Square.area`, `Frame.depth` and `Dog.fetch` of classes, `Mode.Legacy`, `Tone.Neon` and `Size.Huge` of enums and
`Geo.pole` of a namespace, in `src/members/`. A member is dead when deleting it still type-checks and no executed code
reads it. Next to the dead code sits live code reached only through a pattern that has caused false findings on real
repositories, or that matching members by name can get wrong. A false finding that names that code is attributed to
its pattern:

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
| `member-same-name` | `Circle.area`, read as `shape.area()` with `shape: Circle`, while the dead `Square.area` shares its name |
| `member-interface` | `Badge.label`, read only through an interface the class satisfies without `implements` |
| `member-generic` | `Parcel.weight`, read only as `item.weight` with `item: T` and `T extends HasWeight` |
| `member-destructure` | `Frame.width`, read only by destructuring an instance |
| `member-computed-literal` | `Frame.height`, read only as `frame[axis]` with `axis: "height"` |
| `member-inherited` | `Animal.sleep`, read only through a `Dog` instance |
| `member-override` | `Animal.speak` and its override `Dog.speak`, read only as `animal.speak()` with `animal: Animal` |
| `enum-typed-alias` | `Tone.Cool`, read only through `const palette: typeof Tone = Tone` |
| `enum-keyof-typeof` | `Size.Large`, read only as `Size[largest]` with `largest: keyof typeof Size` set to `"Large"` |
| `namespace-aliased-import` | `Geo.distance` through a renamed import, `Geo.origin` through `import origin = Maps.origin` |
| `member-cross-package` | class, enum and namespace members the entry re-exports and only the next package reads |

The workspace is not installed, so nothing depends on `node_modules`: the only `package.json` script runs `node`, the
`vercel.json` command runs a listed devDependency named like its binary, and every import of a dependency names a listed
package. Every package has a `tsconfig.json`.
The `skif-bench` unit tests run skif in process on the same workspace (two seeds, no binary, network or Node) and
fail when it misses a seeded finding other than the known ones they list, or reports a false one. CI exercises neither the `skif` binary invocation nor
the knip side: `skif-bench` cannot depend on the `skif` binary in its tests, and knip needs Node and the npm registry.
Run `deadcode` with `--knip` by hand to check them.

Default run (`--packages 5 --seed 1`: 73 seeded findings, 209 live items), measured on 2026-11-17 with skif 0.8.0
(commit `4d3bdd7f`) and knip 6.45.1; rerun with `mise run bench:deadcode -- --knip` to refresh it. Precision and recall
per run, `-` where nothing was reported:

| Issue type | Seeded | skif | knip |
| --- | ---: | --- | --- |
| `classMembers` | 20 | 1.000, 0.750 | -, 0.000 (no check) |
| `dependencies` | 4 | 1.000, 1.000 | 1.000, 1.000 |
| `devDependencies` | 5 | 1.000, 1.000 | 0.500, 1.000 |
| `enumMembers` | 15 | 1.000, 0.333 | 1.000, 0.333 |
| `exports` | 15 | 1.000, 1.000 | 0.750, 1.000 |
| `files` | 6 | 1.000, 1.000 | 0.545, 1.000 |
| `namespaceMembers` | 5 | 1.000, 1.000 | 1.000, 1.000 |
| `types` | 3 | 1.000, 1.000 | 1.000, 1.000 |
| base | 33 | 1.000, 1.000 | 0.688, 1.000 |
| members | 40 | 1.000, 0.625 | 1.000, 0.250 |
| All | 73 | 1.000, 0.795 | 0.741, 0.589 |

skif finds every seeded non-member finding and reports nothing else there: it does not report files in
`fixtures` directories (ADR 0007), credits the export `typeof import("./typed")["settings"]` names and the
devDependency `vercel.json` runs. knip's false findings are those five exports (`typeof-import`), the five fixture
files (`fixture-dir`) and the five `dc-build-tool` devDependencies (`vercel-json-command`).

On members, skif misses `Square.area` (`shape.area()` on a `Circle` reads the name) and reports nothing live. skif and
knip both miss `Tone.Neon` and `Size.Huge`: assigning the enum to a variable and indexing it with a `keyof typeof` key
use it as a whole (ADR 0009). knip has no class-member check. The
patterns are synthetic and cover only these cases, so the numbers say which known traps a tool falls into, not how
precise it is on a real repository.

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
  with `bench-knip-failure` and knip's row is published as failing, or, with `timed = true` where knip prints its
  whole report first, with its time and the reason.
- hyperfine runs with `--ignore-failure`, since linters exit 1 when they find something; an exit above 1 is still
  flagged as an error, as "What it measures" describes.
- The skif command line is `skif . --format json --no-install [--no-js] [--config <entry files>]`, plus `--recursive`
  on the informational `skif --recursive` row alone (ADR 0014); the working directory is the target.
- Wall-clock times include process start-up. For sub-10 ms runs hyperfine warns about short commands; trust the
  mean over many `--runs`.
- The corpus pins live in `corpus.toml` (full commit SHAs, the knip, syncpack and sherif versions, the suites of each
  repository). Repositories are fetched shallow with `git fetch --depth 1 origin <sha>` (plus a cone sparse checkout
  where `sparse` is set), so `fetch` is idempotent and cheap to repeat. Bump a repository pin deliberately; tool pins
  move with `skif-bench tools update` (check with `tools check`). Refresh the baselines (parity, syncpack)
  in the same change.
