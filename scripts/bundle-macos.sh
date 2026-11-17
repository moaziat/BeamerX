#!/bin/sh
# Build a release BeamerX.app for Apple Silicon into dist/.
set -eu
cd "$(dirname "$0")/.."
TARGET=aarch64-apple-darwin
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --release --target "$TARGET"
APP=dist/BeamerX.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$TARGET/release/beamerx" "$APP/Contents/MacOS/BeamerX"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>BeamerX</string>
  <key>CFBundleDisplayName</key><string>BeamerX</string>
  <key>CFBundleIdentifier</key><string>io.github.moaziat.beamerx</string>
  <key>CFBundleExecutable</key><string>BeamerX</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>BeamerX contributors. MIT OR Apache-2.0.</string>
</dict>
</plist>
PLIST
# Ad-hoc signature so Gatekeeper treats the bundle consistently on this Mac.
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "note: codesign unavailable; bundle is unsigned"
du -sh "$APP"
echo "built $APP"
