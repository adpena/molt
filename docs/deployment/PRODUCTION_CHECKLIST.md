# Production Deployment Checklist

## Worker (falcon-ocr.adpena.workers.dev)

- [x] Workers AI primary OCR (Gemma 3 12B, 1.03s TTFB)
- [x] Workers AI retry logic (3 retries + 3 fallback models)
- [x] x402 payment enforcement ($0.001/req USDC)
- [x] Browser bypass (Origin: freeinvoicemaker.app)
- [x] Multi-level caching (Edge + KV)
- [x] Batch OCR endpoint (/ocr/batch)
- [x] Structured OCR endpoint (/ocr/structured)
- [x] Template extraction (/template/extract)
- [x] Health endpoint (/health)
- [x] CORS headers
- [x] Monitoring (structured JSON, no PII)
- [x] R2 asset serving for browser WASM (GET /wasm/*, GET /weights/*)
- [x] Immutable cache headers on R2 assets (max-age=86400)
- [x] WASM loading removed from ensureModelLoaded (CPU limit)

## Browser (freeinvoicemaker.app)

- [x] Molt OCR backend registered
- [x] PaddleOCR fallback
- [x] Template-from-scan button
- [x] Falcon-OCR WASM loader (deploy/browser/falcon-ocr-loader.js)
- [x] IndexedDB caching for offline use
- [x] WebGPU detection and selection
- [ ] Weight download with progress UI (onProgress callback wired, UI pending)
- [ ] End-to-end browser WASM accuracy validation

## Model

- [x] Real weights in R2 (1.03 GB F32)
- [x] INT4 weights in R2 (129 MB, 5 shards)
- [x] INT8 weights in R2 (257 MB, 6 shards)
- [x] WASM binary in R2 (13 MB, 0 dead exports)
- [x] Tokenizer in R2 (4.8 MB)
- [x] Config in R2

## Architecture Decisions

- [x] Workers: Workers AI (GPU fleet) -- NOT WASM (CPU limit)
- [x] Browser: Falcon-OCR WASM -- NOT Workers AI (privacy, offline)
- [x] Self-hosted: WASM or native binary
- [x] Documented in docs/architecture/browser-webgpu-inference.md

## Quality

- [x] Workers AI OCR accuracy (Gemma 3 12B)
- [ ] Falcon-OCR WASM accuracy (not yet tested end-to-end in browser)
- [ ] Real invoice comparison vs PaddleOCR
- [ ] Mobile browser memory pressure testing (INT4 on low-end devices)
