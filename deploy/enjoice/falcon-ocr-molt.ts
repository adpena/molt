/**
 * Drop-in replacement for enjoice's falcon-wrapper.ts using molt-compiled WASM.
 *
 * Loads the Falcon-OCR WASM binary from R2/CDN, initializes it with WebGPU
 * memory when available, streams weights, runs inference, and decodes tokens.
 *
 * Usage:
 *   import { createFalconOcrSession } from "./falcon-ocr-molt";
 *   const session = await createFalconOcrSession({ wasmUrl, weightsUrl, tokenizerUrl });
 *   const result = await session.ocr(imageData);
 *   session.dispose();
 */

// --------------------------------------------------------------------------
// Types
// --------------------------------------------------------------------------

export interface FalconOcrConfig {
  /** URL to the molt-compiled falcon-ocr.wasm binary. */
  wasmUrl: string;
  /** URL to the model weights (safetensors format, served from R2). */
  weightsUrl: string;
  /** URL to the tokenizer vocabulary JSON. */
  tokenizerUrl: string;
  /** URL to the model config JSON. */
  configUrl: string;
  /** Maximum tokens to generate per inference call. Default: 512. */
  maxNewTokens?: number;
  /** Chunk size in bytes for streaming weight downloads. Default: 1MB. */
  weightChunkSize?: number;
  /** Base URL of the falcon-ocr Worker (for template extraction and batch endpoints). */
  workerUrl?: string;
}

export interface OcrResult {
  text: string;
  boundingBoxes: BoundingBox[];
  confidence: number;
  timeMs: number;
  device: "webgpu" | "wasm";
  tokenCount: number;
}

export interface BoundingBox {
  x: number;
  y: number;
  width: number;
  height: number;
  text: string;
  confidence: number;
}

export interface FalconOcrSession {
  /** Run OCR on an ImageData or raw RGB buffer. */
  ocr(image: ImageData | { width: number; height: number; rgb: Uint8Array }): Promise<OcrResult>;
  /** Run OCR and return raw token IDs (no decode). */
  ocrTokens(
    width: number,
    height: number,
    rgb: Uint8Array,
    promptIds: number[],
    maxNewTokens?: number,
  ): Promise<number[]>;
  /** Whether the model is initialized and ready. */
  readonly ready: boolean;
  /** Release all resources. */
  dispose(): void;
}

// --------------------------------------------------------------------------
// Tokenizer
// --------------------------------------------------------------------------

interface TokenizerVocab {
  idToToken: Map<number, string>;
  tokenToId: Map<string, number>;
}

async function loadTokenizer(url: string): Promise<TokenizerVocab> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to load tokenizer from ${url}: ${response.status}`);
  }
  const json: Record<string, number> = await response.json();
  const idToToken = new Map<number, string>();
  const tokenToId = new Map<string, number>();
  for (const [token, id] of Object.entries(json)) {
    idToToken.set(id, token);
    tokenToId.set(token, id);
  }
  return { idToToken, tokenToId };
}

function decodeTokens(tokenIds: number[], vocab: TokenizerVocab): string {
  const parts: string[] = [];
  for (const id of tokenIds) {
    const token = vocab.idToToken.get(id);
    if (token !== undefined) {
      // Handle sentencepiece-style tokens: leading \u2581 -> space
      parts.push(token.replace(/\u2581/g, " "));
    }
  }
  return parts.join("").trim();
}

// --------------------------------------------------------------------------
// Image preprocessing
// --------------------------------------------------------------------------

const PATCH_SIZE = 16;

function imageDataToRgb(imageData: ImageData): { width: number; height: number; rgb: Uint8Array } {
  const { width, height, data } = imageData;
  const pixelCount = width * height;
  const rgb = new Uint8Array(pixelCount * 3);
  for (let i = 0; i < pixelCount; i++) {
    rgb[i * 3] = data[i * 4];
    rgb[i * 3 + 1] = data[i * 4 + 1];
    rgb[i * 3 + 2] = data[i * 4 + 2];
  }
  return { width, height, rgb };
}

function alignToPatch(
  width: number,
  height: number,
): { width: number; height: number } {
  return {
    width: Math.ceil(width / PATCH_SIZE) * PATCH_SIZE,
    height: Math.ceil(height / PATCH_SIZE) * PATCH_SIZE,
  };
}

function padRgb(
  rgb: Uint8Array,
  origWidth: number,
  origHeight: number,
  newWidth: number,
  newHeight: number,
): Uint8Array {
  if (origWidth === newWidth && origHeight === newHeight) {
    return rgb;
  }
  const padded = new Uint8Array(newWidth * newHeight * 3);
  for (let y = 0; y < origHeight; y++) {
    const srcOffset = y * origWidth * 3;
    const dstOffset = y * newWidth * 3;
    padded.set(rgb.subarray(srcOffset, srcOffset + origWidth * 3), dstOffset);
  }
  return padded;
}

// --------------------------------------------------------------------------
// Weight streaming
// --------------------------------------------------------------------------

async function streamWeights(
  url: string,
  chunkSize: number,
): Promise<Uint8Array> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to fetch weights from ${url}: ${response.status}`);
  }

  const contentLength = parseInt(response.headers.get("Content-Length") ?? "0", 10);
  if (!response.body) {
    // Fallback: read entire body at once
    return new Uint8Array(await response.arrayBuffer());
  }

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let totalRead = 0;

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    totalRead += value.byteLength;
  }

  // Concatenate chunks
  const result = new Uint8Array(totalRead);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

