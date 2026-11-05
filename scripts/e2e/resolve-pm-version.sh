#!/usr/bin/env bash
# Prints the newest stable release of the latest or the second-latest major of a package manager, so the e2e matrix
# tests what ADR 0027 promises (the latest two majors of npm, pnpm and Yarn 2+) without a version list to keep current.
#
# Usage: scripts/e2e/resolve-pm-version.sh <npm|pnpm|yarn> <newest|previous>
#
# Reads the registry with `npm view`. Yarn 2+ is published as @yarnpkg/cli-dist; the `yarn` package is Classic 1.x.
# Pre-releases are ignored, so a major that only has release candidates does not count yet.
set -euo pipefail

pm="${1:?usage: resolve-pm-version.sh <npm|pnpm|yarn> <newest|previous>}"
slot="${2:?usage: resolve-pm-version.sh <npm|pnpm|yarn> <newest|previous>}"

case "$pm" in
npm | pnpm) package="$pm" ;;
yarn) package="@yarnpkg/cli-dist" ;;
*)
  echo "unsupported package manager: ${pm}" >&2
  exit 2
  ;;
esac
case "$slot" in
newest) back=0 ;;
previous) back=1 ;;
*)
  echo "unknown slot '${slot}': use newest or previous" >&2
  exit 2
  ;;
esac

version="$(
  npm view "$package" versions --json | jq -r --argjson back "$back" '
    def parts: split(".") | map(tonumber);
    [.[] | select(test("^[0-9]+\\.[0-9]+\\.[0-9]+$"))] as $stable
    | ($stable | map(parts[0]) | unique) as $majors
    | $majors[-1 - $back] as $major
    | if $major == null then empty
      else [$stable[] | select(parts[0] == $major)] | sort_by(parts) | last end'
)"

if [[ -z "$version" ]]; then
  echo "no ${slot} major of ${package} found in the registry" >&2
  exit 1
fi
echo "$version"
