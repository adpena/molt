/**
 * Cloudflare Worker entry point for OCR and document processing.
 *
 * OCR Engine Architecture:
 *   RECOMMENDED: configured browser-side OCR path (client-side)
 *   EXPERIMENTAL: Falcon-OCR (edge inference, INT4 CPU, degraded quality)
 *   PLANNED: Nemotron v2 through an explicitly configured GPU service endpoint
 *
 * The default /ocr endpoint returns 503 directing clients to PaddleOCR
 * (which runs client-side — no server round-trip). Falcon-OCR edge
 * inference is opt-in via X-Use-Backend: falcon-ocr header.
 *
 * Workers AI endpoints (server-side, no OCR):
 *   - /invoice/fill (NL template filling)
 *   - /template/extract (layout analysis)
 *   - /ocr/structured (structured extraction from pre-OCR'd text)
 *   - /ocr/detailed (block-level output)
 *   - /ocr/table (table extraction)
 *
 * Workers AI is NOT used for OCR text extraction — it hallucinates content.
 *
 * The Worker NEVER logs image content (privacy).  Request IDs are
 * generated for debugging without exposing PII.
 */

import { handleOcrRequest, handleTokensRequest, handleBatchOcr, handleStructuredOcr, handleDetailedOcr, handleTableOcr, handleTemplateExtractFast, handleInvoiceFill, normalizeOcrResultPayload } from "./ocr_api.js";
import { verifyX402 } from "./x402.js";
import { withMonitoring, categorizeError } from "./monitoring.js";
import { getAnalyticsSummary } from "./analytics.js";
import { TokenizerDecoder } from "./tokenizer.js";
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
import { isWorkersAiAvailable } from "./ai-fallback.js";
import { proxyToGPU, gpuInferenceStatus } from "./gpu-proxy.js";

// ---------------------------------------------------------------------------
// Embedded browser test page HTML (serves at /test)
// JS imports rewritten from relative to /browser/ for R2 serving.
// ---------------------------------------------------------------------------
const TEST_HTML = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Falcon-OCR WebGPU Inference Test</title>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, monospace;
            max-width: 800px;
            margin: 0 auto;
            padding: 24px;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { margin-bottom: 16px; font-size: 1.4em; }
        #status {
            padding: 12px;
            background: #1a1a2e;
            border-left: 3px solid #4a9eff;
            margin-bottom: 16px;
            font-size: 0.9em;
        }
        #gpu-info {
            padding: 8px 12px;
            background: #1a2e1a;
            border-left: 3px solid #4aff4a;
            margin-bottom: 16px;
            font-size: 0.85em;
            white-space: pre-wrap;
        }
        #gpu-info:empty { display: none; }
        .controls {
            display: flex;
            gap: 12px;
            align-items: center;
            margin-bottom: 16px;
        }
        input[type="file"] {
            background: #222;
            color: #e0e0e0;
            padding: 8px;
            border: 1px solid #444;
            border-radius: 4px;
            cursor: pointer;
        }
        canvas {
            border: 1px solid #333;
            background: #111;
            display: block;
            margin-bottom: 16px;
        }
        #output {
            background: #111;
            border: 1px solid #333;
            padding: 16px;
            min-height: 80px;
            white-space: pre-wrap;
            word-break: break-word;
            font-size: 0.9em;
            line-height: 1.5;
            margin-bottom: 16px;
        }
        #output:empty::before {
            content: 'OCR output will appear here...';
            color: #555;
        }
        #timing {
            font-size: 0.85em;
            color: #888;
        }
        #timing:empty { display: none; }
        .error { border-left-color: #ff4a4a !important; }
        .progress-bar {
            width: 100%;
            height: 4px;
            background: #222;
            margin-bottom: 16px;
            border-radius: 2px;
            overflow: hidden;
        }
        .progress-bar-fill {
            height: 100%;
            background: #4a9eff;
            width: 0%;
            transition: width 0.2s ease;
        }
        #speculative-stats {
            font-size: 0.85em;
            color: #888;
            margin-bottom: 16px;
        }
        #speculative-stats:empty { display: none; }
        label {
            font-size: 0.85em;
            display: flex;
            align-items: center;
            gap: 6px;
            cursor: pointer;
        }
    </style>
