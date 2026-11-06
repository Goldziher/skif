#!/usr/bin/env bash
# Renders the committed PNG brand assets from their SVG sources, so the PNGs stay reproducible.
#
# Usage: scripts/render-assets.sh
#
# Needs rsvg-convert (librsvg). The SVGs hold outlined paths only, so the output does not depend on installed fonts.
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v rsvg-convert >/dev/null 2>&1; then
  echo "::error::rsvg-convert not found; install librsvg" >&2
  exit 1
fi

render() {
  local source="$1" target="$2" width="$3" height="$4"
  shift 4
  rsvg-convert --width "$width" --height "$height" "$@" --output "$target" "$source"
  echo "rendered ${target} (${width}x${height})"
}

render website/src/assets/banner.svg website/public/og.png 1280 640
cp website/public/og.png docs/media/skif-social.png
echo "copied website/public/og.png to docs/media/skif-social.png"

render website/public/favicon.svg website/public/favicon-32.png 32 32
# iOS shows transparency as black, so the tile's rounded corners are filled with its background colour.
render website/public/favicon.svg website/public/apple-touch-icon.png 180 180 --background-color=#0e0b08
