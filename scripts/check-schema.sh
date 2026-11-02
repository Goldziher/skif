#!/usr/bin/env bash
# Fails when schema/skif.schema.json differs from the schema `skif schema` prints (ADR 0004).
#
# Usage: scripts/check-schema.sh [path-to-skif-binary]
#
# Without an argument the binary is run through `cargo run`. Refresh the file with
# `cargo run -p skif-cli -- schema > schema/skif.schema.json`.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# -gt 0 ]]; then
  generated="$("$1" schema)"
else
  generated="$(cargo run --quiet --locked -p skif-cli -- schema)"
fi

if ! diff -u schema/skif.schema.json <(printf '%s\n' "$generated"); then
  echo "::error file=schema/skif.schema.json::schema/skif.schema.json is stale; regenerate it with 'cargo run -p skif-cli -- schema > schema/skif.schema.json'"
  exit 1
fi
echo "schema/skif.schema.json is current"
