#!/usr/bin/env bash
# Fails when a committed comparison baseline was recorded with another version of its tool than the one pinned in
# tools/bench/corpus.toml [tools], so a pin bump cannot leave the baselines describing the old release.
#
# Usage: scripts/check-baseline-tool-versions.sh
#
# Checked: the top-level "tool" of tools/bench/baselines/parity.json (knip@X) and syncpack.json (syncpack@X).
set -euo pipefail

cd "$(dirname "$0")/.."

corpus=tools/bench/corpus.toml
baselines=tools/bench/baselines

pinned() {
  awk -v tool="$1" '/^\[tools\]/{p=1;next} /^\[/{p=0} p && $1==tool && $2=="="{gsub(/"/,"",$3);print $3;exit}' "$corpus"
}

failures=0
fail() {
  echo "::error file=$1::$2"
  failures=$((failures + 1))
}

# check <baseline file> <tool>
check() {
  local file="${baselines}/$1" tool="$2" pin recorded
  pin="$(pinned "$tool")"
  if [[ -z "$pin" ]]; then
    fail "$corpus" "no ${tool} version in [tools]"
    return
  fi
  if [[ ! -f "$file" ]]; then
    fail "$file" "the baseline does not exist"
    return
  fi
  recorded="$(sed -nE 's/^  "tool": "'"${tool}"'@([^"]+)".*/\1/p' "$file" | head -n 1)"
  if [[ -z "$recorded" ]]; then
    fail "$file" "no \"tool\": \"${tool}@<version>\" line found"
  elif [[ "$recorded" != "$pin" ]]; then
    fail "$file" "recorded with ${tool} ${recorded}, but ${corpus} pins ${pin}; re-record the baseline"
  fi
}

check parity.json knip
check syncpack.json syncpack

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} baseline(s) do not match ${corpus}" >&2
  exit 1
fi
echo "The parity and syncpack baselines match the knip and syncpack pins of ${corpus}"
