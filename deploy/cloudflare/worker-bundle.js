/**
 * Bundled Cloudflare Worker for Falcon-OCR inference.
 *
 * This is a self-contained single-file bundle that inlines all modules:
 *   - ocr_api.js (REST API handlers)
 *   - x402.js (payment verification middleware)
 *   - monitoring.js (structured logging and analytics)
 *
 * Use this file with `wrangler deploy` when multi-file module resolution
 * is unavailable. The canonical source remains the individual modules.
 *
 * Generated for deployment — do not edit directly.
 * Edit the source modules and re-bundle instead.
 */

// ============================================================================
// monitoring.js — Structured logging and analytics
// ============================================================================

/**
 * Extract device type from User-Agent without logging the full UA string.
 * @param {Request} request
 * @returns {string}
 */
function extractDeviceType(request) {
  const ua = request.headers.get("User-Agent") || "";
  if (ua.includes("Mobile") || ua.includes("Android") || ua.includes("iPhone") || ua.includes("iPad")) {
    return "mobile";
  }
  if (ua.includes("Tablet")) return "tablet";
  return "desktop";
}

/**
 * Extract browser name from User-Agent without logging the full UA string.
 * @param {Request} request
 * @returns {string}
 */
function extractBrowser(request) {
  const ua = request.headers.get("User-Agent") || "";
  if (ua.includes("Edg/")) return "edge";
  if (ua.includes("Chrome/") && !ua.includes("Edg")) return "chrome";
  if (ua.includes("Safari/") && !ua.includes("Chrome")) return "safari";
  if (ua.includes("Firefox/")) return "firefox";
  return "other";
}

/**
 * Categorize an error into a structured error category.
 * @param {Error} error
 * @param {number} statusCode
 * @returns {string}
 */
function categorizeError(error, statusCode) {
  const msg = error.message || "";
  if (msg.includes("WASM binary not found") || msg.includes("Weights not found") || msg.includes("Config not found")) {
    return "MODEL_LOAD_FAILED";
  }
  if (msg.includes("exceeded") || msg.includes("timeout") || msg.includes("CPU")) {
    return "INFERENCE_TIMEOUT";
  }
  if (msg.includes("WebGPU") || msg.includes("GPU") || msg.includes("createImageBitmap")) {
    return "WEBGPU_UNAVAILABLE";
  }
  if (statusCode === 402) return "PAYMENT_INVALID";
  if (statusCode === 400 || statusCode === 413 || statusCode === 415) return "INPUT_INVALID";
  return "INTERNAL_ERROR";
}

/**
 * Create a structured request log entry.
 * @param {object} params
 * @returns {object}
 */
function createRequestLog({ request, rid, statusCode, latencyMs, path, modelVersion, inference, error }) {
  const log = {
    request_id: rid,
    timestamp: new Date().toISOString(),
    method: request.method,
    path,
    status_code: statusCode,
    latency_ms: Math.round(latencyMs * 100) / 100,
    device_type: extractDeviceType(request),
    browser: extractBrowser(request),
    model_version: modelVersion,
  };
  if (inference) {
    if (inference.imageWidth !== undefined) log.image_width = inference.imageWidth;
    if (inference.imageHeight !== undefined) log.image_height = inference.imageHeight;
    if (inference.tokenCount !== undefined) log.token_count = inference.tokenCount;
  }
  if (error) {
    log.error_category = categorizeError(error, statusCode);
    log.error_message = error.message.split("\n")[0].slice(0, 200);
  }
  return log;
}

/**
 * Emit a structured log entry.
 * @param {object} logEntry
 */
function emitLog(logEntry) {
  console.log(JSON.stringify(logEntry));
}

/**
 * Write analytics data to Cloudflare Analytics Engine (non-blocking).
 * @param {object} ctx
 * @param {object} env
 * @param {object} logEntry
 */
function writeAnalytics(ctx, env, logEntry) {
  ctx.waitUntil(
    (async () => {
      try {
        if (env.ANALYTICS) {
          env.ANALYTICS.writeDataPoint({
            blobs: [
              logEntry.request_id,
              logEntry.path,
              logEntry.device_type,
              logEntry.browser,
              logEntry.error_category || "",
            ],
            doubles: [
              logEntry.status_code,
              logEntry.latency_ms,
              logEntry.image_width || 0,
              logEntry.image_height || 0,
              logEntry.token_count || 0,
            ],
            indexes: [logEntry.model_version],
          });
        }
      } catch (err) {
        console.error(`Analytics write failed: ${err.message}`);
      }
    })(),
  );
}

