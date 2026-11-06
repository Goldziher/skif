#!/usr/bin/env bash
# Tests the release scripts against stubs and fixtures, so the first real tag is not their first run:
#   homebrew-update.sh, scoop-update.sh   formula and manifest output from a checksums.txt (missing asset, bad hash)
#   publish-crates.sh                     crates.io probe 200 / 404 / 429 and the publish order, with stub cargo and curl
#   release-bump.sh, check-versions.sh    a bump on a temp copy of the tree, then the consistency check
#   check-archive.sh, check-release-gate.sh, check-npm-tarballs.sh, check-glibc-floor.sh, e2e/resolve-pm-version.sh
#
# Usage: scripts/test-release-scripts.sh
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

# ---------------------------------------------------------------- checksums fixtures
hex() { printf '%064d' "$1"; }
sums="${work}/checksums.txt"
n=0
while IFS= read -r asset; do
  [[ "$asset" == checksums.txt ]] && continue
  n=$((n + 1))
  printf '%s  %s\n' "$(hex "$n")" "$asset" >>"$sums"
done < <(bash scripts/release-assets.sh)

# ---------------------------------------------------------------- homebrew
expect "homebrew formula" 0 bash scripts/homebrew-update.sh 1.2.3 "$sums" Acme/skif
formula="$(bash scripts/homebrew-update.sh 1.2.3 "$sums" Acme/skif)"
for want in 'version "1.2.3"' 'https://github.com/Acme/skif/releases/download/v1.2.3/skif-aarch64-apple-darwin.tar.gz' \
  'releases/download/v1.2.3/skif-x86_64-unknown-linux-musl.tar.gz' 'releases/download/v1.2.3/skif-aarch64-unknown-linux-musl.tar.gz' \
  'OS::Linux::Glibc.system_version >= Version.new("2.34")' \
  "$(hex 1)" "$(hex 2)" "$(hex 3)" "$(hex 4)" "$(hex 5)" "$(hex 6)"; do
  grep -qF -- "$want" <<<"$formula" || fail "homebrew formula lacks '${want}'"
done
if command -v ruby >/dev/null 2>&1; then
  printf '%s\n' "$formula" >"${work}/skif.rb"
  expect "homebrew formula is valid Ruby" 0 ruby -c "${work}/skif.rb"
fi

grep -v 'skif-x86_64-apple-darwin' "$sums" >"${work}/missing-asset.txt"
expect "homebrew missing asset" 1 bash scripts/homebrew-update.sh 1.2.3 "${work}/missing-asset.txt"
expect_in_out "homebrew missing asset" "skif-x86_64-apple-darwin.tar.gz"

sed 's/^[0-9]\{64\}/not-a-hash/' "$sums" >"${work}/bad-hash.txt"
expect "homebrew bad hash" 1 bash scripts/homebrew-update.sh 1.2.3 "${work}/bad-hash.txt"
expect_in_out "homebrew bad hash" "not a sha256"
expect "homebrew no checksums file" 1 bash scripts/homebrew-update.sh 1.2.3 "${work}/nope.txt"
expect "homebrew usage" 1 bash scripts/homebrew-update.sh

