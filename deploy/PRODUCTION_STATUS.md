# Falcon-OCR Deployment Status

Last updated: 2026-04-22

This document is a deployment status note, not a guarantee that every listed
path is production-proven. Claims below are limited to behavior covered by the
current source contracts and targeted tests.

## Architecture

```
Browser (test.html / enjoice)
  |
  v
falcon-ocr-loader.js       -- Browser WASM + optional accelerated compute
  |
  +-> compute-engine.js     -- Backend selection with fail-closed forced modes
  +-> webgpu-engine.js      -- WebGPU compute engine
  +-> webgl2-engine.js      -- WebGL2 compute engine
  +-> webgpu-matmul.js      -- Shared WebGPU matmul kernel
  +-> speculative-browser.js-- Speculative decoding support when explicitly available
  +-> simd-ops-zig/simd.wasm-- WASM SIMD artifact
  +-> simd-ops.wasm         -- Rust WASM SIMD parity artifact
  |
  v
R2 Storage                  -- WASM binary + INT8 sharded weights + tokenizer
  |
falcon-ocr.adpena.workers.dev
  +-> /test                 -- Browser test page
  +-> /dashboard            -- Status page
  +-> /ocr                  -- Explicit backend routing only
  +-> /invoice/fill         -- NL invoice fill
  +-> /template/extract     -- Template extraction
  +-> /browser/*            -- Static JS/WASM assets from R2
  +-> /wasm/*               -- WASM binaries from R2
  +-> /weights/*            -- Model weights from R2
```

## Component Status

| Component | Status | Evidence / contract |
|-----------|--------|---------------------|
| Browser test page (/test) | Contracted | Exposes `window.__falconOCR` for Browser Rendering automation and releases `ImageBitmap` previews. |
| Browser WASM loader | Contracted | Fails closed on unexpected compute backend init errors; preserves token decode signal. |
| WebGPU/WebGL2 compute engines | Experimental | Backend selection is explicit; forced unavailable backends raise. |
| WASM SIMD engine | Contracted | Requires real module exports before reporting `wasm-simd`; Rust parity artifact stays under the size gate. |
| Speculative decoding | Experimental | Only exposed when WebGPU/WebGL2 support is available. No production acceptance-rate claim is made here. |
| Tokenizer decoders | Contracted | Browser, Cloudflare, enjoice, and tinygrad decoders preserve unknown token IDs or raise on vocab drift. |
| Worker `/ocr` | Explicit backend routing | Missing tokenizer and GPU proxy failures fail closed; default guidance points clients to configured alternatives. |
| Worker `/invoice/fill` | Experimental | Workers AI structured prompting path. |
| Worker `/template/extract` | Experimental | Workers AI template extraction path. |
| enjoice integration | Contracted bridge | No hard-coded Nemotron endpoint; unknown tokens and whitespace are preserved. |
| x402 payment | Contracted | Price metadata excludes unverified GPU quality or latency claims. |

## Browser WASM Inference Path

The browser loader runs OCR on-device when assets are available:

1. Detect compute backend.
2. Download/compile WASM from R2 or IndexedDB cache.
3. Download model shards and tokenizer.
4. Upload weights to the selected compute backend when supported.
5. Run inference and decode tokens locally.

Unexpected compute-engine import/init failures raise instead of silently
reporting a normal WASM backend. Known no-accelerator cases may use the WASM
path with an explicit fallback reason.

## Workers AI Scope

Workers AI is not claimed as the canonical OCR text extractor in this status
file. It is used for structured/NL tasks such as invoice filling and template
classification where those routes are configured. OCR text extraction should be
validated through the browser/WASM path, explicit GPU proxy path, or a separate
configured OCR service.

## Known Limits

- The browser path depends on correctly deployed WASM, tokenizer, and sharded
  weights in R2.
- Nemotron OCR v2 is not a Worker-native runtime here. It requires an explicitly
  configured GPU service or a separate native/exported runtime path.
- Browser Rendering GPU automation depends on `window.__falconOCR` becoming ready
  on `/test`; failures should be surfaced as automation errors, not hidden as
  successful empty OCR.
