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

case "${FROM_SHERIF:-false}" in
true | false) ;;
*)
  echo "::error::from-sherif must be \"true\" or \"false\", not \"${FROM_SHERIF}\""
  exit 2
  ;;
esac

cd "$WORKING_DIRECTORY"

# The subcommands of skif (scripts/test-install-scripts.sh checks this list against the CLI). A run of one of them is
# not a lint run: it gets neither --fail-on-warnings nor a translated sherif configuration, flags only a lint run takes.
subcommands=" rules schema init config explain-usage migrate baseline update list json format mcp lsp help "

is_lint_run() {
  [ "${#parsed_args[@]}" -eq 0 ] || [[ "$subcommands" != *" ${parsed_args[0]} "* ]]
}

if [ "${FAIL_ON_WARNINGS:-false}" = "true" ] && is_lint_run; then
  parsed_args+=("--fail-on-warnings")
fi

# Whether skif would read a skif.toml or .skif.toml: in the working directory or a directory above it up to the
# repository root, or anywhere else the repository tracks one.
has_skif_config() {
  local top dir="$PWD"
  top="$(git rev-parse --show-toplevel 2>/dev/null)" || top=""
  if [ -n "$top" ] && [ -n "$(git -C "$top" ls-files -- ':(glob)**/skif.toml' ':(glob)**/.skif.toml' 2>/dev/null)" ]; then
    return 0
  fi
  while :; do
    if [ -f "${dir}/skif.toml" ] || [ -f "${dir}/.skif.toml" ]; then
      return 0
    fi
    if [ -z "$top" ] || [ "$dir" = "$top" ] || [ "$dir" = "/" ]; then
      return 1
    fi
    dir="$(dirname "$dir")"
  done
}

# from-sherif (opt-in migration aid): a project with no skif configuration yet whose root package.json runs sherif (a `sherif` key, or a
# script that runs it) is linted with its sherif settings translated by `skif migrate`, so a job that replaces sherif
# keeps its ignores and --fail-on-warnings. `--profile full` keeps skif's default rules; the translation goes to a
# temporary file, never into the repository. The skif binary itself never reads sherif configuration (ADR 0039).
use_sherif_config() {
  [ "${FROM_SHERIF:-false}" = "true" ] || return 1
  if [ ! -f package.json ] || ! grep -q 'sherif' package.json; then
    return 1
  fi
  is_lint_run || return 1
  local arg
  for arg in ${parsed_args[@]+"${parsed_args[@]}"}; do
    case "$arg" in --config | --config=* | --no-config) return 1 ;; esac
  done
  ! has_skif_config
}

if use_sherif_config; then
  sherif_config="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/skif-from-sherif.XXXXXX")/skif.toml"
  status=0
  echo "::group::Translate the sherif configuration (skif migrate --from sherif)"
  skif migrate --from sherif --profile full --strict --output "$sherif_config" || status=$?
  echo "::endgroup::"
  case "$status" in
  0) parsed_args+=("--config" "$sherif_config") ;;
  1)
    echo "::warning::Some sherif settings were not translated; see the skif migrate report. Run skif migrate and commit skif.toml to settle them."
    parsed_args+=("--config" "$sherif_config")
    ;;
  *) echo "::warning::skif migrate could not translate the sherif configuration; skif runs without it." ;;
  esac
fi

echo "Running: skif ${parsed_args[*]:-}"
# `${arr[@]+...}`: an empty array is an unbound variable under `set -u` on bash 3.2 (macOS runners).
exec skif ${parsed_args[@]+"${parsed_args[@]}"}
