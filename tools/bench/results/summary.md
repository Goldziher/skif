# skif benchmark results

skif 0.6.0 at `5bd5c5cc37dd`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1014-aws, x86_64, Intel(R) Xeon(R) 6975P-C, 4 logical CPUs (2 physical cores), 15.3 GiB memory, hypervisor KVM; load average 7.07 / 4.00 / 1.82 before and 4.12 / 3.48 / 2.75 after the run.

Runner: github-hosted `GitHub Actions 1000201211`, image `ubuntu24` `20261111.127.1`, Linux X64; [workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43801031855/attempts/1).

Toolchain: rustc `rustc 1.100.0 (0ef1bc386 2026-11-09)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.44.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 4.4 | 0.3 | 4.3 | 4.0 | 5.1 | 1.0x |
| sherif | 25.0 | 1.2 | 24.9 | 23.1 | 27.0 | 5.8x |
| syncpack lint | 30.7 | 1.0 | 30.6 | 28.8 | 32.7 | 7.1x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 19.4 | 0.6 | 19.4 | 18.4 | 20.9 | 1.0x |
| sherif | 39.2 | 1.3 | 39.4 | 36.9 | 42.0 | 2.0x |
| syncpack lint | 116 | 1.5 | 116 | 114 | 119 | 6.0x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 87.2 | 7.9 | 84.0 | 81.0 | 109 | 1.0x |
| sherif | 98.4 | 2.1 | 98.6 | 94.8 | 103 | 1.1x |
| syncpack lint | 516 | 5.4 | 516 | 510 | 532 | 5.9x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 185 | 3.9 | 186 | 178 | 190 | 1.0x |
| skif --type-aware | 242 | 5.9 | 241 | 232 | 255 | 1.3x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 31.3 | 2.6 | 31.3 | 27.9 | 36.1 | 1.0x |
| skif --type-aware | 456 | 9.1 | 455 | 441 | 477 | 14.5x |
| skif --no-js | 3.2 | 0.1 | 3.1 | 3.0 | 3.5 | 0.1x |
| sherif | 23.2 | 1.4 | 22.8 | 21.2 | 26.5 | 7.4x |
| syncpack lint | 26.8 | 0.9 | 26.7 | 25.4 | 29.3 | 8.5x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 63.8 | 0.5 | 63.7 | 63.0 | 64.8 | 1.0x |
| skif --type-aware | 378 | 19.1 | 375 | 361 | 443 | 5.9x |
| skif --no-js | 3.3 | 0.1 | 3.3 | 3.1 | 3.5 | 0.1x |
| sherif | 23.9 | 1.9 | 23.6 | 21.4 | 27.7 | 7.2x |
| syncpack lint | 27.7 | 1.2 | 27.9 | 26.0 | 30.6 | 8.3x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 359 | 4.1 | 359 | 352 | 366 | 1.0x |
| skif --type-aware | 3127 | 41.3 | 3118 | 3074 | 3188 | 8.7x |
| skif --no-js | 8.5 | 0.1 | 8.5 | 8.3 | 8.8 | 0.0x |
| sherif | 30.7 | 1.0 | 31.2 | 28.9 | 32.2 | 3.6x |
| syncpack lint | 79.6 | 1.2 | 79.4 | 77.9 | 82.0 | 9.4x |
| knip --reporter json | 2987 | 19.5 | 2990 | 2941 | 3014 | 8.3x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 881 | 5.6 | 880 | 872 | 890 | 1.0x |
| skif --type-aware | 8887 | 86.7 | 8887 | 8767 | 9026 | 10.1x |
| skif --no-js | 13.1 | 0.4 | 13.1 | 12.4 | 13.7 | 0.0x |
| sherif | 25.8 | 1.8 | 25.3 | 23.6 | 29.5 | 2.0x |
| syncpack lint | 187 | 1.2 | 187 | 185 | 189 | 14.3x |
| knip --reporter json | 4346 | 41.6 | 4334 | 4275 | 4403 | 4.9x |

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 89.9 | 3.1 | 91.0 | 83.6 | 93.6 | 1.0x |
| skif --type-aware | 114 | 4.6 | 113 | 106 | 122 | 1.3x |
| skif --no-js | 5.1 | 0.1 | 5.0 | 4.9 | 5.3 | 0.1x |
| sherif | 24.1 | 1.2 | 24.3 | 22.0 | 25.9 | 4.8x |
| syncpack lint | 51.1 | 1.7 | 50.7 | 49.3 | 55.2 | 10.1x |
| knip --reporter json | 580 | 6.2 | 580 | 570 | 593 | 6.4x |

## corpus: angular (pnpm)

Installed with `pnpm 12.16.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 968 | 3.9 | 969 | 963 | 973 | 1.0x |
| skif --type-aware | 3479 | 33.5 | 3487 | 3434 | 3511 | 3.6x |
| skif --no-js | 5.2 | 0.1 | 5.2 | 5.1 | 5.3 | 0.0x |
| sherif | 24.2 | 1.8 | 23.7 | 22.6 | 27.3 | 4.7x |
| syncpack lint | 62.6 | 1.1 | 62.9 | 61.0 | 63.8 | 12.1x |
| knip --reporter json | 3445 | 55.5 | 3447 | 3378 | 3508 | 3.6x |

