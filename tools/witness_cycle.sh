#!/usr/bin/env bash
# One-command witness cycle: build the Kernel A probe (or a given entry) and
# run it through the split-runtime node chain, emitting a compact verdict.
# Usage: tools/witness_cycle.sh [entry.py] [build|run|cycle]
# Env: WITNESS_TIMEOUT (node seconds, default 300), WITNESS_REBUILD=1.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENTRY="${1:-tmp/pact_witness_acceptance_queue/debug_import_trace/alias_probe.py}"
MODE="${2:-cycle}"
OUT="$ROOT/tmp/pact_witness_acceptance_queue/debug_import_trace/build"
LOG="$ROOT/tmp/pact_witness_acceptance_queue/debug_import_trace/cycle.log"
export MOLT_MEMORY_GUARD_POLL_SEC="${MOLT_MEMORY_GUARD_POLL_SEC:-2.0}"
export MOLT_STDLIB_PROFILE=full
export MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy"
export MOLT_MODULE_ROOTS="$ROOT/tmp/pact_numpy_multiarray_sealed_for_witness;$ROOT/tmp/pact_scipy_ndimage_sealed_for_witness_next;$ROOT/tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi;$ROOT/bench/friends/repos/numpy_off_the_shelf;$ROOT/bench/friends/repos/scipy_off_the_shelf"
export PATH="/e/molt-target/toolchains/zig-x86_64-windows-0.16.0:$PATH"

verdict() { printf 'WITNESS %s rc=%s %s\n' "$1" "$2" "$3"; }

if [ "$MODE" = "build" ] || [ "$MODE" = "cycle" ]; then
  REBUILD=""
  [ "${WITNESS_REBUILD:-0}" = "1" ] && REBUILD="--rebuild"
  start=$(date +%s)
  "$ROOT/.venv/Scripts/python.exe" -m molt build "$ENTRY" --target wasm \
    --profile browser --wasm-profile auto --split-runtime $REBUILD \
    --out-dir "$OUT" >"$LOG" 2>&1
  rc=$?
  secs=$(( $(date +%s) - start ))
  if [ $rc -ne 0 ]; then
    err=$(grep -m1 -E "panicked|error\[|link failed|failed to validate|MOLT_COMPAT_ERROR|custody|missing required|integrity" "$LOG")
    verdict build $rc "${secs}s ${err:-see $LOG}"
    exit $rc
  fi
  verdict build 0 "${secs}s"
fi

if [ "$MODE" = "run" ] || [ "$MODE" = "cycle" ]; then
  export MOLT_WASM_DIRECT_LINK=1 MOLT_WASM_PREFER_LINKED=0
  export MOLT_RUNTIME_WASM="$OUT/molt_runtime.wasm"
  start=$(date +%s)
  out=$(timeout "${WITNESS_TIMEOUT:-300}" node "$ROOT/wasm/run_wasm.js" "$OUT/app.wasm" 2>&1)
  rc=$?
  secs=$(( $(date +%s) - start ))
  echo "$out" >"$LOG.run"
  tail=$(echo "$out" | grep -vE "^\s+at |wasm-function|ExperimentalWarning|trace-warnings" | tail -6)
  verdict run $rc "${secs}s"
  printf '%s\n' "$tail"
fi
