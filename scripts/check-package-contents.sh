#!/usr/bin/env bash
# Fails unless every published crate carries the root LICENSE and ATTRIBUTIONS.md (byte-identical copies) and
# ships both in its package, as the MIT notices of vendored and derived code require (ADR 0015).
#
# Usage: scripts/check-package-contents.sh
set -euo pipefail

cd "$(dirname "$0")/.."

failures=0
for manifest in crates/*/Cargo.toml; do
  dir="$(dirname "$manifest")"
  crate="$(basename "$dir")"
  for notice in LICENSE ATTRIBUTIONS.md; do
    if ! cmp -s "$notice" "${dir}/${notice}"; then
      echo "::error file=${dir}/${notice}::${crate} must carry a copy of the root ${notice}"
      failures=$((failures + 1))
    fi
  done
  listing="$(cargo package -p "$crate" --list --locked --allow-dirty)"
  for notice in LICENSE ATTRIBUTIONS.md; do
    if ! grep -qx "$notice" <<<"$listing"; then
      echo "::error file=${manifest}::the ${crate} package does not contain ${notice}"
      failures=$((failures + 1))
    fi
  done
done

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} package content problem(s)" >&2
  exit 1
fi
echo "every published crate ships LICENSE and ATTRIBUTIONS.md"
