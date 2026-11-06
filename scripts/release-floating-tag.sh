#!/usr/bin/env bash
# Moves the floating version tag to a release tag.
#
# A stable major N floats as `vN`. A 0.x minor may break, so a 0.x release floats as `v0.M` and `v0` never moves.
#
# Usage: scripts/release-floating-tag.sh <version> <tag>
# Environment:
#   DRY_RUN   "true" prints the tag that would move and changes nothing (no tag, no push)
#   REMOTE    remote the tag is pushed to (default origin)
set -euo pipefail

version="${1:?usage: release-floating-tag.sh <version> <tag>}"
tag="${2:?usage: release-floating-tag.sh <version> <tag>}"
dry_run="${DRY_RUN:-false}"
remote="${REMOTE:-origin}"

case "$version" in
*-*)
  echo "release-floating-tag: '${version}' is a pre-release and has no floating tag" >&2
  exit 2
  ;;
[0-9]*.[0-9]*.[0-9]*) ;;
*)
  echo "release-floating-tag: '${version}' is not a MAJOR.MINOR.PATCH version" >&2
  exit 2
  ;;
esac

if [[ "${version%%.*}" == "0" ]]; then
  floating="v${version%.*}"
else
  floating="v${version%%.*}"
fi

sha="$(git rev-list -n1 "$tag")"
if [[ "$dry_run" == "true" ]]; then
  echo "Would move ${floating} to ${tag} (${sha})"
  exit 0
fi

git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"
git -c tag.gpgsign=false tag -f "$floating" "$sha"
git push -f "$remote" "refs/tags/${floating}"
echo "Moved ${floating} to ${tag} (${sha})"
