# skif benchmark results

skif 0.3.0 at `150634614dea`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, AMD EPYC 9V45 96-Core Processor, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft; load average 3.76 / 2.39 / 1.02 before and 1.72 / 1.95 / 1.12 after the run.

Runner: github-hosted `GitHub Actions 1000195012`, image `ubuntu24` `20261101.360.1`, Linux X64; [workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43136971156/attempts/1).

Toolchain: rustc `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.44.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 3.6 | 0.1 | 3.6 | 3.4 | 3.7 | 1.0x |
| sherif | 23.0 | 1.1 | 22.4 | 21.8 | 25.3 | 6.5x |
| syncpack lint | 30.1 | 1.2 | 29.8 | 28.5 | 32.5 | 8.4x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 18.5 | 0.6 | 18.3 | 17.8 | 19.5 | 1.0x |
| sherif | 37.8 | 0.8 | 38.0 | 36.6 | 39.1 | 2.0x |
| syncpack lint | 110 | 3.6 | 109 | 108 | 122 | 6.0x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 81.5 | 2.9 | 81.3 | 77.7 | 87.7 | 1.0x |
| sherif | 94.0 | 2.1 | 93.9 | 90.6 | 98.0 | 1.2x |
| syncpack lint | 498 | 24.0 | 495 | 477 | 565 | 6.1x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 156 | 8.0 | 156 | 148 | 182 | 1.0x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 25.1 | 5.3 | 23.0 | 22.4 | 42.0 | 1.0x |
| skif --no-js | 2.9 | 0.1 | 3.0 | 2.9 | 3.2 | 0.1x |
| sherif | 25.6 | 7.4 | 22.3 | 20.6 | 43.2 | 8.7x |
| syncpack lint | 26.3 | 1.4 | 26.1 | 24.4 | 30.1 | 8.9x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 43.1 | 0.9 | 42.9 | 41.9 | 46.0 | 1.0x |
| skif --no-js | 3.1 | 0.1 | 3.1 | 3.1 | 3.4 | 0.1x |
| sherif | 22.7 | 1.1 | 22.6 | 21.3 | 25.5 | 7.3x |
| syncpack lint | 28.1 | 1.6 | 27.7 | 26.0 | 30.7 | 9.0x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 281 | 8.0 | 280 | 266 | 292 | 1.0x |
| skif --no-js | 8.9 | 0.3 | 8.9 | 8.4 | 9.4 | 0.0x |
| sherif | 33.6 | 2.9 | 34.6 | 28.4 | 37.5 | 3.8x |
| syncpack lint | 85.7 | 3.0 | 85.6 | 80.6 | 90.2 | 9.7x |
| knip --reporter json | 2541 | 36.0 | 2533 | 2491 | 2615 | 9.0x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 604 | 11.7 | 603 | 584 | 625 | 1.0x |
| skif --no-js | 12.2 | 0.4 | 12.1 | 11.7 | 13.1 | 0.0x |
| sherif | 25.3 | 1.9 | 24.8 | 24.0 | 31.6 | 2.1x |
| syncpack lint | 251 | 2.7 | 251 | 248 | 258 | 20.6x |
| knip --reporter json | 3677 | 52.1 | 3675 | 3619 | 3786 | 6.1x |

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 63.2 | 2.4 | 64.0 | 58.1 | 66.1 | 1.0x |
| skif --no-js | 4.6 | 0.1 | 4.6 | 4.5 | 4.9 | 0.1x |
| sherif | 21.8 | 0.9 | 21.6 | 20.7 | 24.0 | 4.7x |
| syncpack lint | 55.9 | 1.2 | 55.8 | 53.9 | 58.3 | 12.1x |
| knip --reporter json | 483 | 7.1 | 481 | 475 | 504 | 7.6x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 18.5 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 155.7 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 9.0x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 6.1x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 7.6x) | pass |
