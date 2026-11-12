#!/usr/bin/env bash
# Fails when a rule id recorded in rule-ids.txt is no longer documented in docs/rules.md, when docs/rules.md
# documents an id that rule-ids.txt does not record yet (ADR 0008), and when the rule ids registered in code (the
# top-level `RuleMeta` statics under crates/*/src) differ from rule-ids.txt. Rule ids are a public contract: a released id
# is never removed, only retired through a superseding decision, and a new id is added to rule-ids.txt in the same
# change that adds the rule. Prose that states a rule count ("N rules", "the N `js` rules") in the README files, the
# website, the agent skills and the contributor context must equal the count of rule ids (and, for `js` rules, the js
# rows of the table).
#
# Usage: scripts/check-rule-ids.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# shellcheck disable=SC2016
documented="$(grep -oE '^\| \[`[a-z0-9-]+`\]' docs/rules.md | sed -E 's/^\| \[`([a-z0-9-]+)`\]$/\1/' | sort)"
recorded="$(sort rule-ids.txt)"
# shellcheck disable=SC2016
registered="$({ grep -rhE -A1 '^(pub(\([a-z]+\))? )?(static|const) [A-Z_]+: RuleMeta = RuleMeta::new\(' crates/*/src || true; } |
  tr '\n' ' ' | grep -oE 'RuleMeta::new\( *"[a-z0-9-]+"' | sed -E 's/^.*"([a-z0-9-]+)"$/\1/' | sort || true)"
if [[ -z "$registered" ]]; then
  echo "::error::no RuleMeta statics found under crates/*/src; update the pattern in scripts/check-rule-ids.sh"
  exit 1
fi

failures=0
while IFS= read -r id; do
  echo "::error file=rule-ids.txt::rule '${id}' is recorded but no longer documented in docs/rules.md"
  failures=$((failures + 1))
done < <(comm -13 <(printf '%s\n' "$documented") <(printf '%s\n' "$recorded"))
while IFS= read -r id; do
  echo "::error file=rule-ids.txt::rule '${id}' is documented in docs/rules.md but missing from rule-ids.txt"
  failures=$((failures + 1))
done < <(comm -23 <(printf '%s\n' "$documented") <(printf '%s\n' "$recorded"))

while IFS= read -r id; do
  echo "::error file=rule-ids.txt::rule '${id}' is recorded but not registered in code"
  failures=$((failures + 1))
done < <(comm -13 <(printf '%s\n' "$registered") <(printf '%s\n' "$recorded"))
while IFS= read -r id; do
  echo "::error file=rule-ids.txt::rule '${id}' is registered in code but missing from rule-ids.txt"
  failures=$((failures + 1))
done < <(comm -23 <(printf '%s\n' "$registered") <(printf '%s\n' "$recorded"))

total="$(grep -c . rule-ids.txt)"
# shellcheck disable=SC2016
js_total="$(grep -cE '^\| \[`[a-z0-9-]+`\]\(#[a-z0-9-]+\) \| js \|' docs/rules.md || true)"
count_files=(README.md crates/*/README.md npm-package/README.md CONTRIBUTING.md .ai-rulez/context/*.md)
while IFS= read -r -d '' file; do count_files+=("$file"); done < <(find website/src/content -name '*.md*' -print0)
if [[ -d skills ]]; then
  while IFS= read -r -d '' file; do count_files+=("$file"); done < <(find skills -name '*.md' -print0)
fi
# shellcheck disable=SC2016
while IFS=: read -r file line claim; do
  number="$(grep -oE '[0-9]+' <<<"$claim" | head -n1)"
  if [[ "$claim" == *'`js` rules'* || "$claim" == *'JS analysis rules'* ]]; then
    expected="$js_total"
  else
    expected="$total"
  fi
  if [[ "$number" != "$expected" ]]; then
    echo "::error file=${file},line=${line}::states ${number} rules (\"${claim}\") but there are ${expected}"
    failures=$((failures + 1))
  fi
done < <(grep -nEo '[0-9]+ (`js` |JS analysis )?rules' "${count_files[@]}" 2>/dev/null || true)

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} rule id problem(s)" >&2
  exit 1
fi
echo "rule-ids.txt matches docs/rules.md and the rules registered in code"