# ---------------------------------------------------------------- scoop
if command -v jq >/dev/null 2>&1; then
  manifest="$(bash scripts/scoop-update.sh 1.2.3 "$sums" Acme/skif)"
  [[ "$(jq -r '.version' <<<"$manifest")" == 1.2.3 ]] || fail "scoop manifest version"
  [[ "$(jq -r '.architecture."64bit".url' <<<"$manifest")" == https://github.com/Acme/skif/releases/download/v1.2.3/skif-x86_64-pc-windows-msvc.zip ]] || fail "scoop x64 url"
  [[ "$(jq -r '.architecture."64bit".hash' <<<"$manifest")" == "$(hex 7)" ]] || fail "scoop x64 hash"
  [[ "$(jq -r '.architecture.arm64.hash' <<<"$manifest")" == "$(hex 8)" ]] || fail "scoop arm64 hash"
  [[ "$(jq -r '.autoupdate.hash.url' <<<"$manifest")" == *'/v$version/checksums.txt' ]] || fail "scoop autoupdate hash url"

  grep -v 'windows-msvc.zip' "$sums" >"${work}/no-windows.txt"
  expect "scoop missing asset" 1 bash scripts/scoop-update.sh 1.2.3 "${work}/no-windows.txt"
  expect "scoop bad hash" 1 bash scripts/scoop-update.sh 1.2.3 "${work}/bad-hash.txt"
else
  echo "skip scoop tests: jq is not installed" >&2
fi

# ---------------------------------------------------------------- publish-crates
stubs="${work}/stubs"
mkdir -p "$stubs"
cat >"${stubs}/curl" <<'STUB'
#!/usr/bin/env bash
# Prints the HTTP status the crates.io probe asked for with -w '%{http_code}'.
printf '%s' "${STUB_HTTP_CODE:?}"
STUB
cat >"${stubs}/cargo" <<'STUB'
#!/usr/bin/env bash
# Logs every call; `cargo publish -p <crate>` follows STUB_CARGO_MODE:
#   ok | rate-limit-once | rate-limit-always | exists
echo "$*" >>"${STUB_LOG:?}"
[[ "$1 $2" == "publish -p" ]] || exit 0
crate="$3"
case "${STUB_CARGO_MODE:-ok}" in
ok) exit 0 ;;
exists)
  echo "error: crate ${crate}@1.2.3 already exists on crates.io index" >&2
  exit 101
  ;;
rate-limit-always)
  echo "error: failed to publish: 429 Too Many Requests" >&2
  exit 101
  ;;
rate-limit-once)
  marker="${STUB_LOG}.${crate}.limited"
  if [[ ! -e "$marker" ]]; then
    : >"$marker"
    echo "error: failed to publish: 429 Too Many Requests" >&2
    exit 101
  fi
  exit 0
  ;;
esac
STUB
chmod +x "${stubs}/curl" "${stubs}/cargo"

# crates_case <label> <expected status> <http code> <cargo mode> <extra env...>; the log is left in ${work}/cargo.log.
crates_case() {
  local label="$1" want="$2" code="$3" mode="$4"
  shift 4
  rm -f "${work}"/cargo.log*
  expect "publish-crates: ${label}" "$want" env PATH="${stubs}:${PATH}" STUB_LOG="${work}/cargo.log" \
    STUB_HTTP_CODE="$code" STUB_CARGO_MODE="$mode" VERSION=1.2.3 MAX_ATTEMPTS=3 BACKOFF_SECONDS=0 SETTLE_SECONDS=0 \
    "$@" bash scripts/publish-crates.sh
}
published_crates() { sed -n 's/^publish -p \([^ ]*\) .*/\1/p' "${work}/cargo.log" | uniq; }

crates_case "unpublished crates go out leaves first after a workspace dry run" 0 404 ok
if [[ "$(head -n 1 "${work}/cargo.log")" != publish\ --workspace\ --dry-run* ]]; then
  fail "publish-crates: the first cargo call is not the workspace dry run"
fi
order="$(published_crates | paste -sd, -)"
case "$order" in
skif-core,skif-specifier,*,skif,skif-cli) ;;
*) fail "publish-crates: unexpected publish order: ${order}" ;;
esac
[[ "$(published_crates | wc -l | tr -d ' ')" -eq 14 ]] || fail "publish-crates: expected 14 crates, got ${order}"

# A crate is published after every skif crate it depends on (cargo publish resolves them on the registry).
position=0
published_before=" "
while read -r crate; do
  position=$((position + 1))
  runtime_deps="$(awk '/^\[/ { in_deps = ($0 == "[dependencies]" || $0 == "[build-dependencies]") } in_deps && /^skif[a-z-]*[ .=]/ { sub(/[ .=].*/, ""); print }' "crates/${crate}/Cargo.toml")"
  for dependency in $runtime_deps; do
    [[ "$published_before" == *" ${dependency} "* ]] || fail "publish-crates: ${crate} is published before its dependency ${dependency}"
  done
  published_before="${published_before}${crate} "
