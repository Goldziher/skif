#!/usr/bin/env bash
# Tests the scripts that put a skif binary in front of a user, against stub release servers and a stub `gh`:
#   pre-commit/run-skif.sh   resolution order, a non-executable SKIF_BINARY, a corrupted archive, a missing checksum
#   lib/libc.sh              the glibc 2.34 floor: gnu on 2.34 or newer, musl below it or when undetectable
#   gh-action-resolve.sh     tag and asset selection per runner, the version baked into a release of the Action
#   gh-action-install.sh     checksum and attestation verification, extraction, outputs, the verified cache-hit path
#   gh-action-run.sh         argument assembly
#
# The launcher is served from a file:// release directory (SKIF_RELEASE_URL), the Action from a stub `gh`.
#
# Usage: scripts/test-install-scripts.sh
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
  if [[ "$want" == nonzero && "$status" -eq 0 ]] || [[ "$want" != nonzero && "$status" -ne "$want" ]]; then
    fail "${label}: exit ${status}, want ${want}"
    sed 's/^/    /' "${work}/out" >&2
  fi
}

expect_in_out() {
  grep -qF -- "$2" "${work}/out" || fail "$1: output does not contain '$2'"
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi
}

# A release asset that is a shell script announcing itself, packed the way the release packs it.
make_tarball() {
  local dir="$1" name="$2" stage
  stage="$(mktemp -d "${work}/stage.XXXXXX")"
  printf '#!/bin/sh\necho "fake skif $*"\n' >"${stage}/skif"
  chmod +x "${stage}/skif"
  echo licence >"${stage}/LICENSE"
  tar czf "${dir}/${name}" -C "$stage" skif LICENSE
}

# ---------------------------------------------------------------- pre-commit launcher
launcher="${repo}/scripts/pre-commit/run-skif.sh"
# `gh` is stubbed so a developer's real, authenticated gh never verifies a fixture archive.
launcher_stubs="${work}/launcher-stubs"
mkdir -p "$launcher_stubs"
cat >"${launcher_stubs}/gh" <<'STUB'
#!/usr/bin/env bash
case "$1 $2" in
"auth status") [[ "${STUB_GH_AUTH:-}" == 1 ]] || exit 1 ;;
"attestation verify")
  [[ " $* " == *" --help "* ]] && exit 0
  echo "gh $*" >>"${STUB_GH_LOG:-/dev/null}"
  [[ -z "${STUB_ATTEST_FAIL:-}" ]] || { echo "no attestation found" >&2; exit 1; }
  ;;
esac
STUB
chmod +x "${launcher_stubs}/gh"
basic_path="${launcher_stubs}:/usr/bin:/bin"
cache="${work}/cache"

# The launcher names the target it resolved on stderr; learn it from a download that cannot succeed.
empty="${work}/empty-release"
mkdir -p "$empty"
target="$(
  PATH="$basic_path" SKIF_CACHE_DIR="${work}/probe-cache" SKIF_RELEASE_URL="file://${empty}" SKIF_VERSION=1.2.3 \
    bash "$launcher" --version 2>&1 | sed -n 's/^Downloading skif v1.2.3 for \(.*\)\.\.\.$/\1/p' || true
)"

if [[ -z "$target" || "$target" == *windows* ]]; then
  echo "skip launcher tests: no tar.gz target on this host (${target:-unknown})" >&2
