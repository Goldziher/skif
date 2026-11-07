#!/usr/bin/env bash
# Builds skif with js-analysis while every oxc crate is redirected to the git commit of an oxc release
# tag, the way poly redirects them with [patch.crates-io]. Fails when the pins drift from that release
# (cargo reports the patch as unused), when the build breaks, or when two copies of an oxc crate remain
# in the graph. See ADR 0021.
#
# Usage: scripts/check-oxc-git-patch.sh [OXC_TAG]
#   OXC_TAG  tag in oxc-project/oxc, e.g. crates_v0.157.0. Defaults to the release matching the
#            `oxc_parser` pin in Cargo.toml. The environment variable of the same name works too.
#   OXC_REPO overrides the repository URL (default https://github.com/oxc-project/oxc).
set -euo pipefail

cd "$(dirname "$0")/.."

# The crates poly patches; oxc_resolver lives in oxc-project/oxc-resolver and is not part of the patch.
crates=(oxc_allocator oxc_ast oxc_ast_visit oxc_parser oxc_semantic oxc_span oxc_str)
repo="${OXC_REPO:-https://github.com/oxc-project/oxc}"

pinned="$(sed -n 's/^oxc_parser *= *"\([0-9][0-9.]*\)".*/\1/p' Cargo.toml)"
if [[ -z "$pinned" ]]; then
  echo "::error file=Cargo.toml::cannot read the oxc_parser version" >&2
  exit 1
fi
tag="${1:-${OXC_TAG:-crates_v${pinned}}}"

rev="$(git ls-remote "$repo" "refs/tags/${tag}^{}" "refs/tags/${tag}" | awk 'NR==1 {print $1}')"
if [[ ! "$rev" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error::tag ${tag} not found in ${repo}" >&2
  exit 1
fi
echo "oxc ${tag} -> ${rev} (skif pins ${pinned})"

# cargo rewrites Cargo.lock for the patched graph; put the committed one back afterwards.
lock_backup="$(mktemp)"
log="$(mktemp)"
cp Cargo.lock "$lock_backup"
trap 'cp "$lock_backup" Cargo.lock; rm -f "$lock_backup" "$log"' EXIT

config=()
for crate in "${crates[@]}"; do
  config+=(--config "patch.crates-io.${crate}.git=\"${repo}\"" --config "patch.crates-io.${crate}.rev=\"${rev}\"")
done

cargo "${config[@]}" build -p skif --features js-analysis 2>&1 | tee "$log"

if grep -E "Patch .* was not used" "$log"; then
  echo "::error::oxc pins in Cargo.toml do not match ${tag}: a patch was not used. Bump all oxc pins together (ADR 0021)." >&2
  exit 1
fi

tree="$(cargo "${config[@]}" tree -p skif --features js-analysis -e normal -d)"
if grep -E '^oxc_' <<<"$tree"; then
  echo "::error::more than one copy of an oxc crate is in the dependency graph" >&2
  exit 1
fi

for crate in "${crates[@]}"; do
  if ! cargo "${config[@]}" tree -p skif --features js-analysis -e normal -i "$crate" 2>/dev/null | head -n 1 | grep -q "oxc-project/oxc"; then
    echo "::error::${crate} does not resolve to the git copy of ${tag}" >&2
    exit 1
  fi
done

echo "ok: skif builds with js-analysis on ${tag}"
