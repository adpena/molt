#!/bin/bash
# Build WASM SIMD binaries for browser compute engines.
#
# Produces two binaries from independent Rust and Zig sources:
#   simd-ops.wasm       — Rust (wide crate, 4x4 tiled matmul)
#   simd-ops-zig.wasm   — Zig (@Vector(4,f32), 15 exports)
#
# Both must produce bit-identical results (verified by differential tests).
#
# Usage:
#   ./build-simd.sh           # Build both
#   ./build-simd.sh rust      # Build Rust only
#   ./build-simd.sh zig       # Build Zig only
#
# Prerequisites:
#   - Rust with wasm32-unknown-unknown target: rustup target add wasm32-unknown-unknown
#   - Zig >= 0.14: brew install zig
#   - wasm-opt (optional): brew install binaryen

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="${1:-all}"

build_rust() {
  echo "==> Building Rust SIMD WASM..."
  cd "$SCRIPT_DIR/simd-ops-rs"
  cargo build --target wasm32-unknown-unknown --release
  cp target/wasm32-unknown-unknown/release/simd_ops.wasm "$SCRIPT_DIR/simd-ops.wasm"

  # Strip and optimize if tools available
  if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -Oz "$SCRIPT_DIR/simd-ops.wasm" -o "$SCRIPT_DIR/simd-ops.wasm"
    echo "    wasm-opt applied"
  fi

  local size
  size=$(wc -c < "$SCRIPT_DIR/simd-ops.wasm" | tr -d ' ')
  echo "    simd-ops.wasm: ${size} bytes"
}

build_zig() {
  echo "==> Building Zig SIMD WASM..."
  cd "$SCRIPT_DIR/simd-ops-zig"
  zig build-lib simd.zig -target wasm32-freestanding -O ReleaseSmall -femit-bin=simd.wasm
  cp simd.wasm "$SCRIPT_DIR/simd-ops-zig.wasm"

  if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -Oz "$SCRIPT_DIR/simd-ops-zig.wasm" -o "$SCRIPT_DIR/simd-ops-zig.wasm"
    echo "    wasm-opt applied"
  fi

  local size
  size=$(wc -c < "$SCRIPT_DIR/simd-ops-zig.wasm" | tr -d ' ')
  echo "    simd-ops-zig.wasm: ${size} bytes"
}

case "$TARGET" in
  rust) build_rust ;;
  zig)  build_zig ;;
  all)  build_rust; build_zig ;;
  *)    echo "Usage: $0 [rust|zig|all]"; exit 1 ;;
esac

echo "==> Done."