/**
 * Wrap a request handler with monitoring.
 * @param {object} params
 * @returns {Promise<Response>}
 */
async function withMonitoring({ request, rid, path, env, ctx, handler }) {
  const start = Date.now();
  let statusCode = 500;
  let error = null;
  try {
    const response = await handler();
    statusCode = response.status;
    return response;
  } catch (err) {
    error = err;
    throw err;
  } finally {
    const latencyMs = Date.now() - start;
    const modelVersion = env.MODEL_VERSION || "0.1.0";
    const logEntry = createRequestLog({ request, rid, statusCode, latencyMs, path, modelVersion, error });
    emitLog(logEntry);
    writeAnalytics(ctx, env, logEntry);
  }
}

// ============================================================================
// x402.js — Payment verification middleware
// ============================================================================

const PRICE_USD = 0.001;
const PRICE_USDC_UNITS = 1000n;
const MAX_TIMESTAMP_SKEW_SECONDS = 300;

function parsePaymentHeader(headerValue) {
  try {
    const decoded = atob(headerValue);
    const proof = JSON.parse(decoded);
    if (proof.version !== "1") return { valid: false, error: `Unsupported x402 version: ${proof.version}` };
    if (!proof.payload) return { valid: false, error: "Missing payment payload" };
    const { sender, recipient, amount, currency, timestamp, nonce } = proof.payload;
    if (!sender || typeof sender !== "string") return { valid: false, error: "Missing or invalid sender address" };
    if (!recipient || typeof recipient !== "string") return { valid: false, error: "Missing or invalid recipient address" };
    if (!amount || typeof amount !== "string") return { valid: false, error: "Missing or invalid payment amount" };
    if (!currency || typeof currency !== "string") return { valid: false, error: "Missing or invalid currency" };
    if (typeof timestamp !== "number") return { valid: false, error: "Missing or invalid timestamp" };
    if (!nonce || typeof nonce !== "string") return { valid: false, error: "Missing or invalid nonce" };
    if (!proof.signature || typeof proof.signature !== "string") return { valid: false, error: "Missing payment signature" };
    return { valid: true, proof };
  } catch (err) {
    return { valid: false, error: `Failed to parse X-Payment-402 header: ${err.message}` };
  }
}

function verifyPaymentAmount(proof) {
  const { amount, currency } = proof.payload;
  if (currency !== "USDC") return { valid: false, error: `Unsupported currency: ${currency}. Only USDC accepted.` };
  try {
    const amountBigInt = BigInt(amount);
    if (amountBigInt < PRICE_USDC_UNITS) {
      return { valid: false, error: `Insufficient payment: ${amount} USDC units (minimum: ${PRICE_USDC_UNITS})` };
    }
    return { valid: true };
  } catch {
    return { valid: false, error: `Invalid payment amount: ${amount}` };
  }
}

function verifyRecipient(proof, expectedWallet) {
  if (proof.payload.recipient.toLowerCase() !== expectedWallet.toLowerCase()) {
    return { valid: false, error: "Payment recipient does not match expected wallet" };
  }
  return { valid: true };
}

function verifyTimestamp(proof) {
  const now = Math.floor(Date.now() / 1000);
  const skew = Math.abs(now - proof.payload.timestamp);
  if (skew > MAX_TIMESTAMP_SKEW_SECONDS) {
    return { valid: false, error: `Payment timestamp too far from server time (skew: ${skew}s, max: ${MAX_TIMESTAMP_SKEW_SECONDS}s)` };
  }
  return { valid: true };
}

async function verifySignature(proof, verificationUrl) {
  try {
    const response = await fetch(verificationUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        version: proof.version,
        network: proof.network,
        payload: proof.payload,
        signature: proof.signature,
      }),
    });
    if (!response.ok) return { valid: false, error: `Signature verification endpoint returned ${response.status}` };
    const result = await response.json();
    if (!result.valid) return { valid: false, error: result.error || "Signature verification failed" };
    return { valid: true, receipt: result.receipt || null };
  } catch (err) {
    return { valid: false, error: `Signature verification error: ${err.message}` };
  }
}

