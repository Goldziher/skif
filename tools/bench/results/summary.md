# skif benchmark results

skif 0.7.0 at `b2a21d3adb74` (harness at `27ef55dd0372`); hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, AMD EPYC 7763 64-Core Processor, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft; load average 5.76 / 3.53 / 2.07 before and 3.24 / 3.01 / 2.82 after the run.

Runner: github-hosted `GitHub Actions 1000002879`, image `ubuntu24` `20261111.370.1`, Linux X64; [workflow run, attempt 1](https://github.com/polylint/skif/actions/runs/44028742119/attempts/1).

Toolchain: rustc `rustc 1.100.0 (0ef1bc386 2026-11-09)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.45.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 5.6 | 0.1 | 5.6 | 5.4 | 6.1 | 1.0x |
| sherif | 35.3 | 1.4 | 35.4 | 33.2 | 37.4 | 6.3x |
| syncpack lint | 46.1 | 1.4 | 45.9 | 44.1 | 49.2 | 8.2x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 29.3 | 0.4 | 29.3 | 28.4 | 30.1 | 1.0x |
| sherif | 59.4 | 1.3 | 59.5 | 57.2 | 61.5 | 2.0x |
| syncpack lint | 178 | 1.5 | 178 | 176 | 181 | 6.1x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 122 | 1.7 | 121 | 119 | 126 | 1.0x |
| sherif | 144 | 1.2 | 144 | 143 | 147 | 1.2x |
| syncpack lint | 792 | 11.5 | 786 | 776 | 815 | 6.5x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 347 | 21.9 | 336 | 322 | 385 | 1.0x |
| skif --type-aware | 437 | 8.9 | 437 | 424 | 451 | 1.3x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 41.7 | 0.9 | 42.0 | 40.4 | 42.9 | 1.0x |
| skif --type-aware | 672 | 42.5 | 656 | 644 | 792 | 16.1x |
| skif --no-js | 4.4 | 0.0 | 4.4 | 4.3 | 4.5 | 0.1x |
| sherif | 33.7 | 1.4 | 32.9 | 31.9 | 36.2 | 7.6x |
| syncpack lint | 40.6 | 1.8 | 39.9 | 38.6 | 44.8 | 9.2x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 92.7 | 0.9 | 92.7 | 91.7 | 94.6 | 1.0x |
| skif --type-aware | 421 | 10.5 | 422 | 401 | 444 | 4.5x |
| skif --no-js | 4.7 | 0.1 | 4.6 | 4.5 | 4.8 | 0.1x |
| sherif | 34.5 | 1.0 | 34.5 | 32.7 | 37.1 | 7.4x |
| syncpack lint | 43.2 | 1.5 | 43.4 | 41.2 | 47.4 | 9.3x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 513 | 3.6 | 512 | 507 | 520 | 1.0x |
| skif --type-aware | 3904 | 82.4 | 3893 | 3778 | 4111 | 7.6x |
| skif --no-js | 12.2 | 0.2 | 12.2 | 11.9 | 12.4 | 0.0x |
| sherif | 40.9 | 1.2 | 41.3 | 38.6 | 42.2 | 3.4x |
| syncpack lint | 123 | 1.5 | 122 | 121 | 125 | 10.1x |
| knip --reporter json | 4327 | 22.1 | 4329 | 4290 | 4356 | 8.4x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 1117 | 6.9 | 1118 | 1107 | 1133 | 1.0x |
| skif --type-aware | 12969 | 168 | 13004 | 12722 | 13159 | 11.6x |
| skif --no-js | 12.2 | 0.4 | 12.2 | 11.6 | 12.7 | 0.0x |
| skif --recursive | 1103 | 8.8 | 1101 | 1088 | 1118 | - |
| sherif | 36.6 | 1.4 | 36.8 | 34.2 | 39.4 | 3.0x |
| syncpack lint | 343 | 1.6 | 342 | 340 | 345 | 28.0x |
| knip --reporter json | 6552 | 43.1 | 6551 | 6486 | 6649 | 5.9x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 121 | 1.5 | 121 | 119 | 126 | 1.0x |
| skif --type-aware | 156 | 4.2 | 156 | 148 | 162 | 1.3x |
| skif --no-js | 7.0 | 0.2 | 7.0 | 6.8 | 7.5 | 0.1x |
| sherif | 32.4 | 1.2 | 32.5 | 30.7 | 34.7 | 4.6x |
| syncpack lint | 77.0 | 1.0 | 76.8 | 75.3 | 79.5 | 11.0x |
| knip --reporter json | 824 | 5.6 | 823 | 816 | 836 | 6.8x |

## corpus: angular (pnpm)

Installed with `pnpm 12.16.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 1159 | 4.7 | 1161 | 1154 | 1166 | 1.0x |
| skif --type-aware | 5087 | 38.6 | 5089 | 5035 | 5140 | 4.4x |
| skif --no-js | 6.9 | 0.2 | 6.9 | 6.7 | 7.2 | 0.0x |
| sherif | 33.0 | 0.6 | 32.9 | 32.1 | 33.7 | 4.7x |
| syncpack lint | 103 | 1.2 | 104 | 101 | 105 | 14.8x |
| knip --reporter json | 4862 | 28.1 | 4858 | 4823 | 4892 | 4.2x |

## corpus: vscode (pnpm)

Installed with `pnpm 12.16.0`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 3249 | 5.6 | 3246 | 3244 | 3258 | 1.0x |
| skif --type-aware | 13932 | 126 | 13940 | 13788 | 14114 | 4.3x |
| skif --no-js | 61.6 | 3.0 | 60.9 | 58.5 | 66.4 | 0.0x |
| skif --recursive | 5137 | 4.1 | 5137 | 5132 | 5142 | - |
| sherif | 34.6 | 1.0 | 34.4 | 33.8 | 36.3 | 0.6x |
| syncpack lint | 163 | 0.8 | 163 | 162 | 164 | 2.7x |
| knip --reporter json | 16369 | 183 | 16323 | 16111 | 16564 | 5.0x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

## corpus: n8n (pnpm)

Installed with `pnpm 12.4.2`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 6276 | 11.7 | 6284 | 6263 | 6286 | 1.0x |
| skif --type-aware | 27676 | 212 | 27577 | 27496 | 27934 | 4.4x |
| skif --no-js | 130 | 0.9 | 130 | 130 | 132 | 0.0x |
| sherif | 36.7 | 1.0 | 36.0 | 35.8 | 37.9 | 0.3x |
| syncpack lint | 232 | 1.1 | 231 | 231 | 234 | 1.8x |
| knip --reporter json | 29725 | 77.0 | 29735 | 29614 | 29798 | 4.7x |

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries): `ERROR: Error loading packages/@n8n/nodes-langchain/vite.config.ts (Cannot find module 'packages/@n8n/nodes-langchain/node_modules/n8n-workflow/dist/cjs/index.js`.

