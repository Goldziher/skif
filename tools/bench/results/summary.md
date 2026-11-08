# skif benchmark results

hyperfine `hyperfine 2.0.0`, 3 warmup runs, 15 measured runs. Times are wall clock, warm file cache.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 6.3 | 0.5 | 6.3 | 5.5 | 7.4 | 1.0x |
| sherif | 40.4 | 6.1 | 38.8 | 30.9 | 55.3 | 6.4x |
| syncpack lint | 46.8 | 5.2 | 46.3 | 39.4 | 62.7 | 7.4x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 27.1 | 2.7 | 26.9 | 24.0 | 31.6 | 1.0x |
| sherif | 59.0 | 4.7 | 58.8 | 50.2 | 68.2 | 2.2x |
| syncpack lint | 155 | 11.4 | 156 | 130 | 178 | 5.7x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 99.4 | 7.1 | 102 | 87.8 | 108 | 1.0x |
| sherif | 131 | 12.4 | 130 | 114 | 158 | 1.3x |
| syncpack lint | 649 | 52.9 | 655 | 533 | 739 | 6.5x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 276 | 24.0 | 271 | 249 | 332 | 1.0x |

## corpus: create-t3-turbo (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 33.2 | 3.4 | 31.9 | 30.1 | 43.1 | 1.0x |
| skif --no-js | 4.9 | 0.6 | 4.6 | 4.2 | 5.8 | 0.1x |
| sherif | 35.4 | 3.3 | 36.0 | 29.7 | 41.5 | 7.2x |
| syncpack lint | 41.1 | 3.2 | 41.0 | 36.5 | 48.7 | 8.4x |
| knip --reporter json | 2011 | 105 | 2011 | 1776 | 2233 | 60.7x |

knip --reporter json exited with an error in 15 of 15 runs, so its time may be that of a partial run.

## corpus: vue-core (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 62.7 | 5.3 | 61.9 | 55.1 | 79.5 | 1.0x |
| skif --no-js | 5.5 | 0.6 | 5.5 | 4.7 | 6.6 | 0.1x |
| sherif | 36.7 | 4.5 | 35.6 | 30.6 | 44.9 | 6.7x |
| syncpack lint | 42.7 | 3.3 | 42.5 | 37.9 | 50.9 | 7.8x |
| knip --reporter json | 1688 | 84.0 | 1657 | 1581 | 1838 | 26.9x |

knip --reporter json exited with an error in 15 of 15 runs, so its time may be that of a partial run.

## corpus: tanstack-query (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 375 | 16.9 | 375 | 352 | 406 | 1.0x |
| skif --no-js | 12.2 | 3.6 | 11.1 | 10.7 | 24.9 | 0.0x |
| sherif | 43.2 | 4.5 | 42.0 | 36.2 | 53.0 | 3.6x |
| syncpack lint | 111 | 11.7 | 106 | 100.0 | 140 | 9.2x |
| knip --reporter json | 4563 | 148 | 4527 | 4381 | 4828 | 12.2x |

## corpus: babel (yarn)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 884 | 54.8 | 884 | 758 | 999 | 1.0x |
| skif --no-js | 9.2 | 0.9 | 9.4 | 7.7 | 11.1 | 0.0x |
| sherif | 37.5 | 2.2 | 37.3 | 34.4 | 41.0 | 4.1x |
| syncpack lint | 277 | 21.3 | 280 | 238 | 309 | 30.2x |
| knip --reporter json | 7181 | 233 | 7217 | 6693 | 7475 | 8.1x |

## corpus: npm-cli (npm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 99.8 | 9.4 | 98.6 | 84.0 | 126 | 1.0x |
| skif --no-js | 5.3 | 0.4 | 5.3 | 4.6 | 6.1 | 0.1x |
| sherif | 38.0 | 5.7 | 38.0 | 29.2 | 47.8 | 7.2x |
| syncpack lint | 79.1 | 7.0 | 77.2 | 70.7 | 91.2 | 14.9x |
| knip --reporter json | 960 | 94.7 | 968 | 833 | 1170 | 9.6x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 27.1 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 275.8 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (measured 60.7x; knip exited with an error in 15 runs) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (measured 26.9x; knip exited with an error in 15 runs) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 12.2x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 8.1x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 9.6x) | pass |
