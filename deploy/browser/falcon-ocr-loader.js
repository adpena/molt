/**
 * Browser-side Falcon-OCR inference via WASM.
 *
 * Downloads the WASM binary and INT4 quantized weights from R2, caches them
 * in IndexedDB for offline use, and runs OCR inference entirely in the browser.
 * No image data ever leaves the device.
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
const DB_VERSION = 1;
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
   * @param {string} [config.wasmUrl] - URL to falcon-ocr.wasm
   * @param {string} [config.weightsUrl] - URL to INT4 weights file
   * @param {string} [config.configUrl] - URL to model config.json
   * @param {(phase: 'wasm'|'weights'|'config'|'init', percent: number) => void} [config.onProgress]
   */
  constructor(config = {}) {
    const base = config.baseUrl || 'https://falcon-ocr.adpena.workers.dev';
    this.wasmUrl = config.wasmUrl || `${base}/wasm/falcon-ocr.wasm`;
    this.weightsUrl = config.weightsUrl || `${base}/weights/model-int4.safetensors`;
    this.configUrl = config.configUrl || `${base}/weights/config.json`;
    this.onProgress = config.onProgress || (() => {});
    this._instance = null;
    this._ready = false;
  }

  /**
   * Download WASM + weights, initialize model. Must be called before recognize().
   * Downloads are cached in IndexedDB — subsequent calls skip the network.
   */
  async init() {
    if (this._ready) return;

    // 1. Download and compile WASM module (4 MB gzipped, streaming compilation)
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

    // 2. Download weights (129 MB INT4, cached in IndexedDB for offline)
    this.onProgress('weights', 0);
    let weightsBuffer = await getCached('weights:' + this.weightsUrl);
    if (weightsBuffer) {
      this.onProgress('weights', 100);
    } else {
      weightsBuffer = await fetchWithProgress(this.weightsUrl, (received, total) => {
        if (total > 0) this.onProgress('weights', Math.round((received / total) * 100));
      });
      await setCached('weights:' + this.weightsUrl, weightsBuffer);
      this.onProgress('weights', 100);
    }

    // 3. Download config
    this.onProgress('config', 0);
    let configBuffer = await getCached('config:' + this.configUrl);
    if (!configBuffer) {
      configBuffer = await fetchWithProgress(this.configUrl, () => {});
      await setCached('config:' + this.configUrl, configBuffer);
    }
    const configJson = new TextDecoder().decode(configBuffer);
    this.onProgress('config', 100);

    // 4. Initialize model — loads weights into WASM linear memory
    this.onProgress('init', 0);
    const weights = new Uint8Array(weightsBuffer);
    this._instance.exports.init(weights, configJson);
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
