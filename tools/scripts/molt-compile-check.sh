#!/usr/bin/env bash
# PostToolUse hook: auto-compile Python files with molt after edits.
# Reports binary size and compile time back to Claude's context.
set -euo pipefail

FILE_PATH=$(jq -r '.tool_input.file_path // empty' 2>/dev/null || true)

# Only trigger for Python files in the cloudflare demo
[[ -z "$FILE_PATH" ]] && exit 0
[[ "$FILE_PATH" != *.py ]] && exit 0
case "$FILE_PATH" in
    */examples/cloudflare-demo/src/*) ;;
    *) exit 0 ;;
esac

OUTDIR=$(mktemp -d -t molt-check-XXXXXX)
trap 'rm -rf "$OUTDIR"' EXIT

START_MS=$(($(date +%s) * 1000 + $(date +%N 2>/dev/null | sed 's/^0*//' | head -c3 || echo 0)))

if MOLT_WASM_PROFILE=pure .venv/bin/python -m molt build "$FILE_PATH" \
    --target wasm --stdlib-profile micro \
    --output "$OUTDIR/output.wasm" \
    --linked-output "$OUTDIR/linked.wasm" 2>/dev/null; then

    END_MS=$(($(date +%s) * 1000 + $(date +%N 2>/dev/null | sed 's/^0*//' | head -c3 || echo 0)))
    ELAPSED=$((END_MS - START_MS))
    SIZE=$(stat -f%z "$OUTDIR/linked.wasm" 2>/dev/null || stat -c%s "$OUTDIR/linked.wasm" 2>/dev/null || echo 0)
    SIZE_KB=$((SIZE / 1024))

    echo "molt: compiled OK (${SIZE_KB}KB wasm, ${ELAPSED}ms)" >&2
else
    echo "molt: COMPILATION FAILED" >&2
fi
