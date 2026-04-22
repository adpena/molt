#!/usr/bin/env bash
# Pre-deployment validation checklist.
#
# Run this before every deployment. All checks must pass.
# Usage: ./pre_deploy_check.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

ERRORS=0
WARNINGS=0

echo "=== Pre-Deploy Checklist ==="
echo ""

# 1. Rust tests (molt-gpu)
echo -n "1. Rust tests (molt-gpu): "
if cargo test -p molt-gpu --all-features 2>&1 | tail -5 | grep -q "passed"; then
    echo "PASS"
else
    echo "FAIL"
    ERRORS=$((ERRORS + 1))
fi

# 2. Clippy
echo -n "2. Clippy (molt-gpu): "
if cargo clippy -p molt-gpu --all-features -- -D warnings 2>&1 | tail -1 | grep -q "Finished"; then
    echo "PASS"
else
    echo "FAIL"
    ERRORS=$((ERRORS + 1))
fi

# 3. WASM compilation
echo -n "3. WASM target check: "
if cargo check -p molt-gpu --target wasm32-unknown-unknown --no-default-features --features cpu-backend,wasm-backend 2>&1 | tail -1 | grep -q "Finished"; then
    echo "PASS"
else
    echo "FAIL"
    ERRORS=$((ERRORS + 1))
fi

# 4. Worker JS syntax (all files)
echo -n "4. Worker JS syntax: "
JS_ERRORS=0
for js_file in deploy/cloudflare/worker.js deploy/cloudflare/ocr_api.js deploy/cloudflare/x402.js deploy/cloudflare/monitoring.js; do
    if [ -f "$js_file" ]; then
        if ! node --check "$js_file" 2>/dev/null; then
            echo ""
            echo "   FAIL: $js_file"
            JS_ERRORS=$((JS_ERRORS + 1))
        fi
    fi
done
if [ $JS_ERRORS -eq 0 ]; then
    echo "PASS"
else
    ERRORS=$((ERRORS + JS_ERRORS))
fi

# 5. No stale markers in deploy/
echo -n "5. No stale markers in deploy/: "
STALE_HITS=$(grep -rn "$(printf 'FI%sME\|HA%sK\|X%sX' X C X)" deploy/ --include="*.js" --include="*.ts" --include="*.toml" --include="*.sh" 2>/dev/null | grep -v node_modules | grep -v 'printf' || true)
if [ -z "$STALE_HITS" ]; then
    echo "PASS"
else
    echo "FAIL"
    echo "$STALE_HITS" | while IFS= read -r line; do echo "   $line"; done
    ERRORS=$((ERRORS + 1))
fi

# 6. No stale generated Worker bundle
echo -n "6. No stale Worker bundle: "
if [ -f deploy/cloudflare/worker-bundle.js ]; then
    echo "FAIL"
    ERRORS=$((ERRORS + 1))
else
    echo "PASS"
fi

# 7. Deploy scripts are executable
echo -n "7. Deploy scripts executable: "
SCRIPT_ERRORS=0
for script in deploy/scripts/deploy.sh deploy/scripts/upload_weights.sh deploy/scripts/pre_deploy_check.sh deploy/scripts/load_test.sh; do
    if [ -f "$script" ] && [ ! -x "$script" ]; then
        echo ""
        echo "   NOT EXECUTABLE: $script"
        SCRIPT_ERRORS=$((SCRIPT_ERRORS + 1))
    fi
done
if [ $SCRIPT_ERRORS -eq 0 ]; then
    echo "PASS"
else
    ERRORS=$((ERRORS + SCRIPT_ERRORS))
fi

# 8. wrangler.toml exists and is valid
echo -n "8. wrangler.toml present: "
if [ -f deploy/cloudflare/wrangler.toml ]; then
    echo "PASS"
else
    echo "FAIL"
    ERRORS=$((ERRORS + 1))
fi

# 9. MCP tool definition exists
echo -n "9. MCP tool definition: "
if [ -f deploy/mcp/ocr_tool.json ]; then
    if node -e "JSON.parse(require('fs').readFileSync('deploy/mcp/ocr_tool.json','utf8'))" 2>/dev/null; then
        echo "PASS"
    else
        echo "FAIL (invalid JSON)"
        ERRORS=$((ERRORS + 1))
    fi
else
    echo "FAIL (missing)"
    ERRORS=$((ERRORS + 1))
fi

# 10. Git state
echo -n "10. Git state: "
STAGED=$(git diff --cached --name-only 2>/dev/null || true)
CONFLICT=$(grep -rn "^<<<<<<< \|^=======$\|^>>>>>>> " deploy/ docs/deployment/ 2>/dev/null || true)
if [ -n "$CONFLICT" ]; then
    echo "FAIL (merge conflict markers found)"
    echo "$CONFLICT" | while IFS= read -r line; do echo "   $line"; done
    ERRORS=$((ERRORS + 1))
elif [ -n "$STAGED" ]; then
    echo "WARN (uncommitted staged changes)"
    WARNINGS=$((WARNINGS + 1))
else
    echo "PASS"
fi

echo ""
echo "================================================================"
if [ $ERRORS -eq 0 ]; then
    if [ $WARNINGS -gt 0 ]; then
        echo "  ALL CHECKS PASSED ($WARNINGS warning(s)) -- ready to deploy"
    else
        echo "  ALL CHECKS PASSED -- ready to deploy"
    fi
else
    echo "  $ERRORS CHECK(S) FAILED -- fix before deploying"
    exit 1
fi
echo "================================================================"
