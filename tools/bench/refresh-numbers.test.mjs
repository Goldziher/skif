// Tests of refresh-numbers.mjs. Run with `node --test tools/bench/refresh-numbers.test.mjs`.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  baselineProblems,
  benchClaims,
  benchDetail,
  benchSummary,
  duration,
  markers,
  mdxText,
  mebibytes,
  parityAgreement,
  pinnedTools,
  provenance,
  readmeClaims,
  replaceBlock,
  speedClaim,
  summaryProblems,
} from "./refresh-numbers.mjs";

// Synthetic tool versions: these tests exercise the rendering, not the versions shipped in corpus.toml.
const KNIP = "1.2.3";
const SHERIF = "4.5.6";
const SYNCPACK = "7.8.9";

const row = (tool, median_ms, extra = {}) => ({
  tool,
  mean_ms: median_ms,
  stddev_ms: null,
  median_ms,
  min_ms: median_ms,
  max_ms: median_ms,
  relative_to_skif: 1,
  error_runs: 0,
  ...extra,
});

const MIB = 1024 * 1024;
/** The peak RSS of a skif mode's row, in mebibytes. */
const rss = (mebibytes) => ({ peak_rss_bytes: mebibytes * MIB });

/** The machine of a GitHub-hosted x86 runner, as the harness records it there. */
function machine() {
  return {
    os: "Ubuntu 24.04.3 LTS",
    kernel: "Linux 6.11.0-1018-azure",
    arch: "x86_64",
    cpu: "AMD EPYC 7763 64-Core Processor",
    logical_cpus: 4,
    available_cpus: 4,
    physical_cores: 2,
    memory_bytes: 16_374_836 * 1024,
    cgroup_cpu_limit: null,
    virtualization: "Microsoft",
    github: {
      image_os: "ubuntu24",
      image_version: "20261102.1.0",
      runner_os: "Linux",
      runner_arch: "X64",
      runner_environment: "github-hosted",
      runner_name: "GitHub Actions 1000012345",
      run_attempt: 2,
      run_url: "https://github.com/Goldziher/skif/actions/runs/19283746/attempts/2",
    },
  };
}

function summary() {
  return {
    schema: 7,
    skif: "skif 0.3.0",
    source_rev: "0123456789ab",
    harness_rev: "0123456789ab",
    measured_at: 1_794_268_800,
    machine: machine(),
    machine_text:
      "Ubuntu 24.04.3 LTS, Linux 6.11.0-1018-azure, x86_64, AMD EPYC 7763 64-Core Processor, 4 logical CPUs " +
      "(2 physical cores), 15.6 GiB memory, hypervisor Microsoft",
    toolchain: {
      rustc: "rustc 1.99.0 (0123abcd 2026-10-01)",
      node: "v26.13.0",
      npm: "11.6.2",
      pnpm: "12.16.1",
      yarn: null,
      bun: null,
    },
    load_before: [1.5, 1.25, 1],
    load_after: [2, 1.5, 1.25],
    hyperfine: "hyperfine 1.20.0",
    tool_versions: { knip: KNIP, sherif: `sherif ${SHERIF}`, syncpack: SYNCPACK },
    warmup: 3,
    runs: 15,
    corpus_only: [],
    skipped_tools: [],
    targets: [
      {
        target: "generated: 500 packages (pnpm)",
        kind: "generated",
        packages: 500,
        files: null,
        warmup: 3,
        runs: 15,
        results: [row("skif --no-js", 27), row("sherif", 59), row("syncpack lint", 156)],
      },
      {
        target: "corpus: babel (yarn)",
        kind: "corpus",
        packages: null,
        files: null,
        installed_with: "yarn 4.10.3",
        warmup: 3,
        runs: 15,
        results: [
          row("skif", 884, rss(400)),
          row("skif --no-js", 9.4),
          row("sherif", 37),
          row("syncpack lint", 280),
          row("knip --reporter json", 7200, { relative_to_skif: 8.1 }),
          row("skif --type-aware", 1326, rss(1200)),
        ],
      },
    ],
    adr_0001_checks: [{ target: "corpus: babel (yarn)", description: "d", status: "pass" }],
    type_aware_budget_checks: [{ target: "corpus: babel (yarn)", description: "b", status: "pass" }],
  };
}

test("pinnedTools reads the [tools] table of corpus.toml and stops at the next table", () => {
  const toml = '# c\n[tools]\nknip = "6.44.1"\nsyncpack = "15.3.3"\nsherif = "1.13.0"\n\n[repos.a]\nrev = "x"\n';
  assert.deepEqual(pinnedTools(toml), { knip: "6.44.1", syncpack: "15.3.3", sherif: "1.13.0" });
  assert.throws(() => pinnedTools("[repos.a]\n"), /no \[tools\] table/);
  const real = pinnedTools(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "corpus.toml"), "utf8"));
  assert.deepEqual(Object.keys(real).sort(), ["knip", "sherif", "syncpack"]);
});

