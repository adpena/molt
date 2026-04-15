# Deployment Log

## 2026-04-14: Initial Production Deployment

### R2 Bucket
- Status: already exists (created 2026-04-12)
- Bucket: `falcon-ocr-weights`
- Note: wrangler.toml binds this as `WEIGHTS`; worker accesses objects via `env.WEIGHTS.get(...)`

### Weight Upload (COMPLETED 2026-04-14)
- `model.safetensors` (1,029.8 MiB): **UPLOADED** via `wrangler r2 object put --pipe`
- `config.json`: **UPLOADED** to `models/falcon-ocr/config.json`
- `model_args.json`: **UPLOADED** to `models/falcon-ocr/model_args.json`
- `tokenizer.json`: **UPLOADED** to `models/falcon-ocr/tokenizer.json` (also at `v1/tokenizer.json`)
- `tokenizer_config.json`: **UPLOADED** to `models/falcon-ocr/tokenizer_config.json`
- `special_tokens_map.json`: **UPLOADED** to `models/falcon-ocr/special_tokens_map.json`
- All 6 files uploaded successfully. Total: ~1.03 GB.

### WASM Inference Module
- Status: **NOT YET BUILT** — requires `molt build wasm_driver.py --target wasm` which is part of molt's Python-to-WASM compilation pipeline
- The Worker gracefully degrades: returns 503 with `fallback_url: "/api/ocr/paddle"` when WASM module is unavailable
- Once built, upload to R2 at `models/falcon-ocr/falcon-ocr.wasm`

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
