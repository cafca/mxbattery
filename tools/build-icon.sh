#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
SVG=app-icon-source.svg
ICONSET=AppIcon.iconset
mkdir -p "$ICONSET"
for SIZE in 16 32 64 128 256 512 1024; do
    rsvg-convert -w $SIZE -h $SIZE "$SVG" -o "$ICONSET/icon_${SIZE}x${SIZE}.png"
done
iconutil -c icns "$ICONSET"
rm -rf "$ICONSET"