test("a summary measured against the pinned tool versions is accepted, whether or not a version carries its tool name", () => {
  assert.deepEqual(summaryProblems(summary(), { knip: KNIP, syncpack: SYNCPACK, sherif: SHERIF }), []);
});

test("a summary measured against other tool versions than corpus.toml pins is refused", () => {
  const pinned = { knip: "6.44.1", syncpack: "15.3.3", sherif: "1.13.0" };
  const stale = { ...summary(), tool_versions: { knip: "6.40.0", sherif: "sherif 1.12.0", syncpack: "15.3.3" } };
  assert.deepEqual(summaryProblems(stale, pinned), [
    "the summary was measured against knip 6.40.0, but corpus.toml pins 6.44.1; measure again with the bench workflow",
    "the summary was measured against sherif 1.12.0, but corpus.toml pins 1.13.0; measure again with the bench workflow",
  ]);
  const missing = { ...summary(), tool_versions: { knip: "6.44.1", sherif: "sherif 1.13.0" } };
  assert.deepEqual(summaryProblems(missing, pinned), [
    "the summary records no syncpack version; corpus.toml pins syncpack 15.3.3",
  ]);
});

test("a complete summary has no problems and names what was measured, on which runner and in which run", () => {
  assert.deepEqual(summaryProblems(summary()), []);
  assert.equal(
    provenance(summary()),
    "Medians of 15 runs after 3 warmup runs (hyperfine 1.20.0), skif 0.3.0 at `0123456789ab` built with " +
      "rustc 1.99.0 (0123abcd 2026-10-01) against knip " +
      KNIP +
      ", sherif " +
      SHERIF +
      ", syncpack " +
      SYNCPACK +
      ", measured on " +
      "2026-11-10 on a GitHub-hosted runner, image ubuntu24 20261102.1.0, Ubuntu 24.04.3 LTS, " +
      "Linux 6.11.0-1018-azure, x86_64, AMD EPYC 7763 64-Core Processor, 4 logical CPUs (2 physical cores), " +
      "15.6 GiB memory, hypervisor Microsoft, load average 1.50 / 1.25 / 1.00 before and 2.00 / 1.50 / 1.25 after " +
      "the run ([workflow run, attempt 2](https://github.com/Goldziher/skif/actions/runs/19283746/attempts/2)).",
  );
});

test("a summary in which a tool exited with an error is refused, with the cause the harness recorded", () => {
  const errored = summary();
  const knip = errored.targets[1].results.find((r) => r.tool === "knip --reporter json");
  knip.error_runs = 15;
  knip.error_output = "ERROR: Error loading apps/expo/metro.config.js";
  errored.targets[0].results[1].error_runs = 1;
  errored.adr_0001_checks[0].status = "invalid";
  assert.deepEqual(summaryProblems(errored), [
    "generated: 500 packages (pnpm): sherif exited with an error in 1 of 15 runs, so its time may be that of a partial run",
    "corpus: babel (yarn): knip --reporter json exited with an error in 15 of 15 runs " +
      "(ERROR: Error loading apps/expo/metro.config.js), so its time may be that of a partial run",
    "corpus: babel (yarn): the ADR 0001 check is invalid: d",
  ]);
});

const METRO_ERROR = "ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')";
const METRO_REASON = "its Expo Metro config needs metro-cache, which pnpm's strict install does not provide";
const GOAL = "JS analysis >= 5x faster than knip";
const DECLARATION = "corpus.toml declares that knip fails here with `Cannot find module 'metro-cache'`";

/**
 * The summary with a create-t3-turbo target on which knip exited with an error in `errorRuns` runs, `failure` the
 * failure the harness matched to corpus.toml's declaration (or null), and its ADR 0001 check `check`.
 */
function withT3({ errorRuns, failure, check }) {
  const result = summary();
  result.targets.push({
    target: "corpus: create-t3-turbo (pnpm)",
    kind: "corpus",
    packages: null,
    files: null,
    installed_with: "pnpm 10.19.0",
    declared_knip_failure: { matches: "Cannot find module 'metro-cache'", reason: METRO_REASON, timed: false },
    warmup: 3,
    runs: 15,
    results: [
      row("skif", 33, rss(200)),
      row("skif --no-js", 4),
      row("sherif", 30),
      row("syncpack lint", 36),
      row("knip --reporter json", 1590, {
        relative_to_skif: failure == null ? 48.2 : null,
        error_runs: errorRuns,
        error_output: errorRuns > 0 ? METRO_ERROR : null,
        failure,
      }),
      row("skif --type-aware", 36.3, rss(300)),
    ],
  });
  result.adr_0001_checks.push({ target: "corpus: create-t3-turbo (pnpm)", ...check });
  return result;
}

