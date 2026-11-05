// Rewrites the numbers section of tools/bench/README.md from tools/bench/results/summary.json.
// Usage: node tools/bench/refresh-numbers.mjs [summary.json] [README.md]

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const summaryPath = process.argv[2] ?? join(here, "results", "summary.json");
const readmePath = process.argv[3] ?? join(here, "README.md");
const START = "<!-- bench:numbers:start -->";
const END = "<!-- bench:numbers:end -->";

const summary = JSON.parse(readFileSync(summaryPath, "utf8"));
if (summary.schema !== 1) {
  throw new Error(`${summaryPath}: unsupported schema ${summary.schema}`);
}

const ms = (value) => (value >= 100 ? value.toFixed(0) : value.toFixed(1));

const lines = [
  `Measured with ${summary.hyperfine}, ${summary.warmup} warmup and ${summary.runs} measured runs on ${summary.host}; ${summary.skif}.`,
  "",
];
for (const target of summary.targets) {
  lines.push(
    `**${target.target}**`,
    "",
    "| Tool | Mean (ms) | Median (ms) | vs skif |",
    "| --- | ---: | ---: | ---: |",
  );
  for (const row of target.results) {
    const relative = row.relative_to_skif == null ? "-" : `${row.relative_to_skif.toFixed(1)}x`;
    lines.push(`| ${row.tool} | ${ms(row.mean_ms)} | ${ms(row.median_ms)} | ${relative} |`);
  }
  lines.push("");
}
if (summary.adr_0001_checks.length > 0) {
  lines.push("| ADR 0001 target | Check | Result |", "| --- | --- | --- |");
  for (const check of summary.adr_0001_checks) {
    lines.push(`| ${check.target} | ${check.description} | ${check.passed ? "pass" : "FAIL"} |`);
  }
  lines.push("");
}

const readme = readFileSync(readmePath, "utf8");
const start = readme.indexOf(START);
const end = readme.indexOf(END);
if (start < 0 || end < start) {
  throw new Error(`${readmePath}: the ${START} and ${END} markers are missing`);
}
const next = `${readme.slice(0, start + START.length)}\n${lines.join("\n")}\n${readme.slice(end)}`;
writeFileSync(readmePath, next);
console.log(`refreshed ${readmePath} from ${summaryPath}`);
