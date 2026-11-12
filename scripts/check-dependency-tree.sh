#!/usr/bin/env bash
# Fails unless the dependency layering of ADR 0002 and ADR 0013 holds:
#   - `skif` without default features pulls in no oxc crate;
#   - `skif-core` depends on no other workspace crate;
#   - oxc crates are reachable only through skif-js-syntax, skif-js-plugins and skif-js.
#
# Usage: scripts/check-dependency-tree.sh
set -euo pipefail

cd "$(dirname "$0")/.."

failures=0
errors="$(mktemp)"
trap 'rm -f "$errors"' EXIT

if cargo tree -p skif --no-default-features -e normal --prefix none --locked | grep '^oxc_' >/dev/null; then
  echo "::error::skif without default features depends on an oxc crate"
  failures=$((failures + 1))
fi

while IFS= read -r crate; do
  echo "::error::skif-core depends on the workspace crate ${crate}"
  failures=$((failures + 1))
done < <(cargo tree -p skif-core -e normal --prefix none --locked | awk '$1 ~ /^skif/ && $1 != "skif-core" {print $1}' | sort -u)

allowed=" skif-js-syntax skif-js-plugins skif-js "
for oxc in oxc_allocator oxc_ast oxc_ast_visit oxc_parser oxc_semantic oxc_span oxc_str oxc_resolver; do
  if ! tree="$(cargo tree -e normal --invert "$oxc" --depth 1 --prefix none --locked 2>"$errors")"; then
    if grep -q 'did not match any packages' "$errors"; then
      echo "::warning::${oxc} is not in the dependency graph; nothing to check for it"
      continue
    fi
    echo "::error::cargo tree failed for ${oxc}: $(cat "$errors")"
    failures=$((failures + 1))
    continue
  fi
  while IFS= read -r crate; do
    if [[ "$allowed" != *" ${crate} "* ]]; then
      echo "::error::${crate} depends on ${oxc}; only skif-js-syntax, skif-js-plugins and skif-js may"
      failures=$((failures + 1))
    fi
  done < <(awk '$1 ~ /^skif/ {print $1}' <<<"$tree" | sort -u)
done

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} dependency layering problem(s)" >&2
  exit 1
fi
echo "the dependency layering holds"
