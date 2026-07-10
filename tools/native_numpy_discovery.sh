#!/usr/bin/env bash
# tools/native_numpy_discovery.sh — Native CPython-ABI C-extension DISCOVERY engine.
#
# Drives a REAL prebuilt CPython 3.12 extension's PyInit against molt's ABI +
# REAL runtime hooks, NATIVELY, to surface every remaining ABI frontier at once
# with real backtraces — killing the ~30-min-per-leaf wasm-witness discovery loop.
#
#   linear wasm-witness discovery : ~1800 s / frontier
#   this engine (warm)            : ~seconds / frontier (edit -> next frontier)
#
# Re-runnable: after each inline fix, re-run; it rebuilds the harness
# incrementally and advances to the next frontier.
#
# Usage:
#   tools/native_numpy_discovery.sh [module_name]
#
# Environment:
#   NUMPY_SO         explicit path to the extension .so to drive
#                    (default: auto-detected numpy _multiarray_umath under
#                     $NUMPY_WHEEL_DIR or ~/molt-discovery-numpy/wheel)
#   NUMPY_WHEEL_DIR  dir to search for the extension (default ~/molt-discovery-numpy)
#   MOLT_DISCOVERY_PROFILE   cargo profile for the harness (default: dev)
#   MOLT_DISCOVERY_LLDB=1    run the driver under lldb --batch to capture hard
#                            native crashes (SIGSEGV/SIGABRT) with a full bt
#   CARGO_TARGET_DIR         (recommended) off-OneDrive fast volume
#
# Requires: a Unix host with real dlopen/RTLD_GLOBAL (Linux or macOS). On
# Windows this cannot work (no flat-namespace / RTLD_GLOBAL) — use WSL or a
# Linux/macOS box.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODULE="${1:-_multiarray_umath}"
PROFILE="${MOLT_DISCOVERY_PROFILE:-dev}"
WHEEL_DIR="${NUMPY_WHEEL_DIR:-$HOME/molt-discovery-numpy}"

OS="$(uname -s)"
case "$OS" in
    Darwin) DYLIB_EXT="dylib" ;;
    Linux)  DYLIB_EXT="so" ;;
    *) echo "FATAL: unsupported OS '$OS' — need Linux/macOS (real RTLD_GLOBAL)"; exit 2 ;;
esac

# ── 1. Locate the real extension .so ──────────────────────────────────────────
if [[ -z "${NUMPY_SO:-}" ]]; then
    NUMPY_SO="$(find "$WHEEL_DIR" -name "${MODULE}*.so" 2>/dev/null | head -1)"
fi
if [[ -z "${NUMPY_SO:-}" || ! -f "$NUMPY_SO" ]]; then
    echo "FATAL: extension .so for '$MODULE' not found."
    echo "  Set NUMPY_SO=/path/to/${MODULE}....so, or unpack a cp312 numpy wheel into $WHEEL_DIR."
    echo "  e.g.: pip download --only-binary=:all: --python-version 3.12 --abi cp312 \\"
    echo "          --platform macosx_11_0_arm64 numpy==1.26.4 -d $WHEEL_DIR && \\"
    echo "        (cd $WHEEL_DIR && unzip -o numpy-*.whl -d wheel)"
    exit 2
fi
echo "== extension:  $NUMPY_SO"
echo "== module:     $MODULE"
echo "== profile:    $PROFILE"

# ── 2. Build the harness cdylib (incremental after the first build) ───────────
echo "== building molt-cext-discovery ($PROFILE) ..."
BUILD_DIR="runtime"
if [[ "$PROFILE" == "dev" ]]; then
    PROFILE_DIR="debug"
    ( cd "$REPO_ROOT/$BUILD_DIR" && cargo build -p molt-cext-discovery ) || { echo "FATAL: harness build failed"; exit 3; }
else
    PROFILE_DIR="$PROFILE"
    ( cd "$REPO_ROOT/$BUILD_DIR" && cargo build -p molt-cext-discovery --profile "$PROFILE" ) || { echo "FATAL: harness build failed"; exit 3; }
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/$BUILD_DIR/target}"
HARNESS="$TARGET_DIR/$PROFILE_DIR/libmolt_cext_discovery.$DYLIB_EXT"
[[ -f "$HARNESS" ]] || { echo "FATAL: harness lib not found at $HARNESS"; exit 3; }
echo "== harness:    $HARNESS"

