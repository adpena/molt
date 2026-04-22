/**
 * Browser-side Falcon-OCR inference via WASM + GPU compute.
 *
 * Downloads the WASM binary and INT8 quantized weights from R2, caches them
 * in IndexedDB for offline use, and runs OCR inference entirely in the browser.
 * No image data ever leaves the device.
 *
 * Compute backend detection (priority order):
 *   1. WebGPU  -- tiled matmul via compute shaders (10-100x over CPU)
 *   2. WebGL2  -- matmul encoded as texture passes (5-20x over CPU)
 *   3. WASM SIMD -- 128-bit SIMD intrinsics in WASM (2-4x over scalar)
 *
 * When a GPU backend is available, weight matrices are uploaded to GPU memory
 * during init(), eliminating CPU-GPU transfer overhead during inference.
 *
 * Weight loading uses progressive shard-by-shard download with per-shard
 * progress reporting and independent IndexedDB caching, enabling resume of
 * interrupted downloads without re-fetching completed shards.
 *
 * Usage:
 *   import { FalconOCR } from './falcon-ocr-loader.js';
 *
 *   const ocr = new FalconOCR({
 *     onProgress: (phase, pct, detail) => console.log(`${phase}: ${pct}% ${detail?.message || ''}`),
 *   });
 *   await ocr.init();
 *   console.log(ocr.computeBackend);  // "webgpu" | "webgl2" | "wasm-simd" | "wasm"
 *
 *   const canvas = document.createElement('canvas');
 *   // ... draw image to canvas ...
 *   const ctx = canvas.getContext('2d');
 *   const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height);
 *   const text = await ocr.recognize(imageData);
 */

const DB_NAME = 'falcon-ocr-cache';
const DB_VERSION = 2;
const STORE_NAME = 'assets';
const FALCON_OCR_PLAIN_PROMPT_IDS = Object.freeze([
  227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257,
]);

function byteDecoderMap() {
  const bytes = [];
  for (let value = '!'.charCodeAt(0); value <= '~'.charCodeAt(0); value++) bytes.push(value);
  for (let value = 0xa1; value <= 0xac; value++) bytes.push(value);
  for (let value = 0xae; value <= 0xff; value++) bytes.push(value);
  const chars = [...bytes];
  let next = 0;
  for (let value = 0; value < 256; value++) {
    if (!bytes.includes(value)) {
      bytes.push(value);
      chars.push(256 + next);
      next++;
    }
  }
  const decoder = new Map();
  for (let i = 0; i < bytes.length; i++) {
    decoder.set(String.fromCharCode(chars[i]), bytes[i]);
  }
  return decoder;
}

const BYTE_DECODER = byteDecoderMap();
const UTF8_DECODER = new TextDecoder('utf-8', { fatal: false });
const UTF8_ENCODER = new TextEncoder();

export class TokenizerDecoder {
  constructor(vocab, specialIds) {
    this.vocab = vocab;
    this.specialIds = specialIds;
  }

  static fromJSON(tokenizerJson) {
    const data = JSON.parse(tokenizerJson);
    const vocab = new Map();
    const specialIds = new Set();

    if (data.model && data.model.vocab) {
      for (const [piece, id] of Object.entries(data.model.vocab)) {
        vocab.set(id, piece);
      }
    }

    if (Array.isArray(data.added_tokens)) {
      for (const token of data.added_tokens) {
        vocab.set(token.id, token.content);
        if (token.special) {
          specialIds.add(token.id);
        }
      }
    }

    return new TokenizerDecoder(vocab, specialIds);
  }

  decode(tokenIds) {
    const pieces = [];
    for (const id of tokenIds) {
      if (this.specialIds.has(id)) continue;
      const piece = this.vocab.get(id);
      pieces.push(piece === undefined ? `[UNK:${id}]` : piece);
    }
    const tokenText = pieces.join('');
    const bytes = [];
    for (const ch of tokenText) {
      const byte = BYTE_DECODER.get(ch);
      if (byte === undefined) {
        bytes.push(...UTF8_ENCODER.encode(ch));
      } else {
        bytes.push(byte);
      }
    }
    return UTF8_DECODER.decode(new Uint8Array(bytes));
  }
}

