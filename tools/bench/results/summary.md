# skif benchmark results

skif 0.6.0 at `e591efab18bb`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1014-aws, x86_64, Intel(R) Xeon(R) 6975P-C, 4 logical CPUs (2 physical cores), 15.3 GiB memory, hypervisor KVM; load average 6.44 / 4.01 / 2.16 before and 2.56 / 2.62 / 2.41 after the run.

Runner: github-hosted `GitHub Actions 1000000982`, image `ubuntu24` `20261111.127.1`, Linux X64; [workflow run, attempt 1](https://github.com/polylint/skif/actions/runs/43873961099/attempts/1).

Toolchain: rustc `rustc 1.100.0 (0ef1bc386 2026-11-09)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.45.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 4.2 | 0.2 | 4.3 | 4.0 | 4.8 | 1.0x |
| sherif | 24.7 | 1.4 | 24.6 | 22.0 | 27.1 | 5.8x |
| syncpack lint | 35.7 | 9.4 | 31.2 | 29.3 | 64.1 | 8.4x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 18.7 | 0.4 | 18.7 | 18.2 | 19.3 | 1.0x |
| sherif | 38.3 | 1.8 | 38.4 | 36.0 | 43.2 | 2.0x |
| syncpack lint | 113 | 1.5 | 113 | 111 | 115 | 6.0x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 79.8 | 0.8 | 80.0 | 78.6 | 81.1 | 1.0x |
| sherif | 90.9 | 2.3 | 90.4 | 88.9 | 98.5 | 1.1x |
| syncpack lint | 513 | 2.7 | 512 | 509 | 519 | 6.4x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 203 | 5.8 | 203 | 192 | 213 | 1.0x |
| skif --type-aware | 268 | 4.6 | 269 | 260 | 274 | 1.3x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 28.8 | 0.4 | 28.9 | 28.0 | 29.3 | 1.0x |
| skif --type-aware | 456 | 12.8 | 454 | 438 | 486 | 15.9x |
| skif --no-js | 3.2 | 0.1 | 3.3 | 3.0 | 3.4 | 0.1x |
| sherif | 22.2 | 1.2 | 22.4 | 20.5 | 24.4 | 6.9x |
| syncpack lint | 27.4 | 1.7 | 27.0 | 25.1 | 31.7 | 8.6x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 61.6 | 0.5 | 61.4 | 60.8 | 62.5 | 1.0x |
| skif --type-aware | 357 | 17.9 | 355 | 340 | 419 | 5.8x |
| skif --no-js | 3.3 | 0.1 | 3.3 | 3.2 | 3.4 | 0.1x |
| sherif | 23.0 | 1.8 | 22.2 | 20.7 | 26.1 | 7.0x |
| syncpack lint | 25.8 | 1.5 | 25.7 | 24.1 | 28.7 | 7.9x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 340 | 2.8 | 339 | 337 | 348 | 1.0x |
| skif --type-aware | 2950 | 12.2 | 2946 | 2927 | 2976 | 8.7x |
| skif --no-js | 8.0 | 0.1 | 8.0 | 7.7 | 8.2 | 0.0x |
| sherif | 27.1 | 1.7 | 26.8 | 25.1 | 30.4 | 3.4x |
| syncpack lint | 72.6 | 1.5 | 72.0 | 70.8 | 74.9 | 9.1x |
| knip --reporter json | 2776 | 19.8 | 2782 | 2747 | 2802 | 8.2x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 792 | 4.4 | 793 | 783 | 800 | 1.0x |
| skif --type-aware | 8381 | 35.7 | 8386 | 8302 | 8432 | 10.6x |
| skif --no-js | 8.3 | 0.3 | 8.3 | 7.9 | 8.8 | 0.0x |
| skif --recursive | 804 | 5.5 | 804 | 793 | 816 | - |
| sherif | 25.6 | 0.9 | 25.8 | 24.2 | 27.4 | 3.1x |
| syncpack lint | 181 | 2.3 | 180 | 177 | 185 | 21.7x |
| knip --reporter json | 4257 | 26.6 | 4268 | 4198 | 4290 | 5.4x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 81.0 | 0.9 | 80.7 | 79.8 | 82.7 | 1.0x |
| skif --type-aware | 106 | 3.4 | 107 | 100 | 113 | 1.3x |
| skif --no-js | 4.9 | 0.1 | 4.9 | 4.7 | 5.0 | 0.1x |
| sherif | 23.1 | 1.4 | 23.0 | 20.5 | 24.9 | 4.7x |
| syncpack lint | 46.7 | 2.4 | 46.3 | 42.6 | 50.8 | 9.6x |
| knip --reporter json | 518 | 4.5 | 518 | 509 | 525 | 6.4x |

## corpus: angular (pnpm)

Installed with `pnpm 12.16.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 819 | 6.7 | 815 | 814 | 828 | 1.0x |
| skif --type-aware | 3204 | 22.9 | 3201 | 3177 | 3240 | 3.9x |
| skif --no-js | 5.2 | 0.1 | 5.2 | 5.1 | 5.4 | 0.0x |
| sherif | 24.0 | 1.1 | 24.0 | 22.9 | 25.3 | 4.6x |
| syncpack lint | 59.5 | 1.6 | 60.3 | 56.7 | 60.6 | 11.3x |
| knip --reporter json | 3318 | 40.2 | 3313 | 3258 | 3367 | 4.1x |

## corpus: vscode (pnpm)

Installed with `pnpm 12.16.0`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 2501 | 27.3 | 2485 | 2476 | 2537 | 1.0x |
| skif --type-aware | 9179 | 69.5 | 9192 | 9108 | 9276 | 3.7x |
| skif --no-js | 37.1 | 4.5 | 35.3 | 33.2 | 44.0 | 0.0x |
| skif --recursive | 4075 | 42.9 | 4101 | 4010 | 4107 | - |
| sherif | 26.9 | 1.7 | 27.3 | 24.7 | 29.0 | 0.7x |
| syncpack lint | 101 | 1.3 | 101 | 98.9 | 103 | 2.7x |
| knip --reporter json | 12560 | 139 | 12614 | 12324 | 12681 | 5.0x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

## corpus: n8n (pnpm)

Installed with `pnpm 12.4.2`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 5002 | 12.8 | 5004 | 4982 | 5017 | 1.0x |
| skif --type-aware | 20461 | 101 | 20425 | 20386 | 20632 | 4.1x |
| skif --no-js | 82.0 | 0.3 | 82.0 | 81.5 | 82.3 | 0.0x |
| sherif | 28.4 | 1.2 | 28.2 | 27.3 | 30.4 | 0.3x |
| syncpack lint | 137 | 0.6 | 136 | 136 | 138 | 1.7x |
| knip --reporter json | 20992 | 90.6 | 20959 | 20896 | 21121 | 4.2x |

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries): `ERROR: Error loading packages/@n8n/nodes-langchain/vite.config.ts (Cannot find module 'packages/@n8n/nodes-langchain/node_modules/n8n-workflow/dist/cjs/index.js`.