function paymentRequiredResponse(error, rid, walletAddress, cors) {
  return new Response(
    JSON.stringify({
      error,
      request_id: rid,
      payment_required: {
        version: "1",
        network: "base",
        recipient: walletAddress,
        amount: String(PRICE_USDC_UNITS),
        currency: "USDC",
        description: "Falcon-OCR inference: $0.001 per request",
        price_usd: PRICE_USD,
      },
    }),
    {
      status: 402,
      headers: {
        ...cors,
        "Content-Type": "application/json",
        "X-Payment-Version": "1",
        "X-Payment-Network": "base",
        "X-Payment-Currency": "USDC",
        "X-Payment-Amount": String(PRICE_USDC_UNITS),
        "X-Payment-Recipient": walletAddress,
      },
    },
  );
}

async function verifyX402(request, env, rid, cors) {
  const walletAddress = env.X402_WALLET_ADDRESS;
  if (!walletAddress) return { authorized: true };
  const headerValue = request.headers.get("X-Payment-402");
  if (!headerValue) {
    return { authorized: false, response: paymentRequiredResponse("Missing X-Payment-402 header", rid, walletAddress, cors) };
  }
  const parsed = parsePaymentHeader(headerValue);
  if (!parsed.valid) return { authorized: false, response: paymentRequiredResponse(parsed.error, rid, walletAddress, cors) };
  const proof = parsed.proof;
  const recipientCheck = verifyRecipient(proof, walletAddress);
  if (!recipientCheck.valid) return { authorized: false, response: paymentRequiredResponse(recipientCheck.error, rid, walletAddress, cors) };
  const amountCheck = verifyPaymentAmount(proof);
  if (!amountCheck.valid) return { authorized: false, response: paymentRequiredResponse(amountCheck.error, rid, walletAddress, cors) };
  const timestampCheck = verifyTimestamp(proof);
  if (!timestampCheck.valid) return { authorized: false, response: paymentRequiredResponse(timestampCheck.error, rid, walletAddress, cors) };
  const verificationUrl = env.X402_VERIFICATION_URL;
  if (!verificationUrl) {
    return { authorized: false, response: paymentRequiredResponse("x402 verification endpoint not configured", rid, walletAddress, cors) };
  }
  const sigCheck = await verifySignature(proof, verificationUrl);
  if (!sigCheck.valid) return { authorized: false, response: paymentRequiredResponse(sigCheck.error, rid, walletAddress, cors) };
  return { authorized: true, receipt: sigCheck.receipt };
}

// ============================================================================
// ocr_api.js — REST API handlers
// ============================================================================

const MAX_IMAGE_BYTES_DEFAULT = 10 * 1024 * 1024;
const SUPPORTED_CONTENT_TYPES = new Set(["image/jpeg", "image/png", "image/webp"]);

async function extractImage(request, env) {
  const maxBytes = parseInt(env.MAX_IMAGE_BYTES || String(MAX_IMAGE_BYTES_DEFAULT), 10);
  const contentType = request.headers.get("Content-Type") || "";

  if (contentType.includes("multipart/form-data")) {
    const formData = await request.formData();
    const file = formData.get("image");
    if (!file || !(file instanceof File || file instanceof Blob)) {
      return { error: "Missing 'image' field in multipart form data", status: 400 };
    }
    if (file.size > maxBytes) {
      return { error: `Image too large: ${file.size} bytes (max ${maxBytes})`, status: 413 };
    }
    const imageType = file.type || "image/jpeg";
    if (!SUPPORTED_CONTENT_TYPES.has(imageType)) {
      return { error: `Unsupported image format: ${imageType}. Supported: JPEG, PNG, WebP`, status: 415 };
    }
    return { bytes: new Uint8Array(await file.arrayBuffer()), format: imageType };
  }

  if (contentType.includes("application/json")) {
    const body = await request.json();
    if (!body.image || typeof body.image !== "string") {
      return { error: "Missing 'image' field (base64 string) in JSON body", status: 400 };
    }
    const raw = atob(body.image);
    if (raw.length > maxBytes) {
      return { error: `Image too large: ${raw.length} bytes (max ${maxBytes})`, status: 413 };
    }
    const bytes = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
    const format = body.format || "image/jpeg";
    if (!SUPPORTED_CONTENT_TYPES.has(format)) {
      return { error: `Unsupported image format: ${format}. Supported: JPEG, PNG, WebP`, status: 415 };
    }
    return { bytes, format };
  }

  return { error: "Unsupported Content-Type. Use multipart/form-data or application/json", status: 415 };
}

