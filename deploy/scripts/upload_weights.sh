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

BUCKET="falcon-ocr-weights"
SNAP_DIR="$HOME/.cache/molt/falcon-ocr/models--tiiuae--Falcon-OCR/snapshots/3a4d95a8b0008f7430df30a82cf35e6c3b6bcb66"
WEIGHTS_DIR="${1:-$SNAP_DIR}"

echo "Checking weights at $WEIGHTS_DIR ..."

# NOTE: model.safetensors (1.03 GB) exceeds wrangler's 300 MiB upload limit.
# Use deploy/scripts/upload_weights_s3.sh for the large file (via S3 API).
# This script uploads only the smaller auxiliary files via wrangler.

echo "Uploading config.json..."
REAL_CONFIG="$(readlink -f "$WEIGHTS_DIR/config.json" 2>/dev/null || echo "$WEIGHTS_DIR/config.json")"
if [ -f "$REAL_CONFIG" ]; then
    wrangler r2 object put "$BUCKET/models/falcon-ocr/config.json" \
        --file "$REAL_CONFIG" \
        --content-type "application/json" \
        --remote
else
    echo "  config.json not found, skipping"
fi

echo "Uploading tokenizer.json..."
REAL_TOKENIZER="$(readlink -f "$WEIGHTS_DIR/tokenizer.json" 2>/dev/null || echo "$WEIGHTS_DIR/tokenizer.json")"
if [ -f "$REAL_TOKENIZER" ]; then
    wrangler r2 object put "$BUCKET/models/falcon-ocr/tokenizer.json" \
        --file "$REAL_TOKENIZER" \
        --content-type "application/json" \
        --remote
else
    echo "  tokenizer.json not found, skipping"
fi

echo ""
echo "Small files uploaded."
echo ""
echo "model.safetensors (1.03 GB) must be uploaded via S3 API:"
echo "  ./deploy/scripts/upload_weights_s3.sh"
echo ""
echo "See that script for R2 API token setup instructions."
