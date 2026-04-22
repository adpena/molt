# Falcon-OCR on Cloudflare Workers

Production-grade OCR inference at the edge. Three deployment modes: browser-local (offline, private), API (cloud, x402 payment), and MCP (AI agent integration).

## Quick Start

### Browser (offline, private inference)

```html
<script type="module">
import { FalconOCR } from 'https://falcon-ocr.adpena.workers.dev/sdk/falcon-ocr.js';

const ocr = new FalconOCR();
await ocr.init(); // Downloads 130 MB model (cached for offline use)

const canvas = document.getElementById('invoice');
const text = await ocr.recognize(canvas);
console.log(text);
</script>
```

Key properties:
- Model is cached in the browser after first download (Service Worker + Cache API)
- All inference runs locally via WebGPU/WASM -- no data leaves the device
- Works fully offline after initial model fetch
- Supports `<canvas>`, `<img>`, `ImageData`, `Blob`, and `File` inputs

### API (x402 payment, cloud inference)

**Important:** API clients must set `User-Agent: FalconOCR-Client/1.0` to bypass Cloudflare Bot Protection (error 1010). Requests with the `X-Payment-402` header are also exempt from bot checks.

```bash
curl -X POST https://falcon-ocr.adpena.workers.dev/ocr \
  -H "Content-Type: application/json" \
  -H "User-Agent: FalconOCR-Client/1.0" \
  -H "Origin: https://your-app.example.com" \
  -H "X-Payment-402: <payment-proof>" \
  -d '{"image": "<base64-png-or-jpeg>"}'
```

Response:
```json
{
  "text": "INVOICE\nVendor: Acme Corp...",
  "confidence": 0.94,
  "request_id": "req_abc123",
  "backend": "workers-ai",
  "latency_ms": 320
}
```

#### Structured extraction

```bash
curl -X POST https://falcon-ocr.adpena.workers.dev/ocr/structured \
  -H "Content-Type: application/json" \
  -H "Origin: https://your-app.example.com" \
  -d '{"image": "<base64>", "schema": "invoice"}'
```

Returns structured JSON with vendor, line items, totals, dates.

### MCP (for AI agents)

The MCP tool definition is at `deploy/mcp/ocr_tool.json`. Point any MCP-compatible agent at the endpoint:

```json
{
  "name": "falcon_ocr",
  "server": {
    "url": "https://falcon-ocr.adpena.workers.dev",
    "transport": "http"
  }
}
```

Supported tools: `ocr_extract_text`, `ocr_structured_extract`, `ocr_batch`.

## Architecture

```
Browser (WebGPU/WASM)     API Request
        |                      |
        v                      v
   Local model          Cloudflare Worker
   (130 MB cached)            |
                              v
                    +-------------------+
                    | Fallback chain:   |
                    | 1. Workers AI GPU |
                    | 2. Molt-GPU WASM  |
                    | 3. PaddleOCR CPU  |
                    +-------------------+
                              |
                              v
                     KV Cache (dedup)
                              |
                              v
                        Response
```

## Model Comparison

| Engine | Params | Accuracy | Latency (browser) | Languages | Use Case |
|--------|--------|----------|--------------------|-----------|----------|
| PaddleOCR | ~12M | 99.6% | ~200ms | 10 | Invoices, receipts, forms |
| Falcon-OCR INT8 | 300M | High | ~2-4s | Multi | Complex documents, tables |
| Falcon-OCR INT4 | 300M | Good | ~1-2s | Multi | Memory-constrained edge |
| Whisper tiny | 39M | -- | Scaffold | -- | Speech-to-text (coming) |

## Browser Compute Priority Chain

The browser runtime probes backends in order and uses the first available:

```
1. WebNN        -- Hardware ML accelerator (Chrome 127+, Edge)
2. WebGPU       -- GPU compute shaders (Chrome, Firefox Nightly)
3. WebGL2       -- Fragment shader fallback (universal GPU)
4. WASM SIMD    -- CPU vectorized (all modern browsers)
5. WASM scalar  -- CPU baseline (universal fallback)
```

Server-side fallback chain:
```
6. Workers AI   -- Cloudflare GPU fleet
7. External GPU -- HuggingFace / Replicate / RunPod / Modal / Fly.io
```

