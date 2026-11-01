#!/usr/bin/env bash
# Prints the Scoop manifest for a skif release to stdout.
#
# Usage: scripts/scoop-update.sh <version> <checksums-file> [release-repo]
#
# The manifest lists the x64 and arm64 Windows archives, with hashes read from the release's own
# checksums.txt rather than recomputed. The bucket's autoupdate block follows the same asset names.
set -euo pipefail

if [ $# -lt 2 ] || [ $# -gt 3 ]; then
  echo "Usage: $0 <version> <checksums-file> [release-repo]" >&2
  exit 1
fi

version="$1"
sums_file="$2"
release_repo="${3:-Goldziher/skif}"
base_url="https://github.com/${release_repo}/releases/download"

command -v jq >/dev/null 2>&1 || {
  echo "error: jq is required" >&2
  exit 1
}
[ -f "$sums_file" ] || {
  echo "error: no such checksums file: ${sums_file}" >&2
  exit 1
}

hash_for() {
  local asset="$1" hash
  hash="$(awk -v asset="$asset" '{ name = $NF; sub(/^\*/, "", name); sub(/^\.\//, "", name); if (name == asset) print $1 }' "$sums_file" | head -1)"
  if [ -z "$hash" ]; then
    echo "error: ${sums_file} has no entry for ${asset}" >&2
    exit 1
  fi
  if ! [[ "$hash" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "error: not a sha256 for ${asset}: ${hash}" >&2
    exit 1
  fi
  printf '%s' "$hash"
}

x64_asset="skif-x86_64-pc-windows-msvc.zip"
arm64_asset="skif-aarch64-pc-windows-msvc.zip"
x64_hash="$(hash_for "$x64_asset")"
arm64_hash="$(hash_for "$arm64_asset")"

manifest="$(jq -n -a --indent 4 \
  --arg version "$version" \
  --arg base "$base_url" \
  --arg x64_asset "$x64_asset" \
  --arg arm64_asset "$arm64_asset" \
  --arg x64_hash "$x64_hash" \
  --arg arm64_hash "$arm64_hash" \
  --arg homepage "https://github.com/${release_repo}" '
  {
    version: $version,
    description: "Fast dependency linter for JavaScript and TypeScript monorepos.",
    homepage: $homepage,
    license: "MIT",
    architecture: {
      "64bit": {url: "\($base)/v\($version)/\($x64_asset)", hash: $x64_hash},
      arm64: {url: "\($base)/v\($version)/\($arm64_asset)", hash: $arm64_hash}
    },
    bin: "skif.exe",
    checkver: {github: $homepage},
    autoupdate: {
      architecture: {
        "64bit": {url: "\($base)/v$version/\($x64_asset)"},
        arm64: {url: "\($base)/v$version/\($arm64_asset)"}
      },
      hash: {url: "\($base)/v$version/checksums.txt"}
    }
  }')"

[ "$(printf '%s' "$manifest" | jq -r '.version')" = "$version" ] || {
  echo "error: version did not land in the manifest" >&2
  exit 1
}
[ "$(printf '%s' "$manifest" | jq -r '.architecture["64bit"].hash')" = "$x64_hash" ] || {
  echo "error: x64 hash did not land in the manifest" >&2
  exit 1
}
[ "$(printf '%s' "$manifest" | jq -r '.architecture.arm64.hash')" = "$arm64_hash" ] || {
  echo "error: arm64 hash did not land in the manifest" >&2
  exit 1
}

printf '%s\n' "$manifest"
