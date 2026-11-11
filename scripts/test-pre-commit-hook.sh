#!/usr/bin/env bash
# Runs the skif pre-commit hooks through pre-commit (or prek) against a fixture repository with a known violation and
# a clean one, using this checkout as the hook repository.
#
# Usage: scripts/test-pre-commit-hook.sh <path-to-skif-binary>
#
# PRE_COMMIT overrides the runner (default: pre-commit, e.g. PRE_COMMIT=prek). The hook scripts are pointed at the
# given binary through SKIF_BINARY, so no release is downloaded.
set -euo pipefail

[[ $# -eq 1 ]] || {
  echo "usage: $0 <skif-binary>" >&2
  exit 2
}
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=e2e/lib.sh
source "${repo_root}/scripts/e2e/lib.sh"
skif="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
runner="${PRE_COMMIT:-pre-commit}"
[[ -x "$skif" ]] || {
  echo "skif binary not found or not executable: $skif" >&2
  exit 2
}
command -v "$runner" >/dev/null 2>&1 || {
  echo "${runner} is not installed" >&2
  exit 2
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
export PRE_COMMIT_HOME="${work}/pre-commit-home"

# The hook repository is this checkout at HEAD; pre-commit clones it, so uncommitted changes are not seen.
hook_rev="$(git -C "$repo_root" rev-parse HEAD)"

failures=0
fail() {
  echo "FAIL $*" >&2
  failures=$((failures + 1))
}

# make_project <dir-name> <scenario> <hook>: a git repository holding the end-to-end scenario generated as an npm
# workspace and a config using the hook. Nothing is installed, so the npm version of the pin is only test input.
make_project() {
  local scenario="$2" hook="$3" dir="${work}/$1"
  e2e_generate "${repo_root}/fixtures/e2e/${scenario}" npm hoisted 11.6.2 "$dir" || return
  cat >"${dir}/.pre-commit-config.yaml" <<YAML
repos:
  - repo: ${repo_root}
    rev: ${hook_rev}
    hooks:
      - id: ${hook}
YAML
  git -C "$dir" init -q
  git -C "$dir" add -A
  echo "$dir"
}

run_hooks() {
  (cd "$1" && SKIF_BINARY="$skif" "$runner" run --all-files)
}

clean_dir="$(make_project clean clean skif)"
if run_hooks "$clean_dir" >"${work}/clean.log" 2>&1; then
  echo "ok: the skif hook passes on a clean workspace"
else
  fail "the skif hook failed on a clean workspace"
  sed 's/^/    /' "${work}/clean.log" >&2
fi

dirty_dir="$(make_project dirty dirty skif)"
if run_hooks "$dirty_dir" >"${work}/dirty.log" 2>&1; then
  fail "the skif hook passed on a workspace with known violations"
  sed 's/^/    /' "${work}/dirty.log" >&2
elif grep -q 'version-mismatch' "${work}/dirty.log"; then
  echo "ok: the skif hook fails and reports the known violation"
else
  fail "the skif hook failed without reporting version-mismatch"
  sed 's/^/    /' "${work}/dirty.log" >&2
fi

# The fix hook rewrites manifests, so the dirty fixture must change.
fix_dir="$(make_project fix dirty skif-fix)"
(cd "$fix_dir" && env -u CI SKIF_BINARY="$skif" "$runner" run --all-files) >"${work}/fix.log" 2>&1 || true
if (cd "$fix_dir" && git diff --quiet); then
  fail "the skif-fix hook did not modify the dirty workspace"
  sed 's/^/    /' "${work}/fix.log" >&2
else
  echo "ok: the skif-fix hook rewrote the manifests"
fi

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "pre-commit hooks: all cases passed"
