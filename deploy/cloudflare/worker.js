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

import { handleOcrRequest, handleHealthRequest, handleTokensRequest } from "./ocr_api.js";
import { verifyX402 } from "./x402.js";
import { withMonitoring, createRequestLog, emitLog, writeAnalytics, categorizeError } from "./monitoring.js";
import { createModel } from "./inference-cpu.js";
import { MICRO_MODEL_B64, MICRO_MODEL_CONFIG } from "./micro-model-data.js";

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

/** @type {object | null} */
let cpuModelConfig = null;

/** @type {Uint8Array | null} */
let cpuWeightsBytes = null;

/** @type {import("./inference-cpu.js").FalconOCRMicro | null} */
let cpuModel = null;

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
   */
  init(weights, config) {
    cpuWeightsBytes = weights;
    cpuModelConfig = config;
    cpuModel = createModel(weights.buffer, config);
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
      // Phase 2b (CPU): Try loading the full model from R2 first (paid
      // plan provides sufficient CPU/memory).  Fall back to the embedded
      // micro model only when R2 weights are unavailable.
      let weightsBytes = null;
      let config = null;

      const r2Weights = await env.WEIGHTS.get("models/falcon-ocr/model.safetensors");
      const r2Config = await env.WEIGHTS.get("models/falcon-ocr/config.json");
      if (r2Weights && r2Config) {
        try {
          weightsBytes = new Uint8Array(await r2Weights.arrayBuffer());
          config = JSON.parse(await r2Config.text());
          console.log(`Loaded full model from R2: ${weightsBytes.byteLength} bytes`);
        } catch (err) {
          console.warn(`R2 full model load failed, falling back to micro: ${err.message}`);
          weightsBytes = null;
          config = null;
        }
      }

      if (!weightsBytes) {
        // Fallback: use the embedded micro model (263 KB, no R2 fetch).
        const raw = atob(MICRO_MODEL_B64);
        weightsBytes = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) {
          weightsBytes[i] = raw.charCodeAt(i);
        }
        config = MICRO_MODEL_CONFIG;
      }

      CpuDevice.init(weightsBytes, config);
      activeDevice = "cpu";
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
    "Access-Control-Allow-Headers": "Content-Type, X-Payment-402, X-Request-ID",
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

    return withMonitoring({
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
          return new Response(
            JSON.stringify({
              status: modelReady ? "ready" : initError ? "error" : "loading",
              model: "falcon-ocr",
              version: env.MODEL_VERSION || "0.1.0",
              device,
              request_id: rid,
              backends: {
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
              status: 200,
              headers: { ...cors, "Content-Type": "application/json" },
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
          const response = await handleOcrRequest(request, inferenceBackend, env, cors, rid, activeDevice);
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

        return new Response(
          JSON.stringify({ error: "Not found", request_id: rid }),
          { status: 404, headers: { ...cors, "Content-Type": "application/json" } },
        );
      },
    });
  },
};
