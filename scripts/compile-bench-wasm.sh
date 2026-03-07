#!/usr/bin/env bash
# compile-bench-wasm.sh — Pre-compile Molt benchmark Python files to WASM
# for use with wasm/bench_pyodide.html.
#
# Usage:
#   ./scripts/compile-bench-wasm.sh [bench_name]
#   ./scripts/compile-bench-wasm.sh fib          # compile only bench_fib.py
#   ./scripts/compile-bench-wasm.sh              # compile all benchmarks
#
# Output:
#   wasm/bench/<name>_linked.wasm   — browser-ready WASM
#   wasm/bench/<name>_output.wasm   — object file (kept for inspection)
#
# Dependencies:
#   - Rust wasm32-wasip1 target (rustup target add wasm32-wasip1)
#   - wasm-ld (llvm, via `brew install llvm`)
#   - Python 3.12+ with molt-lang installed (uv run)
#   - CARGO_TARGET_DIR pointing to cargo target (defaults to external volume)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BENCH_DIR="$REPO_ROOT/tests/benchmarks"
OUT_DIR="$REPO_ROOT/wasm/bench"
RUNTIME_WASM="$REPO_ROOT/wasm/molt_runtime.wasm"
WASM_LINK="$REPO_ROOT/tools/wasm_link.py"
CARGO_TARGET="${CARGO_TARGET_DIR:-/Volumes/APDataStore/Molt/cargo-target}"

mkdir -p "$OUT_DIR"

BACKEND_BIN="$CARGO_TARGET/release/molt-backend"
if [[ ! -f "$BACKEND_BIN" ]]; then
  echo "molt-backend not found at $BACKEND_BIN — building..."
  cd "$REPO_ROOT"
  CARGO_TARGET_DIR="$CARGO_TARGET" cargo build --release -p molt-lang-backend
fi

FRONTEND_CMD=(uv run --python 3.12 python3 -m molt_lang_python)

_compile_one() {
  local name="$1"
  local py_file="$BENCH_DIR/bench_${name}.py"

  if [[ ! -f "$py_file" ]]; then
    echo "ERROR: $py_file not found" >&2
    return 1
  fi

  local ir_file="$OUT_DIR/${name}.ir.json"
  local obj_file="$OUT_DIR/${name}_output.wasm"
  local linked_file="$OUT_DIR/${name}_linked.wasm"

  echo "=== Compiling $name ==="

  echo "  [1/3] Python -> IR..."
  cd "$REPO_ROOT"
  uv run --python 3.12 python3 -c "
import sys, json
sys.path.insert(0, '.')
from runtime.molt_python.src import molt_lang_python as fe
with open('$py_file') as f:
    src = f.read()
ir = fe.compile_to_ir(src, filename='$name')
with open('$ir_file', 'w') as out:
    json.dump(ir, out)
" 2>/dev/null || {
    echo "  [1/3] Python -> IR (via tools pipeline)..."
    uv run --python 3.12 python3 "$REPO_ROOT/tools/compile_governor.py" \
      --input "$py_file" \
      --ir-output "$ir_file" \
      --target wasm32-wasip1
  }

  echo "  [2/3] IR -> WASM object..."
  "$BACKEND_BIN" \
    --target wasm32-wasip1 \
    --output "$obj_file" \
    < "$ir_file"

  echo "  [3/3] Linking WASM..."
  uv run --python 3.12 python3 "$WASM_LINK" \
    --input "$obj_file" \
    --runtime "$RUNTIME_WASM" \
    --output "$linked_file"

  echo "  Done: $linked_file ($(du -sh "$linked_file" | cut -f1))"
}

BENCH_NAMES=(fib sum_list list_ops dict_ops str_find matrix_math)

if [[ $# -ge 1 ]]; then
  _compile_one "$1"
else
  for name in "${BENCH_NAMES[@]}"; do
    _compile_one "$name" || echo "SKIP: $name (compile failed)"
  done
  echo ""
  echo "All done. WASM files in: $OUT_DIR/"
  echo "Serve locally with:  python3 -m http.server 8080 -d $REPO_ROOT/wasm"
  echo "Then open:           http://localhost:8080/bench_pyodide.html"
fi