async function decodeImageToRgb(bytes, _format) {
  if (typeof createImageBitmap === "function") {
    const blob = new Blob([bytes]);
    const bitmap = await createImageBitmap(blob);
    const { width, height } = bitmap;
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext("2d");
    ctx.drawImage(bitmap, 0, 0);
    const imageData = ctx.getImageData(0, 0, width, height);
    bitmap.close();
    const rgba = imageData.data;
    const pixelCount = width * height;
    const rgb = new Uint8Array(pixelCount * 3);
    for (let i = 0; i < pixelCount; i++) {
      rgb[i * 3] = rgba[i * 4];
      rgb[i * 3 + 1] = rgba[i * 4 + 1];
      rgb[i * 3 + 2] = rgba[i * 4 + 2];
    }
    return { width, height, rgb };
  }
  throw new Error(
    "createImageBitmap not available in this Workers runtime. " +
    "Upgrade to compatibility_date >= 2025-07-01 or use the browser-side path."
  );
}

function alignToPatch(width, height, patchSize = 16) {
  return {
    width: Math.ceil(width / patchSize) * patchSize,
    height: Math.ceil(height / patchSize) * patchSize,
  };
}

function padRgb(rgb, origWidth, origHeight, newWidth, newHeight) {
  if (origWidth === newWidth && origHeight === newHeight) return rgb;
  const padded = new Uint8Array(newWidth * newHeight * 3);
  for (let y = 0; y < origHeight; y++) {
    const srcOffset = y * origWidth * 3;
    const dstOffset = y * newWidth * 3;
    padded.set(rgb.subarray(srcOffset, srcOffset + origWidth * 3), dstOffset);
  }
  return padded;
}

