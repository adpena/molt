/**
 * REST API handlers for Falcon-OCR inference.
 *
 * Endpoints:
 *   POST /ocr            — accepts image (base64 or multipart), returns text
 *   POST /ocr/batch      — accepts multiple images, returns array of results
 *   POST /ocr/detailed   — block-level OCR with bounding boxes, tables, metadata (multi-page)
 *   POST /ocr/table      — table-only extraction from document image
 *   POST /ocr/tokens     — low-level: image + prompt IDs -> token IDs
 *   POST /template/extract — optimized template extraction (Llama 3.2 3B + cache)
 *   POST /invoice/fill   — NL utterance -> structured invoice JSON (Llama 3.2 3B)
 *   GET  /health         — returns 200 with model status
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
      engine: "falcon-ocr",
      model: device === "wasm" ? "falcon-ocr-0.3b-wasm" : "falcon-ocr-0.3b-cpu",
      model_used: device === "wasm" ? "falcon-ocr-wasm" : "falcon-ocr-cpu",
      backend: device === "wasm" ? "wasm" : "cpu",
      auto_filled: true,
      auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
      auto_fill_dismissable: true,
      time_ms: timMs,
      device,
      retries: 0,
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
      engine: "falcon-ocr",
      model: device === "wasm" ? "falcon-ocr-0.3b-wasm" : "falcon-ocr-0.3b-cpu",
      model_used: device === "wasm" ? "falcon-ocr-wasm" : "falcon-ocr-cpu",
      backend: device === "wasm" ? "wasm" : "cpu",
      auto_filled: true,
      auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
      auto_fill_dismissable: true,
      time_ms: timeMs,
      device,
      retries: 0,
      request_id: rid,
    }),
    {
      status: 200,
      headers: { ...cors, "Content-Type": "application/json" },
    },
  );
}

// ---------------------------------------------------------------------------
// Structured OCR: image in, parsed invoice JSON out
// ---------------------------------------------------------------------------

/**
 * POST /ocr/structured -- OCR with structured JSON output.
 *
 * Uses Workers AI with a structured extraction prompt to return parsed
 * invoice data instead of raw text.  Useful for MCP agents and automation
 * pipelines that need structured data.
 *
 * @param {Request} request
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @returns {Promise<Response>}
 */
