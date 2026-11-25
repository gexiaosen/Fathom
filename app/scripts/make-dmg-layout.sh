#!/usr/bin/env bash
# Bake packaging/dmg/DS_Store once. You only need to rerun this when the
# background image, window size, or icon positions change.
#
# Requires dmgbuild (it writes the Finder metadata without driving Finder):
#   python3 -m pip install --user dmgbuild
#
# Usage:
#   MAKE_DMG=0 ./scripts/package_macos.sh   # ensure dist/Fathom.app exists
#   ./scripts/make-dmg-layout.sh
set -euo pipefail

cd "$(dirname "$0")/.."

APP="${1:-dist/Fathom.app}"
DMGBUILD="${DMGBUILD:-dmgbuild}"
[ -d "$APP" ] || { echo "找不到 $APP，先运行 MAKE_DMG=0 ./scripts/package_macos.sh" >&2; exit 1; }
command -v "$DMGBUILD" >/dev/null || {
  echo "找不到 dmgbuild；安装：python3 -m pip install --user dmgbuild" >&2
  exit 1
}
[ -f packaging/dmg/background.png ] || { echo "缺少 packaging/dmg/background.png" >&2; exit 1; }

TMP="$(mktemp -d "${TMPDIR:-/tmp}/fathom-layout.XXXXXX")"
DMG="$TMP/Fathom-layout.dmg"
MOUNT="$TMP/mnt"
cleanup() {
  hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

"$DMGBUILD" -s packaging/dmg/layout.py \
  -D app="$(pwd)/$APP" \
  -D background="$(pwd)/packaging/dmg/background.png" \
  Fathom "$DMG"

mkdir -p "$MOUNT"
hdiutil attach -readonly -nobrowse -mountpoint "$MOUNT" -quiet "$DMG"
[ -f "$MOUNT/.DS_Store" ] || { echo "dmgbuild 没有写出 .DS_Store" >&2; exit 1; }
cp "$MOUNT/.DS_Store" packaging/dmg/DS_Store
echo "已更新 packaging/dmg/DS_Store"
