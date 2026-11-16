# shellcheck shell=bash
# Build-attestation identities of skif releases, shared by the pre-commit launcher and the GitHub Action.
#
# The repository moved from Goldziher/skif to polylint/skif after v0.6.0. A release built before the move carries an
# attestation whose source repository and signer workflow name Goldziher/skif, so it fails a check pinned to
# polylint/skif. A polylint/skif release up to v0.6.0 is therefore also accepted when Goldziher/skif attested it;
# every later release, and every other repository (a mirror or a fork), is checked against its own identity only.

SKIF_ATTESTATION_REPOSITORY="polylint/skif"
SKIF_PRE_TRANSFER_REPOSITORY="Goldziher/skif"

# skif_pre_transfer_release <tag>: succeeds when the tag is v0.6.0 or older. A pre-release counts as its release, so
# v0.6.1-rc.1 is not pre-transfer.
skif_pre_transfer_release() {
  local version="${1#v}" major minor patch
  version="${version%%-*}"
  IFS=. read -r major minor patch <<<"$version"
  patch="${patch:-0}"
  [[ "$major" =~ ^[0-9]+$ && "$minor" =~ ^[0-9]+$ && "$patch" =~ ^[0-9]+$ ]] || return 1
  ((10#$major == 0 && (10#$minor < 6 || (10#$minor == 6 && 10#$patch == 0))))
}

# skif_attestation_repos <repo> <tag>: prints, one per line, the repositories whose release workflow may have attested
# the release, in the order they are tried.
skif_attestation_repos() {
  local release_repo="$1" tag="$2"
  printf '%s\n' "$release_repo"
  if [[ "$release_repo" == "$SKIF_ATTESTATION_REPOSITORY" ]] && skif_pre_transfer_release "$tag"; then
    printf '%s\n' "$SKIF_PRE_TRANSFER_REPOSITORY"
  fi
}

# skif_verify_attestation <archive> <repo> <tag>: runs `gh attestation verify` against each identity until one passes
# and prints the repository that attested the archive. gh's own output goes to stderr. Fails when none passes.
skif_verify_attestation() {
  local archive="$1" release_repo="$2" tag="$3" candidate candidates
  candidates="$(skif_attestation_repos "$release_repo" "$tag")"
  while IFS= read -r candidate; do
    if gh attestation verify "$archive" --repo "$candidate" \
      --signer-workflow "${candidate}/.github/workflows/publish.yaml" >&2; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done <<<"$candidates"
  return 1
}
