#!/usr/bin/env bash
# Tests the end-to-end scripts without a package manager or the network:
#   e2e/resolve-pm-version.sh   the newest and previous majors from a stub `npm view`, Yarn Berry and Classic, Bun 1.x
#   e2e/lib.sh                  the manager tables, the generated projects (workspace files, catalogs, linkers, pins,
#                               overlays), the expectation overrides and the findings format, and every committed
#                               scenario generated and its expectation evaluated for every manager and linker
#
# Usage: scripts/test-e2e-scripts.sh
# The literals checked below contain `$` on purpose; nothing is meant to expand.
# shellcheck disable=SC2016
set -euo pipefail

cd "$(dirname "$0")/.."
repo="$PWD"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# shellcheck source=e2e/lib.sh
source "${repo}/scripts/e2e/lib.sh"

failures=0
fail() {
  echo "FAIL $*" >&2
  failures=$((failures + 1))
}

# expect <label> <expected exit status> <command...>: runs the command, output to ${work}/out.
expect() {
  local label="$1" want="$2" status=0
  shift 2
  "$@" >"${work}/out" 2>&1 || status=$?
  if [[ "$status" -ne "$want" ]]; then
    fail "${label}: exit ${status}, want ${want}"
    sed 's/^/    /' "${work}/out" >&2
  fi
}

expect_in_out() {
  grep -qF -- "$2" "${work}/out" || fail "$1: output does not contain '$2'"
}

# same <label> <want> <got>
same() {
  [[ "$3" == "$2" ]] || fail "$1: got '$3', want '$2'"
}

# ---------------------------------------------------------------- e2e/resolve-pm-version
pm_stubs="${work}/pm-stubs"
mkdir -p "$pm_stubs"
cat >"${pm_stubs}/npm" <<'STUB'
#!/usr/bin/env bash
# `npm view <package> versions --json`: pre-releases and majors with no stable release must not count.
case "$2" in
npm) echo '["10.9.0","10.9.2","11.0.0","11.4.1","12.0.0-rc.1","9.9.4"]' ;;
bun) echo '["1.1.0","1.2.20","1.10.1","2.0.0-canary.1","2.0.1","1.2.3"]' ;;
@yarnpkg/cli-dist) echo '["2.4.3","3.8.7","3.2.0","4.9.2","4.10.0","5.0.0-alpha.1"]' ;;
yarn) echo '["0.28.4","1.9.4","1.22.19","1.22.22","2.0.0-rc.1"]' ;;
*) echo '[]' ;;
esac
STUB
chmod +x "${pm_stubs}/npm"
resolve() {
  env PATH="${pm_stubs}:${PATH}" bash scripts/e2e/resolve-pm-version.sh "$@"
}
pm_case() {
  local label="$1" want="$2"
  shift 2
  expect "resolve-pm-version: ${label}" 0 resolve "$@"
  same "resolve-pm-version: ${label}" "$want" "$(cat "${work}/out")"
}
pm_case "npm newest ignores a release candidate" 11.4.1 npm newest
pm_case "npm previous" 10.9.2 npm previous
pm_case "yarn reads the berry package, newest" 4.10.0 yarn newest
pm_case "yarn previous sorts numerically" 3.8.7 yarn previous
pm_case "yarn-classic reads the yarn package and sorts numerically" 1.22.22 yarn-classic newest 1
pm_case "yarn-classic ignores a Yarn 2 release candidate" 1.22.22 yarn-classic newest
expect "resolve-pm-version: no such major" 1 resolve pnpm newest
pm_case "bun restricted to 1.x ignores Bun 2 and sorts numerically" 1.10.1 bun newest 1
pm_case "bun without a major takes the newest major" 2.0.1 bun newest
expect "resolve-pm-version: a non-numeric major" 2 resolve bun newest one
expect "resolve-pm-version: unknown package manager" 2 resolve deno newest
expect "resolve-pm-version: unknown slot" 2 resolve npm oldest

