#!/usr/bin/env bash
# Builds the skif invocation from the action's inputs and runs it. With --format github (the
# default under GITHUB_ACTIONS) skif emits workflow annotations itself, so no problem matcher is
# registered.
set -euo pipefail

# ARGS is a space-separated argument string. Splitting it on whitespace is the point; globbing is
# disabled around the split so a pattern skif understands is not pre-expanded by the shell.
set -f
# shellcheck disable=SC2206
parsed_args=(${ARGS:-})
set +f

if [ "${FAIL_ON_WARNINGS:-false}" = "true" ]; then
  parsed_args+=("--fail-on-warnings")
fi

cd "$WORKING_DIRECTORY"
echo "Running: skif ${parsed_args[*]:-}"
# `${arr[@]+...}`: an empty array is an unbound variable under `set -u` on bash 3.2 (macOS runners).
exec skif ${parsed_args[@]+"${parsed_args[@]}"}
