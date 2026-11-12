#!/usr/bin/env bash
# The skif version pins in the documentation examples. Sourced by release-bump.sh, which rewrites them, and by
# check-doc-pins.sh, which fails when they name an older release line than the workspace version.
#
# A pin is one of
#   Goldziher/skif@v0.2 / @v1      the floating tag of a release line (GitHub Action)
#   Goldziher/skif@v0.2.0          a full release tag (GitHub Action)
#   rev: v0.2.0                    a pre-commit / prek hook revision, within two lines of the skif repository URL
#   revision = "v0.2.0"            a Poly hook source revision, within two lines of the skif repository URL
#   Goldziher/skif/releases/download/v0.2.0/   a release archive URL (Bazel http_archive)
#
# CHANGELOG.md and adrs/ record history and are never scanned.

# Prints the documentation files that may hold pins, relative to the repository root: the READMEs, the website and
# the agent skills under skills/.
skif_doc_pin_files() {
  local file
  for file in README.md CONTRIBUTING.md; do
    [[ -f "$file" ]] && printf '%s\n' "$file"
  done
  if [[ -d website/src/content/docs ]]; then
    find website/src/content/docs -type f \( -name '*.md' -o -name '*.mdx' \) | LC_ALL=C sort
  fi
  if [[ -d skills ]]; then
    find skills -type f -name '*.md' | LC_ALL=C sort
  fi
  return 0
}

# Prints the release line a version or tag belongs to: "0.M" for a 0.x version, "N" from 1.0.0. The floating tag
# of a release is "v" followed by its line (see release-floating-tag.sh).
skif_release_line() {
  local version="${1#v}" major minor
  version="${version%%-*}"
  major="${version%%.*}"
  if [[ "$major" != "0" ]]; then
    printf '%s\n' "$major"
    return 0
  fi
  minor="${version#*.}"
  if [[ "$minor" == "$version" ]]; then
    # A bare `v0` names no 0.x line.
    printf '0\n'
    return 0
  fi
  printf '0.%s\n' "${minor%%.*}"
}

# The awk program shared by the listing and the rewrite. With mode=list it prints "file:line:pin" for every pin; with
# mode=rewrite it prints the file with floating pins set to `floating` and full tags set to `full`.
# shellcheck disable=SC2016 # an awk program; its `$0` is awk's
skif_doc_pins_awk='
  function replace_action_pins(s,   out, pin, parts) {
    out = ""
    while (match(s, /Goldziher\/skif@v[0-9]+(\.[0-9]+)*/)) {
      pin = substr(s, RSTART + 15, RLENGTH - 15)
      if (mode == "list") print FILENAME ":" FNR ":" pin
      out = out substr(s, 1, RSTART + 14) (split(pin, parts, ".") == 3 ? full : floating)
      s = substr(s, RSTART + RLENGTH)
    }
    return out s
  }
  function replace_download_pins(s,   out, pin) {
    out = ""
    while (match(s, /Goldziher\/skif\/releases\/download\/v[0-9]+(\.[0-9]+)*\//)) {
      pin = substr(s, RSTART + 33, RLENGTH - 34)
      if (mode == "list") print FILENAME ":" FNR ":" pin
      out = out substr(s, 1, RSTART + 32) full "/"
      s = substr(s, RSTART + RLENGTH)
    }
    return out s
  }
  function replace_revision(s,   pin) {
    if (!match(s, /v[0-9]+(\.[0-9]+)*/)) return s
    pin = substr(s, RSTART, RLENGTH)
    if (mode == "list") print FILENAME ":" FNR ":" pin
    return substr(s, 1, RSTART - 1) full substr(s, RSTART + RLENGTH)
  }
  FNR == 1 { skif_at = -10 }
  {
    line = $0
    if (line ~ /github\.com\/Goldziher\/skif/) skif_at = FNR
    line = replace_action_pins(line)
    line = replace_download_pins(line)
    if (FNR - skif_at <= 2 && (line ~ /^[[:space:]-]*rev:[[:space:]]*v[0-9]/ || line ~ /^[[:space:]]*revision[[:space:]]*=[[:space:]]*"v[0-9]/)) {
      line = replace_revision(line)
    }
    if (mode == "rewrite") print line
  }
'

# Prints "file:line:pin" for every pin in the documentation files.
skif_doc_pins() {
  local file files=()
  while IFS= read -r file; do files+=("$file"); done < <(skif_doc_pin_files)
  [[ "${#files[@]}" -gt 0 ]] || return 0
  awk -v mode=list -v floating= -v full= "$skif_doc_pins_awk" "${files[@]}"
}

# Rewrites every pin to the release of <version>: floating pins to its floating tag, full tags to v<version>.
skif_rewrite_doc_pins() {
  local version="$1" floating file tmp
  floating="v$(skif_release_line "$version")"
  while IFS= read -r file; do
    tmp="$(mktemp)"
    awk -v mode=rewrite -v floating="$floating" -v full="v${version}" "$skif_doc_pins_awk" "$file" >"$tmp"
    cat "$tmp" >"$file"
    rm -f "$tmp"
  done < <(skif_doc_pin_files)
}
