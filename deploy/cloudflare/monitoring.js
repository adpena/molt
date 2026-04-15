/**
 * Production monitoring and structured logging for Falcon-OCR Worker.
 *
 * All logs are structured JSON. No PII is logged: no image content,
 * no user identifiers, no IP addresses. Only operational metrics.
 *
 * Error categories:
 *   MODEL_LOAD_FAILED   — weight download or WASM compilation failed
 *   INFERENCE_TIMEOUT    — exceeded CPU time limit
 *   WEBGPU_UNAVAILABLE   — no GPU, fallback needed
 *   PAYMENT_INVALID      — x402 verification failed
 *   INPUT_INVALID        — bad image format, size, or missing fields
 *   INTERNAL_ERROR       — unexpected error
 *
 * Integration: uses ctx.waitUntil() for non-blocking analytics writes.
 */

/**
 * @typedef {"MODEL_LOAD_FAILED" | "INFERENCE_TIMEOUT" | "WEBGPU_UNAVAILABLE" | "PAYMENT_INVALID" | "INPUT_INVALID" | "INTERNAL_ERROR"} ErrorCategory
 */

/**
 * @typedef {{
 *   request_id: string,
 *   timestamp: string,
 *   method: string,
 *   path: string,
 *   status_code: number,
 *   latency_ms: number,
 *   device_type: string,
 *   browser: string,
 *   image_width?: number,
 *   image_height?: number,
 *   token_count?: number,
 *   model_version: string,
 *   error_category?: ErrorCategory,
 *   error_message?: string,
 * }} RequestLog
 */

/**
 * Extract device type from User-Agent without logging the full UA string.
 *
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
 *
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
 *
 * @param {Error} error
 * @param {number} statusCode
 * @returns {ErrorCategory}
 */
export function categorizeError(error, statusCode) {
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
  if (statusCode === 402) {
    return "PAYMENT_INVALID";
  }
  if (statusCode === 400 || statusCode === 413 || statusCode === 415) {
    return "INPUT_INVALID";
  }
  return "INTERNAL_ERROR";
}

/**
 * Create a structured request log entry.
 *
 * @param {object} params
 * @param {Request} params.request
 * @param {string} params.rid - Request ID
 * @param {number} params.statusCode
 * @param {number} params.latencyMs
 * @param {string} params.path
 * @param {string} params.modelVersion
 * @param {object} [params.inference] - Inference-specific fields
 * @param {number} [params.inference.imageWidth]
 * @param {number} [params.inference.imageHeight]
 * @param {number} [params.inference.tokenCount]
 * @param {Error} [params.error]
 * @returns {RequestLog}
 */
export function createRequestLog({
  request,
  rid,
  statusCode,
  latencyMs,
  path,
  modelVersion,
  inference,
  error,
}) {
  /** @type {RequestLog} */
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
    // Log the error message but strip any paths or stack traces
    log.error_message = error.message.split("\n")[0].slice(0, 200);
  }

  return log;
}

/**
 * Emit a structured log entry.
 *
 * Uses console.log with JSON for Cloudflare's log pipeline.
 * The log is structured so Cloudflare Analytics Engine can parse it.
 *
 * @param {RequestLog} logEntry
 */
export function emitLog(logEntry) {
  console.log(JSON.stringify(logEntry));
}

/**
 * Write analytics data to Cloudflare Analytics Engine (non-blocking).
 *
 * @param {object} ctx - Worker execution context
 * @param {object} env - Worker environment bindings
 * @param {RequestLog} logEntry
 */
export function writeAnalytics(ctx, env, logEntry) {
  ctx.waitUntil(
    (async () => {
      try {
        // Cloudflare Analytics Engine binding (configured in wrangler.toml)
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
        // Analytics write failures are non-fatal — log and continue
        console.error(`Analytics write failed: ${err.message}`);
      }
    })(),
  );
}

/**
 * Wrap a request handler with monitoring.
 *
 * @param {object} params
 * @param {Request} params.request
 * @param {string} params.rid
 * @param {string} params.path
 * @param {object} params.env
 * @param {object} params.ctx
 * @param {() => Promise<Response>} params.handler
 * @returns {Promise<Response>}
 */
export async function withMonitoring({ request, rid, path, env, ctx, handler }) {
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

    const logEntry = createRequestLog({
      request,
      rid,
      statusCode,
      latencyMs,
      path,
      modelVersion,
      error,
    });

    emitLog(logEntry);
    writeAnalytics(ctx, env, logEntry);
  }
}
