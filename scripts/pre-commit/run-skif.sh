#!/usr/bin/env bash
# Runs skif for the pre-commit and prek hooks without requiring Rust or Node.
#
# Resolution order:
#   1. SKIF_BINARY, which must be executable (it never falls back to another binary)
#   2. skif on PATH
#   3. the pinned release, downloaded once into the cache and verified against checksums.txt
#
# SKIF_VERSION overrides the pinned release; SKIF_CACHE_DIR overrides the cache location;
# SKIF_RELEASE_URL overrides the download base (a mirror or a test server).
# SKIF_RESOLVE_ONLY=1 prints the resolved binary path instead of running it.
#
# On Linux the glibc archive is used only on glibc 2.34 or newer; older glibc, musl and an unidentifiable libc get the
# musl archive. SKIF_VERIFY_ATTESTATION=0 skips the build-attestation check, which also needs an authenticated gh.
set -euo pipefail

# shellcheck source=../lib/libc.sh
source "$(dirname "${BASH_SOURCE[0]}")/../lib/libc.sh"

SKIF_VERSION="${SKIF_VERSION:-v0.1.0}"
SKIF_BINARY="${SKIF_BINARY:-}"
SKIF_RESOLVE_ONLY="${SKIF_RESOLVE_ONLY:-0}"

tag="v${SKIF_VERSION#v}"
SKIF_RELEASE_URL="${SKIF_RELEASE_URL:-https://github.com/Goldziher/skif/releases/download/${tag}}"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    echo "skif: neither sha256sum nor shasum is available, refusing to install an unverified binary" >&2
    return 1
  fi
}

# The build attestation ties the archive to the release workflow; checksums.txt only detects corruption. It needs
# an authenticated `gh`; without one the checksum is all there is, and the launcher says so. Never silent.
verify_attestation() {
  local archive="$1"
  if [[ "${SKIF_VERIFY_ATTESTATION:-1}" == "0" ]]; then
    return 0
  fi
  if ! command -v gh >/dev/null 2>&1 || ! gh attestation verify --help >/dev/null 2>&1 || ! gh auth status >/dev/null 2>&1; then
    echo "skif: build attestation not verified (needs an authenticated gh with 'attestation'); checksum verified" >&2
    return 0
  fi
  local repo="${SKIF_REPOSITORY:-Goldziher/skif}"
  if ! gh attestation verify "$archive" --repo "$repo" --signer-workflow "${repo}/.github/workflows/publish.yaml" >&2; then
    echo "skif: attestation verification failed for $(basename "$archive"), refusing to install it" >&2
    return 1
  fi
}

detect_target() {
  local os arch libc
  case "$(uname -s)" in
  Linux*) os="unknown-linux" ;;
  Darwin*) os="apple-darwin" ;;
  MINGW* | MSYS* | CYGWIN* | Windows_NT) os="pc-windows-msvc" ;;
  *)
    echo "skif: unsupported platform: $(uname -s)" >&2
    return 1
    ;;
  esac
  case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *)
    echo "skif: unsupported architecture: $(uname -m)" >&2
    return 1
    ;;
  esac
  case "$os" in
  unknown-linux)
    libc="$(skif_libc)"
    echo "${arch}-unknown-linux-${libc}"
    ;;
  *) echo "${arch}-${os}" ;;
  esac
}

cache_root() {
  if [[ -n "${SKIF_CACHE_DIR:-}" ]]; then
    echo "$SKIF_CACHE_DIR"
  elif [[ -n "${LOCALAPPDATA:-}" ]]; then
    echo "${LOCALAPPDATA}/skif/pre-commit"
  else
    echo "${XDG_CACHE_HOME:-$HOME/.cache}/skif/pre-commit"
  fi
}

# Git Bash on Windows often has no unzip, and GNU tar cannot read zips: fall back to bsdtar, then Python.
extract_zip() {
  local archive="$1" member="$2" destination="$3"
  if command -v unzip >/dev/null 2>&1; then
    unzip -q "$archive" "$member" -d "$destination"
  elif tar --version 2>/dev/null | grep -qi bsdtar; then
    tar -xf "$archive" -C "$destination" "$member"
  elif command -v python3 >/dev/null 2>&1; then
    python3 -I -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extract(sys.argv[2], sys.argv[3])' \
      "$archive" "$member" "$destination"
  else
    echo "skif: no unzip, bsdtar or python3 to unpack ${archive}" >&2
    return 1
  fi
}

download() {
  local target ext exe archive cache dest expected actual tmp
  target="$(detect_target)"
  ext="tar.gz"
  exe="skif"
  if [[ "$target" == *windows* ]]; then
    ext="zip"
    exe="skif.exe"
  fi
  archive="skif-${target}.${ext}"
  cache="$(cache_root)/${tag}"
  dest="${cache}/${target}/${exe}"

  if [[ -x "$dest" ]]; then
    echo "$dest"
    return 0
  fi

  mkdir -p "$cache"
  tmp="$(mktemp -d "${cache}/.download.XXXXXX")"
  download_tmp="$tmp"
  trap 'rm -rf "${download_tmp:-}"' EXIT

  echo "Downloading skif ${tag} for ${target}..." >&2
  curl -fsSL "${SKIF_RELEASE_URL}/${archive}" -o "${tmp}/${archive}"
  if ! curl -fsSL "${SKIF_RELEASE_URL}/checksums.txt" -o "${tmp}/checksums.txt"; then
    echo "skif: could not download checksums.txt, refusing to install an unverified binary" >&2
    return 1
  fi

  expected="$(awk -v name="$archive" '$2 == name || $2 == "*" name { print $1; exit }' "${tmp}/checksums.txt")"
  if [[ -z "$expected" ]]; then
    echo "skif: no checksum for ${archive} in checksums.txt, refusing to install an unverified binary" >&2
    return 1
  fi
  actual="$(sha256 "${tmp}/${archive}")"
  if [[ "$expected" != "$actual" ]]; then
    echo "skif: checksum verification failed for ${archive}" >&2
    return 1
  fi
  verify_attestation "${tmp}/${archive}" || return 1

  mkdir -p "${tmp}/extract"
  if [[ "$ext" == "zip" ]]; then
    extract_zip "${tmp}/${archive}" "$exe" "${tmp}/extract"
  else
    tar -xzf "${tmp}/${archive}" -C "${tmp}/extract" "$exe"
  fi
  chmod +x "${tmp}/extract/${exe}"

  mkdir -p "${cache}/${target}"
  mv -f "${tmp}/extract/${exe}" "$dest"
  echo "$dest"
}

if [[ -n "$SKIF_BINARY" ]]; then
  if [[ ! -f "$SKIF_BINARY" || ! -x "$SKIF_BINARY" ]]; then
    echo "SKIF_BINARY is not executable: ${SKIF_BINARY}" >&2
    exit 1
  fi
  binary="$SKIF_BINARY"
elif command -v skif >/dev/null 2>&1; then
  binary="$(command -v skif)"
else
  binary="$(download)"
fi

if [[ "$SKIF_RESOLVE_ONLY" == "1" ]]; then
  printf '%s\n' "$binary"
  exit 0
fi

exec "$binary" "$@"
