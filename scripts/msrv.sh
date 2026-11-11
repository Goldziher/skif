#!/usr/bin/env bash
# Prints the `rust-version` a Cargo manifest declares in its [workspace.package] or [package] table: the MSRV the CI
# job checks and the toolchain the bench workflow builds each checkout with. Fails when it declares none.
#
# Usage: scripts/msrv.sh [path/to/Cargo.toml]   (default: the Cargo.toml of the current directory)
set -euo pipefail

manifest="${1:-Cargo.toml}"
if [[ ! -f "$manifest" ]]; then
  echo "msrv: ${manifest} does not exist" >&2
  exit 1
fi
version="$(awk '/^\[(workspace\.)?package\]/{p=1;next} /^\[/{p=0} p && /^rust-version *= *"/{gsub(/"/,"",$3);print $3;exit}' "$manifest")"
if [[ -z "$version" ]]; then
  echo "msrv: ${manifest} declares no rust-version" >&2
  exit 1
fi
echo "$version"
