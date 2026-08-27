#!/usr/bin/env bash
# Build the navi-dart native library for the current platform.
# Usage: ./build.sh [--release]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROFILE="debug"
TARGET_DIR="$PROJECT_ROOT/target/debug"
DART_LIB_DIR="$SCRIPT_DIR/lib/src"

if [[ "${1:-}" == "--release" ]]; then
  PROFILE="release"
  TARGET_DIR="$PROJECT_ROOT/target/release"
fi

echo "Building navi-dart ($PROFILE)..."
cd "$PROJECT_ROOT"
cargo build -p navi-dart --profile "$PROFILE"

# Copy native library to the Dart package's lib/src directory
LIB_NAME="libnavi_dart"
if [[ "$(uname)" == "Darwin" ]]; then
  EXT="dylib"
elif [[ "$(uname)" == "Linux" ]]; then
  EXT="so"
else
  EXT="dll"
fi

SRC="$TARGET_DIR/${LIB_NAME}.${EXT}"
DST="$DART_LIB_DIR/${LIB_NAME}.${EXT}"

if [[ -f "$SRC" ]]; then
  cp "$SRC" "$DST"
  echo "Copied $SRC → $DST"
else
  echo "Error: $SRC not found"
  exit 1
fi

echo "Done. Run 'dart test' in $SCRIPT_DIR to verify."
