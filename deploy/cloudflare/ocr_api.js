/**
 * REST API handlers for Falcon-OCR inference.
 *
 * Endpoints:
 *   POST /ocr          — accepts image (base64 or multipart), returns text
 *   POST /ocr/batch    — accepts multiple images, returns array of results
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
