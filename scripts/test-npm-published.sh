#!/usr/bin/env bash
# Tests the npm registry probes against a stub `npm`:
#   npm-published.sh             a definite 404 is "unpublished", a version that exists is "published", and a rate
#                                limit is retried and never mistaken for "unpublished"
#   check-npm-packages-exist.sh  the umbrella and every platform package napi derives from napi.targets must exist
set -euo pipefail

cd "$(dirname "$0")/.."
probe="$PWD/scripts/npm-published.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "${work}/bin"

# The stub replays the lines of $STUB_SCRIPT, one per call: "<exit status>|<output>"; the last line repeats.
cat >"${work}/bin/npm" <<'STUB'
#!/usr/bin/env bash
count_file="${STUB_COUNT:?}"
count=$(($(cat "$count_file" 2>/dev/null || echo 0) + 1))
echo "$count" >"$count_file"
line="$(sed -n "${count}p" "${STUB_SCRIPT:?}")"
[[ -n "$line" ]] || line="$(tail -n 1 "${STUB_SCRIPT}")"
status="${line%%|*}"
output="${line#*|}"
[[ -z "$output" ]] || echo "$output"
exit "$status"
STUB
chmod +x "${work}/bin/npm"

failures=0

# probe_case <label> <expected stdout> <expected exit> <stub lines...>
probe_case() {
  local label="$1" want_out="$2" want_status="$3" got_out status=0
  shift 3
  printf '%s\n' "$@" >"${work}/script"
  rm -f "${work}/count"
  got_out="$(PATH="${work}/bin:${PATH}" STUB_SCRIPT="${work}/script" STUB_COUNT="${work}/count" \
    NPM_PROBE_ATTEMPTS=3 NPM_PROBE_DELAY=0 bash "$probe" skif 1.2.3 2>/dev/null)" || status=$?
  if [[ "$got_out" != "$want_out" || "$status" -ne "$want_status" ]]; then
    echo "FAIL ${label}: got '${got_out}' (exit ${status}), want '${want_out}' (exit ${want_status})" >&2
    failures=$((failures + 1))
  fi
}

probe_case "version exists" "published" 0 "0|1.2.3"
probe_case "package or version unknown" "unpublished" 0 "1|npm error code E404"
probe_case "rate limit then success" "published" 0 "1|npm error code E429" "0|1.2.3"
probe_case "rate limit then 404" "unpublished" 0 "1|npm error 429 Too Many Requests" "1|npm error code E404"
probe_case "persistent rate limit is not 'unpublished'" "" 2 "1|npm error 429 Too Many Requests"
probe_case "network failure is not 'unpublished'" "" 2 "1|npm error code ENOTFOUND"

# ---------------------------------------------------------------- check-npm-packages-exist
# This stub answers `npm view <name> name` from ${STUB_REGISTRY}/<name> ("<exit status>|<output>"); a name with no file
# is a 404. Every call is logged to ${STUB_REGISTRY}.log.
mkdir -p "${work}/exist-bin" "${work}/registry"
cat >"${work}/exist-bin/npm" <<'STUB'
#!/usr/bin/env bash
echo "$*" >>"${STUB_REGISTRY:?}.log"
[[ "$1 $3" == "view name" ]] || { echo "unexpected npm call: $*" >&2; exit 99; }
if [[ -f "${STUB_REGISTRY}/$2" ]]; then
  line="$(cat "${STUB_REGISTRY}/$2")"
  [[ -z "${line#*|}" ]] || echo "${line#*|}"
  exit "${line%%|*}"
fi
echo "npm error code E404" >&2
exit 1
STUB
chmod +x "${work}/exist-bin/npm"
cat >"${work}/package.json" <<'JSON'
{
  "name": "skif-cli",
  "napi": {
    "binaryName": "skif",
    "packageName": "skif",
    "targets": ["aarch64-apple-darwin", "x86_64-unknown-linux-musl", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"]
  }
}
JSON
all_packages=(skif-cli skif-darwin-arm64 skif-linux-x64-musl skif-win32-x64-msvc skif-win32-arm64-msvc)

# exist_case <label> <expected exit> <manifest>: runs the check against the stub registry, output to ${work}/exist-out.
exist_case() {
  local label="$1" want="$2" status=0
  : >"${work}/registry.log"
  PATH="${work}/exist-bin:${PATH}" STUB_REGISTRY="${work}/registry" NPM_PROBE_ATTEMPTS=2 NPM_PROBE_DELAY=0 \
    bash scripts/check-npm-packages-exist.sh "$3" >"${work}/exist-out" 2>&1 || status=$?
  if [[ "$status" -ne "$want" ]]; then
    echo "FAIL ${label}: exit ${status}, want ${want}" >&2
    sed 's/^/    /' "${work}/exist-out" >&2
    failures=$((failures + 1))
  fi
}
exist_expect() {
  grep -qF -- "$2" "${work}/exist-out" || {
    echo "FAIL $1: output does not contain '$2'" >&2
    failures=$((failures + 1))
  }
}

for name in "${all_packages[@]}"; do echo "0|${name}" >"${work}/registry/${name}"; done
exist_case "every package exists" 0 "${work}/package.json"
exist_expect "every package exists" "npm packages: all 5 exist"
for name in "${all_packages[@]}"; do
  grep -qx "view ${name} name" "${work}/registry.log" || {
    echo "FAIL every package exists: ${name} was not probed" >&2
    failures=$((failures + 1))
  }
done

rm "${work}/registry/skif-win32-x64-msvc" "${work}/registry/skif-win32-arm64-msvc"
exist_case "the Windows placeholders are missing" 1 "${work}/package.json"
exist_expect "missing x64" "::error::npm package skif-win32-x64-msvc does not exist"
exist_expect "missing arm64" "::error::npm package skif-win32-arm64-msvc does not exist"
exist_expect "missing: the others are reported" "skif-darwin-arm64: exists"

echo "0|skif-win32-x64-msvc" >"${work}/registry/skif-win32-x64-msvc"
echo "1|npm error code E429" >"${work}/registry/skif-win32-arm64-msvc"
exist_case "a rate limit is not 'exists' or 'missing'" 2 "${work}/package.json"
exist_expect "rate limit" "could not tell whether the npm package skif-win32-arm64-msvc exists"
[[ "$(grep -c "view skif-win32-arm64-msvc name" "${work}/registry.log")" -eq 2 ]] || {
  echo "FAIL rate limit: the probe was not retried" >&2
  failures=$((failures + 1))
}

jq '.napi.targets += ["riscv64gc-unknown-linux-gnu"]' "${work}/package.json" >"${work}/unknown.json"
exist_case "a target napi does not name" 2 "${work}/unknown.json"
exist_expect "unknown target" "napi target riscv64gc-unknown-linux-gnu has no known npm platform name"

# The real manifest: the umbrella plus its eight platform packages, the two Windows ones included.
for name in skif-cli skif-darwin-arm64 skif-darwin-x64 skif-linux-x64-gnu skif-linux-arm64-gnu skif-linux-x64-musl \
  skif-linux-arm64-musl skif-win32-x64-msvc skif-win32-arm64-msvc; do
  echo "0|${name}" >"${work}/registry/${name}"
done
exist_case "npm-package/package.json" 0 npm-package/package.json
exist_expect "npm-package/package.json" "npm packages: all 9 exist"
rm "${work}/registry/skif-linux-arm64-musl"
exist_case "npm-package/package.json without a platform package" 1 npm-package/package.json
exist_expect "npm-package/package.json missing" "npm package skif-linux-arm64-musl does not exist"

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "npm registry probes: all cases passed"
