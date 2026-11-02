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

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} vendored file(s) lack an attribution header" >&2
  exit 1
fi
echo "every vendored file carries an attribution header"
