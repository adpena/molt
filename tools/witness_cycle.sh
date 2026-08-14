#!/usr/bin/env bash
# One-command witness cycle: build the Kernel A probe (or a given entry) and
# run it through the split-runtime node chain, emitting a compact verdict.
# Usage: tools/witness_cycle.sh [entry.py] [build|run|cycle]
# Env: WITNESS_TIMEOUT (node seconds, default 300), WITNESS_REBUILD=1.
set -u
SHELL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
. "$SHELL_ROOT/tools/molt_shell_env.sh"
molt_init_shell_context "$SHELL_ROOT"
ROOT="$MOLT_HOST_ROOT"
ENTRY="${1:-tmp/pact_witness_acceptance_queue/debug_import_trace/alias_probe.py}"
MODE="${2:-cycle}"
OUT="$ROOT/tmp/pact_witness_acceptance_queue/debug_import_trace/build"
OUT_SHELL="$SHELL_ROOT/tmp/pact_witness_acceptance_queue/debug_import_trace/build"
LOG="$SHELL_ROOT/tmp/pact_witness_acceptance_queue/debug_import_trace/cycle.log"
PYTHON="$MOLT_PYTHON"
NODE="${NODE:-$(command -v node || command -v node.exe || true)}"
export MOLT_SESSION_ID="${MOLT_SESSION_ID:-witness-warm}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
if [ -z "${MOLT_EXT_ROOT:-}" ] || [ -z "${CARGO_TARGET_DIR:-}" ] || [ -z "${MOLT_TARGET_ROOT:-}" ]; then
  eval "$(molt_run_context_env "$SHELL_ROOT" \
    --session-prefix witness \
    --session-scoped-uv-project-env \
    --prefer-external-artifacts \
    --dx \
    --format posix)"
fi
export MOLT_MEMORY_GUARD_POLL_SEC="${MOLT_MEMORY_GUARD_POLL_SEC:-2.0}"
export MOLT_STDLIB_PROFILE=full
export MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy"
SCIENTIFIC_WITNESS_ROOTS="$(PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}" "$PYTHON" -c 'from molt.cli.source_package_seal import verify_source_package_seal; from molt.scientific_stack_versions import resolve_scientific_stack, scientific_witness_seal_root, scientific_witness_variant; stack = resolve_scientific_stack(); variant = scientific_witness_variant(stack=stack); print(";".join(str(verify_source_package_seal(scientific_witness_seal_root(package, variant=variant, stack=stack)).payload_root) for package in ("numpy", "scipy")))')"
if [ -z "$SCIENTIFIC_WITNESS_ROOTS" ]; then
  echo "canonical scientific witness seals are missing or invalid" >&2
  echo "produce each with: molt extension produce-set --package <numpy|scipy> --module-set pact-witness --source <verified-checkout> --build-root <fresh-build-root>" >&2
  exit 2
fi
export MOLT_MODULE_ROOTS="$SCIENTIFIC_WITNESS_ROOTS"

for _toolchain_root in "${MOLT_TARGET_ROOT:-}" "${MOLT_EXT_ROOT:-}"; do
  [ -n "$_toolchain_root" ] || continue
  _zig_dir="$(molt_shell_path "$_toolchain_root/toolchains/zig-x86_64-windows-0.16.0")"
  if [ -d "$_zig_dir" ]; then
    export PATH="$_zig_dir:$PATH"
    break
  fi
done

verdict() { printf 'WITNESS %s rc=%s %s\n' "$1" "$2" "$3"; }

if [ "$MODE" = "build" ] || [ "$MODE" = "cycle" ]; then
  REBUILD=""
  [ "${WITNESS_REBUILD:-0}" = "1" ] && REBUILD="--rebuild"
  start=$(date +%s)
  case "$ENTRY" in
    /*) ENTRY_FOR_PYTHON="$(molt_path_for_command "$ENTRY" "$PYTHON")" ;;
    [A-Za-z]:/* | [A-Za-z]:\\*) ENTRY_FOR_PYTHON="$ENTRY" ;;
    *) ENTRY_FOR_PYTHON="$ROOT/$ENTRY" ;;
  esac
  mkdir -p "$(dirname "$LOG")"
  PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}" "$PYTHON" -m molt build "$ENTRY_FOR_PYTHON" --target wasm \
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
  if [ -z "$NODE" ]; then
    echo "node or node.exe is required for witness run mode." >&2
    exit 127
  fi
  NODE_RUNNER="$(molt_path_for_command "$SHELL_ROOT/wasm/run_wasm.js" "$NODE")"
  NODE_MANIFEST="$(molt_path_for_command "$OUT_SHELL/manifest.json" "$NODE")"
  start=$(date +%s)
  out=$(timeout "${WITNESS_TIMEOUT:-300}" "$NODE" "$NODE_RUNNER" "$NODE_MANIFEST" 2>&1)
  rc=$?
  secs=$(( $(date +%s) - start ))
  echo "$out" >"$LOG.run"
  tail=$(echo "$out" | grep -vE "^\s+at |wasm-function|ExperimentalWarning|trace-warnings" | tail -6)
  verdict run $rc "${secs}s"
  printf '%s\n' "$tail"
fi
