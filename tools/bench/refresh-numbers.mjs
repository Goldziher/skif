// Rewrites the generated number blocks of the docs from the committed results:
//   --bench   tools/bench/results/summary.json (a bench workflow run's, which `mise run bench:import` downloads) into
//             tools/bench/README.md, README.md and the comparison guide;
//   --parity  tools/bench/baselines/parity.json (written by `skif-bench parity --write-baseline`) into README.md and
//             the comparison guide.
// Without either flag both are refreshed. It fails, and writes nothing, when the input does not hold every number
// the blocks show: a tool that was not timed, a corpus repository without a knip row, a stale baseline.
// Usage: node tools/bench/refresh-numbers.mjs [--bench] [--parity] [--summary FILE] [--baseline FILE]

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");

export const SUMMARY_SCHEMA = 3;
const SKIF = "skif";
const SKIF_MANIFEST = "skif --no-js";
const SHERIF = "sherif";
const SYNCPACK = "syncpack lint";
const KNIP = "knip --reporter json";

/** The marker pair of a block: HTML comments in Markdown, expression comments in MDX. */
export function markers(name, mdx) {
  return mdx ? [`{/* ${name}:start */}`, `{/* ${name}:end */}`] : [`<!-- ${name}:start -->`, `<!-- ${name}:end -->`];
}

/** Replaces the text between a block's markers; throws when the markers are missing. */
export function replaceBlock(text, name, body, { mdx = false, file = "the file" } = {}) {
  const [start, end] = markers(name, mdx);
  const from = text.indexOf(start);
  const to = from < 0 ? -1 : text.indexOf(end, from + start.length);
  if (from < 0 || to < 0) {
    throw new Error(`${file}: the ${start} and ${end} markers are missing`);
  }
  if (text.indexOf(start, to) >= 0) {
    throw new Error(`${file}: the ${start} marker appears more than once`);
  }
  return `${text.slice(0, from + start.length)}\n${body.trimEnd()}\n${text.slice(to)}`;
}

const ms = (value) => (value >= 100 ? value.toFixed(0) : value.toFixed(1));

/** A median in the unit a reader takes in at a glance: `4.6 ms`, `375 ms`, `2.0 s`. */
export function duration(milliseconds) {
  if (milliseconds >= 1000) return `${(milliseconds / 1000).toFixed(1)} s`;
  if (milliseconds >= 10) return `${milliseconds.toFixed(0)} ms`;
  return `${milliseconds.toFixed(1)} ms`;
}

const oneLine = (text) => text.split(/\s+/).filter(Boolean).join(" ");
const gib = (bytes) => (bytes / 2 ** 30).toFixed(1);
const RUNNER_ENVIRONMENTS = { "github-hosted": "GitHub-hosted", "self-hosted": "self-hosted" };

/**
 * The machine in one line, as the `Machine:` line of `summary.md` writes it: operating system, kernel, architecture,
 * CPU model, CPUs, memory, cgroup limit and hypervisor; an unknown OS or CPU is named as such, any other unknown fact
 * is left out.
 */
export function machineText(machine) {
  const logical = machine.logical_cpus ?? machine.available_cpus;
  let cpus = `${logical} logical CPUs`;
  if (machine.physical_cores != null) cpus += ` (${machine.physical_cores} physical cores)`;
  if (machine.logical_cpus != null && machine.available_cpus < machine.logical_cpus) {
    cpus += `, ${machine.available_cpus} available to the run`;
  }
  return [
    machine.os == null ? "unknown OS" : oneLine(machine.os),
    machine.kernel == null ? null : oneLine(machine.kernel),
    machine.arch,
    machine.cpu == null ? "unknown CPU" : oneLine(machine.cpu),
    cpus,
    machine.memory_bytes == null ? null : `${gib(machine.memory_bytes)} GiB memory`,
    machine.cgroup_cpu_limit == null ? null : `cgroup limit of ${machine.cgroup_cpu_limit} CPUs`,
    machine.virtualization == null ? null : `hypervisor ${oneLine(machine.virtualization)}`,
  ]
    .filter((part) => part != null)
    .join(", ");
}

/** The runner of a workflow run with its image, before the machine; empty outside GitHub Actions. */
function runnerText(github) {
  if (github == null) return "";
  const environment = RUNNER_ENVIRONMENTS[github.runner_environment] ?? "GitHub Actions";
  return `a ${environment} runner, image ${github.image_os ?? "unknown"} ${github.image_version ?? "unknown"}, `;
}

