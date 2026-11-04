#!/usr/bin/env bash
# Refuses to release a tag that is not on main or whose commit has no green CI run.
#
# Usage: scripts/check-release-gate.sh <tag>
#
# Environment:
#   MAIN_REF      the ref the tag must be reachable from (default origin/main)
#   CI_WORKFLOW   workflow file whose successful run is required (default ci.yml)
#   GH_REPO, GH_TOKEN   consumed by `gh run list`
#
# Publishing to crates.io and npm cannot be undone, so a tag on a feature branch, or on a commit whose CI failed
# or never ran, must stop before anything is uploaded.
set -euo pipefail

tag="${1:?usage: check-release-gate.sh <tag>}"
main_ref="${MAIN_REF:-origin/main}"
workflow="${CI_WORKFLOW:-ci.yml}"

sha="$(git rev-list -n 1 "$tag")"
if ! git merge-base --is-ancestor "$sha" "$main_ref"; then
  echo "::error::${tag} (${sha}) is not an ancestor of ${main_ref}; release only commits that are on main" >&2
  exit 1
fi

green="$(gh run list --workflow "$workflow" --commit "$sha" --status success --json databaseId --jq 'length')"
if [[ "${green:-0}" -lt 1 ]]; then
  echo "::error::no successful ${workflow} run for ${tag} (${sha}); wait for CI to pass on this commit" >&2
  exit 1
fi
echo "release gate: ${tag} (${sha}) is on ${main_ref} and ${workflow} passed"
