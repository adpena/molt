#!/usr/bin/env bash
# Deploy Falcon-OCR to Cloudflare Workers.
#
# Usage: ./deploy.sh [staging|production]
#
# Prerequisites:
#   - wrangler CLI installed and authenticated
#   - molt CLI installed
#   - R2 bucket "falcon-ocr-weights" created
#   - Worker secrets configured (X402_WALLET_ADDRESS, X402_VERIFICATION_URL)
#
# CRITICAL BUILD INSTRUCTIONS (no tribal knowledge):
#   - WASM builds: MOLT_HERMETIC_MODULE_ROOTS=1 (skip venv scanning)
#   - Post-build: wasm-strip + wasm-opt -Oz --enable-reference-types (6x size reduction)
#   - R2 uploads: ALWAYS use --remote flag (without it, uploads go to local emulator)
#   - Workers: /ocr routes browsers to client-side PaddleOCR WASM; Workers AI
#     serves /invoice/fill and /template/extract only (see worker.js)
#   - Browser: use WASM for inference (offline, private, zero cost)
#   - Import aliases: avoid 'from X import Y as Z' in WASM-compiled code (linker bug)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DEPLOY_DIR="$PROJECT_ROOT/deploy/cloudflare"
ENVIRONMENT="${1:-staging}"
WORKER_NAME="falcon-ocr"
R2_BUCKET="falcon-ocr-weights"
BUILD_OUTPUT_DIR="$PROJECT_ROOT/build/falcon-ocr"

# Validate environment argument
if [[ "$ENVIRONMENT" != "staging" && "$ENVIRONMENT" != "production" ]]; then
  echo "Error: environment must be 'staging' or 'production', got '$ENVIRONMENT'"
  exit 1
fi

echo "================================================================"
echo "  Falcon-OCR Deployment: $ENVIRONMENT"
echo "================================================================"
echo ""

# Step 1: Build WASM binary
echo "=== Step 1/5: Building WASM binary ==="
mkdir -p "$BUILD_OUTPUT_DIR"
python3 "$PROJECT_ROOT/tools/guarded_exec.py" --prefix MOLT_DEPLOY --cwd "$PROJECT_ROOT" -- \
  python3 -m molt build --module demos.tinygrad.wasm_driver --target wasm --output "$BUILD_OUTPUT_DIR" --rebuild
echo "WASM binary built at: $BUILD_OUTPUT_DIR"
echo ""

# Step 2: Upload artifacts to R2
echo "=== Step 2/5: Uploading artifacts to R2 ==="

if [[ -f "$BUILD_OUTPUT_DIR/falcon-ocr.wasm" ]]; then
  wrangler r2 object put "$R2_BUCKET/models/falcon-ocr/falcon-ocr.wasm" \
    --file "$BUILD_OUTPUT_DIR/falcon-ocr.wasm" \
    --content-type "application/wasm"
  echo "  Uploaded: falcon-ocr.wasm"
else
  echo "Error: WASM binary not found at $BUILD_OUTPUT_DIR/falcon-ocr.wasm"
  exit 1
fi

if [[ -f "$BUILD_OUTPUT_DIR/weights.safetensors" ]]; then
  wrangler r2 object put "$R2_BUCKET/models/falcon-ocr/weights.safetensors" \
    --file "$BUILD_OUTPUT_DIR/weights.safetensors" \
    --content-type "application/octet-stream"
  echo "  Uploaded: weights.safetensors"
else
  echo "Warning: weights.safetensors not found at $BUILD_OUTPUT_DIR/weights.safetensors"
  echo "  Ensure weights are uploaded separately before deployment."
fi

if [[ -f "$BUILD_OUTPUT_DIR/config.json" ]]; then
  wrangler r2 object put "$R2_BUCKET/models/falcon-ocr/config.json" \
    --file "$BUILD_OUTPUT_DIR/config.json" \
    --content-type "application/json"
  echo "  Uploaded: config.json"
else
  echo "Warning: config.json not found at $BUILD_OUTPUT_DIR/config.json"
fi
echo ""

# Step 3: Validate Worker source
echo "=== Step 3/5: Validating Worker source ==="
for js_file in worker.js ocr_api.js x402.js monitoring.js; do
  if [[ ! -f "$DEPLOY_DIR/$js_file" ]]; then
    echo "Error: $js_file not found at $DEPLOY_DIR/$js_file"
    exit 1
  fi
  echo "  Found: $js_file"
done
echo ""

# Step 4: Deploy Worker
echo "=== Step 4/5: Deploying Worker ($ENVIRONMENT) ==="
cd "$DEPLOY_DIR"
DEPLOY_LOG="$(mktemp)"
trap 'rm -f "$DEPLOY_LOG"' EXIT

if [[ "$ENVIRONMENT" == "production" ]]; then
  wrangler deploy --config wrangler.toml 2>&1 | tee "$DEPLOY_LOG"
else
  wrangler deploy --config wrangler.toml --env staging 2>&1 | tee "$DEPLOY_LOG"
fi
echo ""

# Step 5: Verify the deployed Worker at the URL wrangler printed. The
# falcon-ocr case set checks health, CORS, the browser fallback, the x402
# payment demand and the public assets; a failing case fails the deploy.
echo "=== Step 5/5: Verifying the deployed Worker ==="
python3 "$PROJECT_ROOT/tools/cloudflare_demo_endpoint_verify.py" \
  --wrangler-log "$DEPLOY_LOG" --case-set falcon-ocr --attempts 5
echo ""
echo "================================================================"
echo "  Deployment complete: $ENVIRONMENT"
echo "================================================================"
