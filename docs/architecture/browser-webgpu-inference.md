# Browser WebGPU Inference for Falcon-OCR

## Overview

Falcon-OCR runs entirely in the browser via WebAssembly SIMD, providing
private, offline-capable OCR inference with no server round-trips.

## Architecture

```
Browser loads page
  -> Download falcon-ocr.wasm (4 MB gzipped) from CDN
  -> Download model weights from R2 (INT8: 257 MB, or INT4: 129 MB)
  -> WebAssembly.instantiateStreaming(fetch('falcon-ocr.wasm'))
  -> wasm.init(weights, config)
  -> User uploads invoice image
  -> wasm.ocr_tokens(width, height, rgb, prompt, max_tokens)
  -> Decode tokens via tokenizer
  -> Display extracted text
```

## WASM Module Exports

The compiled WASM binary (`falcon-ocr.wasm`, 13.4 MB uncompressed, ~4 MB gzipped)
exports two functions from `wasm_driver.py`:

```
init(weights_bytes: Uint8Array, config_json: string) -> void
    Initialize the model by loading weights into WASM linear memory.

ocr_tokens(width: u32, height: u32, rgb: Uint8Array, prompt_ids: Int32Array, max_new_tokens: u32) -> Int32Array
    Run OCR inference on an image and return token IDs.
```

## Performance Characteristics

| Metric | INT4 (129 MB) | INT8 (257 MB) |
|--------|---------------|---------------|
| Weight download (100 Mbps) | ~10s | ~20s |
| WASM module download | <1s | <1s |
| Cold start (download + init) | ~12s | ~22s |
| Warm inference (invoice) | TBD (WASM SIMD) | TBD (WASM SIMD) |
| Accuracy vs F32 | 90-97% | 99%+ |

INT4 is the recommended default for browser deployment: acceptable accuracy
with dramatically faster cold start.

## Privacy Model

All inference runs locally in the browser. No image data, tokens, or OCR
results are transmitted to any server. The only network requests are:

1. One-time download of `falcon-ocr.wasm` from CDN
2. One-time download of quantized weights from R2/CDN

After initial load, the system works fully offline.

## Caching Strategy

Assets are cached in IndexedDB for offline operation after first load:

