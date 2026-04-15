#!/usr/bin/env bash
# Build Falcon-OCR inference as WASM module.
#
# This script builds the WASM binary that the Cloudflare Worker loads
# for GPU-accelerated (via WebGPU) or CPU inference of the Falcon-OCR model.
#
# There are two compilation paths:
#
# 1. Rust molt-gpu crate -> WASM (primitive tensor ops)
#    Provides the low-level compute kernels. Available now.
#
# 2. Python inference code -> WASM via molt compiler
#    Compiles the full Falcon-OCR inference pipeline (attention, MLP,
#    tokenizer) from Python to WASM. Requires molt's WASM backend.
#    This is the target for full edge inference.
#
# Usage: ./build_wasm.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

export MOLT_SESSION_ID="wasm-build"
export CARGO_TARGET_DIR="$PROJECT_ROOT/target-wasm_build"

echo "=== Checking wasm32 target ==="
if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
    echo "Installing wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown
fi

echo ""
echo "=== Building molt-gpu WASM library ==="
echo "    Target dir: $CARGO_TARGET_DIR"

# Check if molt-gpu crate exists
if [ -f "$PROJECT_ROOT/runtime/molt-gpu/Cargo.toml" ]; then
    cargo build -p molt-gpu \
        --target wasm32-unknown-unknown \
        --release \
        --no-default-features \
        --features cpu-backend,wasm-backend 2>&1 || {
        echo ""
        echo "WARNING: molt-gpu WASM build failed. This is expected if the crate"
        echo "         does not yet support the wasm32 target or required features."
        echo ""
    }

    RLIB="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/libmolt_gpu.rlib"
    if [ -f "$RLIB" ]; then
        echo ""
        echo "=== WASM rlib built ==="
        ls -lh "$RLIB"
    fi
else
    echo "WARNING: molt-gpu crate not found at runtime/molt-gpu/Cargo.toml"
fi

echo ""
echo "=== Full WASM inference module ==="
echo "The complete Falcon-OCR WASM binary requires molt's Python-to-WASM pipeline:"
echo ""
echo "  molt build src/molt/stdlib/tinygrad/wasm_driver.py --target wasm \\"
echo "    --output deploy/cloudflare/falcon-ocr.wasm"
echo ""
echo "This compiles the Python inference code (vision transformer, attention,"
echo "MLP, tokenizer) to a standalone WASM module that exports ocr_tokens()."
echo ""
echo "Until then, the Worker operates in CPU fallback mode."
