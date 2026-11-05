#!/usr/bin/env bash
# Builds the skif invocation from the action's inputs and runs it. With --format github (the
# default under GITHUB_ACTIONS) skif emits workflow annotations itself, so no problem matcher is
# registered.
set -euo pipefail

# ARGS is either a single line split on whitespace or, when it spans several lines, one argument per line (so an
# argument may contain spaces). Globbing is disabled around the split so a pattern skif understands is not
# pre-expanded by the shell.
parsed_args=()
if [[ "${ARGS:-}" == *$'\n'* ]]; then
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    [ -n "$line" ] && parsed_args+=("$line")
  done <<<"$ARGS"
else
  set -f
  # shellcheck disable=SC2206
  parsed_args=(${ARGS:-})
  set +f
fi

if [ "${FAIL_ON_WARNINGS:-false}" = "true" ]; then
  parsed_args+=("--fail-on-warnings")
fi

cd "$WORKING_DIRECTORY"
echo "Running: skif ${parsed_args[*]:-}"
# `${arr[@]+...}`: an empty array is an unbound variable under `set -u` on bash 3.2 (macOS runners).
exec skif ${parsed_args[@]+"${parsed_args[@]}"}
