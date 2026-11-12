#!/usr/bin/env bash
# Fails unless the dependency declarations follow the workspace conventions:
#   - the oxc crates in [workspace.dependencies] use caret ranges, and all but oxc_resolver name one release, see
#     ADR 0013;
#   - every dependency of a member crate is inherited with `workspace = true`.
#
# Usage: scripts/check-pins.sh
set -euo pipefail

cd "$(dirname "$0")/.."

failures=0

while IFS= read -r line; do
  echo "::error file=Cargo.toml::oxc dependency is not a caret range: ${line}"
  failures=$((failures + 1))
done < <(awk '/^\[workspace\.dependencies\]/{p=1;next} /^\[/{p=0} p && /^oxc[A-Za-z0-9_-]* *=/ && !/= *"[0-9]/ && !/version *= *"[0-9]/' Cargo.toml)

releases="$(awk '/^\[workspace\.dependencies\]/{p=1;next} /^\[/{p=0}
  p && /^oxc[A-Za-z0-9_-]* *=/ && !/^oxc_resolver/ { match($0, /"[^"]*"/); print substr($0, RSTART + 1, RLENGTH - 2) }' Cargo.toml | sort -u)"
if [[ "$(grep -c . <<<"$releases")" -gt 1 ]]; then
  echo "::error file=Cargo.toml::the oxc crates must name one release, found: ${releases//$'\n'/, }"
  failures=$((failures + 1))
fi

for manifest in crates/*/Cargo.toml tools/*/Cargo.toml; do
  while IFS= read -r line; do
    echo "::error file=${manifest}::dependency is not inherited from the workspace: ${line}"
    failures=$((failures + 1))
  done < <(awk '
    /^\[/ { in_deps = ($0 ~ /^\[(dev-|build-)?dependencies\]$/ || $0 ~ /^\[target\..*\.(dev-|build-)?dependencies\]$/); next }
    in_deps && /^[A-Za-z0-9_-]+ *=/ && !/workspace *= *true/ { print }
  ' "$manifest")
done

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} dependency declaration(s) break the workspace conventions" >&2
  exit 1
fi
echo "dependency declarations follow the workspace conventions"
