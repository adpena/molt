/**
 * Browser-side Falcon-OCR inference via WASM.
 *
 * Downloads the WASM binary and INT4 quantized weights from R2, caches them
 * in IndexedDB for offline use, and runs OCR inference entirely in the browser.
 * No image data ever leaves the device.
 *
 * Weight loading uses progressive shard-by-shard download with per-shard
 * progress reporting and independent IndexedDB caching, enabling resume of
 * interrupted downloads without re-fetching completed shards.
 *
 * Usage:
 *   import { FalconOCR } from './falcon-ocr-loader.js';
 *
 *   const ocr = new FalconOCR({
 *     onProgress: (phase, pct) => console.log(`${phase}: ${pct}%`),
 *   });
 *   await ocr.init();
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
   * @param {string} [config.weightsVariant] - Weight variant path (default: 'falcon-ocr-int4')
   * @param {(phase: string, percent: number, detail?: object) => void} [config.onProgress]
   */
  constructor(config = {}) {
    const base = config.baseUrl || 'https://falcon-ocr.adpena.workers.dev';
    this.wasmUrl = config.wasmUrl || `${base}/wasm/falcon-ocr.wasm`;
    this.weightsVariant = config.weightsVariant || 'falcon-ocr-int4';
    this.weightsBaseUrl = `${base}/weights/${this.weightsVariant}`;
    this.onProgress = config.onProgress || (() => {});
    this._instance = null;
    this._ready = false;
  }

  /**
   * Download WASM + weights, initialize model. Must be called before recognize().
   *
   * Weight shards are downloaded progressively and cached independently in
   * IndexedDB. If a download is interrupted, only uncached shards are fetched
   * on the next init() call. Progress is reported per-shard via onProgress
   * with phase 'weights' and detail { shard, totalShards, shardBytes }.
   */
  async init() {
    if (this._ready) return;

    // 1. Download and compile WASM module (streaming compilation)
    this.onProgress('wasm', 0);
    let wasmModule = await getCached('wasm:' + this.wasmUrl);
    if (wasmModule) {
      this.onProgress('wasm', 100);
    } else {
      wasmModule = await fetchWithProgress(this.wasmUrl, (received, total) => {
        if (total > 0) this.onProgress('wasm', Math.round((received / total) * 100));
      });
      await setCached('wasm:' + this.wasmUrl, wasmModule);
      this.onProgress('wasm', 100);
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

    // 4. Download scales
    const scalesUrl = `${this.weightsBaseUrl}/scales.json`;
    let scalesBuffer = await getCached('scales:' + scalesUrl);
    if (!scalesBuffer) {
      scalesBuffer = await fetchWithProgress(scalesUrl, () => {});
      await setCached('scales:' + scalesUrl, scalesBuffer);
    }
    const scalesJson = new TextDecoder().decode(scalesBuffer);
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

    // 7. Initialize model — loads weights into WASM linear memory
    this.onProgress('init', 0);
    const config = JSON.parse(configJson);
    const scales = JSON.parse(scalesJson);
    this._instance.exports.init(weightsBuffer, config, scales);
    this._ready = true;
    this.onProgress('init', 100);
  }

  /**
   * Run OCR on an ImageData or raw RGB buffer.
   *
   * @param {ImageData | { width: number, height: number, rgb: Uint8Array }} image
   * @param {object} [options]
   * @param {string} [options.prompt] - Optional prompt for guided extraction
   * @param {number} [options.maxTokens] - Maximum tokens to generate (default 512)
   * @returns {Promise<{ text: string, tokenIds: number[], timeMs: number }>}
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

    // Encode prompt as token IDs (BOS token = 1 if no prompt)
    const promptIds = new Int32Array([1]);

    const start = performance.now();
    const tokenIds = this._instance.exports.ocr_tokens(
      width, height, rgb, promptIds, maxTokens
    );
    const timeMs = performance.now() - start;

    // Decode tokens using the bundled tokenizer export
    let text;
    if (this._instance.exports.decode_tokens) {
      text = this._instance.exports.decode_tokens(tokenIds);
    } else {
      // Fallback: return space-joined token IDs
      text = Array.from(tokenIds).join(' ');
    }

    return { text, tokenIds: Array.from(tokenIds), timeMs };
  }

  /**
   * Check if the model is ready for inference.
   * @returns {boolean}
   */
  get ready() {
    return this._ready;
  }

  /**
   * Release WASM memory. The instance cannot be reused after this.
   */
  dispose() {
    this._instance = null;
    this._ready = false;
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
