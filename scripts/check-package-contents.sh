#!/usr/bin/env bash
# Fails unless every published crate and the npm package carry the root LICENSE and ATTRIBUTIONS.md (byte-identical
# copies) and ship both in their package, as the MIT notices of vendored and derived code require (ADR 0033).
#
# Usage: scripts/check-package-contents.sh
set -euo pipefail

cd "$(dirname "$0")/.."

failures=0
for manifest in crates/*/Cargo.toml; do
  dir="$(dirname "$manifest")"
  crate="$(basename "$dir")"
  # Crates that never reach crates.io (the npm addon) ship no package.
  if grep -qx 'publish = false' "$manifest"; then
    continue
  fi
  for notice in LICENSE ATTRIBUTIONS.md; do
    if ! cmp -s "$notice" "${dir}/${notice}"; then
      echo "::error file=${dir}/${notice}::${crate} must carry a copy of the root ${notice}"
      failures=$((failures + 1))
    fi
  done
  listing="$(cargo package -p "$crate" --list --locked --allow-dirty)"
  # The tests of these crates read files of the repository (rule ids, fixtures, ADRs) that a source tarball lacks.
  case "$crate" in
  skif-migrate | skif-rules)
    if grep -q '^tests/' <<<"$listing"; then
      echo "::error file=${manifest}::the ${crate} package ships tests that read files outside it"
      failures=$((failures + 1))
    fi
    ;;
  esac
  for notice in LICENSE ATTRIBUTIONS.md; do
    if ! grep -qx "$notice" <<<"$listing"; then
      echo "::error file=${manifest}::the ${crate} package does not contain ${notice}"
      failures=$((failures + 1))
    fi
  done
done

# The npm umbrella packs from npm-package/, so it needs the same copies (the platform packages get them at publish).
for notice in LICENSE ATTRIBUTIONS.md; do
  if ! cmp -s "$notice" "npm-package/${notice}"; then
    echo "::error file=npm-package/${notice}::npm-package must carry a copy of the root ${notice}"
    failures=$((failures + 1))
  fi
done

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} package content problem(s)" >&2
  exit 1
fi
echo "every published crate and the npm package ship LICENSE and ATTRIBUTIONS.md"
