#!/usr/bin/env bash
# Bumps skif to one version across every release surface.
#
# Usage: scripts/release-bump.sh <version>
#
#   Cargo.toml                              [workspace.package] version and internal dependency pins
#   crates/*/Cargo.toml                     internal dependency pins
#   npm-package/package.json                version and every optionalDependencies pin
#   npm-package/platforms/*/package.json    version
#   poly-hooks.toml                         pinned npm channel version
#   scripts/pre-commit/run-skif.sh          SKIF_VERSION default
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
for manifest in npm-package/package.json npm-package/platforms/*/package.json; do
  tmp="$(mktemp)"
  jq --arg v "$version" '.version = $v | if has("optionalDependencies") then .optionalDependencies |= with_entries(.value = $v) else . end' "$manifest" >"$tmp"
  cat "$tmp" >"$manifest"
  rm -f "$tmp"
done

echo "poly-hooks.toml, scripts/pre-commit/run-skif.sh -> ${version}"
rewrite poly-hooks.toml "s/(skif@)[0-9][^ \"]*/\1${version}/g"
rewrite scripts/pre-commit/run-skif.sh "s/^(SKIF_VERSION=\"\\$\\{SKIF_VERSION:-)v?[^}]+(\\}\")$/\1v${version}\2/"

if [[ "${SKIP_CARGO_UPDATE:-0}" != "1" ]]; then
  cargo update --workspace >/dev/null 2>&1 || echo "warn: cargo update --workspace failed; refresh Cargo.lock before committing" >&2
fi

bash scripts/check-versions.sh "$version"
