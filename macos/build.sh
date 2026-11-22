#!/bin/sh
# Builds BatteryTray.app for macOS 13 and later, signed ad hoc, and the zip a release carries:
#   macos/build.sh [native|universal]     (default: universal, Apple Silicon and Intel in one app)
# Out: target/macos/BatteryTray.app and target/macos/BatteryTray-macOS.zip. Needs Rust with the
# aarch64-apple-darwin and x86_64-apple-darwin targets, the Xcode command line tools, and cargo-about for the crates'
# licenses in the app (`cargo install cargo-about --locked --features cli`; without it the app is built without them).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/target/macos"
APP="$OUT/BatteryTray.app"
WORK="$OUT/build"
SOURCES="$ROOT/macos/BatteryTray"
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1)

case "${1:-universal}" in
  native) ARCHS=$(uname -m) ;;
  universal) ARCHS="arm64 x86_64" ;;
  *) echo "usage: macos/build.sh [native|universal]" >&2; exit 2 ;;
esac

rm -rf "$APP" "$WORK"
mkdir -p "$WORK" "$APP/Contents/MacOS" "$APP/Contents/Resources"

BINARIES=""
for ARCH in $ARCHS; do
  case $ARCH in
    arm64) TARGET=aarch64-apple-darwin ;;
    x86_64) TARGET=x86_64-apple-darwin ;;
  esac
  echo "== core ($TARGET)"
  MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --release --locked -p batterytray-macos --target "$TARGET" \
    --manifest-path "$ROOT/Cargo.toml"
  echo "== app ($ARCH)"
  # shellcheck disable=SC2046
  swiftc -O -swift-version 5 -target "$ARCH-apple-macos13.0" \
    -import-objc-header "$SOURCES/core.h" $(ls "$SOURCES"/*.swift) \
    -L "$ROOT/target/$TARGET/release" -lbatterytray_macos \
    -framework IOKit -framework CoreFoundation -framework Foundation -framework AppKit -framework SwiftUI \
    -framework Carbon -framework GameController -framework UserNotifications -framework CoreAudio \
    -o "$WORK/BatteryTray-$ARCH"
  BINARIES="$BINARIES $WORK/BatteryTray-$ARCH"
done
# shellcheck disable=SC2086
lipo -create $BINARIES -output "$APP/Contents/MacOS/BatteryTray"

echo "== bundle $VERSION"
sed "s/@VERSION@/$VERSION/g" "$ROOT/macos/Info.plist" > "$APP/Contents/Info.plist"
swift "$ROOT/macos/make-icon.swift" "$WORK/AppIcon.iconset"
iconutil -c icns -o "$APP/Contents/Resources/AppIcon.icns" "$WORK/AppIcon.iconset"
# The licenses that come with the code: the app's own, its sources', the crates' in it
cp "$ROOT/LICENSE" "$ROOT/THIRD_PARTY_NOTICES.md" "$APP/Contents/Resources/"
if cargo about --version >/dev/null 2>&1; then
  cargo about generate --locked --manifest-path "$ROOT/macos/core/Cargo.toml" -c "$ROOT/macos/about.toml" \
    "$ROOT/macos/about.hbs" -o "$APP/Contents/Resources/THIRD_PARTY_LICENSES.md"
else
  echo "warning: cargo-about isn't installed: the app has no THIRD_PARTY_LICENSES.md, a release must not go out so" >&2
fi
# Ad hoc: no Developer ID. macOS asks once whether to open an app downloaded from the internet
codesign --force --sign - "$APP"
codesign --verify --strict "$APP"

rm -f "$OUT/BatteryTray-macOS.zip"
ditto -c -k --keepParent "$APP" "$OUT/BatteryTray-macOS.zip"
echo "== $APP"
echo "== $OUT/BatteryTray-macOS.zip"
