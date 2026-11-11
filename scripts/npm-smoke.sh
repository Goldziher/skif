#!/usr/bin/env bash
# Smoke-tests the host-built npm addon under one runtime: import, lint(), and the `skif` bin the way npx/bunx run it.
#
# Usage: scripts/npm-smoke.sh <node|bun>
#
# Expects `pnpm build` to have produced npm-package/index.js and the host .node file. With SKIF_EXPECT_PANIC_HOOK=1
# the addon must also export the `panicForTest` hook (built with `--features test-hooks`).
set -euo pipefail

runtime="${1:?usage: npm-smoke.sh <node|bun>}"
cd "$(dirname "$0")/.."
root="$PWD"
# shellcheck source=e2e/lib.sh
source "${root}/scripts/e2e/lib.sh"
version="$(jq -r '.version' npm-package/package.json)"

case "$runtime" in
node) exec_bin=(npx --no-install skif) ;;
bun) exec_bin=(bunx --bun --no-install skif) ;;
*)
  echo "error: runtime must be node or bun (got '${runtime}')" >&2
  exit 1
  ;;
esac

# A temporary project with skif symlinked in, so the bin resolves exactly as it does from node_modules/.bin, and beside
# it the clean end-to-end scenario generated as an npm workspace: the fixture the addon and the bin lint. Nothing is
# installed, so the npm version of its `packageManager` pin is only test input.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
project="${work}/project"
fixture="${work}/fixture"
mkdir "$project"
e2e_generate "${root}/fixtures/e2e/clean" npm hoisted 11.6.2 "$fixture"

cat >"${project}/addon.mjs" <<'JS'
import * as addon from "ADDON";

const [fixture, missing] = process.argv.slice(2);
const fail = (message) => {
  throw new Error(message);
};

if (typeof addon.run !== "function") fail("run is not exported");
if (addon.run(["--version"]) !== 0) fail("run(['--version']) did not exit 0");

const report = JSON.parse(addon.lint(fixture));
if (report.version !== 1) fail("unexpected report: " + JSON.stringify(report));
if (report.summary.errors !== 0) fail("the clean fixture reported errors: " + JSON.stringify(report.diagnostics));

const asyncOutcome = await addon.lintAsync(fixture, { noJs: false });
if (JSON.stringify(JSON.parse(asyncOutcome.report)) !== JSON.stringify(report)) fail("lintAsync differs from lint");

const isolated = await addon.lintAsync(fixture, { noConfig: true });
if (!isolated.report) fail("noConfig produced no report: " + JSON.stringify(isolated.error));

const noFile = await addon.lintAsync(fixture, { config: missing + "/skif.toml" });
if (noFile.report || !noFile.error) fail("a missing config file did not fail: " + JSON.stringify(noFile));

const failure = await addon.lintAsync(missing);
if (failure.report !== undefined && failure.report !== null) fail("a missing path produced a report");
if (!failure.error || !failure.error.kind || !failure.error.message) fail("no structured error: " + JSON.stringify(failure));

let thrown;
try {
  addon.lint(missing);
} catch (error) {
  thrown = error;
}
if (!thrown) fail("lint() of a missing path did not throw");

if (typeof addon.panicForTest === "function") {
  let panic;
  try {
    addon.panicForTest();
  } catch (error) {
    panic = error;
  }
  if (!panic || !String(panic.message).includes("deliberate panic")) fail("a panic did not become an exception");
  if (addon.run(["--version"]) !== 0) fail("the addon is unusable after a panic");
} else if (process.env.SKIF_EXPECT_PANIC_HOOK === "1") {
  fail("the test-hooks build does not export panicForTest");
}
JS
sed -i.bak "s#ADDON#${root}/npm-package/index.js#" "${project}/addon.mjs"

echo "== ${runtime}: import, run(), lint(), lintAsync() and a panic"
"$runtime" "${project}/addon.mjs" "$fixture" "${project}/does-not-exist"

mkdir -p "${project}/node_modules/.bin"
ln -s "${root}/npm-package" "${project}/node_modules/skif-cli"
ln -s ../skif-cli/bin/skif.js "${project}/node_modules/.bin/skif"
echo '{"name":"smoke","private":true}' >"${project}/package.json"

echo "== ${runtime}: bin --version"
out="$(cd "$project" && "${exec_bin[@]}" --version)"
grep -qF "$version" <<<"$out" || {
  echo "::error::expected version ${version}, got: ${out}"
  exit 1
}

echo "== ${runtime}: bin lint of the clean fixture"
(cd "$project" && "${exec_bin[@]}" "$fixture" --format json) >"${project}/out.json"
jq -e '.summary' "${project}/out.json" >/dev/null

echo "== ${runtime}: bin under bazel run lints the working directory, not the current one"
(cd "$project" && BUILD_WORKSPACE_DIRECTORY="$work" BUILD_WORKING_DIRECTORY=fixture "${exec_bin[@]}" --format json) \
  >"${project}/bazel-run.json"
[[ "$(jq -S . "${project}/bazel-run.json")" == "$(jq -S . "${project}/out.json")" ]] || {
  echo "::error::under bazel run the bin did not lint BUILD_WORKING_DIRECTORY: $(cat "${project}/bazel-run.json")"
  exit 1
}

echo "== ${runtime}: exit code propagates"
status=0
(cd "$project" && "${exec_bin[@]}" --no-such-flag) >/dev/null 2>&1 || status=$?
[[ "$status" -eq 2 ]] || {
  echo "::error::expected exit 2 for a usage error, got ${status}"
  exit 1
}
echo "ok"
