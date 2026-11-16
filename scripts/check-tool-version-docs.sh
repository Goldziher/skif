#!/usr/bin/env bash
# Fails when the documentation names another version of a compared tool (knip, syncpack, sherif) than the one pinned in
# tools/bench/corpus.toml [tools], so the capability tables and the measured numbers describe the same releases.
#
# Usage: scripts/check-tool-version-docs.sh
#
# Only statements of the compared version are checked; history ("Added in knip 6.45.0", "dropped in knip 6") is not.
# A stated version may be shorter than the pin (`knip 6.45` matches 6.45.2, `sherif 1.13` matches 1.13.0), never
# longer or different. The patterns, each followed by the version, per file and tool:
#   website/src/content/docs/guides/comparison.mdx
#     intro of the capability tables   The tables compare skif with sherif V, syncpack V and knip V
#     the measured-numbers sentence    against knip V, sherif V, syncpack V
#     the Sources list                 "- sherif V:", "- syncpack V:", "- knip V:"
#     Sources links                    QuiiBz/sherif/{blob,tree,releases/tag}/vV, JamieMason/syncpack/{blob,tree}/V,
#                                      webpro-nl/knip/{blob,tree}/knip%40V
#     coverage statements              "issue types of knip V", "Every issue type of knip V"
#   website/src/content/docs/guides/migrating.mdx
#     the port's sources               "(sherif V, syncpack V, knip V)"
#     the fixed behaviours             "behaviours of sherif V"
# Every tool must be stated at least once per file, so a pattern that no longer matches fails instead of passing.
set -euo pipefail

cd "$(dirname "$0")/.."

corpus=tools/bench/corpus.toml
comparison=website/src/content/docs/guides/comparison.mdx
migrating=website/src/content/docs/guides/migrating.mdx
version_re='[0-9]+(\.[0-9]+)*'

pinned() {
  awk -v tool="$1" '/^\[tools\]/{p=1;next} /^\[/{p=0} p && $1==tool && $2=="="{gsub(/"/,"",$3);print $3;exit}' "$corpus"
}

failures=0
fail() {
  echo "::error file=$1${2:+,line=$2}::$3"
  failures=$((failures + 1))
}

# check <file> <tool> <prefix regex>...: every statement of the tool's version in the file matches the pin.
check() {
  local file="$1" tool="$2" pin found=0 prefix hit number stated
  shift 2
  pin="$(pinned "$tool")"
  if [[ -z "$pin" ]]; then
    fail "$corpus" "" "no ${tool} version in [tools]"
    return
  fi
  for prefix in "$@"; do
    while IFS= read -r hit; do
      number="${hit%%:*}"
      stated="$(sed -E "s#^[0-9]+:##; s#^${prefix}##" <<<"$hit")"
      found=$((found + 1))
      if [[ "$pin" != "$stated" && "$pin" != "$stated".* ]]; then
        fail "$file" "$number" "the documentation names ${tool} ${stated}, but ${corpus} pins ${pin}"
      fi
    done < <(grep -noE "${prefix}${version_re}" "$file" || true)
  done
  if [[ "$found" -eq 0 ]]; then
    fail "$file" "" "no ${tool} version found; the patterns in scripts/check-tool-version-docs.sh no longer match the page"
  fi
}

for file in "$comparison" "$migrating"; do
  [[ -f "$file" ]] || fail "$file" "" "the file does not exist"
done
if [[ "$failures" -gt 0 ]]; then exit 1; fi

check "$comparison" sherif 'The tables compare skif with sherif ' 'against knip [0-9.]+, sherif ' '^- sherif ' \
  'QuiiBz/sherif/(blob|tree|releases/tag)/v'
check "$comparison" syncpack 'The tables compare skif with sherif [0-9.]+, syncpack ' \
  'against knip [0-9.]+, sherif [0-9.]+, syncpack ' '^- syncpack ' 'JamieMason/syncpack/(blob|tree)/'
check "$comparison" knip 'syncpack [0-9.]+ and knip ' 'against knip ' '^- knip ' 'webpro-nl/knip/(blob|tree)/knip%40' \
  'issue types of knip ' 'Every issue type of knip '
check "$migrating" sherif '\(sherif ' 'behaviours of sherif '
check "$migrating" syncpack '\(sherif [0-9.]+, syncpack '
check "$migrating" knip '\(sherif [0-9.]+, syncpack [0-9.]+, knip '

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} documented tool version(s) differ from ${corpus}; update the docs or the pin together" >&2
  exit 1
fi
echo "The documented knip, syncpack and sherif versions match ${corpus}"
