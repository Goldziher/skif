# skif benchmark results

hyperfine `hyperfine 1.20.0`, 3 warmup runs, 10 measured runs. Times are wall clock, warm file cache.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 10.8 | 0.3 | 10.8 | 10.0 | 11.2 | 1.0x |
| sherif | 46.7 | 18.6 | 37.7 | 33.9 | 92.8 | 4.3x |
| syncpack lint | 41.8 | 1.2 | 41.7 | 40.3 | 43.6 | 3.9x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 54.1 | 2.1 | 53.9 | 51.8 | 59.0 | 1.0x |
| sherif | 61.5 | 1.5 | 61.3 | 59.9 | 64.5 | 1.1x |
| syncpack lint | 183 | 18.7 | 176 | 158 | 220 | 3.4x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 272 | 17.2 | 268 | 252 | 307 | 1.0x |
| sherif | 194 | 5.0 | 194 | 183 | 204 | 0.7x |
| syncpack lint | 849 | 172 | 869 | 651 | 1051 | 3.1x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 377 | 217 | 318 | 219 | 921 | 1.0x |

## corpus: create-t3-turbo (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 22.2 | 0.9 | 22.3 | 20.1 | 23.2 | 1.0x |
| skif --no-js | 4.6 | 0.2 | 4.6 | 4.4 | 4.9 | 0.2x |
| sherif | 31.6 | 1.3 | 32.0 | 29.6 | 34.1 | 6.8x |
| syncpack lint | 47.6 | 8.3 | 45.9 | 37.7 | 62.7 | 10.3x |
| knip --reporter json | 820 | 12.3 | 824 | 802 | 834 | 37.0x |

## corpus: vue-core (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 37.3 | 2.0 | 37.4 | 34.3 | 40.0 | 1.0x |
| skif --no-js | 5.0 | 0.3 | 5.0 | 4.7 | 5.5 | 0.1x |
| sherif | 33.0 | 1.1 | 33.2 | 31.4 | 34.7 | 6.6x |
| syncpack lint | 38.5 | 0.7 | 38.5 | 37.5 | 39.7 | 7.7x |
| knip --reporter json | 1589 | 974 | 1316 | 638 | 4080 | 42.6x |

## corpus: tanstack-query (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 180 | 11.3 | 175 | 170 | 198 | 1.0x |
| skif --no-js | 17.1 | 0.5 | 17.1 | 16.5 | 18.2 | 0.1x |
| sherif | 41.1 | 1.2 | 41.2 | 39.5 | 43.2 | 2.4x |
| syncpack lint | 109 | 1.6 | 109 | 107 | 111 | 6.4x |
| knip --reporter json | 1802 | 13.9 | 1803 | 1777 | 1826 | 10.0x |

## corpus: babel (yarn)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 421 | 12.3 | 420 | 401 | 441 | 1.0x |
| skif --no-js | 13.0 | 0.4 | 13.0 | 12.4 | 13.5 | 0.0x |
| sherif | 39.8 | 1.8 | 39.5 | 37.4 | 43.1 | 3.1x |
| syncpack lint | 496 | 23.3 | 495 | 473 | 540 | 38.2x |
| knip --reporter json | 4152 | 594 | 3943 | 3521 | 5420 | 9.9x |

## corpus: npm-cli (npm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 83.4 | 3.7 | 82.0 | 79.6 | 91.4 | 1.0x |
| skif --no-js | 6.8 | 1.1 | 6.5 | 5.7 | 9.5 | 0.1x |
| sherif | 35.2 | 1.9 | 34.6 | 33.3 | 38.9 | 5.2x |
| syncpack lint | 87.9 | 2.0 | 88.7 | 85.0 | 90.5 | 12.9x |
| knip --reporter json | 316 | 5.1 | 315 | 310 | 328 | 3.8x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 54.1 ms) | FAIL |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 376.5 ms) | pass |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (measured 37.0x) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (measured 42.6x) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 10.0x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 9.9x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 3.8x) | FAIL |

