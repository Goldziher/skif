# skif benchmark results

skif 0.4.0 at `ba896a935d0c`; hyperfine `hyperfine 1.20.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

Machine: Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, AMD EPYC 9V45 96-Core Processor, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft; load average 4.34 / 2.96 / 1.35 before and 1.42 / 2.19 / 1.43 after the run.

Runner: github-hosted `GitHub Actions 1000196709`, image `ubuntu24` `20261101.360.1`, Linux X64; [workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43407846396/attempts/1).

Toolchain: rustc `rustc 1.100.0 (0ef1bc386 2026-11-09)`, node `v26.13.0`, npm `11.20.0`, pnpm `11.24.0`, yarn `1.22.22`.

Comparison tools: knip `6.44.1`, sherif `sherif 1.13.0`, syncpack `syncpack 15.3.3`.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 3.7 | 0.1 | 3.7 | 3.6 | 4.0 | 1.0x |
| sherif | 26.2 | 0.7 | 26.1 | 24.9 | 27.5 | 7.0x |
| syncpack lint | 34.5 | 1.9 | 33.4 | 32.1 | 38.9 | 9.2x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 22.4 | 5.6 | 20.3 | 19.6 | 37.9 | 1.0x |
| sherif | 42.3 | 1.2 | 42.2 | 40.5 | 44.1 | 1.9x |
| syncpack lint | 117 | 5.2 | 118 | 111 | 133 | 5.2x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 79.1 | 1.9 | 79.2 | 76.4 | 82.8 | 1.0x |
| sherif | 98.7 | 2.1 | 98.3 | 95.6 | 103 | 1.2x |
| syncpack lint | 517 | 8.8 | 516 | 505 | 540 | 6.5x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 171 | 7.8 | 169 | 161 | 195 | 1.0x |

## corpus: create-t3-turbo (pnpm)

Installed with `pnpm 10.19.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 24.5 | 0.7 | 24.4 | 23.4 | 26.0 | 1.0x |
| skif --no-js | 3.0 | 0.1 | 3.0 | 2.9 | 3.1 | 0.1x |
| sherif | 24.4 | 1.2 | 24.1 | 23.0 | 27.7 | 8.2x |
| syncpack lint | 28.8 | 1.1 | 28.7 | 26.6 | 30.2 | 9.7x |
| knip --reporter json | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.

## corpus: vue-core (pnpm)

Installed with `pnpm 12.11.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 47.1 | 2.8 | 46.3 | 45.1 | 54.9 | 1.0x |
| skif --no-js | 3.0 | 0.1 | 3.0 | 2.9 | 3.2 | 0.1x |
| sherif | 23.9 | 1.6 | 24.1 | 22.0 | 28.0 | 7.9x |
| syncpack lint | 31.4 | 1.5 | 31.9 | 28.7 | 33.5 | 10.3x |
| knip --reporter json | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - | - | - | - | - |

knip --reporter json exited with an error in 15 of 15 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading packages-private/sfc-playground/vite.config.ts (__DEV__ is not defined)`.

## corpus: tanstack-query (pnpm)

Installed with `pnpm 12.4.2`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 305 | 15.9 | 302 | 288 | 335 | 1.0x |
| skif --no-js | 8.4 | 0.2 | 8.5 | 8.1 | 8.9 | 0.0x |
| sherif | 31.7 | 1.0 | 31.3 | 30.1 | 34.3 | 3.8x |
| syncpack lint | 86.2 | 4.5 | 84.4 | 83.2 | 101 | 10.2x |
| knip --reporter json | 2768 | 47.4 | 2762 | 2677 | 2862 | 9.1x |

## corpus: babel (yarn)

Installed with `yarn 4.17.0`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 2157 | 25.6 | 2152 | 2104 | 2192 | 1.0x |
| skif --no-js | 12.4 | 0.6 | 12.3 | 11.7 | 13.8 | 0.0x |
| sherif | 29.3 | 1.0 | 29.2 | 28.0 | 31.4 | 2.4x |
| syncpack lint | 264 | 1.9 | 264 | 261 | 267 | 21.3x |
| knip --reporter json | 4113 | 122 | 4134 | 3847 | 4297 | 1.9x |

## corpus: npm-cli (npm)

Installed with `npx --yes npm@11.6.2 ci --ignore-scripts`.

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 70.9 | 3.1 | 70.1 | 66.8 | 75.1 | 1.0x |
| skif --no-js | 4.8 | 0.1 | 4.8 | 4.6 | 5.0 | 0.1x |
| sherif | 25.5 | 1.4 | 25.4 | 23.8 | 28.0 | 5.3x |
| syncpack lint | 61.2 | 6.6 | 59.3 | 56.8 | 84.5 | 12.7x |
| knip --reporter json | 544 | 19.9 | 538 | 522 | 600 | 7.7x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 22.4 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 171.1 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide) | not measured |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its SFC playground Vite config reads a build-time global that only the repository's own build defines) | not measured |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 9.1x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 1.9x) | FAIL |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 7.7x) | pass |