const load = (value) => (value ? value.map((n) => n.toFixed(2)).join(" / ") : "unknown");
const date = (seconds) => new Date(seconds * 1000).toISOString().slice(0, 10);

/** The rows of the tools a target must have for the published tables. */
function requiredTools(target) {
  if (target.kind === "corpus") return [SKIF, SKIF_MANIFEST, SHERIF, SYNCPACK, KNIP];
  if (target.kind === "generated") return [SKIF_MANIFEST, SHERIF, SYNCPACK];
  return [SKIF];
}

/** Why the summary was not measured by a run of the bench workflow on a GitHub-hosted runner from clean checkouts. */
function provenanceProblems(summary) {
  const problems = [];
  const { github } = summary.machine;
  if (github == null) {
    problems.push("the summary records no GitHub Actions runner; only a run of the bench workflow is published");
  } else {
    if (github.runner_environment !== "github-hosted") {
      problems.push(
        `the run's runner environment is ${github.runner_environment ?? "unknown"}, not github-hosted; ` +
          "only a GitHub-hosted runner is published",
      );
    }
    if (github.run_url == null) problems.push("the summary names no workflow run (machine.github.run_url)");
  }
  for (const field of ["source_rev", "harness_rev"]) {
    if (summary[field].endsWith("-dirty")) {
      problems.push(`${field} ${summary[field]} has local changes; only clean checkouts are published`);
    }
  }
  return problems;
}

/** Every reason the summary cannot be published; empty when it can. */
export function summaryProblems(summary) {
  if (summary.schema !== SUMMARY_SCHEMA) {
    return [
      `summary schema ${summary.schema} is not ${SUMMARY_SCHEMA}; measure again with the bench workflow and ` +
        "`mise run bench:import -- <run-id>`",
    ];
  }
  const problems = provenanceProblems(summary);
  problems.push(
    ...summary.skipped_tools.map(
      (skip) => `${skip.tool} was not timed on ${skip.target ?? "any target"}: ${skip.reason}`,
    ),
  );
  if (summary.corpus_only.length > 0) {
    problems.push(
      `the run timed only some corpus repositories (${summary.corpus_only.join(", ")}); a partial run is not published`,
    );
  }
  for (const target of summary.targets) {
    const timed = new Set(target.results.map((row) => row.tool));
    for (const tool of requiredTools(target).filter((name) => !timed.has(name))) {
      problems.push(`${target.target} has no ${tool} row`);
    }
  }
  if (!summary.targets.some((target) => target.kind === "corpus")) {
    problems.push("the summary has no corpus target (was it run with --no-corpus?)");
  }
  return problems;
}

/** One sentence that says what was measured, built with what, on what machine and runner, when, and in which run. */
export function provenance(summary) {
  const tools = Object.entries(summary.tool_versions)
    .map(([tool, version]) => (version.startsWith(`${tool} `) ? version : `${tool} ${version}`))
    .join(", ");
  const { github } = summary.machine;
  const rustc = summary.toolchain.rustc == null ? "" : ` built with ${summary.toolchain.rustc}`;
  const attempt = github?.run_attempt == null ? "" : `, attempt ${github.run_attempt}`;
  const run = github?.run_url == null ? "" : ` ([workflow run${attempt}](${github.run_url}))`;
  return (
    `Medians of ${summary.runs} runs after ${summary.warmup} warmup runs (${summary.hyperfine}), ` +
    `${summary.skif} at \`${summary.source_rev}\`${rustc} against ${tools}, measured on ` +
    `${date(summary.measured_at)} on ${runnerText(github)}${machineText(summary.machine)}, load average ` +
    `${load(summary.load_before)} before and ${load(summary.load_after)} after the run${run}.`
  );
}

