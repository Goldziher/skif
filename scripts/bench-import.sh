#!/usr/bin/env bash
# Imports the results of a run of the bench workflow (.github/workflows/bench.yml, ADR 0113): downloads its artifact,
# refreshes the speed tables of tools/bench/README.md, README.md and the comparison guide from its summary, and puts
# summary.json, summary.md and the raw hyperfine exports (raw/) into tools/bench/results, ready to be committed.
#
# Only a successful, dispatched run of the bench workflow whose commit is on main is imported: the commit is checked
# against origin/main after `git fetch origin main` (which updates that remote-tracking branch), and the summary's
# harness commit has to be the run's. Nothing is written when one of these checks fails, the download fails, the
# artifact is incomplete, or refresh-numbers.mjs refuses the summary (a partial run, an older schema, a tool that was
# not timed, no GitHub-hosted runner).
#
# Usage: scripts/bench-import.sh <run-id>   (needs gh, git, jq and Node)
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

refuse() {
  echo "run ${run_id} of ${repo}: $*; not imported" >&2
  exit 1
}

fields=()
while IFS= read -r line; do
  fields+=("$line")
done < <(gh run view "$run_id" --repo "$repo" --json workflowName,event,conclusion,headSha,headBranch \
  --jq '.workflowName, .event, (.conclusion // ""), .headSha, .headBranch')
[[ ${#fields[@]} -eq 5 ]] || refuse "gh run view did not describe the run"
workflow="${fields[0]}" event="${fields[1]}" conclusion="${fields[2]}"
head_sha="${fields[3]}" head_branch="${fields[4]}"
[[ "$workflow" == bench ]] || refuse "it is a run of the ${workflow} workflow, not of bench"
[[ "$event" == workflow_dispatch ]] || refuse "it was triggered by ${event}, not workflow_dispatch"
[[ "$conclusion" == success ]] || refuse "it concluded ${conclusion:-nothing yet}, not success"
[[ "$head_sha" =~ ^[0-9a-f]{40}$ ]] || refuse "its commit ${head_sha} is not a commit SHA"
git fetch --quiet origin main
if ! git merge-base --is-ancestor "$head_sha" origin/main 2>/dev/null; then
  refuse "it measured ${head_sha:0:12} of the branch ${head_branch}, which is not on origin/main"
fi

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

harness_rev="$(jq -r '.harness_rev // ""' "${artifact}/summary.json")"
if [[ "$harness_rev" != "${head_sha:0:12}" ]]; then
  refuse "its summary names the harness commit ${harness_rev:-none}, not the run's ${head_sha:0:12}"
fi

node tools/bench/refresh-numbers.mjs --bench --summary "${artifact}/summary.json"

rm -rf "${results}/raw"
cp "${artifact}/summary.json" "${artifact}/summary.md" "${results}/"
cp -R "${artifact}/raw" "${results}/raw"
echo "imported $(basename "$artifact") of run ${run_id} into ${results}"
