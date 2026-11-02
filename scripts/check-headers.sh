#!/usr/bin/env bash
# Fails unless every source file of the vendored skif-specifier crate starts with an attribution header:
# either `// Vendored from syncpack ...` or `// Added for skif (not in syncpack) ...` (ADR 0015).
#
# Usage: scripts/check-headers.sh
set -euo pipefail

cd "$(dirname "$0")/.."

failures=0
for file in crates/skif-specifier/src/*.rs; do
  first="$(head -n 1 "$file")"
  case "$first" in
  "// Vendored from syncpack "* | "// Added for skif (not in syncpack)"*) ;;
  *)
    echo "::error file=${file}::missing attribution header (expected '// Vendored from syncpack ...' or '// Added for skif (not in syncpack) ...')"
    failures=$((failures + 1))
    ;;
  esac
done

# Every header names the syncpack commit that ATTRIBUTIONS.md records, and ATTRIBUTIONS.md keeps the sections
# and the Modifications lists the licences require.
# shellcheck disable=SC2016
recorded="$(sed -nE 's/^- \*\*Upstream commit\*\*: `([0-9a-f]{40})`.*/\1/p' ATTRIBUTIONS.md | sed -n '2p')"
for file in crates/skif-specifier/src/*.rs; do
  commit="$(head -n 1 "$file" | sed -nE 's/^\/\/ Vendored from syncpack [^ ]+ @([0-9a-f]+) .*/\1/p')"
  if [[ -n "$commit" && "$recorded" != "$commit"* ]]; then
    echo "::error file=${file}::header commit ${commit} differs from the syncpack commit recorded in ATTRIBUTIONS.md (${recorded:-none})"
    failures=$((failures + 1))
  fi
done
for section in "## sherif" "## syncpack specifier" "## knip"; do
  if ! grep -qx "$section" ATTRIBUTIONS.md; then
    echo "::error file=ATTRIBUTIONS.md::missing section '${section}'"
    failures=$((failures + 1))
  fi
done
if [[ "$(grep -c '^### Modifications$' ATTRIBUTIONS.md)" -lt 3 ]]; then
  echo "::error file=ATTRIBUTIONS.md::every upstream section needs a '### Modifications' list"
  failures=$((failures + 1))
fi

# The knip section records the commit the JS analysis follows and the licence holder (ADR 0015).
knip_section="$(sed -n '/^## knip$/,$p' ATTRIBUTIONS.md)"
for needle in "da07b6e9887f9a1eb08545da29755b7f9d309bca" "Copyright (c) 2022-2026 Lars Kappert" "ISC License"; do
  if ! grep -qF "$needle" <<<"$knip_section"; then
    echo "::error file=ATTRIBUTIONS.md::the knip section is missing '${needle}'"
    failures=$((failures + 1))
  fi
done

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} attribution problem(s)" >&2
  exit 1
fi
echo "every vendored file carries an attribution header"
