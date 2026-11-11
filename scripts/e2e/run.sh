#!/usr/bin/env bash
# End-to-end check of skif against a real package manager.
#
# Usage: scripts/e2e/run.sh <npm|pnpm|yarn|yarn-classic|bun> <path-to-skif-binary>
#
# Every scenario under fixtures/e2e/ that applies to the manager is generated (scripts/e2e/lib.sh) once per linker of
# the manager, installed with the real manager, and linted with JS analysis on. The findings and the exit code are
# compared with the scenario's expectation. A scenario with a `migrate` step translates its sherif, syncpack and knip
# configuration and lints again. A scenario that expects issues then records them with `skif baseline write`, which
# must silence them and make `baseline check` fail against an empty baseline, and is fixed from another working
# directory (skif runs the real `<pm> install` itself) and linted again.
#
# A scenario may name a script for `<pm> run` in one of its packages and whether it succeeds under the installed layout,
# which proves the binaries the layout provides. An override with a `known_issue` (a GitHub issue and its title) marks
# an expectation that records a skif bug rather than the correct answer; the runner prints it as KNOWN ISSUE.
#
# The version under test is the one `<pm> --version` prints outside any project, or E2E_PM_VERSION; it is written to
# every root's `packageManager`, and each root must report it, so corepack or pnpm cannot switch to another version.
# E2E_SCENARIOS and E2E_LINKERS (space separated) limit the run.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <npm|pnpm|yarn|yarn-classic|bun> <skif-binary>" >&2
  exit 2
fi

pm="$1"
skif="$2"
# Git Bash finds a Windows binary under its name without `.exe` only for some commands; use the real file name.
if [[ ! -e "$skif" && -e "${skif}.exe" ]]; then
  skif="${skif}.exe"
fi
[[ -x "$skif" ]] || {
  echo "skif binary not found or not executable: $skif" >&2
  exit 2
}
skif="$(cd "$(dirname "$skif")" && pwd)/$(basename "$skif")"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib.sh
source "${repo_root}/scripts/e2e/lib.sh"

cmd="$(e2e_command "$pm")" || exit 2
linkers="$(e2e_linkers "$pm")"
if [[ -n "${E2E_LINKERS:-}" ]]; then
  for linker in $E2E_LINKERS; do
    if [[ " ${linkers} " != *" ${linker} "* ]]; then
      echo "E2E_LINKERS: ${pm} has no linker '${linker}' (its linkers: ${linkers})" >&2
      exit 2
    fi
  done
  linkers="$E2E_LINKERS"
fi

work_root="$(mktemp -d)"
trap 'rm -rf "$work_root"' EXIT

version="${E2E_PM_VERSION:-}"
if [[ -z "$version" ]]; then
  mkdir "${work_root}/version"
  version="$(cd "${work_root}/version" && "$cmd" --version | tr -d '\r')"
fi
echo "Testing ${pm} ${version} with the linkers: ${linkers}"

failures=0
label=""

fail() {
  echo "FAIL [${label}]: $*" >&2
  failures=$((failures + 1))
}

# skif_json <args...>: runs skif in JSON mode outside GitHub's annotation output; sets $skif_out and $skif_code.
skif_json() {
  skif_code=0
  skif_out="$(env -u GITHUB_ACTIONS "$skif" --format json "$@")" || skif_code=$?
}

# check <step> <expected-json>: compares the exit code and the findings of the last skif run with `exit_code` and
# `diagnostics` of the expectation.
check() {
  local step="$1" want="$2" want_code want_findings got_findings
  want_code="$(jqr -r '.exit_code' <<<"$want")"
  if [[ ! "$want_code" =~ ^[0-9]+$ ]]; then
    fail "${step}: the expectation has no numeric exit_code: ${want}"
    return
  fi
  want_findings="$(jqr -c '.diagnostics' <<<"$want")"
  if ! got_findings="$(e2e_findings "$skif_out")"; then
    fail "${step}: skif did not print valid JSON: ${skif_out}"
    return
  fi
  if [[ "$skif_code" -ne "$want_code" ]]; then
    fail "${step}: exit code ${skif_code}, expected ${want_code}"
  fi
  if [[ "$got_findings" != "$want_findings" ]]; then
    fail "${step}: findings differ (expected < > actual)"
    diff <(jq '.[]' <<<"$want_findings") <(jq '.[]' <<<"$got_findings") >&2 || true
  fi
  if [[ "$skif_code" -ne "$want_code" || "$got_findings" != "$want_findings" ]]; then
    echo "$skif_out" >&2
  fi
}

