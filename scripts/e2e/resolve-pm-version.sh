#!/usr/bin/env bash
# Prints the newest stable release of the latest or the second-latest major of a package manager, so the e2e matrix
# tests what ADR 0027 promises (the latest two majors of npm, pnpm and Yarn 2+) without a version list to keep current.
#
# Usage: scripts/e2e/resolve-pm-version.sh <npm|pnpm|yarn|bun> <newest|previous> [major]
#
# With a `major`, only releases of that major are considered, so `bun newest 1` is the newest Bun 1.x and keeps
# working when Bun 2 ships (ADR 0027 supports Bun 1.x only).
#
# Reads the registry with `npm view`. Yarn 2+ is published as @yarnpkg/cli-dist; the `yarn` package is Classic 1.x.
# Pre-releases are ignored, so a major that only has release candidates does not count yet.
set -euo pipefail

pm="${1:?usage: resolve-pm-version.sh <npm|pnpm|yarn|bun> <newest|previous> [major]}"
slot="${2:?usage: resolve-pm-version.sh <npm|pnpm|yarn|bun> <newest|previous> [major]}"
major="${3:-}"

case "$pm" in
npm | pnpm | bun) package="$pm" ;;
yarn) package="@yarnpkg/cli-dist" ;;
*)
  echo "unsupported package manager: ${pm}" >&2
  exit 2
  ;;
esac
if [[ -n "$major" && ! "$major" =~ ^[0-9]+$ ]]; then
  echo "major must be a number (got '${major}')" >&2
  exit 2
fi
case "$slot" in
newest) back=0 ;;
previous) back=1 ;;
*)
  echo "unknown slot '${slot}': use newest or previous" >&2
  exit 2
  ;;
esac

version="$(
  npm view "$package" versions --json | jq -r --argjson back "$back" --arg want "$major" '
    def parts: split(".") | map(tonumber);
    [.[] | select(test("^[0-9]+\\.[0-9]+\\.[0-9]+$")) | select($want == "" or parts[0] == ($want | tonumber))] as $stable
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
