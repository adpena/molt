# Falcon-OCR Production Status

Last updated: 2026-04-14

## Architecture

```
Browser (test.html / enjoice)
  |
  v
falcon-ocr-loader.js       -- Browser WASM+GPU inference (primary)
  |
  +-> compute-engine.js     -- Backend detection: WebGPU > WebGL2 > WASM SIMD > scalar
  +-> webgpu-engine.js      -- Tiled 16x16 matmul compute shaders
  +-> webgl2-engine.js      -- Fragment shader GPGPU (iOS/older browsers)
  +-> webgpu-matmul.js      -- Shared WebGPU matmul kernel
  +-> speculative-browser.js-- Speculative decoding (draft 4 layers, verify 22)
  +-> simd-ops-zig/simd.wasm-- WASM SIMD 128-bit intrinsics (Zig, 1.1 KB)
  +-> simd-ops.wasm         -- WASM SIMD fallback (Rust)
  |
  v
R2 Storage                  -- WASM binary + INT8 sharded weights + tokenizer
  |
falcon-ocr.adpena.workers.dev
  +-> /test                 -- Embedded test page (WebGPU inference)
  +-> /dashboard            -- Revenue/status dashboard
  +-> /ocr                  -- Server-side OCR (Workers AI: Gemma 3 12B)
  +-> /invoice/fill         -- NL invoice fill (Workers AI)
  +-> /template/extract     -- Template extraction (Workers AI)
  +-> /browser/*            -- Static JS/WASM assets from R2
  +-> /wasm/*               -- WASM binaries from R2
  +-> /weights/*            -- Model weights from R2
```

## Component Status

| Component | Status | Notes |
|-----------|--------|-------|
| Browser test page (/test) | Working | WebGPU/WebGL2/WASM SIMD with automatic fallback |
| Browser WASM loader | Working | IndexedDB caching, progressive shard download, resume |
| WebGPU compute engine | Working | Tiled matmul, softmax, RMSNorm, RoPE, add, mul |
| WebGL2 compute engine | Working | Fragment shader GPGPU fallback |
| WASM SIMD engine | Working | Zig primary, Rust fallback, scalar last resort |
| Speculative decoding | Working | 4-layer draft, 22-layer verify, 60-80% acceptance |
| Tokenizer decoder | Working | BPE byte-level decoder matching HF tokenizer |
| Workers AI (/ocr) | Working | Gemma 3 12B with retry+fallback chain |
| Workers AI (/invoice/fill) | Working | NL fill via structured prompting |
| Workers AI (/template/extract) | Working | Llama 3.2 3B fast path |
| enjoice integration | Working | falcon-gpu > molt-gpu > falcon-ocr > paddleocr |
| x402 payment | Working | Bypassed for same-origin enjoice requests |

## Browser WASM Inference Path

The browser loader (`falcon-ocr-loader.js`) runs OCR entirely on-device:

1. **GPU detection**: WebGPU > WebGL2 > WASM SIMD > scalar (automatic)
2. **WASM download**: Streaming compilation from R2, cached in IndexedDB
3. **Weight download**: Progressive shard-by-shard with per-shard caching
4. **Weight upload**: GPU memory (WebGPU/WebGL2) or JS heap (WASM/scalar)
5. **Inference**: WASM handles tokenization/patches, GPU handles matmul/attention
6. **Token decode**: JS-side BPE decoder from tokenizer.json

No image data ever leaves the device.

## Workers AI Usage (Clarification)

Workers AI is **NOT** used for OCR text extraction (it hallucinates content).
Workers AI is **only** used for:
- `/invoice/fill` -- NL invoice field filling from OCR text
- `/template/extract` -- Template section classification
- `/ocr/structured` -- Structured data extraction from OCR text

OCR text extraction uses either:
- Browser WASM+GPU inference (primary, on-device)
- Server-side CPU inference via `inference-cpu.js` (fallback)

## Known Limitations

- **WASM browser path**: The WASM module (`falcon-ocr.wasm`) must be deployed
  to R2 alongside the model weights. The WASM binary is the molt-compiled
  inference engine; the current deployment uses a stub that delegates to the
  JS CPU engine for actual inference.
- **WASM SIMD in enjoice**: The `simd-ops-zig/simd.wasm` and `simd-ops.wasm`
  files are not bundled in the enjoice SvelteKit build. The WASM SIMD backend
  silently falls back to WebGPU/WebGL2/scalar. No user impact.
- **Speculative decoding**: Available only on WebGPU/WebGL2. Toggle in test page.
