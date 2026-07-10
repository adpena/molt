#!/usr/bin/env bash
# tools/native_witness_symbol_sweep.sh — WHOLE-witness ABI symbol-gap sweep.
#
# The companion to `tools/native_numpy_discovery.sh`. Where that engine drives
# ONE extension's `PyInit` to surface runtime-semantic frontiers, this sweeps
# EVERY extension `.so` the pact witness compute path loads (numpy core +
# numpy.linalg + scipy.ndimage) and reports, in ONE native pass (~1 s), every
# `Py*` ABI symbol each `.so` imports that molt's ABI does not export — the
# COMPLETE Tier-A frontier for the whole witness surface, all at once. This is
# what makes "batch-fix, not leaf-by-leaf" real for the symbol dimension.
#
# A missing DATA/function symbol is a GUARANTEED frontier: on macOS
# `dlopen(RTLD_NOW)` aborts on the first unresolved symbol; on ELF it is an
# unresolved-lookup trap on first call. So this static diff is exhaustive and
# exact — no need to reach the symbol at runtime to know it is a frontier.
#
# Usage:
#   CARGO_TARGET_DIR=<fast-dir> tools/native_witness_symbol_sweep.sh
#
# Environment:
#   NUMPY_WHEEL_DIR   unpacked cp312 numpy wheel root (default ~/molt-discovery-numpy/wheel)
#   SCIPY_WHEEL_DIR   unpacked cp312 scipy wheel root (default ~/molt-discovery-scipy/wheel)
#   MOLT_DISCOVERY_PROFILE   harness cargo profile (default dev)
#   CARGO_TARGET_DIR  (recommended) off-OneDrive fast volume
#
# Requires: Linux/macOS with `nm`. Reuses the `molt-cext-discovery` harness as
# the single authority for "what molt's ABI exports" (it links the real
# molt-cpython-abi + molt-runtime), so the exported set is exactly what a real
# molt native artifact would expose.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${MOLT_DISCOVERY_PROFILE:-dev}"
NUMPY_WHEEL_DIR="${NUMPY_WHEEL_DIR:-$HOME/molt-discovery-numpy/wheel}"
SCIPY_WHEEL_DIR="${SCIPY_WHEEL_DIR:-$HOME/molt-discovery-scipy/wheel}"

OS="$(uname -s)"
case "$OS" in
    Darwin) DYLIB_EXT="dylib"; STRIP1="sed s/^_//" ;;
    Linux)  DYLIB_EXT="so";    STRIP1="cat" ;;
    *) echo "FATAL: unsupported OS '$OS' — need Linux/macOS"; exit 2 ;;
esac

# ── 1. Build the harness cdylib (the ABI-export authority) ────────────────────
echo "== building molt-cext-discovery ($PROFILE) to enumerate molt's exported ABI ..."
if [[ "$PROFILE" == "dev" ]]; then
    PROFILE_DIR="debug"
    ( cd "$REPO_ROOT/runtime" && cargo build -p molt-cext-discovery ) || { echo "FATAL: harness build failed"; exit 3; }
else
    PROFILE_DIR="$PROFILE"
    ( cd "$REPO_ROOT/runtime" && cargo build -p molt-cext-discovery --profile "$PROFILE" ) || { echo "FATAL: harness build failed"; exit 3; }
fi
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/runtime/target}"
HARNESS="$TARGET_DIR/$PROFILE_DIR/libmolt_cext_discovery.$DYLIB_EXT"
[[ -f "$HARNESS" ]] || { echo "FATAL: harness lib not found at $HARNESS"; exit 3; }

undef_pysyms() { # $1 = .so -> undefined Py* C names, one per line, sorted-unique
    nm -u "$1" 2>/dev/null | $STRIP1 | grep -E '(^|_)Py' | sort -u
}
defined_pysyms() { # $1 = lib -> exported/defined Py* C names
    if [[ "$OS" == "Darwin" ]]; then
        nm -gU "$1" 2>/dev/null | awk '{print $NF}' | $STRIP1 | grep -E '(^|_)Py' | sort -u
    else
        nm -D --defined-only "$1" 2>/dev/null | awk '{print $NF}' | grep -E '(^|_)Py' | sort -u
    fi
}

EXPORTED="$(mktemp)"; defined_pysyms "$HARNESS" > "$EXPORTED"
echo "== molt ABI exports $(wc -l < "$EXPORTED" | tr -d ' ') Py* symbols (authority: $HARNESS)"
echo ""

# ── 2. The witness compute-path extension set (field_solve.py) ────────────────
# numpy: core ndarray+ufuncs; numpy.linalg (np.linalg.eigh); scipy.ndimage
# (distance_transform_edt / gaussian_filter / maximum_filter / minimum_filter /
#  label). Each row: "logical-name|path".
# Python C-extensions are ALWAYS `.so` (`<name>.cpython-<abitag>.so`), even on
# macOS — resolve each by glob so the platform ABI tag is irrelevant. Each row:
# "logical-name|dir|basename-prefix".
NP="$NUMPY_WHEEL_DIR"; SP="$SCIPY_WHEEL_DIR"
SOROWS=(
    "numpy.core._multiarray_umath|$NP/numpy/core|_multiarray_umath"
    "numpy.linalg._umath_linalg|$NP/numpy/linalg|_umath_linalg"
    "numpy.linalg.lapack_lite|$NP/numpy/linalg|lapack_lite"
    "scipy.ndimage._nd_image|$SP/scipy/ndimage|_nd_image"
    "scipy.ndimage._ni_label|$SP/scipy/ndimage|_ni_label"
)
resolve_so() { # $1=dir $2=prefix -> first matching .so, or empty
    ls "$1/$2".*.so "$1/$2.so" 2>/dev/null | head -1
}
SOS=()
for row in "${SOROWS[@]}"; do
    lbl="${row%%|*}"; rest="${row#*|}"; dir="${rest%%|*}"; pfx="${rest#*|}"
    SOS+=("$lbl|$(resolve_so "$dir" "$pfx")")
done

ALLGAP="$(mktemp)"
printf '%-34s  %8s  %8s\n' "extension (.so)" "needs" "GAP"
printf '%-34s  %8s  %8s\n' "----------------------------------" "--------" "--------"
for entry in "${SOS[@]}"; do
    label="${entry%%|*}"; so="${entry#*|}"
    if [[ ! -f "$so" ]]; then
        printf '%-34s  %8s  %8s\n' "$label" "MISSING" "-"
        continue
    fi
    NEEDED="$(mktemp)"; GAP="$(mktemp)"
    undef_pysyms "$so" > "$NEEDED"
    comm -23 "$NEEDED" "$EXPORTED" > "$GAP"
    printf '%-34s  %8s  %8s\n' "$label" "$(wc -l < "$NEEDED" | tr -d ' ')" "$(wc -l < "$GAP" | tr -d ' ')"
    if [[ -s "$GAP" ]]; then sed 's/^/      /' "$GAP"; fi
    cat "$GAP" >> "$ALLGAP"
    rm -f "$NEEDED" "$GAP"
done

echo ""
echo "== AGGREGATE unique Tier-A symbol-gap frontier across the whole witness compute surface:"
sort -u "$ALLGAP" | sed 's/^/   /'
echo "   ($(sort -u "$ALLGAP" | wc -l | tr -d ' ') unique symbols)"
rm -f "$EXPORTED" "$ALLGAP"
