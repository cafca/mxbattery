#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION=$(awk -F'"' '/^version *= */ {print $2; exit}' Cargo.toml)
BUNDLE_DIR="target/release/MXBattery.app"

cargo build --release --target aarch64-apple-darwin

rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/Contents/MacOS" "$BUNDLE_DIR/Contents/Resources"

sed "s/__VERSION__/$VERSION/g" tools/Info.plist.template > "$BUNDLE_DIR/Contents/Info.plist"
cp target/aarch64-apple-darwin/release/mxbattery "$BUNDLE_DIR/Contents/MacOS/mxbattery"

if [[ -f tools/AppIcon.icns ]]; then
    cp tools/AppIcon.icns "$BUNDLE_DIR/Contents/Resources/AppIcon.icns"
fi

if [[ -n "${DEVELOPER_ID:-}" ]]; then
    codesign --force --options runtime --sign "Developer ID Application: $DEVELOPER_ID" "$BUNDLE_DIR"
else
    codesign --force --sign - "$BUNDLE_DIR"
fi

echo "Built: $BUNDLE_DIR"
