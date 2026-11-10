#!/usr/bin/env bash
# Fails unless every skif pin in the documentation examples names the release line of the workspace version, so a
# reader copying an example gets the current release. The pins and the files scanned are listed in lib/doc-pins.sh.
#
# Usage: scripts/check-doc-pins.sh [expected-version]
#
# Without an argument the workspace version in Cargo.toml is the reference. Only the line is compared (0.M for a 0.x
# version, N from 1.0.0): `rev: v0.2.0` stays valid through every 0.2.x patch. A pre-release has no floating tag and
# the docs keep naming the previous release until the final one, so a pre-release version only lists the pins.
set -euo pipefail

cd "$(dirname "$0")/.."

# shellcheck source=lib/doc-pins.sh
source scripts/lib/doc-pins.sh

workspace_version="$(awk '/^\[workspace\.package\]/{p=1;next} /^\[/{p=0} p && /^version *= *"/{gsub(/"/,"",$3);print $3;exit}' Cargo.toml)"
expected="${1:-$workspace_version}"
line="$(skif_release_line "$expected")"

pins="$(skif_doc_pins)"
if [[ -z "$pins" ]]; then
  echo "::error::no skif version pins found in the documentation; scripts/lib/doc-pins.sh no longer matches the examples"
  exit 1
fi

if [[ "$expected" == *-* ]]; then
  echo "${expected} is a pre-release; the documentation pins are not checked:"
  while IFS= read -r entry; do printf '  %s\n' "$entry"; done <<<"$pins"
  exit 0
fi

failures=0
while IFS= read -r entry; do
  file="${entry%%:*}"
  rest="${entry#*:}"
  number="${rest%%:*}"
  pin="${rest#*:}"
  if [[ "$(skif_release_line "$pin")" != "$line" ]]; then
    echo "::error file=${file},line=${number}::skif pin ${pin} is not on the ${expected} release line (v${line}); run scripts/release-bump.sh or update the example"
    failures=$((failures + 1))
  fi
done <<<"$pins"

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} documentation pin(s) not on the release line of ${expected}" >&2
  exit 1
fi
echo "Every documentation pin names the v${line} line of ${expected}"