# check_layout <dir> <linker>: proves the install used the linker, so a scenario cannot pass on the wrong layout.
check_layout() {
  local dir="$1" layout hoisted marker
  if ! layout="$(e2e_layout "$pm" "$2")"; then
    fail "no layout is known for the ${2} linker"
    return
  fi
  read -r hoisted marker <<<"$layout"
  if [[ "$marker" != - && ! -e "${dir}/${marker}" ]]; then
    fail "the ${2} layout lacks ${marker}"
  fi
  if [[ "$hoisted" == yes && ! -f "${dir}/node_modules/is-number/package.json" ]]; then
    fail "the ${2} layout did not hoist is-number into the root node_modules"
  fi
  if [[ "$hoisted" == no && -e "${dir}/node_modules/is-number" ]]; then
    fail "the ${2} layout hoisted is-number into the root node_modules"
  fi
}

# skif_files <dir>: the skif.toml and skif.baseline.toml files below dir, sorted.
skif_files() {
  find "$1" -name node_modules -prune -o \( -name skif.toml -o -name skif.baseline.toml \) -type f -print |
    LC_ALL=C sort
}

# check_baseline <dir> <lint-args...>: `baseline write` records every finding, so the lint passes; the baseline grew
# compared with an empty one. The files it created are removed again; those the scenario ships stay.
check_baseline() {
  local dir="$1" before written empty
  shift
  before="$(skif_files "$dir")"
  skif_code=0
  env -u GITHUB_ACTIONS "$skif" baseline write "$@" "$dir" >/dev/null || skif_code=$?
  [[ "$skif_code" -eq 0 ]] || fail "baseline write exited ${skif_code}"
  skif_json "$@" "$dir"
  check "lint with the baseline" '{"exit_code": 0, "diagnostics": []}'
  if [[ $# -eq 0 && ! -f "${dir}/skif.baseline.toml" ]]; then
    fail "baseline write did not create skif.baseline.toml"
  elif [[ $# -eq 0 ]]; then
    written="${work_root}/written.baseline.toml"
    empty="${work_root}/empty.baseline.toml"
    cp "${dir}/skif.baseline.toml" "$written"
    : >"$empty"
    skif_code=0
    env -u GITHUB_ACTIONS "$skif" baseline check --base-file "$written" "$dir" >/dev/null 2>&1 || skif_code=$?
    [[ "$skif_code" -eq 0 ]] || fail "baseline check against itself exited ${skif_code}, expected 0"
    skif_code=0
    env -u GITHUB_ACTIONS "$skif" baseline check --base-file "$empty" "$dir" >/dev/null 2>&1 || skif_code=$?
    [[ "$skif_code" -eq 1 ]] || fail "baseline check against an empty baseline exited ${skif_code}, expected 1"
  fi
  LC_ALL=C comm -13 <(printf '%s\n' "$before") <(skif_files "$dir") | while IFS= read -r file; do
    [[ -z "$file" ]] || rm "$file"
  done
}

# check_run <dir> <expected-json>: runs the expected script with the manager and compares whether it succeeded.
check_run() {
  local dir="$1" want="$2" member script succeeds code=0 out
  member="$(jqr -r '.dir' <<<"$want")"
  script="$(jqr -r '.script' <<<"$want")"
  succeeds="$(jqr -r '.succeeds' <<<"$want")"
  out="$(cd "${dir}/${member}" && env -u CI -u GITHUB_ACTIONS "$cmd" run "$script" 2>&1)" || code=$?
  if [[ "$succeeds" == true && "$code" -ne 0 ]]; then
    fail "'${cmd} run ${script}' in ${member} exited ${code}, expected it to succeed: ${out}"
  elif [[ "$succeeds" == false && "$code" -eq 0 ]]; then
    fail "'${cmd} run ${script}' in ${member} succeeded, expected it to fail: ${out}"
  elif [[ "$succeeds" != true && "$succeeds" != false ]]; then
    fail "run.succeeds must be true or false, got '${succeeds}'"
  fi
}

# check_fix <dir> <expected-json> <lint-args...>: fixes from a directory that is not the workspace, so every write must
# land under the workspace root and nothing may be created in the current directory. skif refuses --fix when CI is
# truthy and runs the real `<pm> install` itself afterwards with the flags that let the lockfile change, so neither CI
# nor GITHUB_ACTIONS (which pnpm and yarn read through ci-info to freeze the lockfile) may leak into the run.
check_fix() {
  local dir="$1" want="$2" elsewhere fix_out fix_code want_code
  shift 2
  want_code="$(jqr -r '.exit_code' <<<"$want")"
  if [[ ! "$want_code" =~ ^[0-9]+$ ]]; then
    fail "after fix: the expectation has no numeric exit_code: ${want}"
    return
  fi
  elsewhere="$(mktemp -d "${work_root}/cwd.XXXXXX")"
  fix_code=0
  fix_out="$(cd "$elsewhere" && env -u CI -u GITHUB_ACTIONS \
    "$skif" --fix-unsafe --allow-remove-files --select highest --format json "$@" "$dir")" || fix_code=$?
  [[ "$fix_code" -eq "$want_code" ]] || fail "--fix exited ${fix_code}, expected ${want_code}: ${fix_out}"
  [[ -z "$(ls -A "$elsewhere")" ]] || fail "--fix created files in the current directory: $(ls -A "$elsewhere")"
  jq -e '(.fix.applied | length) > 0 and (.fix.changed | length) > 0' <<<"$fix_out" >/dev/null ||
    fail "the fix object does not list the applied fixes and the changed files: ${fix_out}"
  jq -e '.fix.skipped == []' <<<"$fix_out" >/dev/null || fail "--fix skipped fixes: ${fix_out}"
  jq -e --arg cmd "$cmd" '.fix.install.status == "ran" and (.fix.install.command | startswith($cmd + " install"))' \
    <<<"$fix_out" >/dev/null || fail "skif did not run '${cmd} install': ${fix_out}"
  skif_json "$@" "$dir"
  check "lint after fix" "$want"
}

# run_scenario <scenario-dir> <linker>
run_scenario() {
  local scenario="$1" linker="$2" name dir root reported expected lint_args=() migrate
  name="$(basename "$scenario")"
  label="${pm}/${linker}/${name}"
  dir="${work_root}/${name}-${linker}"
  echo "== ${label}"

  if ! e2e_generate "$scenario" "$pm" "$linker" "$version" "$dir"; then
    fail "generating the project failed"
    return
  fi
  while IFS= read -r root; do
    if ! e2e_install "$pm" "${dir}/${root}"; then
      fail "${cmd} install failed in ${root}"
      return
    fi
    reported="$(cd "${dir}/${root}" && "$cmd" --version | tr -d '\r')"
    [[ "$reported" == "$version" ]] || fail "${cmd} in ${root} is ${reported}, not the ${version} under test"
  done < <(e2e_roots "$scenario")
  check_layout "$dir" "$linker"

  if ! expected="$(e2e_expectation "$scenario" "$pm" "$linker")"; then
    fail "the expectation does not evaluate"
    return
  fi
  while IFS= read -r issue; do
    echo "KNOWN ISSUE [${label}]: ${issue}"
  done < <(jqr -r '.known_issues // [] | .[]' <<<"$expected")
  if [[ "$(jqr -c '.run // empty' <<<"$expected")" != "" ]]; then
    check_run "$dir" "$(jqr -c '.run' <<<"$expected")"
  fi
  while IFS= read -r arg; do lint_args+=("$arg"); done < <(jqr -r '.lint_args // [] | .[]' "${scenario}/scenario.json")

  skif_json ${lint_args[@]+"${lint_args[@]}"} "$dir"
  check "lint" "$expected"

  migrate="$(jqr -r '.migrate.from // empty' "${scenario}/scenario.json")"
  if [[ -n "$migrate" ]]; then
    skif_code=0
    env -u GITHUB_ACTIONS "$skif" migrate --from "$migrate" --profile full "$dir" >/dev/null || skif_code=$?
    [[ "$skif_code" -eq 0 ]] || fail "migrate exited ${skif_code}"
    if [[ "$(jqr -c '.migrate // empty' <<<"$expected")" == "" ]]; then
      fail "the scenario migrates but its expectation has no migrate step"
      return
    fi
    skif_json ${lint_args[@]+"${lint_args[@]}"} "$dir"
    check "lint after migrate" "$(jqr -c '.migrate' <<<"$expected")"
    return
  fi

  if [[ "$(jqr -r '.exit_code' <<<"$expected")" != 0 ]]; then
    if [[ "$(jqr -c '.after_fix // empty' <<<"$expected")" == "" ]]; then
      fail "the expectation has a non-zero exit code but no after_fix step"
      return
    fi
    check_baseline "$dir" ${lint_args[@]+"${lint_args[@]}"}
    check_fix "$dir" "$(jqr -c '.after_fix' <<<"$expected")" ${lint_args[@]+"${lint_args[@]}"}
  fi
}

for scenario in "${repo_root}"/fixtures/e2e/*/; do
  scenario="${scenario%/}"
  name="$(basename "$scenario")"
  [[ -f "${scenario}/scenario.json" ]] || continue
  if [[ -n "${E2E_SCENARIOS:-}" && " ${E2E_SCENARIOS} " != *" ${name} "* ]]; then
    continue
  fi
  reason="$(e2e_applies "$scenario" "$pm" "$version")"
  if [[ -n "$reason" ]]; then
    echo "-- skip ${pm}/${name}: ${reason}"
    continue
  fi
  for linker in $linkers; do
    run_scenario "$scenario" "$linker"
  done
done

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} check(s) failed for ${pm} ${version}" >&2
  exit 1
fi
echo "all ${pm} ${version} scenarios passed"
