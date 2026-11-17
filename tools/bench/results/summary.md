# skif benchmark results

skif 0.9.0 at `69850aeed43d`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1014-aws, x86_64, Intel(R) Xeon(R) 6975P-C, 4 logical CPUs (2 physical cores), 15.3 GiB memory, hypervisor KVM; load average 5.71 / 3.16 / 1.64 before and 2.97 / 2.56 / 2.01 after the run.

Runner: github-hosted `GitHub Actions 1000004325`, image `ubuntu24` `20261111.127.1`, Linux X64; [workflow run, attempt 1](https://github.com/polylint/skif/actions/runs/44151292705/attempts/1).

Toolchain: rustc `rustc 1.100.0 (0ef1bc386 2026-11-09)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.45.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 4.0 | 0.2 | 3.9 | 3.7 | 4.6 | 1.0x |
| sherif | 25.6 | 1.9 | 24.9 | 23.1 | 29.1 | 6.4x |
| syncpack lint | 31.5 | 1.1 | 31.8 | 30.0 | 33.6 | 7.9x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 19.0 | 0.5 | 19.0 | 17.8 | 19.8 | 1.0x |
| sherif | 38.1 | 1.1 | 38.2 | 36.8 | 40.6 | 2.0x |
| syncpack lint | 115 | 1.9 | 115 | 111 | 117 | 6.0x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 83.9 | 7.3 | 82.2 | 79.5 | 110 | 1.0x |
| sherif | 94.2 | 3.3 | 93.7 | 90.7 | 104 | 1.1x |
| syncpack lint | 514 | 9.7 | 512 | 507 | 548 | 6.1x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 203 | 3.1 | 204 | 197 | 207 | 1.0x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 28.0 | 0.6 | 28.1 | 27.0 | 29.3 | 1.0x |
| skif --no-js | 3.0 | 0.1 | 2.9 | 2.9 | 3.1 | 0.1x |
| sherif | 23.4 | 1.5 | 23.4 | 21.2 | 26.3 | 7.9x |
| syncpack lint | 27.2 | 1.4 | 27.2 | 25.1 | 30.4 | 9.2x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 63.2 | 0.6 | 63.3 | 62.2 | 64.1 | 1.0x |
| skif --no-js | 3.2 | 0.3 | 3.1 | 3.0 | 4.4 | 0.1x |
| sherif | 23.9 | 1.8 | 23.9 | 21.1 | 27.3 | 7.5x |
| syncpack lint | 28.8 | 1.6 | 28.5 | 26.9 | 32.2 | 9.1x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 359 | 2.9 | 359 | 355 | 366 | 1.0x |
| skif --no-js | 7.9 | 0.2 | 7.9 | 7.7 | 8.4 | 0.0x |
| sherif | 27.7 | 1.2 | 27.9 | 25.5 | 29.3 | 3.5x |
| syncpack lint | 78.1 | 2.7 | 77.3 | 75.2 | 84.5 | 9.9x |
| knip --reporter json | 2874 | 20.4 | 2869 | 2842 | 2914 | 8.0x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 808 | 4.4 | 807 | 801 | 815 | 1.0x |
| skif --no-js | 8.3 | 0.2 | 8.3 | 7.9 | 8.6 | 0.0x |
| skif --recursive | 811 | 6.3 | 812 | 803 | 825 | - |
| sherif | 25.3 | 1.4 | 25.2 | 23.1 | 27.8 | 3.1x |
| syncpack lint | 186 | 1.6 | 186 | 183 | 189 | 22.5x |
| knip --reporter json | 4441 | 21.7 | 4441 | 4411 | 4485 | 5.5x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 82.7 | 0.7 | 82.6 | 80.5 | 83.7 | 1.0x |
| skif --no-js | 4.8 | 0.1 | 4.8 | 4.6 | 5.0 | 0.1x |
| sherif | 24.5 | 1.5 | 24.6 | 22.0 | 26.8 | 5.1x |
| syncpack lint | 50.0 | 2.5 | 50.1 | 44.8 | 55.1 | 10.4x |
| knip --reporter json | 539 | 4.8 | 539 | 532 | 548 | 6.5x |

## corpus: angular (pnpm)

Installed with `pnpm 12.16.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 855 | 7.0 | 857 | 847 | 862 | 1.0x |
| skif --no-js | 5.1 | 0.2 | 5.0 | 4.9 | 5.4 | 0.0x |
| sherif | 25.6 | 1.6 | 25.1 | 24.0 | 28.2 | 5.1x |
| syncpack lint | 63.1 | 0.7 | 63.2 | 62.2 | 64.1 | 12.5x |
| knip --reporter json | 3466 | 55.4 | 3452 | 3414 | 3559 | 4.1x |

## corpus: vscode (pnpm)

Installed with `pnpm 12.16.0`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 2556 | 18.3 | 2566 | 2527 | 2570 | 1.0x |
| skif --no-js | 34.3 | 0.5 | 34.3 | 33.7 | 34.8 | 0.0x |
| skif --recursive | 4067 | 17.8 | 4067 | 4050 | 4093 | - |
| sherif | 27.3 | 2.1 | 27.4 | 24.8 | 29.9 | 0.8x |
| syncpack lint | 99.5 | 0.5 | 99.7 | 98.8 | 100 | 2.9x |
| knip --reporter json | 12969 | 155 | 12924 | 12827 | 13222 | 5.1x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

## corpus: n8n (pnpm)

Installed with `pnpm 12.4.2`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 5065 | 28.4 | 5079 | 5027 | 5093 | 1.0x |
| skif --no-js | 82.6 | 1.2 | 82.2 | 81.4 | 84.4 | 0.0x |
| sherif | 29.7 | 2.2 | 29.4 | 28.0 | 33.4 | 0.4x |
| syncpack lint | 138 | 1.0 | 137 | 137 | 139 | 1.7x |
| knip --reporter json | 21572 | 253 | 21703 | 21148 | 21773 | 4.3x |

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries): `ERROR: Error loading packages/@n8n/nodes-langchain/vite.config.ts (Cannot find module 'packages/@n8n/nodes-langchain/node_modules/n8n-workflow/dist/cjs/index.js`.