## corpus: vscode (pnpm)

Installed with `pnpm 12.16.0`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 3973 | 18.3 | 3979 | 3948 | 3991 | 1.0x |
| skif --type-aware | 11718 | 74.1 | 11696 | 11633 | 11811 | 2.9x |
| skif --no-js | 64.7 | 0.8 | 65.0 | 63.6 | 65.4 | 0.0x |
| sherif | 26.5 | 2.5 | 25.4 | 24.8 | 30.7 | 0.4x |
| syncpack lint | 98.1 | 1.0 | 97.7 | 97.3 | 99.3 | 1.5x |
| knip --reporter json | 12800 | 123 | 12849 | 12585 | 12891 | 3.2x |

## corpus: n8n (pnpm)

Installed with `pnpm 12.4.2`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 5415 | 55.9 | 5411 | 5356 | 5502 | 1.0x |
| skif --type-aware | 20605 | 94.4 | 20619 | 20448 | 20693 | 3.8x |
| skif --no-js | 98.4 | 1.5 | 98.3 | 96.6 | 100 | 0.0x |
| sherif | 26.3 | 1.5 | 26.6 | 24.4 | 28.6 | 0.3x |
| syncpack lint | 129 | 0.9 | 129 | 128 | 130 | 1.3x |
| knip --reporter json | 20828 | 171 | 20753 | 20632 | 21038 | 3.8x |

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries): `ERROR: Error loading packages/@n8n/nodes-langchain/vite.config.ts (Cannot find module 'packages/@n8n/nodes-langchain/node_modules/n8n-workflow/dist/cjs/index.js`.

## corpus: next-js (pnpm)

Installed with `pnpm 12.9.1`.

1 warmup run and 5 measured runs, as corpus.toml caps them here.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 3062 | 8.3 | 3059 | 3053 | 3072 | 1.0x |
| skif --type-aware | 5949 | 53.1 | 5941 | 5897 | 6038 | 1.9x |
| skif --no-js | 162 | 1.8 | 162 | 160 | 165 | 0.1x |
| sherif | 24.7 | 1.0 | 25.4 | 23.5 | 25.6 | 0.2x |
| syncpack lint | 233 | 1.1 | 233 | 232 | 234 | 1.4x |
| knip --reporter json | 4706 | 68.9 | 4678 | 4662 | 4827 | 1.5x |

