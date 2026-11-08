# skif benchmark results

hyperfine `hyperfine 2.0.0`, 3 warmup runs, 10 measured runs. Times are wall clock, warm file cache.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 6.6 | 0.4 | 6.4 | 6.0 | 7.3 | 1.0x |
| sherif | 39.0 | 2.2 | 38.7 | 36.3 | 42.6 | 6.0x |
| syncpack lint | 52.0 | 4.2 | 52.6 | 46.4 | 58.0 | 7.9x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 34.7 | 2.7 | 34.3 | 31.0 | 40.5 | 1.0x |
| sherif | 69.4 | 8.5 | 67.8 | 59.1 | 86.4 | 2.0x |
| syncpack lint | 157 | 13.7 | 159 | 131 | 183 | 4.5x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 137 | 7.0 | 137 | 127 | 150 | 1.0x |
| sherif | 138 | 7.3 | 135 | 127 | 151 | 1.0x |
| syncpack lint | 642 | 56.5 | 665 | 534 | 693 | 4.7x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 282 | 15.5 | 280 | 265 | 316 | 1.0x |

## corpus: create-t3-turbo (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 35.3 | 1.8 | 35.2 | 32.8 | 38.8 | 1.0x |
| skif --no-js | 5.7 | 3.3 | 4.7 | 4.3 | 15.0 | 0.2x |
| sherif | 39.2 | 5.1 | 39.2 | 31.4 | 46.0 | 6.9x |
| syncpack lint | 44.1 | 3.2 | 43.7 | 40.2 | 49.8 | 7.8x |
| knip --reporter json | 2164 | 81.9 | 2142 | 2086 | 2349 | 61.3x |

knip --reporter json exited with an error in 10 of 10 runs, so its time may be that of a partial run.

## corpus: vue-core (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 73.9 | 6.9 | 72.0 | 66.4 | 89.4 | 1.0x |
| skif --no-js | 4.7 | 0.3 | 4.7 | 4.0 | 5.1 | 0.1x |
| sherif | 39.0 | 4.7 | 38.2 | 31.6 | 50.1 | 8.4x |
| syncpack lint | 50.8 | 4.1 | 50.2 | 46.4 | 58.9 | 10.9x |
| knip --reporter json | 1885 | 75.8 | 1862 | 1793 | 2034 | 25.5x |

knip --reporter json exited with an error in 10 of 10 runs, so its time may be that of a partial run.

## corpus: tanstack-query (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 416 | 10.6 | 415 | 402 | 440 | 1.0x |
| skif --no-js | 12.2 | 0.5 | 12.5 | 11.3 | 12.9 | 0.0x |
| sherif | 48.6 | 6.4 | 46.5 | 42.9 | 61.8 | 4.0x |
| syncpack lint | 124 | 12.3 | 124 | 97.1 | 145 | 10.1x |
| knip --reporter json | 5295 | 208 | 5360 | 4896 | 5570 | 12.7x |

## corpus: babel (yarn)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 1040 | 72.2 | 1037 | 914 | 1124 | 1.0x |
| skif --no-js | 10.7 | 1.2 | 10.2 | 9.6 | 13.5 | 0.0x |
| sherif | 45.6 | 7.3 | 44.4 | 34.4 | 62.2 | 4.3x |
| syncpack lint | 338 | 34.3 | 337 | 280 | 402 | 31.7x |
| knip --reporter json | 8514 | 505 | 8497 | 7648 | 9407 | 8.2x |

## corpus: npm-cli (npm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 124 | 8.2 | 124 | 106 | 135 | 1.0x |
| skif --no-js | 6.0 | 0.6 | 5.9 | 5.2 | 7.3 | 0.0x |
| sherif | 45.4 | 6.0 | 43.5 | 38.3 | 54.4 | 7.6x |
| syncpack lint | 86.7 | 6.2 | 87.7 | 73.5 | 94.1 | 14.4x |
| knip --reporter json | 1063 | 81.8 | 1051 | 956 | 1221 | 8.5x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 34.7 ms) | pass |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 282.3 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (measured 61.3x; knip exited with an error in 10 runs) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (measured 25.5x; knip exited with an error in 10 runs) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 12.7x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 8.2x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 8.5x) | pass |