## corpus: next-js (pnpm)

Installed with `pnpm 12.9.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 1071 | 5.7 | 1070 | 1066 | 1080 | 1.0x |
| skif --type-aware | 3277 | 31.6 | 3279 | 3240 | 3323 | 3.1x |
| skif --no-js | 95.4 | 1.3 | 95.5 | 94.0 | 97.3 | 0.1x |
| skif --recursive | 3326 | 11.3 | 3324 | 3313 | 3344 | - |
| sherif | 34.7 | 1.0 | 34.3 | 33.5 | 35.9 | 0.4x |
| syncpack lint | 436 | 1.5 | 436 | 434 | 438 | 4.6x |
| knip --reporter json | 7524 | 52.8 | 7531 | 7466 | 7587 | 7.0x |

`skif --recursive` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries): `ERROR: Error loading jest.config.js (Cannot find module './dist/build/jest/jest')`.

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 29.3 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 347.3 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 8.4x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 5.9x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 6.8x) | pass |
| corpus: angular (pnpm) | JS analysis >= 5x faster than knip (measured 4.2x) | FAIL |
| corpus: vscode (pnpm) | JS analysis >= 5x faster than knip (measured 5.0x) | pass |
| corpus: n8n (pnpm) | JS analysis >= 5x faster than knip (measured 4.7x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries) | FAIL |
| corpus: next-js (pnpm) | JS analysis >= 5x faster than knip (measured 7.0x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries) | pass |

## Type awareness (ADR 0010)

| Target | types on, mean (ms) | types off (default), mean (ms) | on / off | types on, peak RSS (MiB) | types off (default), peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| graph: 10000 files in 100 packages | 437 | 347 | 1.3x | 119 | 111 |
| corpus: create-t3-turbo (pnpm) | 672 | 41.7 | 16.1x | 250 | 28 |
| corpus: vue-core (pnpm) | 421 | 92.7 | 4.5x | 147 | 36 |
| corpus: tanstack-query (pnpm) | 3904 | 513 | 7.6x | 403 | 109 |
| corpus: babel (yarn) | 12969 | 1117 | 11.6x | 462 | 158 |
| corpus: npm-cli (npm) | 156 | 121 | 1.3x | 50 | 45 |
| corpus: angular (pnpm) | 5087 | 1159 | 4.4x | 619 | 218 |
| corpus: vscode (pnpm) | 13932 | 3249 | 4.3x | 2964 | 439 |
| corpus: n8n (pnpm) | 27676 | 6276 | 4.4x | 3661 | 829 |
| corpus: next-js (pnpm) | 3277 | 1071 | 3.1x | 554 | 193 |

| Target | Budget check | Result |
| --- | --- | --- |
| graph: 10000 files in 100 packages | peak RSS with types <= 4096 MiB (measured 119 MiB) | pass |
| graph: 10000 files in 100 packages | whole run with types < 5000 ms (measured 437.4 ms) | pass |
| corpus: create-t3-turbo (pnpm) | peak RSS with types <= 4096 MiB (measured 250 MiB) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | peak RSS with types <= 4096 MiB (measured 147 MiB) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | peak RSS with types <= 4096 MiB (measured 403 MiB) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 1.1x) | FAIL |
| corpus: babel (yarn) | peak RSS with types <= 4096 MiB (measured 462 MiB) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 0.5x) | FAIL |
| corpus: npm-cli (npm) | peak RSS with types <= 4096 MiB (measured 50 MiB) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 5.3x) | pass |
| corpus: angular (pnpm) | peak RSS with types <= 4096 MiB (measured 619 MiB) | pass |
| corpus: angular (pnpm) | JS analysis >= 5x faster than knip (measured 1.0x) | FAIL |
| corpus: vscode (pnpm) | peak RSS with types <= 4096 MiB (measured 2964 MiB) | pass |
| corpus: vscode (pnpm) | JS analysis >= 5x faster than knip (measured 1.2x) | FAIL |
| corpus: n8n (pnpm) | peak RSS with types <= 4096 MiB (measured 3661 MiB) | pass |
| corpus: n8n (pnpm) | JS analysis >= 5x faster than knip (measured 1.1x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries) | FAIL |
| corpus: next-js (pnpm) | peak RSS with types <= 4096 MiB (measured 554 MiB) | pass |
| corpus: next-js (pnpm) | JS analysis >= 5x faster than knip (measured 2.3x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries) | FAIL |
| corpus: n8n (pnpm) | types on <= 3x types off on the largest repository, the one whose untyped run takes longest (measured 4.4x: 27676 ms against 6276 ms) | FAIL |
| corpus | median types on / types off across 9 repositories <= 2x (measured 4.4x) | FAIL |
