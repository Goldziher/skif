#!/usr/bin/env bash
# Tests the scripts that put a skif binary in front of a user, against stub release servers and a stub `gh`:
#   pre-commit/run-skif.sh   resolution order, a non-executable SKIF_BINARY, a corrupted archive, a missing checksum
#   gh-action-resolve.sh     tag and asset selection per runner
#   gh-action-install.sh     checksum verification, extraction, outputs, the cache-hit path
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
basic_path="/usr/bin:/bin"
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
  [[ -n "${STUB_FAIL_DOWNLOAD:-}" ]] && { echo "gh must not be called on a cache hit" >&2; exit 1; }
  while [[ $# -gt 0 ]]; do
    case "$1" in
    -p) cp "${STUB_RELEASE:?}/$2" . ; shift 2 ;;
    *) shift ;;
    esac
  done
  ;;
esac
STUB
chmod +x "${stubs}/gh"

# resolve_case <label> <expected exit> <RUNNER_OS> <RUNNER_ARCH> <VERSION_INPUT> <ACTION_REF>; outputs in ${work}/gh-output.
resolve_case() {
  local label="$1" want="$2" os="$3" arch="$4" input="$5" ref="$6"
  : >"${work}/gh-output"
  expect "resolve: ${label}" "$want" env PATH="${stubs}:${PATH}" GITHUB_OUTPUT="${work}/gh-output" RUNNER_TEMP="${work}/runner" \
    RUNNER_OS="$os" RUNNER_ARCH="$arch" VERSION_INPUT="$input" ACTION_REF="$ref" REPOSITORY=Acme/skif \
    STUB_TAGS="v1.0.0 v0.2.1 v0.2.0 v0.1.0 v0" bash scripts/gh-action-resolve.sh
}
output_of() { sed -n "s/^$1=//p" "${work}/gh-output"; }

resolve_case "an exact version" 0 macOS ARM64 0.1.0 main
[[ "$(output_of tag)" == v0.1.0 && "$(output_of asset)" == skif-aarch64-apple-darwin.tar.gz ]] || fail "resolve: exact version on macOS arm64"
resolve_case "a v-prefixed version" 0 macOS X64 v0.1.0 ""
[[ "$(output_of tag)" == v0.1.0 && "$(output_of asset)" == skif-x86_64-apple-darwin.tar.gz ]] || fail "resolve: v-prefixed version on macOS x64"
resolve_case "a floating major tag resolves to the newest 0.x" 0 Windows X64 "" v0
[[ "$(output_of tag)" == v0.2.1 && "$(output_of asset)" == skif-x86_64-pc-windows-msvc.zip && "$(output_of ext)" == zip ]] ||
  fail "resolve: floating major on Windows x64"
resolve_case "an action pinned to a full tag" 0 Windows ARM64 "" v0.2.0
[[ "$(output_of tag)" == v0.2.0 && "$(output_of asset)" == skif-aarch64-pc-windows-msvc.zip ]] || fail "resolve: full tag on Windows arm64"
resolve_case "a branch ref falls back to the newest release" 0 Linux X64 "" main
[[ "$(output_of tag)" == v1.0.0 ]] || fail "resolve: latest"
[[ "$(output_of asset)" =~ ^skif-x86_64-unknown-linux-(gnu|musl)\.tar\.gz$ ]] || fail "resolve: Linux x64 asset: $(output_of asset)"
resolve_case "Linux arm64" 0 Linux ARM64 0.1.0 ""
[[ "$(output_of asset)" =~ ^skif-aarch64-unknown-linux-(gnu|musl)\.tar\.gz$ ]] || fail "resolve: Linux arm64 asset"
[[ "$(output_of checksums)" == checksums.txt && "$(output_of repo)" == Acme/skif ]] || fail "resolve: repo or checksums output"
[[ "$(output_of bin-dir)" == "${work}/runner/skif-action/v0.1.0-Linux-ARM64/bin" ]] || fail "resolve: bin-dir $(output_of bin-dir)"
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

# install_case <label> <expected exit> <release dir> [VAR=value...]; BIN_DIR is ${work}/action-bin.
install_case() {
  local label="$1" want="$2" release_dir="$3"
  shift 3
  rm -rf "${work}/action-bin"
  : >"${work}/gh-output"
  : >"${work}/gh-path"
  expect "install: ${label}" "$want" env PATH="${stubs}:${PATH}" STUB_RELEASE="$release_dir" RUNNER_OS=Linux \
    GITHUB_OUTPUT="${work}/gh-output" GITHUB_PATH="${work}/gh-path" REPO=Acme/skif TAG=v1.2.3 \
    ASSET=skif-x86_64-unknown-linux-gnu.tar.gz EXT=tar.gz CHECKSUMS=checksums.txt BIN_DIR="${work}/action-bin" \
    CACHE_HIT=false "$@" bash scripts/gh-action-install.sh
}

install_case "a verified archive" 0 "$action_release"
[[ -x "${work}/action-bin/skif" ]] || fail "install: skif was not installed"
[[ "$(output_of version)" == 1.2.3 && "$(output_of path)" == "${work}/action-bin/skif" ]] || fail "install: outputs"
grep -qxF "${work}/action-bin" "${work}/gh-path" || fail "install: BIN_DIR is not on PATH"
[[ ! -e "${work}/action-bin/LICENSE" ]] || fail "install: only the binary may land in the PATH directory"

tampered="${work}/action-tampered"
cp -R "$action_release" "$tampered"
echo corrupt >>"${tampered}/skif-x86_64-unknown-linux-gnu.tar.gz"
install_case "a checksum mismatch" 1 "$tampered"
expect_in_out "install: checksum mismatch" "Checksum mismatch"
[[ ! -e "${work}/action-bin/skif" ]] || fail "install: a tampered archive was installed"

printf '%064d  other.tar.gz\n' 1 >"${tampered}/checksums.txt"
install_case "no checksum entry" 1 "$tampered"
expect_in_out "install: no checksum entry" "No checksum entry"

install_case "an unknown archive type" 1 "$action_release" EXT=rar
install_case "a cache hit downloads nothing" 0 "$action_release" CACHE_HIT=true STUB_FAIL_DOWNLOAD=1
[[ "$(output_of version)" == 1.2.3 ]] || fail "install: a cache hit still sets the outputs"

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

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "install scripts: all cases passed"
