#!/usr/bin/env bash
# Checks that release archives ship the binary together with the notices their licences require.
#
# Usage: scripts/check-archive.sh <archive>...
#
# Every skif-<target>.tar.gz or .zip must hold the binary (skif, or skif.exe for Windows), LICENSE and
# ATTRIBUTIONS.md at its top level, and ATTRIBUTIONS.md must hold the tsrs section the checker's NOTICE requires
# (ADR 0045). The archives are listed and read to stdout, never extracted to disk.
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

# read_member <archive> <member>: prints the member's content.
read_member() {
  case "$1" in
  *.tar.gz) tar -xzOf "$1" "$2" ;;
  *.zip)
    if command -v unzip >/dev/null 2>&1; then
      unzip -p "$1" "$2"
    else
      tar -xOf "$1" "$2"
    fi
    ;;
  esac
}

tsrs_section="## tsrs (and TypeScript)"
failures=0
for archive in "$@"; do
  listed="$(list_members "$archive")"
  mapfile -t names <<<"$listed"
  members="$(printf '%s\n' "${names[@]#./}")"
  binary=skif
  [[ "$archive" == *windows* ]] && binary=skif.exe
  for member in "$binary" LICENSE ATTRIBUTIONS.md; do
    if ! grep -qxF "$member" <<<"$members"; then
      echo "::error::$(basename "$archive") does not contain ${member}" >&2
      failures=$((failures + 1))
    fi
  done
  attributions="$(grep -xE '(\./)?ATTRIBUTIONS\.md' <<<"$listed" || true)"
  [[ -n "$attributions" ]] || continue
  # Not `grep -q`: it stops at the first match, and the reader fails on the closed pipe, which pipefail reports as a miss.
  if ! read_member "$archive" "${attributions%%$'\n'*}" | grep -xF "$tsrs_section" >/dev/null; then
    echo "::error::ATTRIBUTIONS.md in $(basename "$archive") lacks the '${tsrs_section}' section" >&2
    failures=$((failures + 1))
  fi
done

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} problem(s) in the release archives" >&2
  exit 1
fi
echo "release archives: binary, LICENSE and ATTRIBUTIONS.md (with the tsrs section) present in $# archive(s)"
