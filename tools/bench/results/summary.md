# skif benchmark results

skif 0.3.0 at `329644e11010`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, INTEL(R) XEON(R) PLATINUM 8573C, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft; load average 3.87 / 2.90 / 1.32 before and 1.56 / 2.05 / 1.40 after the run.

Runner: github-hosted `GitHub Actions 1000194852`, image `ubuntu24` `20261101.360.1`, Linux X64; [workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43070718530/attempts/1).

Toolchain: rustc `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.44.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 5.1 | 0.1 | 5.1 | 4.8 | 5.3 | 1.0x |
| sherif | 31.7 | 2.3 | 31.9 | 28.7 | 37.3 | 6.2x |
| syncpack lint | 41.6 | 1.9 | 40.8 | 39.0 | 45.8 | 8.1x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 28.9 | 1.4 | 28.8 | 27.0 | 30.9 | 1.0x |
| sherif | 51.6 | 2.1 | 51.6 | 48.9 | 55.0 | 1.8x |
| syncpack lint | 153 | 2.6 | 152 | 151 | 159 | 5.3x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 124 | 6.1 | 126 | 113 | 131 | 1.0x |
| sherif | 123 | 5.6 | 123 | 117 | 138 | 1.0x |
| syncpack lint | 679 | 3.5 | 677 | 674 | 686 | 5.5x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 259 | 9.4 | 256 | 250 | 276 | 1.0x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 36.3 | 0.6 | 36.1 | 35.2 | 37.3 | 1.0x |
| skif --no-js | 3.9 | 0.1 | 3.9 | 3.8 | 4.0 | 0.1x |
| sherif | 30.2 | 1.3 | 30.1 | 28.6 | 32.5 | 7.8x |
| syncpack lint | 36.9 | 1.4 | 36.5 | 35.3 | 39.6 | 9.5x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 79.9 | 0.7 | 79.8 | 78.8 | 80.9 | 1.0x |
| skif --no-js | 3.9 | 0.1 | 3.9 | 3.7 | 4.2 | 0.0x |
| sherif | 28.9 | 1.5 | 28.8 | 26.0 | 32.8 | 7.4x |
| syncpack lint | 36.5 | 2.2 | 36.1 | 33.6 | 41.4 | 9.4x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 483 | 4.2 | 483 | 475 | 488 | 1.0x |
| skif --no-js | 13.0 | 0.3 | 13.0 | 12.5 | 13.7 | 0.0x |
| sherif | 36.6 | 1.7 | 36.0 | 34.4 | 39.1 | 2.8x |
| syncpack lint | 101 | 1.6 | 101 | 98.9 | 105 | 7.7x |
| knip --reporter json | 3709 | 25.7 | 3707 | 3672 | 3771 | 7.7x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 899 | 20.0 | 895 | 873 | 961 | 1.0x |
| skif --no-js | 18.9 | 0.5 | 18.8 | 18.3 | 20.0 | 0.0x |
| sherif | 33.2 | 2.6 | 32.8 | 30.1 | 39.8 | 1.8x |
| syncpack lint | 235 | 2.3 | 234 | 232 | 241 | 12.4x |
| knip --reporter json | 5869 | 44.3 | 5860 | 5801 | 5949 | 6.5x |

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 106 | 3.6 | 105 | 101 | 112 | 1.0x |
| skif --no-js | 6.3 | 0.1 | 6.3 | 6.1 | 6.4 | 0.1x |
| sherif | 29.8 | 1.3 | 30.0 | 27.2 | 32.8 | 4.7x |
| syncpack lint | 60.5 | 1.5 | 60.4 | 58.1 | 64.2 | 9.6x |
| knip --reporter json | 735 | 7.3 | 735 | 726 | 751 | 7.0x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 28.9 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 259.3 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 7.7x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 6.5x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 7.0x) | pass |