else
  release="${work}/release"
  mkdir -p "$release"
  asset="skif-${target}.tar.gz"
  make_tarball "$release" "$asset"
  printf '%s  %s\n' "$(sha256 "${release}/${asset}")" "$asset" >"${release}/checksums.txt"

  # 1. SKIF_BINARY wins over everything and must be executable.
  fake="${work}/my-skif"
  printf '#!/bin/sh\necho "env skif $*"\n' >"$fake"
  chmod +x "$fake"
  expect "launcher: SKIF_BINARY" 0 env SKIF_RESOLVE_ONLY=1 SKIF_BINARY="$fake" bash "$launcher"
  [[ "$(cat "${work}/out")" == "$fake" ]] || fail "launcher: SKIF_BINARY was not resolved"
  expect "launcher: SKIF_BINARY runs with the hook arguments" 0 env SKIF_BINARY="$fake" bash "$launcher" --no-js
  [[ "$(cat "${work}/out")" == "env skif --no-js" ]] || fail "launcher: arguments were not passed through"

  plain="${work}/plain-file"
  echo data >"$plain"
  chmod -x "$plain"
  expect "launcher: non-executable SKIF_BINARY" 1 env SKIF_BINARY="$plain" bash "$launcher"
  expect_in_out "launcher: non-executable SKIF_BINARY" "SKIF_BINARY is not executable"
  expect "launcher: SKIF_BINARY does not fall back to PATH" 1 env PATH="${work}/bin-on-path:${basic_path}" \
    SKIF_BINARY="${work}/does-not-exist" bash "$launcher"

  # 2. skif on PATH.
  mkdir -p "${work}/bin-on-path"
  printf '#!/bin/sh\necho "path skif $*"\n' >"${work}/bin-on-path/skif"
  chmod +x "${work}/bin-on-path/skif"
  expect "launcher: skif on PATH" 0 env PATH="${work}/bin-on-path:${basic_path}" SKIF_RESOLVE_ONLY=1 bash "$launcher"
  [[ "$(cat "${work}/out")" == "${work}/bin-on-path/skif" ]] || fail "launcher: PATH skif was not resolved"

  # 3. the pinned release, downloaded once.
  expect "launcher: download" 0 env SKIF_RESOLVE_ONLY=1 PATH="$basic_path" SKIF_CACHE_DIR="$cache" \
    SKIF_RELEASE_URL="file://${release}" SKIF_VERSION=1.2.3 bash "$launcher"
  cached="${cache}/v1.2.3/${target}/skif"
  [[ "$(tail -n 1 "${work}/out")" == "$cached" && -x "$cached" ]] || fail "launcher: the download is not cached at ${cached}"
  expect "launcher: the downloaded binary runs" 0 env PATH="$basic_path" SKIF_CACHE_DIR="$cache" \
    SKIF_RELEASE_URL="file://${work}/gone" SKIF_VERSION=v1.2.3 bash "$launcher" --version
  [[ "$(cat "${work}/out")" == "fake skif --version" ]] || fail "launcher: the cached binary did not run offline"

  # 4. a corrupted archive is refused and leaves nothing in the cache.
  bad_release="${work}/bad-release"
  cp -R "$release" "$bad_release"
  echo corrupt >>"${bad_release}/${asset}"
  expect "launcher: corrupted archive" 1 env PATH="$basic_path" SKIF_CACHE_DIR="${work}/cache-bad" \
    SKIF_RELEASE_URL="file://${bad_release}" SKIF_VERSION=1.2.3 bash "$launcher" --version
  expect_in_out "launcher: corrupted archive" "checksum verification failed"
  [[ ! -e "${work}/cache-bad/v1.2.3/${target}/skif" ]] || fail "launcher: a corrupted archive was installed"

  # 5. no checksums.txt, or no entry for the archive: refuse to install an unverified binary.
  rm "${bad_release}/checksums.txt"
  expect "launcher: no checksums.txt" 1 env PATH="$basic_path" SKIF_CACHE_DIR="${work}/cache-nosums" \
    SKIF_RELEASE_URL="file://${bad_release}" SKIF_VERSION=1.2.3 bash "$launcher"
  expect_in_out "launcher: no checksums.txt" "refusing to install an unverified binary"
  printf '%064d  other-archive.tar.gz\n' 1 >"${bad_release}/checksums.txt"
  expect "launcher: no checksum entry" 1 env PATH="$basic_path" SKIF_CACHE_DIR="${work}/cache-noentry" \
    SKIF_RELEASE_URL="file://${bad_release}" SKIF_VERSION=1.2.3 bash "$launcher"
  expect_in_out "launcher: no checksum entry" "no checksum for ${asset}"

  # 6. a release that does not exist.
  expect "launcher: unknown release" nonzero env PATH="$basic_path" SKIF_CACHE_DIR="${work}/cache-404" \
    SKIF_RELEASE_URL="file://${work}/missing" SKIF_VERSION=9.9.9 bash "$launcher"
