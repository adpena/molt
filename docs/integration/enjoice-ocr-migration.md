# enjoice OCR Migration Guide: molt GPU Stack

This document describes how to update enjoice's Falcon-OCR integration
to use the new molt-compiled WASM module deployed on Cloudflare Workers.

## Overview

The current enjoice OCR architecture has three backends (defined in
`site/src/lib/ocr/index.ts`):

1. **Falcon-OCR** (browser-side, WebGPU) -- best accuracy, currently
   loading from a manifest-driven Molt driver module
2. **PaddleOCR** (browser-side, WASM) -- production-ready fallback
3. **Server-side OCR** (via `/api/ocr`) -- last resort

The migration replaces the browser-side Falcon-OCR path with the new
molt-compiled WASM module.  PaddleOCR and server-side OCR are
**unchanged**.

## What Changes

### 1. `site/src/lib/ocr/falcon-wrapper.ts`

**Current state:** Loads a browser module via dynamic `import()` from a
configured URL, initializes a `FalconDriverSession`, and calls
`session.ocrTokens()`.

**New state:** Load the molt-compiled `falcon-ocr.wasm` directly.  The
WASM module exports `init()` and `ocr_tokens()` matching the Python API.

Changes required:

- Replace `loadFalconDriverModule()` with a direct `WebAssembly.instantiateStreaming()`
  call pointing to the deployed WASM URL.
- Remove `FalconDriverModule` and `FalconDriverSession` interfaces.
- Replace `session.ocrTokens()` with a direct call to the WASM export:
  `wasmInstance.exports.ocr_tokens(width, height, rgb, promptIds, maxNewTokens)`.
- Keep the existing `imageToRgbPatchAligned()` logic for image
  preprocessing (or move it to a local utility since it's no longer
  part of the driver module).
- Keep the tokenizer loading and decode logic (`falcon-tokenizer.ts`)
  -- the WASM module returns token IDs, not text.

### 2. `site/src/lib/ocr/falcon-config.ts`

**Current state:** Three configurable URLs: browser module, manifest,
tokenizer.

**New state:** Two URLs: WASM module and tokenizer.

Changes required:

- Remove `browserModuleUrl` / `FALCON_OCR_BROWSER_MODULE_URL` -- replaced
  by a direct WASM URL.
- Remove `manifestUrl` / `FALCON_OCR_MANIFEST_URL` -- no longer needed.
- Add `wasmUrl` / `FALCON_OCR_WASM_URL` pointing to the deployed WASM binary
  (default: `https://falcon-ocr.freeinvoicemaker.workers.dev/falcon-ocr.wasm`
  or served from R2 via the Worker).
- Keep `tokenizerUrl` / `FALCON_OCR_TOKENIZER_URL`.

### 3. `site/src/lib/capabilities.ts`

**No changes required.**  The existing `webgpu` capability check is
sufficient.  The new WASM module uses WebGPU when available and falls
back to CPU WASM execution.

### 4. `site/src/lib/ocr/index.ts`

**Minimal changes.**  The `OcrResult` type, the multi-backend fallback
chain, and the PaddleOCR/server paths are all unchanged.

The only change is that `loadFalconOcrWasm()` now loads the new WASM
module instead of the old manifest-driven driver.  This is encapsulated
in `falcon-wrapper.ts`, so `index.ts` should not need changes beyond
possibly updating error messages.

### 5. `site/src/lib/ocr/falcon-tokenizer.ts`

**No changes required.**  Token encoding/decoding is independent of the
inference module.

## What Does NOT Change

- PaddleOCR fallback path (unchanged)
- Server-side OCR fallback path (unchanged)
- `OcrResult` type definition (unchanged)
- `OcrBlock` type definition (unchanged)
- Multi-backend auto-selection logic (unchanged)
- Telemetry tracking calls (unchanged)
- Privacy model: images never leave the user's device for the
  browser-side paths (unchanged)

## Performance Expectations

| Metric | Target | Notes |
|--------|--------|-------|
| WASM load | < 200ms | Cached after first load via Service Worker |
| Weight fetch | < 500ms | Served from R2 with edge caching |
| Cold start (first inference) | < 2s | WASM load + weight fetch + init |
| Warm inference (TTFT) | < 500ms | Model already initialized |
| Token throughput | 50-100 tok/s | WebGPU path; CPU WASM is slower |
| WASM binary size (gzip) | < 2 MB | Target for acceptable load time |

## Deployment Checklist

1. **Upload artifacts to R2:**
   ```bash
   # Compile WASM module
   molt build src/molt/stdlib/tinygrad/wasm_driver.py --target wasm

   # Upload to R2 (via wrangler or dashboard)
   wrangler r2 object put falcon-ocr-weights/models/falcon-ocr/falcon-ocr.wasm --file falcon-ocr.wasm
   wrangler r2 object put falcon-ocr-weights/models/falcon-ocr/weights.safetensors --file weights.safetensors
   wrangler r2 object put falcon-ocr-weights/models/falcon-ocr/config.json --file config.json
   ```

2. **Deploy the Cloudflare Worker:**
   ```bash
   cd deploy/cloudflare
   wrangler secret put X402_WALLET_ADDRESS
   wrangler secret put X402_VERIFICATION_URL
   wrangler deploy
   ```

3. **Verify the Worker:**
   ```bash
   curl https://falcon-ocr.freeinvoicemaker.workers.dev/health
   # Expected: {"status":"loading","model":"falcon-ocr","version":"0.1.0","device":"wasm"}
   ```

4. **Update enjoice config:**
   - Set `FALCON_OCR_WASM_URL` to the R2 public URL or Worker URL
   - Set `FALCON_OCR_TOKENIZER_URL` to the tokenizer JSON URL
   - Remove old `FALCON_OCR_BROWSER_MODULE_URL` and `FALCON_OCR_MANIFEST_URL`

5. **Test in staging:**
   - Verify Falcon-OCR loads and runs in Chrome (WebGPU)
   - Verify PaddleOCR fallback works in Firefox (no WebGPU)
   - Verify server-side fallback works when both client-side engines fail
   - Run the existing OCR test suite

6. **Deploy to production:**
   - Feature-flag the new WASM path behind `FALCON_OCR_V2=true`
   - Canary deploy to 5% of traffic
   - Monitor error rates, latency, and accuracy via telemetry
   - Roll out to 100% after 48h stability

## Rollback Plan

If the new WASM module has issues in production:

1. Remove `FALCON_OCR_WASM_URL` from enjoice config
2. The `isFalconConfigured()` check will return false
3. PaddleOCR automatically becomes the primary backend
4. No code changes required -- just a config change
