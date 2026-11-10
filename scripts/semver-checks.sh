#!/usr/bin/env bash
# Runs cargo-semver-checks on every crate scripts/publish-crates.sh publishes, against its latest crates.io release,
# and writes one table of the results to $GITHUB_STEP_SUMMARY (stdout when unset) for a release PR to quote.
#
# Usage: scripts/semver-checks.sh
#
# Every crate is checked on its own: the facade is mostly `pub use` of the other crates, and cargo-semver-checks does
# not follow re-exports across crates, so checking `skif` alone misses a breaking change in a crate it re-exports.
# A crate not on crates.io yet is skipped. Exits 1 when any crate needs a major (for 0.x: minor) bump or could not be
# checked, 0 otherwise; the CI job is advisory while skif is 0.x.
#
# CRATES_API overrides the registry API base for tests (default https://crates.io/api/v1/crates).
set -euo pipefail

cd "$(dirname "$0")/.."

api="${CRATES_API:-https://crates.io/api/v1/crates}"
summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

# The publish list is the single source of the published crates.
crates=()
while IFS= read -r crate; do crates+=("$crate"); done < <(sed -n '/^crates=(/,/^)/s/^  \([a-z][a-z-]*\)$/\1/p' scripts/publish-crates.sh)
if [[ "${#crates[@]}" -eq 0 ]]; then
  echo "::error::found no crates in scripts/publish-crates.sh" >&2
  exit 2
fi

on_registry() {
  local status
  status="$(curl -sS -o /dev/null -w '%{http_code}' -H 'User-Agent: skif-ci (https://github.com/Goldziher/skif)' \
    "${api}/$1" || true)"
  case "$status" in
  200) return 0 ;;
  404) return 1 ;;
  *) return 2 ;;
  esac
}

log="$(mktemp -d)"
trap 'rm -rf "$log"' EXIT
rows=()
failed=()
for crate in "${crates[@]}"; do
  state=0
  on_registry "$crate" || state=$?
  case "$state" in
  1)
    rows+=("| \`${crate}\` | skipped: not on crates.io yet |")
    continue
    ;;
  2)
    rows+=("| \`${crate}\` | not checked: crates.io did not answer |")
    failed+=("$crate")
    continue
    ;;
  esac
  echo "::group::cargo semver-checks -p ${crate}"
  status=0
  cargo semver-checks --package "$crate" --all-features 2>&1 | tee "${log}/${crate}" || status=$?
  echo "::endgroup::"
  if [[ "$status" -eq 0 ]]; then
    rows+=("| \`${crate}\` | pass |")
  else
    rows+=("| \`${crate}\` | **semver update required** (exit ${status}) |")
    failed+=("$crate")
  fi
done

{
  echo "## Public API (cargo-semver-checks)"
  echo
  echo "Each published crate against its latest crates.io release, all features."
  echo
  echo "| Crate | Result |"
  echo "| --- | --- |"
  printf '%s\n' "${rows[@]}"
  for crate in ${failed[@]+"${failed[@]}"}; do
    [[ -f "${log}/${crate}" ]] || continue
    echo
    echo "<details><summary><code>${crate}</code></summary>"
    echo
    echo '```text'
    cat "${log}/${crate}"
    echo '```'
    echo
    echo "</details>"
  done
} >>"$summary"

if [[ "${#failed[@]}" -ne 0 ]]; then
  echo "::warning::cargo-semver-checks: ${failed[*]} need attention; see the job summary" >&2
  exit 1
fi
