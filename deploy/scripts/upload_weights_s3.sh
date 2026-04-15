#!/usr/bin/env bash
# Upload large weights to R2 via S3-compatible API (multipart, handles >300MB).
#
# Wrangler CLI hard-limits uploads to 300 MiB. For model.safetensors (1.03 GB),
# we must use the S3-compatible multipart upload API that R2 exposes.
#
# Prerequisites:
#   1. Create an R2 API Token at:
#      https://dash.cloudflare.com -> R2 -> Manage R2 API Tokens -> Create API Token
#      - Permissions: Object Read & Write
#      - Scope: Apply to bucket "falcon-ocr-weights"
#
#   2. Configure the AWS CLI with R2 credentials:
#      aws configure --profile r2
#        Access Key ID: <your R2 token access key>
#        Secret Access Key: <your R2 token secret key>
#        Default region: auto
#        Default output format: json
#
#   3. Set your Cloudflare Account ID:
#      export R2_ACCOUNT_ID="7d9a81409923a4884287d303dbdc4586"
#      (or edit the default below)
#
# Usage: ./upload_weights_s3.sh

set -euo pipefail

R2_ACCOUNT_ID="${R2_ACCOUNT_ID:-7d9a81409923a4884287d303dbdc4586}"
R2_ENDPOINT="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
BUCKET="falcon-ocr-weights"
AWS_PROFILE="${R2_AWS_PROFILE:-r2}"

SNAP_DIR="$HOME/.cache/molt/falcon-ocr/models--tiiuae--Falcon-OCR/snapshots/3a4d95a8b0008f7430df30a82cf35e6c3b6bcb66"

if ! command -v aws &>/dev/null; then
    echo "ERROR: AWS CLI not found. Install with: brew install awscli"
    exit 1
fi

if ! aws configure list --profile "$AWS_PROFILE" &>/dev/null; then
    echo "ERROR: AWS profile '$AWS_PROFILE' not configured."
    echo ""
    echo "Create an R2 API Token at:"
    echo "  https://dash.cloudflare.com -> R2 -> Manage R2 API Tokens -> Create API Token"
    echo ""
    echo "Then configure:"
    echo "  aws configure --profile $AWS_PROFILE"
    echo "  (Access Key = R2 token access key, Secret = R2 token secret key, Region = auto)"
    exit 1
fi

REAL_SAFETENSORS="$(readlink -f "$SNAP_DIR/model.safetensors" 2>/dev/null || echo "$SNAP_DIR/model.safetensors")"
if [ ! -f "$REAL_SAFETENSORS" ]; then
    echo "ERROR: model.safetensors not found at $SNAP_DIR"
    echo "Download first: python3 tests/e2e/falcon_ocr_real_weights.py --download"
    exit 1
fi

FILE_SIZE=$(stat -f%z "$REAL_SAFETENSORS" 2>/dev/null || stat -c%s "$REAL_SAFETENSORS" 2>/dev/null)
echo "=== Uploading model.safetensors ($(echo "$FILE_SIZE" | numfmt --to=iec 2>/dev/null || echo "${FILE_SIZE} bytes")) ==="
echo "    Source: $REAL_SAFETENSORS"
echo "    Destination: s3://$BUCKET/models/falcon-ocr/model.safetensors"
echo "    Endpoint: $R2_ENDPOINT"
echo ""

aws s3 cp "$REAL_SAFETENSORS" \
    "s3://$BUCKET/models/falcon-ocr/model.safetensors" \
    --endpoint-url "$R2_ENDPOINT" \
    --profile "$AWS_PROFILE" \
    --content-type "application/octet-stream" \
    --expected-size "$FILE_SIZE"

echo ""
echo "=== Upload config.json ==="
REAL_CONFIG="$(readlink -f "$SNAP_DIR/config.json" 2>/dev/null || echo "$SNAP_DIR/config.json")"
if [ -f "$REAL_CONFIG" ]; then
    aws s3 cp "$REAL_CONFIG" \
        "s3://$BUCKET/models/falcon-ocr/config.json" \
        --endpoint-url "$R2_ENDPOINT" \
        --profile "$AWS_PROFILE" \
        --content-type "application/json"
    echo "  Done."
else
    echo "  config.json not found, skipping"
fi

echo ""
echo "=== Upload tokenizer.json ==="
REAL_TOKENIZER="$(readlink -f "$SNAP_DIR/tokenizer.json" 2>/dev/null || echo "$SNAP_DIR/tokenizer.json")"
if [ -f "$REAL_TOKENIZER" ]; then
    aws s3 cp "$REAL_TOKENIZER" \
        "s3://$BUCKET/models/falcon-ocr/tokenizer.json" \
        --endpoint-url "$R2_ENDPOINT" \
        --profile "$AWS_PROFILE" \
        --content-type "application/json"
    echo "  Done."
else
    echo "  tokenizer.json not found, skipping"
fi

echo ""
echo "=== Verifying uploads ==="
aws s3 ls "s3://$BUCKET/models/falcon-ocr/" \
    --endpoint-url "$R2_ENDPOINT" \
    --profile "$AWS_PROFILE"

echo ""
echo "All uploads complete."