test("a knip failure corpus.toml declares and the run matched is published as failing, without a ratio", () => {
  const matched = withT3({
    errorRuns: 15,
    failure: { reason: METRO_REASON, error: METRO_ERROR, timed: false },
    check: { description: `${GOAL} (not measured: knip fails here: ${METRO_REASON})`, status: "not-measured" },
  });
  assert.deepEqual(summaryProblems(matched), []);
  const compact = benchSummary(matched);
  assert.ok(compact.includes(`| create-t3-turbo | 33 ms | fails: ${METRO_REASON} | - |\n`), compact);
  assert.match(compact, /\| create-t3-turbo \| 4\.0 ms \| 30 ms \| 36 ms \|/);
  assert.match(compact, /\| babel \| 884 ms \| 7\.2 s \| 8x \|/);
  assert.ok(
    benchClaims(matched).startsWith(
      "- **8.1x faster than knip** for dead-code analysis, on 1 pinned public repository where knip completes; " +
        "it fails on create-t3-turbo.\n",
    ),
    benchClaims(matched),
  );
  const detail = benchDetail(matched);
  assert.ok(detail.includes(`| knip --reporter json | fails: ${METRO_REASON} | - | - |\n`), detail);
  assert.ok(detail.includes("| sherif | 30.0 | 30.0 | 1.0x |\n"), detail);
  assert.ok(
    detail.includes(
      `| corpus: create-t3-turbo (pnpm) | ${GOAL} (not measured: knip fails here: ${METRO_REASON}) | not measured |\n`,
    ),
    detail,
  );
});

test("a knip error that does not match the declaration, or that is not declared, is refused", () => {
  const stale =
    `${GOAL} (measured 48.2x; knip --reporter json exited with an error in 15 runs; ${DECLARATION}, ` +
    "but knip's error is another one: the declaration is stale)";
  const unmatched = withT3({ errorRuns: 15, failure: null, check: { description: stale, status: "invalid" } });
  assert.deepEqual(summaryProblems(unmatched), [
    "corpus: create-t3-turbo (pnpm): knip --reporter json exited with an error in 15 of 15 runs " +
      `(${METRO_ERROR}), so its time may be that of a partial run`,
    `corpus: create-t3-turbo (pnpm): the ADR 0001 check is invalid: ${stale}`,
  ]);
  const plain = `${GOAL} (measured 48.2x; knip --reporter json exited with an error in 15 runs)`;
  const undeclared = withT3({ errorRuns: 15, failure: null, check: { description: plain, status: "invalid" } });
  delete undeclared.targets[2].declared_knip_failure;
  assert.deepEqual(summaryProblems(undeclared), [
    "corpus: create-t3-turbo (pnpm): knip --reporter json exited with an error in 15 of 15 runs " +
      `(${METRO_ERROR}), so its time may be that of a partial run`,
    `corpus: create-t3-turbo (pnpm): the ADR 0001 check is invalid: ${plain}`,
  ]);
});

test("a declared knip failure that did not happen is refused through its invalid check", () => {
  const clean = `${GOAL} (measured 48.2x; ${DECLARATION}, but knip ran without an error: the declaration is stale)`;
  const declaredButClean = withT3({ errorRuns: 0, failure: null, check: { description: clean, status: "invalid" } });
  assert.deepEqual(summaryProblems(declaredButClean), [
    `corpus: create-t3-turbo (pnpm): the ADR 0001 check is invalid: ${clean}`,
  ]);
});

const JEST_ERROR = "ERROR: Error loading jest.config.js (Cannot find module './dist/build/jest/jest')";
const JEST_REASON = "its root Jest config requires the built next/jest";

/**
 * The summary with a next-js target timed in 5 runs after 1 warmup run, on which knip exited with an error after its
 * report in every run, as corpus.toml declares (`timed`), and whose check passed.
 */
function withNextJs() {
  const result = summary();
  result.targets.push({
    target: "corpus: next-js (pnpm)",
    kind: "corpus",
    packages: null,
    files: null,
    installed_with: "pnpm 12.9.1",
    declared_knip_failure: { matches: "Error loading jest.config.js", reason: JEST_REASON, timed: true },
    warmup: 1,
    runs: 5,
    results: [
      row("skif", 6000, rss(310)),
      row("skif --no-js", 40),
      row("sherif", 90),
      row("syncpack lint", 900),
      row("knip --reporter json", 30000, {
        relative_to_skif: 5,
        error_runs: 5,
        error_output: JEST_ERROR,
        failure: { reason: JEST_REASON, error: JEST_ERROR, timed: true },
      }),
      row("skif --type-aware", 18000, rss(3800)),
    ],
  });
  result.adr_0001_checks.push({ target: "corpus: next-js (pnpm)", description: "d", status: "pass" });
  return result;
}

