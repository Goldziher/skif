# skif benchmark results

skif 0.5.0 at `e46391c1ab1c`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, AMD EPYC 9V74 80-Core Processor, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft; load average 4.79 / 3.53 / 1.67 before and 1.52 / 2.46 / 1.71 after the run.

Runner: github-hosted `GitHub Actions 1000198101`, image `ubuntu24` `20261111.370.1`, Linux X64; [workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43595557667/attempts/1).

Toolchain: rustc `rustc 1.100.0 (0ef1bc386 2026-11-09)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.44.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 4.2 | 0.1 | 4.2 | 4.1 | 4.3 | 1.0x |
| sherif | 29.5 | 1.1 | 29.3 | 28.0 | 32.4 | 7.0x |
| syncpack lint | 55.3 | 36.3 | 40.9 | 38.2 | 172 | 13.1x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 23.7 | 0.5 | 23.7 | 22.8 | 24.5 | 1.0x |
| sherif | 52.0 | 1.0 | 52.2 | 50.5 | 53.9 | 2.2x |
| syncpack lint | 151 | 1.3 | 151 | 148 | 154 | 6.4x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 104 | 3.6 | 104 | 97.9 | 110 | 1.0x |
| sherif | 126 | 2.2 | 126 | 123 | 131 | 1.2x |
| syncpack lint | 680 | 12.6 | 676 | 669 | 712 | 6.5x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 216 | 3.7 | 216 | 210 | 227 | 1.0x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 31.5 | 0.8 | 31.3 | 29.9 | 32.7 | 1.0x |
| skif --no-js | 3.3 | 0.0 | 3.3 | 3.2 | 3.4 | 0.1x |
| sherif | 29.0 | 3.3 | 28.0 | 26.0 | 36.6 | 8.7x |
| syncpack lint | 35.5 | 3.3 | 34.5 | 32.8 | 46.2 | 10.6x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 60.7 | 0.8 | 60.5 | 60.0 | 62.8 | 1.0x |
| skif --no-js | 3.5 | 0.1 | 3.5 | 3.5 | 3.7 | 0.1x |
| sherif | 29.0 | 1.0 | 28.7 | 27.7 | 32.1 | 8.2x |
| syncpack lint | 36.4 | 0.9 | 36.0 | 35.2 | 38.4 | 10.3x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 375 | 2.6 | 375 | 370 | 380 | 1.0x |
| skif --no-js | 9.4 | 0.3 | 9.3 | 9.1 | 10.3 | 0.0x |
| sherif | 34.4 | 0.8 | 34.3 | 33.3 | 36.3 | 3.7x |
| syncpack lint | 101 | 1.1 | 102 | 99.4 | 103 | 10.8x |
| knip --reporter json | 3371 | 24.8 | 3384 | 3325 | 3403 | 9.0x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 837 | 13.0 | 835 | 817 | 861 | 1.0x |
| skif --no-js | 14.8 | 0.4 | 14.9 | 14.2 | 15.7 | 0.0x |
| sherif | 30.7 | 0.6 | 30.4 | 30.0 | 32.4 | 2.1x |
| syncpack lint | 299 | 1.5 | 299 | 297 | 302 | 20.1x |
| knip --reporter json | 5044 | 68.9 | 5038 | 4935 | 5191 | 6.0x |

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 87.2 | 4.6 | 86.2 | 81.0 | 96.0 | 1.0x |
| skif --no-js | 5.4 | 0.1 | 5.4 | 5.2 | 5.5 | 0.1x |
| sherif | 28.5 | 1.1 | 28.0 | 26.8 | 30.8 | 5.3x |
| syncpack lint | 69.3 | 1.4 | 69.0 | 67.5 | 72.2 | 12.9x |
| knip --reporter json | 670 | 5.8 | 668 | 662 | 680 | 7.7x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 23.7 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 216.1 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 9.0x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 6.0x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 7.7x) | pass |
