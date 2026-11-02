#!/usr/bin/env bash
# Tests scripts/npm-published.sh against a stub `npm`: a definite 404 is "unpublished", a version that exists is
# "published", and a rate limit is retried and never mistaken for "unpublished".
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

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} case(s) failed" >&2
  exit 1
fi
echo "npm-published probe: all cases passed"
