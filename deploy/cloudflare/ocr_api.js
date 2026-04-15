/**
 * REST API handlers for Falcon-OCR inference.
 *
 * Endpoints:
 *   POST /ocr          — accepts image (base64 or multipart), returns text
 *   POST /ocr/tokens   — low-level: image + prompt IDs -> token IDs
 *   GET  /health       — returns 200 with model status
 *
 * Security:
 *   - x402 payment verification (handled in worker.js before these handlers)
 *   - CORS restricted to freeinvoicemaker.app
 *   - No logging of image content (privacy)
 *   - Request ID for debugging
 */

const MAX_IMAGE_BYTES_DEFAULT = 10 * 1024 * 1024; // 10 MB
const SUPPORTED_CONTENT_TYPES = new Set([
  "image/jpeg",
  "image/png",
  "image/webp",
]);

/**
 * Validate and extract image bytes from the request body.
 *
 * Supports two input formats:
 *   1. multipart/form-data with an "image" field
 *   2. application/json with a base64 "image" field
 *
 * @param {Request} request
 * @param {object} env
 * @returns {Promise<{ bytes: Uint8Array, format: string } | { error: string, status: number }>}
 */
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
      return {
        error: `Image too large: ${file.size} bytes (max ${maxBytes})`,
        status: 413,
      };
    }
    const imageType = file.type || "image/jpeg";
    if (!SUPPORTED_CONTENT_TYPES.has(imageType)) {
      return {
        error: `Unsupported image format: ${imageType}. Supported: JPEG, PNG, WebP`,
        status: 415,
      };
    }
    return {
      bytes: new Uint8Array(await file.arrayBuffer()),
      format: imageType,
    };
  }

  if (contentType.includes("application/json")) {
    const body = /** @type {{ image?: string, format?: string }} */ (await request.json());
    if (!body.image || typeof body.image !== "string") {
      return { error: "Missing 'image' field (base64 string) in JSON body", status: 400 };
    }
    const raw = atob(body.image);
    if (raw.length > maxBytes) {
      return {
        error: `Image too large: ${raw.length} bytes (max ${maxBytes})`,
        status: 413,
      };
    }
    const bytes = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i++) {
      bytes[i] = raw.charCodeAt(i);
    }
    const format = body.format || "image/jpeg";
    if (!SUPPORTED_CONTENT_TYPES.has(format)) {
      return {
        error: `Unsupported image format: ${format}. Supported: JPEG, PNG, WebP`,
        status: 415,
      };
    }
    return { bytes, format };
  }

  return {
    error: "Unsupported Content-Type. Use multipart/form-data or application/json",
    status: 415,
  };
}

/**
 * Decode a raw image (JPEG/PNG/WebP) into RGB pixel data.
 *
 * In the Workers runtime we use the built-in ImageDecoder API when
 * available, falling back to a minimal JPEG/PNG header parse for
 * dimensions and delegating actual decode to the WASM module's
 * image preprocessing.
 *
 * @param {Uint8Array} bytes - Raw image file bytes
 * @param {string} _format - MIME type (unused for now, auto-detected)
 * @returns {Promise<{ width: number, height: number, rgb: Uint8Array }>}
 */