fi

# 7. the build attestation: verified when gh is authenticated, refused when it fails, announced when it cannot run.
if [[ -n "$target" && "$target" != *windows* ]]; then
  attest_cache="${work}/cache-attest"
  expect "launcher: attestation verified" 0 env SKIF_RESOLVE_ONLY=1 PATH="$basic_path" STUB_GH_AUTH=1 STUB_GH_LOG="${work}/gh.log" \
    SKIF_CACHE_DIR="$attest_cache" SKIF_RELEASE_URL="file://${release}" SKIF_VERSION=1.2.3 bash "$launcher"
  grep -qF "attestation verify" "${work}/gh.log" || fail "launcher: gh attestation verify was not called"
  grep -qF -- "--repo Goldziher/skif" "${work}/gh.log" || fail "launcher: the attestation is not checked against the release repository"
  grep -qF -- "--signer-workflow Goldziher/skif/.github/workflows/publish.yaml" "${work}/gh.log" ||
    fail "launcher: the attestation is not pinned to the publish workflow"
  expect "launcher: failed attestation" 1 env PATH="$basic_path" STUB_GH_AUTH=1 STUB_ATTEST_FAIL=1 SKIF_CACHE_DIR="${work}/cache-attest-bad" \
    SKIF_RELEASE_URL="file://${release}" SKIF_VERSION=1.2.3 bash "$launcher" --version
  expect_in_out "launcher: failed attestation" "attestation verification failed"
  [[ ! -e "${work}/cache-attest-bad/v1.2.3/${target}/skif" ]] || fail "launcher: an unattested archive was installed"
  expect "launcher: no authenticated gh" 0 env PATH="$basic_path" SKIF_CACHE_DIR="${work}/cache-attest-none" \
    SKIF_RELEASE_URL="file://${release}" SKIF_VERSION=1.2.3 bash "$launcher" --version
  expect_in_out "launcher: no authenticated gh" "build attestation not verified"
  expect "launcher: attestation can be switched off" 0 env PATH="$basic_path" STUB_GH_AUTH=1 STUB_ATTEST_FAIL=1 SKIF_VERIFY_ATTESTATION=0 \
    SKIF_CACHE_DIR="${work}/cache-attest-off" SKIF_RELEASE_URL="file://${release}" SKIF_VERSION=1.2.3 bash "$launcher" --version
fi

# ---------------------------------------------------------------- libc selection (the glibc 2.34 floor)
libc_stubs="${work}/libc-stubs"
mkdir -p "$libc_stubs"
cat >"${libc_stubs}/getconf" <<'STUB'
#!/usr/bin/env bash
[[ -n "${STUB_GETCONF:-}" ]] || exit 1
echo "$STUB_GETCONF"
STUB
cat >"${libc_stubs}/ldd" <<'STUB'
#!/usr/bin/env bash
[[ -n "${STUB_LDD:-}" ]] || exit 1
echo "$STUB_LDD"
STUB
cat >"${libc_stubs}/uname" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
-s) echo Linux ;;
-m) echo "${STUB_ARCH:-x86_64}" ;;
*) echo Linux ;;
esac
STUB
chmod +x "${libc_stubs}/getconf" "${libc_stubs}/ldd" "${libc_stubs}/uname"