/** The full tables of tools/bench/README.md. */
export function benchDetail(summary) {
  const lines = [provenance(summary), ""];
  for (const target of summary.targets) {
    lines.push(`**${target.target}**`, "");
    if (target.installed_with != null) lines.push(`Installed with \`${target.installed_with}\`.`, "");
    lines.push(
      "| Tool | Mean (ms) | Median (ms) | vs skif |",
      "| --- | ---: | ---: | ---: |",
    );
    for (const row of target.results) {
      const skifRow = row.tool === SKIF || row.tool === SKIF_MANIFEST;
      const relative = skifRow || row.relative_to_skif == null ? "-" : `${row.relative_to_skif.toFixed(1)}x`;
      lines.push(`| ${row.tool} | ${ms(row.mean_ms)} | ${ms(row.median_ms)} | ${relative} |`);
    }
    lines.push("");
    for (const row of target.results.filter((r) => (r.error_runs ?? 0) > 0)) {
      lines.push(
        `${row.tool} exited with an error in ${row.error_runs} of ${summary.runs} runs, so its time may be that of a partial run.`,
        "",
      );
    }
  }
  if (summary.adr_0001_checks.length > 0) {
    lines.push("| ADR 0001 target | Check | Result |", "| --- | --- | --- |");
    for (const check of summary.adr_0001_checks) {
      lines.push(`| ${check.target} | ${check.description} | ${check.passed ? "pass" : "FAIL"} |`);
    }
    lines.push("");
  }
  return lines.join("\n");
}

const median = (target, tool) => target.results.find((row) => row.tool === tool)?.median_ms;
const repoName = (target) => target.target.replace(/^corpus: (\S+).*$/, "$1");
const generatedName = (target) => `Generated, ${target.packages} packages`;

/** The compact tables of README.md and the comparison guide: medians, and knip's median over skif's. */
export function benchSummary(summary) {
  const corpus = summary.targets.filter((target) => target.kind === "corpus");
  const generated = summary.targets.filter((target) => target.kind === "generated");
  const lines = [provenance(summary), "", "**Dead-code analysis against knip** (full skif run)", ""];
  lines.push("| Repository | skif | knip | knip / skif |", "| --- | ---: | ---: | ---: |");
  for (const target of corpus) {
    const skif = median(target, SKIF);
    const knip = median(target, KNIP);
    lines.push(`| ${repoName(target)} | ${duration(skif)} | ${duration(knip)} | ${(knip / skif).toFixed(0)}x |`);
  }
  const errors = corpus.filter((target) =>
    target.results.some((row) => row.tool === KNIP && (row.error_runs ?? 0) > 0),
  );
  if (errors.length > 0) {
    lines.push(
      "",
      `knip exited with an error status on ${errors.map(repoName).join(", ")}: it prints its report, then exits with 2 because some of the repository's tool configurations do not load.`,
    );
  }
  lines.push("", "**Manifest and policy checks** (`skif --no-js`)", "");
  lines.push("| Target | skif --no-js | sherif | syncpack |", "| --- | ---: | ---: | ---: |");
  for (const target of [...corpus, ...generated]) {
    const name = target.kind === "corpus" ? repoName(target) : generatedName(target);
    lines.push(
      `| ${name} | ${duration(median(target, SKIF_MANIFEST))} | ${duration(median(target, SHERIF))} | ${duration(median(target, SYNCPACK))} |`,
    );
  }
  return lines.join("\n");
}

const CATEGORY_NAMES = {
  exports: "Unused exports",
  files: "Unused files",
  types: "Unused types",
  nsExports: "Unused namespace exports",
  nsTypes: "Unused namespace types",
  enumMembers: "Unused enum members",
  namespaceMembers: "Unused namespace members",
  duplicates: "Duplicate exports",
  unlisted: "Unlisted dependencies",
  unresolved: "Unresolved imports",
  binaries: "Unlisted binaries",
  dependencies: "Unused dependencies",
  devDependencies: "Unused dev dependencies",
  optionalPeerDependencies: "Unused optional peer dependencies",
};

const percent = (part, whole) => (whole === 0 ? "-" : `${Math.round((part * 100) / whole)}%`);
const count = (value) => value.toLocaleString("en-US");

/** Every reason the parity baseline cannot be published; empty when it can. */
export function baselineProblems(baseline) {
  if (!baseline.category_set) {
    return [
      "the parity baseline records no category set; re-record it with `mise run parity -- --install --write-baseline`",
    ];
  }
  return Object.entries(baseline.entries)
    .filter(([, entry]) => entry.status === "compared" && entry.skif_rev == null)
    .map(([name]) => `${name}: the baseline entry predates the current harness; re-record it with --write-baseline`);
}

