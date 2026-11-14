#!/usr/bin/env bash
# Fails unless the dependency layering of ADR 0002, ADR 0013 and ADR 0044 holds:
#   - `skif` without default features pulls in no oxc crate and no tsrs crate;
#   - `skif-core` depends on no other workspace crate;
#   - oxc crates are reachable only through skif-js-syntax, skif-js-plugins and skif-js;
#   - tsrs crates are reachable only through skif-js-types.
#
# Usage: scripts/check-dependency-tree.sh
set -euo pipefail

cd "$(dirname "$0")/.."

failures=0
errors="$(mktemp)"
trap 'rm -f "$errors"' EXIT

facade="$(cargo tree -p skif --no-default-features -e normal --prefix none --locked)"
if grep '^oxc_' >/dev/null <<<"$facade"; then
  echo "::error::skif without default features depends on an oxc crate"
  failures=$((failures + 1))
fi
if grep '^tsrs_' >/dev/null <<<"$facade"; then
  echo "::error::skif without default features depends on a tsrs crate"
  failures=$((failures + 1))
fi

while IFS= read -r crate; do
  echo "::error::skif-core depends on the workspace crate ${crate}"
  failures=$((failures + 1))
done < <(cargo tree -p skif-core -e normal --prefix none --locked | awk '$1 ~ /^skif/ && $1 != "skif-core" {print $1}' | sort -u)

# only_through <allowed crates> <allowed, as prose> <dependency...>: fails for every workspace crate outside the allowed
# ones (space-separated) that depends directly on one of the dependencies.
only_through() {
  local allowed=" $1 " prose="$2" dependency crate tree
  shift 2
  for dependency in "$@"; do
    if ! tree="$(cargo tree -e normal --invert "$dependency" --depth 1 --prefix none --locked 2>"$errors")"; then
      if grep -q 'did not match any packages' "$errors"; then
        echo "::warning::${dependency} is not in the dependency graph; nothing to check for it"
        continue
      fi
      echo "::error::cargo tree failed for ${dependency}: $(cat "$errors")"
      failures=$((failures + 1))
      continue
    fi
    while IFS= read -r crate; do
      if [[ "$allowed" != *" ${crate} "* ]]; then
        echo "::error::${crate} depends on ${dependency}; only ${prose} may"
        failures=$((failures + 1))
      fi
    done < <(awk '$1 ~ /^skif/ {print $1}' <<<"$tree" | sort -u)
  done
}

only_through "skif-js-syntax skif-js-plugins skif-js" "skif-js-syntax, skif-js-plugins and skif-js" \
  oxc_allocator oxc_ast oxc_ast_visit oxc_parser oxc_semantic oxc_span oxc_str oxc_resolver
only_through "skif-js-types" "skif-js-types" \
  tsrs_ast tsrs_checker tsrs_compiler tsrs_core tsrs_scanner tsrs_tsoptions tsrs_vfs

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} dependency layering problem(s)" >&2
  exit 1
fi
echo "the dependency layering holds"
