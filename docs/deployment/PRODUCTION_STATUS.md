# Production Status — 2026-04-20

## Live Endpoints
| Endpoint | URL | Status |
|----------|-----|--------|
| Worker | falcon-ocr.adpena.workers.dev | Live |
| App | freeinvoicemaker.app | Live |
| Test Page | falcon-ocr.adpena.workers.dev/test | Live |

## OCR Quality
| Engine | Quality | Speed | Location |
|--------|---------|-------|----------|
| Falcon-OCR WebGPU | TBD (needs browser test) | 10-100x faster than CPU | Browser |
| Falcon-OCR INT8 CPU | Expected good | 60s/token | Edge |
| Falcon-OCR INT4 CPU | Degraded (14% quant error) | 24s/token (8 layers) | Edge |
| PaddleOCR | 99.6% | Instant | Browser (last resort) |

## Assets on R2
- WASM binary: 14 MB (3.9 MB gzipped)
- INT4 weights: 129 MB (5 shards)
- INT8 weights: 257 MB (6 shards) — uploading
- Tokenizer: 4.8 MB
- SIMD ops: 4.1 KB
- Browser JS: 6 files, ~155 KB total
