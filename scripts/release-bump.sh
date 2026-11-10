#!/usr/bin/env bash
# Bumps skif to one version across every release surface.
#
# Usage: scripts/release-bump.sh <version>
#
#   Cargo.toml                              [workspace.package] version and internal dependency pins
#   crates/*/Cargo.toml                     internal dependency pins
#   npm-package/package.json                version (the platform packages take it from `napi version`)
#   poly-hooks.toml                         pinned npm channel version
#   scripts/pre-commit/run-skif.sh          SKIF_VERSION default
#   action.yml                              BAKED_VERSION, the version the Action installs by default
#   README.md, CONTRIBUTING.md, website     the Action, pre-commit and Poly pins of the examples (lib/doc-pins.sh);
#                                           a pre-release leaves them on the previous release
#   Cargo.lock                              refreshed with `cargo update --workspace`; a failure fails the bump
#
# Set SKIP_CARGO_UPDATE=1 to leave Cargo.lock alone (offline bumps).
set -euo pipefail

version="${1:?usage: release-bump.sh <version>}"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-(rc|beta|alpha)\.[0-9]+)?$ ]]; then
  echo "error: version must be MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-rc.N (got '${version}')" >&2
  exit 1
fi

cd "$(dirname "$0")/.."

rewrite() {
  local file="$1" expression="$2" tmp
  tmp="$(mktemp)"
  sed -E "$expression" "$file" >"$tmp"
  cat "$tmp" >"$file"
  rm -f "$tmp"
}

echo "Cargo.toml [workspace.package] -> ${version}"
tmp="$(mktemp)"
awk -v v="$version" '
  /^\[workspace\.package\]/ { in_pkg = 1; print; next }
  /^\[/ { in_pkg = 0 }
  in_pkg && /^version *= *"/ { print "version = \"" v "\""; next }
  { print }
' Cargo.toml >"$tmp"
cat "$tmp" >Cargo.toml
rm -f "$tmp"

echo "internal dependency pins -> =${version}"
for manifest in Cargo.toml crates/*/Cargo.toml; do
  rewrite "$manifest" "/^ *skif[A-Za-z0-9_-]* *= *\{.*path *=.*version *= *\"/ s/(version *= *\")=?[^\"]+(\")/\1=${version}\2/"
done

echo "npm-package -> ${version}"
tmp="$(mktemp)"
jq --arg v "$version" '.version = $v' npm-package/package.json >"$tmp"
cat "$tmp" >npm-package/package.json
rm -f "$tmp"

echo "poly-hooks.toml, scripts/pre-commit/run-skif.sh -> ${version}"
rewrite poly-hooks.toml "s/(skif-cli@)[0-9][^ \"]*/\1${version}/g"
rewrite scripts/pre-commit/run-skif.sh "s/^(SKIF_VERSION=\"\\$\\{SKIF_VERSION:-)v?[^}]+(\\}\")$/\1v${version}\2/"

echo "action.yml -> ${version}"
rewrite action.yml "s/^( *BAKED_VERSION: \")[^\"]*(\")/\1${version}\2/"

# shellcheck source=lib/doc-pins.sh
source scripts/lib/doc-pins.sh
if [[ "$version" == *-* ]]; then
  echo "documentation pins: ${version} is a pre-release, left unchanged"
else
  echo "documentation pins -> v$(skif_release_line "$version"), v${version}"
  skif_rewrite_doc_pins "$version"
fi

if [[ "${SKIP_CARGO_UPDATE:-0}" != "1" ]]; then
  update_log="$(mktemp)"
  if ! cargo update --workspace >/dev/null 2>"$update_log"; then
    cat "$update_log" >&2
    rm -f "$update_log"
    echo "error: cargo update --workspace failed, so Cargo.lock still names the old version (SKIP_CARGO_UPDATE=1 skips it)" >&2
    exit 1
  fi
  rm -f "$update_log"
fi

bash scripts/check-versions.sh "$version"
bash scripts/check-doc-pins.sh "$version"