test("a knip error declared as after the report is published with its time, its ratio and the reason", () => {
  const timed = withNextJs();
  assert.deepEqual(summaryProblems(timed), []);
  const compact = benchSummary(timed);
  assert.ok(compact.includes("| next-js | 6.0 s | 30.0 s | 5x |\n"), compact);
  assert.ok(
    compact.includes(
      `\n\nknip exits with an error after its report on next-js (${JEST_REASON}); its time there is that of a run ` +
        "without the entries of the configurations it could not load.\n",
    ),
    compact,
  );
  const detail = benchDetail(timed);
  assert.ok(detail.includes("| knip --reporter json | 30000 | 30000 | 5.0x |\n"), detail);
  assert.ok(
    detail.includes(
      `\n\nknip --reporter json exits with an error after its report here, as corpus.toml declares: ${JEST_REASON}.\n`,
    ),
    detail,
  );
  assert.ok(!detail.includes("fails:"), detail);
  assert.equal(
    benchClaims(timed).split("\n")[0],
    "- **5.0–8.1x faster than knip** for dead-code analysis, on 2 pinned public repositories (on next-js it exits " +
      "with an error after its report).",
  );
});

test("a target with capped runs names its run counts, and its errors count out of its own runs", () => {
  const capped = withNextJs();
  assert.ok(
    provenance(capped).startsWith(
      "Medians of 15 runs after 3 warmup runs (5 runs after 1 warmup run on next-js) (hyperfine 1.20.0), ",
    ),
    provenance(capped),
  );
  assert.match(
    benchDetail(capped),
    /\*\*corpus: next-js \(pnpm\)\*\*\n\nInstalled with `pnpm 12\.9\.1`\.\n\n5 runs after 1 warmup run, as corpus\.toml caps them here\.\n\n\| Tool \|/,
  );
  assert.doesNotMatch(
    benchDetail(capped),
    /\*\*corpus: babel \(yarn\)\*\*\n\nInstalled with `yarn 4\.10\.3`\.\n\n\d+ runs/,
  );
  assert.match(benchClaims(capped), /: medians of 15 runs \(5 on next-js\) on a GitHub-hosted runner, measured on /);
  const undeclared = withNextJs();
  const knip = undeclared.targets[2].results[4];
  knip.failure = null;
  undeclared.adr_0001_checks[1].status = "invalid";
  assert.deepEqual(summaryProblems(undeclared), [
    `corpus: next-js (pnpm): knip --reporter json exited with an error in 5 of 5 runs (${JEST_ERROR}), ` +
      "so its time may be that of a partial run",
    "corpus: next-js (pnpm): the ADR 0001 check is invalid: d",
  ]);
  const grouped = withNextJs();
  grouped.targets.push({ ...structuredClone(grouped.targets[2]), target: "corpus: n8n (pnpm)" });
  grouped.targets.push({ ...structuredClone(grouped.targets[2]), target: "corpus: vscode (pnpm)", runs: 3 });
  assert.ok(
    provenance(grouped).startsWith(
      "Medians of 15 runs after 3 warmup runs (5 runs after 1 warmup run on next-js and n8n; 3 runs after 1 warmup " +
        "run on vscode) (hyperfine 1.20.0), ",
    ),
    provenance(grouped),
  );
});

test("a summary measured outside a GitHub Actions job is refused", () => {
  const local = summary();
  local.machine.github = null;
  assert.deepEqual(summaryProblems(local), [
    "the summary records no GitHub Actions runner; only a run of the bench workflow is published",
  ]);
});

test("a summary from a self-hosted or unnamed runner environment is refused", () => {
  const selfHosted = summary();
  selfHosted.machine.github.runner_environment = "self-hosted";
  assert.deepEqual(summaryProblems(selfHosted), [
    "the run's runner environment is self-hosted, not github-hosted; only a GitHub-hosted runner is published",
  ]);
  const unnamed = summary();
  unnamed.machine.github.runner_environment = null;
  assert.deepEqual(summaryProblems(unnamed), [
    "the run's runner environment is unknown, not github-hosted; only a GitHub-hosted runner is published",
  ]);
});

test("a summary that names no workflow run is refused", () => {
  const noRun = summary();
  noRun.machine.github.run_url = null;
  assert.deepEqual(summaryProblems(noRun), ["the summary names no workflow run (machine.github.run_url)"]);
});

test("a summary measured from a checkout with local changes is refused", () => {
  const dirtySource = summary();
  dirtySource.source_rev = "0123456789ab-dirty";
  assert.deepEqual(summaryProblems(dirtySource), [
    "source_rev 0123456789ab-dirty has local changes; only clean checkouts are published",
  ]);
  const dirtyHarness = summary();
  dirtyHarness.harness_rev = "ba9876543210-dirty";
  assert.deepEqual(summaryProblems(dirtyHarness), [
    "harness_rev ba9876543210-dirty has local changes; only clean checkouts are published",
  ]);
});