done < <(published_crates)
[[ "$position" -eq 14 ]] || fail "publish-crates: checked ${position} crates for dependency order, want 14"

crates_case "versions already on crates.io are skipped" 0 200 ok
[[ ! -s "${work}/cargo.log" ]] || fail "publish-crates: cargo ran although every version is published"

crates_case "dry run uploads nothing" 0 404 ok DRY_RUN=true
[[ -z "$(published_crates)" ]] || fail "publish-crates: a dry run published"
grep -q -- '--dry-run' "${work}/cargo.log" || fail "publish-crates: the dry run did not run cargo publish --dry-run"

crates_case "429 is retried" 0 404 rate-limit-once
[[ "$(grep -c '^publish -p skif-core ' "${work}/cargo.log")" -eq 2 ]] || fail "publish-crates: skif-core was not retried once"

crates_case "a persistent 429 fails the release" 1 404 rate-limit-always
[[ "$(grep -c '^publish -p skif-core ' "${work}/cargo.log")" -eq 3 ]] || fail "publish-crates: expected 3 attempts"

crates_case "'already exists' is a skip" 0 404 exists
crates_case "a missing VERSION is refused" 1 404 ok VERSION=

# ---------------------------------------------------------------- release-bump + check-versions
tree="${work}/tree"
mkdir -p "$tree"
tar -cf - Cargo.toml Cargo.lock crates/*/Cargo.toml npm-package/package.json poly-hooks.toml .pre-commit-hooks.yaml action.yml scripts \
  .github/workflows/publish.yaml |
  tar -xf - -C "$tree"
current="$(bash scripts/check-versions.sh 2>/dev/null | sed -n 's/.*agree on \(.*\)/\1/p')"
[[ -n "$current" ]] || fail "check-versions: the repository itself is inconsistent"

expect "release-bump rejects a bad version" 1 env SKIP_CARGO_UPDATE=1 bash "${tree}/scripts/release-bump.sh" 1.2
expect "release-bump bumps every surface" 0 env SKIP_CARGO_UPDATE=1 bash "${tree}/scripts/release-bump.sh" 9.8.7
expect "check-versions after a bump that skipped Cargo.lock" 1 bash "${tree}/scripts/check-versions.sh" 9.8.7
expect_in_out "check-versions stale lock" "Cargo.lock: skif"
# A bump that cannot refresh the lock must fail rather than warn: a stub cargo fails, then one that rewrites the lock.
bump_stubs="${work}/bump-stubs"
mkdir -p "$bump_stubs"
printf '#!/usr/bin/env bash\necho "cargo: no network" >&2\nexit 101\n' >"${bump_stubs}/cargo"
chmod +x "${bump_stubs}/cargo"
expect "release-bump fails when cargo update fails" 1 env PATH="${bump_stubs}:${PATH}" bash "${tree}/scripts/release-bump.sh" 9.8.7
expect_in_out "release-bump cargo failure" "cargo update --workspace failed"
cat >"${bump_stubs}/cargo" <<'STUB'
#!/usr/bin/env bash
# Stands in for `cargo update --workspace`: points the workspace packages of Cargo.lock at the manifest version.
version="$(awk '/^\[workspace\.package\]/{p=1;next} /^\[/{p=0} p && /^version *= *"/{gsub(/"/,"",$3);print $3;exit}' Cargo.toml)"
awk -v v="$version" '/^name = "skif/ { skif = 1 } /^name = "/ && !/^name = "skif/ { skif = 0 } skif && /^version = / { print "version = \"" v "\""; next } { print }' Cargo.lock >Cargo.lock.new
mv Cargo.lock.new Cargo.lock
STUB
expect "release-bump refreshes Cargo.lock through cargo" 0 env PATH="${bump_stubs}:${PATH}" bash "${tree}/scripts/release-bump.sh" 9.8.7
expect "check-versions after the bump" 0 bash "${tree}/scripts/check-versions.sh" 9.8.7
sed -i.bak 's/^\( *glibc: \)"2.34"/\1"2.31"/' "${tree}/.github/workflows/publish.yaml"
expect "check-versions catches a drifted glibc matrix value" 1 bash "${tree}/scripts/check-versions.sh" 9.8.7
expect_in_out "check-versions glibc floor" "SKIF_GLIBC_FLOOR"
mv "${tree}/.github/workflows/publish.yaml.bak" "${tree}/.github/workflows/publish.yaml"
expect "check-versions with the glibc matrix restored" 0 bash "${tree}/scripts/check-versions.sh" 9.8.7
[[ "$(jq -r '.version' "${tree}/npm-package/package.json")" == 9.8.7 ]] || fail "release-bump: npm-package version"
grep -qF 'SKIF_VERSION="${SKIF_VERSION:-v9.8.7}"' "${tree}/scripts/pre-commit/run-skif.sh" || fail "release-bump: launcher pin"
grep -qF 'BAKED_VERSION: "9.8.7"' "${tree}/action.yml" || fail "release-bump: the Action's baked version"
grep -qF '"=9.8.7"' "${tree}/Cargo.toml" || fail "release-bump: internal dependency pin"

jq '.version = "0.0.1"' "${tree}/npm-package/package.json" >"${work}/package.json"
cat "${work}/package.json" >"${tree}/npm-package/package.json"
expect "check-versions catches a drifted npm version" 1 bash "${tree}/scripts/check-versions.sh" 9.8.7
expect_in_out "check-versions drift" "npm-package/package.json version"

# ---------------------------------------------------------------- check-archive
archive_dir="${work}/archives"
mkdir -p "${archive_dir}/full" "${archive_dir}/bare"
for member in skif skif.exe LICENSE ATTRIBUTIONS.md; do echo "$member" >"${archive_dir}/full/${member}"; done
for member in skif skif.exe LICENSE; do echo "$member" >"${archive_dir}/bare/${member}"; done
tar czf "${archive_dir}/skif-x86_64-unknown-linux-gnu.tar.gz" -C "${archive_dir}/full" skif LICENSE ATTRIBUTIONS.md
tar czf "${archive_dir}/skif-bare-unknown-linux-gnu.tar.gz" -C "${archive_dir}/bare" skif LICENSE
expect "check-archive accepts a complete tarball" 0 bash scripts/check-archive.sh "${archive_dir}/skif-x86_64-unknown-linux-gnu.tar.gz"
expect "check-archive rejects a tarball without ATTRIBUTIONS.md" 1 bash scripts/check-archive.sh "${archive_dir}/skif-bare-unknown-linux-gnu.tar.gz"
expect_in_out "check-archive missing notice" "ATTRIBUTIONS.md"
if command -v python3 >/dev/null 2>&1 && command -v unzip >/dev/null 2>&1; then
  zip_members() { (cd "$1" && python3 -I -m zipfile -c "$2" "${@:3}"); }
  zip_members "${archive_dir}/full" "${archive_dir}/skif-x86_64-pc-windows-msvc.zip" skif.exe LICENSE ATTRIBUTIONS.md
  zip_members "${archive_dir}/bare" "${archive_dir}/skif-bare-pc-windows-msvc.zip" skif.exe LICENSE
  expect "check-archive accepts a complete zip" 0 bash scripts/check-archive.sh "${archive_dir}/skif-x86_64-pc-windows-msvc.zip"
  expect "check-archive rejects an incomplete zip" 1 bash scripts/check-archive.sh "${archive_dir}/skif-bare-pc-windows-msvc.zip"
fi
expect "check-archive needs an archive" 2 bash scripts/check-archive.sh

# ---------------------------------------------------------------- check-npm-tarballs
platforms=(darwin-arm64 darwin-x64 linux-arm64-gnu linux-arm64-musl linux-x64-gnu linux-x64-musl)

# pack_tarball <directory> <tarball name> <package.json content> <member...>: a tarball whose members all sit under package/.
pack_tarball() {
  local out="$1" name="$2" manifest="$3" stage member
  shift 3
  stage="$(mktemp -d "${work}/pack.XXXXXX")"
  mkdir -p "${stage}/package"
  printf '%s\n' "$manifest" >"${stage}/package/package.json"
  for member in "$@"; do
    mkdir -p "$(dirname "${stage}/package/${member}")"
    echo "$member" >"${stage}/package/${member}"
  done
  tar czf "${out}/${name}" -C "$stage" package
}

# npm_fixture <directory> <version>: the complete umbrella plus the six platform packages.
npm_fixture() {
  local out="$1" version="$2" optional platform
  mkdir -p "$out"
  optional="$(for platform in "${platforms[@]}"; do printf '"skif-%s": "%s",' "$platform" "$version"; done)"
  pack_tarball "$out" "skif-cli-${version}.tgz" "{\"optionalDependencies\": {${optional%,}}}" \
    index.js index.d.ts bin/skif.js LICENSE ATTRIBUTIONS.md README.md
  for platform in "${platforms[@]}"; do
    pack_tarball "$out" "skif-${platform}-${version}.tgz" '{}' "skif.${platform}.node" LICENSE ATTRIBUTIONS.md
  done
}

npm_fixture "${work}/npm-ok" 1.2.3
expect "check-npm-tarballs accepts the complete set" 0 bash scripts/check-npm-tarballs.sh "${work}/npm-ok" 1.2.3

npm_fixture "${work}/npm-missing" 1.2.3
rm "${work}/npm-missing/skif-linux-x64-musl-1.2.3.tgz"
expect "check-npm-tarballs rejects a missing platform package" 1 bash scripts/check-npm-tarballs.sh "${work}/npm-missing" 1.2.3
expect_in_out "check-npm-tarballs missing platform" "not the umbrella plus the six platform packages"

npm_fixture "${work}/npm-extra" 1.2.3
cp "${work}/npm-extra/skif-cli-1.2.3.tgz" "${work}/npm-extra/skif-other-1.2.3.tgz"
expect "check-npm-tarballs rejects an extra tarball" 1 bash scripts/check-npm-tarballs.sh "${work}/npm-extra" 1.2.3

npm_fixture "${work}/npm-version" 1.2.3
expect "check-npm-tarballs rejects a different version" 1 bash scripts/check-npm-tarballs.sh "${work}/npm-version" 1.2.4

npm_fixture "${work}/npm-no-loader" 1.2.3
pack_tarball "${work}/npm-no-loader" skif-cli-1.2.3.tgz \
  "$(tar -xzOf "${work}/npm-ok/skif-cli-1.2.3.tgz" package/package.json)" bin/skif.js LICENSE ATTRIBUTIONS.md README.md
expect "check-npm-tarballs rejects an umbrella without the generated loader" 1 bash scripts/check-npm-tarballs.sh "${work}/npm-no-loader" 1.2.3
expect_in_out "check-npm-tarballs missing loader" "does not contain index.js"

npm_fixture "${work}/npm-no-addon" 1.2.3
pack_tarball "${work}/npm-no-addon" skif-linux-x64-gnu-1.2.3.tgz '{}' LICENSE ATTRIBUTIONS.md
expect "check-npm-tarballs rejects a platform package without its addon" 1 bash scripts/check-npm-tarballs.sh "${work}/npm-no-addon" 1.2.3
expect_in_out "check-npm-tarballs missing addon" "skif.linux-x64-gnu.node"

npm_fixture "${work}/npm-bad-optional" 1.2.3
pack_tarball "${work}/npm-bad-optional" skif-cli-1.2.3.tgz '{"optionalDependencies": {"skif-darwin-arm64": "1.2.3"}}' \
  index.js index.d.ts bin/skif.js LICENSE ATTRIBUTIONS.md README.md
expect "check-npm-tarballs rejects incomplete optionalDependencies" 1 bash scripts/check-npm-tarballs.sh "${work}/npm-bad-optional" 1.2.3
expect_in_out "check-npm-tarballs optionalDependencies" "optionalDependencies"
expect "check-npm-tarballs usage" 1 bash scripts/check-npm-tarballs.sh

# ---------------------------------------------------------------- check-glibc-floor
glibc_stubs="${work}/glibc-stubs"
mkdir -p "$glibc_stubs"
cat >"${glibc_stubs}/objdump" <<'STUB'
#!/usr/bin/env bash
# Prints one dynamic symbol per version in STUB_GLIBC_VERSIONS (space separated).
for version in ${STUB_GLIBC_VERSIONS:-}; do
  echo "0000000000000000      DF *UND*  0000000000000000  GLIBC_${version} memcpy"
done
STUB
chmod +x "${glibc_stubs}/objdump"
echo binary >"${work}/skif-binary"

floor_case() {
  local label="$1" want="$2" versions="$3"
  shift 3
  expect "check-glibc-floor: ${label}" "$want" env PATH="${glibc_stubs}:${PATH}" STUB_GLIBC_VERSIONS="$versions" \
    bash scripts/check-glibc-floor.sh "${work}/skif-binary" "$@"
}
floor_case "symbols at or below 2.34 pass" 0 "2.2.5 2.17 2.34 2.28"
floor_case "a 2.35 symbol fails the 2.34 floor" 1 "2.17 2.35 2.28"
expect_in_out "check-glibc-floor: newer symbol" "needs glibc 2.35"
floor_case "2.100 is newer than 2.34" 1 "2.34 2.100"
floor_case "the floor can be passed explicitly" 0 "2.35" 2.39
floor_case "a binary without versioned symbols fails" 1 ""
expect "check-glibc-floor: a missing binary" 1 env PATH="${glibc_stubs}:${PATH}" bash scripts/check-glibc-floor.sh "${work}/nope"

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
*) echo '[]' ;;
esac
STUB
chmod +x "${pm_stubs}/npm"
pm_case() {
  local label="$1" want="$2" pm="$3" slot="$4"
  shift 4
  expect "resolve-pm-version: ${label}" 0 env PATH="${pm_stubs}:${PATH}" bash scripts/e2e/resolve-pm-version.sh "$pm" "$slot" "$@"
  [[ "$(cat "${work}/out")" == "$want" ]] || fail "resolve-pm-version: ${label}: got '$(cat "${work}/out")', want ${want}"
}
pm_case "npm newest ignores a release candidate" 11.4.1 npm newest
pm_case "npm previous" 10.9.2 npm previous
pm_case "yarn reads the berry package, newest" 4.10.0 yarn newest
pm_case "yarn previous sorts numerically" 3.8.7 yarn previous
expect "resolve-pm-version: no such major" 1 env PATH="${pm_stubs}:${PATH}" bash scripts/e2e/resolve-pm-version.sh pnpm newest
pm_case "bun restricted to 1.x ignores Bun 2 and sorts numerically" 1.10.1 bun newest 1
pm_case "bun without a major takes the newest major" 2.0.1 bun newest
expect "resolve-pm-version: a non-numeric major" 2 env PATH="${pm_stubs}:${PATH}" bash scripts/e2e/resolve-pm-version.sh bun newest one
expect "resolve-pm-version: unknown package manager" 2 env PATH="${pm_stubs}:${PATH}" bash scripts/e2e/resolve-pm-version.sh deno newest
expect "resolve-pm-version: unknown slot" 2 env PATH="${pm_stubs}:${PATH}" bash scripts/e2e/resolve-pm-version.sh npm oldest

# ---------------------------------------------------------------- check-release-gate
cat >"${stubs}/gh" <<'STUB'
#!/usr/bin/env bash
# `gh run list ... --jq length` prints the number of green runs the test configured.
printf '%s\n' "${STUB_GREEN_RUNS:?}"
STUB
chmod +x "${stubs}/gh"

gate="${work}/gate-repo"
git init -q -b main "$gate"
git_in() { git -C "$gate" -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false -c tag.gpgsign=false "$@"; }
git_in commit -q --allow-empty -m "on main"
git_in tag v1.0.0
git_in checkout -q -b feature
git_in commit -q --allow-empty -m "only on a branch"
git_in tag v1.1.0

gate_case() {
  local label="$1" want="$2" tag="$3" green="$4"
  expect "release gate: ${label}" "$want" env PATH="${stubs}:${PATH}" STUB_GREEN_RUNS="$green" MAIN_REF=main \
    bash -c 'cd "$1" && bash "$2/scripts/check-release-gate.sh" "$3"' _ "$gate" "$repo" "$tag"
}
gate_case "a green tag on main passes" 0 v1.0.0 1
gate_case "a tag without a green CI run is refused" 1 v1.0.0 0
expect_in_out "release gate: no green run" "no successful ci.yml run"
gate_case "a tag that is not on main is refused" 1 v1.1.0 1
expect_in_out "release gate: not on main" "not an ancestor"

# ---------------------------------------------------------------- floating tag
float_remote="${work}/float-remote.git"
float_repo="${work}/float-repo"
git init -q --bare "$float_remote"
git init -q -b main "$float_repo"
git -C "$float_repo" remote add origin "$float_remote"
fgit() { git -C "$float_repo" -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false -c tag.gpgsign=false "$@"; }
fgit commit -q --allow-empty -m "release"
for t in v0.1.0 v0.10.3 v1.2.3; do fgit tag "$t"; done

float_run() {
  expect "$1" "$2" env DRY_RUN="$5" bash -c 'cd "$1" && bash "$2/scripts/release-floating-tag.sh" "$3" "$4"' _ "$float_repo" "$repo" "$3" "$4"
}
float_case() {
  local label="$1" version="$2" tag="$3" floating="$4"
  float_run "floating tag: ${label}" 0 "$version" "$tag" false
  expect_in_out "floating tag: ${label}" "Moved ${floating} to ${tag}"
  [[ "$(git -C "$float_remote" rev-parse "refs/tags/${floating}")" == "$(git -C "$float_repo" rev-parse "${tag}^{commit}")" ]] ||
    fail "floating tag: ${label}: ${floating} is not on ${tag} in the remote"
}
float_case "a 0.x release floats as v0.M" 0.1.0 v0.1.0 v0.1
float_case "the minor keeps two digits" 0.10.3 v0.10.3 v0.10
float_case "a stable release floats as vN" 1.2.3 v1.2.3 v1
if git -C "$float_remote" rev-parse -q --verify refs/tags/v0 >/dev/null; then
  fail "floating tag: v0 must never move"
fi
float_run "floating tag: a pre-release has no floating tag" 2 1.0.0-rc.1 v1.2.3 false
expect_in_out "floating tag: pre-release" "pre-release"
float_run "floating tag: a malformed version is refused" 2 latest v1.2.3 false
fgit commit -q --allow-empty -m "next"
fgit tag v0.1.1
float_run "floating tag: a dry run succeeds" 0 0.1.1 v0.1.1 true
expect_in_out "floating tag: dry run" "Would move v0.1 to v0.1.1"
[[ "$(git -C "$float_remote" rev-parse refs/tags/v0.1)" == "$(git -C "$float_repo" rev-parse 'v0.1.0^{commit}')" ]] ||
  fail "floating tag: a dry run moved the remote tag"
float_case "a later patch moves the floating tag" 0.1.1 v0.1.1 v0.1

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "release scripts: all cases passed"
