# Production Status — 2026-04-14

## Live Endpoints
| Endpoint | URL | Status |
|----------|-----|--------|
| Worker | falcon-ocr.adpena.workers.dev | Live (v310c2be8) |
| App | freeinvoicemaker.app | Live |
| Test Page | falcon-ocr.adpena.workers.dev/test | Live |
| Health | falcon-ocr.adpena.workers.dev/health | Live (503 while loading, 200 when ready) |

## OCR Quality
| Engine | Quality | Speed | Location |
|--------|---------|-------|----------|
| PaddleOCR | 99.6% | Instant | Browser (production primary) |
| Falcon-OCR INT8 CPU | Good (16x better than INT4) | ~60s/token | Edge (streaming shard load) |
| Falcon-OCR INT4 CPU | Degraded (14% quant error) | ~24s/token (8 layers) | Edge (fallback) |
| Falcon-OCR WebGPU | TBD | 10-100x faster than CPU | Browser |

## Model Loading Strategy (Workers, 256 MB limit)
| Priority | Variant | Total Size | Peak Memory | Strategy |
|----------|---------|-----------|-------------|----------|
| 0 | INT8 sharded | 257 MB (6 shards) | ~80 MB | Stream: load shard, extract tensors, drop buffer, next |
| 1 | INT4 sharded | 129 MB (5 shards) | ~60 MB | Same streaming approach |
| 2 | INT4 single | 129 MB | 129 MB | Direct load |
| 3 | Micro model | 263 KB | <1 MB | Embedded, always works |

## Assets on R2
| Asset | Size | Path |
|-------|------|------|
| WASM binary | 10.5 MB (raw) | models/falcon-ocr/falcon-ocr.wasm |
| INT8 weights | 270 MB (6 shards, 43-52 MB each) | models/falcon-ocr-int8/ |
| INT4 weights | 129 MB (5 shards) | models/falcon-ocr-int4-sharded/ |
| Tokenizer | 4.8 MB | models/falcon-ocr/tokenizer.json |
| Zig SIMD | 1.1 KB | browser/simd-ops-zig.wasm |
| Rust SIMD (fallback) | 4.1 KB | browser/simd-ops.wasm |
| Browser JS | ~155 KB total | browser/*.js |

## WASM SIMD Backend
- Primary: Zig binary (1.1 KB, 47% smaller per-op)
- Fallback: Rust binary (4.1 KB)
- Both provide f32x4 vectorized matmul, softmax, rmsNorm, add, mul

## Production Hardening
| Check | Status |
|-------|--------|
| Rate limiting | Per-IP, 100 req/min via KV counter (POST only) |
| CORS | Locked to freeinvoicemaker.app (no wildcard) |
| Error responses | All paths return JSON with request_id |
| PII logging | None (no image content logged) |
| x402 payment | Required for all POST inference endpoints |
| Bot protection | CF Bot Management + User-Agent allowlist |
| Pre-warming | ctx.waitUntil model load on every request |
| Cache layers | Edge Cache API + KV + in-memory model state |
