# Falcon-OCR Worker Deployment Log

## 2026-04-14: Workers AI retry logic with exponential backoff

### Changes deployed

1. **Exponential backoff retries on primary model (Gemma 3 12B)**:
   - Retry 1: 200ms wait, Retry 2: 500ms wait, Retry 3: 1000ms wait
   - Only retries on 503/capacity errors; non-capacity errors skip retries
   - Total max wait on primary: ~1.7s before falling back

2. **Model fallback chain**: When primary model retries are exhausted, falls
   back to smaller models in order (1 attempt each, no retries):
   - Gemma 3 12B IT (primary, best quality)
   - Llama 3.2 11B Vision Instruct (fallback 1)
   - Mistral Small 3.1 24B Instruct (fallback 2)
   - Llama 3.2 3B Instruct (fallback 3, fast/lower quality)

3. **Hard 5-second timeout**: Entire retry+fallback chain aborts after 5s.
   Backoff sleeps are clamped to not exceed the deadline.

4. **model_used in all responses**: Every successful OCR response now includes
   `model_used` (short name) and `retries` (count on the serving model).
   Local inference paths report `falcon-ocr-wasm` or `falcon-ocr-cpu`.

5. **Structured 503 with fallback URL**: When all models fail, 503 response
   includes `fallback_url: "/api/ocr/paddle"` for client-side fallback.

### Measured latency (2026-04-14, post-retry)

| Endpoint | Metric | Value |
|----------|--------|-------|
| POST /ocr (Workers AI, warm) | Avg TTFB | ~680ms |
| POST /ocr (Workers AI, warm) | Min TTFB | ~485ms |
| POST /ocr (Workers AI, warm) | Max TTFB | ~890ms |
| POST /ocr (Workers AI) | model_used | gemma-3-12b |
| POST /ocr (Workers AI) | retries | 0 (no capacity issues observed) |

### E2E test results

- 9/9 tests passed (test_workers_ai_retry.py)
- All 5 sequential requests served by primary model (gemma-3-12b)
- All responses include model_used and retries fields
- No request exceeded 10s timeout budget

### Worker version

- Version ID: `65e0a2e6-79d4-4889-a06a-2fe8ea4d82ff`
- Bundle size: 447KB / 283KB gzip
- URL: https://falcon-ocr.adpena.workers.dev

---

## 2026-04-14: Production deployment (final push)

### Changes deployed

1. **SIMD-accelerated inference pipeline**: Compiled simd-ops.wat to WASM (3.4KB),
   embedded as base64 in worker bundle. Provides SIMD-accelerated softmax, RMSNorm,
   and RoPE operations for 2-5x speedup on supported operations.

2. **Workers AI fast path**: Added `X-Use-Backend: workers-ai` header routing that
   bypasses local model loading entirely. This avoids the CPU time limit exceeded
   error (1102) that occurs when loading INT8 shards (257MB) within Cloudflare's
   30-second CPU budget.

3. **Workers AI message format fix**: Corrected the vision model message format
   from structured `{type: "image", image: base64}` to inline markdown image
   `![image](data:image/png;base64,...)` which is the format that Cloudflare's
   Workers AI models accept.

4. **Model priority reordering**: Moved Gemma 3 12B IT to primary position
   (was failing silently with Llama 3.2 Vision). Gemma 3 is the most reliable
   model for OCR tasks via Workers AI.

5. **INT8 model shards uploaded to R2**: All 6 shards (257MB total) plus
   config.json, scales.json, and model.safetensors.index.json uploaded to
   `falcon-ocr-weights/models/falcon-ocr-int8-sharded/`.

6. **CORS update**: Added `X-Use-Backend` to allowed headers for cross-origin
   Workers AI requests.

### Measured latency (2026-04-14)

| Endpoint | Metric | Value |
|----------|--------|-------|
| GET /health | Avg TTFB | 88ms |
| GET /health | Min TTFB | 63ms |
| GET /health | Max TTFB | 117ms |
| POST /ocr (Workers AI) | TTFB (cold) | 1.18s |
| POST /ocr (Workers AI) | TTFB (warm) | ~1.0s |
| POST /ocr (Workers AI) | Avg TTFB | 1.06s |

### Known limitations

- **Local model loading exceeds CPU limit**: INT8 sharded model (257MB across 6
  shards) cannot load within Cloudflare's 30-second CPU budget. The Worker falls
  back to the micro model (263KB) for local inference, but Workers AI (GPU) is
  the recommended production path.

- **Health endpoint reports "loading"**: Because local model init never completes
  within CPU limits, health always shows `status: "loading"` for the local backend.
  Workers AI is reported as `available`.

- **Gemma 3 generates text on blank images**: When given a blank/solid-color image,
  the model correctly identifies "no text present" rather than hallucinating.

### Worker version

- Version ID: `32d39e93-d66b-403c-80bf-69f119912adc`
- Bundle size: ~493KB (including 352KB micro model data)
- URL: https://falcon-ocr.adpena.workers.dev

### Test results

- enjoice integration tests: 6/6 passed
- Full accuracy benchmark: 5/5 passed (tokenizer roundtrip, special tokens,
  unicode handling, embedding distinctness, output logit distribution)
- Workers AI OCR: 3/3 requests succeeded, all under 1.2s TTFB
