#!/usr/bin/env bash
# Checks the packed npm tarballs of a release before anything is published.
#
# Usage: scripts/check-npm-tarballs.sh <directory-of-tarballs> <version>
#
# The directory must hold exactly the umbrella `skif-cli` and the eight platform packages, no more and no fewer. The
# umbrella must ship the generated loader (index.js, index.d.ts) and the bin, and list the eight platform packages
# as optional dependencies; each platform package must ship its .node addon. This is what turns a missing
# artifact (the loader is generated, not committed) into a failed job instead of a broken release. Every package
# also carries LICENSE and ATTRIBUTIONS.md, the notices the MIT and ISC code inside the addon requires.
set -euo pipefail

dir="${1:?usage: check-npm-tarballs.sh <directory-of-tarballs> <version>}"
version="${2:?usage: check-npm-tarballs.sh <directory-of-tarballs> <version>}"

platforms=(
  darwin-arm64
  darwin-x64
  linux-arm64-gnu
  linux-arm64-musl
  linux-x64-gnu
  linux-x64-musl
  win32-arm64-msvc
  win32-x64-msvc
)

failures=0
fail() {
  echo "::error::$*" >&2
  failures=$((failures + 1))
}

expected_files="$(
  {
    echo "skif-cli-${version}.tgz"
    for platform in "${platforms[@]}"; do echo "skif-${platform}-${version}.tgz"; done
  } | LC_ALL=C sort
)"
# `find -printf` is GNU only; strip the leading "./" instead so the check also runs on macOS.
actual_files="$(cd "$dir" && find . -maxdepth 1 -type f -name '*.tgz' 2>/dev/null | sed 's|^\./||' | LC_ALL=C sort || true)"
if [[ "$expected_files" != "$actual_files" ]]; then
  fail "the packed tarballs are not the umbrella plus the eight platform packages"
  diff <(echo "$expected_files") <(echo "$actual_files") >&2 || true
fi

listing() {
  tar -tzf "${dir}/$1"
}

require() {
  local tarball="$1" member="$2"
  # Not `grep -q`: it stops at the first match, and tar fails on the closed pipe, which pipefail reports as a miss.
  if ! listing "$tarball" | grep -xF "package/${member}" >/dev/null; then
    fail "${tarball} does not contain ${member}"
  fi
}

if [[ -f "${dir}/skif-cli-${version}.tgz" ]]; then
  for member in index.js index.d.ts bin/skif.js package.json LICENSE ATTRIBUTIONS.md README.md; do
    require "skif-cli-${version}.tgz" "$member"
  done
  declared="$(tar -xzOf "${dir}/skif-cli-${version}.tgz" package/package.json |
    jq -r '(.optionalDependencies // {}) | to_entries[] | "\(.key)@\(.value)"' | LC_ALL=C sort)"
  wanted="$(for platform in "${platforms[@]}"; do echo "skif-${platform}@${version}"; done | LC_ALL=C sort)"
  if [[ "$declared" != "$wanted" ]]; then
    fail "the umbrella's optionalDependencies are not the eight platform packages at ${version}"
    diff <(echo "$wanted") <(echo "$declared") >&2 || true
  fi
fi

for platform in "${platforms[@]}"; do
  tarball="skif-${platform}-${version}.tgz"
  [[ -f "${dir}/${tarball}" ]] || continue
  require "$tarball" "skif.${platform}.node"
  require "$tarball" "package.json"
  require "$tarball" "LICENSE"
  require "$tarball" "ATTRIBUTIONS.md"
done

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} problem(s) in the npm tarballs" >&2
  exit 1
fi
echo "npm tarballs: the umbrella and the eight platform packages are complete"
