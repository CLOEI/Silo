#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/release/silo}"
OUT_DIR="${2:-$ROOT/dist}"
VERSION="${VERSION:-0.0.0}"

APP="$OUT_DIR/Silo.app"
PLIST="$APP/Contents/Info.plist"
RESOURCES="$APP/Contents/Resources"

if [[ ! -x "$BIN" ]]; then
  echo "error: executable not found: $BIN" >&2
  exit 1
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$RESOURCES"

cp "$BIN" "$APP/Contents/MacOS/silo"
chmod +x "$APP/Contents/MacOS/silo"

ICONSET="$OUT_DIR/Silo.iconset"
SQUARE="$OUT_DIR/Silo-icon-1024.png"
SCALED="$OUT_DIR/Silo-icon-scaled.png"
rm -rf "$ICONSET" "$SCALED" "$SQUARE"
mkdir -p "$ICONSET"
sips -Z 1024 "$ROOT/assets/icon.png" --out "$SCALED" >/dev/null
sips --padToHeightWidth 1024 1024 --padColor FFFFFF "$SCALED" --out "$SQUARE" >/dev/null
sips -z 16 16 "$SQUARE" --out "$ICONSET/icon_16x16.png" >/dev/null
sips -z 32 32 "$SQUARE" --out "$ICONSET/icon_16x16@2x.png" >/dev/null
sips -z 32 32 "$SQUARE" --out "$ICONSET/icon_32x32.png" >/dev/null
sips -z 64 64 "$SQUARE" --out "$ICONSET/icon_32x32@2x.png" >/dev/null
sips -z 128 128 "$SQUARE" --out "$ICONSET/icon_128x128.png" >/dev/null
sips -z 256 256 "$SQUARE" --out "$ICONSET/icon_128x128@2x.png" >/dev/null
sips -z 256 256 "$SQUARE" --out "$ICONSET/icon_256x256.png" >/dev/null
sips -z 512 512 "$SQUARE" --out "$ICONSET/icon_256x256@2x.png" >/dev/null
sips -z 512 512 "$SQUARE" --out "$ICONSET/icon_512x512.png" >/dev/null
sips -z 1024 1024 "$SQUARE" --out "$ICONSET/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$ICONSET" -o "$RESOURCES/Silo.icns"
rm -rf "$ICONSET" "$SCALED" "$SQUARE"

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>silo</string>
    <key>CFBundleIconFile</key>
    <string>Silo</string>
    <key>CFBundleIdentifier</key>
    <string>com.cendy.silo</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Silo</string>
    <key>CFBundleDisplayName</key>
    <string>Silo</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleURLTypes</key>
    <array>
        <dict>
            <key>CFBundleURLName</key>
            <string>com.cendy.silo.nxm</string>
            <key>CFBundleURLSchemes</key>
            <array>
                <string>nxm</string>
            </array>
        </dict>
    </array>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

plutil -lint "$PLIST" >/dev/null

codesign --force --deep --sign - "$APP" >/dev/null

echo "built $APP"
