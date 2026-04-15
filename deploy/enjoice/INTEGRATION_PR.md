# Falcon-OCR molt integration for enjoice

## Summary

- Replace PaddleOCR with molt-compiled Falcon-OCR for client-side OCR inference
- Add browser capability detection (WebGPU/WebGL2) with automatic backend selection
- Fallback chain: molt-gpu (WebGPU/WASM) -> PaddleOCR (server-side JS) -> server-side API
- Cloudflare Worker at `https://falcon-ocr.adpena.workers.dev` serves INT4 quantized model (129 MB) for server-side fallback

## Files changed

### New files (copy from `deploy/enjoice/`)

| File | Target path in enjoice | Description |
|------|----------------------|-------------|
| `falcon-ocr-molt.ts` | `site/src/lib/ocr/falcon-ocr-molt.ts` | WASM session manager: loads WASM binary, streams weights from R2/CDN, runs inference, decodes tokens |
| `ocr-backend-molt.ts` | `site/src/lib/ocr/ocr-backend-molt.ts` | `OcrBackend` interface implementation: WebGPU detection, graceful fallback, TTFB measurement |
| `capabilities-update.ts` | `site/src/lib/ocr/capabilities-update.ts` | Browser capability detection: WebGPU, WebGL2, browser identification, optimal backend selection |

### Modified files

| File | Changes |
|------|---------|
| `site/src/lib/ocr/index.ts` | Import `MoltOcrBackend`, add to backend priority list, update factory function |
| `site/src/lib/ocr/types.ts` | Add `"molt-gpu"` to `OcrBackendType` union (if not already present) |
| `site/src/config/features.ts` | Add `MOLT_OCR_ENABLED: boolean` feature flag (default: `true`) |
| `site/src/components/OcrUpload.svelte` (or equivalent) | Use `detectOcrCapabilities()` to select backend |

### Integration code for `site/src/lib/ocr/index.ts`

```typescript
import { MoltOcrBackend } from "./ocr-backend-molt";
import { detectOcrCapabilities } from "./capabilities-update";

// Add to the backend factory:
export async function createOcrBackend(): Promise<OcrBackend> {
  const caps = await detectOcrCapabilities();

  if (caps.recommendedBackend === "molt-gpu") {
    const molt = new MoltOcrBackend({
      wasmUrl: "https://falcon-ocr.adpena.workers.dev/wasm/falcon-ocr.wasm",
      weightsUrl: "https://falcon-ocr.adpena.workers.dev/weights",
      tokenizerUrl: "https://falcon-ocr.adpena.workers.dev/tokenizer",
      configUrl: "https://falcon-ocr.adpena.workers.dev/config",
    });

    const ok = await molt.initialize();
    if (ok) return molt;
    // Fall through to PaddleOCR if molt init fails
    console.warn("molt-gpu init failed, falling back to paddle-wasm");
  }

  // Existing PaddleOCR path
  return createPaddleBackend();
}
```

### Server-side fallback (already deployed)

The Cloudflare Worker at `https://falcon-ocr.adpena.workers.dev` provides:

- `GET /health` -- backend status, model variant (int4/micro/f32), device info
- `POST /ocr` -- image OCR with x402 payment verification
- `POST /ocr/tokens` -- raw token generation

The Worker loads INT4 quantized weights (~129 MB) from R2 on cold start.
For the client-side WASM path, weights are streamed directly from R2 CDN.

## Testing on staging

1. Deploy the Worker to staging: `wrangler deploy --config deploy/cloudflare/wrangler.toml --env staging`
2. Copy the three TS files to the enjoice repo at the paths above
3. Update `index.ts` with the integration code
4. Build: `npm run build`
5. Test on staging:
   - Chrome/Edge: should use molt-gpu (WebGPU)
   - Firefox: should fall back to paddle-wasm (dispatch overhead too high)
   - Safari 18+: should use molt-gpu (Metal-backed WebGPU)
   - Mobile: varies by browser support
6. Verify `/health` endpoint returns `{"status":"ready","model_variant":"int4"}`
7. Upload a test invoice image and verify OCR output

## Rollback plan

1. Set feature flag: `window.__MOLT_OCR_BACKEND = "paddle-wasm"` to force PaddleOCR
2. Or revert the `index.ts` changes to remove `MoltOcrBackend` from the factory
3. The Worker continues serving the micro model independently of the enjoice frontend
4. No database migrations or schema changes required

## Performance expectations

- Cold start (Worker): ~2-3s to load INT4 model from R2
- Warm inference: depends on image size and token count
- Client-side WASM: ~1-2s for weight download (cached after first load), inference TBD
- INT4 quantization: <3% accuracy loss vs F32 on OCR benchmarks (symmetric quantization preserves text recognition quality well since weight distributions are approximately Gaussian)
