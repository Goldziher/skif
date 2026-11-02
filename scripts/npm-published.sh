#!/usr/bin/env bash
# Asks the npm registry whether <name>@<version> is already published. Prints `published` or `unpublished`.
#
# Usage: scripts/npm-published.sh <name> <version>
#
# Only a definite answer counts: a 404 means unpublished (including a package that does not exist yet). Anything
# else - a rate limit (429), a 5xx, a network failure - is retried and then fails with exit 2, so a publish job never
# mistakes "the registry did not answer" for "not published" and never skips a publish on a wrong guess.
#
# NPM_PROBE_ATTEMPTS (default 5) and NPM_PROBE_DELAY seconds between attempts (default 10) tune the retries.
set -euo pipefail

name="${1:?usage: npm-published.sh <name> <version>}"
version="${2:?usage: npm-published.sh <name> <version>}"
attempts="${NPM_PROBE_ATTEMPTS:-5}"
delay="${NPM_PROBE_DELAY:-10}"

for ((attempt = 1; attempt <= attempts; attempt++)); do
  status=0
  output="$(npm view "${name}@${version}" version 2>&1)" || status=$?
  if [[ "$status" -eq 0 ]]; then
    if [[ -n "$output" ]]; then
      echo published
    else
      echo unpublished
    fi
    exit 0
  fi
  if [[ "$output" == *E404* ]]; then
    echo unpublished
    exit 0
  fi
  echo "attempt ${attempt}/${attempts}: npm view ${name}@${version} failed: ${output}" >&2
  if [[ "$attempt" -lt "$attempts" ]]; then
    sleep "$delay"
  fi
done

echo "::error::could not tell whether ${name}@${version} is published" >&2
exit 2