/** The agreement table of README.md and the comparison guide, from the parity baseline. */
export function parityAgreement(baseline) {
  const entries = Object.entries(baseline.entries);
  const counted = entries.filter(([, entry]) => entry.status === "compared" && !entry.invalid);
  const invalid = entries.filter(([, entry]) => entry.status === "compared" && entry.invalid);
  const skipped = entries.filter(([, entry]) => entry.status !== "compared");
  const totals = {};
  for (const [, entry] of counted) {
    for (const [category, found] of Object.entries(entry.categories)) {
      const sum = (totals[category] ??= { skif_only: 0, tool_only: 0, both: 0 });
      sum.skif_only += found.skif_only;
      sum.tool_only += found.tool_only;
      sum.both += found.both;
    }
  }
  const revs = [...new Set(counted.map(([, entry]) => entry.skif_rev))].sort();
  const installed = counted.filter(([, entry]) => entry.installed).length;
  const lines = [
    `Findings ${baseline.tool} and skif share on ${counted.length} pinned repositories ` +
      `(${counted.map(([name]) => name).join(", ")}; ${installed} of them installed), from ` +
      `\`tools/bench/baselines/parity.json\`, recorded at skif ${revs.map((rev) => `\`${rev}\``).join(", ")}.`,
  ];
  if (invalid.length > 0) {
    lines.push(
      "",
      `Not counted, because knip's run there is not a valid comparison: ${invalid.map(([name, entry]) => `${name} (${entry.invalid})`).join("; ")}.`,
    );
  }
  if (skipped.length > 0) {
    lines.push(
      "",
      `Not compared: ${skipped.map(([name, entry]) => `${name} (${entry.reason ?? "skipped"})`).join("; ")}.`,
    );
  }
  lines.push("", "| Finding | Shared | Of skif's findings | Of knip's findings |", "| --- | ---: | ---: | ---: |");
  const rows = Object.entries(totals)
    .filter(([category]) => category in CATEGORY_NAMES)
    .sort(([, a], [, b]) => b.both - a.both);
  for (const [category, sum] of rows) {
    lines.push(
      `| ${CATEGORY_NAMES[category]} | ${count(sum.both)} | ${percent(sum.both, sum.both + sum.skif_only)} | ${percent(sum.both, sum.both + sum.tool_only)} |`,
    );
  }
  return lines.join("\n");
}

function parseArgs(argv) {
  const options = {
    bench: false,
    parity: false,
    summary: join(here, "results", "summary.json"),
    baseline: join(here, "baselines", "parity.json"),
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--bench") options.bench = true;
    else if (arg === "--parity") options.parity = true;
    else if (arg === "--summary") options.summary = argv[++index];
    else if (arg === "--baseline") options.baseline = argv[++index];
    else throw new Error(`unknown argument ${arg}`);
  }
  if (!options.bench && !options.parity) {
    options.bench = true;
    options.parity = true;
  }
  return options;
}

const readJson = (file) => JSON.parse(readFileSync(file, "utf8"));

function main() {
  const options = parseArgs(process.argv.slice(2));
  const benchReadme = join(here, "README.md");
  const readme = join(repo, "README.md");
  const guide = join(repo, "website", "src", "content", "docs", "guides", "comparison.mdx");
  const files = new Map([benchReadme, readme, guide].map((file) => [file, readFileSync(file, "utf8")]));
  const update = (file, name, body, mdx = false) => {
    files.set(file, replaceBlock(files.get(file), name, body, { mdx, file }));
  };
  const problems = [];
  if (options.bench) {
    const summary = readJson(options.summary);
    const found = summaryProblems(summary);
    problems.push(...found.map((problem) => `${options.summary}: ${problem}`));
    if (found.length === 0) {
      update(benchReadme, "bench:numbers", benchDetail(summary));
      update(readme, "bench:summary", benchSummary(summary));
      update(guide, "bench:summary", benchSummary(summary), true);
    }
  }
  if (options.parity) {
    const baseline = readJson(options.baseline);
    const found = baselineProblems(baseline);
    problems.push(...found.map((problem) => `${options.baseline}: ${problem}`));
    if (found.length === 0) {
      update(readme, "parity:agreement", parityAgreement(baseline));
      update(guide, "parity:agreement", parityAgreement(baseline), true);
    }
  }
  if (problems.length > 0) {
    for (const problem of problems) console.error(`refresh-numbers: ${problem}`);
    console.error("refresh-numbers: nothing was written");
    process.exit(1);
  }
  for (const [file, text] of files) writeFileSync(file, text);
  console.log(`refreshed ${[...files.keys()].join(", ")}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
