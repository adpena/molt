/**
 * Cloudflare Worker entry point for Falcon-OCR inference.
 *
 * Lifecycle:
 *   1. Cold start: fetch WASM module + weights from R2, initialize model.
 *   2. Warm request: run OCR inference on the cached model.
 *
 * The Worker NEVER logs image content (privacy).  Request IDs are
 * generated for debugging without exposing PII.
 */

import { handleOcrRequest, handleHealthRequest, handleTokensRequest } from "./ocr_api.js";

/** @type {WebAssembly.Instance | null} */
let wasmInstance = null;

/** @type {boolean} */
let modelReady = false;

/** @type {Promise<void> | null} */
let initPromise = null;

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
  })();

  try {
    await initPromise;
  } catch (err) {
    initPromise = null;
    throw err;
  }
}

/**
 * Verify x402 payment header.
 *
 * @param {Request} request
 * @param {object} env
 * @returns {{ valid: boolean, error?: string }}
 */
async function verifyX402Payment(request, env) {
  const paymentHeader = request.headers.get("X-Payment-402");
  if (!paymentHeader) {
    return { valid: false, error: "Missing X-Payment-402 header" };
  }

  const walletAddress = env.X402_WALLET_ADDRESS;
  if (!walletAddress) {
    // If no wallet configured, skip payment verification (dev mode).
    return { valid: true };
  }

  const verificationUrl = env.X402_VERIFICATION_URL;
  if (!verificationUrl) {
    return { valid: false, error: "x402 verification endpoint not configured" };
  }

  try {
    const res = await fetch(verificationUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        payment: paymentHeader,
        wallet: walletAddress,
      }),
    });

    if (!res.ok) {
      return { valid: false, error: `x402 verification failed: ${res.status}` };
    }

    const result = /** @type {{ valid: boolean }} */ (await res.json());
    return { valid: result.valid, error: result.valid ? undefined : "Payment invalid" };
  } catch (err) {
    return { valid: false, error: `x402 verification error: ${err.message}` };
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

    // CORS preflight
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    const url = new URL(request.url);
    const path = url.pathname;

    try {
      // Health check — no auth required, no model load required.
      if (path === "/health" && request.method === "GET") {
        return handleHealthRequest(modelReady, env, cors, rid);
      }

      // All other endpoints require POST and x402 payment.
      if (request.method !== "POST") {
        return new Response(
          JSON.stringify({ error: "Method not allowed", request_id: rid }),
          { status: 405, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }

      // Verify payment
      const payment = await verifyX402Payment(request, env);
      if (!payment.valid) {
        return new Response(
          JSON.stringify({ error: payment.error, request_id: rid }),
          { status: 402, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }

      // Ensure model is loaded (lazy init on first request)
      await ensureModelLoaded(env);

      if (path === "/ocr") {
        return handleOcrRequest(request, wasmInstance, env, cors, rid);
      }

      if (path === "/ocr/tokens") {
        return handleTokensRequest(request, wasmInstance, env, cors, rid);
      }

      return new Response(
        JSON.stringify({ error: "Not found", request_id: rid }),
        { status: 404, headers: { ...cors, "Content-Type": "application/json" } },
      );
    } catch (err) {
      // Never expose stack traces in production.  Log the error server-side
      // and return a generic 500 with the request ID for correlation.
      console.error(`[${rid}] Unhandled error:`, err.message);
      return new Response(
        JSON.stringify({
          error: "Internal server error",
          request_id: rid,
        }),
        { status: 500, headers: { ...cors, "Content-Type": "application/json" } },
      );
    }
  },
};