# ---------------------------------------------------------------- e2e/lib.sh: tables
same "command of yarn-classic" yarn "$(e2e_command yarn-classic)"
same "command of bun" bun "$(e2e_command bun)"
expect "command of an unknown manager" 2 e2e_command deno
same "linkers of pnpm" "isolated hoisted" "$(e2e_linkers pnpm)"
same "linkers of yarn" "pnp node-modules pnpm" "$(e2e_linkers yarn)"
same "linkers of bun" "default hoisted isolated" "$(e2e_linkers bun)"
same "linkers of yarn-classic" hoisted "$(e2e_linkers yarn-classic)"
expect "linkers of an unknown manager" 2 e2e_linkers deno
for pm in npm pnpm yarn yarn-classic bun; do
  for linker in $(e2e_linkers "$pm"); do
    expect "layout of ${pm}/${linker}" 0 e2e_layout "$pm" "$linker"
  done
done
same "layout of pnpm/isolated" "no node_modules/.pnpm" "$(e2e_layout pnpm isolated)"
same "layout of yarn/pnp" "no .pnp.cjs" "$(e2e_layout yarn pnp)"
same "layout of bun/default" "no node_modules/.bun" "$(e2e_layout bun default)"
same "layout of pnpm/hoisted" "yes -" "$(e2e_layout pnpm hoisted)"
expect "layout of a linker the manager lacks" 2 e2e_layout npm pnp

# ---------------------------------------------------------------- e2e/lib.sh: generated projects
# A synthetic scenario: a workspace root, a nested root, a catalog and overlays for two managers.
scenario="${work}/scenario"
mkdir -p "${scenario}/project/packages/a" "${scenario}/project/nested" "${scenario}/overlays/yarn-classic" \
  "${scenario}/overlays/npm/packages/a"
cat >"${scenario}/scenario.json" <<'JSON'
{
  "managers": ["npm", "pnpm", "yarn", "yarn-classic", "bun"],
  "requires": {"yarn": "4.10.0"},
  "roots": [".", "nested"],
  "catalog": {"is-number": "7.0.0", "@scope/pkg": "^1.0.0"},
  "expect": {
    "exit_code": 1,
    "diagnostics": ["b-rule:root", "a-rule:@e2e/a:dep"],
    "after_fix": {"exit_code": 0, "diagnostics": []},
    "migrate": {"exit_code": 0, "diagnostics": ["c-rule:root"]}
  },
  "overrides": {
    "pnpm": {"reason": "a note", "add": ["d-rule:root"], "after_fix": {"exit_code": 1, "add": ["e-rule:root"]}},
    "pnpm/hoisted": {"remove": ["a-rule:@e2e/a:dep"], "migrate": {"remove": ["c-rule:root"]}},
    "bun/default": {"exit_code": 0},
    "npm": {"remove": ["no-such-rule:root"]}
  }
}
JSON
echo '{"name":"root","version":"0.0.0","workspaces":["packages/*"]}' >"${scenario}/project/package.json"
echo '{"name":"nested","version":"0.0.0","workspaces":{"packages":["libs/*"]}}' >"${scenario}/project/nested/package.json"
echo '{"name":"@e2e/a","version":"0.0.0","devDependencies":{}}' >"${scenario}/project/packages/a/package.json"
echo '{"private":true,"workspaces":null}' >"${scenario}/overlays/yarn-classic/package.merge.json"
echo 'registry=https://example.invalid/' >"${scenario}/overlays/npm/.npmrc"
echo '{"devDependencies":null,"main":"index.js"}' >"${scenario}/overlays/npm/packages/a/package.merge.json"

# generate <pm> <linker> <version>: generates the synthetic scenario into ${dir}.
generate() {
  dir="${work}/gen-$1-$2"
  e2e_generate "$scenario" "$1" "$2" "$3" "$dir" >"${work}/out" 2>&1 || fail "generate $1/$2: $(cat "${work}/out")"
}
has_line() {
  grep -qxF -- "$2" "$3" || fail "$1: $3 lacks the line '$2'"
}
nl=$'\n'

