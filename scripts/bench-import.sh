#!/usr/bin/env bash
# Imports the results of a run of the bench workflow (.github/workflows/bench.yml, ADR 0113): downloads its artifact,
# refreshes the speed tables of tools/bench/README.md, README.md and the comparison guide from its summary, and puts
# summary.json, summary.md and the raw hyperfine exports (raw/) into tools/bench/results, ready to be committed. Nothing
# is written when the download fails, the artifact is incomplete, or refresh-numbers.mjs refuses the summary (a partial
# run, an older schema, a tool that was not timed).
#
# Usage: scripts/bench-import.sh <run-id>
#   SKIF_BENCH_REPO  the repository the run belongs to (default Goldziher/skif)
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# -ne 1 || ! "$1" =~ ^[0-9]+$ ]]; then
  echo "usage: scripts/bench-import.sh <run-id> (the numeric id of a bench workflow run, from its URL)" >&2
  exit 2
fi
run_id="$1"
repo="${SKIF_BENCH_REPO:-Goldziher/skif}"
results="tools/bench/results"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

gh run download "$run_id" --repo "$repo" --pattern 'bench-*' --dir "${work}/download"

# With --pattern gh writes each artifact into a directory named after it; a single artifact extracted in place is
# taken as it is.
if [[ -f "${work}/download/summary.json" ]]; then
  artifacts=("${work}/download/")
else
  shopt -s nullglob
  artifacts=("${work}"/download/*/)
  shopt -u nullglob
fi
if [[ ${#artifacts[@]} -ne 1 ]]; then
  echo "run ${run_id} of ${repo} has ${#artifacts[@]} bench artifacts; expected one" >&2
  exit 1
fi
artifact="${artifacts[0]%/}"
for file in summary.json summary.md; do
  if [[ ! -f "${artifact}/${file}" ]]; then
    echo "the artifact $(basename "$artifact") of run ${run_id} has no ${file}" >&2
    exit 1
  fi
done
if [[ ! -d "${artifact}/raw" ]]; then
  echo "the artifact $(basename "$artifact") of run ${run_id} has no raw/" >&2
  exit 1
fi

node tools/bench/refresh-numbers.mjs --bench --summary "${artifact}/summary.json"

rm -rf "${results}/raw"
cp "${artifact}/summary.json" "${artifact}/summary.md" "${results}/"
cp -R "${artifact}/raw" "${results}/raw"
echo "imported $(basename "$artifact") of run ${run_id} into ${results}"
