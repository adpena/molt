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
2. **Molt-GPU** -- Local WASM + WebGPU, used for browser-side and high-memory Worker plans
3. **PaddleOCR** -- CPU fallback, available when GPU paths are exhausted
