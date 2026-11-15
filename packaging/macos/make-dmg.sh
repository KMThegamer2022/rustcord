#!/usr/bin/env bash
# Wraps Rustcord.app in a disk image with an Applications shortcut (drag to install).
# Usage: packaging/macos/make-dmg.sh <dist-dir>   (expects <dist-dir>/Rustcord.app)
set -euo pipefail

DIST="${1:-dist}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
STAGE="$(mktemp -d)"
DMG="$DIST/Rustcord-${VERSION}-macos.dmg"

cp -R "$DIST/Rustcord.app" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

rm -f "$DMG"
hdiutil create -volname "Rustcord ${VERSION}" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

echo "Built $DMG"
