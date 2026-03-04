#!/bin/bash
# Build standalone executables for macOS ARM and Windows x86_64
# Requires: cargo, rustup, Docker (for Windows cross-compilation via cross)
# Optional: cargo install cross

set -e
cd "$(dirname "$0")"

VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
DIST="dist"
mkdir -p "$DIST"

echo "Building S.H.I.T v$VERSION"
echo "========================="

# --- macOS ARM (aarch64-apple-darwin) ---
echo ""
echo ">>> Building for macOS ARM (aarch64-apple-darwin)..."
if [[ $(uname -m) == "arm64" ]]; then
  cargo build --release
  cp target/release/shit "$DIST/shit-macos-aarch64"
  echo "    -> $DIST/shit-macos-aarch64"
else
  echo "    Cross-compiling (host is $(uname -m))..."
  rustup target add aarch64-apple-darwin 2>/dev/null || true
  cargo build --release --target aarch64-apple-darwin
  cp target/aarch64-apple-darwin/release/shit "$DIST/shit-macos-aarch64"
  echo "    -> $DIST/shit-macos-aarch64"
fi

# --- Windows x86_64 ---
echo ""
echo ">>> Building for Windows x86_64..."
if command -v cargo-zigbuild &>/dev/null; then
  cargo zigbuild --release --target x86_64-pc-windows-gnu
  cp target/x86_64-pc-windows-gnu/release/shit.exe "$DIST/shit-windows-x86_64.exe"
  echo "    -> $DIST/shit-windows-x86_64.exe"
elif command -v cross &>/dev/null; then
  cross build --release --target x86_64-pc-windows-gnu
  cp target/x86_64-pc-windows-gnu/release/shit.exe "$DIST/shit-windows-x86_64.exe"
  echo "    -> $DIST/shit-windows-x86_64.exe"
else
  echo "    Neither cargo-zigbuild nor cross found."
  echo "    Install cargo-zigbuild (recommended): cargo install cargo-zigbuild"
  echo "    Requires: zig (brew install zig), rustup target add x86_64-pc-windows-gnu"
  exit 1
fi

echo ""
echo "Done. Artifacts in $DIST/:"
ls -la "$DIST/"