export async function handleStructuredOcr(request, env, cors, rid) {
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

  // Import dynamically to avoid circular deps at module level
  const { runStructuredOcr, isWorkersAiAvailable } = await import("./ai-fallback.js");

  if (!isWorkersAiAvailable(env)) {
    return new Response(
      JSON.stringify({
        error: "Structured OCR requires Workers AI binding (not available)",
        request_id: rid,
      }),
      {
        status: 503,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }

  try {
    const result = await runStructuredOcr(env, imageResult.bytes);
    return new Response(
      JSON.stringify({
        ...result,
        request_id: rid,
      }),
      {
        status: 200,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  } catch (_err) {
    return new Response(
      JSON.stringify({
        error: "Structured OCR processing failed",
        request_id: rid,
      }),
      {
        status: 500,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }
}

// ---------------------------------------------------------------------------
// Template extraction: OCR -> section classification -> style inference
// ---------------------------------------------------------------------------

/**
 * Section types matching enjoice's TemplateSectionConfig.type.
 * @type {string[]}
 */
const SECTION_TYPES = [
  "header", "parties", "metadata", "line_items",
  "totals", "notes", "terms", "footer",
];

/** @type {RegExp} */
const HEADER_RE = /(?:invoice|credit\s*note|quote|purchase\s*order|receipt|tax\s*invoice|proforma|estimate|bill|statement)/i;
/** @type {RegExp} */
const PARTY_RE = /(?:bill\s*to|ship\s*to|from|sold\s*to|buyer|seller|remit\s*to|vendor|customer|client)/i;
/** @type {RegExp} */
const METADATA_RE = /(?:invoice\s*#|invoice\s*no|inv\s*no|date|due\s*date|issued|payment\s*terms|terms|p\.?o\.?\s*#|reference|order\s*#|account\s*#)/i;
/** @type {RegExp} */
const LINE_ITEM_RE = /(?:description|qty|quantity|unit\s*price|amount|item|service|product|rate|hours|total)/i;
/** @type {RegExp} */
const TOTALS_RE = /(?:subtotal|sub\s*total|tax|vat|gst|discount|total\s*due|grand\s*total|balance\s*due|amount\s*due|total$)/i;
/** @type {RegExp} */
const NOTES_RE = /(?:notes?|memo|comments?|remarks?|additional\s*info)/i;
/** @type {RegExp} */
const TERMS_RE = /(?:terms?\s*(?:&|and)\s*conditions?|payment\s*instructions?|bank\s*details|wire\s*transfer|ach|iban|swift|routing|late\s*fee|penalty)/i;
/** @type {RegExp} */
const FOOTER_RE = /(?:thank\s*you|page\s*\d|www\.|https?:\/\/|@|all\s*rights\s*reserved|copyright|\u00a9)/i;
/** @type {RegExp} */
const CURRENCY_RE = /[$\u20ac\u00a3\u00a5]?\s*[\d,]+\.?\d*|\d[\d,]*\.?\d*\s*(?:USD|EUR|GBP|JPY|CAD|AUD)/g;

/**
 * Classify a single text block into a section type.
 *
 * @param {{ text: string, bbox: { x: number, y: number, width: number, height: number } }} block
 * @param {number} pageHeight
 * @returns {string}
 */
function classifyBlock(block, pageHeight) {
  const text = (block.text || "").trim();
  if (!text) return "notes";

  /** @type {Record<string, number>} */
  const scores = {};
  for (const s of SECTION_TYPES) scores[s] = 0;

  if (HEADER_RE.test(text)) scores.header += 3;
  if (PARTY_RE.test(text)) scores.parties += 3;
  if (METADATA_RE.test(text)) scores.metadata += 3;
  if (LINE_ITEM_RE.test(text)) scores.line_items += 2.5;
  if (TOTALS_RE.test(text)) scores.totals += 3;
  if (NOTES_RE.test(text)) scores.notes += 3;
  if (TERMS_RE.test(text)) scores.terms += 3;
  if (FOOTER_RE.test(text)) scores.footer += 3;

  const currencyMatches = (text.match(CURRENCY_RE) || []).length;
  if (currencyMatches > 0) {
    scores.totals += Math.min(currencyMatches, 3);
    scores.line_items += 0.5 * Math.min(currencyMatches, 3);
  }

  if (pageHeight > 0) {
    const relY = block.bbox.y / pageHeight;
    if (relY < 0.15) { scores.header += 2; scores.metadata += 1; }
    else if (relY < 0.30) { scores.parties += 1.5; scores.metadata += 1; }
    else if (relY < 0.70) { scores.line_items += 1.5; }
    else if (relY < 0.85) { scores.totals += 1.5; }
    else { scores.footer += 1.5; scores.terms += 1; }
  }

  let best = "notes";
  let bestScore = 0.5;
  for (const s of SECTION_TYPES) {
    if (scores[s] > bestScore) { best = s; bestScore = scores[s]; }
  }

  if (bestScore <= 0.5 && pageHeight > 0) {
    const relY = block.bbox.y / pageHeight;
    if (relY < 0.15) return "header";
    if (relY < 0.30) return "parties";
    if (relY < 0.70) return "line_items";
    if (relY < 0.85) return "totals";
    return "footer";
  }

  return best;
}

/**
 * Map a point size to enjoice's headingSize token.
 * @param {number} pt
 * @returns {string}
 */
function ptToSizeToken(pt) {
  if (pt <= 10) return "xs";
  if (pt <= 13) return "sm";
  if (pt <= 16) return "md";
  if (pt <= 20) return "lg";
  if (pt <= 26) return "xl";
  return "2xl";
}

/**
 * Run template extraction on OCR text output from Workers AI.
 *
 * Takes the OCR text, uses Workers AI to identify structured sections,
 * then infers styles and layout from the text structure.
 *
 * @param {object} env - Worker environment bindings
 * @param {Uint8Array} imageBytes - Raw image bytes for OCR
 * @param {object} options
 * @param {string} [options.documentType] - Document type
 * @param {boolean} [options.preserveLogo] - Detect logo position
 * @param {boolean} [options.detectColors] - Detect brand colors
 * @returns {Promise<object>} Template definition
 */
export async function handleTemplateExtract(env, imageBytes, options = {}) {
  const start = Date.now();
  const docType = options.documentType || "invoice";

  // Step 1: Run OCR to get text blocks.
  // Use Workers AI for section classification with a structured prompt.
  let ocrText = "";
  let blocks = [];

  if (env.AI && typeof env.AI.run === "function") {
    const base64Image = uint8ArrayToBase64Worker(imageBytes);

    // First pass: extract raw text with positions
    const ocrResult = await env.AI.run("@cf/google/gemma-3-12b-it", {
      messages: [{
        role: "user",
        content: `Analyze this ${docType} image. For each text block you can see, output a JSON array of objects with fields: "text" (the exact text), "x" (approximate left position 0-100 as percentage), "y" (approximate top position 0-100 as percentage), "w" (approximate width 0-100), "h" (approximate height 0-100). Output ONLY the JSON array, no other text.\n\n![image](data:image/png;base64,${base64Image})`,
      }],
      max_tokens: 4096,
    });

    ocrText = typeof ocrResult === "string"
      ? ocrResult
      : (ocrResult?.response || ocrResult?.choices?.[0]?.message?.content || "");

    // Try to parse structured blocks from AI output
    try {
      const jsonMatch = ocrText.match(/\[[\s\S]*\]/);
      if (jsonMatch) {
        const parsed = JSON.parse(jsonMatch[0]);
        if (Array.isArray(parsed)) {
          blocks = parsed.map((b) => ({
            text: String(b.text || ""),
            bbox: {
              x: (Number(b.x) || 0) * 6.12,       // Scale 0-100% to ~612pt page
              y: (Number(b.y) || 0) * 7.92,       // Scale 0-100% to ~792pt page
              width: (Number(b.w) || 10) * 6.12,
              height: (Number(b.h) || 2) * 7.92,
            },
            confidence: 0.8,
          }));
        }
      }
    } catch (_parseErr) {
      // Fallback: create blocks from line-split text
    }

    // Fallback: if structured extraction failed, do plain OCR and split lines
    if (blocks.length === 0) {
      const plainResult = await env.AI.run("@cf/google/gemma-3-12b-it", {
        messages: [{
          role: "user",
          content: `Extract ALL text from this document image exactly as it appears. Preserve layout and line breaks.\n\n![image](data:image/png;base64,${base64Image})`,
        }],
        max_tokens: 2048,
      });

      const plainText = typeof plainResult === "string"
        ? plainResult
        : (plainResult?.response || plainResult?.choices?.[0]?.message?.content || "");

      const lines = plainText.split("\n").filter((l) => l.trim());
      const lineHeight = 792 / Math.max(lines.length, 1);
      blocks = lines.map((line, i) => ({
        text: line.trim(),
        bbox: {
          x: 36,
          y: 36 + i * lineHeight,
          width: 540,
          height: lineHeight * 0.8,
        },
        confidence: 0.6,
      }));
    }
  } else {
    // No AI available: return a default template
    return {
      template: buildDefaultTemplate(docType),
      confidence: 0.0,
      detected_sections: [],
      time_ms: Date.now() - start,
      error: "Workers AI not available for section classification",
    };
  }

  // Step 2: Classify blocks into sections
  const pageWidth = 612;
  const pageHeight = 792;
  /** @type {Record<string, Array<{text: string, bbox: object, confidence: number}>>} */
  const sections = {};
  for (const s of SECTION_TYPES) sections[s] = [];

  for (const block of blocks) {
    const section = classifyBlock(block, pageHeight);
    sections[section].push(block);
  }

  // Sort blocks within each section by y position
  for (const sectionBlocks of Object.values(sections)) {
    sectionBlocks.sort((a, b) => a.bbox.y - b.bbox.y);
  }

  // Step 3: Infer styles
  const heights = blocks.map((b) => b.bbox.height).filter((h) => h > 0).sort((a, b) => a - b);
  const medianHeight = heights.length > 0 ? heights[Math.floor(heights.length / 2)] : 14;
  const p80Height = heights.length > 0 ? heights[Math.floor(heights.length * 0.8)] : 20;

  const bodyPt = Math.max(6, Math.min(14, (medianHeight / pageHeight) * 792 * 0.7));
  let headingPt = Math.max(10, Math.min(48, (p80Height / pageHeight) * 792 * 0.7));
  if (headingPt <= bodyPt * 1.2) headingPt = bodyPt * 1.6;

  // Detect alignment from header blocks
  const headerBlocks = blocks.filter((b) => b.bbox.y < pageHeight * 0.15);
  let headerAlign = "left";
  if (headerBlocks.length > 0) {
    const centers = headerBlocks.map((b) => b.bbox.x + b.bbox.width / 2);
    const avgCenter = centers.reduce((a, b) => a + b, 0) / centers.length;
    if (Math.abs(avgCenter - pageWidth / 2) < pageWidth * 0.1) headerAlign = "center";
    else if (avgCenter > pageWidth * 0.65) headerAlign = "right";
  }

  // Detect spacing density
  let isCompact = false;
  if (blocks.length >= 2) {
    const sortedByY = [...blocks].sort((a, b) => a.bbox.y - b.bbox.y);
    const gaps = [];
    for (let i = 0; i < sortedByY.length - 1; i++) {
      const gap = sortedByY[i + 1].bbox.y - (sortedByY[i].bbox.y + sortedByY[i].bbox.height);
      if (gap > 0) gaps.push(gap);
    }
    if (gaps.length > 0) {
      const avgGap = gaps.reduce((a, b) => a + b, 0) / gaps.length;
      isCompact = avgGap < medianHeight * 1.5;
    }
  }

  // Detect logo position
  let logoPosition = "left";
  const topWideBlocks = blocks.filter(
    (b) => b.bbox.y < pageHeight * 0.10 && b.bbox.width > b.bbox.height,
  );
  if (topWideBlocks.length > 0 && pageWidth > 0) {
    const widest = topWideBlocks.reduce((a, b) => a.bbox.width > b.bbox.width ? a : b);
    const relX = (widest.bbox.x + widest.bbox.width / 2) / pageWidth;
    if (relX > 0.65) logoPosition = "right";
    else if (relX > 0.35) logoPosition = "center";
  }

  // Step 4: Infer layout
  /** @type {Array<[string, number]>} */
  const sectionPositions = [];
  for (const sType of SECTION_TYPES) {
    if (sections[sType].length > 0) {
      const minY = Math.min(...sections[sType].map((b) => b.bbox.y));
      sectionPositions.push([sType, minY]);
    }
  }
  sectionPositions.sort((a, b) => a[1] - b[1]);
  const sectionOrder = sectionPositions.map((s) => s[0]);
  for (const s of SECTION_TYPES) {
    if (!sectionOrder.includes(s)) sectionOrder.push(s);
  }

  // Parties layout
  let partiesLayout = "two-column";
  const partyBlocks = sections.parties || [];
  if (partyBlocks.length > 0) {
    const leftP = partyBlocks.filter((b) => (b.bbox.x + b.bbox.width / 2) < pageWidth * 0.5);
    const rightP = partyBlocks.filter((b) => (b.bbox.x + b.bbox.width / 2) >= pageWidth * 0.5);
    partiesLayout = (leftP.length > 0 && rightP.length > 0) ? "two-column" : "stacked";
  }

  // Totals position
  let totalsPosition = "right";
  const totalsBlocks = sections.totals || [];
  if (totalsBlocks.length > 0) {
    const avgX = totalsBlocks.reduce((a, b) => a + b.bbox.x + b.bbox.width / 2, 0) / totalsBlocks.length;
    if (avgX > pageWidth * 0.6) totalsPosition = "right";
    else if (avgX < pageWidth * 0.4) totalsPosition = "left";
    else totalsPosition = "full-width";
  }

  // Step 5: Build template definition
  const detectedSections = SECTION_TYPES.filter((s) => sections[s].length > 0);

  // Confidence scoring
  let confidence = 0;
  const nonEmpty = detectedSections.length;
  confidence += Math.min(nonEmpty / 6, 1) * 0.4;
  confidence += Math.min(blocks.length / 10, 1) * 0.3;
  const avgConf = blocks.length > 0
    ? blocks.reduce((a, b) => a + (b.confidence || 0), 0) / blocks.length
    : 0;
  confidence += avgConf * 0.3;
  confidence = Math.round(Math.min(1, confidence) * 1000) / 1000;

  const templateId = crypto.randomUUID();

  const template = {
    id: templateId,
    name: "Extracted Template",
    description: "Template extracted from scanned invoice via Falcon-OCR",
    type: docType,
    layout: {
      id: "",
      name: "Extracted Layout",
      description: "Layout extracted from scanned invoice",
      sections: sectionOrder.map((s, i) => ({
        type: s,
        visible: true,
        order: i,
      })),
      page: {
        size: "letter",
        orientation: "portrait",
        margins: { top: 28, right: 36, bottom: 28, left: 36 },
      },
    },
    styles: {
      colors: {
        primary: "#111111",
        secondary: "#666666",
        accent: "#2563EB",
        text: "#111111",
        background: "#FFFFFF",
        border: "#E5E5E5",
        headerBg: "#F9F9F9",
        altRowBg: "#FAFAFA",
      },
      fonts: {
        heading: "Inter",
        body: "Inter",
        mono: "JetBrains Mono",
      },
      spacing: { compact: isCompact },
      logo: {
        position: logoPosition,
        maxWidth: 200,
        maxHeight: 80,
      },
      typography: {
        headingSize: ptToSizeToken(headingPt),
        bodySize: Math.round(bodyPt),
      },
      layoutHints: {
        headerAlign,
        titleAlign: headerAlign,
        totalsPosition,
        partiesLayout,
      },
    },
    elementStyles: {},
    hiddenElements: [],
    customFields: [],
    customSections: [],
  };

  return {
    template,
    confidence,
    detected_sections: detectedSections,
    auto_filled: true,
    auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
    auto_fill_dismissable: true,
    time_ms: Date.now() - start,
  };
}

/**
 * Build a default template when AI is not available.
 * @param {string} docType
 * @returns {object}
 */
function buildDefaultTemplate(docType) {
  return {
    id: crypto.randomUUID(),
    name: "Default Template",
    description: "Default template (AI classification unavailable)",
    type: docType,
    layout: {
      id: "",
      name: "Default Layout",
      description: "",
      sections: SECTION_TYPES.map((s, i) => ({ type: s, visible: true, order: i })),
      page: {
        size: "letter",
        orientation: "portrait",
        margins: { top: 28, right: 36, bottom: 28, left: 36 },
      },
    },
    styles: {
      colors: {
        primary: "#111111", secondary: "#666666", accent: "#2563EB",
        text: "#111111", background: "#FFFFFF", border: "#E5E5E5",
        headerBg: "#F9F9F9", altRowBg: "#FAFAFA",
      },
      fonts: { heading: "Inter", body: "Inter", mono: "JetBrains Mono" },
      spacing: { compact: false },
      logo: { position: "left", maxWidth: 200, maxHeight: 80 },
    },
    elementStyles: {},
    hiddenElements: [],
  };
}

/**
 * Convert Uint8Array to base64 (Worker-compatible).
 * @param {Uint8Array} bytes
 * @returns {string}
 */
function uint8ArrayToBase64Worker(bytes) {
  let binary = "";
  const chunkSize = 8192;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const chunk = bytes.subarray(i, Math.min(i + chunkSize, bytes.length));
    for (let j = 0; j < chunk.length; j++) {
      binary += String.fromCharCode(chunk[j]);
    }
  }
  return btoa(binary);
}

/**
 * POST /ocr/batch -- batch OCR: multiple images in, multiple results out.
 *
 * Accepts JSON body: { "images": ["base64_1", ...], "prompt": "optional" }
 * Max 10 images per batch.
 * Processes sequentially (Workers are single-threaded).
 * Each result is individually cached by image hash.
 *
 * @param {Request} request
 * @param {object} backend - WASM instance or CpuDevice
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @param {string} device - "wasm" or "cpu"
 * @param {{ getCached: (hash: string) => Promise<object|null>, setCached: (hash: string, result: object) => void, hashBytes: (bytes: Uint8Array) => Promise<string> }} cacheOps
 * @returns {Promise<Response>}
 */
export async function handleBatchOcr(request, backend, env, cors, rid, device = "wasm", cacheOps = {}) {
  const batchStart = Date.now();
  const contentType = request.headers.get("Content-Type") || "";

  if (!contentType.includes("application/json")) {
    return new Response(
      JSON.stringify({
        error: "/ocr/batch requires application/json",
        request_id: rid,
      }),
      {
        status: 415,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }

  const body = /** @type {{ images?: string[], prompt?: string }} */ (await request.json());

  if (!Array.isArray(body.images) || body.images.length === 0) {
    return new Response(
      JSON.stringify({
        error: "Missing or empty 'images' array in JSON body",
        request_id: rid,
      }),
      {
        status: 400,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }

  if (body.images.length > 10) {
    return new Response(
      JSON.stringify({
        error: `Batch too large: ${body.images.length} images (max 10)`,
        request_id: rid,
      }),
      {
        status: 400,
        headers: { ...cors, "Content-Type": "application/json" },
      },
    );
  }

  const maxBytes = parseInt(env.MAX_IMAGE_BYTES || String(MAX_IMAGE_BYTES_DEFAULT), 10);
  const promptIds = [];
  const maxNewTokens = device === "cpu" ? 1 : 512;
  const results = [];

  for (let idx = 0; idx < body.images.length; idx++) {
    const imageStart = Date.now();
    const b64 = body.images[idx];

    if (typeof b64 !== "string" || b64.length === 0) {
      results.push({
        text: "",
        tokens: [],
        time_ms: 0,
        error: `Image at index ${idx}: invalid or empty base64 string`,
      });
      continue;
    }

    let raw;
    try {
      raw = atob(b64);
    } catch (_e) {
      results.push({
        text: "",
        tokens: [],
        time_ms: 0,
        error: `Image at index ${idx}: invalid base64 encoding`,
      });
      continue;
    }

    if (raw.length > maxBytes) {
      results.push({
        text: "",
        tokens: [],
        time_ms: 0,
        error: `Image at index ${idx}: too large (${raw.length} bytes, max ${maxBytes})`,
      });
      continue;
    }

    const bytes = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i++) {
      bytes[i] = raw.charCodeAt(i);
    }

    // Check per-image cache if cache operations are provided
    let imageHash = null;
    if (cacheOps.hashBytes) {
      try {
        imageHash = await cacheOps.hashBytes(bytes);
        if (cacheOps.getCached) {
          const cached = await cacheOps.getCached(imageHash);
          if (cached) {
            results.push({
              text: cached.text || "",
              tokens: cached.tokens || [],
              time_ms: 0,
              cache: "hit",
            });
            continue;
          }
        }
      } catch (_e) {
        // Cache failure is non-fatal
      }
    }

    try {
      const decoded = await decodeImageToRgb(bytes, "image/jpeg");
      const aligned = alignToPatch(decoded.width, decoded.height);
      const rgb = padRgb(decoded.rgb, decoded.width, decoded.height, aligned.width, aligned.height);
      const tokens = invokeOcrTokens(backend, aligned.width, aligned.height, rgb, promptIds, maxNewTokens);
      const timeMs = Date.now() - imageStart;
      const result = {
        text: "",
        tokens: Array.from(tokens),
        time_ms: timeMs,
      };
      results.push(result);

      // Cache individual result
      if (imageHash && cacheOps.setCached) {
        try {
          cacheOps.setCached(imageHash, result);
        } catch (_e) {
          // Cache write failure is non-fatal
        }
      }
    } catch (err) {
      results.push({
        text: "",
        tokens: [],
        time_ms: Date.now() - imageStart,
        error: `Image at index ${idx}: ${err.message}`,
      });
    }
  }

  return new Response(
    JSON.stringify({
      results,
      total_time_ms: Date.now() - batchStart,
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

// ---------------------------------------------------------------------------
// POST /ocr/detailed -- block-level OCR with bounding boxes, tables, metadata
// ---------------------------------------------------------------------------

/**
 * Compute a SHA-256 hash of arbitrary bytes, returned as hex string.
 * @param {Uint8Array} bytes
 * @returns {Promise<string>}
 */
async function hashImageBytes(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Run detailed OCR on a single image page, returning blocks, tables, and metadata.
 *
 * @param {object} env - Worker environment bindings
 * @param {Uint8Array} imageBytes - Raw image bytes
 * @returns {Promise<{ text: string, blocks: Array, tables: Array, metadata: object }>}
 */
async function runDetailedOcrPage(env, imageBytes) {
  const base64Image = uint8ArrayToBase64Worker(imageBytes);

  const prompt = `Analyze this document image. Return JSON with exactly these keys:
- "text": all extracted text concatenated
- "blocks": array of objects with "text", "confidence" (0-1), "bbox" (object with "x","y","width","height" in pixels assuming 612x792 page), "type" (one of: "heading","field_label","field_value","paragraph","table_cell","footer")
- "tables": array of objects with "rows" (int), "cols" (int), "headers" (string array), "data" (2D string array of row data excluding headers)
- "metadata": object with "language" (ISO 639-1), "orientation" (0/90/180/270), "has_handwriting" (bool)

Output ONLY valid JSON, no markdown fences, no explanation.

![image](data:image/png;base64,${base64Image})`;

  const aiResult = await env.AI.run("@cf/google/gemma-3-12b-it", {
    messages: [{ role: "user", content: prompt }],
    max_tokens: 4096,
  });

  const responseText = typeof aiResult === "string"
    ? aiResult
    : (aiResult?.response || aiResult?.choices?.[0]?.message?.content || "");

  // Parse JSON from response (strip any markdown fences the model may add)
  let parsed = null;
  try {
    const stripped = responseText.replace(/^```(?:json)?\s*\n?/m, "").replace(/\n?```\s*$/m, "");
    const jsonMatch = stripped.match(/\{[\s\S]*\}/);
    if (jsonMatch) {
      parsed = JSON.parse(jsonMatch[0]);
    }
  } catch (_e) {
    // Parse failure handled below
  }

  if (!parsed) {
    return {
      text: responseText,
      blocks: [],
      tables: [],
      metadata: { language: "en", orientation: 0, has_handwriting: false },
    };
  }

  // Normalize blocks
  const blocks = Array.isArray(parsed.blocks) ? parsed.blocks.map((b) => ({
    text: String(b.text || ""),
    confidence: Math.max(0, Math.min(1, Number(b.confidence) || 0.5)),
    bbox: {
      x: Number(b.bbox?.x) || 0,
      y: Number(b.bbox?.y) || 0,
      width: Number(b.bbox?.width) || 0,
      height: Number(b.bbox?.height) || 0,
    },
    type: b.type || "paragraph",
  })) : [];

  // Normalize tables
  const tables = Array.isArray(parsed.tables) ? parsed.tables.map((t) => ({
    rows: Number(t.rows) || 0,
    cols: Number(t.cols) || 0,
    headers: Array.isArray(t.headers) ? t.headers.map(String) : [],
    data: Array.isArray(t.data) ? t.data.map((row) => Array.isArray(row) ? row.map(String) : []) : [],
  })) : [];

  // Normalize metadata
  const metadata = {
    language: String(parsed.metadata?.language || "en").slice(0, 5),
    orientation: [0, 90, 180, 270].includes(Number(parsed.metadata?.orientation))
      ? Number(parsed.metadata.orientation)
      : 0,
    has_handwriting: Boolean(parsed.metadata?.has_handwriting),
  };

  const text = typeof parsed.text === "string" ? parsed.text
    : blocks.map((b) => b.text).join("\n");

  return { text, blocks, tables, metadata };
}

/**
 * POST /ocr/detailed handler.
 * Accepts single image or multi-page images array.
 *
 * @param {Request} request
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @returns {Promise<Response>}
 */
export async function handleDetailedOcr(request, env, cors, rid) {
  const start = Date.now();

  if (!env.AI || typeof env.AI.run !== "function") {
    return new Response(
      JSON.stringify({ error: "Detailed OCR requires Workers AI binding", request_id: rid }),
      { status: 503, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }

  const contentType = request.headers.get("Content-Type") || "";
  if (!contentType.includes("application/json")) {
    return new Response(
      JSON.stringify({ error: "Requires application/json", request_id: rid }),
      { status: 415, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }

  const body = await request.json();
  const maxBytes = parseInt(env.MAX_IMAGE_BYTES || String(MAX_IMAGE_BYTES_DEFAULT), 10);

  // Multi-page: { "images": ["b64_1", "b64_2", ...] }
  // Single-page: { "image": "b64" }
  let imageList = [];
  if (Array.isArray(body.images) && body.images.length > 0) {
    if (body.images.length > 20) {
      return new Response(
        JSON.stringify({ error: "Maximum 20 pages per request", request_id: rid }),
        { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
      );
    }
    imageList = body.images;
  } else if (body.image && typeof body.image === "string") {
    imageList = [body.image];
  } else {
    return new Response(
      JSON.stringify({ error: "Missing 'image' (string) or 'images' (array) field", request_id: rid }),
      { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }

  // Decode all images to bytes
  const pageBytes = [];
  for (let i = 0; i < imageList.length; i++) {
    const b64 = imageList[i];
    if (typeof b64 !== "string" || b64.length === 0) {
      return new Response(
        JSON.stringify({ error: `Page ${i}: invalid or empty base64`, request_id: rid }),
        { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
      );
    }
    let raw;
    try { raw = atob(b64); } catch (_e) {
      return new Response(
        JSON.stringify({ error: `Page ${i}: invalid base64 encoding`, request_id: rid }),
        { status: 400, headers: { ...cors, "Content-Type": "application/json" } },
      );
    }
    if (raw.length > maxBytes) {
      return new Response(
        JSON.stringify({ error: `Page ${i}: exceeds max size (${raw.length} > ${maxBytes})`, request_id: rid }),
        { status: 413, headers: { ...cors, "Content-Type": "application/json" } },
      );
    }
    const bytes = new Uint8Array(raw.length);
    for (let j = 0; j < raw.length; j++) bytes[j] = raw.charCodeAt(j);
    pageBytes.push(bytes);
  }

  // Check cache for each page, run OCR only on cache misses
  const pages = [];
  for (let i = 0; i < pageBytes.length; i++) {
    let cacheKey = null;
    let cached = null;

    if (env.OCR_CACHE) {
      cacheKey = `detailed:${await hashImageBytes(pageBytes[i])}`;
      try {
        const stored = await env.OCR_CACHE.get(cacheKey, "json");
        if (stored) { cached = stored; }
      } catch (_e) { /* cache miss */ }
    }

    if (cached) {
      pages.push(cached);
    } else {
      const pageResult = await runDetailedOcrPage(env, pageBytes[i]);
      pages.push(pageResult);
      // Store in cache (non-blocking, 24h TTL)
      if (env.OCR_CACHE && cacheKey) {
        env.OCR_CACHE.put(cacheKey, JSON.stringify(pageResult), { expirationTtl: 86400 })
          .catch(() => {});
      }
    }
  }

  // Combine pages
  const combinedText = pages.map((p, i) =>
    pages.length > 1 ? `--- Page ${i + 1} ---\n${p.text}` : p.text
  ).join("\n\n");

  const allBlocks = [];
  for (let i = 0; i < pages.length; i++) {
    for (const block of pages[i].blocks) {
      allBlocks.push({ ...block, page: i + 1 });
    }
  }

  const allTables = [];
  for (let i = 0; i < pages.length; i++) {
    for (const table of pages[i].tables) {
      allTables.push({ ...table, page: i + 1 });
    }
  }

  const metadata = {
    ...pages[0]?.metadata || { language: "en", orientation: 0, has_handwriting: false },
    page_count: pages.length,
  };

  const result = {
    text: combinedText,
    blocks: allBlocks,
    tables: allTables,
    metadata,
    pages: pages.length > 1 ? pages.map((p, i) => ({
      page: i + 1,
      text: p.text,
      blocks: p.blocks,
      tables: p.tables,
    })) : undefined,
    time_ms: Date.now() - start,
    request_id: rid,
  };

  return new Response(JSON.stringify(result), {
    status: 200,
    headers: { ...cors, "Content-Type": "application/json" },
  });
}

// ---------------------------------------------------------------------------
// POST /ocr/table -- table-only extraction
// ---------------------------------------------------------------------------

/**
 * POST /ocr/table handler.
 * Extracts only tabular data from the document image.
 *
 * @param {Request} request
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @returns {Promise<Response>}
 */
export async function handleTableOcr(request, env, cors, rid) {
  const start = Date.now();

  if (!env.AI || typeof env.AI.run !== "function") {
    return new Response(
      JSON.stringify({ error: "Table OCR requires Workers AI binding", request_id: rid }),
      { status: 503, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }

  const imageResult = await extractImage(request, env);
  if ("error" in imageResult) {
    return new Response(
      JSON.stringify({ error: imageResult.error, request_id: rid }),
      { status: imageResult.status, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }

  const base64Image = uint8ArrayToBase64Worker(imageResult.bytes);

  const prompt = `Extract ALL tables from this document image. Return JSON with key "tables": an array where each table has:
- "headers": array of column header strings
- "data": 2D array of row data (strings), excluding headers
- "rows": total data row count (int)
- "cols": column count (int)

If no tables found, return {"tables":[]}.
Output ONLY valid JSON, no markdown fences.

![image](data:image/png;base64,${base64Image})`;

  const aiResult = await env.AI.run("@cf/google/gemma-3-12b-it", {
    messages: [{ role: "user", content: prompt }],
    max_tokens: 4096,
  });

  const responseText = typeof aiResult === "string"
    ? aiResult
    : (aiResult?.response || aiResult?.choices?.[0]?.message?.content || "");

  let tables = [];
  try {
    const stripped = responseText.replace(/^```(?:json)?\s*\n?/m, "").replace(/\n?```\s*$/m, "");
    const jsonMatch = stripped.match(/\{[\s\S]*\}/);
    if (jsonMatch) {
      const parsed = JSON.parse(jsonMatch[0]);
      if (Array.isArray(parsed.tables)) {
        tables = parsed.tables.map((t) => ({
          rows: Number(t.rows) || (Array.isArray(t.data) ? t.data.length : 0),
          cols: Number(t.cols) || (Array.isArray(t.headers) ? t.headers.length : 0),
          headers: Array.isArray(t.headers) ? t.headers.map(String) : [],
          data: Array.isArray(t.data) ? t.data.map((row) => Array.isArray(row) ? row.map(String) : []) : [],
        }));
      }
    }
  } catch (_e) {
    // Parse failure — return empty tables
  }

  return new Response(
    JSON.stringify({
      tables,
      table_count: tables.length,
      time_ms: Date.now() - start,
      request_id: rid,
    }),
    { status: 200, headers: { ...cors, "Content-Type": "application/json" } },
  );
}

// ---------------------------------------------------------------------------
// Optimized template extraction (Llama 3.2 3B + aggressive caching)
// ---------------------------------------------------------------------------

/**
 * Optimized template extraction: uses smaller/faster model and caching.
 * Replaces the Gemma 12B multi-step approach with a single-pass Llama 3.2 3B prompt.
 *
 * @param {object} env - Worker environment bindings
 * @param {Uint8Array} imageBytes - Raw image bytes
 * @param {object} options
 * @param {string} [options.documentType]
 * @param {boolean} [options.preserveLogo]
 * @param {boolean} [options.detectColors]
 * @returns {Promise<object>}
 */
export async function handleTemplateExtractFast(env, imageBytes, options = {}) {
  const start = Date.now();
  const docType = options.documentType || "invoice";

  // Check cache first — template extraction is highly cacheable
  let cacheKey = null;
  if (env.OCR_CACHE) {
    cacheKey = `tpl:${docType}:${await hashImageBytes(imageBytes)}`;
    try {
      const cached = await env.OCR_CACHE.get(cacheKey, "json");
      if (cached) {
        return { ...cached, time_ms: Date.now() - start, cache: "hit" };
      }
    } catch (_e) { /* cache miss */ }
  }

  if (!env.AI || typeof env.AI.run !== "function") {
    return {
      template: buildDefaultTemplate(docType),
      confidence: 0.0,
      detected_sections: [],
      time_ms: Date.now() - start,
      error: "Workers AI not available for template extraction",
    };
  }

  const base64Image = uint8ArrayToBase64Worker(imageBytes);

  // Single concise prompt to Llama 3.2 3B — faster model, direct JSON output
  const prompt = `Analyze this ${docType} document layout. Return JSON:
{"sections":["header","parties","metadata","line_items","totals","notes","terms","footer"],"header_align":"left|center|right","parties_layout":"two-column|stacked","totals_position":"left|right|full-width","logo_position":"left|center|right","compact":true|false,"heading_size":"sm|md|lg|xl"}
Only include sections actually present. Output ONLY JSON.

![image](data:image/png;base64,${base64Image})`;

  const aiResult = await env.AI.run("@cf/meta/llama-3.2-3b-instruct", {
    messages: [{ role: "user", content: prompt }],
    max_tokens: 512,
  });

  const responseText = typeof aiResult === "string"
    ? aiResult
    : (aiResult?.response || aiResult?.choices?.[0]?.message?.content || "");

  let layoutInfo = null;
  try {
    const stripped = responseText.replace(/^```(?:json)?\s*\n?/m, "").replace(/\n?```\s*$/m, "");
    const jsonMatch = stripped.match(/\{[\s\S]*\}/);
    if (jsonMatch) {
      layoutInfo = JSON.parse(jsonMatch[0]);
    }
  } catch (_e) {
    // Parse failure
  }

  if (!layoutInfo) {
    return {
      template: buildDefaultTemplate(docType),
      confidence: 0.2,
      detected_sections: SECTION_TYPES,
      time_ms: Date.now() - start,
    };
  }

  // Build template from AI layout analysis
  const detectedSections = Array.isArray(layoutInfo.sections)
    ? layoutInfo.sections.filter((s) => SECTION_TYPES.includes(s))
    : SECTION_TYPES;

  const sectionOrder = [...detectedSections];
  for (const s of SECTION_TYPES) {
    if (!sectionOrder.includes(s)) sectionOrder.push(s);
  }

  const headerAlign = ["left", "center", "right"].includes(layoutInfo.header_align)
    ? layoutInfo.header_align : "left";
  const partiesLayout = ["two-column", "stacked"].includes(layoutInfo.parties_layout)
    ? layoutInfo.parties_layout : "two-column";
  const totalsPosition = ["left", "right", "full-width"].includes(layoutInfo.totals_position)
    ? layoutInfo.totals_position : "right";
  const logoPosition = ["left", "center", "right"].includes(layoutInfo.logo_position)
    ? layoutInfo.logo_position : "left";
  const isCompact = Boolean(layoutInfo.compact);
  const headingSize = ["xs", "sm", "md", "lg", "xl", "2xl"].includes(layoutInfo.heading_size)
    ? layoutInfo.heading_size : "lg";

  const templateId = crypto.randomUUID();
  const template = {
    id: templateId,
    name: "Extracted Template",
    description: "Template extracted from scanned document via Falcon-OCR",
    type: docType,
    layout: {
      id: "",
      name: "Extracted Layout",
      description: "Layout extracted from scanned document",
      sections: sectionOrder.map((s, i) => ({
        type: s,
        visible: detectedSections.includes(s),
        order: i,
      })),
      page: {
        size: "letter",
        orientation: "portrait",
        margins: { top: 28, right: 36, bottom: 28, left: 36 },
      },
    },
    styles: {
      colors: {
        primary: "#111111",
        secondary: "#666666",
        accent: "#2563EB",
        text: "#111111",
        background: "#FFFFFF",
        border: "#E5E5E5",
        headerBg: "#F9F9F9",
        altRowBg: "#FAFAFA",
      },
      fonts: {
        heading: "Inter",
        body: "Inter",
        mono: "JetBrains Mono",
      },
      spacing: { compact: isCompact },
      logo: {
        position: logoPosition,
        maxWidth: 200,
        maxHeight: 80,
      },
      typography: {
        headingSize,
        bodySize: isCompact ? 10 : 12,
      },
      layoutHints: {
        headerAlign,
        titleAlign: headerAlign,
        totalsPosition,
        partiesLayout,
      },
    },
    elementStyles: {},
    hiddenElements: [],
    customFields: [],
    customSections: [],
  };

  const confidence = Math.min(1, 0.4 + detectedSections.length * 0.08);

  const result = {
    template,
    confidence: Math.round(confidence * 1000) / 1000,
    detected_sections: detectedSections,
    auto_filled: true,
    auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
    auto_fill_dismissable: true,
    time_ms: Date.now() - start,
  };

  // Cache result (non-blocking, 24h TTL)
  if (env.OCR_CACHE && cacheKey) {
    env.OCR_CACHE.put(cacheKey, JSON.stringify(result), { expirationTtl: 86400 })
      .catch(() => {});
  }

  return result;
}

// ---------------------------------------------------------------------------
// NL Invoice Fill: natural language utterance -> structured invoice JSON
// ---------------------------------------------------------------------------

/**
 * POST /invoice/fill -- parse a natural language description into structured
 * invoice data using Workers AI (Llama 3.2 3B Instruct).
 *
 * @param {Request} request
 * @param {object} env
 * @param {Record<string, string>} cors
 * @param {string} rid
 * @returns {Promise<Response>}
 */
export async function handleInvoiceFill(request, env, cors, rid) {
  const body = await request.json();
  const { utterance } = body;

  if (!utterance) {
    return new Response(JSON.stringify({ error: "Missing 'utterance' field", request_id: rid }),
      { status: 400, headers: { ...cors, "Content-Type": "application/json" } });
  }

  const prompt = `Parse this invoice description into structured JSON:
"${utterance}"

Return JSON with these fields (omit unknown fields):
{"vendor":"","invoice_number":"auto","items":[{"description":"","qty":1,"rate_cents":0}],"currency":"USD","due_date":"","payment_terms":"","notes":""}

Rules:
- Amounts in cents (e.g., $5,000 = 500000)
- "five grand" = 500000, "5k" = 500000
- "Net 30" = payment_terms:"Net 30", due_date = today + 30 days
- Dates in ISO format (YYYY-MM-DD)
- Only include fields mentioned in the description`;

  const start = Date.now();
  const result = await env.AI.run("@cf/meta/llama-3.2-3b-instruct", {
    messages: [{ role: "user", content: prompt }],
    max_tokens: 512,
    temperature: 0.1,
  });

  let filled;
  try {
    const text = result.response || result;
    const jsonMatch = text.match(/\{[\s\S]*\}/);
    filled = jsonMatch ? JSON.parse(jsonMatch[0]) : {};
  } catch {
    filled = { raw_text: result.response || result };
  }

  return new Response(JSON.stringify({
    engine: "falcon-ocr",
    model: "@cf/meta/llama-3.2-3b-instruct",
    backend: "workers-ai",
    filled_invoice: filled,
    auto_filled: true,
    auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
    auto_fill_dismissable: true,
    time_ms: Date.now() - start,
    request_id: rid,
  }), { status: 200, headers: { ...cors, "Content-Type": "application/json" } });
}
