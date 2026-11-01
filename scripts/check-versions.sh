#!/usr/bin/env bash
# Fails unless every versioned release surface agrees on one version.
#
# Usage: scripts/check-versions.sh [expected-version]
#
# Without an argument the workspace version in Cargo.toml is the reference.
set -euo pipefail

cd "$(dirname "$0")/.."

workspace_version="$(awk '/^\[workspace\.package\]/{p=1;next} /^\[/{p=0} p && /^version *= *"/{gsub(/"/,"",$3);print $3;exit}' Cargo.toml)"
expected="${1:-$workspace_version}"
failures=0

check() {
  local what="$1" actual="$2"
  if [[ "$actual" != "$expected" ]]; then
    echo "::error::${what}: expected ${expected}, found ${actual:-<none>}"
    failures=$((failures + 1))
  fi
}

check "Cargo.toml [workspace.package] version" "$workspace_version"

while IFS= read -r line; do
  file="${line%%:*}"
  pin="$(sed -E 's/.*version *= *"=?([^"]+)".*/\1/' <<<"${line#*:}")"
  name="$(sed -E 's/^ *([A-Za-z0-9_-]+) *=.*/\1/' <<<"${line#*:}")"
  check "${file}: internal dependency ${name}" "$pin"
done < <(grep -HE '^ *skif[A-Za-z0-9_-]* *= *\{.*path *=.*version *= *"' Cargo.toml crates/*/Cargo.toml || true)

check "npm-package/package.json version" "$(jq -r '.version' npm-package/package.json)"

while IFS=$'\t' read -r name pin; do
  check "npm-package/package.json optionalDependencies.${name}" "$pin"
done < <(jq -r '.optionalDependencies | to_entries[] | [.key, .value] | @tsv' npm-package/package.json)

declared="$(jq -r '.optionalDependencies | keys[]' npm-package/package.json | sort)"
present="$(for manifest in npm-package/platforms/*/package.json; do jq -r '.name' "$manifest"; done | sort)"
if [[ "$declared" != "$present" ]]; then
  echo "::error::npm-package optionalDependencies do not match the packages under npm-package/platforms"
  failures=$((failures + 1))
fi

for manifest in npm-package/platforms/*/package.json; do
  check "${manifest} version" "$(jq -r '.version' "$manifest")"
done

check "poly-hooks.toml pinned skif version" "$(sed -nE 's/.*skif@([0-9][^ "]*).*/\1/p' poly-hooks.toml | sort -u | paste -sd, -)"
check "scripts/pre-commit/run-skif.sh SKIF_VERSION default" "$(sed -nE 's/^SKIF_VERSION="\$\{SKIF_VERSION:-v?([^}]+)\}"$/\1/p' scripts/pre-commit/run-skif.sh)"

for hooks in .pre-commit-hooks.yaml poly-hooks.toml; do
  if [[ "$(grep -c -- '--no-js' "$hooks")" -ne 2 ]]; then
    echo "::error::${hooks}: both hooks must pass --no-js"
    failures=$((failures + 1))
  fi
  if [[ "$(grep -c -- '--no-install' "$hooks")" -ne 1 ]]; then
    echo "::error::${hooks}: only the fix hook may pass --no-install"
    failures=$((failures + 1))
  fi
done

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} version surface(s) disagree with ${expected}" >&2
  exit 1
fi
echo "All version surfaces agree on ${expected}"
