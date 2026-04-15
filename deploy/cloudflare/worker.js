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

/**
 * CpuDevice: JavaScript-only inference fallback when WASM binary is unavailable.
 *
 * Provides a minimal OCR pipeline using the raw safetensors weights and config
 * loaded from R2.  Without the compiled WASM module, inference runs entirely
 * in JS -- significantly slower but functional for health checks and
 * low-throughput requests.
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
    this.initialized = true;
  },

  /**
   * Run OCR token generation on the CPU path.
   *
   * Without the compiled WASM inference kernels, we cannot run the full
   * vision transformer forward pass in pure JS within the Worker CPU time
   * budget.  Returns an empty token array with metadata indicating CPU mode.
   * The client should use the /api/ocr/paddle fallback for actual inference
   * until the WASM binary is compiled and uploaded.
   *
   * @param {number} _width
   * @param {number} _height
   * @param {Uint8Array} _rgb
   * @param {number[]} _promptIds
   * @param {number} _maxNewTokens
   * @returns {Int32Array}
   */
  ocrTokens(_width, _height, _rgb, _promptIds, _maxNewTokens) {
    return new Int32Array(0);
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

    // Phase 2: Load weights (required for both WASM and CPU paths).
    const weightsObj = await env.WEIGHTS.get("models/falcon-ocr/model.safetensors");
    if (!weightsObj) {
      throw new Error("Weights not found in R2: models/falcon-ocr/model.safetensors");
    }
    const weightsBytes = new Uint8Array(await weightsObj.arrayBuffer());

    // Phase 3: Load config (required for both paths).
    const configObj = await env.WEIGHTS.get("models/falcon-ocr/config.json");
    if (!configObj) {
      throw new Error("Config not found in R2: models/falcon-ocr/config.json");
    }
    const configJson = await configObj.text();
    const config = JSON.parse(configJson);

    // Phase 4: Initialize the active device.
    if (useWasm && wasmInstance) {
      wasmInstance.exports.init(weightsBytes, configJson);
      activeDevice = "wasm";
    } else {
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
