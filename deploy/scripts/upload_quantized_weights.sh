#!/usr/bin/env bash
# Upload quantized Falcon-OCR weights to Cloudflare R2.
#
# Uploads INT4 quantized model (model.safetensors ~129 MB, scales.json, config.json)
# to the R2 bucket under models/falcon-ocr-int4/.
#
# For models under 300 MB, wrangler CLI upload works directly.
# For larger files, use the S3 API variant.
#
# Prerequisites:
#   - wrangler CLI installed and authenticated
#   - R2 bucket "falcon-ocr-weights" created
#   - Quantized model generated: python3 deploy/scripts/quantize_model.py --bits 4
#
# Usage: ./upload_quantized_weights.sh [--bits 4|8] [--s3]
set -euo pipefail

BITS="${1:---bits}"
if [ "$BITS" = "--bits" ]; then
    BITS="${2:-4}"
fi

BUCKET="falcon-ocr-weights"
QUANT_DIR="$HOME/.cache/molt/falcon-ocr/quantized-int${BITS}"
R2_PREFIX="models/falcon-ocr-int${BITS}"
USE_S3=false

# Parse flags
for arg in "$@"; do
    case "$arg" in
        --s3) USE_S3=true ;;
    esac
done

echo "=== Uploading INT${BITS} quantized Falcon-OCR model ==="
echo "    Source: $QUANT_DIR"
echo "    Destination: $BUCKET/$R2_PREFIX/"
echo ""

if [ ! -f "$QUANT_DIR/model.safetensors" ]; then
    echo "ERROR: Quantized model not found at $QUANT_DIR/model.safetensors"
    echo "Run: python3 deploy/scripts/quantize_model.py --bits $BITS"
    exit 1
fi

MODEL_SIZE=$(stat -f%z "$QUANT_DIR/model.safetensors" 2>/dev/null || stat -c%s "$QUANT_DIR/model.safetensors" 2>/dev/null)
echo "Model size: $(echo "$MODEL_SIZE" | numfmt --to=iec 2>/dev/null || echo "${MODEL_SIZE} bytes")"
echo ""

if [ "$USE_S3" = true ]; then
    # Use S3 API for large files
    R2_ACCOUNT_ID="${R2_ACCOUNT_ID:-7d9a81409923a4884287d303dbdc4586}"
    R2_ENDPOINT="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
    AWS_PROFILE="${R2_AWS_PROFILE:-r2}"

    echo "Using S3 API (endpoint: $R2_ENDPOINT)"

    aws s3 cp "$QUANT_DIR/model.safetensors" \
        "s3://$BUCKET/$R2_PREFIX/model.safetensors" \
        --endpoint-url "$R2_ENDPOINT" \
        --profile "$AWS_PROFILE" \
        --content-type "application/octet-stream" \
        --expected-size "$MODEL_SIZE"

    aws s3 cp "$QUANT_DIR/scales.json" \
        "s3://$BUCKET/$R2_PREFIX/scales.json" \
        --endpoint-url "$R2_ENDPOINT" \
        --profile "$AWS_PROFILE" \
        --content-type "application/json"

    aws s3 cp "$QUANT_DIR/config.json" \
        "s3://$BUCKET/$R2_PREFIX/config.json" \
        --endpoint-url "$R2_ENDPOINT" \
        --profile "$AWS_PROFILE" \
        --content-type "application/json"

    echo ""
    echo "=== Verifying uploads ==="
    aws s3 ls "s3://$BUCKET/$R2_PREFIX/" \
        --endpoint-url "$R2_ENDPOINT" \
        --profile "$AWS_PROFILE"
else
    # Use wrangler for files under 300 MB
    if [ "$MODEL_SIZE" -gt 314572800 ]; then
        echo "ERROR: Model ($MODEL_SIZE bytes) exceeds wrangler's 300 MiB limit."
        echo "Use: $0 --bits $BITS --s3"
        exit 1
    fi

    echo "Uploading model.safetensors..."
    wrangler r2 object put "$BUCKET/$R2_PREFIX/model.safetensors" \
        --file "$QUANT_DIR/model.safetensors" \
        --content-type "application/octet-stream" \
        --remote

    echo "Uploading scales.json..."
    wrangler r2 object put "$BUCKET/$R2_PREFIX/scales.json" \
        --file "$QUANT_DIR/scales.json" \
        --content-type "application/json" \
        --remote

    echo "Uploading config.json..."
    wrangler r2 object put "$BUCKET/$R2_PREFIX/config.json" \
        --file "$QUANT_DIR/config.json" \
        --content-type "application/json" \
        --remote
fi

echo ""
echo "=== Upload complete ==="
echo "INT${BITS} model uploaded to: $BUCKET/$R2_PREFIX/"
echo ""
echo "Redeploy the worker to load the new model:"
echo "  wrangler deploy --config deploy/cloudflare/wrangler.toml"
