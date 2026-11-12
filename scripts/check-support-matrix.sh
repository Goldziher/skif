#!/usr/bin/env bash
# Fails when the package managers, linkers, package manager versions or Node.js versions that the docs say skif is
# tested with differ from what CI tests (ADR 0030): the `e2e` matrix of .github/workflows/ci.yml (managers,
# version slots, operating systems), `e2e_linkers` in scripts/e2e/lib.sh and the `npm-addon` matrix (runtimes). The
# expected text is built from those sources and compared with the skif cells of the support tables in README.md and
# the comparison guide, and with the table under "Supported versions" in the installation guide.
#
# Usage: scripts/check-support-matrix.sh
set -euo pipefail

cd "$(dirname "$0")/.."

ci=.github/workflows/ci.yml
readme=README.md
guide=website/src/content/docs/guides/comparison.mdx
install=website/src/content/docs/start/installation.mdx

# shellcheck source=e2e/lib.sh
source scripts/e2e/lib.sh

# job <name>: the lines of a job of the CI workflow, up to the next job.
job() {
  awk -v job="$1" '
    $0 == "  " job ":" { found = 1; next }
    found && /^  [A-Za-z0-9_-]+:$/ { exit }
    found
  ' "$ci"
}

# matrix_axis <job> <axis>: the values of a `<axis>: [a, b]` matrix line of a job, one per line.
matrix_axis() {
  job "$1" | grep -E "^ +$2: \[" | head -n1 | sed -E 's/^[^[]*\[(.*)\].*$/\1/' | tr -d ' ' | tr ',' '\n' | grep . || true
}

pm_name() {
  case "$1" in
  npm | pnpm) echo "$1" ;;
  yarn) echo "Yarn 2+" ;;
  yarn-classic) echo "Yarn Classic" ;;
  bun) echo "Bun" ;;
  *) return 1 ;;
  esac
}

os_name() {
  case "$1" in
  ubuntu) echo Linux ;;
  macos) echo macOS ;;
  windows) echo Windows ;;
  *) return 1 ;;
  esac
}

