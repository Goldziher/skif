#!/usr/bin/env bash
# Fails when a Linux binary needs a newer glibc than the floor the release promises.
#
# Usage: scripts/check-glibc-floor.sh <binary> [floor]     (floor defaults to the one in scripts/lib/libc.sh)
#
# Reads the versioned glibc symbols the binary imports (`objdump -T`) and takes the highest GLIBC_x.y. A binary built
# against the runner's glibc instead of the cargo-zigbuild sysroot fails here, in the build job, rather than on a
# user's older distribution.
set -euo pipefail

# shellcheck source=lib/libc.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/libc.sh"

binary="${1:?usage: check-glibc-floor.sh <binary> [floor]}"
floor="${2:-$SKIF_GLIBC_FLOOR}"

[[ -f "$binary" ]] || {
  echo "::error::no such binary: ${binary}"
  exit 1
}

highest=""
while IFS= read -r version; do
  if [[ -z "$highest" ]] || ! skif_version_ge "$highest" "$version"; then
    highest="$version"
  fi
done < <(objdump -T "$binary" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sed 's/^GLIBC_//' | sort -u)

if [[ -z "$highest" ]]; then
  echo "::error::${binary} imports no versioned glibc symbols; is it a glibc binary?"
  exit 1
fi
if ! skif_version_ge "$floor" "$highest"; then
  echo "::error::${binary} needs glibc ${highest}, newer than the promised floor ${floor}"
  exit 1
fi
echo "${binary}: needs glibc ${highest} at most (floor ${floor})"