# libc_case <label> <expected flavour> <getconf output> <ldd banner>
libc_case() {
  local label="$1" want="$2" getconf_out="$3" ldd_out="$4"
  # shellcheck disable=SC2016
  expect "libc: ${label}" 0 env PATH="${libc_stubs}:${PATH}" STUB_GETCONF="$getconf_out" STUB_LDD="$ldd_out" \
    bash -c 'source "$1/scripts/lib/libc.sh" && skif_libc' _ "$repo"
  [[ "$(tail -n 1 "${work}/out")" == "$want" ]] || fail "libc: ${label}: picked '$(tail -n 1 "${work}/out")', want ${want}"
}
libc_case "glibc 2.31 (Debian 11) gets musl" musl "glibc 2.31" ""
libc_case "glibc 2.33 gets musl" musl "glibc 2.33" ""
libc_case "glibc 2.34, the floor, gets gnu" gnu "glibc 2.34" ""
libc_case "glibc 2.35 (Ubuntu 22.04) gets gnu" gnu "glibc 2.35" ""
libc_case "glibc 2.100 compares numerically" gnu "glibc 2.100" ""
libc_case "glibc 3.0 gets gnu" gnu "glibc 3.0" ""
libc_case "ldd alone: Debian 11" musl "" "ldd (Debian GLIBC 2.31-13+deb11u7) 2.31"
libc_case "ldd alone: Ubuntu 22.04" gnu "" "ldd (Ubuntu GLIBC 2.35-0ubuntu3.8) 2.35"
libc_case "musl banner gets musl" musl "" "musl libc (x86_64)"
libc_case "an unidentifiable libc gets musl" musl "" ""
libc_case "an unparseable version gets musl" musl "glibc banana" ""

# The container smoke check: it must pass when the rule matches the expectation and fail when it does not.
smoke_bins="${work}/smoke-bins"
for flavour in gnu musl; do
  mkdir -p "${smoke_bins}/${flavour}"
  printf '#!/bin/sh\necho "skif %s $*"\n' "$flavour" >"${smoke_bins}/${flavour}/skif"
  chmod +x "${smoke_bins}/${flavour}/skif"
done
expect "libc smoke: glibc 2.31 runs the musl binary" 0 env PATH="${libc_stubs}:${PATH}" STUB_GETCONF="glibc 2.31" \
  bash scripts/check-libc-selection.sh "$smoke_bins" musl
expect_in_out "libc smoke: runs the chosen binary" "skif musl --version"
expect "libc smoke: glibc 2.34 runs the gnu binary" 0 env PATH="${libc_stubs}:${PATH}" STUB_GETCONF="glibc 2.34" \
  bash scripts/check-libc-selection.sh "$smoke_bins" gnu
expect "libc smoke: a wrong pick fails" 1 env PATH="${libc_stubs}:${PATH}" STUB_GETCONF="glibc 2.31" \
  bash scripts/check-libc-selection.sh "$smoke_bins" gnu
expect_in_out "libc smoke: a wrong pick" "picked musl, expected gnu"

# launcher_libc_case <label> <expected archive flavour> <getconf output>: the launcher downloads the matching archive.
launcher_libc_case() {
  local label="$1" want="$2" getconf_out="$3" libc_release libc_cache
  libc_release="$(mktemp -d "${work}/libc-release.XXXXXX")"
  libc_cache="$(mktemp -d "${work}/libc-cache.XXXXXX")"
  for flavour in gnu musl; do
    make_tarball "$libc_release" "skif-x86_64-unknown-linux-${flavour}.tar.gz"
  done
  : >"${libc_release}/checksums.txt"
  for flavour in gnu musl; do
    printf '%s  %s\n' "$(sha256 "${libc_release}/skif-x86_64-unknown-linux-${flavour}.tar.gz")" "skif-x86_64-unknown-linux-${flavour}.tar.gz" \
      >>"${libc_release}/checksums.txt"
  done
  expect "launcher: ${label}" 0 env SKIF_RESOLVE_ONLY=1 PATH="${libc_stubs}:${basic_path}" STUB_GETCONF="$getconf_out" \
    SKIF_CACHE_DIR="$libc_cache" SKIF_RELEASE_URL="file://${libc_release}" SKIF_VERSION=1.2.3 bash "$launcher"
  [[ "$(tail -n 1 "${work}/out")" == "${libc_cache}/v1.2.3/x86_64-unknown-linux-${want}/skif" ]] ||
    fail "launcher: ${label}: resolved '$(tail -n 1 "${work}/out")', want the ${want} archive"
}
launcher_libc_case "glibc 2.31 downloads the musl archive" musl "glibc 2.31"
launcher_libc_case "glibc 2.34 downloads the gnu archive" gnu "glibc 2.34"
launcher_libc_case "an unidentifiable libc downloads the musl archive" musl ""

