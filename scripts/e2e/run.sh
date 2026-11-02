#!/usr/bin/env bash
# End-to-end check of skif against a real package manager.
#
# Usage: scripts/e2e/run.sh <npm|pnpm|yarn|bun> <path-to-skif-binary>
#
# For each scenario under fixtures/e2e/<pm>/ the fixture is copied to a temp dir, installed with
# the real package manager, linted, and (for scenarios expecting issues) fixed from another working
# directory (skif runs the real install itself, no --no-install), and re-linted. The JSON `fix` object of the fix run
# is checked too. JS analysis stays on: every package imports its dependencies from an entry file, so the fixtures
# are clean under it. Expectations live in each scenario's expected.json.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <npm|pnpm|yarn|bun> <skif-binary>" >&2
  exit 2
fi

pm="$1"
skif="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixtures="$repo_root/fixtures/e2e/$pm"

[[ -d "$fixtures" ]] || {
  echo "no fixtures for package manager '$pm' in $fixtures" >&2
  exit 2
}
[[ -x "$skif" ]] || {
  echo "skif binary not found or not executable: $skif" >&2
  exit 2
}

work_root="$(mktemp -d)"
trap 'rm -rf "$work_root"' EXIT

# CI sets CI=true, which makes every manager default to a frozen lockfile. The fixtures commit no
# lockfile, so each install must be allowed to create one.
install() {
  local dir="$1"
  case "$pm" in
  npm) (cd "$dir" && npm install --ignore-scripts --no-audit --no-fund) ;;
  pnpm) (cd "$dir" && pnpm install --ignore-scripts --no-frozen-lockfile) ;;
  yarn) (cd "$dir" && YARN_ENABLE_IMMUTABLE_INSTALLS=false yarn install --mode=skip-build) ;;
  bun) (cd "$dir" && bun install --ignore-scripts) ;;
  *)
    echo "unsupported package manager: $pm" >&2
    return 2
    ;;
  esac
}

failures=0
scenario=""

fail() {
  echo "FAIL [$pm/$scenario]: $*" >&2
  failures=$((failures + 1))
}

# Runs skif in JSON mode; sets $skif_out and $skif_code.
lint() {
  skif_code=0
  skif_out="$(env -u GITHUB_ACTIONS "$skif" --format json "$1")" || skif_code=$?
}

# Compares the exit code and sorted rule ids of the last lint with the expectation.
check() {
  local label="$1" want_code="$2" want_rules="$3"
  local got_rules
  if ! got_rules="$(jq -c '[.diagnostics[].rule] | sort' <<<"$skif_out")"; then
    fail "$label: skif did not print valid JSON: $skif_out"
    return
  fi
  want_rules="$(jq -c 'sort' <<<"$want_rules")"
  if [[ "$skif_code" -ne "$want_code" ]]; then
    fail "$label: exit code $skif_code, expected $want_code"
  fi
  if [[ "$got_rules" != "$want_rules" ]]; then
    fail "$label: rule mismatch (expected < > actual)"
    diff <(jq . <<<"$want_rules") <(jq . <<<"$got_rules") >&2 || true
  fi
  if [[ "$skif_code" -ne "$want_code" || "$got_rules" != "$want_rules" ]]; then
    echo "$skif_out" >&2
  fi
}

for fixture in "$fixtures"/*/; do
  scenario="$(basename "$fixture")"
  expected="$fixture/expected.json"
  dir="$work_root/$scenario"
  echo "== $pm/$scenario"

  cp -R "$fixture" "$dir"
  rm "$dir/expected.json"
  # Yarn berry needs a lockfile to treat the copy as its own project root.
  if [[ "$pm" == yarn ]]; then
    touch "$dir/yarn.lock"
  fi

  want_code="$(jq -r '.exit_code' "$expected")"
  want_rules="$(jq -c '.rules' "$expected")"
  want_rules_after_fix="$(jq -c '.rules_after_fix' "$expected")"

  install "$dir"
  lint "$dir"
  check "lint" "$want_code" "$want_rules"

  if [[ "$want_code" -ne 0 ]]; then
    # Fix from a directory that is not the workspace: every write must land under the workspace root, and
    # nothing may be created in the current directory. skif refuses --fix when CI is truthy. It runs the real
    # `<pm> install` itself afterwards and passes the flags that let the lockfile change, so neither CI nor
    # GITHUB_ACTIONS (which pnpm and yarn read through ci-info to freeze the lockfile) may leak into the run.
    elsewhere="$(mktemp -d "$work_root/cwd.XXXXXX")"
    fix_code=0
    fix_out="$(cd "$elsewhere" && env -u CI -u GITHUB_ACTIONS \
      "$skif" --fix-unsafe --select highest --format json "$dir")" || fix_code=$?
    [[ "$fix_code" -eq 0 ]] || fail "--fix exited $fix_code: $fix_out"
    [[ -z "$(ls -A "$elsewhere")" ]] || fail "--fix created files in the current directory: $(ls -A "$elsewhere")"
    jq -e '.private == true' "$dir/package.json" >/dev/null || fail "--fix did not fix the root package.json"
    jq -e '(.fix.applied | length) > 0 and (.fix.changed | index("package.json") != null)' <<<"$fix_out" >/dev/null ||
      fail "the fix object does not list the applied fixes and the changed files: $fix_out"
    jq -e '.fix.skipped == []' <<<"$fix_out" >/dev/null || fail "--fix skipped fixes: $fix_out"
    jq -e --arg pm "$pm" '.fix.install.status == "ran" and (.fix.install.command | startswith($pm + " install"))' <<<"$fix_out" >/dev/null ||
      fail "skif did not run '$pm install': $fix_out"
    lint "$dir"
    check "lint after fix" 0 "$want_rules_after_fix"
  fi
done

if [[ "$failures" -ne 0 ]]; then
  echo "$failures check(s) failed for $pm" >&2
  exit 1
fi
echo "all $pm scenarios passed"
