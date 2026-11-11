#!/usr/bin/env bash
# Tests the repository guards that decide what ships, each on a synthetic tree, so a pattern that drifts cannot make a
# guard pass on nothing: every script gets a passing tree and the negative cases that must fail.
#   check-pins, check-headers, check-rule-ids, check-plugin-list, check-dependency-tree, check-package-contents,
#   check-schema, check-oxc-git-patch, check-doc-pins, npm-smoke.sh
#
# Usage: scripts/test-check-scripts.sh
# The literals checked below contain `$` on purpose; nothing is meant to expand.
# shellcheck disable=SC2016
set -euo pipefail

cd "$(dirname "$0")/.."
repo="$PWD"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

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

# new_tree <name>: an empty tree holding a copy of the scripts; prints its path.
new_tree() {
  local tree="${work}/$1"
  mkdir -p "${tree}/scripts"
  cp "${repo}"/scripts/*.sh "${tree}/scripts/"
  echo "$tree"
}

# run_in <tree> <script> [args...]: runs the copy of the script inside its tree, with the stubs of ${stubs} first on PATH.
stubs="${work}/stubs"
mkdir -p "$stubs"
export PATH="${stubs}:${PATH}"
run_in() {
  local tree="$1" script="$2"
  shift 2
  bash "${tree}/scripts/${script}" "$@"
}

# ---------------------------------------------------------------- check-pins
pins="$(new_tree pins)"
mkdir -p "${pins}/crates/a" "${pins}/tools"
cat >"${pins}/Cargo.toml" <<'TOML'
[workspace.dependencies]
oxc_parser = "0.1.0"
oxc_span = { version = "0.1.0", default-features = false }
oxc_resolver = "11.0.0"
serde = "1"
TOML
cat >"${pins}/crates/a/Cargo.toml" <<'TOML'
[package]
name = "a"

[dependencies]
serde.workspace = true
oxc_parser = { workspace = true }
TOML
expect "check-pins accepts caret ranges and inherited dependencies" 0 run_in "$pins" check-pins.sh

sed -i.bak 's/oxc_parser = "0.1.0"/oxc_parser = "=0.1.0"/' "${pins}/Cargo.toml"
expect "check-pins rejects an exact oxc pin" 1 run_in "$pins" check-pins.sh
expect_in_out "check-pins exact" "oxc dependency is not a caret range"
mv "${pins}/Cargo.toml.bak" "${pins}/Cargo.toml"

sed -i.bak 's/oxc_parser = "0.1.0"/oxc_parser = "0.2.0"/' "${pins}/Cargo.toml"
expect "check-pins rejects oxc crates on two releases" 1 run_in "$pins" check-pins.sh
expect_in_out "check-pins lockstep" "the oxc crates must name one release"
mv "${pins}/Cargo.toml.bak" "${pins}/Cargo.toml"

printf 'tokio = "1"\n' >>"${pins}/crates/a/Cargo.toml"
expect "check-pins rejects a dependency that is not inherited" 1 run_in "$pins" check-pins.sh
expect_in_out "check-pins inherited" "not inherited from the workspace"

# ---------------------------------------------------------------- check-headers
headers="$(new_tree headers)"
mkdir -p "${headers}/crates/skif-specifier/src" "${headers}/.ai-rulez"
cp ATTRIBUTIONS.md mise.toml "${headers}/"
cp .ai-rulez/config.toml "${headers}/.ai-rulez/config.toml"
commit="$(sed -n '/^## syncpack specifier$/,/^## knip$/p' ATTRIBUTIONS.md | sed -nE 's/^- \*\*Upstream commit\*\*: `([0-9a-f]{40})`.*/\1/p' | head -n 1)"
[[ -n "$commit" ]] || fail "check-headers: ATTRIBUTIONS.md records no syncpack commit"
printf '// Vendored from syncpack 15.0.0 @%s (src/a.ts)\nfn a() {}\n' "${commit:0:12}" >"${headers}/crates/skif-specifier/src/a.rs"
printf '// Added for skif (not in syncpack): b\nfn b() {}\n' >"${headers}/crates/skif-specifier/src/b.rs"
expect "check-headers accepts headed files" 0 run_in "$headers" check-headers.sh

printf 'fn c() {}\n' >"${headers}/crates/skif-specifier/src/c.rs"
expect "check-headers rejects a file without a header" 1 run_in "$headers" check-headers.sh
expect_in_out "check-headers missing header" "missing attribution header"
rm "${headers}/crates/skif-specifier/src/c.rs"

printf '// Vendored from syncpack 15.0.0 @%s (src/d.ts)\nfn d() {}\n' "0000000000000000" >"${headers}/crates/skif-specifier/src/d.rs"
expect "check-headers rejects a header naming another commit" 1 run_in "$headers" check-headers.sh
expect_in_out "check-headers commit" "differs from the syncpack commit"
rm "${headers}/crates/skif-specifier/src/d.rs"

grep -v 'Lars Kappert' ATTRIBUTIONS.md >"${headers}/ATTRIBUTIONS.md"
expect "check-headers rejects an ATTRIBUTIONS.md without the knip licence holder" 1 run_in "$headers" check-headers.sh
expect_in_out "check-headers knip" "the knip section is missing"
cp ATTRIBUTIONS.md "${headers}/ATTRIBUTIONS.md"

sed -i.bak 's/^\(package = "pkg:github\/Goldziher\/poly@v\)[^"]*/\10.0.1/' "${headers}/.ai-rulez/config.toml"
expect "check-headers rejects a poly version that differs from mise.toml" 1 run_in "$headers" check-headers.sh
expect_in_out "check-headers poly" "differs from mise.toml"

# ---------------------------------------------------------------- check-rule-ids
rules="$(new_tree rules)"
mkdir -p "${rules}/crates/skif-rules/src" "${rules}/docs" "${rules}/website/src/content" "${rules}/.ai-rulez/context"
cat >"${rules}/crates/skif-rules/src/lib.rs" <<'RS'
pub static ALPHA: RuleMeta = RuleMeta::new(
    "alpha",
    Severity::Error,
);
pub(crate) static BETA: RuleMeta = RuleMeta::new("beta", Severity::Warn);
RS
cat >"${rules}/docs/rules.md" <<'MD'
| Rule | Group |
| [`alpha`](#alpha) | core |
| [`beta`](#beta) | core |
MD
printf 'alpha\nbeta\n' >"${rules}/rule-ids.txt"
printf 'skif ships 2 rules.\n' >"${rules}/README.md"
expect "check-rule-ids accepts matching registry, docs and list" 0 run_in "$rules" check-rule-ids.sh

printf 'alpha\nbeta\ngamma\n' >"${rules}/rule-ids.txt"
expect "check-rule-ids rejects a recorded rule that is neither documented nor registered" 1 run_in "$rules" check-rule-ids.sh
expect_in_out "check-rule-ids recorded" "rule 'gamma' is recorded but no longer documented"
expect_in_out "check-rule-ids recorded in code" "rule 'gamma' is recorded but not registered in code"

printf 'alpha\n' >"${rules}/rule-ids.txt"
expect "check-rule-ids rejects a registered rule missing from the list" 1 run_in "$rules" check-rule-ids.sh
expect_in_out "check-rule-ids registered" "rule 'beta' is registered in code but missing from rule-ids.txt"
printf 'alpha\nbeta\n' >"${rules}/rule-ids.txt"

printf 'skif ships 3 rules.\n' >"${rules}/README.md"
expect "check-rule-ids rejects a wrong rule count in the docs" 1 run_in "$rules" check-rule-ids.sh
expect_in_out "check-rule-ids count" "states 3 rules"
printf 'skif ships 2 rules.\n' >"${rules}/README.md"
mkdir -p "${rules}/skills/skif"
printf 'skif ships 3 rules.\n' >"${rules}/skills/skif/SKILL.md"
expect "check-rule-ids rejects a wrong rule count in an agent skill" 1 run_in "$rules" check-rule-ids.sh
expect_in_out "check-rule-ids skill count" "file=skills/skif/SKILL.md,line=1::states 3 rules"
rm -r "${rules}/skills"

printf 'pub fn unrelated() {}\n' >"${rules}/crates/skif-rules/src/lib.rs"
expect "check-rule-ids fails when the registry pattern matches nothing" 1 run_in "$rules" check-rule-ids.sh
expect_in_out "check-rule-ids empty registry" "no RuleMeta statics found"

# ---------------------------------------------------------------- check-plugin-list
plugins="$(new_tree plugins)"
mkdir -p "${plugins}/crates/skif-js-plugins/src/plugins" "${plugins}/website/src/content/docs/reference"
printf 'pub fn p() -> Plugin {\n    Plugin {\n        id: "alpha",\n    }\n}\n' >"${plugins}/crates/skif-js-plugins/src/plugins/alpha.rs"
# A plugin may name its id through its `ID` constant.
printf 'pub const ID: crate::plugin::PluginId = "beta";\n\npub fn p() -> Plugin {\n    Plugin {\n        id: ID,\n    }\n}\n' >"${plugins}/crates/skif-js-plugins/src/plugins/beta.rs"
printf 'The table holds 2 plugins:\n\n`alpha`, `beta`\n' >"${plugins}/crates/skif-js-plugins/README.md"
printf 'The table holds 2 built-in plugins:\n`alpha`, `beta`\n' >"${plugins}/website/src/content/docs/reference/configuration.mdx"
printf 'skif has 2 plugins.\n' >"${plugins}/README.md"
printf 'Unreleased\n' >"${plugins}/CHANGELOG.md"
expect "check-plugin-list accepts matching lists" 0 run_in "$plugins" check-plugin-list.sh

printf 'The table holds 2 plugins:\n\n`alpha`\n' >"${plugins}/crates/skif-js-plugins/README.md"
expect "check-plugin-list rejects a plugin missing from the README" 1 run_in "$plugins" check-plugin-list.sh
expect_in_out "check-plugin-list missing" "plugin 'beta' is defined in crates/skif-js-plugins but not listed"
printf 'The table holds 2 plugins:\n\n`alpha`, `beta`, `gamma`\n' >"${plugins}/crates/skif-js-plugins/README.md"
expect "check-plugin-list rejects a listed plugin that is not defined" 1 run_in "$plugins" check-plugin-list.sh
expect_in_out "check-plugin-list extra" "plugin 'gamma' is listed but not defined"
printf 'The table holds 2 plugins:\n\n`alpha`, `beta`\n' >"${plugins}/crates/skif-js-plugins/README.md"

printf 'skif has 5 plugins.\n' >"${plugins}/README.md"
expect "check-plugin-list rejects a wrong plugin count" 1 run_in "$plugins" check-plugin-list.sh
expect_in_out "check-plugin-list count" "states 5 plugins"
printf 'skif has 2 plugins.\n' >"${plugins}/README.md"

printf 'No list here.\n' >"${plugins}/website/src/content/docs/reference/configuration.mdx"
expect "check-plugin-list rejects a missing list" 1 run_in "$plugins" check-plugin-list.sh
expect_in_out "check-plugin-list absent list" "no plugin list found"
printf 'The table holds 2 built-in plugins:\n`alpha`, `beta`\n' >"${plugins}/website/src/content/docs/reference/configuration.mdx"

rm "${plugins}"/crates/skif-js-plugins/src/plugins/*.rs
printf 'pub fn nothing() {}\n' >"${plugins}/crates/skif-js-plugins/src/plugins/empty.rs"
expect "check-plugin-list fails when no plugin id is found" 1 run_in "$plugins" check-plugin-list.sh
expect_in_out "check-plugin-list no ids" "no plugin ids found"

# ---------------------------------------------------------------- check-dependency-tree
deps="$(new_tree deps)"
cat >"${stubs}/cargo" <<'STUB'
#!/usr/bin/env bash
# `cargo tree` answers from fixtures named by the arguments: STUB_TREE_<kind> holds the output, STUB_TREE_FAIL_<crate> an error.
args=" $* "
if [[ "$args" == *" --invert "* ]]; then
  crate="$(sed -E 's/.* --invert ([a-z_]+) .*/\1/' <<<"$args")"
  fail_var="STUB_TREE_FAIL_${crate}"
  if [[ -n "${!fail_var:-}" ]]; then
    echo "${!fail_var}" >&2
    exit 101
  fi
  var="STUB_TREE_INVERT_${crate}"
  printf '%s\n' "${!var:-}"
elif [[ "$args" == *" -p skif-core "* ]]; then
  printf '%s\n' "${STUB_TREE_CORE:-skif-core v0.0.0}"
else
  printf '%s\n' "${STUB_TREE_FACADE:-skif v0.0.0}"
fi
STUB
chmod +x "${stubs}/cargo"
expect "check-dependency-tree accepts the layering" 0 run_in "$deps" check-dependency-tree.sh

expect "check-dependency-tree rejects an oxc crate behind default-less skif" 1 env STUB_TREE_FACADE=$'skif v0.0.0\noxc_parser v0.1.0' \
  bash "${deps}/scripts/check-dependency-tree.sh"
expect_in_out "check-dependency-tree facade" "without default features depends on an oxc crate"
expect "check-dependency-tree rejects skif-core depending on a workspace crate" 1 env STUB_TREE_CORE=$'skif-core v0.0.0\nskif-rules v0.0.0' \
  bash "${deps}/scripts/check-dependency-tree.sh"
expect_in_out "check-dependency-tree core" "skif-core depends on the workspace crate skif-rules"
expect "check-dependency-tree rejects a crate that may not use oxc" 1 env STUB_TREE_INVERT_oxc_ast=$'oxc_ast v0.1.0\nskif-js v0.0.0\nskif-lint v0.0.0' \
  bash "${deps}/scripts/check-dependency-tree.sh"
expect_in_out "check-dependency-tree oxc" "skif-lint depends on oxc_ast"
expect "check-dependency-tree allows the js crates" 0 env STUB_TREE_INVERT_oxc_ast=$'oxc_ast v0.1.0\nskif-js v0.0.0\nskif-js-syntax v0.0.0' \
  bash "${deps}/scripts/check-dependency-tree.sh"
expect "check-dependency-tree reports a crate absent from the graph as a warning" 0 env STUB_TREE_FAIL_oxc_str="package ID specification did not match any packages" \
  bash "${deps}/scripts/check-dependency-tree.sh"
expect_in_out "check-dependency-tree absent crate" "oxc_str is not in the dependency graph"
expect "check-dependency-tree fails when cargo tree fails" 1 env STUB_TREE_FAIL_oxc_str="registry unreachable" \
  bash "${deps}/scripts/check-dependency-tree.sh"
expect_in_out "check-dependency-tree failure" "cargo tree failed for oxc_str"

# ---------------------------------------------------------------- check-package-contents
pkg="$(new_tree pkg)"
mkdir -p "${pkg}/crates/pub" "${pkg}/crates/private" "${pkg}/npm-package"
echo licence >"${pkg}/LICENSE"
echo notices >"${pkg}/ATTRIBUTIONS.md"
for dir in crates/pub npm-package; do
  cp "${pkg}/LICENSE" "${pkg}/ATTRIBUTIONS.md" "${pkg}/${dir}/"
done
printf '[package]\nname = "pub"\n' >"${pkg}/crates/pub/Cargo.toml"
printf '[package]\nname = "private"\npublish = false\n' >"${pkg}/crates/private/Cargo.toml"
cat >"${stubs}/cargo" <<'STUB'
#!/usr/bin/env bash
# `cargo package -p <crate> --list`: the files named by STUB_PACKAGE_LIST, one per line.
printf '%s\n' ${STUB_PACKAGE_LIST:-}
STUB
expect "check-package-contents accepts identical notices that ship" 0 env STUB_PACKAGE_LIST="Cargo.toml LICENSE ATTRIBUTIONS.md" \
  bash "${pkg}/scripts/check-package-contents.sh"

expect "check-package-contents rejects a package that omits a notice" 1 env STUB_PACKAGE_LIST="Cargo.toml LICENSE" \
  bash "${pkg}/scripts/check-package-contents.sh"
expect_in_out "check-package-contents omitted" "the pub package does not contain ATTRIBUTIONS.md"

echo stale >"${pkg}/crates/pub/ATTRIBUTIONS.md"
expect "check-package-contents rejects a stale crate copy" 1 env STUB_PACKAGE_LIST="LICENSE ATTRIBUTIONS.md" \
  bash "${pkg}/scripts/check-package-contents.sh"
expect_in_out "check-package-contents stale crate" "pub must carry a copy of the root ATTRIBUTIONS.md"
cp "${pkg}/ATTRIBUTIONS.md" "${pkg}/crates/pub/"

rm "${pkg}/npm-package/LICENSE"
expect "check-package-contents rejects an npm package without the licence" 1 env STUB_PACKAGE_LIST="LICENSE ATTRIBUTIONS.md" \
  bash "${pkg}/scripts/check-package-contents.sh"
expect_in_out "check-package-contents npm" "npm-package must carry a copy of the root LICENSE"

# ---------------------------------------------------------------- check-schema
schema="$(new_tree schema)"
mkdir -p "${schema}/schema"
printf '{"title":"skif"}\n' >"${schema}/schema/skif.schema.json"
printf '#!/usr/bin/env bash\n[[ "$1" == schema ]] && printf %%s "${STUB_SCHEMA:-{\\"title\\":\\"skif\\"\\}}"\n' >"${work}/skif-bin"
chmod +x "${work}/skif-bin"
expect "check-schema accepts a current schema" 0 bash "${schema}/scripts/check-schema.sh" "${work}/skif-bin"
expect "check-schema rejects a stale schema" 1 env STUB_SCHEMA='{"title":"other"}' bash "${schema}/scripts/check-schema.sh" "${work}/skif-bin"
expect_in_out "check-schema stale" "is stale"

# ---------------------------------------------------------------- check-oxc-git-patch
oxc="$(new_tree oxc)"
cp Cargo.lock "${oxc}/Cargo.lock"
printf '[workspace.dependencies]\noxc_parser = "0.99.0"\n' >"${oxc}/Cargo.toml"
cat >"${stubs}/git" <<'STUB'
#!/usr/bin/env bash
[[ "$1" == ls-remote ]] && printf '%s\n' "${STUB_LS_REMOTE:-}"
STUB
chmod +x "${stubs}/git"
printf '#!/usr/bin/env bash\necho "Patch oxc_parser was not used in the crate graph"\n' >"${stubs}/cargo"

expect "check-oxc-git-patch needs a readable parser pin" 1 env bash -c 'cd "$1" && : >Cargo.toml && bash scripts/check-oxc-git-patch.sh' _ "$oxc"
expect_in_out "check-oxc-git-patch pin" "cannot read the oxc_parser version"
printf '[workspace.dependencies]\noxc_parser = "0.99.0"\n' >"${oxc}/Cargo.toml"

expect "check-oxc-git-patch rejects an unknown tag" 1 env STUB_LS_REMOTE="" bash "${oxc}/scripts/check-oxc-git-patch.sh"
expect_in_out "check-oxc-git-patch tag" "tag crates_v0.99.0 not found"
expect "check-oxc-git-patch rejects a malformed revision" 1 env STUB_LS_REMOTE="nothex refs/tags/x" bash "${oxc}/scripts/check-oxc-git-patch.sh"
expect "check-oxc-git-patch fails when a patch is not used" 1 env STUB_LS_REMOTE="$(printf 'a%.0s' {1..40}) refs/tags/x" \
  bash "${oxc}/scripts/check-oxc-git-patch.sh"
expect_in_out "check-oxc-git-patch unused" "a patch was not used"
cmp -s Cargo.lock "${oxc}/Cargo.lock" || fail "check-oxc-git-patch: Cargo.lock was not restored"

# ---------------------------------------------------------------- check-doc-pins
docpins="$(new_tree docpins)"
cp -R "${repo}/scripts/lib" "${docpins}/scripts/"
mkdir -p "${docpins}/website/src/content/docs/guides" "${docpins}/adrs"
printf '[workspace.package]\nversion = "0.4.2"\n' >"${docpins}/Cargo.toml"
# doc_pins_tree <action pin> <pre-commit rev> <poly revision>: writes one pin of each kind.
doc_pins_tree() {
  printf -- '- uses: Goldziher/skif@%s\n' "$1" >"${docpins}/README.md"
  printf 'repos:\n  - repo: https://github.com/Goldziher/skif\n    rev: %s # use the latest release tag\n' "$2" \
    >"${docpins}/website/src/content/docs/guides/hooks.mdx"
  printf '[[hooks.sources]]\ngit = "https://github.com/Goldziher/skif.git"\nrevision = "%s"\n' "$3" \
    >"${docpins}/website/src/content/docs/guides/poly.md"
}
doc_pins_tree v0.4 v0.4.0 v0.4.0
# History may name any release, and a `rev:` that belongs to another repository is not a skif pin.
printf -- '- uses: Goldziher/skif@v0.1\n' >"${docpins}/CHANGELOG.md"
printf -- '- uses: Goldziher/skif@v0.1\n' >"${docpins}/adrs/0001-old.md"
printf 'repos:\n  - repo: https://github.com/example/other\n    rev: v0.1.0\n' >"${docpins}/website/src/content/docs/guides/other.mdx"
expect "check-doc-pins accepts pins on the workspace line" 0 run_in "$docpins" check-doc-pins.sh
expect_in_out "check-doc-pins accepts" "Every documentation pin names the v0.4 line of 0.4.2"
expect "check-doc-pins accepts an explicit version on the same line" 0 run_in "$docpins" check-doc-pins.sh 0.4.9
expect "check-doc-pins rejects pins behind an explicit version" 1 run_in "$docpins" check-doc-pins.sh 0.5.0
expect_in_out "check-doc-pins explicit version" "3 documentation pin(s) not on the release line of 0.5.0"

doc_pins_tree v0.3 v0.4.0 v0.4.0
expect "check-doc-pins rejects a stale Action pin" 1 run_in "$docpins" check-doc-pins.sh
expect_in_out "check-doc-pins Action" "file=README.md,line=1::skif pin v0.3 is not on the 0.4.2 release line (v0.4)"
doc_pins_tree v0 v0.4.0 v0.4.0
expect "check-doc-pins rejects the bare v0 tag, which never moves" 1 run_in "$docpins" check-doc-pins.sh
expect_in_out "check-doc-pins v0" "skif pin v0 is not on the 0.4.2 release line"
doc_pins_tree v0.4 v0.3.1 v0.4.0
expect "check-doc-pins rejects a stale pre-commit rev" 1 run_in "$docpins" check-doc-pins.sh
expect_in_out "check-doc-pins rev" "file=website/src/content/docs/guides/hooks.mdx,line=3::skif pin v0.3.1"
doc_pins_tree v0.4 v0.4.0 v0.3.0
expect "check-doc-pins rejects a stale Poly revision" 1 run_in "$docpins" check-doc-pins.sh
expect_in_out "check-doc-pins revision" "file=website/src/content/docs/guides/poly.md,line=3::skif pin v0.3.0"
expect "check-doc-pins only lists the pins of a pre-release" 0 run_in "$docpins" check-doc-pins.sh 0.5.0-rc.1
expect_in_out "check-doc-pins pre-release" "website/src/content/docs/guides/poly.md:3:v0.3.0"
doc_pins_tree v0.4 v0.4.0 v0.4.0
mkdir -p "${docpins}/skills/skif/references"
printf -- '- uses: Goldziher/skif@v0.3\n' >"${docpins}/skills/skif/references/ci.md"
expect "check-doc-pins rejects a stale pin in an agent skill" 1 run_in "$docpins" check-doc-pins.sh
expect_in_out "check-doc-pins skill" "file=skills/skif/references/ci.md,line=1::skif pin v0.3"
rm -r "${docpins}/skills"

printf '[workspace.package]\nversion = "1.2.0"\n' >"${docpins}/Cargo.toml"
doc_pins_tree v1 v1.0.0 v1.1.3
expect "check-doc-pins compares a stable release by major" 0 run_in "$docpins" check-doc-pins.sh
doc_pins_tree v0.9 v1.0.0 v1.1.3
expect "check-doc-pins rejects a 0.x pin after 1.0.0" 1 run_in "$docpins" check-doc-pins.sh

rm "${docpins}/README.md" "${docpins}/website/src/content/docs/guides/hooks.mdx" "${docpins}/website/src/content/docs/guides/poly.md"
expect "check-doc-pins fails when no pin is found" 1 run_in "$docpins" check-doc-pins.sh
expect_in_out "check-doc-pins no pins" "no skif version pins found"

# ---------------------------------------------------------------- npm-smoke
smoke="$(new_tree smoke)"
mkdir -p "${smoke}/npm-package" "${smoke}/scripts/e2e" "${smoke}/fixtures/e2e"
cp "${repo}/scripts/e2e/lib.sh" "${smoke}/scripts/e2e/"
cp -R "${repo}/fixtures/e2e/clean" "${smoke}/fixtures/e2e/"
printf '{"version":"1.2.3"}\n' >"${smoke}/npm-package/package.json"
# The addon script: passes when it is given the clean scenario generated as an npm workspace.
cat >"${stubs}/node" <<'STUB'
#!/usr/bin/env bash
jq -e '.workspaces == ["packages/*"] and (.packageManager | startswith("npm@"))' "$2/package.json" >/dev/null &&
  [[ -f "$2/packages/a/package.json" && -f "$2/packages/b/package.json" ]]
STUB
cat >"${stubs}/npx" <<'STUB'
#!/usr/bin/env bash
# The skif bin: --version prints STUB_SMOKE_VERSION, a lint prints a report, an unknown flag exits STUB_SMOKE_USAGE_STATUS.
case "$*" in
*--no-such-flag*) exit "${STUB_SMOKE_USAGE_STATUS:-2}" ;;
*--version*) echo "skif ${STUB_SMOKE_VERSION:-1.2.3}" ;;
*) echo '{"summary":{"errors":0}}' ;;
esac
STUB
chmod +x "${stubs}/node" "${stubs}/npx"
expect "npm-smoke runs a clean node smoke test" 0 run_in "$smoke" npm-smoke.sh node
expect "npm-smoke rejects an unknown runtime" 1 run_in "$smoke" npm-smoke.sh deno
expect_in_out "npm-smoke runtime" "runtime must be node or bun"
expect "npm-smoke fails on the wrong bin version" 1 env STUB_SMOKE_VERSION=0.0.1 bash "${smoke}/scripts/npm-smoke.sh" node
expect_in_out "npm-smoke version" "expected version 1.2.3"
expect "npm-smoke fails when the usage error does not exit 2" 1 env STUB_SMOKE_USAGE_STATUS=1 bash "${smoke}/scripts/npm-smoke.sh" node
expect_in_out "npm-smoke exit code" "expected exit 2"
mv "${smoke}/fixtures/e2e/clean" "${smoke}/clean"
expect "npm-smoke fails when the clean scenario is missing" 1 run_in "$smoke" npm-smoke.sh node
mv "${smoke}/clean" "${smoke}/fixtures/e2e/clean"
printf '#!/usr/bin/env bash\nexit 1\n' >"${stubs}/node"
expect "npm-smoke fails when the addon script fails" 1 run_in "$smoke" npm-smoke.sh node

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "check scripts: all cases passed"
