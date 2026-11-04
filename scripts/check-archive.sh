#!/usr/bin/env bash
# Checks that release archives ship the binary together with the notices their licences require.
#
# Usage: scripts/check-archive.sh <archive>...
#
# Every skif-<target>.tar.gz or .zip must hold the binary (skif, or skif.exe for Windows), LICENSE and
# ATTRIBUTIONS.md at its top level. The archives are listed, never extracted.
set -euo pipefail

[[ $# -gt 0 ]] || {
  echo "usage: check-archive.sh <archive>..." >&2
  exit 2
}

list_members() {
  case "$1" in
  *.tar.gz) tar -tzf "$1" ;;
  *.zip)
    if command -v unzip >/dev/null 2>&1; then
      unzip -Z1 "$1"
    else
      tar -tf "$1"
    fi
    ;;
  *)
    echo "::error::unsupported archive type: $1" >&2
    return 1
    ;;
  esac
}

failures=0
for archive in "$@"; do
  members="$(list_members "$archive" | sed 's|^\./||')"
  binary=skif
  [[ "$archive" == *windows* ]] && binary=skif.exe
  for member in "$binary" LICENSE ATTRIBUTIONS.md; do
    if ! grep -qxF "$member" <<<"$members"; then
      echo "::error::$(basename "$archive") does not contain ${member}" >&2
      failures=$((failures + 1))
    fi
  done
done

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} problem(s) in the release archives" >&2
  exit 1
fi
echo "release archives: binary, LICENSE and ATTRIBUTIONS.md present in $# archive(s)"
