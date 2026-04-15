# Falcon-OCR Deployment Runbook

## Architecture

Falcon-OCR runs as a Cloudflare Worker using WASM inference. Model weights are stored in R2 and loaded on cold start. Payment is handled via the x402 protocol.

```
Browser -> Cloudflare Worker (worker.js) -> WASM inference (molt-gpu)
                                         -> R2 (weights, config)
                                         -> x402 verification endpoint
```

## SLA Targets

| Metric | Target |
|--------|--------|
| Uptime | 99.9% |
| p95 TTFB | < 2s |
| p99 TTFB | < 5s |
| Cold start | < 10s |
| Error rate | < 0.1% |

## Deployment

### Prerequisites

1. `wrangler` CLI installed and authenticated (`wrangler login`)
2. R2 bucket `falcon-ocr-weights` created
3. Worker secrets configured:
   - `wrangler secret put X402_WALLET_ADDRESS`
   - `wrangler secret put X402_VERIFICATION_URL`

### Pre-deploy validation

```bash
cd deploy/scripts
./pre_deploy_check.sh
```

All checks must pass before deploying.

### Deploy to staging

```bash
./deploy/scripts/deploy.sh staging
```

This will:
1. Build the WASM binary from the molt compiler
2. Upload artifacts to R2
3. Validate Worker JS syntax
4. Deploy to `falcon-ocr-staging.freeinvoicemaker.workers.dev`
5. Run health check
6. Run smoke test

### Deploy to production

```bash
./deploy/scripts/deploy.sh production
```

Same steps as staging, deploys to `falcon-ocr.freeinvoicemaker.workers.dev`.

### Upload model weights separately

If only weights have changed (no code changes):

```bash
./deploy/scripts/upload_weights.sh ~/.cache/molt/falcon-ocr
```

### Using the bundled Worker

If wrangler module resolution fails, update `wrangler.toml` to use the bundle:

```toml
main = "worker-bundle.js"
```

## Rollback

### Immediate rollback (last known good)

```bash
wrangler rollback --config deploy/cloudflare/wrangler.toml
```

This reverts to the previous Worker version instantly.

### Rollback to a specific version

```bash
# List recent deployments
wrangler deployments list --config deploy/cloudflare/wrangler.toml

# Rollback to a specific deployment ID
wrangler rollback <deployment-id> --config deploy/cloudflare/wrangler.toml
```

### Rollback weights

Weights are versioned in R2 under `/v1/`. To roll back to a previous weight version, re-upload the old weights:

```bash
wrangler r2 object put falcon-ocr-weights/models/falcon-ocr/weights.safetensors \
    --file /path/to/old/weights.safetensors \
    --content-type "application/octet-stream"
```

Then restart all Worker instances to clear the in-memory model cache:

```bash
wrangler deployments list --config deploy/cloudflare/wrangler.toml
# Redeploy the current code version (forces cold start)
wrangler deploy --config deploy/cloudflare/wrangler.toml
```

## Health Checks

### Manual health check

```bash
curl -s https://falcon-ocr.freeinvoicemaker.workers.dev/health | python3 -m json.tool
```

Expected response:
```json
{
    "status": "ready",
    "model": "falcon-ocr",
    "version": "0.1.0",
    "device": "wasm",
    "request_id": "...",
    "backends": {
        "molt-gpu": { "status": "ready" },
        "paddle-ocr": { "status": "available", "url": "/api/ocr/paddle" }
    }
}
```

Status values:
- `ready` -- model loaded, accepting requests
- `loading` -- cold start in progress
- `error` -- model failed to load (check `backends.molt-gpu.error`)

### Load test

```bash
./deploy/scripts/load_test.sh https://falcon-ocr.freeinvoicemaker.workers.dev 10 100
```

## Logs

### Real-time logs (tail)

```bash
wrangler tail falcon-ocr --config deploy/cloudflare/wrangler.toml
```

### Cloudflare Dashboard

1. Go to https://dash.cloudflare.com
2. Navigate to Workers & Pages > falcon-ocr
3. Click "Logs" tab
4. Filter by status code, request ID, or time range

### Log format

All logs are structured JSON with these fields:
- `request_id` -- unique request trace ID
- `timestamp` -- ISO 8601
- `method`, `path` -- HTTP method and path
- `status_code` -- HTTP response status
- `latency_ms` -- request duration
- `device_type` -- mobile/tablet/desktop
- `browser` -- chrome/safari/firefox/edge/other
- `error_category` -- one of: `MODEL_LOAD_FAILED`, `INFERENCE_TIMEOUT`, `WEBGPU_UNAVAILABLE`, `PAYMENT_INVALID`, `INPUT_INVALID`, `INTERNAL_ERROR`
- `error_message` -- truncated error (no PII, no stack traces)