- WASM module: cached via Cache API (immutable, versioned URL)
- Model weights: cached in IndexedDB (too large for Cache API's per-entry limits)
- Tokenizer vocabulary: bundled in the WASM module

```javascript
async function cacheWeights(weightsBuffer, version) {
  const db = await openDB('falcon-ocr-cache', 1, {
    upgrade(db) {
      db.createObjectStore('weights');
    },
  });
  await db.put('weights', weightsBuffer, `model-${version}`);
}

async function getCachedWeights(version) {
  const db = await openDB('falcon-ocr-cache', 1);
  return await db.get('weights', `model-${version}`);
}
```

## Enjoice Integration

The browser SDK integrates with the enjoice invoice editor:

```javascript
import { FalconOCR } from '@molt/falcon-ocr-browser';

// Initialize once (downloads + caches weights on first call)
const ocr = new FalconOCR({
  wasmUrl: 'https://cdn.freeinvoicemaker.app/falcon-ocr/v1/falcon-ocr.wasm',
  weightsUrl: 'https://cdn.freeinvoicemaker.app/falcon-ocr/v1/model-int4.safetensors',
  configUrl: 'https://cdn.freeinvoicemaker.app/falcon-ocr/v1/config.json',
  quantization: 'int4',
  onProgress: (phase, pct) => {
    // phase: 'wasm' | 'weights' | 'init'
    updateLoadingBar(phase, pct);
  },
});

await ocr.init();

// Run OCR on an uploaded invoice image
const file = document.getElementById('invoice-upload').files[0];
const imageData = await createImageBitmap(file)
  .then(bmp => {
    const canvas = new OffscreenCanvas(bmp.width, bmp.height);
    const ctx = canvas.getContext('2d');
    ctx.drawImage(bmp, 0, 0);
    return ctx.getImageData(0, 0, bmp.width, bmp.height);
  });

// Extract RGB from RGBA
const rgb = new Uint8Array(imageData.width * imageData.height * 3);
for (let i = 0, j = 0; i < imageData.data.length; i += 4, j += 3) {
  rgb[j] = imageData.data[i];
  rgb[j + 1] = imageData.data[i + 1];
  rgb[j + 2] = imageData.data[i + 2];
}

const result = await ocr.run(imageData.width, imageData.height, rgb, {
  prompt: 'Extract all text from this invoice',
  maxTokens: 512,
});

console.log(result.text);
// -> "Invoice #12345\nDate: 2026-04-14\nTotal: $1,234.56\n..."
```

## FalconOCR Browser SDK

```javascript
class FalconOCR {
  constructor(options) {
    this.wasmUrl = options.wasmUrl;
    this.weightsUrl = options.weightsUrl;
    this.configUrl = options.configUrl;
    this.quantization = options.quantization || 'int4';
    this.onProgress = options.onProgress || (() => {});
    this._instance = null;
    this._ready = false;
  }

  async init() {
    // Check IndexedDB cache first
    const cachedWeights = await getCachedWeights(this.weightsUrl);

    // Download WASM module (streaming compilation)
    this.onProgress('wasm', 0);
    const wasmResponse = await fetch(this.wasmUrl);
    const importObject = {
      env: { memory: new WebAssembly.Memory({ initial: 512, maximum: 16384 }) },
    };
    const { instance } = await WebAssembly.instantiateStreaming(wasmResponse, importObject);
    this._instance = instance;
    this.onProgress('wasm', 100);

    // Download or load cached weights
    let weightsBuffer;
    if (cachedWeights) {
      weightsBuffer = cachedWeights;
      this.onProgress('weights', 100);
    } else {
      this.onProgress('weights', 0);
      const weightsResponse = await fetch(this.weightsUrl);
      const reader = weightsResponse.body.getReader();
      const contentLength = parseInt(weightsResponse.headers.get('Content-Length') || '0', 10);
      const chunks = [];
      let received = 0;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        received += value.length;
        if (contentLength > 0) {
          this.onProgress('weights', Math.round((received / contentLength) * 100));
        }
      }

      weightsBuffer = new Uint8Array(received);
      let offset = 0;
      for (const chunk of chunks) {
        weightsBuffer.set(chunk, offset);
        offset += chunk.length;
      }

      // Cache for next time
      await cacheWeights(weightsBuffer, this.weightsUrl);
    }

    // Load config
    const configResponse = await fetch(this.configUrl);
    const configJson = await configResponse.text();

    // Initialize model in WASM
    this.onProgress('init', 0);
    this._instance.exports.init(weightsBuffer, configJson);
    this._ready = true;
    this.onProgress('init', 100);
  }

  async run(width, height, rgb, options = {}) {
    if (!this._ready) throw new Error('FalconOCR not initialized. Call init() first.');

    const promptIds = options.promptIds || new Int32Array([1]); // BOS token
    const maxTokens = options.maxTokens || 256;

    const tokenIds = this._instance.exports.ocr_tokens(
      width, height, rgb, promptIds, maxTokens
    );

    // Decode tokens (tokenizer is bundled in WASM or loaded separately)
    const text = this._decodeTokens(tokenIds);
    return { text, tokenIds: Array.from(tokenIds) };
  }

  _decodeTokens(tokenIds) {
    // The WASM module includes a decode_tokens export for the bundled tokenizer
    if (this._instance.exports.decode_tokens) {
      return this._instance.exports.decode_tokens(tokenIds);
    }
    // Fallback: return raw token IDs (caller must decode)
    return Array.from(tokenIds).join(' ');
  }
}
```

## Deployment Targets

| Target | WASM Path | Weights | Memory Limit | Status |
|--------|-----------|---------|--------------|--------|
| Browser (desktop) | falcon-ocr.wasm | INT4 (129 MB) | ~4 GB | Primary target |
| Browser (mobile) | falcon-ocr.wasm | INT4 (129 MB) | ~1-2 GB | Best-effort |
| Cloudflare Worker | falcon-ocr.wasm | F32 (1 GB) | 128 MB (free) / 512 MB (paid) | Not viable (memory) |
| Self-hosted (Node) | falcon-ocr.wasm | F32/INT8 | Unlimited | Fully supported |
| Deno Deploy | falcon-ocr.wasm | INT4 (129 MB) | 512 MB | Viable |

## Limitations

1. **No WebGPU acceleration yet**: Current WASM module uses SIMD only. WebGPU
   compute shaders would provide 10-50x speedup but require a separate
   compilation path (WGSL shaders for matmul/attention).

2. **Mobile memory pressure**: INT4 weights (129 MB) plus WASM linear memory
   may exceed mobile browser limits on low-end devices. Consider a "tiny"
   model variant (~32 MB) for mobile.

3. **First-load latency**: 129 MB download is 5-10s on fast connections but
   unacceptable on slow networks. Progressive loading UX (show partial results
   from a tiny model while large model downloads) is a future enhancement.

4. **No streaming output**: Current `ocr_tokens` returns all tokens at once.
   A streaming variant (`ocr_tokens_stream` with callback) would improve UX
   for long documents.

## Future: WebGPU Compute Path

When WebGPU is available, the inference path changes to:

```
Browser detects WebGPU support
  -> Download WGSL shader bundle (matmul, softmax, layernorm, attention)
  -> Download model weights as GPU buffers (INT8, direct upload)
  -> Create compute pipelines for each layer
  -> Run inference on GPU (10-50x faster than WASM SIMD)
  -> Read back token IDs from GPU buffer
```

This requires a separate compilation target from `molt build --target webgpu`
and is tracked in the GPU parallelism spec (`docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md`).