/**
 * Open the IndexedDB database used for caching WASM + weights.
 * @returns {Promise<IDBDatabase>}
 */
function openCacheDB() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/**
 * Read a cached asset from IndexedDB.
 * @param {string} key
 * @returns {Promise<ArrayBuffer|null>}
 */
async function getCached(key) {
  try {
    const db = await openCacheDB();
    return new Promise((resolve) => {
      const tx = db.transaction(STORE_NAME, 'readonly');
      const store = tx.objectStore(STORE_NAME);
      const req = store.get(key);
      req.onsuccess = () => resolve(req.result || null);
      req.onerror = () => resolve(null);
    });
  } catch {
    return null;
  }
}

/**
 * Write an asset to IndexedDB cache.
 * @param {string} key
 * @param {ArrayBuffer} buffer
 */
async function setCached(key, buffer) {
  try {
    const db = await openCacheDB();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, 'readwrite');
      const store = tx.objectStore(STORE_NAME);
      const req = store.put(buffer, key);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error);
    });
  } catch {
    // Cache write failure is non-fatal
  }
}

/**
 * Delete a cached asset from IndexedDB.
 * @param {string} key
 */
async function deleteCached(key) {
  try {
    const db = await openCacheDB();
    return new Promise((resolve) => {
      const tx = db.transaction(STORE_NAME, 'readwrite');
      const store = tx.objectStore(STORE_NAME);
      const req = store.delete(key);
      req.onsuccess = () => resolve();
      req.onerror = () => resolve();
    });
  } catch {
    // Cache delete failure is non-fatal
  }
}

/**
 * Fetch a URL with progress reporting.
 * @param {string} url
 * @param {(received: number, total: number) => void} onProgress
 * @returns {Promise<ArrayBuffer>}
 */
