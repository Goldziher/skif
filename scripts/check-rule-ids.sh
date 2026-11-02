#!/usr/bin/env bash
# Fails when a rule id recorded in rule-ids.txt is no longer documented in docs/rules.md, and when docs/rules.md
# documents an id that rule-ids.txt does not record yet (ADR 0007). Rule ids are a public contract: a released id
# is never removed, only retired through a superseding decision, and a new id is added to rule-ids.txt in the same
# change that adds the rule.
#
# Usage: scripts/check-rule-ids.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# shellcheck disable=SC2016
documented="$(grep -oE '^\| \[`[a-z0-9-]+`\]' docs/rules.md | sed -E 's/^\| \[`([a-z0-9-]+)`\]$/\1/' | sort)"
recorded="$(sort rule-ids.txt)"

failures=0
while IFS= read -r id; do
  echo "::error file=rule-ids.txt::rule '${id}' is recorded but no longer documented in docs/rules.md"
  failures=$((failures + 1))
done < <(comm -13 <(printf '%s\n' "$documented") <(printf '%s\n' "$recorded"))
while IFS= read -r id; do
  echo "::error file=rule-ids.txt::rule '${id}' is documented in docs/rules.md but missing from rule-ids.txt"
  failures=$((failures + 1))
done < <(comm -23 <(printf '%s\n' "$documented") <(printf '%s\n' "$recorded"))

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} rule id problem(s)" >&2
  exit 1
fi
echo "rule-ids.txt matches docs/rules.md"
