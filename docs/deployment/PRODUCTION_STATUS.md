# Production Status — 2026-04-14

## Live Endpoints
| Endpoint | URL | Status |
|----------|-----|--------|
| Worker | falcon-ocr.adpena.workers.dev | Live |
| App | freeinvoicemaker.app | Live |
| Test Page | falcon-ocr.adpena.workers.dev/test | Live |
| Health | falcon-ocr.adpena.workers.dev/health | Live (503 while loading, 200 when ready) |

## Differential Test Results
- **72/72 pass** — full parity across native, WASM, and LLVM backends
- All edge cases covered: NaN propagation, overflow, denormals, boundary conditions

## OCR Quality
| Engine | Quality | Speed | Location |
|--------|---------|-------|----------|
| PaddleOCR | 99.6% | Instant | Browser (production primary) |
| Falcon-OCR INT8 CPU | Good (16x better than INT4) | ~60s/token | Edge (sharded load) |
| Falcon-OCR INT4 CPU | Degraded (14% quant error) | ~24s/token (8 layers) | Edge (fallback) |
| Falcon-OCR WebGPU | TBD | 10-100x faster than CPU | Browser |

## Model Loading Strategy (Workers, 256 MB limit)
| Priority | Variant | Total Size | Peak Memory | Strategy |
|----------|---------|-----------|-------------|----------|
| 0 | INT8 sharded | 257 MB (6 shards) | Full decoded tensor map + one shard buffer | Load one shard buffer at a time; decoded tensors remain resident |
| 1 | INT4 sharded | 129 MB (5 shards) | Full decoded tensor map + one shard buffer | Same sharded loading approach |
| 2 | INT4 single | 129 MB | 129 MB | Direct load |
| 3 | Micro model | 263 KB | <1 MB | Embedded, always works |

## Assets on R2
| Asset | Size | Path |
|-------|------|------|
| WASM binary | 10.5 MB (raw) | models/falcon-ocr/falcon-ocr.wasm |
| INT8 weights | 270 MB (6 shards, 43-52 MB each) | models/falcon-ocr-int8-sharded/ |
| INT4 weights | 129 MB (5 shards) | models/falcon-ocr-int4-sharded/ |
| Tokenizer | 4.8 MB | models/falcon-ocr/tokenizer.json |
| Zig SIMD | 5.4 KB | browser/simd-ops-zig/simd.wasm |
| Rust SIMD | 14.0 KB | browser/simd-ops.wasm |
| Browser JS | ~155 KB total | browser/*.js |

## WASM SIMD Backend

### Binary Sizes
| Backend | Size | Notes |
|---------|------|-------|
| Zig | 5.4 KB | Primary, smaller binary |
| Rust | 14.0 KB | Full 4x16 register-blocked matmul_f32_fast |

### Benchmark Results (64x64 matmul, Apple Silicon M-series, Node.js V8)
| Operation | Rust ns/op | Zig ns/op | Winner | Speedup |
|-----------|-----------|-----------|--------|---------|
| matmul_f32_tiled 64x64 | ~14,500 | ~36,000 | Rust | 2.5x |
| matmul_f32_fast 64x64 | ~10,500 | N/A | Rust | -- |
| matmul_f32_tiled 16x16 | ~240 | ~560 | Rust | 2.3x |
| softmax_f32_fused 1024 | ~2,250 | ~3,200 | Rust | 1.4x |
| add_f32 4096 | ~350 | ~980 | Rust | 2.8x |
| exp2_f32 1024 | ~360 | ~1,330 | Rust | 3.7x |
| rms_norm_f32 256 | ~143 | ~167 | Rust | 1.2x |
| reduce_sum_f32 4096 | ~600 | ~700 | Rust | 1.2x |

### Optimizations in Rust SIMD
- **matmul_f32_tiled**: 4x4 register blocking, K-loop unrolled by 4
- **matmul_f32_fast**: 4x16 register blocking (16 f32x4 accumulators), fully unrolled K-loop by 4, no memset, precomputed row pointers
- **softmax**: Online 2-pass algorithm (Milakov & Gimelshein 2018), vectorized pass 2
- **exp2**: 6th-order Cephes minimax polynomial (max relative error ~2.3e-8)

## Cloudflare Services Inventory
| Service | Usage |
|---------|-------|
| Workers | Main inference endpoint, API routing, x402 payment verification |
| R2 | Model weight storage (INT8/INT4 shards), WASM binaries, tokenizer |
| KV | Session state, model metadata cache, batch result state |
| Durable Objects | Stateful inference sessions and per-IP rate limiter |
| Browser Rendering | WebGPU inference path (browser-gpu-inference.js) |
| Queues | Batch OCR processing (queue-batch-ocr.js) |

## GPU Inference Proxy
| Setting | Value |
|---------|-------|
| Route | `X-Use-Backend: gpu` on `/ocr` |
| Supported providers | HuggingFace Inference Endpoints, Replicate, RunPod, Modal, Fly.io |
| Status | Wired in Worker, awaiting GPU_INFERENCE_URL/KEY/PROVIDER secrets |
| Docker image | `ghcr.io/tiiuae/falcon-ocr:latest` |
| Minimum GPU | NVIDIA T4 (16 GB VRAM) for INT8 |
| Recommended GPU | A100 40 GB for bfloat16 full-precision |

## WASM Binary Analysis (10.1 MB raw)
| Section | Size | Notes |
|---------|------|-------|
| Code | 8.2 MB (8,644,113 bytes) | 9,934 functions |
| Data | 1.8 MB (1,881,563 bytes) | 1,358 data segments |
| Other | 0.1 MB | type, import, func, elem, export, custom |

Reduction opportunities:
- Tree-shake unused tinygrad Tensor methods (Falcon-OCR uses ~15 of ~100)
- Dead function elimination on unused codegen paths
- Data segment deduplication (1,358 segments may have overlap)
- Further wasm-opt passes (Oz with --converge)
- Theoretical achievable: 4-6 MB (50-60% of current size)

## Load Test Results (2026-04-14)
| Endpoint | Requests | Concurrency | Avg Latency | Status |
|----------|----------|-------------|-------------|--------|
| /health | 20 | 5 | 115ms | 503 (model not warmed) |
| /invoice/fill (Workers AI) | 10 | 5 | 2.04s | 200 (10/10) |
| /ocr (GPU proxy) | 1 | 1 | N/A | 503 (x402 payment-gated) |

## Production Hardening
| Check | Status |
|-------|--------|
| Rate limiting | Per-IP, 100 req/min via Durable Object (POST only) |
| CORS | Locked to freeinvoicemaker.app (no wildcard) |
| Error responses | All paths return JSON with request_id |
| PII logging | None (no image content logged) |
| x402 payment | Required for all POST inference endpoints |
| Bot protection | CF Bot Management + User-Agent allowlist |
| Pre-warming | ctx.waitUntil model load on every request |
| Cache layers | Edge Cache API + KV + in-memory model state |
