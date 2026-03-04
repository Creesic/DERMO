#!/usr/bin/env bash
# Build BootCommander (OpenBLT) for Linux and macOS.
# BootCommander is required for F1 (STM32F103) wideband firmware flashing via XCP/CAN.
# - On Linux: builds natively (requires libusb-1.0-0-dev, cmake, build-essential)
# - On macOS: builds natively (requires cmake, libusb via Homebrew). Use ext/openblt
#   which includes a Darwin port (serial/USB/network; CAN not supported).
# - If ext/openblt not found: clones upstream and uses Docker on macOS (Linux-only).
#
# Usage: ./scripts/build-bootcommander.sh [output_dir]
# Default output: ./dist

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT_DIR="${1:-$PROJECT_ROOT/dist}"
OPENBLT_SRC="${OPENBLT_SRC:-}"

# Try common OpenBLT locations (project ext/ has Darwin port for macOS)
for candidate in \
  "$PROJECT_ROOT/ext/openblt" \
  "/tmp/wideband/firmware/ext/openblt" \
  "$HOME/.cache/openblt"; do
  if [[ -d "$candidate/Host/Source/LibOpenBLT" ]]; then
    OPENBLT_SRC="$candidate"
    break
  fi
done

if [[ -z "$OPENBLT_SRC" ]]; then
  echo "OpenBLT source not found. Cloning..."
  CACHE_DIR="${OPENBLT_CACHE:-$HOME/.cache/openblt}"
  mkdir -p "$CACHE_DIR"
  if [[ ! -d "$CACHE_DIR/Host" ]]; then
    git clone --depth 1 https://github.com/feaser/openblt.git "$CACHE_DIR"
    # On macOS, apply Darwin port patch for native build
    if [[ "$(uname -s)" == "Darwin" ]] && [[ -f "$PROJECT_ROOT/patches/openblt-darwin.patch" ]]; then
      (cd "$CACHE_DIR" && git apply "$PROJECT_ROOT/patches/openblt-darwin.patch") || true
    fi
  fi
  OPENBLT_SRC="$CACHE_DIR"
fi

echo "Using OpenBLT at: $OPENBLT_SRC"
mkdir -p "$OUTPUT_DIR"
HOST_DIR="$OPENBLT_SRC/Host"

do_build() {
  local srcdir="$1"
  local outdir="$2"
  cd "$srcdir/Source/LibOpenBLT"
  mkdir -p build && cd build
  cmake .. && make -j4
  cd "$srcdir/Source/BootCommander"
  mkdir -p build && cd build
  cmake .. && make -j4
  cp "$srcdir/BootCommander" "$outdir/"
  if [[ -f "$srcdir/libopenblt.so" ]]; then
    cp "$srcdir/libopenblt.so" "$outdir/"
  fi
  if [[ -f "$srcdir/libopenblt.dylib" ]]; then
    cp "$srcdir/libopenblt.dylib" "$outdir/"
    # Ensure BootCommander finds libopenblt when run from outdir
    install_name_tool -add_rpath @executable_path "$outdir/BootCommander" 2>/dev/null || true
  fi
}

if [[ "$(uname -s)" == "Linux" ]]; then
  echo "Building BootCommander natively on Linux..."
  BUILD_DIR="$(mktemp -d)"
  trap "rm -rf '$BUILD_DIR'" EXIT
  cp -r "$HOST_DIR" "$BUILD_DIR/host"
  do_build "$BUILD_DIR/host" "$OUTPUT_DIR"
elif [[ "$(uname -s)" == "Darwin" ]] && [[ -d "$OPENBLT_SRC/Host/Source/LibOpenBLT/port/darwin" ]]; then
  echo "Building BootCommander natively on macOS (Darwin port)..."
  do_build "$HOST_DIR" "$OUTPUT_DIR"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  echo "Building BootCommander in Docker (upstream has no macOS port)..."
  docker run --rm \
  -v "$HOST_DIR:/src:ro" \
  -v "$OUTPUT_DIR:/out" \
  -w /build \
  ubuntu:22.04 bash -c '
    set -e
    apt-get update -qq && apt-get install -y -qq cmake build-essential libusb-1.0-0-dev
    cp -r /src /build/host
    cd /build/host/Source/LibOpenBLT
    mkdir -p build && cd build
    cmake .. && make -j4
    cd /build/host/Source/BootCommander
    mkdir -p build && cd build
    cmake .. && make -j4
    cp /build/host/BootCommander /out/
    cp /build/host/libopenblt.so /out/ 2>/dev/null || true
    echo "Built: /out/BootCommander"
  '
else
  echo "Unsupported platform: $(uname -s)"
  exit 1
fi

LIB_NAME="libopenblt.so"
[[ -f "$OUTPUT_DIR/libopenblt.dylib" ]] && LIB_NAME="libopenblt.dylib"
echo ""
echo "BootCommander built successfully."
echo "Binary: $OUTPUT_DIR/BootCommander"
echo "LibOpenBLT: $OUTPUT_DIR/$LIB_NAME (keep next to BootCommander when not in system path)"
echo ""
echo "To use with S.H.I.T wideband plugin:"
echo "  export PATH=\"$OUTPUT_DIR:\$PATH\""
echo "  # or copy BootCommander (and $LIB_NAME) to a directory already in PATH"
echo ""