## corpus: next-js (pnpm)

Installed with `pnpm 12.9.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 818 | 7.3 | 818 | 809 | 829 | 1.0x |
| skif --type-aware | 2398 | 15.5 | 2399 | 2374 | 2417 | 2.9x |
| skif --no-js | 59.1 | 0.3 | 59.1 | 58.6 | 59.3 | 0.1x |
| skif --recursive | 2460 | 3.6 | 2460 | 2457 | 2466 | - |
| sherif | 25.0 | 1.0 | 24.8 | 23.7 | 26.1 | 0.4x |
| syncpack lint | 239 | 1.8 | 240 | 237 | 241 | 4.1x |
| knip --reporter json | 4707 | 43.1 | 4719 | 4655 | 4756 | 5.8x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries): `ERROR: Error loading jest.config.js (Cannot find module './dist/build/jest/jest')`.

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 18.7 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 203.4 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 8.2x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 5.4x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 6.4x) | pass |
| corpus: angular (pnpm) | JS analysis >= 5x faster than knip (measured 4.1x) | FAIL |
| corpus: vscode (pnpm) | JS analysis >= 5x faster than knip (measured 5.0x) | pass |
| corpus: n8n (pnpm) | JS analysis >= 5x faster than knip (measured 4.2x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries) | FAIL |
| corpus: next-js (pnpm) | JS analysis >= 5x faster than knip (measured 5.8x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries) | pass |

## Type awareness (ADR 0029)

| Target | types on, mean (ms) | types off (default), mean (ms) | on / off | types on, peak RSS (MiB) | types off (default), peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| graph: 10000 files in 100 packages | 268 | 203 | 1.3x | 120 | 111 |
| corpus: create-t3-turbo (pnpm) | 456 | 28.8 | 15.9x | 249 | 28 |
| corpus: vue-core (pnpm) | 357 | 61.6 | 5.8x | 167 | 35 |
| corpus: tanstack-query (pnpm) | 2950 | 340 | 8.7x | 428 | 109 |
| corpus: babel (yarn) | 8381 | 792 | 10.6x | 493 | 157 |
| corpus: npm-cli (npm) | 106 | 81.0 | 1.3x | 48 | 47 |
| corpus: angular (pnpm) | 3204 | 819 | 3.9x | 660 | 217 |
| corpus: vscode (pnpm) | 9179 | 2501 | 3.7x | 2965 | 434 |
| corpus: n8n (pnpm) | 20461 | 5002 | 4.1x | 3656 | 850 |
| corpus: next-js (pnpm) | 2398 | 818 | 2.9x | 554 | 191 |

| Target | Budget check | Result |
| --- | --- | --- |
| graph: 10000 files in 100 packages | peak RSS with types <= 4096 MiB (measured 120 MiB) | pass |
| graph: 10000 files in 100 packages | whole run with types < 5000 ms (measured 267.7 ms) | pass |
| corpus: create-t3-turbo (pnpm) | peak RSS with types <= 4096 MiB (measured 249 MiB) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | peak RSS with types <= 4096 MiB (measured 167 MiB) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | peak RSS with types <= 4096 MiB (measured 428 MiB) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 0.9x) | FAIL |
| corpus: babel (yarn) | peak RSS with types <= 4096 MiB (measured 493 MiB) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 0.5x) | FAIL |
| corpus: npm-cli (npm) | peak RSS with types <= 4096 MiB (measured 48 MiB) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 4.9x) | FAIL |
| corpus: angular (pnpm) | peak RSS with types <= 4096 MiB (measured 660 MiB) | pass |
| corpus: angular (pnpm) | JS analysis >= 5x faster than knip (measured 1.0x) | FAIL |
| corpus: vscode (pnpm) | peak RSS with types <= 4096 MiB (measured 2965 MiB) | pass |
| corpus: vscode (pnpm) | JS analysis >= 5x faster than knip (measured 1.4x) | FAIL |
| corpus: n8n (pnpm) | peak RSS with types <= 4096 MiB (measured 3656 MiB) | pass |
| corpus: n8n (pnpm) | JS analysis >= 5x faster than knip (measured 1.0x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries) | FAIL |
| corpus: next-js (pnpm) | peak RSS with types <= 4096 MiB (measured 554 MiB) | pass |
| corpus: next-js (pnpm) | JS analysis >= 5x faster than knip (measured 2.0x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries) | FAIL |
| corpus: n8n (pnpm) | types on <= 3x types off on the largest repository, the one whose untyped run takes longest (measured 4.1x: 20461 ms against 5002 ms) | FAIL |
| corpus | median types on / types off across 9 repositories <= 2x (measured 4.1x) | FAIL |
