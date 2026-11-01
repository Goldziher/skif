#!/usr/bin/env bash
# Stages the prebuilt release binaries into the eight npm platform packages.
#
#   npm-package/stage-binaries.sh --artifacts <dir>   # the release archives, as published
#   npm-package/stage-binaries.sh --local <binary>    # one locally built skif, host platform only
#
# The archives are the published ones, the same files checksums.txt covers; rebuilding a binary
# here would ship bytes the release checksums do not describe.
set -euo pipefail

package_dir="$(cd "$(dirname "$0")" && pwd)"
invocation_dir="$PWD"

targets=(
  "darwin-arm64:aarch64-apple-darwin"
  "darwin-x64:x86_64-apple-darwin"
  "linux-arm64-gnu:aarch64-unknown-linux-gnu"
  "linux-arm64-musl:aarch64-unknown-linux-musl"
  "linux-x64-gnu:x86_64-unknown-linux-gnu"
  "linux-x64-musl:x86_64-unknown-linux-musl"
  "win32-arm64:aarch64-pc-windows-msvc"
  "win32-x64:x86_64-pc-windows-msvc"
)

usage() {
  echo "usage: $0 --artifacts <dir> | --local <path-to-skif>" >&2
  exit 2
}

mode="${1:-}"
arg="${2:-}"
[[ "$mode" == "--artifacts" || "$mode" == "--local" ]] || usage
[[ -n "$arg" ]] || usage

[[ "$arg" == /* ]] || arg="${invocation_dir}/${arg}"

stage_from_archive() {
  local name="$1" triple="$2" archive_dir="$3" dest archive
  dest="${package_dir}/platforms/${name}/bin"
  rm -rf "$dest"
  mkdir -p "$dest"

  if [[ "$triple" == *windows* ]]; then
    archive="${archive_dir}/skif-${triple}.zip"
    [[ -f "$archive" ]] || { echo "error: missing archive ${archive}" >&2; return 1; }
    unzip -q -o "$archive" skif.exe -d "$dest"
    [[ -f "${dest}/skif.exe" ]] || { echo "error: ${archive} contained no skif.exe" >&2; return 1; }
  else
    archive="${archive_dir}/skif-${triple}.tar.gz"
    [[ -f "$archive" ]] || { echo "error: missing archive ${archive}" >&2; return 1; }
    tar xzf "$archive" -C "$dest" skif
    [[ -x "${dest}/skif" ]] || { echo "error: ${archive} contained no executable skif" >&2; return 1; }
  fi
  echo "  staged ${name} from skif-${triple}"
}

if [[ "$mode" == "--artifacts" ]]; then
  [[ -d "$arg" ]] || { echo "error: not a directory: ${arg}" >&2; exit 1; }
  arg="$(cd "$arg" && pwd)"
  echo "Staging skif binaries from ${arg}"
  for target in "${targets[@]}"; do
    stage_from_archive "${target%%:*}" "${target##*:}" "$arg"
  done
  exit 0
fi

[[ -x "$arg" ]] || { echo "error: not an executable: ${arg}" >&2; exit 1; }
case "$(uname -s)/$(uname -m)" in
Darwin/arm64) name="darwin-arm64" ;;
Darwin/x86_64) name="darwin-x64" ;;
Linux/aarch64) name="linux-arm64-gnu" ;;
Linux/x86_64) name="linux-x64-gnu" ;;
*)
  echo "error: --local has no platform package for $(uname -s)/$(uname -m)" >&2
  exit 1
  ;;
esac
dest="${package_dir}/platforms/${name}/bin"
rm -rf "$dest"
mkdir -p "$dest"
cp "$arg" "${dest}/skif"
chmod +x "${dest}/skif"
echo "  staged ${name} from ${arg} (local build, host platform only)"
