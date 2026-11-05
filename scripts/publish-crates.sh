#!/usr/bin/env bash
# Publishes the skif crates to crates.io, leaves first.
#
# Environment:
#   VERSION     version being released (required)
#   DRY_RUN     "true" packages and verifies every crate (cargo publish --workspace --dry-run) but
#               uploads nothing; a real first run performs the same dry run before it uploads anything
#   CRATES_API  registry API base, overridable for tests (default https://crates.io/api/v1/crates)
#
# A crate whose directory does not exist yet is skipped with a log line, a version already on the
# registry is skipped, and a rate-limited publish (HTTP 429) is retried with a growing delay, so a
# re-run completes a partially published release.
set -euo pipefail

cd "$(dirname "$0")/.."

version="${VERSION:?VERSION is required}"
dry_run="${DRY_RUN:-false}"
api="${CRATES_API:-https://crates.io/api/v1/crates}"
max_attempts="${MAX_ATTEMPTS:-6}"
backoff_seconds="${BACKOFF_SECONDS:-60}"
settle_seconds="${SETTLE_SECONDS:-30}"

crates=(
  skif-core
  skif-specifier
  skif-config
  skif-edit
  skif-workspace
  skif-lint
  skif-report
  skif-js-syntax
  skif-js-plugins
  skif-migrate
  skif-rules
  skif-js
  skif
  skif-cli
)

published() {
  local status
  status="$(curl -sS -o /dev/null -w '%{http_code}' -H 'User-Agent: skif-release (https://github.com/Goldziher/skif)' "${api}/$1/${version}" || true)"
  [ "$status" = "200" ]
}

publish() {
  local crate="$1" attempt err
  err="$(mktemp)"
  for ((attempt = 1; attempt <= max_attempts; attempt++)); do
    if cargo publish -p "$crate" --locked 2>"$err"; then
      cat "$err"
      rm -f "$err"
      return 0
    fi
    if grep -Eqi 'already (exists|uploaded)' "$err"; then
      echo "${crate} ${version} already published; skipping"
      rm -f "$err"
      return 0
    fi
    if grep -Eqi '429|too many requests|rate limit' "$err" && [ "$attempt" -lt "$max_attempts" ]; then
      echo "${crate}: rate limited (attempt ${attempt}/${max_attempts}); waiting $((backoff_seconds * attempt))s"
      sleep $((backoff_seconds * attempt))
      continue
    fi
    cat "$err" >&2
    rm -f "$err"
    return 1
  done
  return 1
}

# Packages and builds every crate in dependency order against the local copies of its siblings, which is what a
# real publish would upload. A real run does it first too, so a packaging error in the last crate stops the
# release before the first one is live. A re-run (the first crate is already on the registry) skips it: cargo
# refuses to package versions that already exist.
if [ "$dry_run" = "true" ] || ! published "${crates[0]}"; then
  echo "::group::cargo publish --workspace --dry-run"
  cargo publish --workspace --dry-run --locked
  echo "::endgroup::"
fi

published_any=false
for crate in "${crates[@]}"; do
  if [ ! -f "crates/${crate}/Cargo.toml" ]; then
    echo "skip ${crate}: no crates/${crate} yet"
    continue
  fi
  if published "$crate"; then
    echo "skip ${crate}: ${version} is already on crates.io"
    continue
  fi
  if [ "$dry_run" = "true" ]; then
    echo "dry run: would publish ${crate} ${version}"
    continue
  fi
  if [ "$published_any" = "true" ]; then
    sleep "$settle_seconds"
  fi
  echo "::group::cargo publish -p ${crate}"
  publish "$crate"
  echo "::endgroup::"
  published_any=true
done