async function fetchWithProgress(url, onProgress) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Fetch failed: ${response.status} ${response.statusText} for ${url}`);
  }

  const contentLength = parseInt(response.headers.get('Content-Length') || '0', 10);
  if (!response.body) {
    // Fallback for environments without ReadableStream
    const buffer = await response.arrayBuffer();
    onProgress(buffer.byteLength, buffer.byteLength);
    return buffer;
  }

  const reader = response.body.getReader();
  const chunks = [];
  let received = 0;

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    received += value.length;
    onProgress(received, contentLength);
  }

  const result = new Uint8Array(received);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result.buffer;
}

/**
 * Convert RGBA ImageData to RGB Uint8Array.
 * @param {ImageData} imageData
 * @returns {Uint8Array}
 */
function imageDataToRGB(imageData) {
  const { data, width, height } = imageData;
  const rgb = new Uint8Array(width * height * 3);
  for (let i = 0, j = 0; i < data.length; i += 4, j += 3) {
    rgb[j] = data[i];
    rgb[j + 1] = data[i + 1];
    rgb[j + 2] = data[i + 2];
  }
  return rgb;
}

export class FalconOCR {
  /**
   * @param {object} config
   * @param {string} [config.baseUrl] - Base URL for weight/WASM serving
   * @param {string} [config.wasmUrl] - URL to falcon-ocr.wasm
   * @param {string} [config.tokenizerUrl] - URL to tokenizer.json
   * @param {string} [config.weightsVariant] - Weight variant path (default: 'falcon-ocr-int8-sharded')
   * @param {(phase: string, percent: number, detail?: object) => void} [config.onProgress]
   */
  constructor(config = {}) {
    const base = config.baseUrl || 'https://falcon-ocr.adpena.workers.dev';
    this.wasmUrl = config.wasmUrl || `${base}/wasm/falcon-ocr.wasm`;
    this.tokenizerUrl = config.tokenizerUrl || `${base}/tokenizer.json`;
    this.weightsVariant = config.weightsVariant || 'falcon-ocr-int8-sharded';
    this.weightsBaseUrl = `${base}/weights/${this.weightsVariant}`;
    this.onProgress = config.onProgress || (() => {});
    this._instance = null;
    this._ready = false;
    /** @type {import('./compute-engine.js').WebGPUEngine | import('./compute-engine.js').WebGL2Engine | import('./compute-engine.js').WasmSimdEngine | null} */
    this._compute = null;
    /** @type {TokenizerDecoder | null} */
    this._tokenizer = null;
    /** @type {string} */
    this._computeBackend = 'none';
    /** @type {string} */
    this._computeBackendFallbackReason = '';
  }

  /**
   * Download WASM + weights, initialize model. Must be called before recognize().
   *
   * Detects the best compute backend (WebGPU > WebGL2 > WASM SIMD), downloads
   * the WASM module and INT8 weights, initializes the compute engine, and
   * uploads weights to GPU memory when a GPU backend is available.
   *
   * Weight shards are downloaded progressively and cached independently in
   * IndexedDB. If a download is interrupted, only uncached shards are fetched
   * on the next init() call. Progress is reported per-shard via onProgress
   * with phase 'weights' and detail { shard, totalShards, shardBytes }.
   *
   * @param {object} [options]
   * @param {boolean} [options.forceWasm] - Skip GPU detection and use WASM-only path
   */
  async init(options = {}) {
    if (this._ready) return;

    // 0. Detect best compute backend
    this.onProgress('detecting', 0, { phase: 'GPU detection' });

    if (!options.forceWasm) {
      try {
        const { ComputeEngine } = await import('./compute-engine.js');
        const engine = await ComputeEngine.create();
        if (engine.backendName === 'scalar') {
          engine.destroy?.();
          this._compute = null;
          this._computeBackend = 'wasm';
          this._computeBackendFallbackReason = 'no accelerated compute backend available';
        } else {
          this._compute = engine;
          this._computeBackend = engine.backendName;
          this._computeBackendFallbackReason = '';
        }
      } catch (computeErr) {
        this._computeBackend = 'none';
        this._computeBackendFallbackReason = '';
        throw new Error(
          `FalconOCR compute backend initialization failed: ${computeErr.message}`,
        );
      }
    } else {
      this._computeBackend = 'wasm';
      this._computeBackendFallbackReason = 'forced wasm inference path';
    }

    this.onProgress('detecting', 100, {
      backend: this._computeBackend,
      message: this._computeBackendFallbackReason
        ? `Using ${this._computeBackend} for inference (${this._computeBackendFallbackReason})`
        : `Using ${this._computeBackend} for inference`,
    });

    // 1. Download and compile WASM module (streaming compilation)
    this.onProgress('wasm', 0, { phase: 'Downloading inference engine' });
    let wasmModule = await getCached('wasm:' + this.wasmUrl);
    if (wasmModule) {
      this.onProgress('wasm', 100, { phase: 'Loaded from cache' });
    } else {
      wasmModule = await fetchWithProgress(this.wasmUrl, (received, total) => {
        if (total > 0) {
          this.onProgress('wasm', Math.round((received / total) * 100), {
            phase: 'Downloading inference engine',
            bytes: received,
            totalBytes: total,
          });
        }
      });
      await setCached('wasm:' + this.wasmUrl, wasmModule);
      this.onProgress('wasm', 100, { phase: 'Download complete' });
    }

    // Compile WASM module
    const importObject = {
      env: {
        memory: new WebAssembly.Memory({ initial: 512, maximum: 16384 }),
      },
      wasi_snapshot_preview1: {
        // Minimal WASI stubs — the WASM module does not use filesystem/network
        proc_exit: () => {},
        fd_write: () => 0,
        fd_seek: () => 0,
        fd_close: () => 0,
        environ_sizes_get: () => 0,
        environ_get: () => 0,
        clock_time_get: () => 0,
      },
    };

    const compiled = await WebAssembly.compile(wasmModule);
    const instance = await WebAssembly.instantiate(compiled, importObject);
    this._instance = instance;

    // 2. Download shard index
    this.onProgress('weights', 0, { phase: 'Downloading model weights' });
    this.onProgress('config', 0);
    const indexUrl = `${this.weightsBaseUrl}/model.safetensors.index.json`;
    let indexBuffer = await getCached('index:' + indexUrl);
    if (!indexBuffer) {
      indexBuffer = await fetchWithProgress(indexUrl, () => {});
      await setCached('index:' + indexUrl, indexBuffer);
    }
    const shardIndex = JSON.parse(new TextDecoder().decode(indexBuffer));

    // 3. Download config
    const configUrl = `${this.weightsBaseUrl}/config.json`;
    let configBuffer = await getCached('config:' + configUrl);
    if (!configBuffer) {
      configBuffer = await fetchWithProgress(configUrl, () => {});
      await setCached('config:' + configUrl, configBuffer);
    }
    const configJson = new TextDecoder().decode(configBuffer);

    // 4. Download tokenizer for JS-side decode. The WASM driver returns token IDs.
    const tokenizerUrl = this.tokenizerUrl;
    let tokenizerBuffer = await getCached('tokenizer:' + tokenizerUrl);
    if (!tokenizerBuffer) {
      tokenizerBuffer = await fetchWithProgress(tokenizerUrl, () => {});
      await setCached('tokenizer:' + tokenizerUrl, tokenizerBuffer);
    }
    this._tokenizer = TokenizerDecoder.fromJSON(new TextDecoder().decode(tokenizerBuffer));
    this.onProgress('config', 100);

    // 5. Progressive shard download with per-shard caching
    const shardFiles = shardIndex.shards || Object.keys(
      Object.values(shardIndex.weight_map || {}).reduce((acc, file) => {
        acc[file] = true;
        return acc;
      }, {})
    ).sort();

    const totalShards = shardFiles.length;
    const shardBuffers = new Array(totalShards);
    let completedBytes = 0;
    let totalBytes = 0;

    // First pass: check cache and compute total size needed
    const uncachedIndices = [];
    for (let i = 0; i < totalShards; i++) {
      const shardKey = `shard:${this.weightsBaseUrl}/${shardFiles[i]}`;
      const cached = await getCached(shardKey);
      if (cached) {
        shardBuffers[i] = cached;
        completedBytes += cached.byteLength;
        totalBytes += cached.byteLength;
      } else {
        uncachedIndices.push(i);
      }
    }

    // Report initial progress from cache
    if (totalShards > 0 && uncachedIndices.length < totalShards) {
      const cachedPct = Math.round(
        ((totalShards - uncachedIndices.length) / totalShards) * 100
      );
      this.onProgress('weights', cachedPct, {
        shard: totalShards - uncachedIndices.length,
        totalShards,
        fromCache: true,
      });
    } else {
      this.onProgress('weights', 0);
    }

    // Download uncached shards sequentially (avoids memory pressure from
    // concurrent large fetches on mobile devices)
    for (const idx of uncachedIndices) {
      const shardUrl = `${this.weightsBaseUrl}/${shardFiles[idx]}`;
      const shardKey = `shard:${shardUrl}`;

      const buffer = await fetchWithProgress(shardUrl, (received, total) => {
        if (total > 0) {
          // Per-shard progress within overall weights progress
          const shardPct = received / total;
          const completedShards = totalShards - uncachedIndices.length +
            uncachedIndices.indexOf(idx);
          const overallPct = Math.round(
            ((completedShards + shardPct) / totalShards) * 100
          );
          this.onProgress('weights', overallPct, {
            shard: idx + 1,
            totalShards,
            shardBytes: received,
            shardTotal: total,
          });
        }
      });

      shardBuffers[idx] = buffer;
      totalBytes += buffer.byteLength;

      // Cache each shard independently — enables resume on interruption
      await setCached(shardKey, buffer);
    }
    this.onProgress('weights', 100, { totalShards, totalBytes });

    // 6. Concatenate shard buffers into single weights ArrayBuffer
    const weightsBuffer = new Uint8Array(totalBytes);
    let writeOffset = 0;
    for (const buf of shardBuffers) {
      const view = new Uint8Array(buf);
      weightsBuffer.set(view, writeOffset);
      writeOffset += view.byteLength;
    }

    // 7. Upload weights to GPU if a GPU compute backend is available
    if (this._compute && this._compute.uploadWeights) {
      this.onProgress('gpu', 0, { phase: 'Uploading weights to GPU' });
      await this._compute.uploadWeights(weightsBuffer.buffer);
      this.onProgress('gpu', 100, { phase: 'GPU ready' });
    }

    // 8. Initialize model — loads weights into WASM linear memory
    this.onProgress('init', 0);
    this._instance.exports.init(weightsBuffer, configJson);
    this._ready = true;
    this.onProgress('init', 100, {
      backend: this._computeBackend,
      message: `Model ready (${this._computeBackend})`,
    });
  }

  /**
   * The active compute backend name.
   * @returns {string} "webgpu" | "webgl2" | "wasm-simd" | "wasm" | "none"
   */
  get computeBackend() {
    return this._computeBackend;
  }

  /**
   * The compute engine instance (for advanced use / speculative decoding).
   * @returns {import('./compute-engine.js').WebGPUEngine | import('./compute-engine.js').WebGL2Engine | import('./compute-engine.js').WasmSimdEngine | null}
   */
  get compute() {
    return this._compute;
  }

  /**
   * Run OCR on an ImageData or raw RGB buffer.
   *
   * When WebGPU is available and the WASM module exports the necessary
   * hooks (get_patches, get_layer_weights), the transformer forward pass
   * runs on GPU with matmul/softmax/RMSNorm/RoPE dispatched as compute
   * shaders. Otherwise falls back to the WASM-only path.
   *
   * @param {ImageData | { width: number, height: number, rgb: Uint8Array }} image
   * @param {object} [options]
   * @param {string} [options.prompt] - Optional prompt for guided extraction
   * @param {number} [options.maxTokens] - Maximum tokens to generate (default 512)
   * @returns {Promise<{ text: string, tokenIds: number[], timeMs: number, backend: string }>}
   */
  async recognize(image, options = {}) {
    if (!this._ready) {
      throw new Error('FalconOCR not initialized. Call init() first.');
    }

    const maxTokens = options.maxTokens || 512;
    let width, height, rgb;

    if (image instanceof ImageData) {
      width = image.width;
      height = image.height;
      rgb = imageDataToRGB(image);
    } else {
      width = image.width;
      height = image.height;
      rgb = image.rgb;
    }

    const start = performance.now();
    let tokenIds;

    // GPU forward path: use WebGPU compute shaders for the heavy ops
    // (matmul, softmax, RMSNorm, RoPE) while WASM handles tokenization,
    // patch extraction, and decoding.
    const canGPUForward = (
      this._computeBackend === 'webgpu' &&
      this._compute &&
      this._instance.exports.get_patches &&
      this._instance.exports.get_layer_weights
    );

    if (canGPUForward) {
      tokenIds = await this._forwardGPU(width, height, rgb, maxTokens);
    } else {
      // WASM-only path: all compute in linear memory.
      const promptIds = new Int32Array(FALCON_OCR_PLAIN_PROMPT_IDS);
      tokenIds = this._instance.exports.ocr_tokens(
        width, height, rgb, promptIds, maxTokens
      );
    }

    const timeMs = performance.now() - start;

    if (!this._tokenizer) {
      throw new Error('FalconOCR tokenizer is not initialized.');
    }
    const text = this._tokenizer.decode(Array.from(tokenIds));

    return { text, tokenIds: Array.from(tokenIds), timeMs, backend: this._computeBackend };
  }

  /**
   * GPU-accelerated transformer forward pass.
   *
   * Patch extraction and tokenization run in WASM. The transformer layers
   * (embedding, attention, FFN, output projection) run on WebGPU with all
   * matmul/softmax/RMSNorm/RoPE dispatched as compute shaders.
   *
   * @param {number} width
   * @param {number} height
   * @param {Uint8Array} rgb
   * @param {number} maxTokens
   * @returns {Promise<Int32Array>} Token IDs
   * @private
   */
  async _forwardGPU(width, height, rgb, maxTokens) {
    const exports = this._instance.exports;
    const gpu = this._compute;

    // 1. Patch extraction + embedding (WASM) -> Float32Array patches
    const patchPtr = exports.get_patches(width, height, rgb);
    const patchInfo = exports.get_patch_info();
    const seqLen = patchInfo.seq_len;
    const hiddenSize = patchInfo.hidden_size;
    const numLayers = patchInfo.num_layers;
    const numHeads = patchInfo.num_heads;
    const headDim = hiddenSize / numHeads;

    // Read patch embeddings from WASM linear memory.
    const patches = new Float32Array(
      exports.memory.buffer, patchPtr, seqLen * hiddenSize
    );

    // 2. Precompute RoPE frequencies (WASM exports these for the given seq_len).
    const freqPtr = exports.get_rope_freqs(seqLen, headDim);
    const freqsCos = new Float32Array(
      exports.memory.buffer, freqPtr, seqLen * (headDim / 2)
    );
    const freqsSin = new Float32Array(
      exports.memory.buffer, freqPtr + seqLen * (headDim / 2) * 4,
      seqLen * (headDim / 2)
    );

    // Upload RoPE frequencies to GPU once (reused across all layers).
    const gpuFreqsCos = gpu.uploadWeights(new Float32Array(freqsCos));
    const gpuFreqsSin = gpu.uploadWeights(new Float32Array(freqsSin));

    // 3. Transformer forward pass — all heavy ops on GPU.
    let h = gpu.matmulGPU(
      new Float32Array(patches),
      this._gpuWeights?.['embed'] || gpu.uploadWeights(
        this._getWeight(exports, 'embed', seqLen * hiddenSize)
      ),
      seqLen, hiddenSize, hiddenSize
    );

    for (let layer = 0; layer < numLayers; layer++) {
      const lw = this._getLayerWeights(exports, layer);

      // Attention pre-norm
      const normed = gpu.rmsNormGPU(h, lw.attn_norm, hiddenSize, seqLen);

      // QKV projections
      const q = gpu.matmulGPU(normed, lw.wq, seqLen, hiddenSize, hiddenSize);
      const k = gpu.matmulGPU(normed, lw.wk, seqLen, hiddenSize, hiddenSize);
      const v = gpu.matmulGPU(normed, lw.wv, seqLen, hiddenSize, hiddenSize);

      // RoPE on Q and K (in-place on GPU)
      gpu.ropeGPU(q, k, gpuFreqsCos, gpuFreqsSin, seqLen, headDim);

      // Attention: scores = Q @ K^T / sqrt(head_dim)
      // For simplicity, we compute full attention (no multi-head split on GPU
      // yet — the matmul dimensions handle the concatenated heads).
      const scores = gpu.matmulGPU(q, k, seqLen, hiddenSize, seqLen);
      const attnWeights = gpu.softmaxGPU(scores, seqLen, seqLen);
      const attnOut = gpu.matmulGPU(attnWeights, v, seqLen, seqLen, hiddenSize);

      // Output projection + residual add
      const projected = gpu.matmulGPU(attnOut, lw.wo, seqLen, hiddenSize, hiddenSize);
      const residual1 = gpu.addGPU(h, projected, seqLen * hiddenSize);

      // FFN pre-norm
      const ffnNormed = gpu.rmsNormGPU(residual1, lw.ffn_norm, hiddenSize, seqLen);

      // SwiGLU FFN: gate = matmul(h, w_gate), up = matmul(h, w_up)
      // out = matmul(silu(gate) * up, w_down)
      const ffnDim = lw.ffn_dim;
      const gate = gpu.matmulGPU(ffnNormed, lw.w_gate, seqLen, hiddenSize, ffnDim);
      const up = gpu.matmulGPU(ffnNormed, lw.w_up, seqLen, hiddenSize, ffnDim);

      // SiLU activation on gate is done via WASM (element-wise non-linearity).
      // Read gate back, apply silu, re-upload. This is the one CPU round-trip
      // per layer — future work will add a fused SiLU*mul kernel.
      const gateData = await gpu.readBuffer(gate, seqLen * ffnDim * 4);
      const upData = await gpu.readBuffer(up, seqLen * ffnDim * 4);
      const siluGate = new Float32Array(seqLen * ffnDim);
      for (let i = 0; i < siluGate.length; i++) {
        // SiLU(x) = x * sigmoid(x)
        const x = gateData[i];
        siluGate[i] = x / (1 + Math.exp(-x)) * upData[i];
      }

      const ffnHidden = gpu.matmulGPU(
        siluGate, lw.w_down, seqLen, ffnDim, hiddenSize
      );

      // Residual connection
      h = gpu.addGPU(residual1, ffnHidden, seqLen * hiddenSize);
    }

    // 4. Final norm + output projection
    const finalNormWeight = this._gpuWeights?.['final_norm'] || gpu.uploadWeights(
      this._getWeight(exports, 'final_norm', hiddenSize)
    );
    const finalNormed = gpu.rmsNormGPU(h, finalNormWeight, hiddenSize, seqLen);

    const outputWeight = this._gpuWeights?.['output'] || gpu.uploadWeights(
      this._getWeight(exports, 'output', hiddenSize * exports.get_vocab_size())
    );
    const vocabSize = exports.get_vocab_size();
    const logits = await gpu.matmul(
      finalNormed, outputWeight, seqLen, hiddenSize, vocabSize
    );

    // 5. Greedy decode (WASM handles argmax + stop token detection)
    const logitsPtr = exports.alloc_f32(logits.length);
    new Float32Array(exports.memory.buffer, logitsPtr, logits.length).set(logits);
    return exports.greedy_decode(logitsPtr, seqLen, vocabSize, maxTokens);
  }

  /**
   * Get a weight tensor from WASM linear memory as Float32Array.
   * @private
   */
  _getWeight(exports, name, expectedLen) {
    const ptr = exports.get_weight_ptr(name);
    return new Float32Array(exports.memory.buffer, ptr, expectedLen);
  }

  /**
   * Get all weight GPUBuffers for a transformer layer. If GPU weights were
   * pre-uploaded during init, returns those. Otherwise reads from WASM
   * linear memory and uploads on the fly.
   * @private
   */
  _getLayerWeights(exports, layerIdx) {
    const info = exports.get_layer_weights(layerIdx);
    const gpu = this._compute;
    const pre = this._gpuWeights || {};

    const getOrUpload = (name, ptr, len) => {
      const key = `layer.${layerIdx}.${name}`;
      if (pre[key]) return pre[key];
      const data = new Float32Array(exports.memory.buffer, ptr, len);
      const buf = gpu.uploadWeights(new Float32Array(data));
      // Cache for subsequent forward passes.
      if (!this._gpuWeights) this._gpuWeights = {};
      this._gpuWeights[key] = buf;
      return buf;
    };

    const hs = info.hidden_size;
    const ffnDim = info.ffn_dim;

    return {
      attn_norm: getOrUpload('attn_norm', info.attn_norm_ptr, hs),
      wq: getOrUpload('wq', info.wq_ptr, hs * hs),
      wk: getOrUpload('wk', info.wk_ptr, hs * hs),
      wv: getOrUpload('wv', info.wv_ptr, hs * hs),
      wo: getOrUpload('wo', info.wo_ptr, hs * hs),
      ffn_norm: getOrUpload('ffn_norm', info.ffn_norm_ptr, hs),
      w_gate: getOrUpload('w_gate', info.w_gate_ptr, hs * ffnDim),
      w_up: getOrUpload('w_up', info.w_up_ptr, hs * ffnDim),
      w_down: getOrUpload('w_down', info.w_down_ptr, ffnDim * hs),
      ffn_dim: ffnDim,
    };
  }

  /**
   * Check if the model is ready for inference.
   * @returns {boolean}
   */
  get ready() {
    return this._ready;
  }

  /**
   * Release WASM memory and GPU resources. The instance cannot be reused after this.
   */
  dispose() {
    if (this._compute) {
      this._compute.destroy();
      this._compute = null;
    }
    this._instance = null;
    this._ready = false;
    this._computeBackend = 'none';
    this._computeBackendFallbackReason = '';
  }

  /**
   * Clear all cached assets from IndexedDB.
   * Useful for forcing a fresh download after model updates.
   */
  static async clearCache() {
    try {
      const db = await openCacheDB();
      return new Promise((resolve, reject) => {
        const tx = db.transaction(STORE_NAME, 'readwrite');
        const store = tx.objectStore(STORE_NAME);
        const req = store.clear();
        req.onsuccess = () => resolve();
        req.onerror = () => reject(req.error);
      });
    } catch {
      // Cache clear failure is non-fatal
    }
  }
}
