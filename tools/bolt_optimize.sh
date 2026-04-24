#!/bin/bash
# BOLT post-link optimization for molt output binaries.
# Reorders functions and basic blocks for optimal icache utilization.
#
# Usage:
#   tools/bolt_optimize.sh <binary> [training_command]
#
# Example:
#   tools/bolt_optimize.sh /tmp/sieve_bench /tmp/sieve_bench
#   tools/bolt_optimize.sh ./my_program "./my_program --input data.txt"

set -euo pipefail

if [ $# -lt 1 ]; then
    echo "Usage: $0 <binary> [training_command]" >&2
    exit 1
fi

BINARY="$1"
TRAINING="${2:-$BINARY}"
BOLT_BINARY="${BINARY}.bolt"
FDATA_PATH="/tmp/molt-bolt-prof.fdata"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found: ${BINARY}" >&2
    exit 1
fi

# Resolve BOLT binary — llvm-bolt (standard) or bolt (some distros).
BOLT=""
if command -v llvm-bolt >/dev/null 2>&1; then
    BOLT="$(command -v llvm-bolt)"
elif command -v bolt >/dev/null 2>&1; then
    BOLT="$(command -v bolt)"
else
    echo "ERROR: llvm-bolt not found. Install via: brew install llvm (macOS) or apt install llvm-bolt (Linux)" >&2
    exit 1
fi

# Clean stale profile data from previous runs.
rm -f "${FDATA_PATH}" "${FDATA_PATH}."*

# Step 1: Instrument the binary for profile collection.
echo "==> Instrumenting ${BINARY}..." >&2
"$BOLT" "$BINARY" -o "${BINARY}.instr" -instrument \
    -instrumentation-file="${FDATA_PATH}" \
    -instrumentation-file-append-pid 2>/dev/null || {
    echo "BOLT instrumentation failed — binary may need --emit-relocs" >&2
    echo "Rebuild with: RUSTFLAGS='-C link-arg=-Wl,--emit-relocs' cargo build ..." >&2
    exit 1
}

# Step 2: Run the instrumented binary with the training workload.
echo "==> Profiling with: ${TRAINING}..." >&2
eval "${BINARY}.instr" 2>/dev/null || true
rm -f "${BINARY}.instr"

# Merge any PID-suffixed profile fragments into the canonical path.
# BOLT appends .<pid> when -instrumentation-file-append-pid is used.
FDATA_FOUND=""
for f in "${FDATA_PATH}" "${FDATA_PATH}."*; do
    if [ -f "$f" ] && [ -s "$f" ]; then
        FDATA_FOUND="$f"
        break
    fi
done

if [ -z "$FDATA_FOUND" ]; then
    echo "ERROR: No profile data generated — training workload may have failed." >&2
    rm -f "${BINARY}.instr"
    exit 1
fi

# If the profile data ended up in a PID-suffixed file, move it to the canonical path.
if [ "$FDATA_FOUND" != "$FDATA_PATH" ]; then
    mv "$FDATA_FOUND" "$FDATA_PATH"
fi

# Step 3: Optimize with BOLT using the collected profile.
echo "==> Optimizing with BOLT..." >&2
"$BOLT" "$BINARY" -o "$BOLT_BINARY" \
    -data="${FDATA_PATH}" \
    -reorder-blocks=ext-tsp \
    -reorder-functions=hfsort \
    -split-functions \
    -split-all-cold \
    -dyno-stats 2>&1 | tail -5

# Step 4: Report results.
if [ -f "$BOLT_BINARY" ]; then
    # stat -f%z is macOS, stat --format=%s is Linux.
    SIZE_BEFORE=$(stat -f%z "$BINARY" 2>/dev/null || stat --format=%s "$BINARY")
    SIZE_AFTER=$(stat -f%z "$BOLT_BINARY" 2>/dev/null || stat --format=%s "$BOLT_BINARY")
    echo "==> Binary size: ${SIZE_BEFORE} -> ${SIZE_AFTER} bytes" >&2
    echo "==> BOLT-optimized binary: ${BOLT_BINARY}" >&2
    echo "==> To replace original: mv ${BOLT_BINARY} ${BINARY}" >&2
else
    echo "BOLT optimization failed" >&2
    exit 1
fi

# Clean up profile data.
rm -f "${FDATA_PATH}" "${FDATA_PATH}."*
