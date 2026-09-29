#!/bin/sh
# Builds build/VLC PiP.app (ad-hoc signed, menu-bar only). Usage: ./build.sh [install]
set -eu
cd "$(dirname "$0")"
swift build -c release
app="build/VLC PiP.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp .build/release/VLCPiP "$app/Contents/MacOS/VLCPiP"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>dev.vlc-pip.mac</string>
    <key>CFBundleName</key><string>VLC PiP</string>
    <key>CFBundleExecutable</key><string>VLCPiP</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSAppleEventsUsageDescription</key><string>VLC PiP controls VLC's play/pause and position from the PiP window.</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$app"
echo "built $app"
if [ "${1:-}" = install ]; then
    rm -rf "/Applications/VLC PiP.app"
    cp -R "$app" /Applications/
    echo "installed /Applications/VLC PiP.app"
fi
