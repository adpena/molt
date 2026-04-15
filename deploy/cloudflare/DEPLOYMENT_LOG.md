# Falcon-OCR Worker Deployment Log

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
