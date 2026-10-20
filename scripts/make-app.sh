#!/usr/bin/env bash
# Builds PRmator.app (the app plus its background Claude bridge) and installs it.
#
#   scripts/make-app.sh                  # → ~/Apps/PRmator.app
#   DEST=/Applications scripts/make-app.sh
#
# The icon comes from assets/icon/icon.png; regenerate it with
#   swift assets/icon/make_icon.swift assets/icon/icon.png
set -euo pipefail

cd "$(dirname "$0")/.."
DEST="${DEST:-$HOME/Apps}"
NAME="PRmator"
BUNDLE_ID="dev.prmator.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)"

echo "▸ Building release binaries (v$VERSION)"
cargo build --release -p prmator -p prmator-claude

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/$NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# The app starts `prmator-claude` from next to its own executable.
cp target/release/prmator target/release/prmator-claude "$APP/Contents/MacOS/"

echo "▸ Building icon"
ICON_PNG="assets/icon/icon.png"
[ -f "$ICON_PNG" ] || swift assets/icon/make_icon.swift "$ICON_PNG"
ICONSET="$WORK/AppIcon.iconset"
mkdir "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$ICON_PNG" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$ICON_PNG" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat >"$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>                 <string>$NAME</string>
    <key>CFBundleDisplayName</key>          <string>$NAME</string>
    <key>CFBundleIdentifier</key>           <string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key>           <string>prmator</string>
    <key>CFBundleIconFile</key>             <string>AppIcon</string>
    <key>CFBundlePackageType</key>          <string>APPL</string>
    <key>CFBundleShortVersionString</key>   <string>$VERSION</string>
    <key>CFBundleVersion</key>              <string>$VERSION</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key>       <string>13.0</string>
    <key>LSApplicationCategoryType</key>    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>      <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

echo "▸ Signing (ad hoc)"
codesign --force --deep --sign - "$APP" >/dev/null 2>&1

mkdir -p "$DEST"
rm -rf "$DEST/$NAME.app"
mv "$APP" "$DEST/"
touch "$DEST/$NAME.app" # nudge Finder to pick up the new icon

echo "✓ Installed $DEST/$NAME.app"
echo "  open \"$DEST/$NAME.app\""
