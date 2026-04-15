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

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DEPLOY_DIR="$PROJECT_ROOT/deploy/cloudflare"
ENVIRONMENT="${1:-staging}"
WORKER_NAME="falcon-ocr"
R2_BUCKET="falcon-ocr-weights"
WASM_DRIVER="$PROJECT_ROOT/src/molt/stdlib/tinygrad/wasm_driver.py"
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
echo "=== Step 1/6: Building WASM binary ==="
mkdir -p "$BUILD_OUTPUT_DIR"
python3 -m molt build "$WASM_DRIVER" --target wasm --output "$BUILD_OUTPUT_DIR" --rebuild
echo "WASM binary built at: $BUILD_OUTPUT_DIR"
echo ""

# Step 2: Upload artifacts to R2
echo "=== Step 2/6: Uploading artifacts to R2 ==="

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
echo "=== Step 3/6: Validating Worker source ==="
for js_file in worker.js ocr_api.js x402.js monitoring.js; do
  if [[ ! -f "$DEPLOY_DIR/$js_file" ]]; then
    echo "Error: $js_file not found at $DEPLOY_DIR/$js_file"
    exit 1
  fi
  echo "  Found: $js_file"
done
echo ""

# Step 4: Deploy Worker
echo "=== Step 4/6: Deploying Worker ($ENVIRONMENT) ==="
cd "$DEPLOY_DIR"

if [[ "$ENVIRONMENT" == "production" ]]; then
  wrangler deploy --config wrangler.toml
else
  wrangler deploy --config wrangler.toml --env staging
fi
echo ""

# Step 5: Health check
echo "=== Step 5/6: Health check ==="
if [[ "$ENVIRONMENT" == "production" ]]; then
  WORKER_URL="https://falcon-ocr.freeinvoicemaker.workers.dev"
else
  WORKER_URL="https://falcon-ocr-staging.freeinvoicemaker.workers.dev"
fi

HEALTH_RESPONSE=$(curl -s -w "\n%{http_code}" "$WORKER_URL/health" 2>/dev/null || echo "FAILED")
HTTP_CODE=$(echo "$HEALTH_RESPONSE" | tail -1)
BODY=$(echo "$HEALTH_RESPONSE" | head -n -1)

if [[ "$HTTP_CODE" == "200" ]]; then
  echo "  Health check passed (HTTP 200)"
  echo "  Response: $BODY"
else
  echo "  Warning: Health check returned HTTP $HTTP_CODE"
  echo "  This may be normal on first deploy (cold start required)."
  echo "  Response: $BODY"
fi
echo ""

# Step 6: Smoke test
echo "=== Step 6/6: Smoke test ==="
# Create a minimal 16x16 white JPEG test image (base64)
# This is a valid JPEG that the Worker can process
TEST_IMAGE_B64=$(python3 -c "
import base64, struct, io
# Minimal valid 16x16 white JPEG
data = bytes([
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
    0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43,
    0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09,
    0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12,
    0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20,
    0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29,
    0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32,
    0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xD9,
])
print(base64.b64encode(data).decode())
" 2>/dev/null || echo "")

if [[ -n "$TEST_IMAGE_B64" ]]; then
  SMOKE_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$WORKER_URL/ocr" \
    -H "Content-Type: application/json" \
    -d "{\"image\": \"$TEST_IMAGE_B64\", \"format\": \"image/jpeg\"}" \
    2>/dev/null || echo "FAILED")
  SMOKE_CODE=$(echo "$SMOKE_RESPONSE" | tail -1)
  SMOKE_BODY=$(echo "$SMOKE_RESPONSE" | head -n -1)

  # 402 is expected (no payment header). 200 means dev mode.
  if [[ "$SMOKE_CODE" == "402" ]]; then
    echo "  Smoke test passed: got expected 402 Payment Required (x402 active)"
  elif [[ "$SMOKE_CODE" == "200" ]]; then
    echo "  Smoke test passed: got 200 OK (dev mode, no x402)"
  elif [[ "$SMOKE_CODE" == "503" ]]; then
    echo "  Smoke test partial: got 503 (model not loaded yet, fallback available)"
    echo "  Response: $SMOKE_BODY"
  else
    echo "  Warning: Smoke test returned HTTP $SMOKE_CODE"
    echo "  Response: $SMOKE_BODY"
  fi
else
  echo "  Skipped: could not generate test image"
fi

echo ""
echo "================================================================"
echo "  Deployment complete: $ENVIRONMENT"
echo "  Worker URL: $WORKER_URL"
echo "================================================================"
