#!/usr/bin/env bash
# Refuses to release a tag that is not on main or whose CHANGELOG.md has no section for its version.
#
# Usage: scripts/check-release-gate.sh <tag>
#
# Environment:
#   MAIN_REF      the ref the tag must be reachable from (default origin/main)
#
# Publishing to crates.io and npm cannot be undone, so a tag on a feature branch must stop before anything is
# uploaded. A green CI run is not required: whether to tag over a red or unfinished run is the releaser's call.
set -euo pipefail

tag="${1:?usage: check-release-gate.sh <tag>}"
main_ref="${MAIN_REF:-origin/main}"

sha="$(git rev-list -n 1 "$tag")"
if ! git merge-base --is-ancestor "$sha" "$main_ref"; then
  echo "::error::${tag} (${sha}) is not an ancestor of ${main_ref}; release only commits that are on main" >&2
  exit 1
fi

# The release notes are the CHANGELOG.md section of the version, so the tagged commit must have one.
version="${tag#v}"
changelog="$(git show "${sha}:CHANGELOG.md" 2>/dev/null || true)"
if [[ $'\n'"$changelog" != *$'\n'"## [${version}]"* ]]; then
  echo "::error::CHANGELOG.md at ${tag} has no '## [${version}]' section; move the Unreleased notes under it" >&2
  exit 1
fi
echo "release gate: ${tag} (${sha}) is on ${main_ref} and has a CHANGELOG.md section"