test("the provenance sentence names the harness commit when it is not the measured one", () => {
  const older = summary();
  older.source_rev = "fedcba987654";
  assert.ok(
    provenance(older).startsWith(
      "Medians of 15 runs after 3 warmup runs (hyperfine 1.20.0), skif 0.3.0 at `fedcba987654` built with " +
        "rustc 1.99.0 (0123abcd 2026-10-01) (measured by the harness at `0123456789ab`) against knip " +
        KNIP +
        ", ",
    ),
    provenance(older),
  );
  assert.ok(!provenance(summary()).includes("measured by the harness"));
});

test("the provenance sentence takes the machine line the harness wrote, not one of its own", () => {
  const other = summary();
  other.machine_text = "unknown OS, aarch64, unknown CPU, 2 logical CPUs";
  const sentence = provenance(other);
  assert.ok(
    sentence.includes(
      " on a GitHub-hosted runner, image ubuntu24 20261102.1.0, unknown OS, aarch64, unknown CPU, 2 logical CPUs, " +
        "load average ",
    ),
    sentence,
  );
});

test("a skipped tool, a missing row or an old schema is a problem", () => {
  const skipped = summary();
  skipped.skipped_tools.push({ tool: "sherif", target: null, reason: "npm is not installed" });
  skipped.targets[1].results = skipped.targets[1].results.filter((r) => r.tool !== "knip --reporter json");
  assert.deepEqual(summaryProblems(skipped), [
    "sherif was not timed on any target: npm is not installed",
    "corpus: babel (yarn) has no knip --reporter json row",
  ]);
  assert.deepEqual(summaryProblems({ ...summary(), schema: 6 }), [
    "summary schema 6 is not 7; measure again with the bench workflow and `mise run bench:import -- <run-id>`",
  ]);
  assert.deepEqual(summaryProblems({ ...summary(), corpus_only: ["babel", "vue-core"] }), [
    "the run timed only some corpus repositories (babel, vue-core); a partial run is not published",
  ]);
  const noCorpus = summary();
  noCorpus.targets = noCorpus.targets.filter((t) => t.kind !== "corpus");
  assert.deepEqual(summaryProblems(noCorpus), ["the summary has no corpus target (was it run with --no-corpus?)"]);
});

test("a summary without both skif modes, or without the peak RSS of one, is refused", () => {
  const untyped = summary();
  untyped.targets[1].results = untyped.targets[1].results.filter((r) => r.tool !== "skif --type-aware");
  assert.deepEqual(summaryProblems(untyped), ["corpus: babel (yarn) has no skif --type-aware row"]);
  const unmeasured = summary();
  unmeasured.targets[1].results[0].peak_rss_bytes = null;
  delete unmeasured.targets[1].results[5].peak_rss_bytes;
  assert.deepEqual(summaryProblems(unmeasured), [
    "corpus: babel (yarn): skif has no peak RSS",
    "corpus: babel (yarn): skif --type-aware has no peak RSS",
  ]);
  const graph = summary();
  graph.targets.push({
    target: "graph: 10000 files in 100 packages",
    kind: "graph",
    packages: 100,
    files: 10000,
    warmup: 3,
    runs: 15,
    results: [row("skif", 4000, rss(2048))],
  });
  assert.deepEqual(summaryProblems(graph), ["graph: 10000 files in 100 packages has no skif --type-aware row"]);
});

test("an invalid type-aware budget check is refused and a failed one is published", () => {
  const invalid = summary();
  invalid.type_aware_budget_checks[0].status = "invalid";
  assert.deepEqual(summaryProblems(invalid), ["corpus: babel (yarn): the type-aware budget check is invalid: b"]);
  const failed = summary();
  failed.type_aware_budget_checks[0] = {
    target: "corpus: babel (yarn)",
    description: "peak RSS with types <= 4096 MiB (measured 4200 MiB)",
    status: "fail",
  };
  assert.deepEqual(summaryProblems(failed), []);
  assert.ok(
    benchDetail(failed).includes(
      "| Type-aware budget (ADR 0029) | Check | Result |\n| --- | --- | --- |\n" +
        "| corpus: babel (yarn) | peak RSS with types <= 4096 MiB (measured 4200 MiB) | FAIL |\n",
    ),
    benchDetail(failed),
  );
});

test("the compact tables show both skif modes with their peak RSS", () => {
  const text = benchSummary(withNextJs());
  assert.ok(
    text.includes(
      "| Target | types on | types off (default) | on / off | peak RSS, types on | peak RSS, types off (default) |\n" +
        "| --- | ---: | ---: | ---: | ---: | ---: |\n" +
        "| babel | 1.3 s | 884 ms | 1.5x | 1,200 MiB | 400 MiB |\n" +
        "| next-js | 18.0 s | 6.0 s | 3.0x | 3,800 MiB | 310 MiB |\n",
    ),
    text,
  );
  assert.match(benchDetail(summary()), /\| skif --type-aware \| 1326 \| 1326 \| - \|/);
  assert.equal(mebibytes(3_447_717_888), "3,288 MiB");
});

