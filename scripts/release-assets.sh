#!/usr/bin/env bash
# Prints the names of the assets every skif release must carry, one per line: the eight platform
# archives and checksums.txt.
set -euo pipefail

targets=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  x86_64-pc-windows-msvc
  aarch64-pc-windows-msvc
)

for target in "${targets[@]}"; do
  case "$target" in
  *windows*) echo "skif-${target}.zip" ;;
  *) echo "skif-${target}.tar.gz" ;;
  esac
done
echo "checksums.txt"
