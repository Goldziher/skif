#!/usr/bin/env bash
# Installs the packed npm tarballs of a release into a fresh project and runs `skif --version` the way a user does,
# before anything is published.
#
# Usage: scripts/npm-install-smoke.sh <directory-of-tarballs> <version> <platform> <npm|bun> <project-directory>
#
# <platform> is the napi platform of the host (e.g. win32-x64-msvc). The project depends on the umbrella tarball
# only; `overrides` point every platform package it lists as an optional dependency at its local tarball, so the
# package manager resolves the optionalDependencies itself, filters them by os and cpu, and installs no package from
# the registry. Then `npx skif` (npm, through the bin shim, `.cmd` on Windows) or `bunx skif` (bun) must print
# <version>, the host's platform package must hold its addon, and no other platform package may be installed.
set -euo pipefail

usage="usage: npm-install-smoke.sh <directory-of-tarballs> <version> <platform> <npm|bun> <project-directory>"
tarballs="${1:?$usage}"
version="${2:?$usage}"
platform="${3:?$usage}"
manager="${4:?$usage}"
project="${5:?$usage}"

case "$manager" in
npm) run_bin=(npx --no-install skif --version) ;;
bun) run_bin=(bunx --bun --no-install skif --version) ;;
*)
  echo "error: the package manager must be npm or bun (got '${manager}')" >&2
  exit 2
  ;;
esac

umbrella="skif-cli-${version}.tgz"
for tarball in "$umbrella" "skif-${platform}-${version}.tgz"; do
  [[ -f "${tarballs}/${tarball}" ]] || {
    echo "::error::${tarballs} has no ${tarball}" >&2
    exit 1
  }
done

mkdir -p "${project}/tarballs"
cp "${tarballs}"/*.tgz "${project}/tarballs/"
cd "$project"

# The optional dependencies the umbrella declares, each overridden with its tarball.
optional="$(tar -xzOf "tarballs/${umbrella}" package/package.json | jq -c '.optionalDependencies // {} | keys')"
overrides="$(jq -n --argjson names "$optional" --arg version "$version" \
  '[$names[] | {key: ., value: "file:./tarballs/\(.)-\($version).tgz"}] | from_entries')"
for name in $(jq -r '.[]' <<<"$optional"); do
  [[ -f "tarballs/${name}-${version}.tgz" ]] || {
    echo "::error::the umbrella lists ${name}, but ${tarballs} has no ${name}-${version}.tgz" >&2
    exit 1
  }
done
jq -n --arg umbrella "file:./tarballs/${umbrella}" --argjson overrides "$overrides" \
  '{name: "skif-install-smoke", version: "0.0.0", private: true,
    dependencies: {"skif-cli": $umbrella}, overrides: $overrides}' >package.json

echo "::group::${manager} install"
case "$manager" in
npm) npm install --no-audit --no-fund ;;
bun) bun install ;;
esac
echo "::endgroup::"

failures=0
addon="node_modules/skif-${platform}/skif.${platform}.node"
[[ -f "$addon" ]] || {
  echo "::error::${manager} did not install ${addon} from the umbrella's optional dependencies" >&2
  failures=$((failures + 1))
}
for name in $(jq -r '.[]' <<<"$optional"); do
  if [[ "$name" != "skif-${platform}" && -e "node_modules/${name}" ]]; then
    echo "::error::${manager} installed ${name}, which is not for ${platform}" >&2
    failures=$((failures + 1))
  fi
done

status=0
out="$("${run_bin[@]}" 2>&1)" || status=$?
echo "${run_bin[*]}: ${out}"
if [[ "$status" -ne 0 ]] || ! grep -qF "$version" <<<"$out"; then
  echo "::error::${run_bin[*]} exited ${status} and did not print ${version}" >&2
  failures=$((failures + 1))
fi

[[ "$failures" -eq 0 ]] || exit 1
echo "npm install smoke (${manager}, ${platform}): skif ${version} runs"
