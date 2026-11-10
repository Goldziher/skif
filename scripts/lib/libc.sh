#!/usr/bin/env bash
# Picks the Linux libc flavour of the skif release archives for this host. Sourced by the GitHub Action's resolve step
# and by the pre-commit launcher; the Homebrew formula applies the same rule in Ruby.
#
# The `*-linux-gnu` archives are linked against glibc 2.34 (see SKIF_GLIBC_FLOOR), so they run on any host with at
# least that glibc. Everything else - an older glibc, musl, or a libc that cannot be identified - gets the
# statically linked `*-linux-musl` archive, which runs anywhere.

SKIF_GLIBC_FLOOR="2.34"

# Prints the glibc version ("2.35") of this host, or nothing when it is not glibc or cannot be read.
skif_glibc_version() {
  local out
  out="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
  if [[ "$out" =~ glibc[[:space:]]+([0-9]+\.[0-9]+) ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
    return 0
  fi
  # glibc prints "ldd (Debian GLIBC 2.31-13) 2.31" first; musl prints its own banner and exits non-zero.
  out="$(ldd --version 2>&1 | head -n 1 || true)"
  if [[ "$out" == *[Gg][Ll][Ii][Bb][Cc]* && "$out" =~ ([0-9]+\.[0-9]+)[^[:space:]]*$ ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
  fi
}

# skif_version_ge <version> <floor>: true when MAJOR.MINOR of the first is at least that of the second.
skif_version_ge() {
  local have_major have_minor want_major want_minor
  IFS=. read -r have_major have_minor <<<"$1"
  IFS=. read -r want_major want_minor <<<"$2"
  have_major=$((10#${have_major:-0}))
  have_minor=$((10#${have_minor:-0}))
  want_major=$((10#${want_major:-0}))
  want_minor=$((10#${want_minor:-0}))
  ((have_major > want_major || (have_major == want_major && have_minor >= want_minor)))
}

# Prints `gnu` when the host's glibc meets the floor, otherwise `musl`. The libc is the one `ldd` and `getconf` report,
# not whichever loaders are installed: a glibc host may carry /lib/ld-musl-* (Debian's musl package), and a musl host
# without glibc evidence gets musl anyway.
skif_libc() {
  local banner version
  # The banner is captured on its own: musl's `ldd --version` exits non-zero, which a pipeline would turn into a
  # failure under pipefail.
  banner="$(ldd --version 2>&1 || true)"
  if [[ "$banner" == *musl* ]]; then
    echo musl
    return 0
  fi
  version="$(skif_glibc_version)"
  if [[ -n "$version" ]] && skif_version_ge "$version" "$SKIF_GLIBC_FLOOR"; then
    echo gnu
  else
    echo musl
  fi
}
