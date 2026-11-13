# skif benchmark results

skif 0.4.0 at `84ed0fba77f8`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, Intel(R) Xeon(R) Platinum 8370C CPU @ 2.80GHz, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft; load average 4.17 / 3.46 / 1.73 before and 1.50 / 1.28 / 1.16 after the run.

Runner: github-hosted `GitHub Actions 1000196310`, image `ubuntu24` `20261111.370.1`, Linux X64; [workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43282845612/attempts/1).

Toolchain: rustc `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.44.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 4.6 | 0.2 | 4.6 | 4.4 | 5.0 | 1.0x |
| sherif | 32.0 | 1.8 | 31.3 | 29.8 | 35.5 | 6.9x |
| syncpack lint | 42.0 | 1.7 | 41.9 | 39.9 | 45.8 | 9.1x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 25.0 | 0.5 | 25.1 | 24.1 | 25.6 | 1.0x |
| sherif | 51.5 | 1.1 | 51.2 | 49.9 | 53.3 | 2.1x |
| syncpack lint | 151 | 1.7 | 151 | 148 | 155 | 6.1x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 101 | 1.4 | 101 | 98.9 | 104 | 1.0x |
| sherif | 122 | 2.1 | 122 | 117 | 128 | 1.2x |
| syncpack lint | 669 | 4.2 | 668 | 659 | 676 | 6.6x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 246 | 3.9 | 245 | 239 | 254 | 1.0x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 36.5 | 0.8 | 36.7 | 34.9 | 37.7 | 1.0x |
| skif --no-js | 3.4 | 0.1 | 3.4 | 3.3 | 3.7 | 0.1x |
| sherif | 30.6 | 1.4 | 30.4 | 28.7 | 33.6 | 9.0x |
| syncpack lint | 47.9 | 26.1 | 38.7 | 35.1 | 139 | 14.1x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 71.7 | 0.7 | 71.4 | 71.0 | 73.3 | 1.0x |
| skif --no-js | 3.5 | 0.1 | 3.5 | 3.4 | 3.7 | 0.0x |
| sherif | 30.3 | 0.8 | 30.4 | 28.7 | 31.6 | 8.6x |
| syncpack lint | 36.8 | 1.3 | 36.9 | 34.4 | 38.8 | 10.4x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 437 | 3.1 | 437 | 431 | 442 | 1.0x |
| skif --no-js | 10.6 | 0.2 | 10.6 | 10.2 | 11.0 | 0.0x |
| sherif | 37.4 | 1.3 | 38.0 | 35.4 | 39.9 | 3.5x |
| syncpack lint | 100 | 1.4 | 100.0 | 97.8 | 103 | 9.5x |
| knip --reporter json | 4007 | 31.6 | 4009 | 3958 | 4086 | 9.2x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 492360 | 1885 | 492359 | 489629 | 495536 | 1.0x |
| skif --no-js | 17.3 | 0.7 | 17.1 | 16.5 | 19.4 | 0.0x |
| sherif | 35.3 | 1.6 | 35.0 | 32.5 | 38.6 | 2.0x |
| syncpack lint | 234 | 4.7 | 233 | 230 | 248 | 13.6x |
| knip --reporter json | 6564 | 31.4 | 6554 | 6534 | 6624 | 0.0x |

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 101 | 3.2 | 99.8 | 97.9 | 109 | 1.0x |
| skif --no-js | 6.2 | 0.1 | 6.3 | 6.0 | 6.5 | 0.1x |
| sherif | 32.1 | 1.8 | 32.0 | 29.3 | 34.7 | 5.1x |
| syncpack lint | 63.9 | 2.0 | 63.4 | 60.8 | 69.3 | 10.2x |
| knip --reporter json | 829 | 7.2 | 831 | 815 | 840 | 8.2x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 25.0 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 245.8 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 9.2x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 0.0x) | FAIL |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 8.2x) | pass |