## Deployment Architecture

```
                    +------------------+
                    |  HuggingFace     |
                    |  Space (static)  |
                    +--------+---------+
                             |
                             v
+-------------+    +-------------------+    +----------------+
| Browser     |--->| Cloudflare Worker |--->| R2 Storage     |
| (WebGPU/    |    | (edge inference)  |    | (weights/WASM) |
|  WASM)      |    +--------+----------+    +----------------+
+-------------+             |
                    +-------+--------+
                    | Fallback chain  |
                    | 1. Workers AI   |
                    | 2. External GPU |
                    | 3. PaddleOCR    |
                    +-----------------+
```

## Performance Baselines

| Metric | Value | Notes |
|--------|-------|-------|
| WASM binary | 10.5 MB raw | 4-6 MB achievable with tree-shaking |
| SIMD matmul 64x64 (Rust) | ~14,500 ns | 2.5x faster than Zig |
| SIMD softmax 1024 (Rust) | ~2,250 ns | Online 2-pass algorithm |
| Health endpoint | 79ms avg | 10 concurrent |
| Invoice fill (Workers AI) | 2.74s avg | 5 concurrent |
| ONNX op parity | 29/29 ops | max_diff < 4e-6 |
| Differential tests | 72/72 pass | native + WASM + LLVM |
| Codebase | ~2.3M LOC | Rust 660K + Python 1.5M + JS 63K |

## Health Check

```bash
curl https://falcon-ocr.adpena.workers.dev/health
```

Returns: model status, backend availability, cache state, analytics summary (requests/min, error rate, latency percentiles).

## Development

### Run accuracy benchmark

```bash
cd tests/e2e
python -m pytest test_invoice_accuracy.py -v
```

Generates `docs/benchmarks/invoice_accuracy.md` with per-field accuracy results.

### Deploy

```bash
cd deploy/cloudflare
npx wrangler deploy
```

### Monitor

Analytics are written to Cloudflare Analytics Engine on every request. Query via the health endpoint or directly via the Analytics Engine SQL API.

## Model Variants (priority order)

1. **Workers AI** -- GPU fleet inference, zero CPU cost, preferred for production
2. **External GPU** -- HuggingFace/Replicate/RunPod/Modal/Fly.io, bfloat16 quality (via `X-Use-Backend: gpu`)
3. **Molt-GPU** -- Local WASM + WebGPU, used for browser-side and high-memory Worker plans
4. **PaddleOCR** -- CPU fallback, available when GPU paths are exhausted

## External GPU Inference (bfloat16 quality)

For production-quality OCR with the official Falcon-OCR model at full bfloat16 precision,
use the GPU inference proxy. The Worker forwards requests to an external GPU service.

### Setup

Set three secrets via `wrangler secret put`:

```bash
wrangler secret put GPU_INFERENCE_URL    # Full endpoint URL
wrangler secret put GPU_INFERENCE_KEY    # Bearer token / API key
wrangler secret put GPU_INFERENCE_PROVIDER  # huggingface | replicate | runpod | modal | flyio
```

### Provider comparison

| Provider | GPU Options | Pricing | Cold Start | Notes |
|----------|-----------|---------|------------|-------|
| HuggingFace IE | A10G, A100 | $1.30-6.50/hr | ~60s | Easiest setup for HF models |
| Replicate | A40, A100 | Per prediction (~$0.005) | ~30s | Push Docker image as Cog model |
| RunPod | A100, H100, 4090 | $0.00076-0.00146/s | ~5s (serverless) | Lowest latency for sustained load |
| Modal | A100, H100 | Per-second billing | ~30s | Best for bursty workloads |
| Fly.io | A100, H100 | $2.50/hr | Persistent | Best for always-on deployment |

### Usage

```bash
curl -X POST https://falcon-ocr.adpena.workers.dev/ocr \
  -H "Content-Type: application/json" \
  -H "X-Use-Backend: gpu" \
  -H "X-Payment-402: <payment-proof>" \
  -d '{"image": "<base64>"}'
```

Docker image for self-hosted backends: `ghcr.io/tiiuae/falcon-ocr:latest`
