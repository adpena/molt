#!/usr/bin/env bash
# Upload PaddleOCR Korean and unified multilingual recognizer weights to R2.
#
# Usage: ./deploy/scripts/upload_paddleocr_weights.sh
#
# Prerequisites:
#   - wrangler CLI installed and authenticated
#   - R2 bucket "falcon-ocr-weights" created
#   - Models downloaded to models/paddleocr/
set -euo pipefail

BUCKET="falcon-ocr-weights"
MODEL_ROOT="models/paddleocr"

upload_file() {
    local src="$1" dst="$2" ctype="$3"
    if [ ! -f "$src" ]; then
        echo "  SKIP: $src not found"
        return 1
    fi
    local size
    size=$(stat -f%z "$src" 2>/dev/null || stat -c%s "$src" 2>/dev/null)
    echo "  Uploading $dst ($(echo "scale=1; $size/1048576" | bc) MB)..."
    wrangler r2 object put "$BUCKET/$dst" \
        --file "$src" \
        --content-type "$ctype" \
        --remote
}

echo "=== PaddleOCR Korean Recognizer ==="
upload_file "$MODEL_ROOT/korean_rec/rec/korean/model.onnx" \
    "models/paddleocr/korean_rec/model.onnx" \
    "application/octet-stream"
upload_file "$MODEL_ROOT/korean_rec/rec/korean/dict.txt" \
    "models/paddleocr/korean_rec/dict.txt" \
    "text/plain"

echo ""
echo "=== PaddleOCR Unified Mobile Recognizer (multilingual, covers Japanese) ==="
upload_file "$MODEL_ROOT/unified_mobile_rec/v2/rec/unified_mobile/model.onnx" \
    "models/paddleocr/unified_mobile_rec/model.onnx" \
    "application/octet-stream"
upload_file "$MODEL_ROOT/unified_mobile_rec/v2/rec/unified_mobile/dict.txt" \
    "models/paddleocr/unified_mobile_rec/dict.txt" \
    "text/plain"

echo ""
echo "Done. Verify with:"
echo "  wrangler r2 object list $BUCKET --prefix models/paddleocr/ --remote"