# ---------------------------------------------------------------- Action: resolve
stubs="${work}/stubs"
mkdir -p "$stubs"
cat >"${stubs}/gh" <<'STUB'
#!/usr/bin/env bash
case "$1 $2" in
"api repos/"*) printf '%s\n' ${STUB_TAGS:-} ;;
"release download")
  tag="$3"
  shift 3
  while [[ $# -gt 0 ]]; do
    case "$1" in
    -p)
      [[ "$2" == checksums.txt || -z "${STUB_FAIL_ASSET:-}" ]] || { echo "the archive must not be downloaded again" >&2; exit 1; }
      cp "${STUB_RELEASE:?}/$2" .
      shift 2
      ;;
    *) shift ;;
    esac
  done
  ;;
"attestation verify")
  [[ " $* " == *" --help "* ]] && { [[ -z "${STUB_NO_ATTEST:-}" ]]; exit; }
  echo "gh $*" >>"${STUB_GH_LOG:-/dev/null}"
  [[ -z "${STUB_ATTEST_FAIL:-}" ]] || { echo "no attestation found" >&2; exit 1; }
  ;;
esac
STUB
chmod +x "${stubs}/gh"

# resolve_case <label> <expected exit> <RUNNER_OS> <RUNNER_ARCH> <VERSION_INPUT> <ACTION_REF> [VAR=value...]; outputs in
# ${work}/gh-output. A glibc 2.35 host is stubbed, so the Linux asset does not depend on the machine running the tests.
resolve_case() {
  local label="$1" want="$2" os="$3" arch="$4" input="$5" ref="$6"
  shift 6
  : >"${work}/gh-output"
  expect "resolve: ${label}" "$want" env PATH="${libc_stubs}:${stubs}:${PATH}" STUB_GETCONF="glibc 2.35" GITHUB_OUTPUT="${work}/gh-output" \
    RUNNER_TEMP="${work}/runner" RUNNER_OS="$os" RUNNER_ARCH="$arch" VERSION_INPUT="$input" ACTION_REF="$ref" REPOSITORY=Acme/skif \
    STUB_TAGS="v1.0.0 v0.2.1 v0.2.0 v0.1.0 v0" "$@" bash scripts/gh-action-resolve.sh
}
output_of() { sed -n "s/^$1=//p" "${work}/gh-output"; }

resolve_case "an exact version" 0 macOS ARM64 0.1.0 main
[[ "$(output_of tag)" == v0.1.0 && "$(output_of asset)" == skif-aarch64-apple-darwin.tar.gz ]] || fail "resolve: exact version on macOS arm64"
resolve_case "a v-prefixed version" 0 macOS X64 v0.1.0 ""
[[ "$(output_of tag)" == v0.1.0 && "$(output_of asset)" == skif-x86_64-apple-darwin.tar.gz ]] || fail "resolve: v-prefixed version on macOS x64"
resolve_case "a floating major tag resolves to the newest 0.x" 0 Windows X64 "" v0
[[ "$(output_of tag)" == v0.2.1 && "$(output_of asset)" == skif-x86_64-pc-windows-msvc.zip && "$(output_of ext)" == zip ]] ||
  fail "resolve: floating major on Windows x64"
