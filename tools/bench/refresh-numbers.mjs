// Rewrites the generated number blocks of the docs from the committed results:
//   --bench   tools/bench/results/summary.json (a bench workflow run's, which `mise run bench:import` downloads) into
//             the speed tables of the benchmarks page and the headline claims of the comparison guide;
//   --parity  tools/bench/baselines/parity.json (written by `skif-bench parity --write-baseline`) into the agreement
//             table of the benchmarks page.
// Without either flag both are refreshed, and the speed blocks are left as they are while no run has been imported
// (tools/bench/results/summary.json does not exist and no --summary is given). It fails, and writes nothing, when the
// input does not hold every number the blocks show: a tool that was not timed, a corpus repository without a knip row,
// a tool error that corpus.toml does not declare, an invalid ADR 0001 check, a stale baseline. A knip failure that
// corpus.toml declares and the run matched is published as `fails: <reason>`, without a ratio, and left out of the
// claims.
// Usage: node tools/bench/refresh-numbers.mjs [--bench] [--parity] [--summary FILE] [--baseline FILE]

import { existsSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");

export const SUMMARY_SCHEMA = 5;
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

/** `text` with the characters MDX reads as JSX or expressions (`<`, `{`, `}`) escaped, outside code spans. */
export function mdxText(text) {
  return text
    .split(/(`[^`\n]*`)/)
    .map((part, index) =>
      index % 2 === 1 ? part : part.replaceAll("<", "&lt;").replaceAll("{", "&#123;").replaceAll("}", "&#125;"),
    )
    .join("");
}

const ms = (value) => (value >= 100 ? value.toFixed(0) : value.toFixed(1));

/** A median in the unit a reader takes in at a glance: `4.6 ms`, `375 ms`, `2.0 s`. */
export function duration(milliseconds) {
  if (milliseconds >= 1000) return `${(milliseconds / 1000).toFixed(1)} s`;
  if (milliseconds >= 10) return `${milliseconds.toFixed(0)} ms`;
  return `${milliseconds.toFixed(1)} ms`;
}

const RUNNER_ENVIRONMENTS = { "github-hosted": "GitHub-hosted", "self-hosted": "self-hosted" };
const CHECK_LABELS = { pass: "pass", fail: "FAIL", invalid: "invalid", "not-measured": "not measured" };

/** `text` as a Markdown table cell: on one line, with `|` escaped. */
const cell = (text) => text.split(/\s+/).filter(Boolean).join(" ").replaceAll("|", "\\|");

/** The cell of a row whose tool failed as corpus.toml declares, in place of its times. */
const fails = (row) => `fails: ${cell(row.failure.reason)}`;

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
    for (const row of target.results.filter((r) => r.error_runs > 0 && r.failure == null)) {
      const cause = row.error_output == null ? "" : ` (${row.error_output})`;
      problems.push(
        `${target.target}: ${row.tool} exited with an error in ${row.error_runs} of ${summary.runs} runs${cause}, ` +
          "so its time may be that of a partial run",
      );
    }
  }
  for (const check of summary.adr_0001_checks.filter((c) => c.status === "invalid")) {
    problems.push(`${check.target}: the ADR 0001 check is invalid: ${check.description}`);
  }
  if (!summary.targets.some((target) => target.kind === "corpus")) {
    problems.push("the summary has no corpus target (was it run with --no-corpus?)");
  }
  return problems;
}

/** The comparison tools with their versions: `knip 6.44.1, sherif 1.13.0, syncpack 15.3.3`. */
const toolVersions = (summary) =>
  Object.entries(summary.tool_versions)
    .map(([tool, version]) => (version.startsWith(`${tool} `) ? version : `${tool} ${version}`))
    .join(", ");

/** One sentence that says what was measured, built with what, on what machine and runner, when, and in which run. */
export function provenance(summary) {
  const tools = toolVersions(summary);
  const { github } = summary.machine;
  const rustc = summary.toolchain.rustc == null ? "" : ` built with ${summary.toolchain.rustc}`;
  const harness =
    summary.harness_rev === summary.source_rev ? "" : ` (measured by the harness at \`${summary.harness_rev}\`)`;
  const attempt = github?.run_attempt == null ? "" : `, attempt ${github.run_attempt}`;
  const run = github?.run_url == null ? "" : ` ([workflow run${attempt}](${github.run_url}))`;
  return (
    `Medians of ${summary.runs} runs after ${summary.warmup} warmup runs (${summary.hyperfine}), ` +
    `${summary.skif} at \`${summary.source_rev}\`${rustc}${harness} against ${tools}, measured on ` +
    `${date(summary.measured_at)} on ${runnerText(github)}${summary.machine_text}, load average ` +
    `${load(summary.load_before)} before and ${load(summary.load_after)} after the run${run}.`
  );
}

/** The full tables of the benchmarks page, below the compact ones that carry the provenance sentence. */
export function benchDetail(summary) {
  const lines = [];
  for (const target of summary.targets) {
    lines.push(`**${target.target}**`, "");
    if (target.installed_with != null) lines.push(`Installed with \`${target.installed_with}\`.`, "");
    lines.push("| Tool | Mean (ms) | Median (ms) | vs skif |", "| --- | ---: | ---: | ---: |");
    for (const row of target.results) {
      if (row.failure != null) {
        lines.push(`| ${row.tool} | ${fails(row)} | - | - |`);
        continue;
      }
      const skifRow = row.tool === SKIF || row.tool === SKIF_MANIFEST;
      const relative = skifRow || row.relative_to_skif == null ? "-" : `${row.relative_to_skif.toFixed(1)}x`;
      lines.push(`| ${row.tool} | ${ms(row.mean_ms)} | ${ms(row.median_ms)} | ${relative} |`);
    }
    lines.push("");
  }
  if (summary.adr_0001_checks.length > 0) {
    lines.push("| ADR 0001 target | Check | Result |", "| --- | --- | --- |");
    for (const check of summary.adr_0001_checks) {
      lines.push(`| ${check.target} | ${cell(check.description)} | ${CHECK_LABELS[check.status]} |`);
    }
    lines.push("");
  }
  return lines.join("\n");
}

const find = (target, tool) => target.results.find((row) => row.tool === tool);
const median = (target, tool) => find(target, tool)?.median_ms;
const repoName = (target) => target.target.replace(/^corpus: (\S+).*$/, "$1");
const generatedName = (target) => `Generated, ${target.packages} packages`;

/** The compact tables of the benchmarks page: medians, and knip's over skif's or why knip fails. */
export function benchSummary(summary) {
  const corpus = summary.targets.filter((target) => target.kind === "corpus");
  const generated = summary.targets.filter((target) => target.kind === "generated");
  const lines = [provenance(summary), "", "**Dead-code analysis against knip** (full skif run)", ""];
  lines.push("| Repository | skif | knip | knip / skif |", "| --- | ---: | ---: | ---: |");
  for (const target of corpus) {
    const skif = median(target, SKIF);
    const knip = find(target, KNIP);
    const [time, ratio] =
      knip.failure == null ? [duration(knip.median_ms), `${(knip.median_ms / skif).toFixed(0)}x`] : [fails(knip), "-"];
    lines.push(`| ${repoName(target)} | ${duration(skif)} | ${time} | ${ratio} |`);
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

/** `ratio` rounded down to one decimal, so a claim never states more than was measured. */
const down = (ratio) => (Math.floor(ratio * 10 + 1e-9) / 10).toFixed(1);

function middle(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const half = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[half] : (sorted[half - 1] + sorted[half]) / 2;
}

/**
 * How much faster skif is than `tool`, from the ratios of the tool's median over skif's on each target: the range, and
 * the median ratio from three targets on, each rounded down. Where skif was slower on some target it says so.
 */
export function speedClaim(tool, ratios) {
  const low = Math.min(...ratios);
  const high = Math.max(...ratios);
  const median = ratios.length >= 3 ? ` (median ${down(middle(ratios))}x)` : "";
  if (low >= 1) {
    const range = down(low) === down(high) ? `${down(high)}x` : `${down(low)}–${down(high)}x`;
    return `**${range} faster than ${tool}**${median}`;
  }
  const slower = ratios.filter((ratio) => ratio < 1).length;
  if (high < 1) return `**slower than ${tool}** on every target (${down(high)}x its speed at best)`;
  return `**up to ${down(high)}x faster than ${tool}**${median}, and slower on ${slower} of ${ratios.length} targets`;
}

const names = (items) =>
  items.length <= 1 ? items.join("") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
const repositories = (n) => `${n} pinned public ${n === 1 ? "repository" : "repositories"}`;
const workspaces = (n) => `${n} generated ${n === 1 ? "workspace" : "workspaces"}`;

/** The headline claims of the comparison guide, from the same medians as the compact tables. */
export function benchClaims(summary) {
  const corpus = summary.targets.filter((target) => target.kind === "corpus");
  const generated = summary.targets.filter((target) => target.kind === "generated");
  const lines = [];
  const completed = corpus.filter((target) => find(target, KNIP).failure == null);
  if (completed.length > 0) {
    const failed = corpus.filter((target) => find(target, KNIP).failure != null).map(repoName);
    const where = failed.length === 0 ? "" : ` where knip completes; it fails on ${names(failed)}`;
    const ratios = completed.map((target) => median(target, KNIP) / median(target, SKIF));
    lines.push(`- ${speedClaim("knip", ratios)} for dead-code analysis, on ${repositories(completed.length)}${where}.`);
  }
  const manifest = [...corpus, ...generated];
  const sizes = generated.map((target) => target.packages);
  const scope =
    `on ${repositories(corpus.length)}` +
    (generated.length === 0 ? "" : ` and ${workspaces(generated.length)} of ${names(sizes.map(String))} packages`);
  for (const [label, tool, job] of [
    ["sherif", SHERIF, "manifest and workspace checks"],
    ["syncpack", SYNCPACK, "version-policy checks"],
  ]) {
    const ratios = manifest.map((target) => median(target, tool) / median(target, SKIF_MANIFEST));
    lines.push(`- ${speedClaim(label, ratios)} for ${job} (\`skif --no-js\`), ${scope}.`);
  }
  const environment = RUNNER_ENVIRONMENTS[summary.machine.github?.runner_environment] ?? "GitHub Actions";
  lines.push(
    "",
    `${summary.skif} against ${toolVersions(summary)}: medians of ${summary.runs} runs on a ${environment} runner, ` +
      `measured on ${date(summary.measured_at)}.`,
  );
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

/** The agreement table of the benchmarks page, from the parity baseline. */
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
    benchOptional: false,
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
    options.benchOptional = !argv.includes("--summary");
    options.bench = true;
    options.parity = true;
  }
  return options;
}

const readJson = (file) => JSON.parse(readFileSync(file, "utf8"));

function main() {
  const options = parseArgs(process.argv.slice(2));
  const guides = join(repo, "website", "src", "content", "docs", "guides");
  const comparison = join(guides, "comparison.mdx");
  const benchmarks = join(guides, "benchmarks.mdx");
  const files = new Map([comparison, benchmarks].map((file) => [file, readFileSync(file, "utf8")]));
  const update = (file, name, body, mdx = false) => {
    files.set(file, replaceBlock(files.get(file), name, mdx ? mdxText(body) : body, { mdx, file }));
  };
  const problems = [];
  if (options.bench && options.benchOptional && !existsSync(options.summary)) {
    console.log(`refresh-numbers: no ${options.summary}, so the speed blocks are left as they are`);
  } else if (options.bench) {
    const summary = readJson(options.summary);
    const found = summaryProblems(summary);
    problems.push(...found.map((problem) => `${options.summary}: ${problem}`));
    if (found.length === 0) {
      update(comparison, "bench:claims", benchClaims(summary), true);
      update(benchmarks, "bench:summary", benchSummary(summary), true);
      update(benchmarks, "bench:numbers", benchDetail(summary), true);
    }
  }
  if (options.parity) {
    const baseline = readJson(options.baseline);
    const found = baselineProblems(baseline);
    problems.push(...found.map((problem) => `${options.baseline}: ${problem}`));
    if (found.length === 0) {
      update(benchmarks, "parity:agreement", parityAgreement(baseline), true);
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

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  main();
}