## corpus: next-js (pnpm)

Installed with `pnpm 12.9.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 793 | 12.0 | 795 | 774 | 808 | 1.0x |
| skif --no-js | 57.8 | 0.4 | 57.9 | 57.3 | 58.4 | 0.1x |
| skif --recursive | 2442 | 13.7 | 2440 | 2422 | 2459 | - |
| sherif | 25.7 | 2.0 | 25.5 | 23.2 | 28.5 | 0.4x |
| syncpack lint | 242 | 0.7 | 242 | 241 | 243 | 4.2x |
| knip --reporter json | 4705 | 43.9 | 4695 | 4667 | 4781 | 5.9x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries): `ERROR: Error loading jest.config.js (Cannot find module './dist/build/jest/jest')`.

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 19.0 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 203.1 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 8.0x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 5.5x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 6.5x) | pass |
| corpus: angular (pnpm) | JS analysis >= 5x faster than knip (measured 4.1x) | FAIL |
| corpus: vscode (pnpm) | JS analysis >= 5x faster than knip (measured 5.1x) | pass |
| corpus: n8n (pnpm) | JS analysis >= 5x faster than knip (measured 4.3x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries) | FAIL |
| corpus: next-js (pnpm) | JS analysis >= 5x faster than knip (measured 5.9x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries) | pass |

## Peak RSS

| Target | skif, peak RSS (MiB) |
| --- | ---: |
| graph: 10000 files in 100 packages | 109 |
| corpus: create-t3-turbo (pnpm) | 26 |
| corpus: vue-core (pnpm) | 33 |
| corpus: tanstack-query (pnpm) | 108 |
| corpus: babel (yarn) | 154 |
| corpus: npm-cli (npm) | 45 |
| corpus: angular (pnpm) | 215 |
| corpus: vscode (pnpm) | 440 |
| corpus: n8n (pnpm) | 844 |
| corpus: next-js (pnpm) | 187 |

## Warm cache

The whole skif run with its cache on (`--cache-location`), on the store a run of the same tree wrote. It is compared with no other tool and is part of no check.

| Target | skif, uncached, median (ms) | skif, warm cache, median (ms) | Speed-up |
| --- | ---: | ---: | ---: |
| graph: 10000 files in 100 packages | 204 | 197 | 1.0x |
| corpus: create-t3-turbo (pnpm) | 28.1 | 28.0 | 1.0x |
| corpus: vue-core (pnpm) | 63.3 | 41.8 | 1.5x |
| corpus: tanstack-query (pnpm) | 359 | 317 | 1.1x |
| corpus: babel (yarn) | 807 | 797 | 1.0x |
| corpus: npm-cli (npm) | 82.6 | 56.0 | 1.5x |
| corpus: angular (pnpm) | 857 | 409 | 2.1x |
| corpus: vscode (pnpm) | 2566 | 1377 | 1.9x |
| corpus: n8n (pnpm) | 5079 | 3892 | 1.3x |
| corpus: next-js (pnpm) | 795 | 610 | 1.3x |
