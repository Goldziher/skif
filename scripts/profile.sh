#!/usr/bin/env bash
# Records one skif run on a repository with samply and saves SKIF_TIMINGS beside it.
# Usage: scripts/profile.sh <repo-dir> [skif args...]
set -euo pipefail

if [ "$#" -lt 1 ]; then
  echo "usage: profile.sh <repo-dir> [skif args...]" >&2
  exit 2
fi
target_dir=$1
shift
if [ ! -d "$target_dir" ]; then
  echo "profile.sh: $target_dir is not a directory" >&2
  exit 2
fi

root=$(cd "$(dirname "$0")/.." && pwd)
name=$(basename "$(cd "$target_dir" && pwd)")
stamp=$(date +%Y%m%d-%H%M%S)
mkdir -p "$root/reports"
profile="$root/reports/profile-$name-$stamp.json.gz"
timings="$root/reports/timings-$name-$stamp.txt"

cargo build --profile profiling --locked -p skif-cli --manifest-path "$root/Cargo.toml"

# skif exits non-zero when it finds problems; the profile is still wanted.
SKIF_TIMINGS=1 samply record --save-only -o "$profile" \
  "$root/target/profiling/skif" "$target_dir" --format json --no-install "$@" \
  >/dev/null 2>"$timings" || true

echo "profile: $profile (open with: samply load $profile)"
echo "timings: $timings"
