#!/usr/bin/env bash
# Load test the Falcon-OCR endpoint.
#
# Usage: ./load_test.sh [url] [concurrency] [total_requests]
#
# Prerequisites:
#   - curl installed
#   - bc installed (for throughput calculation)
#   - Target endpoint reachable
set -euo pipefail

URL="${1:-https://ocr-staging.freeinvoicemaker.app}"
CONCURRENCY="${2:-10}"
REQUESTS="${3:-100}"

echo "Load testing $URL"
echo "Concurrency: $CONCURRENCY, Total requests: $REQUESTS"
echo ""

# Create a tiny test image (1x1 white pixel PNG, base64)
TEST_IMAGE="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg=="

# Health check
echo "=== Health Check ==="
HEALTH=$(curl -s -w "\n%{http_code}" "$URL/health" 2>/dev/null || echo -e "\nFAILED")
HEALTH_CODE=$(echo "$HEALTH" | tail -1)
HEALTH_BODY=$(echo "$HEALTH" | head -n -1)
echo "  HTTP $HEALTH_CODE"
if command -v python3 >/dev/null 2>&1 && [ "$HEALTH_CODE" = "200" ]; then
    echo "$HEALTH_BODY" | python3 -m json.tool 2>/dev/null || echo "$HEALTH_BODY"
else
    echo "  $HEALTH_BODY"
fi
echo ""

# Sequential warm-up
echo "=== Warm-up (3 requests) ==="
for i in 1 2 3; do
    TIME=$(curl -s -o /dev/null -w "%{time_total}" -X POST "$URL/ocr" \
        -H "Content-Type: application/json" \
        -d "{\"image\": \"$TEST_IMAGE\"}" 2>/dev/null || echo "error")
    echo "  Request $i: ${TIME}s"
done
echo ""

# Concurrent load test using xargs
echo "=== Load Test ($CONCURRENCY concurrent, $REQUESTS total) ==="
START=$(python3 -c "import time; print(int(time.time_ns()))" 2>/dev/null || date +%s%N)
RESULTS=$(seq 1 "$REQUESTS" | xargs -P "$CONCURRENCY" -I {} sh -c "
    curl -s -o /dev/null -w '%{http_code} %{time_total}\n' -X POST '$URL/ocr' \
        -H 'Content-Type: application/json' \
        -d '{\"image\": \"$TEST_IMAGE\"}' 2>/dev/null || echo 'ERR 0.0'
")
END=$(python3 -c "import time; print(int(time.time_ns()))" 2>/dev/null || date +%s%N)

echo ""
echo "=== Response Code Distribution ==="
echo "$RESULTS" | awk '{print $1}' | sort | uniq -c | sort -rn

echo ""
echo "=== Latency Percentiles ==="
echo "$RESULTS" | awk '{print $2}' | sort -n | awk '
BEGIN { n=0 }
{ vals[n++] = $1 }
END {
    if (n == 0) { print "  No results"; exit }
    sum = 0
    for (i = 0; i < n; i++) sum += vals[i]
    printf "  Count:  %d\n", n
    printf "  Mean:   %.3fs\n", sum/n
    printf "  p50:    %.3fs\n", vals[int(n*0.50)]
    printf "  p90:    %.3fs\n", vals[int(n*0.90)]
    printf "  p95:    %.3fs\n", vals[int(n*0.95)]
    printf "  p99:    %.3fs\n", vals[int(n*0.99)]
    printf "  Max:    %.3fs\n", vals[n-1]
}
'

ELAPSED_NS=$((END - START))
ELAPSED_MS=$((ELAPSED_NS / 1000000))
if command -v bc >/dev/null 2>&1 && [ "$ELAPSED_MS" -gt 0 ]; then
    RPS=$(echo "scale=1; $REQUESTS * 1000 / $ELAPSED_MS" | bc)
    echo ""
    echo "  Total time: ${ELAPSED_MS}ms"
    echo "  Throughput: ${RPS} req/s"
fi
