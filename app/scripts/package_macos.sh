#!/usr/bin/env bash
# Build Fathom.app and drag-to-Applications DMGs for macOS.
#
# Layout follows MiscKit: window background and .DS_Store are baked once under
# packaging/dmg/, packaging only copies them into the staging folder, so no
# Finder/AppleScript control is needed and it works headless.
#
# Architectures are packaged separately:
#   dist/Fathom-v0.0.4-arm64.dmg
#   dist/Fathom-v0.0.4-x86_64.dmg
#
# Usage:
#   ./scripts/package_macos.sh
#   ARCHES=arm64 ./scripts/package_macos.sh
#   ARCHES=x86_64 ./scripts/package_macos.sh
#
# Environment:
#   ARCHES           all (default) = arm64 + x86_64, or one of them
#   BIN              cargo binary (default: fp-desktop)
#   APP_NAME         bundle name (default: Fathom)
#   BUNDLE_ID        default: app.fathom.desktop
#   VERSION          default: 0.0.14
#   BUILD_NUMBER     default: 2
#   MIN_MACOS        deployment target and LSMinimumSystemVersion (default: 11.0)
#   CARGO            cargo executable; its sibling rustc is put first in PATH
#   FATHOM_SIGN_IDENTITY  explicit signing identity; otherwise auto-detect
#                    Apple Development / Developer ID Application.
#   APPLE_ID / APPLE_TEAM_ID / APPLE_PASSWORD  optional notarization credentials.
#   MAKE_DMG         1 = produce DMG (default 1)
set -euo pipefail

cd "$(dirname "$0")/.."

BIN="${BIN:-fp-desktop}"
APP_NAME="${APP_NAME:-Fathom}"
BUNDLE_ID="${BUNDLE_ID:-app.fathom.desktop}"
VERSION="${VERSION:-0.0.4}"
BUILD_NUMBER="${BUILD_NUMBER:-3}"
MIN_MACOS="${MIN_MACOS:-11.0}"
MAKE_DMG="${MAKE_DMG:-1}"
DIST="dist"
ARCHES="${ARCHES:-all}"

if [ "$ARCHES" = "all" ]; then
  ARCHES="arm64 x86_64"
fi

CARGO="${CARGO:-cargo}"
CARGO_BIN="$(command -v "$CARGO")"
export PATH="$(cd "$(dirname "$CARGO_BIN")" && pwd):$PATH"
RUSTC_BIN="$(dirname "$CARGO_BIN")/rustc"
[ -x "$RUSTC_BIN" ] || RUSTC_BIN="$(command -v rustc)"

# Cargo uses rustc from PATH; exporting the deployment target also makes C build
# scripts (objc2 / wgpu) link against the same minimum system version.
export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS"

if [ ! -f assets/AppIcon.icns ]; then
  echo "缺少 assets/AppIcon.icns" >&2
  exit 1
fi

for ARCH in $ARCHES; do
  case "$ARCH" in
    arm64) TARGET="aarch64-apple-darwin" ;;
    x86_64) TARGET="x86_64-apple-darwin" ;;
    *) echo "未知架构：$ARCH（可选 arm64 / x86_64 / all）" >&2; exit 1 ;;
  esac
done

# Fail before a long build when the x86_64 standard library is missing.
if [[ " $ARCHES " == *" x86_64 "* ]]; then
  X86_LIBDIR="$("$RUSTC_BIN" --print target-libdir --target x86_64-apple-darwin 2>/dev/null || true)"
  if ! compgen -G "$X86_LIBDIR/libstd-*.rlib" >/dev/null; then
    echo "缺少 x86_64-apple-darwin 标准库。" >&2
    echo "请安装对应 Rust 目标，例如：rustup target add x86_64-apple-darwin" >&2
    exit 1
  fi
fi

# Pick a real identity when available. MiscKit's rule is deliberately the same:
# Apple Development works locally and keeps the designated requirement stable;
# a Developer ID is used automatically when installed.
IDENTITY="${FATHOM_SIGN_IDENTITY:-${SIGN_IDENTITY:-}}"
if [ -z "$IDENTITY" ]; then
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
    | grep -oE '"(Apple Development|Developer ID Application): [^"]*"' \
    | head -1 | sed 's/^"//; s/"$//' || true)"
fi