// --------------------------------------------------------------------------
// WASM module management
// --------------------------------------------------------------------------

interface WasmExports {
  memory: WebAssembly.Memory;
  init: (weightsBytes: Uint8Array, configJson: string) => void;
  ocr_tokens: (
    width: number,
    height: number,
    rgb: Uint8Array,
    promptIds: number[],
    maxNewTokens: number,
  ) => number[];
}

async function instantiateWasm(
  wasmUrl: string,
): Promise<{ instance: WebAssembly.Instance; exports: WasmExports }> {
  const response = await fetch(wasmUrl);
  if (!response.ok) {
    throw new Error(`Failed to fetch WASM from ${wasmUrl}: ${response.status}`);
  }

  const memory = new WebAssembly.Memory({ initial: 256, maximum: 2048 });
  const importObject = {
    env: { memory },
  };

  let instance: WebAssembly.Instance;
  if (typeof WebAssembly.instantiateStreaming === "function") {
    const result = await WebAssembly.instantiateStreaming(response, importObject);
    instance = result.instance;
  } else {
    const bytes = await response.arrayBuffer();
    const module = await WebAssembly.compile(bytes);
    instance = await WebAssembly.instantiate(module, importObject);
  }

  return {
    instance,
    exports: instance.exports as unknown as WasmExports,
  };
}

// --------------------------------------------------------------------------
// Session factory
// --------------------------------------------------------------------------

export async function createFalconOcrSession(
  config: FalconOcrConfig,
): Promise<FalconOcrSession> {
  const maxNewTokens = config.maxNewTokens ?? 512;
  const chunkSize = config.weightChunkSize ?? 1024 * 1024;

  // Load WASM, weights, tokenizer, and config in parallel
  const [wasmResult, weightsBytes, tokenizer, configResponse] = await Promise.all([
    instantiateWasm(config.wasmUrl),
    streamWeights(config.weightsUrl, chunkSize),
    loadTokenizer(config.tokenizerUrl),
    fetch(config.configUrl).then((r) => {
      if (!r.ok) throw new Error(`Config fetch failed: ${r.status}`);
      return r.text();
    }),
  ]);

  const { exports } = wasmResult;

  // Initialize model with weights and config
  exports.init(weightsBytes, configResponse);

  let disposed = false;

  const session: FalconOcrSession = {
    get ready(): boolean {
      return !disposed;
    },

    async ocr(
      image: ImageData | { width: number; height: number; rgb: Uint8Array },
    ): Promise<OcrResult> {
      if (disposed) {
        throw new Error("FalconOcrSession has been disposed");
      }

      const start = performance.now();

      let rawRgb: { width: number; height: number; rgb: Uint8Array };
      if (image instanceof ImageData) {
        rawRgb = imageDataToRgb(image);
      } else {
        rawRgb = image;
      }

      const aligned = alignToPatch(rawRgb.width, rawRgb.height);
      const paddedRgb = padRgb(
        rawRgb.rgb,
        rawRgb.width,
        rawRgb.height,
        aligned.width,
        aligned.height,
      );

      const tokenIds = exports.ocr_tokens(
        aligned.width,
        aligned.height,
        paddedRgb,
        [],
        maxNewTokens,
      );

      const text = decodeTokens(tokenIds, tokenizer);
      const timeMs = performance.now() - start;

      return {
        text,
        boundingBoxes: [],
        confidence: 0.0,
        timeMs,
        device: "wasm",
        tokenCount: tokenIds.length,
      };
    },

    async ocrTokens(
      width: number,
      height: number,
      rgb: Uint8Array,
      promptIds: number[],
      maxTokens?: number,
    ): Promise<number[]> {
      if (disposed) {
        throw new Error("FalconOcrSession has been disposed");
      }
      return Array.from(
        exports.ocr_tokens(width, height, rgb, promptIds, maxTokens ?? maxNewTokens),
      );
    },

    dispose(): void {
      disposed = true;
    },
  };

  return session;
}