# linker_name <pm> <linker>: a linker as the docs write it, in the setting the end-to-end generator writes.
linker_name() {
  case "$1/$2" in
  npm/hoisted | yarn-classic/hoisted | pnpm/isolated | bun/default) echo "$2" ;;
  yarn/pnp) echo "Plug'n'Play" ;;
  pnpm/* | yarn/*) echo "\`nodeLinker: $2\`" ;;
  bun/*) echo "\`linker = \"$2\"\`" ;;
  *) return 1 ;;
  esac
}

# join <separator> <last separator> <items...>: "a, b and c".
join() {
  local sep="$1" last="$2" out=""
  shift 2
  while [[ $# -gt 0 ]]; do
    if [[ -z "$out" ]]; then
      out="$1"
    elif [[ $# -eq 1 ]]; then
      out+="${last}$1"
    else
      out+="${sep}$1"
    fi
    shift
  done
  printf '%s' "$out"
}

failures=0
error() {
  echo "::error file=$1::$2"
  failures=$((failures + 1))
}

managers=() systems=() runtimes=()
while IFS= read -r value; do managers+=("$value"); done < <(matrix_axis e2e package-manager)
while IFS= read -r value; do systems+=("$value"); done < <(matrix_axis e2e os)
while IFS= read -r value; do runtimes+=("$value"); done < <(matrix_axis npm-addon runtime)
if [[ ${#managers[@]} -eq 0 || ${#systems[@]} -eq 0 || ${#runtimes[@]} -eq 0 ]]; then
  echo "::error file=${ci}::no package-manager or os axis in the e2e job, or no runtime axis in the npm-addon job" >&2
  exit 1
fi
# The managers tested on their newest release only, and the operating systems the previous majors skip.
newest_only="$(job e2e | sed -nE 's/^ +- \{ package-manager: ([a-z-]+), slot: previous \}.*$/\1/p')"
previous_skips="$(job e2e | sed -nE 's/^ +- \{ slot: previous, os: ([a-z]+) \}.*$/\1/p')"
# The major the newest-only managers are held to (`major=1`).
pinned_major="$(job e2e | sed -nE 's/^.*\) major=([0-9]+) ;;.*$/\1/p' | head -n1)"
if [[ -n "$newest_only" && -z "$pinned_major" ]]; then
  echo "::error file=${ci}::no pinned major found for the managers the e2e job tests on their newest release only" >&2
  exit 1
fi

names=() linkers=() both=() newest=()
all_systems=() previous_systems=()
for os in "${systems[@]}"; do
  name="$(os_name "$os")" || {
    error "$ci" "unknown e2e operating system '${os}'; add it to os_name in scripts/check-support-matrix.sh"
    continue
  }
  all_systems+=("$name")
  grep -qxF "$os" <<<"$previous_skips" || previous_systems+=("$name")
done
for pm in "${managers[@]}"; do
  name="$(pm_name "$pm")" || {
    error "$ci" "unknown e2e package manager '${pm}'; add it to pm_name in scripts/check-support-matrix.sh"
    continue
  }
  names+=("$name")
  layouts=()
  for linker in $(e2e_linkers "$pm"); do
    layouts+=("$(linker_name "$pm" "$linker")") || error scripts/e2e/lib.sh "unknown linker '${linker}' for ${pm}"
  done
  layout_cell="$(join ", " ", " "${layouts[@]}")"
  linkers+=("${name} (${layout_cell})")
  if grep -qxF "$pm" <<<"$newest_only"; then
    newest+=("$name ${pinned_major}.x")
    versions="newest ${pinned_major}.x"
    systems_cell="$(join ", " ", " "${all_systems[@]}")"
  else
    both+=("$name")
    versions="latest two majors"
    systems_cell="$(join ", " ", " "${all_systems[@]}") (previous major: $(join ", " ", " "${previous_systems[@]}"))"
  fi
  expected_row="| ${name} | ${versions} | ${layout_cell} | ${systems_cell} |"
  grep -qxF -- "$expected_row" "$install" ||
    error "$install" "the Supported versions table has no row '${expected_row}'"
done

nodes=() bun=""
for runtime in "${runtimes[@]}"; do
  case "$runtime" in
  node-*) nodes+=("${runtime#node-}") ;;
  bun) bun=" and Bun ${pinned_major:-1}.x" ;;
  *) error "$ci" "unknown npm-addon runtime '${runtime}'" ;;
  esac
done

# The rows of the installation table must be exactly the tested managers.
table_rows="$(awk '/^\| Package manager \| Versions \|/ { found = 1; next } found && !/^\|/ { exit } found && !/^\| ---/' "$install" | grep -c . || true)"
if [[ "$table_rows" -ne ${#managers[@]} ]]; then
  error "$install" "the Supported versions table has ${table_rows} rows, the e2e job tests ${#managers[@]} package managers"
fi

# skif_cell <file> <row label>: the skif column (the first after the label) of the table row with that label.
skif_cell() {
  awk -F'|' -v label="$2" '
    { cell = $2; gsub(/^ +| +$/, "", cell) }
    cell == label { value = $3; gsub(/^ +| +$/, "", value); print value; exit }
  ' "$1"
}

# expect_cell <file> <row label> <expected>: the skif cell equals the expected text.
expect_cell() {
  local actual
  actual="$(skif_cell "$1" "$2")"
  [[ "$actual" == "$3" ]] || error "$1" "'$2' says '${actual}' for skif, CI tests '$3'"
}

# expect_phrase <file> <row label> <phrase>: the skif cell contains the phrase.
expect_phrase() {
  local actual
  actual="$(skif_cell "$1" "$2")"
  [[ "$actual" == *"$3"* ]] || error "$1" "'$2' says '${actual}' for skif, which does not contain '$3' as CI tests"
}

pm_cell="$(join ", " ", " "${names[@]}")"
linker_cell="$(join "; " "; " "${linkers[@]}")"
runtime_phrase="Node.js $(join ", " ", " "${nodes[@]}")${bun}"
versions_cell="Latest two majors of $(join ", " " and " "${both[@]}")"
[[ ${#newest[@]} -eq 0 ]] || versions_cell+="; newest $(join ", " " and " "${newest[@]}")"

for file in "$readme" "$guide"; do
  expect_cell "$file" "Package managers" "$pm_cell"
  expect_cell "$file" "Linkers tested end to end in CI" "$linker_cell"
  expect_phrase "$file" "Runtime" "$runtime_phrase"
done
expect_cell "$guide" "Package manager versions tested" "$versions_cell"

if [[ "$failures" -gt 0 ]]; then
  echo "${failures} support matrix problem(s)" >&2
  exit 1
fi
echo "support matrix matches the CI matrices"
