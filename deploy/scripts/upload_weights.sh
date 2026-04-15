#!/usr/bin/env bash
# Upload Falcon-OCR weights to Cloudflare R2.
#
# Usage: ./upload_weights.sh [weights_dir]
#
# Prerequisites:
#   - wrangler CLI installed and authenticated
#   - R2 bucket "molt-ocr-weights" created
#   - Model weights downloaded locally
set -euo pipefail

BUCKET="molt-ocr-weights"
WEIGHTS_DIR="${1:-$HOME/.cache/molt/falcon-ocr}"

echo "Checking weights at $WEIGHTS_DIR ..."

if [ ! -f "$WEIGHTS_DIR/model.safetensors" ]; then
    echo "Weights not found at $WEIGHTS_DIR/model.safetensors"
    echo "Download first: python3 tests/e2e/falcon_ocr_real_weights.py --download"
    exit 1
fi

echo "Uploading model weights..."
wrangler r2 object put "$BUCKET/v1/model.safetensors" \
    --file "$WEIGHTS_DIR/model.safetensors" \
    --content-type "application/octet-stream"

echo "Uploading tokenizer..."
if [ -f "$WEIGHTS_DIR/tokenizer.json" ]; then
    wrangler r2 object put "$BUCKET/v1/tokenizer.json" \
        --file "$WEIGHTS_DIR/tokenizer.json" \
        --content-type "application/json"
else
    echo "  tokenizer.json not found, skipping"
fi

echo "Uploading config..."
if [ -f "$WEIGHTS_DIR/config.json" ]; then
    wrangler r2 object put "$BUCKET/v1/config.json" \
        --file "$WEIGHTS_DIR/config.json" \
        --content-type "application/json"
else
    echo "  config.json not found, skipping"
fi

echo ""
echo "Done. Verify with:"
echo "  wrangler r2 object list $BUCKET"
