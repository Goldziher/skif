#!/usr/bin/env bash
# Checks, before a release is created, that every npm package the release publishes already exists on the registry.
#
# Usage: scripts/check-npm-packages-exist.sh [path/to/package.json]   (default: npm-package/package.json)
#
# npm trusted publishing can only be configured on a package that exists, so a package that was never published by
# hand fails its first OIDC publish half way through a release, after crates.io and the earlier npm packages are live.
# The names are the umbrella's `name` plus the platform package napi derives from each `napi.targets` triple
# (`<napi.packageName>-<platform>-<arch>[-<abi>]`, e.g. `skif-win32-x64-msvc`).
#
# Exit 0 when all exist, 1 when any is missing (all are listed), 2 when the registry gave no definite answer or a
# target is not one napi names. NPM_PROBE_ATTEMPTS (default 5) and NPM_PROBE_DELAY seconds (default 10) tune the
# retries of an indefinite answer (a rate limit, a 5xx, a network failure).
set -euo pipefail

manifest="${1:-npm-package/package.json}"
attempts="${NPM_PROBE_ATTEMPTS:-5}"
delay="${NPM_PROBE_DELAY:-10}"

# The platform suffix napi gives a Rust target triple.
platform_of() {
  local triple="$1" arch os abi=""
  case "$triple" in
  x86_64-*) arch=x64 ;;
  aarch64-*) arch=arm64 ;;
  i686-*) arch=ia32 ;;
  armv7-*) arch=arm ;;
  *) return 1 ;;
  esac
  case "$triple" in
  *-apple-darwin) os=darwin ;;
  *-linux-gnu) os=linux abi=gnu ;;
  *-linux-musl) os=linux abi=musl ;;
  *-linux-gnueabihf) os=linux abi=gnueabihf ;;
  *-linux-musleabihf) os=linux abi=musleabihf ;;
  *-windows-msvc) os=win32 abi=msvc ;;
  *-windows-gnu) os=win32 abi=gnu ;;
  *-freebsd) os=freebsd ;;
  *) return 1 ;;
  esac
  echo "${os}-${arch}${abi:+-${abi}}"
}

umbrella="$(jq -er '.name' "$manifest")"
prefix="$(jq -er '.napi.packageName // .napi.binaryName' "$manifest")"
names=("$umbrella")
while IFS= read -r triple; do
  platform="$(platform_of "$triple")" || {
    echo "::error::napi target ${triple} has no known npm platform name" >&2
    exit 2
  }
  names+=("${prefix}-${platform}")
done < <(jq -er '.napi.targets[]' "$manifest")

# Prints `exists` or `missing` (a definite 404); exits 2 when the registry did not answer.
probe() {
  local name="$1" attempt status output
  for ((attempt = 1; attempt <= attempts; attempt++)); do
    status=0
    output="$(npm view "$name" name 2>&1)" || status=$?
    if [[ "$status" -eq 0 && "$output" == *"$name"* ]]; then
      echo exists
      return 0
    fi
    if [[ "$output" == *E404* ]]; then
      echo missing
      return 0
    fi
    echo "attempt ${attempt}/${attempts}: npm view ${name} name failed: ${output}" >&2
    [[ "$attempt" -ge "$attempts" ]] || sleep "$delay"
  done
  echo "::error::could not tell whether the npm package ${name} exists" >&2
  return 2
}

missing=()
for name in "${names[@]}"; do
  state="$(probe "$name")" || exit 2
  echo "${name}: ${state}"
  [[ "$state" == exists ]] || missing+=("$name")
done

if [[ "${#missing[@]}" -ne 0 ]]; then
  for name in "${missing[@]}"; do
    echo "::error::npm package ${name} does not exist: publish a placeholder by hand and add the trusted publisher before releasing" >&2
  done
  exit 1
fi
echo "npm packages: all ${#names[@]} exist"