generate pnpm isolated 12.1.0
same "pnpm pins the version under test" pnpm@12.1.0 "$(jqr -r .packageManager "${dir}/package.json")"
same "pnpm drops workspaces from package.json" null "$(jqr -c .workspaces "${dir}/package.json")"
same "pnpm writes members, linker and catalog to pnpm-workspace.yaml" \
  "packages:${nl}  - 'packages/*'${nl}nodeLinker: isolated${nl}catalog:${nl}  \"is-number\": \"7.0.0\"${nl}  \"@scope/pkg\": \"^1.0.0\"" \
  "$(cat "${dir}/pnpm-workspace.yaml")"
same "pnpm reads the object form of workspaces in a nested root" \
  "packages:${nl}  - 'libs/*'${nl}nodeLinker: isolated" "$(cat "${dir}/nested/pnpm-workspace.yaml")"
same "every root is pinned" pnpm@12.1.0 "$(jqr -r .packageManager "${dir}/nested/package.json")"
generate pnpm hoisted 12.1.0
has_line "pnpm hoisted" "nodeLinker: hoisted" "${dir}/pnpm-workspace.yaml"

generate yarn pnp 4.10.0
same "yarn pnp writes an empty lockfile" "" "$(cat "${dir}/yarn.lock")"
same "yarn writes the catalog to .yarnrc.yml and no linker for pnp" \
  "catalog:${nl}  \"is-number\": \"7.0.0\"${nl}  \"@scope/pkg\": \"^1.0.0\"" "$(cat "${dir}/.yarnrc.yml")"
[[ ! -e "${dir}/nested/.yarnrc.yml" ]] || fail "yarn pnp: a .yarnrc.yml in the nested root"
[[ -f "${dir}/nested/yarn.lock" ]] || fail "yarn pnp: no lockfile in the nested root"
same "yarn keeps workspaces" '["packages/*"]' "$(jqr -c .workspaces "${dir}/package.json")"
generate yarn node-modules 4.10.0
same "yarn node-modules sets the linker in the nested root" "nodeLinker: node-modules" "$(cat "${dir}/nested/.yarnrc.yml")"

generate bun default 1.3.0
[[ ! -e "${dir}/bunfig.toml" ]] || fail "bun default: a bunfig.toml"
same "bun writes the catalog into workspaces" \
  '{"packages":["packages/*"],"catalog":{"is-number":"7.0.0","@scope/pkg":"^1.0.0"}}' \
  "$(jqr -c .workspaces "${dir}/package.json")"
generate bun isolated 1.3.0
same "bun isolated sets the linker in bunfig.toml" "[install]${nl}linker = \"isolated\"" "$(cat "${dir}/bunfig.toml")"

expect "npm has no catalogs" 1 e2e_generate "$scenario" npm hoisted 11.0.0 "${work}/gen-npm"
expect_in_out "npm has no catalogs" "npm has no catalogs"
same "the npm overlay copies a plain file" "registry=https://example.invalid/" "$(cat "${work}/gen-npm/.npmrc")"
same "a merge overlay deep-merges and a null deletes the key" '{"name":"@e2e/a","version":"0.0.0","main":"index.js"}' \
  "$(jqr -c . "${work}/gen-npm/packages/a/package.json")"

jq 'del(.catalog)' "${scenario}/scenario.json" >"${work}/no-catalog.json"
cp "${work}/no-catalog.json" "${scenario}/scenario.json"
generate yarn-classic hoisted 1.22.22
same "yarn-classic pins yarn 1" yarn@1.22.22 "$(jqr -r .packageManager "${dir}/package.json")"
same "the yarn-classic overlay makes the root private and drops workspaces" \
  '{"name":"root","version":"0.0.0","private":true,"packageManager":"yarn@1.22.22"}' "$(jqr -c . "${dir}/package.json")"