resolve_case "a floating minor tag stays inside its 0.x line" 0 macOS ARM64 "" v0.1
[[ "$(output_of tag)" == v0.1.0 ]] || fail "resolve: v0.1 must not move to a later 0.x minor, got $(output_of tag)"
resolve_case "a floating minor tag takes the newest patch" 0 macOS ARM64 "" v0.2
[[ "$(output_of tag)" == v0.2.1 ]] || fail "resolve: v0.2 must resolve to its newest patch, got $(output_of tag)"
resolve_case "a floating major tag of a stable series" 0 macOS ARM64 "" v1
[[ "$(output_of tag)" == v1.0.0 ]] || fail "resolve: v1 must resolve to the newest 1.x, got $(output_of tag)"
resolve_case "a minor series without a release" 1 macOS ARM64 "" v0.7
expect_in_out "resolve: unreleased minor" "No published release found"
resolve_case "an action pinned to a full tag" 0 Windows ARM64 "" v0.2.0
[[ "$(output_of tag)" == v0.2.0 && "$(output_of asset)" == skif-aarch64-pc-windows-msvc.zip ]] || fail "resolve: full tag on Windows arm64"
resolve_case "a branch ref falls back to the newest release" 0 Linux X64 "" main
[[ "$(output_of tag)" == v1.0.0 ]] || fail "resolve: latest"
[[ "$(output_of asset)" == skif-x86_64-unknown-linux-gnu.tar.gz ]] || fail "resolve: Linux x64 asset on glibc 2.35: $(output_of asset)"
resolve_case "Linux arm64" 0 Linux ARM64 0.1.0 ""
[[ "$(output_of asset)" == skif-aarch64-unknown-linux-gnu.tar.gz ]] || fail "resolve: Linux arm64 asset on glibc 2.35"
resolve_case "Linux below the glibc floor" 0 Linux X64 0.1.0 "" STUB_GETCONF="glibc 2.31"
[[ "$(output_of asset)" == skif-x86_64-unknown-linux-musl.tar.gz ]] || fail "resolve: glibc 2.31 must get the musl archive: $(output_of asset)"
resolve_case "Linux arm64 below the glibc floor" 0 Linux ARM64 0.1.0 "" STUB_GETCONF="glibc 2.28"
[[ "$(output_of asset)" == skif-aarch64-unknown-linux-musl.tar.gz ]] || fail "resolve: arm64 glibc 2.28 must get the musl archive"
resolve_case "Linux at the glibc floor" 0 Linux X64 0.1.0 "" STUB_GETCONF="glibc 2.34"
[[ "$(output_of asset)" == skif-x86_64-unknown-linux-gnu.tar.gz ]] || fail "resolve: glibc 2.34 must get the gnu archive"
resolve_case "Linux with an unidentifiable libc" 0 Linux X64 0.1.0 "" STUB_GETCONF=""
[[ "$(output_of asset)" == skif-x86_64-unknown-linux-musl.tar.gz ]] || fail "resolve: an unidentifiable libc must get the musl archive"
resolve_case "a release of the Action installs its own version" 0 macOS ARM64 "" "0123456789abcdef0123456789abcdef01234567" BAKED_VERSION=0.2.0
[[ "$(output_of tag)" == v0.2.0 ]] || fail "resolve: a SHA-pinned Action must install its baked version, got $(output_of tag)"
resolve_case "the baked version beats a floating tag" 0 macOS ARM64 "" v0 BAKED_VERSION=0.1.0
[[ "$(output_of tag)" == v0.1.0 ]] || fail "resolve: the baked version must beat the floating major tag, got $(output_of tag)"
resolve_case "an explicit version beats the baked one" 0 macOS ARM64 0.2.1 main BAKED_VERSION=0.1.0
[[ "$(output_of tag)" == v0.2.1 ]] || fail "resolve: the version input must beat the baked version"
resolve_case "a development checkout (0.0.0) follows the ref" 0 macOS ARM64 "" v0 BAKED_VERSION=0.0.0
[[ "$(output_of tag)" == v0.2.1 ]] || fail "resolve: 0.0.0 is no baked version"
resolve_case "Linux arm64 outputs" 0 Linux ARM64 0.1.0 ""
[[ "$(output_of checksums)" == checksums.txt && "$(output_of repo)" == Acme/skif ]] || fail "resolve: repo or checksums output"
[[ "$(output_of bin-dir)" == "${work}/runner/skif-action/v0.1.0-Linux-ARM64/bin" ]] || fail "resolve: bin-dir $(output_of bin-dir)"
[[ "$(output_of archive-dir)" == "${work}/runner/skif-action/v0.1.0-Linux-ARM64/archive" ]] || fail "resolve: archive-dir $(output_of archive-dir)"
[[ "$(output_of cache-dir)" == "${work}/runner/skif-action/v0.1.0-Linux-ARM64" ]] || fail "resolve: cache-dir $(output_of cache-dir)"
resolve_case "no release in that series" 1 Linux X64 9 ""
expect_in_out "resolve: no release" "No published release found"
resolve_case "an unsupported architecture" 1 Linux RISCV 0.1.0 ""
resolve_case "an unsupported OS" 1 Plan9 X64 0.1.0 ""

