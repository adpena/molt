#!/usr/bin/env bash
# One-command witness cycle: build the Kernel A probe (or a given entry) and
# run it through the split-runtime node chain, emitting a compact verdict.
# Usage: tools/witness_cycle.sh [entry.py] [build|run|cycle]
# Env: WITNESS_TIMEOUT (node seconds, default 300), WITNESS_REBUILD=1.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Windows Python cannot resolve MSYS POSIX-style paths (/c/Users/...), and
# MSYS converts command arguments but NOT custom env vars, so env-exported
# module roots must be Windows-style. cygpath -m emits C:/... which both
# bash and Windows Python accept.
if command -v cygpath >/dev/null 2>&1; then
  ROOT="$(cygpath -m "$ROOT")"
fi
ENTRY="${1:-tmp/pact_witness_acceptance_queue/debug_import_trace/alias_probe.py}"
MODE="${2:-cycle}"
OUT="$ROOT/tmp/pact_witness_acceptance_queue/debug_import_trace/build"
LOG="$ROOT/tmp/pact_witness_acceptance_queue/debug_import_trace/cycle.log"
export MOLT_MEMORY_GUARD_POLL_SEC="${MOLT_MEMORY_GUARD_POLL_SEC:-2.0}"
export MOLT_STDLIB_PROFILE=full
export MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy"
export MOLT_MODULE_ROOTS="$ROOT/tmp/pact_numpy_multiarray_sealed_for_witness;$ROOT/tmp/pact_scipy_ndimage_sealed_for_witness_next;$ROOT/tmp/pact_scipy_ni_label_molt_ext_wasm_cpython_abi;$ROOT/bench/friends/repos/numpy_off_the_shelf;$ROOT/bench/friends/repos/scipy_off_the_shelf"
# Toolchain root (zig/binaryen/wasi-sysroot) and the warm target come from the
# one DX root authority (RunContext -> APDataStore), never a hardcoded legacy
# drive. If MOLT_TARGET_ROOT is not already exported by the queue/RunContext,
# resolve it through the authority so a standalone run lands on the same volume.
if [ -z "${MOLT_TARGET_ROOT:-}" ]; then
  eval "$(PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}" python3 "$ROOT/tools/run_context_env.py" \
    --root "$ROOT" --prefer-external-artifacts --dx --format posix \
    | grep -E '^export MOLT_TARGET_ROOT=')"
fi
TARGET_ROOT_POSIX="$MOLT_TARGET_ROOT"
if command -v cygpath >/dev/null 2>&1; then
  TARGET_ROOT_POSIX="$(cygpath -u "$MOLT_TARGET_ROOT")"
fi
export PATH="$TARGET_ROOT_POSIX/toolchains/zig-x86_64-windows-0.16.0:$PATH"
# Warm, incremental runtime builds. Without an explicit CARGO_TARGET_DIR the
# runtime wasm build uses a session-scoped (fresh, COLD) target dir and
# CARGO_INCREMENTAL defaults to 0 — so every cpython-abi edit triggers a full
# ~2160s recompile of the molt-runtime god-crate to wasm. A STABLE target dir
# + incremental means a cpython-abi-only edit recompiles just that leaf crate
# and relinks molt_runtime.wasm (a few hundred seconds after the first warm
# build). This is the iteration loop for the numpy-exec C-API closure. The
# stable warm target lives under the authority-resolved toolchain root.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$MOLT_TARGET_ROOT/witness-warm-target}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"

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
