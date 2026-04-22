#!/usr/bin/env bash
# Load test Falcon-OCR Worker endpoints.
#
# Usage:
#   ./test_load.sh [concurrency] [total_requests]
#   ./test_load.sh 10 50
#   ./test_load.sh            # defaults: 10 concurrent, 50 total

set -euo pipefail

URL="${FALCON_OCR_URL:-https://falcon-ocr.adpena.workers.dev}"
CONCURRENCY="${1:-10}"
REQUESTS="${2:-50}"
ORIGIN="https://freeinvoicemaker.app"

echo "=== Load Test: ${CONCURRENCY} concurrent, ${REQUESTS} total ==="
echo "    Target: ${URL}"
echo ""

echo "--- Health endpoint (${REQUESTS} requests, ${CONCURRENCY} concurrent) ---"
seq 1 "${REQUESTS}" | xargs -P "${CONCURRENCY}" -I {} sh -c "
    curl -s -o /dev/null -w '%{http_code} %{time_total}\n' \
        -H 'Origin: ${ORIGIN}' \
        '${URL}/health'
" 2>/dev/null | awk '
{
    codes[$1]++
    sum += $2
    count++
    if ($2 > max) max = $2
    if (min == 0 || $2 < min) min = $2
}
END {
    printf "  Requests: %d\n", count
    for (c in codes) printf "  HTTP %s: %d\n", c, codes[c]
    if (count > 0) {
        printf "  Avg latency: %.3fs\n", sum/count
        printf "  Min latency: %.3fs\n", min
        printf "  Max latency: %.3fs\n", max
    }
}'
echo ""

NL_REQUESTS=10
NL_CONCURRENCY=5
echo "--- NL Fill endpoint (${NL_REQUESTS} requests, ${NL_CONCURRENCY} concurrent) ---"
seq 1 "${NL_REQUESTS}" | xargs -P "${NL_CONCURRENCY}" -I {} sh -c "
    curl -s -o /dev/null -w '%{http_code} %{time_total}\n' \
        -X POST '${URL}/invoice/fill' \
        -H 'Origin: ${ORIGIN}' \
        -H 'Content-Type: application/json' \
        -d '{\"utterance\":\"Bill Acme Corp 5000 dollars for consulting\"}'
" 2>/dev/null | awk '
{
    codes[$1]++
    sum += $2
    count++
    if ($2 > max) max = $2
    if (min == 0 || $2 < min) min = $2
}
END {
    printf "  Requests: %d\n", count
    printf "  "
    for (c in codes) printf "HTTP %s: %d  ", c, codes[c]
    printf "\n"
    if (count > 0) {
        printf "  Avg latency: %.3fs\n", sum/count
        printf "  Min latency: %.3fs\n", min
        printf "  Max latency: %.3fs\n", max
    }
}'
echo ""

echo "--- GPU proxy probe (1 request) ---"
GPU_RESULT=$(curl -s -o /dev/null -w '%{http_code}' \
    -X POST "${URL}/ocr" \
    -H "Origin: ${ORIGIN}" \
    -H "Content-Type: application/json" \
    -H "X-Use-Backend: gpu" \
    -d '{"image":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="}' \
    2>/dev/null)

case "${GPU_RESULT}" in
    200) echo "  GPU inference: ACTIVE (200)" ;;
    501) echo "  GPU inference: NOT CONFIGURED (501)" ;;
    502) echo "  GPU inference: FAILED (502)" ;;
    *)   echo "  GPU inference: HTTP ${GPU_RESULT}" ;;
esac
echo ""

echo "=== Done ==="
