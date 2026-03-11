#!/bin/bash
# Build DERMO as a macOS .app bundle and optionally create a DMG for distribution.
# Run from project root: ./scripts/build-macos-app.sh [--dmg]

set -e
cd "$(dirname "$0")/.."

VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
APP_NAME="DERMO"
BUNDLE_ID="com.dermo.app"
DIST="dist"
APP="$DIST/${APP_NAME}.app"
DMG_NAME="DERMO-${VERSION}.dmg"

echo "Building ${APP_NAME} v${VERSION} for macOS"
echo "=========================================="

# Build release binary
echo ""
echo ">>> Building release binary..."
cargo build --release
BINARY="target/release/dermo"

# Create .app bundle structure
echo ""
echo ">>> Creating .app bundle..."
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"

# Copy binary
cp "$BINARY" "$APP/Contents/MacOS/$APP_NAME"
chmod +x "$APP/Contents/MacOS/$APP_NAME"

# Copy default_layout.ini (app looks for it next to executable)
if [[ -f default_layout.ini ]]; then
  cp default_layout.ini "$APP/Contents/MacOS/"
fi

# Create icon first (needed for Info.plist) - uses Rust tool with squircle mask + proper alpha
ICON_PLIST=""
ICONSET="$DIST/icon.iconset"
if [[ -f assets/Dermologo.jpg ]]; then
  echo ""
  echo ">>> Creating app icon (squircle mask, PNG with alpha)..."
  rm -rf "$ICONSET"
  cargo run --bin build_icon --quiet -- assets/Dermologo.jpg "$ICONSET"
  if iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Icon.icns" 2>/dev/null; then
    ICON_PLIST="    <key>CFBundleIconFile</key>
    <string>Icon.icns</string>
"
    echo "    Icon created successfully"
  fi
  rm -rf "$ICONSET"
fi

# Create Info.plist
cat > "$APP/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
${ICON_PLIST}</dict>
</plist>
EOF

echo ""
echo ">>> Created $APP"
echo "    Your friend can run it by double-clicking, or drag to Applications."

# Create DMG if requested
if [[ "$1" == "--dmg" ]]; then
  echo ""
  echo ">>> Creating DMG..."
  DMG_DIR="$DIST/dmg-build"
  rm -rf "$DMG_DIR"
  mkdir -p "$DMG_DIR"
  cp -R "$APP" "$DMG_DIR/"
  ln -s /Applications "$DMG_DIR/Applications"
  
  hdiutil create -volname "DERMO $VERSION" -srcfolder "$DMG_DIR" -ov -format UDZO "$DIST/$DMG_NAME"
  rm -rf "$DMG_DIR"
  echo "    -> $DIST/$DMG_NAME"
  echo ""
  echo "Share the DMG with your friend. They can:"
  echo "  1. Mount the DMG (double-click)"
  echo "  2. Drag DERMO to Applications"
  echo "  3. Eject the DMG"
fi

echo ""
echo "Done. App: $APP"
ls -la "$APP/Contents/MacOS/"
