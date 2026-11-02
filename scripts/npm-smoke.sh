#!/usr/bin/env bash
# Smoke-tests the host-built npm addon under one runtime: import, lint(), and the `skif` bin the way npx/bunx run it.
#
# Usage: scripts/npm-smoke.sh <node|bun>
#
# Expects `pnpm build` to have produced npm-package/index.js and the host .node file.
set -euo pipefail

runtime="${1:?usage: npm-smoke.sh <node|bun>}"
cd "$(dirname "$0")/.."
root="$PWD"
fixture="${root}/fixtures/e2e/npm/clean"
version="$(jq -r '.version' npm-package/package.json)"

case "$runtime" in
node) exec_bin=(npx --no-install skif) ;;
bun) exec_bin=(bunx --bun --no-install skif) ;;
*)
  echo "error: runtime must be node or bun (got '${runtime}')" >&2
  exit 1
  ;;
esac

echo "== ${runtime}: import and lint()"
"$runtime" -e '
  import { lint, run } from "'"${root}"'/npm-package/index.js";
  const report = JSON.parse(lint(process.argv[1]));
  if (typeof run !== "function" || report.version !== 1) throw new Error("unexpected report: " + JSON.stringify(report));
' "$fixture"

# A temporary project with skif symlinked in, so the bin resolves exactly as it does from node_modules/.bin.
project="$(mktemp -d)"
trap 'rm -rf "$project"' EXIT
mkdir -p "${project}/node_modules/.bin"
ln -s "${root}/npm-package" "${project}/node_modules/skif"
ln -s ../skif/bin/skif.js "${project}/node_modules/.bin/skif"
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

echo "== ${runtime}: exit code propagates"
status=0
(cd "$project" && "${exec_bin[@]}" --no-such-flag) >/dev/null 2>&1 || status=$?
[[ "$status" -eq 2 ]] || {
  echo "::error::expected exit 2 for a usage error, got ${status}"
  exit 1
}
echo "ok"
