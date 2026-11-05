#!/usr/bin/env bash
# Runs inside a Linux container: checks that the installers' libc rule picks the expected flavour for this
# distribution, then runs the binary of that flavour.
#
# Usage: scripts/check-libc-selection.sh <binary-directory> <expected: gnu|musl>
#
# <binary-directory> holds one unpacked release binary per flavour, as <dir>/gnu/skif and <dir>/musl/skif, for this
# machine's architecture. A glibc 2.31 host must get musl, a glibc 2.34 or newer host must get gnu, and the chosen
# binary must start and print its version. Needs only bash, so it runs on every base image.
set -euo pipefail

# shellcheck source=lib/libc.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/libc.sh"

dir="${1:?usage: check-libc-selection.sh <binary-directory> <expected: gnu|musl>}"
expected="${2:?usage: check-libc-selection.sh <binary-directory> <expected: gnu|musl>}"

chosen="$(skif_libc)"
echo "glibc: $(skif_glibc_version || true) -> ${chosen} (expected ${expected})"
if [[ "$chosen" != "$expected" ]]; then
  echo "::error::picked ${chosen}, expected ${expected}"
  exit 1
fi

"${dir}/${chosen}/skif" --version
