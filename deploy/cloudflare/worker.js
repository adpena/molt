/**
 * Cloudflare Worker entry point for Falcon-OCR inference.
 *
 * Lifecycle:
 *   1. Cold start: fetch WASM module + weights from R2, initialize model.
 *   2. Warm request: run OCR inference on the cached model.
 *
 * Fallback chain:
 *   molt-gpu (WebGPU/WASM) -> PaddleOCR (server-side JS) -> structured error
 *
 * The Worker NEVER logs image content (privacy).  Request IDs are
 * generated for debugging without exposing PII.
 */

import { handleOcrRequest, handleHealthRequest, handleTokensRequest, handleBatchOcr, handleTemplateExtract, handleStructuredOcr } from "./ocr_api.js";
import { verifyX402 } from "./x402.js";
import { withMonitoring, createRequestLog, emitLog, writeAnalytics, categorizeError } from "./monitoring.js";
// Lazy-loaded: these heavy modules are imported only when local inference is needed.
// This avoids burning CPU budget on cold start when only the Workers AI fast path is used.
let _inferenceModule = null;
let _microModelData = null;
let _matmulWasm = null;
let _simdOpsWasm = null;

async function getInferenceModule() {
  if (!_inferenceModule) _inferenceModule = await import("./inference-cpu.js");
  return _inferenceModule;
}
async function getMicroModelData() {
  if (!_microModelData) _microModelData = await import("./micro-model-data.js");
  return _microModelData;
}
async function getMatmulWasm() {
  if (!_matmulWasm) _matmulWasm = await import("./matmul-wasm-b64.js");
  return _matmulWasm;
}
async function getSimdOpsWasm() {
  if (!_simdOpsWasm) _simdOpsWasm = await import("./simd-ops-b64.js");
  return _simdOpsWasm;
}
import { isWorkersAiAvailable, hybridOcr, runWorkersAiOcr } from "./ai-fallback.js";

/** @type {WebAssembly.Instance | null} */
let wasmInstance = null;

/** @type {boolean} */
let modelReady = false;

/** @type {Promise<void> | null} */
let initPromise = null;

/** @type {string | null} */
let initError = null;

/** @type {"wasm" | "cpu" | "none"} */
let activeDevice = "none";

/** @type {"f32" | "int4" | "int8" | "int8-sharded" | "int4-sharded" | "micro" | "unknown"} */
let activeModelVariant = "unknown";

/** @type {number} */
let loadedShards = 0;

/** @type {number} */
let totalShards = 0;

/** @type {number} */
let loadStartTime = 0;

/** @type {object | null} */
let cpuModelConfig = null;

/** @type {Uint8Array | null} */
let cpuWeightsBytes = null;

/** @type {import("./inference-cpu.js").FalconOCRMicro | null} */
let cpuModel = null;

// ---------------------------------------------------------------------------
// Multi-level caching infrastructure
// ---------------------------------------------------------------------------

/** Cache TTL: 24 hours for OCR results (images don't change). */
const CACHE_TTL_MS = 24 * 60 * 60 * 1000;

/** Edge cache TTL: 1 hour (Cache API). */
const EDGE_CACHE_TTL_S = 3600;

/**
 * Compute a SHA-256 hash of image bytes for cache keying.
 *
 * Uses the Web Crypto API available in Workers runtime.
 *
 * @param {Uint8Array} imageBytes
 * @returns {Promise<string>} hex-encoded hash
 */
async function hashImageBytes(imageBytes) {
  const digest = await crypto.subtle.digest("SHA-256", imageBytes);
  const hashArray = new Uint8Array(digest);
  let hex = "";
  for (let i = 0; i < hashArray.length; i++) {
    hex += hashArray[i].toString(16).padStart(2, "0");
  }
  return hex;
}

/**
 * Check KV store for a cached OCR result.
 *
 * Returns null if no valid cached result exists.
 * Cached entries expire after CACHE_TTL_MS (24 hours).
 *
 * @param {object} env
 * @param {string} imageHash - SHA-256 hex hash of image bytes
 * @returns {Promise<object|null>}
 */
async function getCachedResult(env, imageHash) {
  if (!env.CACHE) return null;
  try {
    const cached = await env.CACHE.get(`ocr:${imageHash}`, "json");
    if (cached && typeof cached.timestamp === "number" &&
        Date.now() - cached.timestamp < CACHE_TTL_MS) {
      return cached;
    }
  } catch (_err) {
    // KV read failure is non-fatal — proceed with inference
  }
  return null;
}

