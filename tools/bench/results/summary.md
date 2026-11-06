# skif benchmark results

hyperfine `hyperfine 1.20.0`, 3 warmup runs, 10 measured runs. Times are wall clock, warm file cache.

## generated: 50 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 129 | 32.5 | 126 | 93.9 | 191 | 1.0x |
| sherif | 347 | 132 | 347 | 203 | 578 | 2.7x |
| syncpack lint | 564 | 192 | 585 | 294 | 832 | 4.4x |

## generated: 500 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 224 | 51.3 | 242 | 126 | 288 | 1.0x |
| sherif | 439 | 130 | 370 | 302 | 699 | 2.0x |
| syncpack lint | 861 | 402 | 863 | 406 | 1757 | 3.8x |

## generated: 2000 packages (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif --no-js | 417 | 65.5 | 404 | 345 | 528 | 1.0x |
| sherif | 741 | 419 | 558 | 347 | 1579 | 1.8x |
| syncpack lint | 1319 | 470 | 1154 | 1031 | 2590 | 3.2x |

## graph: 10000 files in 100 packages

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 2620 | 950 | 2360 | 1678 | 4439 | 1.0x |

## corpus: create-t3-turbo (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 91.8 | 20.8 | 91.5 | 63.8 | 128 | 1.0x |
| skif --no-js | 19.9 | 10.7 | 17.9 | 7.9 | 39.5 | 0.2x |
| sherif | 215 | 53.7 | 202 | 150 | 302 | 10.8x |
| syncpack lint | 241 | 72.5 | 211 | 165 | 358 | 12.1x |
| knip --reporter json | 2891 | 804 | 2634 | 2106 | 4378 | 31.5x |

## corpus: vue-core (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 185 | 41.4 | 176 | 128 | 259 | 1.0x |
| skif --no-js | 40.4 | 12.0 | 43.5 | 23.0 | 55.2 | 0.2x |
| sherif | 177 | 32.2 | 191 | 107 | 210 | 4.4x |
| syncpack lint | 175 | 20.4 | 173 | 152 | 220 | 4.3x |
| knip --reporter json | 3081 | 1668 | 2861 | 1695 | 7523 | 16.6x |

## corpus: tanstack-query (pnpm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 753 | 137 | 818 | 572 | 890 | 1.0x |
| skif --no-js | 59.0 | 19.4 | 62.7 | 25.5 | 94.2 | 0.1x |
| sherif | 196 | 38.4 | 188 | 140 | 268 | 3.3x |
| syncpack lint | 371 | 64.6 | 366 | 290 | 474 | 6.3x |
| knip --reporter json | 6016 | 1478 | 5714 | 4443 | 8275 | 8.0x |

## corpus: babel (yarn)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 1124 | 65.6 | 1115 | 1041 | 1223 | 1.0x |
| skif --no-js | 115 | 18.4 | 119 | 91.0 | 144 | 0.1x |
| sherif | 404 | 152 | 358 | 204 | 668 | 3.5x |
| syncpack lint | 1507 | 126 | 1473 | 1322 | 1714 | 13.1x |
| knip --reporter json | 21279 | 13176 | 16257 | 13063 | 55372 | 18.9x |

## corpus: npm-cli (npm)

| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| skif | 251 | 63.1 | 241 | 183 | 408 | 1.0x |
| skif --no-js | 37.8 | 14.8 | 35.1 | 21.3 | 62.9 | 0.2x |
| sherif | 340 | 54.4 | 334 | 267 | 429 | 9.0x |
| syncpack lint | 467 | 75.0 | 464 | 361 | 557 | 12.4x |
| knip --reporter json | 2494 | 537 | 2655 | 1630 | 3404 | 9.9x |

## ADR 0001 targets

| Target | Check | Result |
| --- | --- | --- |
| generated: 500 packages (pnpm) | manifest and policy rules < 50 ms (measured 224.0 ms) | FAIL |
| graph: 10000 files in 100 packages | parse and module graph of 10000 files < 2000 ms, whole run including every JS rule (measured 2619.7 ms) | FAIL |
| corpus: create-t3-turbo (pnpm) | JS analysis >= 5x faster than knip (measured 31.5x) | pass |
| corpus: vue-core (pnpm) | JS analysis >= 5x faster than knip (measured 16.6x) | pass |
| corpus: tanstack-query (pnpm) | JS analysis >= 5x faster than knip (measured 8.0x) | pass |
| corpus: babel (yarn) | JS analysis >= 5x faster than knip (measured 18.9x) | pass |
| corpus: npm-cli (npm) | JS analysis >= 5x faster than knip (measured 9.9x) | pass |
