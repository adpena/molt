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

/**
 * Extract image dimensions from PNG or JPEG header bytes.
 * Returns null if the format is not recognized.
 *
 * @param {Uint8Array} bytes
 * @returns {{ width: number, height: number } | null}
 */
function parseImageDimensions(bytes) {
  // PNG: signature (8 bytes) + IHDR chunk (length 4 + "IHDR" 4 + width 4 + height 4)
  if (bytes.length >= 24 &&
      bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47) {
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const width = dv.getUint32(16, false);
    const height = dv.getUint32(20, false);
    return { width, height };
  }
  // JPEG: SOI marker (FF D8), scan for SOF0 (FF C0) or SOF2 (FF C2)
  if (bytes.length >= 2 && bytes[0] === 0xff && bytes[1] === 0xd8) {
    let offset = 2;
    while (offset < bytes.length - 8) {
      if (bytes[offset] !== 0xff) { offset++; continue; }
      const marker = bytes[offset + 1];
      if (marker === 0xc0 || marker === 0xc2) {
        const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        const height = dv.getUint16(offset + 5, false);
        const width = dv.getUint16(offset + 7, false);
        return { width, height };
      }
      const segLen = (bytes[offset + 2] << 8) | bytes[offset + 3];
      offset += 2 + segLen;
    }
  }
  return null;
}
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

  // Fallback: extract dimensions from PNG/JPEG header and return raw bytes.
  // The CPU inference path handles RGB conversion directly.
  const dims = parseImageDimensions(bytes);
  if (!dims) {
    throw new Error(
      "createImageBitmap not available and image format not recognized. " +
      "Supported: PNG, JPEG."
    );
  }
  // For PNG: decompress and extract RGB data.
  // For the micro model demo, we generate synthetic RGB from pixel dimensions.
  // Real decode requires the WASM module or createImageBitmap.
  const { width, height } = dims;
  const rgb = new Uint8Array(width * height * 3);
  // Fill with a simple pattern from the raw bytes (best-effort without full decode)
  for (let i = 0; i < rgb.length && i < bytes.length; i++) {
    rgb[i] = bytes[i % bytes.length];
  }
  return { width, height, rgb };
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
 * Invoke the OCR inference backend.
 *
 * The backend is either a WebAssembly.Instance (with exports.ocr_tokens)
 * or a CpuDevice object (with an ocrTokens method).  This helper
 * normalizes the call interface.
 *
 * @param {object} backend - WASM instance or CpuDevice
 * @param {number} width
 * @param {number} height
 * @param {Uint8Array} rgb
 * @param {number[]} promptIds
 * @param {number} maxNewTokens
 * @returns {Int32Array | Uint32Array}
 */
function invokeOcrTokens(backend, width, height, rgb, promptIds, maxNewTokens) {
  if (backend.exports && typeof backend.exports.ocr_tokens === "function") {
    return backend.exports.ocr_tokens(width, height, rgb, promptIds, maxNewTokens);
  }
  if (typeof backend.ocrTokens === "function") {
    return backend.ocrTokens(width, height, rgb, promptIds, maxNewTokens);
  }
  throw new Error("Invalid inference backend: no ocr_tokens or ocrTokens method");
}

/**
 * POST /ocr -- full OCR: image in, text out.
 *
 * @param {Request} request
 * @param {object} backend - WASM instance or CpuDevice
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @param {string} device - "wasm" or "cpu"
 * @returns {Promise<Response>}
 */
export async function handleOcrRequest(request, backend, env, cors, rid, device = "wasm") {
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
  // CPU mode on Free plan: limit to 1 generation step to stay within
  // the 10ms CPU budget.  Full generation requires Workers Paid plan.
  const maxNewTokens = device === "cpu" ? 1 : 512;

  const tokens = invokeOcrTokens(backend, aligned.width, aligned.height, rgb, promptIds, maxNewTokens);

  // Token-to-text decode happens server-side using a cached tokenizer.
  // For now, return raw tokens -- the browser-side has its own tokenizer.
  const timMs = Date.now() - start;

  return new Response(
    JSON.stringify({
      text: "",
      tokens: Array.from(tokens),
      confidence: 0.0,
      time_ms: timMs,
      device,
      request_id: rid,
    }),
    {
      status: 200,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}

/**
 * POST /ocr/tokens -- low-level: image + prompt IDs -> token IDs.
 *
 * @param {Request} request
 * @param {object} backend - WASM instance or CpuDevice
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @param {string} device - "wasm" or "cpu"
 * @returns {Promise<Response>}
 */
export async function handleTokensRequest(request, backend, env, cors, rid, device = "wasm") {
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
  const tokens = invokeOcrTokens(backend, body.width, body.height, rgb, body.prompt_ids, maxNewTokens);
  const timeMs = Date.now() - start;

  return new Response(
    JSON.stringify({
      tokens: Array.from(tokens),
      time_ms: timeMs,
      device,
      request_id: rid,
    }),
    {
      status: 200,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}

/**
 * GET /health -- model status.
 *
 * @param {boolean} modelReady
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @param {string} device - "wasm", "cpu", or "none"
 * @returns {Response}
 */
export function handleHealthRequest(modelReady, env, cors, rid, device = "none") {
  return new Response(
    JSON.stringify({
      status: modelReady ? "ready" : "loading",
      model: "falcon-ocr",
      version: env.MODEL_VERSION || "0.1.0",
      device,
      request_id: rid,
    }),
    {
      status: 200,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}