test("the compact tables use medians and the knip to skif ratio", () => {
  const text = benchSummary(summary());
  assert.match(text, /\| babel \| 884 ms \| 7\.2 s \| 8x \|/);
  assert.match(text, /\| babel \| 9\.4 ms \| 37 ms \| 280 ms \|/);
  assert.match(text, /\| Generated, 500 packages \| 27 ms \| 59 ms \| 156 ms \|/);
});

test("the detail tables print no ratio for the skif rows and name what installed a corpus repository", () => {
  const text = benchDetail(summary());
  assert.match(text, /^\*\*generated: 500 packages \(pnpm\)\*\*\n/);
  assert.match(text, /\*\*corpus: babel \(yarn\)\*\*\n\nInstalled with `yarn 4\.10\.3`\.\n\n\| Tool \|/);
  assert.match(text, /\*\*generated: 500 packages \(pnpm\)\*\*\n\n\| Tool \|/);
  assert.match(text, /\| skif --no-js \| 9\.4 \| 9\.4 \| - \|/);
  assert.match(text, /\| knip --reporter json \| 7200 \| 7200 \| 8\.1x \|/);
  assert.match(text, /\| corpus: babel \(yarn\) \| d \| pass \|/);
});

test("the claims give each tool's ratio range over the targets it was timed on, with the runs they come from", () => {
  assert.equal(
    benchClaims(summary()),
    [
      "- **8.1x faster than knip** for dead-code analysis, on 1 pinned public repository.",
      "- **2.1–3.9x faster than sherif** for manifest and workspace checks (`skif --no-js`), on 1 pinned public " +
        "repository and 1 generated workspace of 500 packages.",
      "- **5.7–29.7x faster than syncpack** for version-policy checks (`skif --no-js`), on 1 pinned public " +
        "repository and 1 generated workspace of 500 packages.",
      "",
      "skif 0.3.0 against knip " +
        KNIP +
        ", sherif " +
        SHERIF +
        ", syncpack " +
        SYNCPACK +
        ": medians of 15 runs on a GitHub-hosted " +
        "runner, measured on 2026-11-10.",
    ].join("\n"),
  );
});

test("the claims leave out a knip that fails and give the median ratio from three targets on", () => {
  const matched = withT3({
    errorRuns: 15,
    failure: { reason: METRO_REASON, error: METRO_ERROR, timed: false },
    check: { description: `${GOAL} (not measured: knip fails here: ${METRO_REASON})`, status: "not-measured" },
  });
  const lines = benchClaims(matched).split("\n");
  assert.equal(
    lines[0],
    "- **8.1x faster than knip** for dead-code analysis, on 1 pinned public repository where knip completes; it " +
      "fails on create-t3-turbo.",
  );
  assert.equal(
    lines[1],
    "- **2.1–7.5x faster than sherif** (median 3.9x) for manifest and workspace checks (`skif --no-js`), on 2 " +
      "pinned public repositories and 1 generated workspace of 500 packages.",
  );
  assert.match(lines[2], /^- \*\*5\.7–29\.7x faster than syncpack\*\* \(median 9\.0x\) for version-policy checks/);
});

test("the claims leave knip out when it fails on every repository", () => {
  const matched = withT3({
    errorRuns: 15,
    failure: { reason: METRO_REASON, error: METRO_ERROR, timed: false },
    check: { description: `${GOAL} (not measured: knip fails here: ${METRO_REASON})`, status: "not-measured" },
  });
  matched.targets = matched.targets.filter((target) => target.target !== "corpus: babel (yarn)");
  assert.doesNotMatch(benchClaims(matched), /knip\*\*/);
  assert.match(benchClaims(matched), /^- \*\*2\.1–7\.5x faster than sherif\*\* for manifest/);
});

test("the README claims are the comparison guide's, each linking to the speed tables of the benchmarks page", () => {
  const claims = benchClaims(summary()).split("\n");
  const readme = readmeClaims(summary()).split("\n");
  const page = "https://goldziher.github.io/skif/guides/benchmarks/#speed";
  assert.deepEqual(readme.slice(0, 3), [
    `- [**8.1x faster than knip**](${page}) for dead-code analysis, on 1 pinned public repository.`,
    `- [**2.1–3.9x faster than sherif**](${page}) for manifest and workspace checks (\`skif --no-js\`), on 1 pinned ` +
      "public repository and 1 generated workspace of 500 packages.",
    `- [**5.7–29.7x faster than syncpack**](${page}) for version-policy checks (\`skif --no-js\`), on 1 pinned ` +
      "public repository and 1 generated workspace of 500 packages.",
  ]);
  assert.deepEqual(readme.slice(3), claims.slice(3));
});

