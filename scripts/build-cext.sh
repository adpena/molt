#!/usr/bin/env bash
# scripts/build-cext.sh — Compile a Python C extension against the Molt CPython ABI.
#
# Usage:
#   ./scripts/build-cext.sh <source.c> [output_dir]
#   ./scripts/build-cext.sh runtime/molt-cpython-abi/tests/c_extensions/_testmolt.c
#
# Requirements:
#   - cargo build --release -p molt-lang-cpython-abi must have been run first
#   - CARGO_TARGET_DIR must be set or defaults to <repo>/target
#
# The script selects the correct CPython suffix for the current platform:
#   macOS:   .cpython-312-darwin.so  (x86_64 or arm64)
#   Linux:   .cpython-312-x86_64-linux-gnu.so
#   Windows: _testmolt.pyd (not yet supported — use clang-cl)

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="${1:-}"
OUTPUT_DIR="${2:-$(pwd)}"
CARGO_TARGET="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
PROFILE="${BUILD_PROFILE:-release}"
ABI_INCLUDE="$REPO_ROOT/runtime/molt-cpython-abi/include"
LIB_DIR="$CARGO_TARGET/$PROFILE"

if [[ -z "$SOURCE" ]]; then
    echo "Usage: $0 <source.c> [output_dir]" >&2
    exit 1
fi

if [[ ! -f "$SOURCE" ]]; then
    echo "Error: source file not found: $SOURCE" >&2
    exit 1
fi

# ── Platform detection ────────────────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin)
        case "$ARCH" in
            arm64)  CPYTHON_SUFFIX=".cpython-312-darwin.so" ;;
            x86_64) CPYTHON_SUFFIX=".cpython-312-darwin.so" ;;
            *)      CPYTHON_SUFFIX=".cpython-312-darwin.so" ;;
        esac
        LIBNAME="libmolt_cpython_abi.dylib"
        EXTRA_FLAGS="-Wl,-rpath,$LIB_DIR"
        ;;
    Linux)
        case "$ARCH" in
            aarch64) CPYTHON_SUFFIX=".cpython-312-aarch64-linux-gnu.so" ;;
            *)       CPYTHON_SUFFIX=".cpython-312-x86_64-linux-gnu.so" ;;
        esac
        LIBNAME="libmolt_cpython_abi.so"
        EXTRA_FLAGS="-Wl,-rpath,$LIB_DIR"
        ;;
    *)
        echo "Error: unsupported OS: $OS" >&2
        exit 1
        ;;
esac

# ── Derive output filename ────────────────────────────────────────────────────

BASENAME="$(basename "$SOURCE" .c)"
OUTPUT="$OUTPUT_DIR/${BASENAME}${CPYTHON_SUFFIX}"

# ── Check that the library has been built ────────────────────────────────────

if [[ ! -f "$LIB_DIR/$LIBNAME" ]]; then
    echo "Library not found: $LIB_DIR/$LIBNAME"
    echo "Building molt-lang-cpython-abi (release)..."
    cd "$REPO_ROOT"
    CARGO_TARGET_DIR="$CARGO_TARGET" cargo build --release -p molt-lang-cpython-abi
    echo "Build complete."
fi

# ── Compile the extension ─────────────────────────────────────────────────────

mkdir -p "$OUTPUT_DIR"

CC="${CC:-cc}"
CFLAGS="${CFLAGS:--O2 -fPIC -fvisibility=hidden}"

# SIMD optimization flags
case "$ARCH" in
    arm64|aarch64) SIMD_FLAGS="-mcpu=native" ;;
    x86_64)        SIMD_FLAGS="-march=native -msse4.1 -mavx2" ;;
    *)             SIMD_FLAGS="" ;;
esac

echo "Compiling: $SOURCE"
echo "  Include: $ABI_INCLUDE"
echo "  Library: $LIB_DIR/$LIBNAME"
echo "  Output:  $OUTPUT"

"$CC" $CFLAGS $SIMD_FLAGS \
    -shared \
    -I"$ABI_INCLUDE" \
    "$SOURCE" \
    -L"$LIB_DIR" \
    -lmolt_cpython_abi \
    $EXTRA_FLAGS \
    -o "$OUTPUT"

echo "Done: $OUTPUT"

# ── Quick symbol check ────────────────────────────────────────────────────────

if command -v nm &>/dev/null; then
    INIT_SYMBOL="PyInit_$BASENAME"
    if nm -g "$OUTPUT" 2>/dev/null | grep -q "$INIT_SYMBOL"; then
        echo "Symbol check: $INIT_SYMBOL ✓"
    else
        echo "Warning: $INIT_SYMBOL not found in $OUTPUT" >&2
    fi
fi