STAGING=""
VERIFY=""
MOUNT=""
RW_DMG=""
ICON_TMP=""
cleanup_dmg() {
  set +e
  [ -n "${MOUNT:-}" ] && hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1 || true
  [ -n "${VERIFY:-}" ] && hdiutil detach "$VERIFY" -quiet >/dev/null 2>&1 || true
  [ -n "${STAGING:-}" ] && rm -rf "$STAGING"
  [ -n "${VERIFY:-}" ] && rm -rf "$VERIFY"
  [ -n "${MOUNT:-}" ] && rm -rf "$MOUNT"
  [ -n "${RW_DMG:-}" ] && rm -f "$RW_DMG"
  [ -n "${ICON_TMP:-}" ] && rm -f "$ICON_TMP" "$ICON_TMP.rsrc"
}
trap 'status=$?; cleanup_dmg; exit $status' EXIT

package_arch() {
  local arch="$1"
  local target="$2"
  local app="$DIST/$APP_NAME-$arch.app"
  local app_bin="target/$target/release/$BIN"
  local dmg="$DIST/$APP_NAME-v$VERSION-$arch.dmg"

  echo
  echo "==> $arch: cargo build --release --target $target ($BIN)"
  "$CARGO" build --release --locked -p fp-desktop --bin "$BIN" --target "$target"

  echo "==> $arch: assemble $app"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$app_bin" "$app/Contents/MacOS/$APP_NAME"
  cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>zh_CN</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME 方寸</string>
  <key>CFBundleExecutable</key><string>$APP_NAME</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleLocalizations</key>
  <array><string>zh-Hans</string><string>en</string></array>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleSupportedPlatforms</key>
  <array><string>MacOSX</string></array>
  <key>CFBundleVersion</key><string>$BUILD_NUMBER</string>
  <key>LSMinimumSystemVersion</key><string>$MIN_MACOS</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Copyright © 2026 Fathom Contributors.</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
  plutil -lint "$app/Contents/Info.plist" >/dev/null

  local actual_arch minos
  actual_arch="$(lipo -archs "$app/Contents/MacOS/$APP_NAME")"
  if [ "$actual_arch" != "$arch" ]; then
    echo "架构不符：期望 $arch，实际 $actual_arch" >&2
    exit 1
  fi
  minos="$(vtool -show-build "$app/Contents/MacOS/$APP_NAME" | awk '/minos/{print $2; exit}')"
  if [ "$minos" != "$MIN_MACOS" ]; then
    echo "最低系统不符：期望 $MIN_MACOS，实际 $minos" >&2
    exit 1
  fi
  echo "    arch=$actual_arch minos=$minos"

  xattr -cr "$app" 2>/dev/null || true
  if [ -n "$IDENTITY" ]; then
    echo "==> $arch: sign: $IDENTITY"
    codesign --force --options runtime --sign "$IDENTITY" "$app"
  else
    echo "==> $arch: sign: ad-hoc（未找到 Apple Development / Developer ID 身份）"
    codesign --force --sign "-" "$app"
  fi
  codesign --verify --deep --strict "$app"
  echo "    $(codesign -dv --verbose=2 "$app" 2>&1 | grep -E 'Identifier=|Signature=' | tr '\n' ' ')"
  echo "    designated requirement: $(codesign -dr- "$app" 2>&1 | tail -1)"

  if [ "$MAKE_DMG" = "1" ]; then
    echo "==> $arch: dmg"
    STAGING="$(mktemp -d "${TMPDIR:-/tmp}/fathom-dmg.XXXXXX")"
    VERIFY="$(mktemp -d "${TMPDIR:-/tmp}/fathom-verify.XXXXXX")"
    MOUNT="$(mktemp -d "${TMPDIR:-/tmp}/fathom-mount.XXXXXX")"
    RW_DMG="$(mktemp "${TMPDIR:-/tmp}/fathom-rw.XXXXXX").dmg"

    ditto "$app" "$STAGING/$APP_NAME.app"
    ln -s /Applications "$STAGING/Applications"

    # Give the mounted volume the Fathom icon instead of the generic white disk.
    cp assets/AppIcon.icns "$STAGING/.VolumeIcon.icns"

    # Reuse the baked Finder layout (600×360, icons at 150/450,160). Missing
    # assets are a hard error: a silent fallback would ship a white box.
    LAYOUT="packaging/dmg/DS_Store"
    BACKGROUND="packaging/dmg/background.png"
    [ -f "$LAYOUT" ] || { echo "缺少 $LAYOUT（先运行 scripts/make-dmg-layout.sh）" >&2; exit 1; }
    [ -f "$BACKGROUND" ] || { echo "缺少 $BACKGROUND" >&2; exit 1; }
    cp "$BACKGROUND" "$STAGING/.background.png"
    cp "$LAYOUT" "$STAGING/.DS_Store"

    rm -f "$dmg" "$RW_DMG"
    # Build a writable image first so the volume-level custom-icon bit can be set
    # on the mounted root, then convert it to the compressed release image.
    hdiutil create -volname "$APP_NAME" -srcfolder "$STAGING" \
      -format UDRW -ov -quiet "$RW_DMG"
    hdiutil attach -readwrite -nobrowse -mountpoint "$MOUNT" -quiet "$RW_DMG"
    cp assets/AppIcon.icns "$MOUNT/.VolumeIcon.icns"
    SetFile -a C "$MOUNT"
    hdiutil detach "$MOUNT" -quiet
    MOUNT=""
    hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 \
      -ov -quiet -o "$dmg"
    rm -f "$RW_DMG"
    RW_DMG=""

    # Finder also needs a custom icon on the .dmg file itself, otherwise the
    # downloaded file still looks like a blank disk even though the mounted
    # volume is branded. Embed the .icns as a resource fork and set the C bit.
    ICON_TMP="$(mktemp "${TMPDIR:-/tmp}/fathom-icon.XXXXXX").icns"
    cp assets/AppIcon.icns "$ICON_TMP"
    sips -i "$ICON_TMP" >/dev/null
    DeRez -only icns "$ICON_TMP" > "$ICON_TMP.rsrc" 2>/dev/null
    Rez -append "$ICON_TMP.rsrc" -o "$dmg"
    SetFile -a C "$dmg"
    rm -f "$ICON_TMP" "$ICON_TMP.rsrc"
    ICON_TMP=""

    if [ -n "$IDENTITY" ]; then
      codesign --force --sign "$IDENTITY" "$dmg"
    fi
    SetFile -a C "$dmg"

    # Verify the two entry points and the hidden layout actually made it into
    # the image; this is the same terminal gate MiscKit uses.
    hdiutil attach -readonly -nobrowse -mountpoint "$VERIFY" -quiet "$dmg"
    test -d "$VERIFY/$APP_NAME.app" || { echo "DMG 内缺少 $APP_NAME.app" >&2; exit 1; }
    test -L "$VERIFY/Applications" || { echo "DMG 内缺少 Applications 快捷方式" >&2; exit 1; }
    test -f "$VERIFY/.DS_Store" || { echo "DMG 内缺少 .DS_Store" >&2; exit 1; }
    test -f "$VERIFY/.background.png" || { echo "DMG 内缺少背景图" >&2; exit 1; }
    test -f "$VERIFY/.VolumeIcon.icns" || { echo "DMG 内缺少卷图标" >&2; exit 1; }
    [[ "$(GetFileInfo -a "$dmg")" == *C* ]] \
      || { echo "DMG 文件没有自定义图标" >&2; exit 1; }
    [[ "$(GetFileInfo -a "$VERIFY")" == *C* ]] \
      || { echo "DMG 卷没有自定义图标" >&2; exit 1; }
    local dmg_arch
    dmg_arch="$(lipo -archs "$VERIFY/$APP_NAME.app/Contents/MacOS/$APP_NAME")"
    if [ "$dmg_arch" != "$arch" ]; then
      echo "DMG 内架构不符：期望 $arch，实际 $dmg_arch" >&2
      exit 1
    fi
    hdiutil detach "$VERIFY" -quiet
    rm -rf "$STAGING" "$VERIFY"
    STAGING=""
    VERIFY=""
    MOUNT=""
    echo "    $dmg"
  fi
}

for ARCH in $ARCHES; do
  case "$ARCH" in
    arm64) TARGET="aarch64-apple-darwin" ;;
    x86_64) TARGET="x86_64-apple-darwin" ;;
  esac
  package_arch "$ARCH" "$TARGET"
done

echo "==> notarize"
if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ]; then
  for ARCH in $ARCHES; do
    TARGET_DMG="$DIST/$APP_NAME-v$VERSION-$ARCH.dmg"
    [ "$MAKE_DMG" = "1" ] || continue
    xcrun notarytool submit "$TARGET_DMG" \
      --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_PASSWORD" --wait
    xcrun stapler staple "$TARGET_DMG"
    echo "    notarized and stapled: $TARGET_DMG"
  done
else
  echo "    skipped (APPLE_ID / APPLE_TEAM_ID / APPLE_PASSWORD not set)"
fi

echo "==> done"
for ARCH in $ARCHES; do
  echo "    $DIST/$APP_NAME-$ARCH.app"
  if [ "$MAKE_DMG" = "1" ]; then
    echo "    $DIST/$APP_NAME-v$VERSION-$ARCH.dmg"
  fi
done