# ── 3. Symbol-gap frontier check (static, instant) ────────────────────────────
# Any Py* symbol numpy needs that the harness does NOT export is a guaranteed
# runtime frontier (an unresolved dynamic_lookup -> abort on first call).
echo "== static symbol-gap check ..."
NEEDED="$(mktemp)"; EXPORTED="$(mktemp)"; GAP="$(mktemp)"
# Reduce each symbol to its exact C name so `_Py_NoneStruct` (what numpy links)
# is NOT conflated with `Py_None` (what molt exports). On macOS the linker
# prepends exactly ONE `_` to every C name; strip exactly one. On ELF there is
# no prefix.
if [[ "$OS" == "Darwin" ]]; then
    nm -u  "$NUMPY_SO" 2>/dev/null | sed 's/^_//'            | grep -E '(^|_)Py' | sort -u > "$NEEDED"
    nm -gU "$HARNESS"  2>/dev/null | awk '{print $NF}' | sed 's/^_//' | grep -E '(^|_)Py' | sort -u > "$EXPORTED"
else
    nm -D --undefined-only "$NUMPY_SO" 2>/dev/null | awk '{print $NF}' | grep -E '(^|_)Py' | sort -u > "$NEEDED"
    nm -D --defined-only   "$HARNESS"  2>/dev/null | awk '{print $NF}' | grep -E '(^|_)Py' | sort -u > "$EXPORTED"
fi
comm -23 "$NEEDED" "$EXPORTED" > "$GAP"
echo "   numpy needs $(wc -l < "$NEEDED" | tr -d ' ') Py* symbols; harness exports $(wc -l < "$EXPORTED" | tr -d ' '); GAP=$(wc -l < "$GAP" | tr -d ' ')"
if [[ -s "$GAP" ]]; then
    echo "   --- MISSING Py* symbols (symbol-gap frontiers) ---"
    sed 's/^/     /' "$GAP"
fi

# ── 4. Compile the C driver ───────────────────────────────────────────────────
DRIVER_SRC="$REPO_ROOT/tools/native_cext_driver.c"
DRIVER_BIN="$TARGET_DIR/native_cext_driver"
cc -O0 -g "$DRIVER_SRC" -o "$DRIVER_BIN" -ldl 2>/dev/null || cc -O0 -g "$DRIVER_SRC" -o "$DRIVER_BIN" || { echo "FATAL: driver compile failed"; exit 4; }

# ── 5. Drive PyInit — capture the frontier ────────────────────────────────────
echo "== driving PyInit_$MODULE against molt ABI ..."
echo "======================================================================"
export RUST_BACKTRACE="${RUST_BACKTRACE:-full}"
# MOLT_TRACE_CAPI streams every instrumented C-API call + silent-failure to
# stderr — this is what pinpoints the exact init-time semantic frontier
# (e.g. `silent-failure PyCapsule_Import(datetime.datetime_CAPI)`).
export MOLT_TRACE_CAPI="${MOLT_TRACE_CAPI:-1}"
if [[ "${MOLT_DISCOVERY_LLDB:-0}" == "1" && "$OS" == "Darwin" ]]; then
    lldb --batch -o "run $HARNESS $NUMPY_SO $MODULE" -o "bt all" -o "quit" -- "$DRIVER_BIN"
    RC=$?
elif [[ "${MOLT_DISCOVERY_LLDB:-0}" == "1" ]]; then
    gdb --batch -ex "run $HARNESS $NUMPY_SO $MODULE" -ex "bt" --args "$DRIVER_BIN" "$HARNESS" "$NUMPY_SO" "$MODULE"
    RC=$?
else
    "$DRIVER_BIN" "$HARNESS" "$NUMPY_SO" "$MODULE"
    RC=$?
fi
echo "======================================================================"
echo "== driver exit code: $RC"
rm -f "$NEEDED" "$EXPORTED" "$GAP"
exit $RC
