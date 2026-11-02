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
| Pinned corpus | real monorepos from `corpus.toml` | `skif`, `skif --no-js`, `sherif`, `syncpack lint`, `knip --reporter json` |

`sherif`, `syncpack` and `knip` are optional: a tool that is not on `PATH` is skipped with a note. Results go to
`target/bench-results/`: `summary.md` (tables plus the ADR checks), `summary.json`, and one raw hyperfine export per
target under `raw/`.

## Targets (ADR 0001)

| Workload | Target |
| --- | --- |
| Manifest and policy rules, 500 packages | < 50 ms wall clock |
| JS analysis on the corpus repositories | at least 5x faster than `knip` on the same tree |
| Parse and build the module graph, 10,000 files | < 2 s |

`summary.md` ends with a pass/fail line for the first two. The third is measured by the `js_analysis` criterion
bench. The `skif` row on the corpus includes JS analysis and the `skif --no-js` row does not, so the difference is
the cost of the analysis; the knip comparison needs the corpus checkouts to have their dependencies installed.

## Running

```sh
mise run bench            # build release skif, fetch the corpus, run everything
mise run bench:micro      # all criterion benchmarks (see "In-process benchmarks")
mise run bench:corpus     # only fetch the pinned corpus
mise run bench:generate -- --packages 500 --out target/bench-generated/demo
```

Directly:

```sh
cargo run --release -p skif-bench -- generate --packages 500 --out DIR [--seed S] [--pm pnpm|npm|yarn|bun] [--catalog] [--mismatch-rate F] [--force]
cargo run --release -p skif-bench -- corpus fetch [--file corpus.toml] [--dir target/bench-corpus]
cargo run --release -p skif-bench -- run [--skif PATH] [--sizes 50,500,2000] [--pm pnpm] [--no-corpus] [--warmup 3] [--runs 10] [--skif-arg ARG]...
```

`generate` is deterministic for a seed: the same options produce byte-identical files. It writes a root
`package.json`, a lockfile stub (and `pnpm-workspace.yaml` for pnpm) so package manager detection works, and
packages under `apps/*`, `packages/*` and `packages/group/*`. Dependencies are drawn from a fixed pool of about 320
real npm names with a per-name specifier (caret, tilde, exact, `npm:` alias, a few `github:`, `file:` and `link:`),
plus `workspace:*` links between packages. `--mismatch-rate` is the probability, per dependency or package, of each
injected issue: a version mismatch, unsorted `dependencies`, an `@types/*` package in `dependencies`, an empty
`optionalDependencies`. The command prints the injected counts as JSON.

Profile with `cargo build --profile profiling` (release plus debug symbols, no stripping).

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
  `pnpm install` (or the matching package manager) in a corpus checkout before comparing JS analysis.
- Non-zero exits are ignored (`--ignore-failure`): linters exit 1 when they find something.
- The skif command line is `skif . --format json --no-install [--no-js]`; the working directory is the target.
- Wall-clock times include process start-up. For sub-10 ms runs hyperfine warns about short commands; trust the
  mean over many `--runs`.
- The corpus pins live in `corpus.toml` (full commit SHAs). Repositories are fetched shallow with
  `git fetch --depth 1 origin <sha>`, so `fetch` is idempotent and cheap to repeat.
