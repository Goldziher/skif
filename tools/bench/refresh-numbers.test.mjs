// Tests of refresh-numbers.mjs. Run with `node --test tools/bench/refresh-numbers.test.mjs`.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  baselineProblems,
  benchDetail,
  benchSummary,
  duration,
  markers,
  parityAgreement,
  provenance,
  replaceBlock,
  summaryProblems,
} from "./refresh-numbers.mjs";

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
    schema: 4,
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
    tool_versions: { knip: "6.44.1", sherif: "sherif 1.13.0", syncpack: "15.3.3" },
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
        results: [row("skif --no-js", 27), row("sherif", 59), row("syncpack lint", 156)],
      },
      {
        target: "corpus: babel (yarn)",
        kind: "corpus",
        packages: null,
        files: null,
        installed_with: "yarn 4.10.3",
        results: [
          row("skif", 884),
          row("skif --no-js", 9.4),
          row("sherif", 37),
          row("syncpack lint", 280),
          row("knip --reporter json", 7200, { relative_to_skif: 8.1 }),
        ],
      },
    ],
    adr_0001_checks: [{ target: "corpus: babel (yarn)", description: "d", status: "pass" }],
  };
}

test("a complete summary has no problems and names what was measured, on which runner and in which run", () => {
  assert.deepEqual(summaryProblems(summary()), []);
  assert.equal(
    provenance(summary()),
    "Medians of 15 runs after 3 warmup runs (hyperfine 1.20.0), skif 0.3.0 at `0123456789ab` built with " +
      "rustc 1.99.0 (0123abcd 2026-10-01) against knip 6.44.1, sherif 1.13.0, syncpack 15.3.3, measured on " +
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
  ]);
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
  assert.match(
    summaryProblems({ ...summary(), schema: 3 })[0],
    /schema 3 is not 4; measure again with the bench workflow/,
  );
  assert.deepEqual(summaryProblems({ ...summary(), corpus_only: ["babel", "vue-core"] }), [
    "the run timed only some corpus repositories (babel, vue-core); a partial run is not published",
  ]);
  const noCorpus = summary();
  noCorpus.targets = noCorpus.targets.filter((t) => t.kind !== "corpus");
  assert.deepEqual(summaryProblems(noCorpus), ["the summary has no corpus target (was it run with --no-corpus?)"]);
});

test("the compact tables use medians and the knip to skif ratio", () => {
  const text = benchSummary(summary());
  assert.match(text, /\| babel \| 884 ms \| 7\.2 s \| 8x \|/);
  assert.match(text, /\| babel \| 9\.4 ms \| 37 ms \| 280 ms \|/);
  assert.match(text, /\| Generated, 500 packages \| 27 ms \| 59 ms \| 156 ms \|/);
});

test("the detail tables print no ratio for the skif rows and name what installed a corpus repository", () => {
  const text = benchDetail(summary());
  assert.match(text, /\*\*corpus: babel \(yarn\)\*\*\n\nInstalled with `yarn 4\.10\.3`\.\n\n\| Tool \|/);
  assert.match(text, /\*\*generated: 500 packages \(pnpm\)\*\*\n\n\| Tool \|/);
  assert.match(text, /\| skif --no-js \| 9\.4 \| 9\.4 \| - \|/);
  assert.match(text, /\| knip --reporter json \| 7200 \| 7200 \| 8\.1x \|/);
  assert.match(text, /\| corpus: babel \(yarn\) \| d \| pass \|/);
});

test("durations switch unit at 10 ms and 1 s", () => {
  assert.equal(duration(4.62), "4.6 ms");
  assert.equal(duration(375.4), "375 ms");
  assert.equal(duration(1999), "2.0 s");
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
    tool: "knip@6.44.1",
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
