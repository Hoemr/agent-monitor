#!/usr/bin/env bash
# Builds a minimal, runnable macOS .app bundle around the release binary.
# Tauri embeds the frontend (custom-protocol), so no extra resources are needed
# beyond the icon and Info.plist. A persistent signing identity is mandatory.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/sign-mac.py --check
VERSION=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
MINOS=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['bundle']['macOS']['minimumSystemVersion'])")
BIN=src-tauri/target/release/agent-monitor
APP="dist/Agent Monitor.app"
[ -x "$BIN" ] || { echo "build first: (cd src-tauri && cargo build --release)"; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/agent-monitor"

ICON_SRC=src-tauri/icons/icon.png
SET=/tmp/agent-monitor.iconset
rm -rf "$SET"; mkdir -p "$SET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$ICON_SRC" --out "$SET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$ICON_SRC" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$APP/Contents/Resources/icon.icns"
cp "$ICON_SRC" "$APP/Contents/Resources/icon.png"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Agent Monitor</string>
  <key>CFBundleDisplayName</key><string>Agent Monitor</string>
  <key>CFBundleIdentifier</key><string>com.agent.monitor</string>
  <key>CFBundleExecutable</key><string>agent-monitor</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>LSMinimumSystemVersion</key><string>${MINOS}</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSAppleEventsUsageDescription</key>
  <string>Agent Monitor sends Apple Events only to focus the window of the task you click.</string>
</dict>
</plist>
PLIST

python3 scripts/sign-mac.py "$APP"
echo "built: $APP"
echo "run:   open \"$APP\""
echo "Persistent signature: future builds must retain this identity and bundle identifier."