# ---------------------------------------------------------------- Action: install
action_release="${work}/action-release"
mkdir -p "$action_release"
make_tarball "$action_release" skif-x86_64-unknown-linux-gnu.tar.gz
printf '%s  %s\n' "$(sha256 "${action_release}/skif-x86_64-unknown-linux-gnu.tar.gz")" skif-x86_64-unknown-linux-gnu.tar.gz \
  >"${action_release}/checksums.txt"

# run_install <label> <expected exit> <release dir> [VAR=value...]: BIN_DIR is ${work}/action/bin and the verified archive
# is kept in ${work}/action/archive; nothing is reset, so a second call is a cache hit when CACHE_HIT=true.
run_install() {
  local label="$1" want="$2" release_dir="$3"
  shift 3
  : >"${work}/gh-output"
  : >"${work}/gh-path"
  expect "install: ${label}" "$want" env PATH="${stubs}:${PATH}" STUB_RELEASE="$release_dir" STUB_GH_LOG="${work}/attest.log" RUNNER_OS=Linux \
    GITHUB_OUTPUT="${work}/gh-output" GITHUB_PATH="${work}/gh-path" REPO=Acme/skif TAG=v1.2.3 \
    ASSET=skif-x86_64-unknown-linux-gnu.tar.gz EXT=tar.gz CHECKSUMS=checksums.txt BIN_DIR="${work}/action/bin" \
    ARCHIVE_DIR="${work}/action/archive" CACHE_HIT=false "$@" bash scripts/gh-action-install.sh
}
install_case() {
  rm -rf "${work}/action"
  : >"${work}/attest.log"
  run_install "$@"
}
archive_asset="${work}/action/archive/skif-x86_64-unknown-linux-gnu.tar.gz"

install_case "a verified archive" 0 "$action_release"
[[ -x "${work}/action/bin/skif" ]] || fail "install: skif was not installed"
[[ "$(output_of version)" == 1.2.3 && "$(output_of path)" == "${work}/action/bin/skif" ]] || fail "install: outputs"
grep -qxF "${work}/action/bin" "${work}/gh-path" || fail "install: BIN_DIR is not on PATH"
[[ ! -e "${work}/action/bin/LICENSE" ]] || fail "install: only the binary may land in the PATH directory"
[[ -f "$archive_asset" ]] || fail "install: the verified archive is not kept for cache re-verification"
grep -qF -- "attestation verify skif-x86_64-unknown-linux-gnu.tar.gz --repo Acme/skif --signer-workflow Acme/skif/.github/workflows/publish.yaml" "${work}/attest.log" ||
  fail "install: the archive was not checked against its attestation: $(cat "${work}/attest.log")"

tampered="${work}/action-tampered"
cp -R "$action_release" "$tampered"
echo corrupt >>"${tampered}/skif-x86_64-unknown-linux-gnu.tar.gz"
install_case "a checksum mismatch" 1 "$tampered"
expect_in_out "install: checksum mismatch" "Checksum mismatch"
[[ ! -e "${work}/action/bin/skif" ]] || fail "install: a tampered archive was installed"
[[ ! -e "$archive_asset" ]] || fail "install: a tampered archive was cached"

