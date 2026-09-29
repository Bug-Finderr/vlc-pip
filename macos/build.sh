#!/bin/sh
# Builds build/VLC PiP.app: universal (Apple Silicon + Intel), ad-hoc signed, menu-bar only.
# Usage: ./build.sh [install|package]
#   install  also copies the app to /Applications
#   package  also zips it to build/VLC-PiP-macOS-<version>.zip for a release
set -eu
cd "$(dirname "$0")"
version=0.1.0

# Both architectures build into the same products folder, so keep a copy of each.
mkdir -p build/bin
for arch in arm64 x86_64; do
    swift build -c release --triple "$arch-apple-macosx14.0"
    cp "$(swift build -c release --triple "$arch-apple-macosx14.0" --show-bin-path)/VLCPiP" "build/bin/VLCPiP-$arch"
done

app="build/VLC PiP.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
lipo -create build/bin/VLCPiP-arm64 build/bin/VLCPiP-x86_64 -output "$app/Contents/MacOS/VLCPiP"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>dev.vlc-pip.mac</string>
    <key>CFBundleName</key><string>VLC PiP</string>
    <key>CFBundleExecutable</key><string>VLCPiP</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSAppleEventsUsageDescription</key><string>VLC PiP controls VLC's play/pause and position from the PiP window.</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$app"
echo "built $app ($(lipo -archs "$app/Contents/MacOS/VLCPiP"))"

case "${1:-}" in
install)
    rm -rf "/Applications/VLC PiP.app"
    cp -R "$app" /Applications/
    echo "installed /Applications/VLC PiP.app"
    ;;
package)
    zip="build/VLC-PiP-macOS-$version.zip"
    rm -f "$zip"
    ditto -c -k --keepParent "$app" "$zip"  # ditto keeps the bundle's signature and metadata intact
    echo "packaged $zip"
    ;;
esac
