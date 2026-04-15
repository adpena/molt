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
 * Lazy-initialize the WASM module and model weights.
 * Idempotent — concurrent requests share the same init promise.
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
    // 1. Fetch WASM binary from R2
    const wasmObj = await env.WEIGHTS.get("models/falcon-ocr/falcon-ocr.wasm");
    if (!wasmObj) {
      throw new Error("WASM binary not found in R2: models/falcon-ocr/falcon-ocr.wasm");
    }
    const wasmBytes = await wasmObj.arrayBuffer();

    // 2. Compile and instantiate
    const wasmModule = await WebAssembly.compile(wasmBytes);
    wasmInstance = await WebAssembly.instantiate(wasmModule, {
      env: { memory: new WebAssembly.Memory({ initial: 256, maximum: 2048 }) },
    });

    // 3. Fetch weights from R2
    const weightsObj = await env.WEIGHTS.get("models/falcon-ocr/weights.safetensors");
    if (!weightsObj) {
      throw new Error("Weights not found in R2: models/falcon-ocr/weights.safetensors");
    }
    const weightsBytes = new Uint8Array(await weightsObj.arrayBuffer());

    // 4. Fetch config from R2
    const configObj = await env.WEIGHTS.get("models/falcon-ocr/config.json");
    if (!configObj) {
      throw new Error("Config not found in R2: models/falcon-ocr/config.json");
    }
    const configJson = await configObj.text();

    // 5. Initialize the model
    wasmInstance.exports.init(weightsBytes, configJson);
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
        // Reports which backends are available.
        if (path === "/health" && request.method === "GET") {
          return new Response(
            JSON.stringify({
              status: modelReady ? "ready" : initError ? "error" : "loading",
              model: "falcon-ocr",
              version: env.MODEL_VERSION || "0.1.0",
              device: "wasm",
              request_id: rid,
              backends: {
                "molt-gpu": {
                  status: modelReady ? "ready" : initError ? "error" : "loading",
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

        if (path === "/ocr") {
          const response = await handleOcrRequest(request, wasmInstance, env, cors, rid);
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
          const response = await handleTokensRequest(request, wasmInstance, env, cors, rid);
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