printf '%064d  other.tar.gz\n' 1 >"${tampered}/checksums.txt"
install_case "no checksum entry" 1 "$tampered"
expect_in_out "install: no checksum entry" "No checksum entry"

install_case "an unknown archive type" 1 "$action_release" EXT=rar

install_case "a failed attestation" 1 "$action_release" STUB_ATTEST_FAIL=1
expect_in_out "install: failed attestation" "no attestation found"
[[ ! -e "${work}/action/bin/skif" ]] || fail "install: an unattested archive was installed"
[[ ! -e "$archive_asset" ]] || fail "install: an unattested archive was cached"
install_case "attestation switched off" 0 "$action_release" STUB_ATTEST_FAIL=1 VERIFY_ATTESTATION=false
[[ ! -s "${work}/attest.log" ]] || fail "install: attestation was checked although it is switched off"
install_case "a gh without the attestation command" 0 "$action_release" STUB_NO_ATTEST=1
expect_in_out "install: gh without attestation" "no 'attestation' command"

# A cache hit re-checks the cached archive against the release's checksums and unpacks the binary again.
install_case "a fresh install to cache" 0 "$action_release"
rm -f "${work}/action/bin/skif"
: >"${work}/attest.log"
run_install "a cache hit re-verifies and re-unpacks" 0 "$action_release" CACHE_HIT=true STUB_FAIL_ASSET=1
[[ -x "${work}/action/bin/skif" ]] || fail "install: a cache hit did not unpack the binary from the verified archive"
[[ "$(output_of version)" == 1.2.3 ]] || fail "install: a cache hit still sets the outputs"
expect_in_out "install: cache hit" "re-checking"
[[ ! -s "${work}/attest.log" ]] || fail "install: a cache hit repeated the attestation check"

printf '#!/bin/sh\necho poisoned\n' >"${work}/action/bin/skif"
echo corrupt >>"$archive_asset"
run_install "a poisoned cached archive" 1 "$action_release" CACHE_HIT=true STUB_FAIL_ASSET=1
expect_in_out "install: poisoned cache" "Checksum mismatch"

install_case "a fresh install to cache" 0 "$action_release"
rm -f "$archive_asset"
run_install "a cache hit without the cached archive downloads it" 0 "$action_release" CACHE_HIT=true
[[ -f "$archive_asset" ]] || fail "install: the archive was not restored after a cache miss"

# ---------------------------------------------------------------- Action: run
cat >"${stubs}/skif" <<'STUB'
#!/usr/bin/env bash
echo "cwd=${PWD##*/} args=$*"
STUB
chmod +x "${stubs}/skif"
mkdir -p "${work}/project"

run_case() {
  local label="$1" want_out="$2"
  shift 2
  expect "run: ${label}" 0 env PATH="${stubs}:${PATH}" WORKING_DIRECTORY="${work}/project" "$@" bash scripts/gh-action-run.sh
  [[ "$(tail -n 1 "${work}/out")" == "cwd=project args=${want_out}" ]] || fail "run: ${label}: got '$(tail -n 1 "${work}/out")', want args '${want_out}'"
}
run_case "no arguments" "" ARGS= FAIL_ON_WARNINGS=false
run_case "arguments are split on spaces" "--format sarif --output skif.sarif" ARGS="--format sarif --output skif.sarif"
run_case "fail-on-warnings is appended" "--no-js --fail-on-warnings" ARGS="--no-js" FAIL_ON_WARNINGS=true
run_case "globs are passed through unexpanded" "*.json" ARGS="*.json"
run_case "one argument per line keeps spaces" "--config my dir/skif.toml --no-js" ARGS=$'--config\nmy dir/skif.toml\n--no-js'
run_case "blank lines and a trailing newline are dropped" "--no-js" ARGS=$'\n--no-js\n\n'

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "install scripts: all cases passed"