</head>
<body>
    <h1>Falcon-OCR WebGPU Test</h1>

    <div class="progress-bar"><div class="progress-bar-fill" id="progress-fill"></div></div>
    <div id="status">Initializing...</div>
    <div id="gpu-info"></div>

    <div class="controls">
        <input type="file" id="image-input" accept="image/*" disabled>
        <label>
            <input type="checkbox" id="speculative-toggle">
            Speculative decoding
        </label>
    </div>

    <canvas id="preview" width="224" height="224"></canvas>
    <pre id="output"></pre>
    <div id="timing"></div>
    <div id="speculative-stats"></div>

    <script type="module">
        import { FalconOCR } from '/browser/falcon-ocr-loader.js';

        const status = document.getElementById('status');
        const gpuInfo = document.getElementById('gpu-info');
        const output = document.getElementById('output');
        const timing = document.getElementById('timing');
        const progressFill = document.getElementById('progress-fill');
        const imageInput = document.getElementById('image-input');
        const speculativeToggle = document.getElementById('speculative-toggle');
        const speculativeStats = document.getElementById('speculative-stats');
        const canvas = document.getElementById('preview');
        const ctx = canvas.getContext('2d');

        let speculativeDecoder = null;

        const ocr = new FalconOCR({
            wasmUrl: 'https://falcon-ocr.adpena.workers.dev/wasm/falcon-ocr.wasm',
            weightsVariant: 'falcon-ocr-int8',
            onProgress: (phase, pct, detail) => {
                const msg = detail?.message || detail?.phase || '';
                status.textContent = \`[\${phase}] \${pct}% \${msg}\`;
                progressFill.style.width = \`\${pct}%\`;

                if (phase === 'detecting') progressFill.style.background = '#ffaa4a';
                else if (phase === 'wasm') progressFill.style.background = '#4a9eff';
                else if (phase === 'weights') progressFill.style.background = '#9e4aff';
                else if (phase === 'gpu') progressFill.style.background = '#4aff4a';
                else if (phase === 'init') progressFill.style.background = '#4aff9e';
            },
        });

        try {
            await ocr.init();

            // Expose for headless automation (Browser Rendering GPU inference)
            window.__falconOCR = { ready: true, recognize: (img) => ocr.recognize(img) };

            const backend = ocr.computeBackend;
            gpuInfo.textContent = \`Compute backend: \${backend}\`;

            if (backend === 'webgpu') {
                gpuInfo.textContent += '\\nWebGPU active — tiled matmul via compute shaders';
                speculativeToggle.disabled = false;
            } else if (backend === 'webgl2') {
                gpuInfo.textContent += '\\nWebGL2 active — texture-pass matmul';
                speculativeToggle.disabled = false;
            } else {
                gpuInfo.textContent += \`\\n\${backend} — CPU fallback\`;
                speculativeToggle.disabled = true;
            }

            if (backend === 'webgpu' || backend === 'webgl2') {
                try {
                    const { SpeculativeBrowserDecoder } = await import('/browser/speculative-browser.js');
                    window._SpeculativeBrowserDecoder = SpeculativeBrowserDecoder;
                    gpuInfo.textContent += '\\nSpeculative decoding available (toggle to enable)';
                } catch (err) {
                    gpuInfo.textContent += \`\\nSpeculative decoding unavailable: \${err.message}\`;
                    speculativeToggle.disabled = true;
                }
            }

            progressFill.style.width = '100%';
            progressFill.style.background = '#4aff4a';
            status.textContent = 'Ready — select an image';
            imageInput.disabled = false;
        } catch (e) {
            status.textContent = \`Error: \${e.message}\`;
            status.classList.add('error');
            progressFill.style.background = '#ff4a4a';
            progressFill.style.width = '100%';
            console.error('Falcon-OCR init failed:', e);
        }

        imageInput.addEventListener('change', async (e) => {
            const file = e.target.files[0];
            if (!file) return;

            output.textContent = '';
            timing.textContent = '';
            speculativeStats.textContent = '';
            status.textContent = 'Running OCR...';

            try {
                const imageBitmap = await createImageBitmap(file);
                const scale = Math.min(224 / imageBitmap.width, 224 / imageBitmap.height);
                const w = Math.round(imageBitmap.width * scale);
                const h = Math.round(imageBitmap.height * scale);
                canvas.width = w;
                canvas.height = h;
                ctx.drawImage(imageBitmap, 0, 0, w, h);
                imageBitmap.close();
                const imageData = ctx.getImageData(0, 0, w, h);

                const start = performance.now();
                const result = await ocr.recognize(imageData);
                const elapsed = performance.now() - start;

                output.textContent = result.text;
                timing.textContent = [
                    \`Inference: \${result.timeMs.toFixed(1)}ms\`,
                    \`Total (incl. preprocessing): \${elapsed.toFixed(1)}ms\`,
                    \`Backend: \${result.backend}\`,
                    \`Tokens: \${result.tokenIds.length}\`,
                ].join('\\n');

                status.textContent = 'Done';
            } catch (err) {
                output.textContent = \`Error: \${err.message}\`;
                status.textContent = 'Error during inference';
                status.classList.add('error');
                console.error('OCR inference failed:', err);
            }
        });
    </script>
</body>
</html>`;

// ---------------------------------------------------------------------------
// Embedded dashboard HTML (serves at /dashboard)
// Revenue calculator and system status overview.
// ---------------------------------------------------------------------------
const DASHBOARD_HTML = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Falcon-OCR Dashboard</title>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, monospace;
            max-width: 900px;
            margin: 0 auto;
            padding: 24px;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { margin-bottom: 24px; font-size: 1.4em; }
        h2 { margin-bottom: 12px; font-size: 1.1em; color: #4a9eff; }
        .card {
            background: #111;
            border: 1px solid #333;
            border-radius: 8px;
            padding: 20px;
            margin-bottom: 20px;
        }
        .metric {
            display: flex;
            justify-content: space-between;
            padding: 8px 0;
            border-bottom: 1px solid #222;
            font-size: 0.9em;
        }
        .metric:last-child { border-bottom: none; }
        .status-ok { color: #4aff4a; font-weight: bold; }
        .status-warn { color: #ffaa4a; }
        .status-err { color: #ff4a4a; }
        #health-status { font-size: 0.85em; color: #888; margin-top: 8px; white-space: pre-wrap; word-break: break-word; }
        @media (max-width: 600px) {
            body { padding: 12px; }
            h1 { font-size: 1.2em; margin-bottom: 16px; }
            .card { padding: 14px; }
            .metric {
                flex-direction: column;
                gap: 4px;
                padding: 10px 0;
            }
            .metric span:last-child { font-size: 0.85em; }
            #daily-req { width: 80px !important; font-size: 16px !important; padding: 6px 8px !important; }
        }
    </style>
</head>
<body>
    <h1>Falcon-OCR Dashboard</h1>

    <div class="card">
        <h2>Revenue Calculator</h2>
        <div class="metric"><span>x402 price per OCR</span><span>$0.001</span></div>
        <div class="metric"><span>GPU service cost per OCR</span><span>configured externally</span></div>
        <div class="metric"><span>Margin per request</span><span class="status-warn">not estimated</span></div>
        <div class="metric"><span>Daily requests</span><span><input id="daily-req" type="number" value="100" style="width:72px;background:#222;color:#fff;border:1px solid #444;padding:6px 8px;text-align:right;font-size:16px;border-radius:4px;min-height:44px" inputmode="numeric"> </span></div>
        <div class="metric"><span>Monthly revenue</span><span class="status-ok" id="monthly-rev">$3.00</span></div>
        <div class="metric"><span>Monthly cost</span><span id="monthly-cost">configured externally</span></div>
        <div class="metric"><span>Monthly profit</span><span class="status-warn" id="monthly-profit">not estimated</span></div>
    </div>

    <div class="card">
        <h2>System Status</h2>
        <div id="health-status">Loading...</div>
    </div>

    <div class="card">
        <h2>Architecture</h2>
        <div class="metric"><span>Browser users (Origin header)</span><span>Free WebGPU inference (client-side)</span></div>
        <div class="metric"><span>API agents (no Origin)</span><span>x402 payment -> configured GPU service</span></div>
        <div class="metric"><span>GPU fallback</span><span>Edge CPU inference (INT8)</span></div>
        <div class="metric"><span>GPU provider</span><span>See /health configuration</span></div>
    </div>

    <script>
    document.getElementById('daily-req').addEventListener('input', (e) => {
        const daily = parseInt(e.target.value) || 0;
        const monthly = daily * 30;
        document.getElementById('monthly-rev').textContent = '$' + (monthly * 0.001).toFixed(2);
        document.getElementById('monthly-cost').textContent = 'configured externally';
        document.getElementById('monthly-profit').textContent = 'not estimated';
    });
    fetch('/health').then(r => r.json()).then(d => {
        const el = document.getElementById('health-status');
        const lines = [];
        lines.push('Status: ' + d.status);
        lines.push('Version: ' + (d.version || 'unknown'));
        if (d.gpu_inference) lines.push('GPU: ' + (d.gpu_inference.configured ? 'configured (' + (d.gpu_inference.provider || 'unknown') + ')' : 'not configured'));
        lines.push('Workers AI: ' + (d.workers_ai_available ? 'available' : 'not bound'));
        lines.push('Cache KV: ' + (d.cache?.kv ? 'yes' : 'no'));
        if (d.analytics) {
            lines.push('RPM: ' + (d.analytics.requests_per_minute || 0));
            lines.push('Error rate: ' + (d.analytics.error_rate || '0%'));
            lines.push('Cache hit ratio: ' + (d.analytics.cache_hit_ratio || '0%'));
        }
        el.textContent = lines.join('\\n');
    }).catch(err => {
        document.getElementById('health-status').textContent = 'Failed to load: ' + err.message;
    });
    </script>
</body>
</html>`;

// ---------------------------------------------------------------------------
// Production hardening: timeouts, memory pressure, graceful degradation
// ---------------------------------------------------------------------------

/** Maximum time (ms) to wait for a single R2 object fetch before aborting. */
const R2_FETCH_TIMEOUT_MS = 30_000;

/** Maximum time (ms) for a single inference forward pass before aborting. */
const INFERENCE_TIMEOUT_MS = 60_000;

/** Memory pressure threshold (bytes). Skip larger models above this. */
const MEMORY_PRESSURE_THRESHOLD_MB = 200;

/**
 * Fetch an R2 object with a timeout.  Returns null (like R2 miss) on timeout.
 *
 * R2's `.get()` does not support AbortSignal, so we use Promise.race
 * against a timer.  On timeout the R2 request may still complete in the
 * background, but we proceed without waiting.
 *
 * @param {R2Bucket} bucket
 * @param {string} key
 * @param {number} [timeoutMs=R2_FETCH_TIMEOUT_MS]
 * @returns {Promise<R2ObjectBody|null>}
 */
async function fetchR2WithTimeout(bucket, key, timeoutMs = R2_FETCH_TIMEOUT_MS) {
  if (!bucket) return null;
  const TIMEOUT_SENTINEL = Symbol("timeout");
  const result = await Promise.race([
    bucket.get(key),
    new Promise((resolve) => setTimeout(() => resolve(TIMEOUT_SENTINEL), timeoutMs)),
  ]);
  if (result === TIMEOUT_SENTINEL) {
    console.warn(`R2 fetch timeout after ${timeoutMs}ms: ${key}`);
    return null;
  }
  return result;
}

export class OperationTimeoutError extends Error {
  /**
   * @param {string} label
   * @param {number} timeoutMs
   */
  constructor(label, timeoutMs) {
    super(`${label} timed out after ${timeoutMs}ms`);
    this.name = "OperationTimeoutError";
    this.label = label;
    this.timeoutMs = timeoutMs;
  }
}

/**
 * @param {unknown} err
 * @returns {boolean}
 */
export function isOperationTimeoutError(err) {
  return err instanceof OperationTimeoutError
    || (err && typeof err === "object" && err.name === "OperationTimeoutError");
}

/**
 * Run an async function with a timeout.  Rejects with OperationTimeoutError
 * if the function does not resolve within `timeoutMs`.
 *
 * @template T
 * @param {() => Promise<T>|T} fn
 * @param {number} timeoutMs
 * @param {string} label - Human-readable label for timeout error messages
 * @returns {Promise<T>}
 */
export function withTimeout(fn, timeoutMs, label = "operation") {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new OperationTimeoutError(label, timeoutMs));
    }, timeoutMs);
    let result;
    try {
      result = fn();
    } catch (err) {
      clearTimeout(timer);
      reject(err);
      return;
    }
    Promise.resolve(result).then(
      (val) => { clearTimeout(timer); resolve(val); },
      (err) => { clearTimeout(timer); reject(err); },
    );
  });
}

/**
 * Remove fields that belong to one request before sharing/caching a JSON body.
 *
 * @param {Record<string, unknown>} payload
 * @returns {Record<string, unknown>}
 */
export function stripRequestScopedJsonFields(payload) {
  const { request_id: _requestId, ...shared } = payload;
  return shared;
}

/**
 * Attach the current request ID to a JSON object response body.
 *
 * @param {string} bodyText
 * @param {string} rid
 * @returns {string}
 */
export function attachRequestIdToJsonBody(bodyText, rid) {
  try {
    const parsed = JSON.parse(bodyText);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return JSON.stringify({ ...parsed, request_id: rid });
    }
  } catch (_err) {
    // Non-JSON bodies are returned unchanged.
  }
  return bodyText;
}

/**
 * Estimate current memory usage in MB.
 *
 * Workers do not expose process.memoryUsage(), so we estimate based on
 * known allocations (model weights, WASM heap).  Returns 0 if we cannot
 * estimate (conservative: allow all models).
 *
 * @returns {number} estimated memory usage in MB
 */
function estimateMemoryMB() {
  let bytes = 0;
  // Account for loaded model weights
  if (cpuWeightsBytes) bytes += cpuWeightsBytes.byteLength;
  // Account for WASM linear memory (if instantiated)
  if (wasmInstance && wasmInstance.exports && wasmInstance.exports.memory) {
    bytes += wasmInstance.exports.memory.buffer.byteLength;
  }
  // Base JS heap overhead (~60 MB typical for Workers)
  bytes += 60 * 1024 * 1024;
  return Math.round(bytes / (1024 * 1024));
}

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

/** @type {TokenizerDecoder | null} */
let cachedTokenizer = null;

// ---------------------------------------------------------------------------
// Multi-level caching infrastructure
// ---------------------------------------------------------------------------

/** Cache TTL: 24 hours for OCR results (images don't change). */
const CACHE_TTL_MS = 24 * 60 * 60 * 1000;

/** Edge cache TTL: 1 hour (Cache API). */
const EDGE_CACHE_TTL_S = 3600;

/**
 * In-flight deduplication map.
 *
 * When two concurrent requests arrive for the same image (identified by
 * SHA-256 hash), only the first performs inference.  The second awaits
 * the first's promise and shares its result.  The entry is removed once
 * the inference promise settles.
 *
 * @type {Map<string, Promise<{ bodyText: string, status: number, headers: Record<string, string> }>>}
 */
const inflightRequests = new Map();

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
      return stripRequestScopedJsonFields(normalizeOcrResultPayload(cached, "cached OCR result"));
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
  const normalized = stripRequestScopedJsonFields(
    normalizeOcrResultPayload(result, "OCR result"),
  );
  const entry = {
    ...normalized,
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

    // WASM inference path: falcon-ocr.wasm is 13.4 MB.  With the 5-minute CPU
    // budget (300,000 ms on Workers Paid plan), cold-start compilation is feasible.
    // However, the 256 MB memory limit is tight with 129+ MB of INT8 weights plus
    // the JS heap and WASM linear memory.  If memory is the bottleneck, the CPU
    // micro model or INT4-sharded path is the fallback.
    //
    // WASM is also served to browsers via GET /wasm/* for client-side offline inference.
    // See docs/architecture/browser-webgpu-inference.md for the full architecture.
    {
      // Phase 2 (CPU): Load model based on available memory budget.
      //
      // Workers memory limit is 256 MB.  The JS runtime + V8 heap + WASM engine
      // consume ~60-80 MB, leaving ~170-190 MB for model weights + activations.
      //
      // Memory budget enforcement (streaming shard loading):
      //   - INT8 sharded (257 MB total, ~43 MB/shard): shards loaded sequentially,
      //     tensors extracted into model state, shard buffer dropped before next load.
      //     Peak memory: ~80 MB (one shard buffer + accumulated tensor data).
      //     This fits within 256 MB Workers memory when loaded incrementally.
      //   - INT4 sharded (129 MB) is the fallback if INT8 fails.
      //   - Micro model (263 KB) is the last recovery option when larger variants are unavailable.
      //
      // Priority order (best quality first):
      //   1. INT8 sharded (~6x43 MB shards, streaming) — 16x better than INT4
      //   2. INT4 sharded (~5x30 MB shards) — fallback
      //   3. INT4 single-file (~129 MB) — same quality, different packaging
      //   4. Micro model (263 KB embedded) — last recovery option
      let weightsBytes = null;
      let config = null;
      let scales = null;
      let modelVariant = "unknown";

      // Memory pressure gate: skip large models if we are already above threshold.
      const currentMemMB = estimateMemoryMB();
      const skipLargeModels = currentMemMB > MEMORY_PRESSURE_THRESHOLD_MB;
      if (skipLargeModels) {
        console.warn(`Memory pressure: ~${currentMemMB} MB used (threshold: ${MEMORY_PRESSURE_THRESHOLD_MB} MB). Skipping large models, falling back to micro.`);
      }

      // Priority 0: INT8 sharded model (16x quality over INT4).
      // Total weights: 257 MB across 6 shards (~43 MB each). The shard byte
      // buffers are loaded one at a time, but the decoded tensor map remains
      // resident because the CPU model needs all tensors for inference.
      if (!weightsBytes && !skipLargeModels) {
        const int8Prefix = "models/falcon-ocr-int8-sharded";
        const r2Int8Index = await fetchR2WithTimeout(env.WEIGHTS, `${int8Prefix}/model.safetensors.index.json`);
        const r2Int8Config = await fetchR2WithTimeout(env.WEIGHTS, `${int8Prefix}/config.json`);
        const r2Int8Scales = await fetchR2WithTimeout(env.WEIGHTS, `${int8Prefix}/scales.json`);

        if (r2Int8Index && r2Int8Config && r2Int8Scales) {
          try {
            const indexJson = JSON.parse(await r2Int8Index.text());
            config = JSON.parse(await r2Int8Config.text());
            scales = JSON.parse(await r2Int8Scales.text());
            const numShards = indexJson.metadata.num_shards;

            const shardNames = [];
            const seen = new Set();
            for (const shardName of Object.values(indexJson.weight_map)) {
              if (!seen.has(shardName)) {
                seen.add(shardName);
                shardNames.push(shardName);
              }
            }

            console.log(`Loading INT8 sharded model (streaming): ${numShards} shards`);
            totalShards = numShards;
            loadedShards = 0;

            const allTensors = new Map();
            let totalBytes = 0;

            for (const shardName of shardNames) {
              const shardObj = await fetchR2WithTimeout(env.WEIGHTS, `${int8Prefix}/${shardName}`);
              if (!shardObj) {
                throw new Error(`INT8 shard not found or timed out in R2: ${shardName}`);
              }
              // Load shard into a temporary byte buffer, then retain only the
              // parsed tensor views needed by the model.
              const shardBuffer = await shardObj.arrayBuffer();
              totalBytes += shardBuffer.byteLength;

              const shardTensors = inf.parseSafetensorsToMap(shardBuffer);
              for (const [name, tensor] of shardTensors) {
                allTensors.set(name, tensor);
              }
              // shardBuffer goes out of scope here — V8 can reclaim it before
              // the next iteration allocates the next shard's buffer.
              loadedShards++;
              console.log(`  INT8 shard ${loadedShards}/${numShards} ${shardName}: ${shardBuffer.byteLength} bytes, ${shardTensors.size} tensors`);
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
            console.warn(`INT8 streaming load failed: ${err.message}. Falling back to INT4.`);
            config = null;
            scales = null;
          }
        }
      }

      // Priority 1: INT4 sharded model (~5x30 MB shards, fits Workers memory)
      // Each shard is loaded individually, tensors extracted, buffer dropped.
      if (!weightsBytes && !skipLargeModels) {
      const r2ShardIndex = await fetchR2WithTimeout(env.WEIGHTS, "models/falcon-ocr-int4-sharded/model.safetensors.index.json");
      const r2ShardConfig = await fetchR2WithTimeout(env.WEIGHTS, "models/falcon-ocr-int4-sharded/config.json");
      const r2ShardScales = await fetchR2WithTimeout(env.WEIGHTS, "models/falcon-ocr-int4-sharded/scales.json");

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
            const shardObj = await fetchR2WithTimeout(env.WEIGHTS, `models/falcon-ocr-int4-sharded/${shardName}`);
            if (!shardObj) {
              throw new Error(`Shard not found or timed out in R2: ${shardName}`);
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

      // Priority 2: INT4 single-file model (~129 MB, tight but possible)
      if (!weightsBytes && !skipLargeModels) {
      const r2QuantWeights = await fetchR2WithTimeout(env.WEIGHTS, "models/falcon-ocr-int4/model.safetensors");
      const r2QuantConfig = await fetchR2WithTimeout(env.WEIGHTS, "models/falcon-ocr-int4/config.json");
      const r2QuantScales = await fetchR2WithTimeout(env.WEIGHTS, "models/falcon-ocr-int4/scales.json");
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

      // NOTE: F32 model (~1 GB) is SKIPPED — exceeds Workers memory by 4x.
      // F32 is only viable for browser-side or Container deployments.

      // Priority 3: Embedded micro model (263 KB, no R2 fetch, always fits)
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

    // Load tokenizer from R2 for server-side token-to-text decoding.
    // OCR text responses fail closed when token IDs cannot be decoded; token-only
    // endpoints remain available for callers that own decoding.
    if (!cachedTokenizer && env.WEIGHTS) {
      try {
        const tokObj = await fetchR2WithTimeout(env.WEIGHTS, "models/falcon-ocr/tokenizer.json");
        if (tokObj) {
          const tokJson = await tokObj.text();
          cachedTokenizer = TokenizerDecoder.fromJSON(tokJson);
          console.log(`Tokenizer loaded: ${cachedTokenizer.vocab.size} tokens`);
        } else {
          console.warn("Tokenizer not found in R2 (models/falcon-ocr/tokenizer.json)");
        }
      } catch (err) {
        console.warn(`Tokenizer load failed (non-fatal): ${err.message}`);
      }
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
/**
 * Detect whether the request originates from a browser (has Origin header
 * matching the allowed CORS origin).  Browser users get free WebGPU inference
 * client-side; API agents (no Origin) pay via x402 and get server-side GPU.
 *
 * @param {Request} request
 * @param {object} env
 * @returns {boolean}
 */
function isFromBrowser(request, env) {
  const origin = request.headers.get("Origin");
  if (!origin) return false;
  const allowedOrigin = env.CORS_ORIGIN || "https://freeinvoicemaker.app";
  return origin === allowedOrigin;
}

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
      reason: "Falcon-OCR model loading",
      error_category: category,
      request_id: rid,
      fallback_available: true,
      fallback_url: "/api/ocr/paddle",
      backends: {
        "falcon-ocr": { status: "error" },
        "paddle-ocr": { status: "available", url: "/api/ocr/paddle" },
      },
    }),
    {
      status: 503,
      headers: { ...cors, "Content-Type": "application/json", "Retry-After": "10" },
    },
  );
}

export class RateLimiter {
  /**
   * @param {DurableObjectState} state
   * @param {object} env
   */
  constructor(state, env) {
    this.state = state;
    this.env = env;
  }

  /**
   * Serialized per-IP fixed-window limiter. The Worker routes each client IP
   * to a deterministic Durable Object instance, so concurrent requests from
   * that IP cannot pass through a stale KV read.
   *
   * @param {Request} request
   * @returns {Promise<Response>}
   */
  async fetch(request) {
    const url = new URL(request.url);
    const limit = parseInt(url.searchParams.get("limit") || "100", 10);
    const windowSeconds = parseInt(url.searchParams.get("window") || "60", 10);
    const safeLimit = Number.isFinite(limit) && limit > 0 ? limit : 100;
    const safeWindowSeconds =
      Number.isFinite(windowSeconds) && windowSeconds > 0 ? windowSeconds : 60;
    const bucket = Math.floor(Date.now() / (safeWindowSeconds * 1000));
    const state = (await this.state.storage.get("counter")) || {
      bucket: -1,
      count: 0,
    };
    const current = state.bucket === bucket ? Number(state.count) || 0 : 0;

    if (current >= safeLimit) {
      return new Response(
        JSON.stringify({
          allowed: false,
          limit: safeLimit,
          remaining: 0,
          retry_after: safeWindowSeconds,
        }),
        {
          status: 429,
          headers: { "Content-Type": "application/json" },
        },
      );
    }

    const nextCount = current + 1;
    await this.state.storage.put("counter", {
      bucket,
      count: nextCount,
    });

    return new Response(
      JSON.stringify({
        allowed: true,
        limit: safeLimit,
        remaining: Math.max(0, safeLimit - nextCount),
        retry_after: safeWindowSeconds,
      }),
      {
        headers: { "Content-Type": "application/json" },
      },
    );
  }
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

    // -----------------------------------------------------------------------
    // Per-IP rate limiting for POST endpoints (100 requests/minute).
    // Uses a per-IP Durable Object for serialized read/update/write.
    // GET endpoints (health, assets) are not rate-limited.
    // -----------------------------------------------------------------------
    if (request.method === "POST") {
      if (!env.RATE_LIMITER) {
        return new Response(
          JSON.stringify({
            error: "Rate limiter unavailable",
            detail: "RATE_LIMITER Durable Object binding is not configured",
            request_id: rid,
          }),
          {
            status: 503,
            headers: { ...cors, "Content-Type": "application/json" },
          },
        );
      }
      const clientIp = request.headers.get("CF-Connecting-IP") || "unknown";
      const id = env.RATE_LIMITER.idFromName(`rate:${clientIp}`);
      const rateLimiter = env.RATE_LIMITER.get(id);
      const rateResponse = await rateLimiter.fetch(
        "https://rate-limit.local/check?limit=100&window=60",
      );
      if (rateResponse.status === 429) {
        return new Response(
          JSON.stringify({
            error: "Rate limited",
            detail: "Maximum 100 requests per minute per IP",
            request_id: rid,
          }),
          {
            status: 429,
            headers: {
              ...cors,
              "Content-Type": "application/json",
              "Retry-After": "60",
            },
          },
        );
      }
    }

    // -----------------------------------------------------------------------
    // CF Bot Protection bypass for legitimate API clients.
    // Cloudflare's Bot Management returns 403 (error 1010) for requests
    // without a browser-like User-Agent. Programmatic clients (x402 agents,
    // MCP tools) must identify themselves with a recognized User-Agent.
    // Requests carrying X-Payment-402 are always considered legitimate API
    // traffic regardless of User-Agent.
    // -----------------------------------------------------------------------
    const ALLOWED_API_USER_AGENTS = [
      "FalconOCR-Client/",
      "enjoice/",
      "molt-agent/",
    ];
    const ua = request.headers.get("User-Agent") || "";
    const hasPaymentHeader = request.headers.has("X-Payment-402");
    const isRecognizedApiClient = ALLOWED_API_USER_AGENTS.some((prefix) => ua.startsWith(prefix));
    if (!hasPaymentHeader && !isRecognizedApiClient && request.method === "POST") {
      // If cf-bot-score is available and low, allow recognized payment clients through
      const botScore = request.cf?.botManagement?.score ?? 100;
      if (botScore < 30 && !ua) {
        return new Response(
          JSON.stringify({
            error: "Bot detected",
            detail: "API clients must set User-Agent header (e.g. 'FalconOCR-Client/1.0') or include X-Payment-402 header.",
            request_id: rid,
          }),
          { status: 403, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
    }

    // -----------------------------------------------------------------------
    // Browser asset serving: WASM binary and model weights from R2.
    // These are public-read, no x402 required — inference runs locally in the
    // browser, not on this Worker. Immutable caching (weights don't change).
    // -----------------------------------------------------------------------
    if (request.method === "GET" && path.startsWith("/wasm/")) {
      const key = `models/falcon-ocr/${path.slice(6)}`; // strip "/wasm/"
      const obj = await fetchR2WithTimeout(env.WEIGHTS, key);
      if (!obj) {
        return new Response(
          JSON.stringify({ error: "Not found", key, request_id: rid }),
          { status: 404, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
      const headers = {
        ...cors,
        "Content-Type": "application/wasm",
        "Cache-Control": "public, max-age=86400, immutable",
        "Content-Length": String(obj.size),
      };
      return new Response(obj.body, { status: 200, headers });
    }

    if (request.method === "GET" && path === "/tokenizer.json") {
      const key = "models/falcon-ocr/tokenizer.json";
      const obj = await fetchR2WithTimeout(env.WEIGHTS, key);
      if (!obj) {
        return new Response(
          JSON.stringify({ error: "Not found", key, request_id: rid }),
          { status: 404, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
      const headers = {
        ...cors,
        "Content-Type": "application/json",
        "Cache-Control": "public, max-age=86400, immutable",
        "Content-Length": String(obj.size),
      };
      return new Response(obj.body, { status: 200, headers });
    }

    if (request.method === "GET" && path.startsWith("/weights/")) {
      const key = `models/${path.slice(9)}`; // strip "/weights/" -> "models/<variant>/..."
      const obj = await fetchR2WithTimeout(env.WEIGHTS, key);
      if (!obj) {
        return new Response(
          JSON.stringify({ error: "Not found", request_id: rid }),
          { status: 404, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
      // Determine content type from extension
      let contentType = "application/octet-stream";
      if (key.endsWith(".json")) contentType = "application/json";
      else if (key.endsWith(".safetensors")) contentType = "application/octet-stream";

      const headers = {
        ...cors,
        "Content-Type": contentType,
        "Cache-Control": "public, max-age=86400, immutable",
        "Content-Length": String(obj.size),
      };
      return new Response(obj.body, { status: 200, headers });
    }

    // -----------------------------------------------------------------------
    // Browser test page: serves the WebGPU inference test UI and its JS deps.
    // HTML is embedded inline (242 lines); JS files served from R2.
    // -----------------------------------------------------------------------
    if (request.method === "GET" && path === "/test") {
      return new Response(TEST_HTML, {
        headers: { "Content-Type": "text/html; charset=utf-8", ...cors }
      });
    }

    if (request.method === "GET" && path === "/dashboard") {
      return new Response(DASHBOARD_HTML, {
        headers: { "Content-Type": "text/html; charset=utf-8", ...cors }
      });
    }

    if (request.method === "GET" && path.startsWith("/browser/")) {
      const key = `browser/${path.slice(9)}`;
      const obj = await fetchR2WithTimeout(env.WEIGHTS, key);
      if (!obj) {
        return new Response(
          JSON.stringify({ error: "Not found", key, request_id: rid }),
          { status: 404, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
      let contentType = "application/javascript";
      if (key.endsWith(".html")) contentType = "text/html; charset=utf-8";
      else if (key.endsWith(".wasm")) contentType = "application/wasm";
      return new Response(obj.body, {
        status: 200,
        headers: { ...cors, "Content-Type": contentType, "Cache-Control": "public, max-age=3600" },
      });
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
          // Use fast handler (Llama 3.2 3B + caching) for < 3s latency
          const result = await handleTemplateExtractFast(env, imageBytes, extractOptions);
          const responseBody = { ...result, request_id: rid };
          const headers = { ...cors, "Content-Type": "application/json" };
          if (payment.receipt) headers["X-Payment-Receipt"] = payment.receipt;
          return new Response(JSON.stringify(responseBody), { status: 200, headers });
        }
      } catch (err) {
        return new Response(
          JSON.stringify({
            error: "Template extraction temporarily unavailable",
            error_category: categorizeError(err, 503),
            request_id: rid,
          }),
          {
            status: 503,
            headers: { ...cors, "Content-Type": "application/json", "Retry-After": "5" },
          },
        );
      }
    }

    // Fast path: detailed OCR with block-level output, tables, and multi-page support
    if (path === "/ocr/detailed" && request.method === "POST" && isWorkersAiAvailable(env)) {
      const payment = await verifyX402(request, env, rid, cors);
      if (!payment.authorized) {
        return payment.response;
      }
      const response = await handleDetailedOcr(request, env, cors, rid);
      if (payment.receipt) {
        const headers = new Headers(response.headers);
        headers.set("X-Payment-Receipt", payment.receipt);
        return new Response(response.body, { status: response.status, headers });
      }
      return response;
    }

    // Fast path: table-only extraction
    if (path === "/ocr/table" && request.method === "POST" && isWorkersAiAvailable(env)) {
      const payment = await verifyX402(request, env, rid, cors);
      if (!payment.authorized) {
        return payment.response;
      }
      const response = await handleTableOcr(request, env, cors, rid);
      if (payment.receipt) {
        const headers = new Headers(response.headers);
        headers.set("X-Payment-Receipt", payment.receipt);
        return new Response(response.body, { status: response.status, headers });
      }
      return response;
    }

    // Fast path: NL invoice fill uses Workers AI to parse natural language
    // into structured invoice data (no image required).
    if (path === "/invoice/fill" && request.method === "POST" && isWorkersAiAvailable(env)) {
      const payment = await verifyX402(request, env, rid, cors);
      if (!payment.authorized) {
        return payment.response;
      }
      const response = await handleInvoiceFill(request, env, cors, rid);
      if (payment.receipt) {
        const headers = new Headers(response.headers);
        headers.set("X-Payment-Receipt", payment.receipt);
        return new Response(response.body, { status: response.status, headers });
      }
      return response;
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

    // PaddleOCR via ONNX — 16 MB total, fits in Workers memory.
    // Compiled through molt tinygrad: the showcase demo for the compiler.
    if (path === "/ocr/paddle-molt" && request.method === "POST") {
      return new Response(JSON.stringify({
        engine: "paddleocr-molt",
        status: "loading",
        models: {
          detector: { name: "ch_PP-OCRv4_det", size_mb: 4.7, constants: 342, status: "available" },
          recognizer: { name: "ch_PP-OCRv4_rec", size_mb: 10.8, constants: 406, status: "available" },
          classifier: { name: "ch_ppocr_mobile_v2.0_cls", size_mb: 0.6, constants: 308, status: "available" },
        },
        total_size_mb: 16.1,
        pipeline: [
          "ONNX Constant nodes -> OnnxWeightParser -> tinygrad Tensor graph",
          "tinygrad 26 primitives -> molt compiler -> WebGPU/WASM/native",
          "DBNet detector -> direction classifier -> SVTRv2 recognizer -> CTC decode",
        ],
        note: "PaddleOCR compiled through molt tinygrad — the showcase demo",
        request_id: rid,
      }), { status: 200, headers: { ...cors, "Content-Type": "application/json" } });
    }

    // Fast path: Queue-based batch OCR — no local model loading.
    // Uses Workers AI exclusively via the queue consumer.
    if (path === "/batch" && request.method === "POST") {
      const { handleBatchSubmit } = await import("./queue-batch-ocr.js");
      if (!env.OCR_QUEUE) {
        return new Response(
          JSON.stringify({
            error: "Queue not available",
            detail: "OCR_QUEUE binding is not configured",
            hint: "Queues require Workers Paid plan and [[queues.producers]] in wrangler.toml",
            request_id: rid,
          }),
          { status: 501, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
      const response = await handleBatchSubmit(request, env);
      const headers = new Headers(response.headers);
      Object.entries(cors).forEach(([k, v]) => headers.set(k, v));
      headers.set("X-Request-ID", rid);
      return new Response(response.body, { status: response.status, headers });
    }

    if (path.startsWith("/batch/") && request.method === "GET") {
      const { handleBatchStatus } = await import("./queue-batch-ocr.js");
      const response = await handleBatchStatus(request, env);
      const headers = new Headers(response.headers);
      Object.entries(cors).forEach(([k, v]) => headers.set(k, v));
      headers.set("X-Request-ID", rid);
      return new Response(response.body, { status: response.status, headers });
    }

    // OCR path: PaddleOCR (client-side WASM) is the PRODUCTION PRIMARY engine.
    // Falcon-OCR is experimental — only activated via X-Use-Backend: falcon-ocr.
    // Workers AI is NOT used for /ocr — it hallucinates invoice content.
    // Workers AI remains available for /invoice/fill and /template/extract only.

    let result;
    try {
    result = await withMonitoring({
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

          // Fetch analytics (non-blocking best-effort)
          let analyticsSummary = null;
          try {
            analyticsSummary = await getAnalyticsSummary(env);
          } catch (_) {
            // Analytics fetch is non-fatal
          }

          return new Response(
            JSON.stringify({
              status,
              version: env.MODEL_VERSION || "0.1.0",
              request_id: rid,
              ocr_engines: {
                "paddle-ocr": {
                  status: "recommended",
                  location: "browser",
                  quality: "external benchmark required",
                  note: "Recommended browser-side OCR path; verify against current benchmarks",
                },
                "falcon-ocr": {
                  status: "experimental",
                  location: "edge",
                  quality: "degraded (INT4 CPU)",
                  model_variant: modelReady ? activeModelVariant : undefined,
                  device: modelReady ? device : undefined,
                  error: initError || undefined,
                  loading: isLoading ? {
                    shards_loaded: loadedShards,
                    shards_total: totalShards,
                    elapsed_ms: elapsedMs,
                    estimated_remaining_ms: estimatedRemainingMs,
                  } : undefined,
                  note: "Experimental — activate with X-Use-Backend: falcon-ocr header",
                },
              },
              gpu_inference: gpuInferenceStatus(env),
              workers_ai_available: isWorkersAiAvailable(env),
              wasm_available: true,
              cache: {
                kv: !!env.CACHE,
                edge: true,
              },
              analytics: analyticsSummary ? {
                requests_per_minute: analyticsSummary.requests_per_minute,
                error_rate: analyticsSummary.error_rate,
                cache_hit_ratio: analyticsSummary.cache_hit_ratio,
                latency: {
                  p50_ms: analyticsSummary.latency_p50_ms,
                  p95_ms: analyticsSummary.latency_p95_ms,
                  p99_ms: analyticsSummary.latency_p99_ms,
                },
                window_minutes: analyticsSummary.time_window_minutes,
              } : undefined,
              workers_ai_usage: "NL fill and template extraction ONLY (not OCR)",
              backends: {
                "paddle-ocr": {
                  status: "recommended",
                  location: "browser",
                  quality: "external benchmark required",
                  note: "Recommended browser-side OCR engine; verify against current benchmarks",
                },
                "falcon-ocr": {
                  status: modelReady ? "ready" : initError ? "error" : "loading",
                  device: modelReady ? device : undefined,
                  error: initError || undefined,
                  note: "Experimental OCR engine — use X-Use-Backend: falcon-ocr",
                },
                "workers-ai": {
                  status: isWorkersAiAvailable(env) ? "available" : "not-bound",
                  note: "Used for /invoice/fill and /template/extract ONLY (not OCR)",
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
            { status: 405, headers: { ...cors, "Content-Type": "application/json", "X-Request-ID": rid } },
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

        // Backend selection: PaddleOCR is the recommended client-side OCR path.
        // Falcon-OCR is experimental — only when explicitly requested via header.
        const useBackend = request.headers.get("X-Use-Backend");

        // -------------------------------------------------------------------
        // x402 → configured GPU inference path (default for API agents).
        //
        // For API agents (no Origin header, paying via x402):
        //   Route to the configured GPU inference proxy.
        //
        // Browser users (Origin header) skip this — they get free WebGPU
        // inference client-side via PaddleOCR/Falcon-OCR WASM.
        //
        // If the caller explicitly sets X-Use-Backend, respect that choice
        // and fall through to the existing backend routing below.
        // -------------------------------------------------------------------
        if (path === "/ocr" && !useBackend && !isFromBrowser(request, env)) {
          const gpuStatus = gpuInferenceStatus(env);
          if (gpuStatus.configured) {
            try {
              const body = await request.clone().json();
              if (body.image && typeof body.image === "string") {
                const gpuResult = await proxyToGPU(env, body.image, {
                  maxTokens: body.max_tokens || 512,
                  category: body.category || "plain",
                  timeout: 30_000,
                });
                if (gpuResult) {
                  return new Response(
                    JSON.stringify({
                      ...gpuResult,
                      engine: "falcon-ocr",
                      backend: "gpu",
                      payment_receipt: payment.receipt || undefined,
                      request_id: rid,
                    }),
                    {
                      status: 200,
                      headers: {
                        ...cors,
                        ...receiptHeaders,
                        "Content-Type": "application/json",
                        "X-Request-ID": rid,
                      },
                    },
                  );
                }
                return new Response(
                  JSON.stringify({
                    error: "GPU inference backend returned no result",
                    request_id: rid,
                  }),
                  {
                    status: 503,
                    headers: {
                      ...cors,
                      ...receiptHeaders,
                      "Content-Type": "application/json",
                      "X-Request-ID": rid,
                    },
                  },
                );
              }
            } catch (gpuErr) {
              return new Response(
                JSON.stringify({
                  error: "GPU inference backend failed",
                  detail: gpuErr.message,
                  request_id: rid,
                }),
                {
                  status: 503,
                  headers: {
                    ...cors,
                    ...receiptHeaders,
                    "Content-Type": "application/json",
                    "X-Request-ID": rid,
                  },
                },
              );
            }
          }
          // GPU not configured — continue to existing explicit backend guidance below.
        }

        // Route to Durable Object for persistent Falcon-OCR inference.
        if (useBackend === "durable" && env.FALCON_OCR && path === "/ocr") {
          try {
            const id = env.FALCON_OCR.idFromName("default");
            const stub = env.FALCON_OCR.get(id);
            const doResponse = await stub.fetch(request.clone());
            const headers = new Headers(doResponse.headers);
            Object.entries(cors).forEach(([k, v]) => headers.set(k, v));
            headers.set("X-Request-ID", rid);
            if (payment.receipt) headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(doResponse.body, { status: doResponse.status, headers });
          } catch (doErr) {
            console.error(`Durable Object error: ${doErr.message}`, doErr.stack);
            return new Response(
              JSON.stringify({
                error: "Durable Object inference failed",
                reason: doErr.message,
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

        // External GPU service path (X-Use-Backend: gpu).
        // Forwards to an external GPU inference service for bfloat16-quality inference.
        if (path === "/ocr" && useBackend === "gpu") {
          try {
            const body = await request.json();
            if (!body.image || typeof body.image !== "string") {
              return new Response(
                JSON.stringify({ error: "Missing 'image' field (base64 string)", request_id: rid }),
                { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
              );
            }

            const gpuStatus = gpuInferenceStatus(env);
            if (!gpuStatus.configured) {
              return new Response(
                JSON.stringify({
                  error: "GPU inference not configured",
                  detail: gpuStatus.error || "Set GPU_INFERENCE_URL, GPU_INFERENCE_KEY, and GPU_INFERENCE_PROVIDER secrets",
                  request_id: rid,
                  fallback_hint: "Use X-Use-Backend: falcon-ocr for edge CPU, or PaddleOCR client-side",
                }),
                { status: 501, headers: { ...cors, "Content-Type": "application/json" } },
              );
            }

            const result = await proxyToGPU(env, body.image, {
              maxTokens: body.max_tokens || 512,
              category: body.category || "plain",
              timeout: 30_000,
            });

            if (!result) {
              return new Response(
                JSON.stringify({
                  error: "GPU inference failed or timed out",
                  provider: gpuStatus.provider,
                  request_id: rid,
                  fallback_hint: "Use X-Use-Backend: falcon-ocr for edge CPU inference",
                }),
                { status: 502, headers: { ...cors, "Content-Type": "application/json" } },
              );
            }

            return new Response(
              JSON.stringify({ ...result, request_id: rid }),
              {
                status: 200,
                headers: {
                  ...cors,
                  ...receiptHeaders,
                  "Content-Type": "application/json",
                  "X-Request-ID": rid,
                },
              },
            );
          } catch (gpuErr) {
            console.error(`GPU proxy error: ${gpuErr.message} [rid=${rid}]`);
            return new Response(
              JSON.stringify({
                error: "GPU inference request failed",
                reason: gpuErr.message,
                request_id: rid,
                fallback_hint: "Use X-Use-Backend: falcon-ocr for edge CPU, or PaddleOCR client-side",
              }),
              { status: 502, headers: { ...cors, "Content-Type": "application/json" } },
            );
          }
        }

        // Browser Rendering GPU inference path (X-Use-Backend: browser-gpu).
        // Spawns headless Chrome with WebGPU to run Falcon-OCR on the edge.
        if (path === "/ocr" && useBackend === "browser-gpu") {
          const { inferWithBrowserGPU, probeGPU, isBrowserRenderingAvailable } = await import("./browser-gpu-inference.js");
          if (!isBrowserRenderingAvailable(env)) {
            return new Response(
              JSON.stringify({
                error: "Browser Rendering not available",
                detail: "BROWSER binding is not configured or not accessible in this environment",
                hint: "Browser Rendering requires Workers Paid plan and [browser] binding in wrangler.toml",
                request_id: rid,
              }),
              {
                status: 501,
                headers: { ...cors, "Content-Type": "application/json" },
              },
            );
          }

          try {
            const body = await request.json();
            if (!body.image || typeof body.image !== "string") {
              return new Response(
                JSON.stringify({ error: "Missing 'image' field (base64 string)", request_id: rid }),
                { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
              );
            }

            // If probe=true, just check GPU availability without running inference
            if (body.probe) {
              const gpuInfo = await probeGPU(env);
              return new Response(
                JSON.stringify({
                  backend: "browser-gpu",
                  gpu_info: gpuInfo,
                  request_id: rid,
                }),
                {
                  status: 200,
                  headers: { ...cors, "Content-Type": "application/json" },
                },
              );
            }

            const result = await inferWithBrowserGPU(env, body.image, {
              timeoutMs: 30000,
            });

            return new Response(
              JSON.stringify({
                text: result.text,
                confidence: result.confidence,
                backend: "browser-gpu",
                gpu_info: result.gpuInfo,
                latency_ms: result.latencyMs,
                inference_ms: result.inferenceMs,
                request_id: rid,
              }),
              {
                status: 200,
                headers: { ...cors, ...receiptHeaders, "Content-Type": "application/json" },
              },
            );
          } catch (err) {
            console.error(`Browser GPU inference error: ${err.message} [rid=${rid}]`);
            return new Response(
              JSON.stringify({
                error: "Browser GPU inference failed",
                reason: err.message,
                request_id: rid,
                fallback_hint: "Use X-Use-Backend: falcon-ocr for CPU inference, or PaddleOCR client-side",
              }),
              {
                status: 502,
                headers: { ...cors, "Content-Type": "application/json" },
              },
            );
          }
        }

        // Nemotron OCR v2 path (X-Use-Backend: nemotron).
        // The current upstream package is PyTorch/CUDA-oriented and must be
        // routed through an explicitly configured GPU service endpoint before use.
        // Until then this route fails closed instead of guessing an endpoint.
        if (path === "/ocr" && useBackend === "nemotron") {
          return new Response(
            JSON.stringify({
              error: "Nemotron OCR v2 requires a configured GPU service endpoint",
              status: "planned",
              info: {
                model: "nvidia/nemotron-ocr-v2",
                pipeline: "3-stage (detector + recognizer + relational)",
                runtime: "PyTorch/CUDA upstream package",
                format: "PyTorch .pth (no ONNX export available)",
                deployment_target: "configured GPU service endpoint",
                blocker: "no Nemotron GPU service endpoint configured",
              },
              request_id: rid,
            }),
            { status: 501, headers: { ...cors, "Content-Type": "application/json" } },
          );
        }

        // Default /ocr path: PaddleOCR is primary (runs client-side in browser).
        // The Worker returns 503 directing the client to use PaddleOCR locally,
        // unless the caller explicitly opts into Falcon-OCR via header.
        if (path === "/ocr" && useBackend !== "falcon-ocr") {
          return new Response(
            JSON.stringify({
              error: "Use the configured client-side OCR path",
              ocr_engine: "paddle-ocr",
              location: "browser",
              quality: "external benchmark required",
              fallback_url: "/api/ocr/paddle",
              hint: "The client-side OCR path runs in the browser when configured. To use experimental Falcon-OCR edge inference, set header X-Use-Backend: falcon-ocr. Nemotron v2 requires an explicitly configured GPU service endpoint.",
              request_id: rid,
            }),
            {
              status: 503,
              headers: { ...cors, ...receiptHeaders, "Content-Type": "application/json" },
            },
          );
        }

        // Falcon-OCR experimental path (X-Use-Backend: falcon-ocr).
        // Ensure model is loaded (lazy init on first request).
        // On failure, return fallback response instead of 500.
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
              let edgeBodyText = null;
              try {
                const rawEdgeBody = await edgeCached.clone().text();
                const edgePayload = stripRequestScopedJsonFields(
                  normalizeOcrResultPayload(JSON.parse(rawEdgeBody), "edge cached OCR result"),
                );
                edgeBodyText = JSON.stringify(edgePayload);
              } catch (_err) {
                edgeBodyText = null;
              }
              if (!edgeBodyText) {
                // Treat malformed edge cache entries as misses. Fresh inference
                // will repopulate both edge and KV cache with validated output.
              } else {
                const headers = new Headers(edgeCached.headers);
                Object.entries(cors).forEach(([k, v]) => headers.set(k, v));
                headers.set("X-Cache-Status", "HIT-EDGE");
                headers.set("X-Request-ID", rid);
                if (payment.receipt) headers.set("X-Payment-Receipt", payment.receipt);
                return new Response(attachRequestIdToJsonBody(edgeBodyText, rid), {
                  status: edgeCached.status,
                  headers,
                });
              }
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
                    "X-Request-ID": rid,
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

          // Level 3: Compute inference (cache miss) — with inflight dedup + timeout guard.
          //
          // If another request for the same image is already running inference,
          // wait for that result instead of launching a duplicate forward pass.
          // The inflight map stores a Promise that resolves to a buffered JSON
          // body + status, so every waiter can construct an independent Response.
          let response;

          /**
           * Run inference, cache the result, and return a JSON body/status pair
           * that can be shared across concurrent waiters without body-consumed
           * or request_id leakage.
           */
          const doInference = async () => {
            const inferResult = await withTimeout(
              () => handleOcrRequest(request, inferenceBackend, env, cors, rid, activeDevice, cachedTokenizer),
              INFERENCE_TIMEOUT_MS,
              "OCR inference",
            );

            // Buffer the response body so we can share it across waiters.
            const bodyText = await inferResult.text();
            const status = inferResult.status;
            const headers = Object.fromEntries(inferResult.headers.entries());

            let sharedBodyText = bodyText;
            if (status === 200) {
              try {
                const resultBody = JSON.parse(bodyText);
                const normalized = stripRequestScopedJsonFields(
                  normalizeOcrResultPayload(resultBody, "OCR result"),
                );
                sharedBodyText = JSON.stringify(normalized);
                if (imageHash) {
                  setCachedResult(env, imageHash, normalized, ctx);
                  setEdgeCachedResponse(
                    request,
                    imageHash,
                    new Response(sharedBodyText, { status, headers: { ...headers } }),
                    ctx,
                  );
                }
              } catch (err) {
                throw new Error(`Invalid OCR inference response: ${err.message}`);
              }
            }
            return { bodyText: sharedBodyText, status, headers };
          };

          try {
            let shared;
            if (imageHash && inflightRequests.has(imageHash)) {
              // Another request is already computing this image — share the result.
              shared = await inflightRequests.get(imageHash);
            } else {
              const inferencePromise = doInference();
              if (imageHash) {
                inflightRequests.set(imageHash, inferencePromise);
              }
              try {
                shared = await inferencePromise;
              } finally {
                if (imageHash) inflightRequests.delete(imageHash);
              }
            }
            response = new Response(attachRequestIdToJsonBody(shared.bodyText, rid), {
              status: shared.status,
              headers: { ...shared.headers },
            });
          } catch (err) {
            if (imageHash) inflightRequests.delete(imageHash);
            if (!isOperationTimeoutError(err)) {
              console.error(`Inference failed: ${err.message} [rid=${rid}]`);
              return new Response(
                JSON.stringify({
                  error: "OCR inference failed",
                  detail: err.message,
                  request_id: rid,
                  fallback_available: true,
                  fallback_url: "/api/ocr/paddle",
                }),
                {
                  status: 502,
                  headers: { ...cors, "Content-Type": "application/json", "X-Request-ID": rid },
                },
              );
            }
            console.error(`Inference timeout: ${err.message} [rid=${rid}]`);
            return new Response(
              JSON.stringify({
                error: "OCR inference timed out",
                detail: `Forward pass exceeded ${INFERENCE_TIMEOUT_MS}ms limit`,
                request_id: rid,
                fallback_available: true,
                fallback_url: "/api/ocr/paddle",
              }),
              {
                status: 504,
                headers: { ...cors, "Content-Type": "application/json", "Retry-After": "5", "X-Request-ID": rid },
              },
            );
          }

          // Attach payment receipt to successful responses
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            headers.set("X-Request-ID", rid);
            return new Response(response.body, {
              status: response.status,
              headers,
            });
          }
          const finalHeaders = new Headers(response.headers);
          finalHeaders.set("X-Request-ID", rid);
          return new Response(response.body, {
            status: response.status,
            headers: finalHeaders,
          });
        }

        if (path === "/ocr/tokens") {
          let response;
          try {
            response = await withTimeout(
              () => handleTokensRequest(request, inferenceBackend, env, cors, rid, activeDevice),
              INFERENCE_TIMEOUT_MS,
              "Token inference",
            );
          } catch (err) {
            if (!isOperationTimeoutError(err)) {
              console.error(`Token inference failed: ${err.message} [rid=${rid}]`);
              return new Response(
                JSON.stringify({
                  error: "Token inference failed",
                  detail: err.message,
                  request_id: rid,
                }),
                {
                  status: 502,
                  headers: { ...cors, "Content-Type": "application/json" },
                },
              );
            }
            console.error(`Token inference timeout: ${err.message} [rid=${rid}]`);
            return new Response(
              JSON.stringify({
                error: "Token inference timed out",
                detail: `Forward pass exceeded ${INFERENCE_TIMEOUT_MS}ms limit`,
                request_id: rid,
              }),
              {
                status: 504,
                headers: { ...cors, "Content-Type": "application/json", "Retry-After": "5" },
              },
            );
          }
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
          const response = await handleBatchOcr(request, inferenceBackend, env, cors, rid, activeDevice, cacheOps, cachedTokenizer);
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

        // Detailed OCR: block-level output with bounding boxes, tables, multi-page
        if (path === "/ocr/detailed") {
          const response = await handleDetailedOcr(request, env, cors, rid);
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, { status: response.status, headers });
          }
          return response;
        }

        // Table-only extraction
        if (path === "/ocr/table") {
          const response = await handleTableOcr(request, env, cors, rid);
          if (payment.receipt) {
            const headers = new Headers(response.headers);
            headers.set("X-Payment-Receipt", payment.receipt);
            return new Response(response.body, { status: response.status, headers });
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

          const result = await handleTemplateExtractFast(env, imageBytes, extractOptions);
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
          { status: 404, headers: { ...cors, "Content-Type": "application/json", "X-Request-ID": rid } },
        );
      },
    });

    } catch (err) {
      // Top-level catch: ensures all unhandled errors return structured JSON
      // with CORS headers, request_id, and no sensitive data.
      return new Response(
        JSON.stringify({
          error: "Internal server error",
          error_category: categorizeError(err, 500),
          request_id: rid,
        }),
        {
          status: 500,
          headers: { ...cors, "Content-Type": "application/json", "X-Request-ID": rid },
        },
      );
    }

    // Warm instance pool: pre-load the Falcon-OCR model on warm instances.
    // Only pre-warm for paths that actually use the local model (falcon-ocr).
    // Paths that use Workers AI exclusively (batch, structured, fill) skip
    // pre-warming to avoid exceeding the 256 MB memory limit.
    const skipPrewarm = path === "/batch" || path.startsWith("/batch/")
      || path === "/ocr/structured" || path === "/invoice/fill"
      || path === "/ocr/detailed" || path === "/ocr/table"
      || path === "/template/extract";
    if (!skipPrewarm) {
      ctx.waitUntil(
        (async () => {
          if (!modelReady && !initPromise && !initError) {
            try {
              await ensureModelLoaded(env);
            } catch (_err) {
              // Non-fatal: model will be loaded on next request
            }
          }
        })()
      );
    }

    return result;
  },

  /**
   * Queue consumer handler — processes async batch OCR messages.
   * Called by Cloudflare Queues runtime when messages arrive on falcon-ocr-batch.
   *
   * @param {object} batch - Queue message batch
   * @param {object} env - Worker environment bindings
   */
  async queue(batch, env) {
    const { processQueueBatch } = await import("./queue-batch-ocr.js");
    await processQueueBatch(batch, env);
  },
};

// Re-export the Durable Object class for Cloudflare's runtime.
export { FalconOCRInference } from "./falcon-ocr-do.js";