knip --reporter json exited with an error in 5 of 5 runs, as corpus.toml declares, after its report, so its time is compared (its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries): `ERROR: Error loading jest.config.js (Cannot find module './dist/build/jest/jest')`.

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 19.4 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 184.9 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 8.3x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 4.9x) | FAIL |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 6.4x) | pass |
| corpus: angular (pnpm) | JS analysis >= 5x faster than knip (measured 3.6x) | FAIL |
| corpus: vscode (pnpm) | JS analysis >= 5x faster than knip (measured 3.2x) | FAIL |
| corpus: n8n (pnpm) | JS analysis >= 5x faster than knip (measured 3.8x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries) | FAIL |
| corpus: next-js (pnpm) | JS analysis >= 5x faster than knip (measured 1.5x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries) | FAIL |

## Type awareness (ADR 0029)

| Target | types on, mean (ms) | types off (default), mean (ms) | on / off | types on, peak RSS (MiB) | types off (default), peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| graph: 10000 files in 100 packages | 242 | 185 | 1.3x | 99 | 90 |
| corpus: create-t3-turbo (pnpm) | 456 | 31.3 | 14.5x | 251 | 28 |
| corpus: vue-core (pnpm) | 378 | 63.8 | 5.9x | 167 | 34 |
| corpus: tanstack-query (pnpm) | 3127 | 359 | 8.7x | 429 | 109 |
| corpus: babel (yarn) | 8887 | 881 | 10.1x | 499 | 161 |
| corpus: npm-cli (npm) | 114 | 89.9 | 1.3x | 52 | 44 |
| corpus: angular (pnpm) | 3479 | 968 | 3.6x | 643 | 181 |
| corpus: vscode (pnpm) | 11718 | 3973 | 2.9x | 2938 | 436 |
| corpus: n8n (pnpm) | 20605 | 5415 | 3.8x | 3761 | 894 |
| corpus: next-js (pnpm) | 5949 | 3062 | 1.9x | 825 | 257 |

| Target | Budget check | Result |
| --- | --- | --- |
| graph: 10000 files in 100 packages | peak RSS with types <= 4096 MiB (measured 99 MiB) | pass |
| graph: 10000 files in 100 packages | whole run with types < 5000 ms (measured 242.3 ms) | pass |
| corpus: create-t3-turbo (pnpm) | peak RSS with types <= 4096 MiB (measured 251 MiB) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | peak RSS with types <= 4096 MiB (measured 167 MiB) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | peak RSS with types <= 4096 MiB (measured 429 MiB) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 1.0x) | FAIL |
| corpus: babel (yarn) | peak RSS with types <= 4096 MiB (measured 499 MiB) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 0.5x) | FAIL |
| corpus: npm-cli (npm) | peak RSS with types <= 4096 MiB (measured 52 MiB) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 5.1x) | pass |
| corpus: angular (pnpm) | peak RSS with types <= 4096 MiB (measured 643 MiB) | pass |
| corpus: angular (pnpm) | JS analysis >= 5x faster than knip (measured 1.0x) | FAIL |
| corpus: vscode (pnpm) | peak RSS with types <= 4096 MiB (measured 2938 MiB) | pass |
| corpus: vscode (pnpm) | JS analysis >= 5x faster than knip (measured 1.1x) | FAIL |
| corpus: n8n (pnpm) | peak RSS with types <= 4096 MiB (measured 3761 MiB) | pass |
| corpus: n8n (pnpm) | JS analysis >= 5x faster than knip (measured 1.0x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: two Vite configs do not load outside the repository's own build (@n8n/mcp-apps needs an MCP app mode, @n8n/nodes-langchain the unbuilt n8n-workflow); knip reports without their entries) | FAIL |
| corpus: next-js (pnpm) | peak RSS with types <= 4096 MiB (measured 825 MiB) | pass |
| corpus: next-js (pnpm) | JS analysis >= 5x faster than knip (measured 0.8x; knip --reporter json exited with an error in 5 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest, which an install without scripts leaves out; knip reports without that config's entries) | FAIL |
| corpus: n8n (pnpm) | types on <= 3x types off on the largest repository, the one whose untyped run takes longest (measured 3.8x: 20605 ms against 5415 ms) | FAIL |
| corpus | median types on / types off across 9 repositories <= 2x (measured 3.8x) | FAIL |
