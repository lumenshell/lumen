#!/bin/sh
# Build Lumen.app, so macOS shows Lumen's icon in the Dock, the app switcher
# and Finder (a bare binary gets the generic one).
#
#   scripts/bundle-macos.sh             build in release mode, then make target/release/Lumen.app
#   scripts/bundle-macos.sh --no-build  just make the bundle from the existing binary
#
# Then drag target/release/Lumen.app into /Applications. For `lumen msg` in a
# shell, link the binary inside it:  ln -s /Applications/Lumen.app/Contents/MacOS/lumen /usr/local/bin/lumen
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

case "${1:-}" in
    --no-build) ;;
    "") cargo build --release --locked ;;
    *) echo "Usage: $0 [--no-build]" >&2; exit 2 ;;
esac

bin=target/release/lumen
[ -x "$bin" ] || { echo "No $bin: build it first (cargo build --release)." >&2; exit 1; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

app=target/release/Lumen.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/lumen"
cp assets/icons/macos/Lumen.icns "$app/Contents/Resources/Lumen.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Lumen</string>
    <key>CFBundleDisplayName</key><string>Lumen</string>
    <key>CFBundleIdentifier</key><string>dev.lumenshell.lumen</string>
    <key>CFBundleExecutable</key><string>lumen</string>
    <key>CFBundleIconFile</key><string>Lumen</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>10.15.7</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Finder caches icons; touching the bundle makes it look again.
touch "$app"
echo "Made $app"