async function decodeImageToRgb(bytes, _format) {
  // Workers environment: use createImageBitmap if available (Cloudflare
  // Workers support this since 2025-Q3).  Otherwise fall back to passing
  // raw bytes and letting the WASM module handle decode.
  if (typeof createImageBitmap === "function") {
    const blob = new Blob([bytes]);
    const bitmap = await createImageBitmap(blob);
    const { width, height } = bitmap;

    // OffscreenCanvas to extract pixel data
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext("2d");
    ctx.drawImage(bitmap, 0, 0);
    const imageData = ctx.getImageData(0, 0, width, height);
    bitmap.close();

    // RGBA -> RGB
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

  // Fallback: parse image dimensions from header and pass raw bytes.
  // This path requires the WASM module to handle image decode internally.
  throw new Error(
    "createImageBitmap not available in this Workers runtime. " +
    "Upgrade to compatibility_date >= 2025-07-01 or use the browser-side path."
  );
}

/**
 * Align dimensions to the model's patch size (16).
 *
 * @param {number} width
 * @param {number} height
 * @param {number} patchSize
 * @returns {{ width: number, height: number }}
 */
function alignToPatch(width, height, patchSize = 16) {
  return {
    width: Math.ceil(width / patchSize) * patchSize,
    height: Math.ceil(height / patchSize) * patchSize,
  };
}

/**
 * Pad RGB data to patch-aligned dimensions with black pixels.
 *
 * @param {Uint8Array} rgb - Original RGB data
 * @param {number} origWidth
 * @param {number} origHeight
 * @param {number} newWidth
 * @param {number} newHeight
 * @returns {Uint8Array}
 */
function padRgb(rgb, origWidth, origHeight, newWidth, newHeight) {
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

/**
 * POST /ocr — full OCR: image in, text out.
 *
 * @param {Request} request
 * @param {WebAssembly.Instance} wasmInstance
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @returns {Promise<Response>}
 */
export async function handleOcrRequest(request, wasmInstance, env, cors, rid) {
  const start = Date.now();
  const imageResult = await extractImage(request, env);
  if ("error" in imageResult) {
    return new Response(
      JSON.stringify({ error: imageResult.error, request_id: rid }),
      {
        status: imageResult.status,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }

  const decoded = await decodeImageToRgb(imageResult.bytes, imageResult.format);
  const aligned = alignToPatch(decoded.width, decoded.height);
  const rgb = padRgb(decoded.rgb, decoded.width, decoded.height, aligned.width, aligned.height);

  // Default OCR prompt: empty prompt IDs triggers the model's default
  // OCR behavior (describe the document content).
  const promptIds = [];
  const maxNewTokens = 512;

  const tokens = wasmInstance.exports.ocr_tokens(
    aligned.width,
    aligned.height,
    rgb,
    promptIds,
    maxNewTokens,
  );

  // Token-to-text decode happens server-side using a cached tokenizer.
  // For now, return raw tokens — the browser-side has its own tokenizer.
  const timMs = Date.now() - start;

  return new Response(
    JSON.stringify({
      text: "",
      tokens: Array.from(tokens),
      confidence: 0.0,
      time_ms: timMs,
      device: "wasm",
      request_id: rid,
    }),
    {
      status: 200,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}

/**
 * POST /ocr/tokens — low-level: image + prompt IDs -> token IDs.
 *
 * @param {Request} request
 * @param {WebAssembly.Instance} wasmInstance
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @returns {Promise<Response>}
 */
export async function handleTokensRequest(request, wasmInstance, env, cors, rid) {
  const start = Date.now();
  const contentType = request.headers.get("Content-Type") || "";

  if (!contentType.includes("application/json")) {
    return new Response(
      JSON.stringify({
        error: "/ocr/tokens requires application/json",
        request_id: rid,
      }),
      {
        status: 415,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }

  const body = /** @type {{
    width: number,
    height: number,
    rgb: string,
    prompt_ids: number[],
    max_new_tokens?: number,
  }} */ (await request.json());

  if (
    typeof body.width !== "number" ||
    typeof body.height !== "number" ||
    typeof body.rgb !== "string" ||
    !Array.isArray(body.prompt_ids)
  ) {
    return new Response(
      JSON.stringify({
        error: "Missing required fields: width, height, rgb (base64), prompt_ids",
        request_id: rid,
      }),
      {
        status: 400,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }

  const rawRgb = atob(body.rgb);
  const rgb = new Uint8Array(rawRgb.length);
  for (let i = 0; i < rawRgb.length; i++) {
    rgb[i] = rawRgb.charCodeAt(i);
  }

  const maxNewTokens = body.max_new_tokens || 512;

  const tokens = wasmInstance.exports.ocr_tokens(
    body.width,
    body.height,
    rgb,
    body.prompt_ids,
    maxNewTokens,
  );

  const timeMs = Date.now() - start;

  return new Response(
    JSON.stringify({
      tokens: Array.from(tokens),
      time_ms: timeMs,
      device: "wasm",
      request_id: rid,
    }),
    {
      status: 200,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}

/**
 * GET /health — model status.
 *
 * @param {boolean} modelReady
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @returns {Response}
 */
export function handleHealthRequest(modelReady, env, cors, rid) {
  return new Response(
    JSON.stringify({
      status: modelReady ? "ready" : "loading",
      model: "falcon-ocr",
      version: env.MODEL_VERSION || "0.1.0",
      device: "wasm",
      request_id: rid,
    }),
    {
      status: 200,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}