/**
 * Store an OCR result in the KV cache.
 *
 * @param {object} env
 * @param {string} imageHash
 * @param {object} result
 * @param {object} ctx - ExecutionContext for waitUntil
 */
function setCachedResult(env, imageHash, result, ctx) {
  if (!env.CACHE) return;
  const entry = {
    ...result,
    timestamp: Date.now(),
    cached: true,
  };
  // Non-blocking write: use waitUntil so the response is not delayed.
  // KV TTL is set to match our application TTL (86400s = 24h).
  ctx.waitUntil(
    env.CACHE.put(`ocr:${imageHash}`, JSON.stringify(entry), {
      expirationTtl: Math.ceil(CACHE_TTL_MS / 1000),
    }).catch((_err) => {
      // KV write failure is non-fatal
    })
  );
}

/**
 * Try to serve from Cloudflare's edge Cache API.
 *
 * For identical request URLs, the Cache API serves responses from
 * Cloudflare's global edge network without hitting the Worker at all
 * on subsequent requests.
 *
 * @param {Request} request
 * @param {string} imageHash
 * @returns {Promise<Response|null>}
 */
async function getEdgeCachedResponse(request, imageHash) {
  try {
    const cache = caches.default;
    const cacheUrl = new URL(request.url);
    cacheUrl.searchParams.set("_hash", imageHash);
    const cacheKey = new Request(cacheUrl.toString(), { method: "GET" });
    return await cache.match(cacheKey);
  } catch (_err) {
    return null;
  }
}

/**
 * Store a response in Cloudflare's edge Cache API.
 *
 * @param {Request} request
 * @param {string} imageHash
 * @param {Response} response
 * @param {object} ctx
 */
function setEdgeCachedResponse(request, imageHash, response, ctx) {
  try {
    const cache = caches.default;
    const cacheUrl = new URL(request.url);
    cacheUrl.searchParams.set("_hash", imageHash);
    const cacheKey = new Request(cacheUrl.toString(), { method: "GET" });
    const cachedResponse = new Response(response.body, {
      status: response.status,
      headers: new Headers(response.headers),
    });
    cachedResponse.headers.set(
      "Cache-Control",
      `public, max-age=${EDGE_CACHE_TTL_S}`
    );
    cachedResponse.headers.set("X-Cache-Status", "MISS");
    ctx.waitUntil(cache.put(cacheKey, cachedResponse).catch(() => {}));
  } catch (_err) {
    // Edge cache write failure is non-fatal
  }
}

/**
 * CpuDevice: JavaScript-only inference using the real forward pass.
 *
 * Loads SafeTensors weights and runs the full vision transformer in pure JS
 * using Float32Array operations.  Suitable for the micro model (65K params,
 * ~263 KB weights).  For the production 269M-param model, WASM is required.
 */
const CpuDevice = {
  /** @type {boolean} */
  initialized: false,

  /**
   * Initialize the CPU device with weights and config.
   * @param {Uint8Array} weights - Raw safetensors bytes
   * @param {object} config - Model configuration
   * @param {Object<string, number>|null} scales - Per-tensor dequantization scales
   */
  async init(weights, config, scales) {
    cpuWeightsBytes = weights;
    cpuModelConfig = config;
    const inf = await getInferenceModule();
    cpuModel = inf.createModel(weights.buffer, config, scales);
    this.initialized = true;
  },

  /**
   * Run OCR token generation on the CPU path.
   *
   * Runs the full vision transformer forward pass in pure JS.
   * For the micro model (2 layers, dim=32) this completes within
   * the Workers CPU time budget.
   *
   * @param {number} width
   * @param {number} height
   * @param {Uint8Array} rgb
   * @param {number[]} promptIds
   * @param {number} maxNewTokens
   * @returns {Int32Array}
   */
  ocrTokens(width, height, rgb, promptIds, maxNewTokens) {
    if (!cpuModel) {
      return new Int32Array(0);
    }
    return cpuModel.ocrTokens(width, height, rgb, promptIds, maxNewTokens);
  },
};

/**
 * Generate a unique request ID for tracing (no PII).
 * @returns {string}
 */
function requestId() {
  const ts = Date.now().toString(36);
  const rand = Math.random().toString(36).slice(2, 8);
  return `${ts}-${rand}`;
}

/**
 * Lazy-initialize the model.  Tries WASM first, falls back to CPU.
 * Idempotent -- concurrent requests share the same init promise.
 *
 * @param {object} env - Worker environment bindings
 */
