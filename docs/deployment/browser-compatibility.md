# Browser Compatibility Matrix: Falcon-OCR

## Supported Browsers

| Browser | WebGPU | Backend | Dispatch Overhead | Status |
|---------|--------|---------|------------------|--------|
| Chrome 134+ | Yes | Dawn/Vulkan | 24-36 us | Primary target |
| Safari 26+ | Yes | Metal | 32 us | Primary target |
| Edge 144+ | Yes | Dawn/D3D12 | 59-67 us | Supported |
| Firefox | Yes | wgpu | ~1037 us | Warning: rate-limited, use PaddleOCR fallback |
| Chrome (Android) | Yes | Dawn/Vulkan | TBD | Mobile target |
| Safari (iOS 26+) | Yes | Metal | TBD | Mobile target |
| Pre-WebGPU browsers | No | N/A | N/A | PaddleOCR fallback |

Dispatch overhead numbers sourced from Maczan 2026 benchmarks and
internal testing (see `docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md`).

## Detection Logic

### WebGPU Detection

```typescript
async function hasWebGpu(): Promise<boolean> {
  if (typeof navigator === "undefined" || !("gpu" in navigator)) {
    return false;
  }
  try {
    const adapter = await navigator.gpu.requestAdapter({
      powerPreference: "high-performance",
    });
    return adapter !== null;
  } catch {
    return false;
  }
}
```

### WebGL2 Detection (Fallback Check)

```typescript
function hasWebGl2(): boolean {
  try {
    const canvas = document.createElement("canvas");
    return canvas.getContext("webgl2") !== null;
  } catch {
    return false;
  }
}
```

### Browser Identification

```typescript
function detectBrowser(): { name: string; version: number } {
  const ua = navigator.userAgent;
  if (/Edg\/(\d+)/.test(ua)) return { name: "edge", version: parseInt(RegExp.$1) };
  if (/Chrome\/(\d+)/.test(ua) && !/Edg/.test(ua)) return { name: "chrome", version: parseInt(RegExp.$1) };
  if (/Version\/(\d+).*Safari/.test(ua) && !/Chrome/.test(ua)) return { name: "safari", version: parseInt(RegExp.$1) };
  if (/Firefox\/(\d+)/.test(ua)) return { name: "firefox", version: parseInt(RegExp.$1) };
  return { name: "unknown", version: 0 };
}
```

## Backend Selection Logic

```
WebGPU available?
  ├─ Yes
  │   ├─ Firefox? → PaddleOCR (dispatch overhead too high: ~1037 us)
  │   └─ Other    → molt-gpu (primary path)
  └─ No
      ├─ WebGL2?  → PaddleOCR (WASM with WebGL2 acceleration)
      └─ Neither  → Server-side OCR (/api/ocr endpoint)
```

The selection logic is implemented in `deploy/enjoice/capabilities-update.ts`
and can be overridden with the `__MOLT_OCR_BACKEND` feature flag set to
`"molt-gpu"`, `"paddle-wasm"`, or `"server-side"`.

## Firefox Dispatch Rate-Limiting

Firefox uses the `wgpu` WebGPU implementation which has ~1037 us dispatch
overhead per GPU call (Maczan 2026 benchmarks). This is approximately
30x slower than Chrome's Dawn/Vulkan path (24-36 us). The overhead
accumulates across the hundreds of dispatches required per inference call,
making the WebGPU path slower than PaddleOCR's CPU WASM path in Firefox.

Recommendation: default to PaddleOCR for Firefox users. Display a
one-time informational notice:

> "Using PaddleOCR for best Firefox performance. Falcon-OCR WebGPU
> is available but currently slower in Firefox due to dispatch overhead."

The notice should be dismissible and rate-limited (show once per session).

## Performance Expectations

### Desktop

| Browser | Backend | Cold Start | Warm Inference | Token Throughput |
|---------|---------|-----------|----------------|-----------------|
| Chrome | molt-gpu | < 2s | < 500ms | 50-100 tok/s |
| Safari | molt-gpu | < 2s | < 500ms | 50-100 tok/s |
| Edge | molt-gpu | < 2.5s | < 600ms | 40-80 tok/s |
| Firefox | PaddleOCR | < 1s | < 800ms | N/A (full text) |
| Any | Server-side | N/A | < 1.5s | N/A |

### Mobile

| Device | Backend | Cold Start | Warm Inference |
|--------|---------|-----------|----------------|
| Chrome Android | molt-gpu | < 3s | < 1s |
| Safari iOS 26+ | molt-gpu | < 3s | < 1s |
| Older mobile | PaddleOCR | < 2s | < 1.5s |

### Resource Usage

| Metric | Target |
|--------|--------|
| WASM binary (gzip) | < 2 MB |
| Model weights | ~50 MB (streamed from R2, cached) |
| Peak GPU memory | < 512 MB |
| Peak CPU memory (WASM) | < 256 MB |

## Graceful Degradation

The system implements a three-tier fallback chain:

1. **molt-gpu** (WebGPU): highest quality, fastest on supported browsers
2. **PaddleOCR** (WASM): production-ready fallback, works everywhere
3. **Server-side OCR** (API): last resort, works on any browser

If the primary backend fails at runtime (WASM load failure, GPU OOM),
the worker returns a structured 503 response with `fallback_available: true`
and `fallback_url: "/api/ocr/paddle"`. The client-side code detects this
and automatically retries with PaddleOCR.

## Testing

To verify browser compatibility locally:

1. Open Chrome DevTools, check `navigator.gpu` in Console
2. Run `deploy/enjoice/capabilities-update.ts` detection logic
3. Verify correct backend is selected for your browser
4. Test PaddleOCR fallback by disabling WebGPU in chrome://flags
