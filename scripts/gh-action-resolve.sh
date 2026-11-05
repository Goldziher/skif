#!/usr/bin/env bash
# Resolves the release tag and the release asset for this runner, and writes both - plus the
# directory the binary will be cached under — to $GITHUB_OUTPUT. Split out from install so the
# install directory is known before the actions/cache step that wraps it.
set -euo pipefail

# shellcheck source=lib/libc.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/libc.sh"

repo="${REPOSITORY:-Goldziher/skif}"

# Resolve a floating `latest`, major-only `v3` or minor `v0.2` reference to the newest matching stable release.
# A floating tag carries no release assets of its own, so the assets always come from the newest full `vX.Y.Z`
# release in that series. The releases API is newest-first; drafts, pre-releases and the floating tag itself are
# skipped. Releases tag `vN` for a stable major N >= 1 and `v0.M` for a 0.x line (a 0.x minor may break, so `v0`
# is not moved).
newest_release() {
  local series="${1:-}" pattern
  if [ -n "$series" ]; then
    case "$series" in
    *.*) pattern="^v${series//./\\.}\\.[0-9]+$" ;;
    *) pattern="^v${series}\\.[0-9]+\\.[0-9]+$" ;;
    esac
  else
    pattern='^v[0-9]+\.[0-9]+\.[0-9]+$'
  fi
  gh api "repos/${repo}/releases?per_page=100" \
    --jq '.[] | select(.draft | not) | select(.prerelease | not) | .tag_name' |
    grep -E "$pattern" | head -n 1
}

version="${VERSION_INPUT:-}"
if [ -z "$version" ] && [ -n "${BAKED_VERSION:-}" ] && [ "$BAKED_VERSION" != "0.0.0" ]; then
  # The release process writes the release's own version into action.yml, so every ref of a release - a full tag,
  # the floating major tag or a commit SHA - installs the version it was published with. 0.0.0 marks a development
  # checkout, which has no release of its own.
  version="$BAKED_VERSION"
fi
if [ -z "$version" ]; then
  # A full release tag (`v0.1.0`) or a floating major tag (`v0`) when the action is pinned to
  # one; anything else (a branch, a SHA, a local `./` checkout) falls back to the newest release.
  if [[ "${ACTION_REF:-}" =~ ^v?[0-9]+(\.[0-9]+(\.[0-9]+([+.-][0-9A-Za-z.]+)?)?)?$ ]]; then
    version="$ACTION_REF"
  else
    version="latest"
  fi
fi

# `|| tag=""`: grep finds nothing when the series has no release, and pipefail would end the script before the
# error message below.
if [ "$version" = "latest" ]; then
  tag="$(newest_release)" || tag=""
elif [[ "$version" =~ ^v?[0-9]+(\.[0-9]+)?$ ]]; then
  tag="$(newest_release "${version#v}")" || tag=""
else
  case "$version" in
  v*) tag="$version" ;;
  *) tag="v${version}" ;;
  esac
fi

if [ -z "$tag" ]; then
  echo "::error::No published release found for version '${version}' in ${repo}"
  exit 1
fi

# The glibc archives need glibc 2.34 or newer. Older glibc hosts (Ubuntu 20.04, Debian 11, RHEL 8), Alpine and any
# libc that cannot be identified get the statically linked musl archives.
libc=gnu
if [ "$RUNNER_OS" = "Linux" ]; then
  libc="$(skif_libc)"
fi

case "$RUNNER_OS" in
Linux)
  case "$RUNNER_ARCH" in
  X64) triple="x86_64-unknown-linux-${libc}" ;;
  ARM64) triple="aarch64-unknown-linux-${libc}" ;;
  *)
    echo "::error::skif publishes no Linux release asset for architecture $RUNNER_ARCH"
    exit 1
    ;;
  esac
  ext="tar.gz"
  ;;
macOS)
  case "$RUNNER_ARCH" in
  ARM64) triple="aarch64-apple-darwin" ;;
  X64) triple="x86_64-apple-darwin" ;;
  *)
    echo "::error::skif publishes no macOS release asset for architecture $RUNNER_ARCH"
    exit 1
    ;;
  esac
  ext="tar.gz"
  ;;
Windows)
  case "$RUNNER_ARCH" in
  X64) triple="x86_64-pc-windows-msvc" ;;
  ARM64) triple="aarch64-pc-windows-msvc" ;;
  *)
    echo "::error::skif publishes no Windows release asset for architecture $RUNNER_ARCH"
    exit 1
    ;;
  esac
  ext="zip"
  ;;
*)
  echo "::error::Unsupported runner OS: $RUNNER_OS"
  exit 1
  ;;
esac

asset="skif-${triple}.${ext}"
checksums="checksums.txt"
install_dir="${RUNNER_TEMP}/skif-action/${tag}-${RUNNER_OS}-${RUNNER_ARCH}"
bin_dir="${install_dir}/bin"
archive_dir="${install_dir}/archive"

{
  echo "repo=${repo}"
  echo "tag=${tag}"
  echo "asset=${asset}"
  echo "ext=${ext}"
  echo "checksums=${checksums}"
  echo "bin-dir=${bin_dir}"
  echo "archive-dir=${archive_dir}"
  echo "cache-dir=${install_dir}"
} >>"$GITHUB_OUTPUT"