async function ensureModelLoaded(env) {
  if (modelReady) return;
  if (initPromise) {
    await initPromise;
    return;
  }

  initPromise = (async () => {
    loadStartTime = Date.now();

    // Lazy-load the inference module once for the entire init sequence.
    const inf = await getInferenceModule();

    // Phase 0: Initialize WASM SIMD kernels.
    // These modules are only imported when local inference is needed (not Workers AI).
    try {
      const matmulMod = await getMatmulWasm();
      if (matmulMod.default) {
        const raw = atob(matmulMod.default);
        const bytes = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
        await inf.initMatmulWasm(bytes.buffer);
        console.log("WASM SIMD matmul kernel initialized");
      }
    } catch (err) {
      console.warn(`WASM matmul init failed: ${err.message}`);
    }

    try {
      const simdMod = await getSimdOpsWasm();
      if (simdMod.SIMD_OPS_WASM) {
        const raw = atob(simdMod.SIMD_OPS_WASM);
        const bytes = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
        await inf.initSimdOps(bytes.buffer);
        console.log("WASM SIMD ops kernel initialized");
      }
    } catch (err) {
      console.warn(`WASM SIMD ops init failed: ${err.message}`);
    }

    // Phase 1: Try loading the WASM binary for full-speed inference.
    let useWasm = false;
    const wasmObj = await env.WEIGHTS.get("models/falcon-ocr/falcon-ocr.wasm");

    if (wasmObj) {
      try {
        const wasmBytes = await wasmObj.arrayBuffer();
        const wasmModule = await WebAssembly.compile(wasmBytes);
        wasmInstance = await WebAssembly.instantiate(wasmModule, {
          env: { memory: new WebAssembly.Memory({ initial: 256, maximum: 2048 }) },
        });
        useWasm = true;
      } catch (err) {
        console.warn(`WASM compilation failed, falling back to CPU: ${err.message}`);
        wasmInstance = null;
      }
    }

    if (useWasm && wasmInstance) {
      // Phase 2a (WASM): Load full production weights from R2.
      const weightsObj = await env.WEIGHTS.get("models/falcon-ocr/model.safetensors");
      if (!weightsObj) {
        throw new Error("Weights not found in R2: models/falcon-ocr/model.safetensors");
      }
      const weightsBytes = new Uint8Array(await weightsObj.arrayBuffer());
      const configObj = await env.WEIGHTS.get("models/falcon-ocr/config.json");
      if (!configObj) {
        throw new Error("Config not found in R2: models/falcon-ocr/config.json");
      }
      const configJson = await configObj.text();
      wasmInstance.exports.init(weightsBytes, configJson);
      activeDevice = "wasm";
    } else {
      // Phase 2b (CPU): Try loading quantized sharded model from R2 first,
      // then single-file INT4, then full F32, then embedded micro model.
      let weightsBytes = null;
      let config = null;
      let scales = null;
      let modelVariant = "unknown";

      // Priority 1: INT8 sharded model (~257 MB across ~6x50 MB shards)
      // INT8 has < 1% accuracy loss vs F32, far better than INT4's 3-10%.
      // Shards are loaded sequentially for progressive loading; the model
      // becomes usable after the embedding shard is loaded.
      const r2Int8ShardIndex = await env.WEIGHTS.get("models/falcon-ocr-int8-sharded/model.safetensors.index.json");
      const r2Int8ShardConfig = await env.WEIGHTS.get("models/falcon-ocr-int8-sharded/config.json");
      const r2Int8ShardScales = await env.WEIGHTS.get("models/falcon-ocr-int8-sharded/scales.json");

      if (r2Int8ShardIndex && r2Int8ShardConfig && r2Int8ShardScales) {
        try {
          const indexJson = JSON.parse(await r2Int8ShardIndex.text());
          config = JSON.parse(await r2Int8ShardConfig.text());
          scales = JSON.parse(await r2Int8ShardScales.text());
          const numShards = indexJson.metadata.num_shards;

          const shardNames = [];
          const seen = new Set();
          for (const shardName of Object.values(indexJson.weight_map)) {
            if (!seen.has(shardName)) {
              seen.add(shardName);
              shardNames.push(shardName);
            }
          }

          console.log(`Loading INT8 sharded model: ${numShards} shards`);
          totalShards = numShards;
          loadedShards = 0;

          const allTensors = new Map();
          let totalBytes = 0;

          for (const shardName of shardNames) {
            const shardObj = await env.WEIGHTS.get(`models/falcon-ocr-int8-sharded/${shardName}`);
            if (!shardObj) {
              throw new Error(`Shard not found in R2: ${shardName}`);
            }
            const shardBuffer = await shardObj.arrayBuffer();
            totalBytes += shardBuffer.byteLength;

            const shardTensors = inf.parseSafetensorsToMap(shardBuffer);
            for (const [name, tensor] of shardTensors) {
              allTensors.set(name, tensor);
            }
            loadedShards++;
            console.log(`  Loaded shard ${loadedShards}/${numShards} ${shardName}: ${shardBuffer.byteLength} bytes, ${shardTensors.size} tensors`);
          }

          cpuModel = inf.createModelFromTensors(allTensors, config, scales);
          CpuDevice.initialized = true;
          activeDevice = "cpu";
          activeModelVariant = "int8-sharded";
          modelReady = true;
          initError = null;
          console.log(`Model variant: int8-sharded, device: cpu, total: ${totalBytes} bytes, ${allTensors.size} tensors`);
          return;
        } catch (err) {
          console.warn(`R2 sharded INT8 model load failed: ${err.message}`);
          config = null;
          scales = null;
        }
      }

      // Priority 2: INT8 single-file model (~257 MB, tight fit on Paid plan)
      const r2Int8Weights = await env.WEIGHTS.get("models/falcon-ocr-int8/model.safetensors");
      const r2Int8Config = await env.WEIGHTS.get("models/falcon-ocr-int8/config.json");
      const r2Int8Scales = await env.WEIGHTS.get("models/falcon-ocr-int8/scales.json");
      if (r2Int8Weights && r2Int8Config && r2Int8Scales) {
        try {
          weightsBytes = new Uint8Array(await r2Int8Weights.arrayBuffer());
          config = JSON.parse(await r2Int8Config.text());
          scales = JSON.parse(await r2Int8Scales.text());
          modelVariant = "int8";
          console.log(`Loaded INT8 quantized model from R2: ${weightsBytes.byteLength} bytes`);
        } catch (err) {
          console.warn(`R2 INT8 model load failed: ${err.message}`);
          weightsBytes = null;
          config = null;
          scales = null;
        }
      }

      // Priority 3: INT4 sharded model (~5x30 MB shards, fits Workers memory)
      // Each shard is loaded individually, tensors extracted, buffer dropped.
      if (!weightsBytes) {
      const r2ShardIndex = await env.WEIGHTS.get("models/falcon-ocr-int4-sharded/model.safetensors.index.json");
      const r2ShardConfig = await env.WEIGHTS.get("models/falcon-ocr-int4-sharded/config.json");
      const r2ShardScales = await env.WEIGHTS.get("models/falcon-ocr-int4-sharded/scales.json");

      if (r2ShardIndex && r2ShardConfig && r2ShardScales) {
        try {
          const indexJson = JSON.parse(await r2ShardIndex.text());
          config = JSON.parse(await r2ShardConfig.text());
          scales = JSON.parse(await r2ShardScales.text());
          const numShards = indexJson.metadata.num_shards;

          const shardNames = [];
          const seen = new Set();
          for (const shardName of Object.values(indexJson.weight_map)) {
            if (!seen.has(shardName)) {
              seen.add(shardName);
              shardNames.push(shardName);
            }
          }

          console.log(`Loading INT4 sharded model: ${numShards} shards`);
          totalShards = numShards;
          loadedShards = 0;

          const allTensors = new Map();
          let totalBytes = 0;

          for (const shardName of shardNames) {
            const shardObj = await env.WEIGHTS.get(`models/falcon-ocr-int4-sharded/${shardName}`);
            if (!shardObj) {
              throw new Error(`Shard not found in R2: ${shardName}`);
            }
            const shardBuffer = await shardObj.arrayBuffer();
            totalBytes += shardBuffer.byteLength;

            const shardTensors = inf.parseSafetensorsToMap(shardBuffer);
            for (const [name, tensor] of shardTensors) {
              allTensors.set(name, tensor);
            }
            loadedShards++;
            console.log(`  Loaded shard ${loadedShards}/${numShards} ${shardName}: ${shardBuffer.byteLength} bytes, ${shardTensors.size} tensors`);
          }

          cpuModel = inf.createModelFromTensors(allTensors, config, scales);
          CpuDevice.initialized = true;
          activeDevice = "cpu";
          activeModelVariant = "int4-sharded";
          modelReady = true;
          initError = null;
          console.log(`Model variant: int4-sharded, device: cpu, total: ${totalBytes} bytes, ${allTensors.size} tensors`);
          return;
        } catch (err) {
          console.warn(`R2 sharded INT4 model load failed: ${err.message}`);
          config = null;
          scales = null;
        }
      }
      }

      // Priority 4: INT4 single-file model (~129 MB, may exceed memory)
      if (!weightsBytes) {
      const r2QuantWeights = await env.WEIGHTS.get("models/falcon-ocr-int4/model.safetensors");
      const r2QuantConfig = await env.WEIGHTS.get("models/falcon-ocr-int4/config.json");
      const r2QuantScales = await env.WEIGHTS.get("models/falcon-ocr-int4/scales.json");
      if (r2QuantWeights && r2QuantConfig && r2QuantScales) {
        try {
          weightsBytes = new Uint8Array(await r2QuantWeights.arrayBuffer());
          config = JSON.parse(await r2QuantConfig.text());
          scales = JSON.parse(await r2QuantScales.text());
          modelVariant = "int4";
          console.log(`Loaded INT4 quantized model from R2: ${weightsBytes.byteLength} bytes`);
        } catch (err) {
          console.warn(`R2 INT4 model load failed: ${err.message}`);
          weightsBytes = null;
          config = null;
          scales = null;
        }
      }
      }

      // Priority 5: Full F32 model (~1 GB, only if memory permits)
      if (!weightsBytes) {
        const r2Weights = await env.WEIGHTS.get("models/falcon-ocr/model.safetensors");
        const r2Config = await env.WEIGHTS.get("models/falcon-ocr/config.json");
        if (r2Weights && r2Config) {
          try {
            weightsBytes = new Uint8Array(await r2Weights.arrayBuffer());
            config = JSON.parse(await r2Config.text());
            modelVariant = "f32";
            console.log(`Loaded full F32 model from R2: ${weightsBytes.byteLength} bytes`);
          } catch (err) {
            console.warn(`R2 full model load failed, falling back to micro: ${err.message}`);
            weightsBytes = null;
            config = null;
          }
        }
      }

      // Priority 6: Embedded micro model (263 KB, no R2 fetch)
      if (!weightsBytes) {
        const _mm = await getMicroModelData(); const raw = atob(_mm.MICRO_MODEL_B64);
        weightsBytes = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) {
          weightsBytes[i] = raw.charCodeAt(i);
        }
        config = _mm.MICRO_MODEL_CONFIG;
        modelVariant = "micro";
      }

      CpuDevice.init(weightsBytes, config, scales);
      activeDevice = "cpu";
      activeModelVariant = modelVariant;
      console.log(`Model variant: ${modelVariant}, device: cpu`);
    }

    modelReady = true;
    initError = null;
  })();

  try {
    await initPromise;
  } catch (err) {
    initPromise = null;
    initError = err.message;
    throw err;
  }
}