test("a speed claim rounds every ratio down and says where skif was slower", () => {
  assert.equal(speedClaim("t", [1.99, 2.09]), "**1.9–2.0x faster than t**");
  assert.equal(speedClaim("t", [7.0, 7.09]), "**7.0x faster than t**");
  assert.equal(speedClaim("t", [3, 1.25, 9.99]), "**1.2–9.9x faster than t** (median 3.0x)");
  assert.equal(speedClaim("t", [3, 1.5, 9, 4]), "**1.5–9.0x faster than t** (median 3.5x)");
  assert.equal(
    speedClaim("t", [0.8, 2, 5.55]),
    "**up to 5.5x faster than t** (median 2.0x), and slower on 1 of 3 targets",
  );
  assert.equal(speedClaim("t", [0.5, 0.97]), "**slower than t** on every target (0.9x its speed at best)");
});

test("durations switch unit at 10 ms and 1 s", () => {
  assert.equal(duration(4.62), "4.6 ms");
  assert.equal(duration(375.4), "375 ms");
  assert.equal(duration(1999), "2.0 s");
});

test("text for MDX escapes what MDX would parse as JSX or an expression, but not inside code spans", () => {
  assert.equal(mdxText("rules < 50 ms in {x}, `a < {b}` kept"), "rules &lt; 50 ms in &#123;x&#125;, `a < {b}` kept");
  assert.equal(mdxText("no markup"), "no markup");
});

test("blocks are replaced between their markers, which differ in MDX", () => {
  assert.deepEqual(markers("x", true), ["{/* x:start */}", "{/* x:end */}"]);
  const md = "a\n<!-- x:start -->\nold\n<!-- x:end -->\nb\n";
  assert.equal(replaceBlock(md, "x", "new\n"), "a\n<!-- x:start -->\nnew\n<!-- x:end -->\nb\n");
  const mdx = "{/* x:start */}\nold\n{/* x:end */}\n";
  assert.equal(replaceBlock(mdx, "x", "new", { mdx: true }), "{/* x:start */}\nnew\n{/* x:end */}\n");
  assert.throws(() => replaceBlock("no markers", "x", "new", { file: "f.md" }), /f\.md: the <!-- x:start -->/);
  const mentioned = "see <!-- x:end -->\n<!-- x:start -->\nold\n<!-- x:end -->\n";
  assert.equal(replaceBlock(mentioned, "x", "new"), "see <!-- x:end -->\n<!-- x:start -->\nnew\n<!-- x:end -->\n");
  assert.throws(() => replaceBlock(md + md, "x", "new"), /appears more than once/);
});

function baseline() {
  const counts = (skif_only, tool_only, both) => ({ skif_only, tool_only, both });
  return {
    schema: 1,
    tool: `knip@${KNIP}`,
    installed: false,
    category_set: { compared: ["exports", "files"], known_deviations: ["test-file"] },
    entries: {
      jest: {
        status: "compared",
        categories: { exports: counts(1, 9, 90), files: counts(0, 0, 10) },
        installed: true,
        recursive: false,
        skif_rev: "aaa",
      },
      mastra: {
        status: "compared",
        categories: { exports: counts(9, 1, 10) },
        installed: true,
        recursive: true,
        skif_rev: "aaa",
      },
      n8n: {
        status: "compared",
        invalid: "unbuilt configs",
        categories: { exports: counts(1000, 0, 0) },
        installed: true,
        recursive: false,
        skif_rev: "aaa",
      },
      rushstack: { status: "skipped", reason: "knip failed" },
    },
  };
}

test("the agreement table sums the valid comparisons and names what it leaves out", () => {
  assert.deepEqual(baselineProblems(baseline()), []);
  const text = parityAgreement(baseline());
  assert.match(text, /share on 2 pinned repositories \(jest, mastra; 2 of them installed\)/);
  assert.match(text, /recorded at skif `aaa`/);
  assert.match(text, /not a valid comparison: n8n \(unbuilt configs\)/);
  assert.match(text, /Not compared: rushstack \(knip failed\)/);
  // exports: both 100, skif only 10, knip only 10.
  assert.match(text, /\| Unused exports \| 100 \| 91% \| 91% \|/);
  assert.match(text, /\| Unused files \| 10 \| 100% \| 100% \|/);
  assert.ok(text.indexOf("Unused exports") < text.indexOf("Unused files"), "sorted by shared findings");
});

test("a baseline from an older harness is a problem", () => {
  const old = baseline();
  delete old.category_set;
  assert.match(baselineProblems(old)[0], /records no category set/);
  const stale = baseline();
  delete stale.entries.jest.skif_rev;
  assert.deepEqual(baselineProblems(stale), [
    "jest: the baseline entry predates the current harness; re-record it with --write-baseline",
  ]);
});

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
/** The files refresh-numbers.mjs rewrites, relative to the repository root. */
const TARGETS = [
  "website/src/content/docs/guides/comparison.mdx",
  "website/src/content/docs/guides/benchmarks.mdx",
  "README.md",
];