No image content or user identifiers are ever logged.

## Updating Model Weights

1. Download new weights:
   ```bash
   python3 tests/e2e/falcon_ocr_real_weights.py --download
   ```

2. Upload to R2:
   ```bash
   ./deploy/scripts/upload_weights.sh ~/.cache/molt/falcon-ocr
   ```

3. Update `MODEL_VERSION` in `wrangler.toml` if the version changed.

4. Redeploy to force cold start with new weights:
   ```bash
   ./deploy/scripts/deploy.sh production
   ```

5. Verify:
   ```bash
   curl -s https://falcon-ocr.freeinvoicemaker.workers.dev/health | python3 -m json.tool
   ```

## Updating Worker Code

1. Edit source files in `deploy/cloudflare/`:
   - `worker.js` -- main entry point
   - `ocr_api.js` -- OCR API handlers
   - `x402.js` -- payment middleware
   - `monitoring.js` -- logging and analytics

2. Validate syntax:
   ```bash
   node --check deploy/cloudflare/worker.js
   node --check deploy/cloudflare/ocr_api.js
   node --check deploy/cloudflare/x402.js
   node --check deploy/cloudflare/monitoring.js
   ```

3. Regenerate bundle if using bundled mode:
   Re-run the bundle generation process and validate with `node --check deploy/cloudflare/worker-bundle.js`.

4. Deploy:
   ```bash
   ./deploy/scripts/deploy.sh staging   # test first
   ./deploy/scripts/deploy.sh production
   ```

## Emergency Procedures

### High error rate (> 1%)

1. Check logs for error category distribution:
   ```bash
   wrangler tail falcon-ocr --format json | jq '.error_category' | sort | uniq -c
   ```

2. If `MODEL_LOAD_FAILED`: R2 bucket may be unavailable or weights corrupted. Re-upload weights.
3. If `INFERENCE_TIMEOUT`: model is too slow for current load. Check if image sizes are unexpectedly large.
4. If `PAYMENT_INVALID`: x402 verification endpoint may be down. Check `X402_VERIFICATION_URL` secret.
5. If widespread: rollback immediately.

### High latency (p95 > 5s)

1. Check if this is cold start related (first request after idle):
   - Cold start latency includes weight download from R2 (~2-5s depending on weight size).
   - Subsequent requests should be < 2s.

2. If sustained high latency on warm requests:
   - Check WASM module size (should be < 10 MB).
   - Check if image preprocessing is slow (large images).
   - Consider reducing `MAX_IMAGE_BYTES`.

3. If R2 fetch is slow:
   - Verify Smart Placement is enabled in `wrangler.toml`.
   - Check R2 bucket region vs Worker region.

### Complete outage

1. Rollback:
   ```bash
   wrangler rollback --config deploy/cloudflare/wrangler.toml
   ```

2. If rollback fails, check Cloudflare status: https://www.cloudflarestatus.com/

3. If Cloudflare is up but Worker is down:
   ```bash
   wrangler deployments list --config deploy/cloudflare/wrangler.toml
   wrangler rollback <last-known-good-id> --config deploy/cloudflare/wrangler.toml
   ```

4. If R2 is down, the Worker will return 503 with fallback information pointing to PaddleOCR.

### x402 payment issues

1. Verify wallet address is correct:
   ```bash
   wrangler secret list --config deploy/cloudflare/wrangler.toml
   ```

2. Test without payment (dev mode): temporarily remove `X402_WALLET_ADDRESS` secret to bypass payment checks.

3. Re-configure:
   ```bash
   wrangler secret put X402_WALLET_ADDRESS --config deploy/cloudflare/wrangler.toml
   wrangler secret put X402_VERIFICATION_URL --config deploy/cloudflare/wrangler.toml
   ```

## On-Call

| Role | Contact |
|------|---------|
| Primary | Alejandro Pena |
| Escalation | (configure as team grows) |

## Monitoring Dashboard

Cloudflare Analytics Engine receives structured metrics via the `ANALYTICS` binding. Query with:

```sql
SELECT
  blob2 AS path,
  COUNT() AS requests,
  AVG(double2) AS avg_latency_ms,
  quantileExact(0.95)(double2) AS p95_latency_ms,
  SUM(CASE WHEN double1 >= 500 THEN 1 ELSE 0 END) AS errors
FROM falcon_ocr
WHERE timestamp > NOW() - INTERVAL '1' HOUR
GROUP BY path
```