[[ ! -e "${dir}/yarn.lock" && ! -e "${dir}/.yarnrc.yml" ]] || fail "yarn-classic: Berry files written"
expect "a merge overlay into a missing file" 1 e2e_overlay "${scenario}/overlays/npm" "${work}/empty-dest"

# ---------------------------------------------------------------- e2e/lib.sh: applicability and expectations
same "applies to a listed manager" "" "$(e2e_applies "$scenario" pnpm 10.0.0)"
same "a minimum version compares numerically" "" "$(e2e_applies "$scenario" yarn 4.10.0)"
same "below the minimum version" "needs yarn 4.10.0 or later" "$(e2e_applies "$scenario" yarn 4.9.4)"
jq '.managers = ["pnpm"]' "${scenario}/scenario.json" >"${work}/pnpm-only.json"
mkdir -p "${work}/pnpm-only"
cp "${work}/pnpm-only.json" "${work}/pnpm-only/scenario.json"
same "not a listed manager" "not a bun scenario" "$(e2e_applies "${work}/pnpm-only" bun 1.3.0)"

same "expectation without an override" \
  '{"exit_code":1,"diagnostics":["a-rule:@e2e/a:dep","b-rule:root"],"after_fix":{"exit_code":0,"diagnostics":[]},"migrate":{"exit_code":0,"diagnostics":["c-rule:root"]}}' \
  "$(e2e_expectation "$scenario" yarn pnp | tr -d '\r')"
same "manager and linker overrides add, remove and reach the steps" \
  '{"exit_code":1,"diagnostics":["b-rule:root","d-rule:root"],"after_fix":{"exit_code":1,"diagnostics":["e-rule:root"]},"migrate":{"exit_code":0,"diagnostics":[]}}' \
  "$(e2e_expectation "$scenario" pnpm hoisted | tr -d '\r')"
same "an override sets the exit code" 0 "$(e2e_expectation "$scenario" bun default | jqr .exit_code)"
expect "an override removing an absent finding is an error" 5 e2e_expectation "$scenario" npm hoisted
expect_in_out "stale override" "override removes absent findings: no-such-rule:root"

same "findings name the rule, package, dependency and a file that is not a manifest" \
  '["a:@e2e/a:dep","b:root","c:@e2e/b in packages/b/package-lock.json"]' \
  "$(e2e_findings '{"diagnostics":[
    {"rule":"c","package":"@e2e/b","location":{"path":"packages/b/package-lock.json"}},
    {"rule":"b","package":"root","location":{"path":"package.json"}},
    {"rule":"a","package":"@e2e/a","dependency":"dep","location":{"path":"packages/a/package.json"}}]}')"

# ---------------------------------------------------------------- every committed scenario
# Each scenario generates for every manager and linker it applies to, and its expectation evaluates, so a broken
# overlay or a stale override fails here before it costs a package-manager install in CI.
scenarios=0
for fixture in fixtures/e2e/*/; do
  fixture="${fixture%/}"
  [[ -f "${fixture}/scenario.json" ]] || {
    fail "${fixture} has no scenario.json"
    continue
  }
  scenarios=$((scenarios + 1))
  for pm in npm pnpm yarn yarn-classic bun; do
    [[ -z "$(e2e_applies "$fixture" "$pm" 99.0.0)" ]] || continue
    for linker in $(e2e_linkers "$pm"); do
      dest="${work}/fixtures/$(basename "$fixture")-${pm}-${linker}"
      expect "${fixture} generates for ${pm}/${linker}" 0 e2e_generate "$fixture" "$pm" "$linker" 99.0.0 "$dest"
      expect "${fixture} expectation for ${pm}/${linker}" 0 e2e_expectation "$fixture" "$pm" "$linker"
    done
  done
done
[[ "$scenarios" -gt 0 ]] || fail "no scenarios under fixtures/e2e"

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "e2e scripts: all cases passed"