/** A temporary tree holding a copy of the script, the files it rewrites and the parity baseline; returns its root. */
function scriptTree(t) {
  const root = mkdtempSync(join(tmpdir(), "skif-refresh-numbers-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const file of [
    ...TARGETS,
    "tools/bench/refresh-numbers.mjs",
    "tools/bench/baselines/parity.json",
    "tools/bench/corpus.toml",
  ]) {
    mkdirSync(dirname(join(root, file)), { recursive: true });
    cpSync(join(repo, file), join(root, file));
  }
  // The script refuses a summary whose tool versions differ from the pins, so pin the synthetic versions.
  const corpus = join(root, "tools/bench/corpus.toml");
  const synthetic = { knip: KNIP, syncpack: SYNCPACK, sherif: SHERIF };
  writeFileSync(
    corpus,
    readFileSync(corpus, "utf8").replace(
      /^(knip|syncpack|sherif) = "[^"]*"$/gmu,
      (_line, tool) => `${tool} = "${synthetic[tool]}"`,
    ),
  );
  return root;
}

const runScript = (root, ...args) =>
  spawnSync(process.execPath, [join(root, "tools", "bench", "refresh-numbers.mjs"), ...args], { encoding: "utf8" });
const contents = (root) => TARGETS.map((file) => readFileSync(join(root, file)));

test("a refused summary leaves every target file byte for byte as it was", (t) => {
  const root = scriptTree(t);
  const refused = summary();
  refused.machine.github = null;
  const file = join(root, "refused.json");
  writeFileSync(file, JSON.stringify(refused));
  const before = contents(root);
  const run = runScript(root, "--summary", file);
  assert.equal(run.status, 1, run.stderr);
  assert.match(run.stderr, /refused\.json: the summary records no GitHub Actions runner/);
  assert.match(run.stderr, /refresh-numbers: nothing was written/);
  assert.deepEqual(contents(root), before);
});

test("without flags and without an imported summary only the agreement table is refreshed", (t) => {
  const root = scriptTree(t);
  assert.equal(existsSync(join(root, "tools", "bench", "results", "summary.json")), false);
  const page = join(root, "website", "src", "content", "docs", "guides", "benchmarks.mdx");
  const agreement = /(\{\/\* parity:agreement:start \*\/\}\n)[^]*?(\n\{\/\* parity:agreement:end)/;
  writeFileSync(page, readFileSync(page, "utf8").replace(agreement, "$1stale$2"));
  const [comparison] = contents(root);
  const run = runScript(root);
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /results\/summary\.json, so the speed blocks are left as they are/);
  const [comparisonAfter, pageAfter] = contents(root);
  assert.deepEqual(comparisonAfter, comparison);
  assert.doesNotMatch(pageAfter.toString("utf8"), /\nstale\n/);
  assert.match(pageAfter.toString("utf8"), /\| Unused files \| /);
});

test("--bench without an imported summary fails", (t) => {
  const root = scriptTree(t);
  const before = contents(root);
  const run = runScript(root, "--bench");
  assert.notEqual(run.status, 0);
  assert.match(run.stderr, /ENOENT/);
  assert.deepEqual(contents(root), before);
});

test("a summary with a matched knip failure is written into every speed block", (t) => {
  const root = scriptTree(t);
  const matched = withT3({
    errorRuns: 15,
    failure: { reason: METRO_REASON, error: METRO_ERROR, timed: false },
    check: { description: `${GOAL} (not measured: knip fails here: ${METRO_REASON})`, status: "not-measured" },
  });
  const file = join(root, "matched.json");
  writeFileSync(file, JSON.stringify(matched));
  const run = runScript(root, "--bench", "--summary", file);
  assert.equal(run.status, 0, run.stderr);
  const [comparison, page, readme] = contents(root).map((bytes) => bytes.toString("utf8"));
  assert.ok(page.includes(`| knip --reporter json | fails: ${METRO_REASON} | - | - |\n`), page);
  assert.ok(page.includes(`| create-t3-turbo | 33 ms | fails: ${METRO_REASON} | - |\n`), page);
  assert.ok(page.includes("| create-t3-turbo | 36 ms | 33 ms | 1.1x | 300 MiB | 200 MiB |\n"), page);
  const claim = "\n- **8.1x faster than knip** for dead-code analysis";
  assert.ok(comparison.includes(`{/* bench:claims:start */}${claim}`), comparison);
  assert.ok(comparison.includes("it fails on create-t3-turbo.\n"), comparison);
  const linked =
    "\n- [**8.1x faster than knip**](https://goldziher.github.io/skif/guides/benchmarks/#speed) for dead-code";
  assert.ok(readme.includes(`<!-- bench:claims:start -->${linked}`), readme);
});
