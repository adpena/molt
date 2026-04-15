# Deployment Log

## 2026-04-14: Initial Production Deployment

### R2 Bucket
- Status: already exists (created 2026-04-12)
- Bucket: `falcon-ocr-weights`
- Note: wrangler.toml binds this as `WEIGHTS`; worker accesses objects via `env.WEIGHTS.get(...)`

### Weight Upload
- `config.json`: uploaded to `models/falcon-ocr/config.json` (remote R2)
- `tokenizer.json`: uploaded to `v1/tokenizer.json` (remote R2) -- needs re-upload to `models/falcon-ocr/tokenizer.json` if worker needs it
- `model.safetensors` (1,029 MiB): **FAILED** -- wrangler enforces 300 MiB upload limit for remote R2
  - `--pipe` with `--remote` also rejects files > 300 MiB
  - Cloudflare REST API returns 413 Payload Too Large for streaming PUT
  - `--pipe` without `--remote` uploads to local emulator only (0 bytes after download verification)
  - **Root cause**: Uploading files > 300 MiB to R2 requires S3-compatible API with multipart upload, which needs R2 API tokens (Access Key ID + Secret Access Key) created from the Cloudflare dashboard at `https://dash.cloudflare.com/<account>/r2/api-tokens`
- `falcon-ocr.wasm` (compiled WASM inference binary): **NOT AVAILABLE** -- this is the molt-compiled WASM binary, not a raw weight file. Needs to be built via the molt pipeline first.

### KV Namespace
- Status: already exists
- Namespace: `CACHE` (ID: `791309f66ab445e8a0327a34206f7005`)
- wrangler.toml updated from placeholder `falcon-ocr-cache` to real ID

### Worker Deploy
- Status: deployed
- URL: https://falcon-ocr.adpena.workers.dev
- Version ID: `ac91b397-1cb4-40be-8e21-969657acc8ae`
- Bundle size: 23.05 KiB / gzip: 5.85 KiB
- Startup time: 5 ms
- Bindings confirmed:
  - `env.CACHE` -> KV namespace `791309f66ab445e8a0327a34206f7005`
  - `env.WEIGHTS` -> R2 bucket `falcon-ocr-weights`
  - `env.CORS_ORIGIN` -> `https://freeinvoicemaker.app`
  - `env.MAX_IMAGE_BYTES` -> `10485760`
  - `env.MODEL_VERSION` -> `0.1.0`
- Note: `[limits] cpu_ms = 30000` commented out -- requires Workers Paid plan (account is on Free plan)

### Health Check
- Status: HTTP 200
- TTFB: 68 ms
- Response (partial): `{"status":"loading","model":"falcon-ocr","version":"0.1.0","device":"wasm","backends":{"molt-gpu":{"status":"loading"},"paddle-ocr":{"status":"available"}}}`
- Model status is "loading" because WASM binary and weights are not in R2 at expected paths

### Smoke Test (POST /ocr)
- Status: HTTP 503
- TTFB: 108 ms
- Response: `{"error":"Primary OCR backend unavailable","error_category":"MODEL_LOAD_FAILED","fallback_available":true,"fallback_url":"/api/ocr/paddle","backends":{"molt-gpu":{"status":"error","error":"WASM binary not found in R2: models/falcon-ocr/falcon-ocr.wasm"}}}`
- Fallback to PaddleOCR is advertised as available

### Load Test
- Not executed -- primary backend unavailable

### Issues Found

1. **R2 upload size limit (BLOCKER)**: Wrangler CLI caps remote uploads at 300 MiB. The `model.safetensors` file is 1,029 MiB. Need to either:
   - Create R2 S3-compatible API tokens from the dashboard and use `aws s3 cp` with multipart upload
   - Or use `wrangler r2 sippy` to mirror from an external S3 source
   
2. **Missing WASM binary (BLOCKER)**: The worker expects a compiled WASM inference module at `models/falcon-ocr/falcon-ocr.wasm` in R2. This binary does not exist yet -- it is the molt-compiled WASM version of the Falcon-OCR inference pipeline. This needs to be built via `python3 -m molt build --target wasm` or equivalent.

3. **R2 key path mismatch**: The `upload_weights.sh` script uses `v1/` prefix, but the worker code reads from `models/falcon-ocr/` prefix. These need to be aligned.

4. **Workers Free plan**: The account is on the Free plan, which limits CPU time to 10ms per request. Falcon-OCR inference will almost certainly exceed this. Need to upgrade to Workers Paid ($5/month) for 30s CPU time.

5. **upload_weights.sh references wrong bucket**: Script references `molt-ocr-weights` but the worker/toml uses `falcon-ocr-weights`.

### wrangler.toml Changes Made
- `kv_namespaces[0].id`: `falcon-ocr-cache` -> `791309f66ab445e8a0327a34206f7005` (real KV namespace ID)
- `[limits]` section: commented out (requires paid plan)

### Next Actions
1. **P0**: Create R2 S3-compatible API token from Cloudflare dashboard, then upload `model.safetensors` via `aws s3api put-object` with multipart
2. **P0**: Build the WASM inference binary via molt and upload to `models/falcon-ocr/falcon-ocr.wasm`
3. **P1**: Align `upload_weights.sh` bucket name and key prefix with worker expectations
4. **P1**: Upgrade to Workers Paid plan for 30s CPU time
5. **P2**: Set `X402_WALLET_ADDRESS` secret when x402 payment is ready
6. **P2**: Re-upload `tokenizer.json` to `models/falcon-ocr/tokenizer.json` (worker doesn't currently use it, but should for completeness)