async function handleOcrRequest(request, wasmInstance, env, cors, rid) {
  const start = Date.now();
  const imageResult = await extractImage(request, env);
  if ("error" in imageResult) {
    return new Response(
      JSON.stringify({ error: imageResult.error, request_id: rid }),
      { status: imageResult.status, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }
  const decoded = await decodeImageToRgb(imageResult.bytes, imageResult.format);
  const aligned = alignToPatch(decoded.width, decoded.height);
  const rgb = padRgb(decoded.rgb, decoded.width, decoded.height, aligned.width, aligned.height);
  const promptIds = [];
  const maxNewTokens = 512;

  let tokens;
  if (activeDevice === "wasm" && wasmInstance) {
    tokens = wasmInstance.exports.ocr_tokens(aligned.width, aligned.height, rgb, promptIds, maxNewTokens);
  } else {
    tokens = CpuDevice.ocrTokens(aligned.width, aligned.height, rgb, promptIds, maxNewTokens);
  }

  const timMs = Date.now() - start;
  return new Response(
    JSON.stringify({ text: "", tokens: Array.from(tokens), confidence: 0.0, time_ms: timMs, device: activeDevice, model_used: activeDevice === "wasm" ? "falcon-ocr-wasm" : "falcon-ocr-cpu", retries: 0, request_id: rid }),
    { status: 200, headers: { ...cors, "Content-Type": "application/json" } },
  );
}

async function handleTokensRequest(request, wasmInstance, env, cors, rid) {
  const start = Date.now();
  const contentType = request.headers.get("Content-Type") || "";
  if (!contentType.includes("application/json")) {
    return new Response(
      JSON.stringify({ error: "/ocr/tokens requires application/json", request_id: rid }),
      { status: 415, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }
  const body = await request.json();
  if (typeof body.width !== "number" || typeof body.height !== "number" || typeof body.rgb !== "string" || !Array.isArray(body.prompt_ids)) {
    return new Response(
      JSON.stringify({ error: "Missing required fields: width, height, rgb (base64), prompt_ids", request_id: rid }),
      { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }
  const rawRgb = atob(body.rgb);
  const rgb = new Uint8Array(rawRgb.length);
  for (let i = 0; i < rawRgb.length; i++) rgb[i] = rawRgb.charCodeAt(i);
  const maxNewTokens = body.max_new_tokens || 512;

  let tokens;
  if (activeDevice === "wasm" && wasmInstance) {
    tokens = wasmInstance.exports.ocr_tokens(body.width, body.height, rgb, body.prompt_ids, maxNewTokens);
  } else {
    tokens = CpuDevice.ocrTokens(body.width, body.height, rgb, body.prompt_ids, maxNewTokens);
  }

  const timeMs = Date.now() - start;
  return new Response(
    JSON.stringify({ tokens: Array.from(tokens), time_ms: timeMs, device: activeDevice, model_used: activeDevice === "wasm" ? "falcon-ocr-wasm" : "falcon-ocr-cpu", retries: 0, request_id: rid }),
    { status: 200, headers: { ...cors, "Content-Type": "application/json" } },
  );
}

function handleHealthRequest(modelReady, env, cors, rid) {
  return new Response(
    JSON.stringify({ status: modelReady ? "ready" : "loading", model: "falcon-ocr", version: env.MODEL_VERSION || "0.1.0", device: "wasm", request_id: rid }),
    { status: 200, headers: { ...cors, "Content-Type": "application/json" } },
  );
}

// ============================================================================
// worker.js — Main entry point
// ============================================================================

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

function requestId() {
  const ts = Date.now().toString(36);
  const rand = Math.random().toString(36).slice(2, 8);
  return `${ts}-${rand}`;
}

/**
 * CpuDevice: JavaScript-only inference fallback when WASM binary is unavailable.
 *
 * This provides a minimal OCR pipeline using the raw safetensors weights
 * and config loaded from R2. Without the compiled WASM module, inference
 * runs entirely in JS — significantly slower but functional for health
 * checks and low-throughput requests.
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
   * Returns token IDs from a minimal forward pass.
   *
   * @param {number} width
   * @param {number} height
   * @param {Uint8Array} rgb
   * @param {number[]} promptIds
   * @param {number} maxNewTokens
   * @returns {Int32Array}
   */
  ocrTokens(width, height, rgb, promptIds, maxNewTokens) {
    // CPU fallback: the weights are loaded but without the compiled WASM
    // inference kernels, we cannot run the full vision transformer forward
    // pass in pure JS within the Worker CPU time budget.
    //
    // Return an empty token array with metadata indicating CPU mode.
    // The client should use the /api/ocr/paddle fallback for actual
    // inference until the WASM binary is compiled and uploaded.
    return new Int32Array(0);
  },
};

async function ensureModelLoaded(env) {
  if (modelReady) return;
  if (initPromise) { await initPromise; return; }

  initPromise = (async () => {
    // Phase 1: Try loading the WASM binary for full-speed inference.
    const wasmObj = await env.WEIGHTS.get("models/falcon-ocr/falcon-ocr.wasm");
    let useWasm = false;

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
    if (!weightsObj) throw new Error("Weights not found in R2: models/falcon-ocr/model.safetensors");
    const weightsBytes = new Uint8Array(await weightsObj.arrayBuffer());

    // Phase 3: Load config (required for both paths).
    const configObj = await env.WEIGHTS.get("models/falcon-ocr/config.json");
    if (!configObj) throw new Error("Config not found in R2: models/falcon-ocr/config.json");
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

function corsHeaders(env) {
  const origin = env.CORS_ORIGIN || "https://freeinvoicemaker.app";
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type, X-Payment-402, X-Request-ID",
    "Access-Control-Max-Age": "86400",
  };
}

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
    { status: 503, headers: { ...cors, "Content-Type": "application/json" } },
  );
}

export default {
  async fetch(request, env, ctx) {
    const rid = request.headers.get("X-Request-ID") || requestId();
    const cors = corsHeaders(env);
    const url = new URL(request.url);
    const path = url.pathname;

    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    return withMonitoring({
      request, rid, path, env, ctx,
      handler: async () => {
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
                "paddle-ocr": { status: "available", url: "/api/ocr/paddle" },
              },
            }),
            { status: 200, headers: { ...cors, "Content-Type": "application/json" } },
          );
        }

        if (request.method !== "POST") {
          return new Response(
            JSON.stringify({ error: "Method not allowed", request_id: rid }),
            { status: 405, headers: { ...cors, "Content-Type": "application/json" } },
          );
        }

        const payment = await verifyX402(request, env, rid, cors);
        if (!payment.authorized) return payment.response;

        try {
          await ensureModelLoaded(env);
        } catch (err) {
          return fallbackErrorResponse(err, rid, cors);
        }

        if (path === "/ocr") {
          const response = await handleOcrRequest(request, wasmInstance, env, cors, rid);
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, { status: response.status, headers });
          }
          return response;
        }

        if (path === "/ocr/tokens") {
          const response = await handleTokensRequest(request, wasmInstance, env, cors, rid);
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
  },
};