/**
 * Build CORS headers restricted to the configured origin.
 *
 * @param {object} env
 * @returns {Record<string, string>}
 */
function corsHeaders(env) {
  const origin = env.CORS_ORIGIN || "https://freeinvoicemaker.app";
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type, X-Payment-402, X-Request-ID, X-Use-Backend, X-Document-Type",
    "Access-Control-Max-Age": "86400",
  };
}

/**
 * Build a fallback error response when the primary backend fails.
 *
 * @param {Error} err
 * @param {string} rid
 * @param {Record<string, string>} cors
 * @returns {Response}
 */
function fallbackErrorResponse(err, rid, cors) {
  const category = categorizeError(err, 503);
  return new Response(
    JSON.stringify({
      error: "Primary OCR backend unavailable",
      error_category: category,
      request_id: rid,
      fallback_available: true,
      fallback_url: "/api/ocr/paddle",
      backends: {
        "molt-gpu": { status: "error", error: err.message.split("\n")[0].slice(0, 200) },
        "paddle-ocr": { status: "available", url: "/api/ocr/paddle" },
      },
    }),
    {
      status: 503,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}

export default {
  /**
   * @param {Request} request
   * @param {object} env
   * @param {object} ctx
   * @returns {Promise<Response>}
   */
  async fetch(request, env, ctx) {
    const rid = request.headers.get("X-Request-ID") || requestId();
    const cors = corsHeaders(env);
    const url = new URL(request.url);
    const path = url.pathname;

    // CORS preflight
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    // Fast path: template extraction can skip local model loading entirely
    // since it only uses Workers AI for section classification.
    if (path === "/template/extract" && request.method === "POST" && isWorkersAiAvailable(env)) {
      // Verify x402 payment
      const payment = await verifyX402(request, env, rid, cors);
      if (!payment.authorized) {
        return payment.response;
      }

      try {
        const ct = request.headers.get("Content-Type") || "";
        let imageBytes = null;
        let extractOptions = {};

        if (ct.includes("application/json")) {
          const body = await request.json();
          if (body.image && typeof body.image === "string") {
            const raw = atob(body.image);
            imageBytes = new Uint8Array(raw.length);
            for (let i = 0; i < raw.length; i++) imageBytes[i] = raw.charCodeAt(i);
            extractOptions = {
              documentType: body.document_type || body.documentType || "invoice",
              preserveLogo: body.preserve_logo !== false,
              detectColors: body.detect_colors !== false,
            };
          }
        }

        if (imageBytes) {
          const result = await handleTemplateExtract(env, imageBytes, extractOptions);
          const responseBody = { ...result, request_id: rid };
          const headers = { ...cors, "Content-Type": "application/json" };
          if (payment.receipt) headers["X-Payment-Receipt"] = payment.receipt;
          return new Response(JSON.stringify(responseBody), { status: 200, headers });
        }
      } catch (err) {
        return new Response(
          JSON.stringify({
            error: `Template extraction failed: ${err.message}`,
            request_id: rid,
          }),
          {
            status: 503,
            headers: { ...cors, "Content-Type": "application/json" },
          },
        );
      }
    }

    // Fast path: structured OCR skips local model loading entirely
    // since it only uses Workers AI for structured extraction.
    if (path === "/ocr/structured" && request.method === "POST" && isWorkersAiAvailable(env)) {
      const payment = await verifyX402(request, env, rid, cors);
      if (!payment.authorized) {
        return payment.response;
      }
      const response = await handleStructuredOcr(request, env, cors, rid);
      if (payment.receipt) {
        const headers = new Headers(response.headers);
        headers.set("X-Payment-Receipt", payment.receipt);
        return new Response(response.body, { status: response.status, headers });
      }
      return response;
    }

    // Fast path: Workers AI is the DEFAULT backend for /ocr when available.
    // Local model loading is too CPU-heavy for Workers (even with Paid plan).
    // Workers AI runs on Cloudflare's GPU fleet — zero CPU cost on this Worker.
    // Only falls through to local model if Workers AI is unavailable AND
    // X-Use-Backend: local is explicitly requested.
    const requestedBackend = request.headers.get("X-Use-Backend");
    if (path === "/ocr" && request.method === "POST" &&
        isWorkersAiAvailable(env) && requestedBackend !== "local") {
      // x402 payment verification (skipped for same-origin browser requests)
      const payment = await verifyX402(request, env, rid, cors);
      if (!payment.authorized) {
        return payment.response;
      }
      try {
        const ct = request.headers.get("Content-Type") || "";
        let imageBytes = null;
        let prompt = null;
        if (ct.includes("application/json")) {
          const body = await request.json();
          if (body.image && typeof body.image === "string") {
            const raw = atob(body.image);
            imageBytes = new Uint8Array(raw.length);
            for (let i = 0; i < raw.length; i++) imageBytes[i] = raw.charCodeAt(i);
            prompt = body.prompt;
          }
        }
        if (imageBytes) {
          const aiResult = await runWorkersAiOcr(env, imageBytes, {
            prompt: prompt || undefined,
            isInvoice: true,
          });
          return new Response(
            JSON.stringify({ ...aiResult, request_id: rid }),
            {
              status: 200,
              headers: { ...cors, "Content-Type": "application/json" },
            },
          );
        }
      } catch (err) {
        return new Response(
          JSON.stringify({
            error: `Workers AI OCR failed: ${err.message}`,
            request_id: rid,
            fallback_available: true,
            fallback_url: "/api/ocr/paddle",
          }),
          {
            status: 503,
            headers: { ...cors, "Content-Type": "application/json" },
          },
        );
      }
    }

    const result = await withMonitoring({
      request,
      rid,
      path,
      env,
      ctx,
      handler: async () => {
        // Health check — no auth required, no model load required.
        // Reports which backends are available and active device.
        if (path === "/health" && request.method === "GET") {
          const device = modelReady ? activeDevice : "none";
          const isLoading = !modelReady && !initError && initPromise;
          const elapsedMs = isLoading && loadStartTime > 0 ? Date.now() - loadStartTime : undefined;
          // Estimate remaining time based on shard progress
          let estimatedRemainingMs;
          if (isLoading && totalShards > 0 && loadedShards > 0) {
            const msPerShard = elapsedMs / loadedShards;
            estimatedRemainingMs = Math.round(msPerShard * (totalShards - loadedShards));
          }

          const status = modelReady ? "ready" : initError ? "error" : "loading";
          const httpStatus = modelReady ? 200 : initError ? 500 : 503;

          return new Response(
            JSON.stringify({
              status,
              model: "falcon-ocr",
              model_variant: modelReady ? activeModelVariant : undefined,
              version: env.MODEL_VERSION || "0.1.0",
              device,
              request_id: rid,
              loading: isLoading ? {
                shards_loaded: loadedShards,
                shards_total: totalShards,
                elapsed_ms: elapsedMs,
                estimated_remaining_ms: estimatedRemainingMs,
              } : undefined,
              workers_ai_available: isWorkersAiAvailable(env),
              cache: {
                kv: !!env.CACHE,
                edge: true,
              },
              backends: {
                "workers-ai": {
                  status: isWorkersAiAvailable(env) ? "available" : "not-bound",
                  note: isWorkersAiAvailable(env)
                    ? "GPU inference via Workers AI (preferred)"
                    : "Add [ai] binding to wrangler.toml for GPU inference",
                },
                "molt-gpu": {
                  status: modelReady ? "ready" : initError ? "error" : "loading",
                  device: modelReady ? device : undefined,
                  error: initError || undefined,
                },
                "paddle-ocr": {
                  status: "available",
                  url: "/api/ocr/paddle",
                },
              },
            }),
            {
              status: httpStatus,
              headers: {
                ...cors,
                "Content-Type": "application/json",
                ...(isLoading && estimatedRemainingMs ? { "Retry-After": String(Math.ceil(estimatedRemainingMs / 1000)) } : {}),
              },
            },
          );
        }

        // All other endpoints require POST
        if (request.method !== "POST") {
          return new Response(
            JSON.stringify({ error: "Method not allowed", request_id: rid }),
            { status: 405, headers: { ...cors, "Content-Type": "application/json" } },
          );
        }

        // Verify x402 payment
        const payment = await verifyX402(request, env, rid, cors);
        if (!payment.authorized) {
          return payment.response;
        }

        // Build payment receipt header for successful payments
        const receiptHeaders = payment.receipt
          ? { "X-Payment-Receipt": payment.receipt }
          : {};

        // Ensure model is loaded (lazy init on first request)
        // On failure, return fallback response instead of 500
        try {
          await ensureModelLoaded(env);
        } catch (err) {
          return fallbackErrorResponse(err, rid, cors);
        }

        // Select the active inference backend.
        const inferenceBackend = (activeDevice === "wasm" && wasmInstance)
          ? wasmInstance
          : CpuDevice;

        if (path === "/ocr") {
          // --- Multi-level cache check ---
          // Extract image bytes early for hashing (re-read in handler if cache miss).
          let imageHash = null;
          try {
            const clonedReq = request.clone();
            const ct = clonedReq.headers.get("Content-Type") || "";
            if (ct.includes("application/json")) {
              const body = await clonedReq.json();
              if (body.image && typeof body.image === "string") {
                const raw = atob(body.image);
                const bytes = new Uint8Array(raw.length);
                for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
                imageHash = await hashImageBytes(bytes);
              }
            } else if (ct.includes("multipart/form-data")) {
              const formData = await clonedReq.formData();
              const file = formData.get("image");
              if (file && (file instanceof File || file instanceof Blob)) {
                const bytes = new Uint8Array(await file.arrayBuffer());
                imageHash = await hashImageBytes(bytes);
              }
            }
          } catch (_err) {
            // Hash computation failure is non-fatal — skip cache
          }

          // Level 1: Edge Cache API (fastest — served from Cloudflare edge)
          if (imageHash) {
            const edgeCached = await getEdgeCachedResponse(request, imageHash);
            if (edgeCached) {
              const headers = new Headers(edgeCached.headers);
              Object.entries(cors).forEach(([k, v]) => headers.set(k, v));
              headers.set("X-Cache-Status", "HIT-EDGE");
              headers.set("X-Request-ID", rid);
              if (payment.receipt) headers.set("X-Payment-Receipt", payment.receipt);
              return new Response(edgeCached.body, {
                status: edgeCached.status,
                headers,
              });
            }
          }

          // Level 2: KV cache (fast — no inference needed)
          if (imageHash) {
            const kvCached = await getCachedResult(env, imageHash);
            if (kvCached) {
              const cachedResponse = new Response(
                JSON.stringify({
                  ...kvCached,
                  request_id: rid,
                  cache: "hit-kv",
                }),
                {
                  status: 200,
                  headers: {
                    ...cors,
                    "Content-Type": "application/json",
                    "X-Cache-Status": "HIT-KV",
                  },
                },
              );
              if (payment.receipt) {
                cachedResponse.headers.set("X-Payment-Receipt", payment.receipt);
              }
              // Promote to edge cache for future requests
              setEdgeCachedResponse(request, imageHash, cachedResponse.clone(), ctx);
              return cachedResponse;
            }
          }

          // Level 3: Compute inference (cache miss)
          const response = await handleOcrRequest(request, inferenceBackend, env, cors, rid, activeDevice);

          // Cache the result for future requests
          if (imageHash && response.status === 200) {
            try {
              const responseClone = response.clone();
              const resultBody = await responseClone.json();
              setCachedResult(env, imageHash, resultBody, ctx);
              // Also store in edge cache
              setEdgeCachedResponse(request, imageHash, response.clone(), ctx);
            } catch (_err) {
              // Cache write failure is non-fatal
            }
          }

          // Attach payment receipt to successful responses
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, {
              status: response.status,
              headers,
            });
          }
          return response;
        }

        if (path === "/ocr/tokens") {
          const response = await handleTokensRequest(request, inferenceBackend, env, cors, rid, activeDevice);
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, {
              status: response.status,
              headers,
            });
          }
          return response;
        }

        // Batch OCR: multiple images in one request
        if (path === "/ocr/batch") {
          const cacheOps = {
            hashBytes: hashImageBytes,
            getCached: (hash) => getCachedResult(env, hash),
            setCached: (hash, result) => setCachedResult(env, hash, result, ctx),
          };
          const response = await handleBatchOcr(request, inferenceBackend, env, cors, rid, activeDevice, cacheOps);
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, {
              status: response.status,
              headers,
            });
          }
          return response;
        }

        // Structured OCR: returns parsed invoice JSON (Workers AI only)
        if (path === "/ocr/structured") {
          const response = await handleStructuredOcr(request, env, cors, rid);
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, {
              status: response.status,
              headers,
            });
          }
          return response;
        }

        // Template extraction: OCR -> section classification -> template
        if (path === "/template/extract") {
          const ct = request.headers.get("Content-Type") || "";
          let imageBytes = null;
          let extractOptions = {};

          if (ct.includes("application/json")) {
            const body = await request.json();
            if (!body.image || typeof body.image !== "string") {
              return new Response(
                JSON.stringify({ error: "Missing 'image' field (base64 string)", request_id: rid }),
                { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
              );
            }
            const raw = atob(body.image);
            imageBytes = new Uint8Array(raw.length);
            for (let i = 0; i < raw.length; i++) imageBytes[i] = raw.charCodeAt(i);
            extractOptions = {
              documentType: body.document_type || body.documentType || "invoice",
              preserveLogo: body.preserve_logo !== false,
              detectColors: body.detect_colors !== false,
            };
          } else if (ct.includes("multipart/form-data")) {
            const formData = await request.formData();
            const file = formData.get("image");
            if (!file || !(file instanceof File || file instanceof Blob)) {
              return new Response(
                JSON.stringify({ error: "Missing 'image' field in multipart form data", request_id: rid }),
                { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
              );
            }
            imageBytes = new Uint8Array(await file.arrayBuffer());
            extractOptions = {
              documentType: formData.get("document_type") || "invoice",
              preserveLogo: formData.get("preserve_logo") !== "false",
              detectColors: formData.get("detect_colors") !== "false",
            };
          } else {
            return new Response(
              JSON.stringify({ error: "Unsupported Content-Type. Use application/json or multipart/form-data", request_id: rid }),
              { status: 415, headers: { ...cors, "Content-Type": "application/json" } },
            );
          }

          const result = await handleTemplateExtract(env, imageBytes, extractOptions);
          const response = new Response(
            JSON.stringify({ ...result, request_id: rid }),
            {
              status: 200,
              headers: { ...cors, "Content-Type": "application/json" },
            },
          );
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, { status: response.status, headers });
          }
          return response;
        }

        return new Response(
          JSON.stringify({ error: "Not found", request_id: rid }),
          { status: 404, headers: { ...cors, "Content-Type": "application/json" } },
        );
      },
    });

    // Warm instance pool: keep the Worker isolate alive after responding.
    // This ensures subsequent requests hit a warm instance with the model
    // already loaded in memory, eliminating cold start latency.
    // The no-op promise resolves immediately but signals to the runtime
    // that this isolate should be kept warm.
    ctx.waitUntil(
      (async () => {
        // Pre-warm model loading if not already started.
        // On warm instances this is a no-op (ensureModelLoaded is idempotent).
        if (!modelReady && !initPromise && !initError) {
          try {
            await ensureModelLoaded(env);
          } catch (_err) {
            // Non-fatal: model will be loaded on next request
          }
        }
      })()
    );

    return result;
  },
};
