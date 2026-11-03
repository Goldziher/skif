#!/usr/bin/env bash
# Fails when the plugin ids listed in crates/skif-js-plugins/README.md or in the website configuration reference
# differ from the plugin ids defined in crates/skif-js-plugins/src/plugins (ADR 0011). A list is the paragraph that
# follows a line starting "The table holds N plugins" (or "N built-in plugins"), with each id in backticks.
#
# Usage: scripts/check-plugin-list.sh
set -euo pipefail

cd "$(dirname "$0")/.."

defined="$(grep -hoE '^[[:space:]]+id: "[a-z0-9-]+"' crates/skif-js-plugins/src/plugins/*.rs |
  sed -E 's/^[[:space:]]+id: "(.*)"$/\1/' | sort)"
if [[ -z "$defined" ]]; then
  echo "::error::no plugin ids found under crates/skif-js-plugins/src/plugins" >&2
  exit 1
fi

# Prints the sorted backticked ids of the text from the first line of $1 matching $2 to the next blank line after
# the first line that carries an id.
listed() {
  awk -v marker="$2" '
    found && /^$/ { if (started) exit; next }
    found { if ($0 ~ /`/) started = 1; print; next }
    $0 ~ marker { found = 1; if ($0 ~ /`/) { started = 1; print } }
  ' "$1" | grep -oE '`[a-z0-9-]+`' | tr -d '`' | sort || true
}

failures=0
check() {
  local file="$1" marker="$2" list
  list="$(listed "$file" "$marker")"
  if [[ -z "$list" ]]; then
    echo "::error file=${file}::no plugin list found after '${marker}'"
    failures=$((failures + 1))
    return
  fi
  while IFS= read -r id; do
    echo "::error file=${file}::plugin '${id}' is listed but not defined in crates/skif-js-plugins"
    failures=$((failures + 1))
  done < <(comm -13 <(printf '%s\n' "$defined") <(printf '%s\n' "$list"))
  while IFS= read -r id; do
    echo "::error file=${file}::plugin '${id}' is defined in crates/skif-js-plugins but not listed"
    failures=$((failures + 1))
  done < <(comm -23 <(printf '%s\n' "$defined") <(printf '%s\n' "$list"))
}

check crates/skif-js-plugins/README.md '^The table holds [0-9]+ plugins'
check website/src/content/docs/reference/configuration.mdx '^The table holds [0-9]+ built-in plugins'

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} plugin list problem(s)" >&2
  exit 1
fi
echo "plugin lists match crates/skif-js-plugins"
