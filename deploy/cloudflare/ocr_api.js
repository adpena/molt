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

import { buildFalconOcrPromptIds } from "./tokenizer.js";

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
 * @returns {Promise<{ bytes: Uint8Array, format: string, maxTokens?: number, category?: string } | { error: string, status: number }>}
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
      category: typeof formData.get("category") === "string" ? String(formData.get("category")) : undefined,
    };
  }

  if (contentType.includes("application/json")) {
    const body = /** @type {{ image?: string, format?: string, max_tokens?: number, category?: string }} */ (await request.json());
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
    const maxTokens = typeof body.max_tokens === "number" ? body.max_tokens : undefined;
    const category = typeof body.category === "string" ? body.category : undefined;
    return { bytes, format, maxTokens, category };
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

  // Fallback: decode PNG inline when createImageBitmap is unavailable.
  // Cloudflare Workers gained createImageBitmap support in 2025-Q3, but
  // we need a working fallback for environments without it.
  const dims = parseImageDimensions(bytes);
  if (!dims) {
    throw new Error(
      "createImageBitmap not available and image format not recognized. " +
      "Supported: PNG, JPEG."
    );
  }

  // Attempt minimal PNG decode for the most common case.
  const isPng = bytes.length >= 8 &&
    bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47;

  if (isPng) {
    const rgb = decodePngToRgb(bytes, dims.width, dims.height);
    if (rgb) {
      return { width: dims.width, height: dims.height, rgb };
    }
  }

  throw new Error(
    "createImageBitmap not available and cannot decode image in pure JS. " +
    "This environment requires createImageBitmap support (Cloudflare Workers 2025-Q3+) " +
    "or pre-decoded RGB data via POST /ocr/tokens."
  );
}

/**
 * Paeth predictor for PNG unfiltering.
 * @param {number} a - left
 * @param {number} b - above
 * @param {number} c - upper-left
 * @returns {number}
 */
function paethPredictor(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  if (pa <= pb && pa <= pc) return a;
  if (pb <= pc) return b;
  return c;
}

/**
 * Synchronous zlib inflate (RFC 1950 / RFC 1951 DEFLATE).
 *
 * Implements a complete Huffman/DEFLATE decoder for PNG IDAT decompression.
 * Handles stored, fixed Huffman, and dynamic Huffman blocks.
 *
 * @param {Uint8Array} data - zlib-wrapped compressed data
 * @returns {Uint8Array | null}
 */
function inflateSync(data) {
  if (data.length < 6) return null;
  if ((data[0] & 0x0f) !== 8) return null; // Must be deflate

  let bitPos = 16; // Skip 2-byte zlib header
  const output = [];

  function readBits(n) {
    let val = 0;
    for (let i = 0; i < n; i++) {
      const byteIdx = bitPos >> 3;
      const bitIdx = bitPos & 7;
      if (byteIdx >= data.length) return 0;
      val |= ((data[byteIdx] >> bitIdx) & 1) << i;
      bitPos++;
    }
    return val;
  }

  function readByte() {
    bitPos = (bitPos + 7) & ~7;
    const byteIdx = bitPos >> 3;
    bitPos += 8;
    return byteIdx < data.length ? data[byteIdx] : 0;
  }

  // Fixed Huffman code length tables
  const FIXED_LIT_LENS = new Uint8Array(288);
  for (let i = 0; i <= 143; i++) FIXED_LIT_LENS[i] = 8;
  for (let i = 144; i <= 255; i++) FIXED_LIT_LENS[i] = 9;
  for (let i = 256; i <= 279; i++) FIXED_LIT_LENS[i] = 7;
  for (let i = 280; i <= 287; i++) FIXED_LIT_LENS[i] = 8;

  const FIXED_DIST_LENS = new Uint8Array(32);
  for (let i = 0; i < 32; i++) FIXED_DIST_LENS[i] = 5;

  function buildHuffmanTable(codeLengths, maxSymbols) {
    let maxBits = 0;
    for (let i = 0; i < maxSymbols; i++) {
      if (codeLengths[i] > maxBits) maxBits = codeLengths[i];
    }
    if (maxBits === 0) return { table: new Map(), maxBits: 1 };

    const blCount = new Uint16Array(maxBits + 1);
    for (let i = 0; i < maxSymbols; i++) {
      if (codeLengths[i] > 0) blCount[codeLengths[i]]++;
    }
    const nextCode = new Uint16Array(maxBits + 1);
    let code = 0;
    for (let bits = 1; bits <= maxBits; bits++) {
      code = (code + blCount[bits - 1]) << 1;
      nextCode[bits] = code;
    }
    const table = new Map();
    for (let sym = 0; sym < maxSymbols; sym++) {
      const len = codeLengths[sym];
      if (len > 0) {
        table.set((len << 16) | nextCode[len], sym);
        nextCode[len]++;
      }
    }
    return { table, maxBits };
  }

  function readSymbol(ht) {
    let code = 0;
    for (let len = 1; len <= ht.maxBits; len++) {
      code = (code << 1) | readBits(1);
      const key = (len << 16) | code;
      if (ht.table.has(key)) return ht.table.get(key);
    }
    return -1;
  }

  const LEN_BASE = [3,4,5,6,7,8,9,10,11,13,15,17,19,23,27,31,35,43,51,59,67,83,99,115,131,163,195,227,258];
  const LEN_EXTRA = [0,0,0,0,0,0,0,0,1,1,1,1,2,2,2,2,3,3,3,3,4,4,4,4,5,5,5,5,0];
  const DIST_BASE = [1,2,3,4,5,7,9,13,17,25,33,49,65,97,129,193,257,385,513,769,1025,1537,2049,3073,4097,6145,8193,12289,16385,24577];
  const DIST_EXTRA = [0,0,0,0,1,1,2,2,3,3,4,4,5,5,6,6,7,7,8,8,9,9,10,10,11,11,12,12,13,13];
  const CL_ORDER = [16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1,15];

  let bfinal = 0;
  while (!bfinal) {
    bfinal = readBits(1);
    const btype = readBits(2);

    if (btype === 0) {
      const len = readByte() | (readByte() << 8);
      readByte(); readByte(); // Skip nlen
      for (let i = 0; i < len; i++) output.push(readByte());
    } else if (btype === 1 || btype === 2) {
      let litHt, distHt;

      if (btype === 1) {
        litHt = buildHuffmanTable(FIXED_LIT_LENS, 288);
        distHt = buildHuffmanTable(FIXED_DIST_LENS, 32);
      } else {
        const hlit = readBits(5) + 257;
        const hdist = readBits(5) + 1;
        const hclen = readBits(4) + 4;
        const clLens = new Uint8Array(19);
        for (let i = 0; i < hclen; i++) clLens[CL_ORDER[i]] = readBits(3);
        const clHt = buildHuffmanTable(clLens, 19);

        const allLens = new Uint8Array(hlit + hdist);
        let idx = 0;
        while (idx < hlit + hdist) {
          const sym = readSymbol(clHt);
          if (sym < 16) {
            allLens[idx++] = sym;
          } else if (sym === 16) {
            const rep = readBits(2) + 3;
            const val = idx > 0 ? allLens[idx - 1] : 0;
            for (let r = 0; r < rep; r++) allLens[idx++] = val;
          } else if (sym === 17) {
            const rep = readBits(3) + 3;
            for (let r = 0; r < rep; r++) allLens[idx++] = 0;
          } else if (sym === 18) {
            const rep = readBits(7) + 11;
            for (let r = 0; r < rep; r++) allLens[idx++] = 0;
          }
        }
        litHt = buildHuffmanTable(allLens.subarray(0, hlit), hlit);
        distHt = buildHuffmanTable(allLens.subarray(hlit), hdist);
      }

      while (true) {
        const sym = readSymbol(litHt);
        if (sym < 0) return null;
        if (sym === 256) break;
        if (sym < 256) {
          output.push(sym);
        } else {
          const lenIdx = sym - 257;
          const length = LEN_BASE[lenIdx] + readBits(LEN_EXTRA[lenIdx]);
          const distSym = readSymbol(distHt);
          const distance = DIST_BASE[distSym] + readBits(DIST_EXTRA[distSym]);
          const srcStart = output.length - distance;
          for (let i = 0; i < length; i++) output.push(output[srcStart + i]);
        }
      }
    } else {
      return null;
    }
  }

  return new Uint8Array(output);
}

/**
 * Minimal PNG decoder: handles all standard filter types (none, sub, up,
 * average, paeth) with zlib-compressed IDAT chunks.
 *
 * Supports 8-bit RGB (colorType 2), RGBA (colorType 6), and grayscale
 * (colorType 0). Returns null if decode fails.
 *
 * @param {Uint8Array} png - Full PNG file bytes
 * @param {number} width
 * @param {number} height
 * @returns {Uint8Array | null} - RGB data or null on failure
 */
function decodePngToRgb(png, width, height) {
  try {
    const idatChunks = [];
    let offset = 8;
    let bitDepth = 8;
    let colorType = 2;

    while (offset < png.length - 4) {
      const dv = new DataView(png.buffer, png.byteOffset, png.byteLength);
      const chunkLen = dv.getUint32(offset, false);
      const chunkType = String.fromCharCode(
        png[offset + 4], png[offset + 5], png[offset + 6], png[offset + 7]
      );

      if (chunkType === "IHDR") {
        bitDepth = png[offset + 16];
        colorType = png[offset + 17];
      } else if (chunkType === "IDAT") {
        idatChunks.push(png.slice(offset + 8, offset + 8 + chunkLen));
      } else if (chunkType === "IEND") {
        break;
      }
      offset += 12 + chunkLen;
    }

    if (idatChunks.length === 0 || bitDepth !== 8) return null;

    const totalLen = idatChunks.reduce((sum, c) => sum + c.length, 0);
    const compressed = new Uint8Array(totalLen);
    let pos = 0;
    for (const chunk of idatChunks) {
      compressed.set(chunk, pos);
      pos += chunk.length;
    }

    const inflated = inflateSync(compressed);
    if (!inflated) return null;

    const bpp = colorType === 6 ? 4 : colorType === 2 ? 3 : colorType === 0 ? 1 : null;
    if (bpp === null) return null;

    const stride = width * bpp;
    const rgb = new Uint8Array(width * height * 3);
    let prevRow = new Uint8Array(stride);

    for (let y = 0; y < height; y++) {
      const filterByte = inflated[y * (stride + 1)];
      const rowStart = y * (stride + 1) + 1;
      const row = new Uint8Array(stride);

      for (let i = 0; i < stride; i++) {
        const raw = inflated[rowStart + i];
        const a = i >= bpp ? row[i - bpp] : 0;
        const b = prevRow[i];
        const c = i >= bpp ? prevRow[i - bpp] : 0;

        switch (filterByte) {
          case 0: row[i] = raw; break;
          case 1: row[i] = (raw + a) & 0xff; break;
          case 2: row[i] = (raw + b) & 0xff; break;
          case 3: row[i] = (raw + ((a + b) >> 1)) & 0xff; break;
          case 4: row[i] = (raw + paethPredictor(a, b, c)) & 0xff; break;
          default: return null;
        }
      }

      for (let x = 0; x < width; x++) {
        const srcIdx = x * bpp;
        const dstIdx = (y * width + x) * 3;
        if (bpp >= 3) {
          rgb[dstIdx] = row[srcIdx];
          rgb[dstIdx + 1] = row[srcIdx + 1];
          rgb[dstIdx + 2] = row[srcIdx + 2];
        } else {
          rgb[dstIdx] = rgb[dstIdx + 1] = rgb[dstIdx + 2] = row[srcIdx];
        }
      }
      prevRow = row;
    }
    return rgb;
  } catch (_e) {
    return null;
  }
}

/**
 * Align dimensions DOWN to the model's patch size (16).
 *
 * Falcon-OCR requires exact multiples of patchSize. We floor (not ceil)
 * to avoid introducing black padding that corrupts patch embeddings.
 * The Python reference raises ValueError on non-multiples, so we
 * truncate to the nearest lower multiple instead.
 *
 * @param {number} width
 * @param {number} height
 * @param {number} patchSize
 * @returns {{ width: number, height: number }}
 */
function alignToPatch(width, height, patchSize = 16) {
  return {
    width: Math.floor(width / patchSize) * patchSize,
    height: Math.floor(height / patchSize) * patchSize,
  };
}

/**
 * Crop RGB data to patch-aligned dimensions (discard right/bottom edges).
 *
 * @param {Uint8Array} rgb - Original RGB data
 * @param {number} origWidth
 * @param {number} origHeight
 * @param {number} newWidth
 * @param {number} newHeight
 * @returns {Uint8Array}
 */
function cropRgb(rgb, origWidth, origHeight, newWidth, newHeight) {
  if (origWidth === newWidth && origHeight === newHeight) {
    return rgb;
  }
  const cropped = new Uint8Array(newWidth * newHeight * 3);
  for (let y = 0; y < newHeight; y++) {
    const srcOffset = y * origWidth * 3;
    const dstOffset = y * newWidth * 3;
    cropped.set(rgb.subarray(srcOffset, srcOffset + newWidth * 3), dstOffset);
  }
  return cropped;
}

/**
 * Downsample an image to fit within maxDim while maintaining aspect ratio,
 * then align to the nearest multiple of patchSize.
 *
 * This is the primary speed optimization: reducing image size from 224x224
 * (196 patches, O(n^2) attention) to 128x128 (64 patches) gives a ~9x
 * speedup in the attention layers (quadratic in sequence length).
 *
 * Tradeoffs:
 *   - 128x128 (64 patches): ~9x faster than 224x224, minor quality loss
 *     on fine text. Good for invoices, receipts, printed documents.
 *   - 96x96 (36 patches): ~30x faster, noticeable quality loss on small
 *     text. Acceptable for large-font documents.
 *   - 64x64 (16 patches): ~150x faster, significant quality loss.
 *     Only useful for single-line text or very large text.
 *
 * Uses bilinear interpolation for quality downsampling.
 *
 * @param {Uint8Array} rgb - Source RGB pixels
 * @param {number} width - Source width
 * @param {number} height - Source height
 * @param {number} maxDim - Maximum dimension (default 128)
 * @param {number} patchSize - Patch size for alignment (default 16)
 * @returns {{ rgb: Uint8Array, width: number, height: number }}
 */
function resizeForInference(rgb, width, height, maxDim = 128, patchSize = 16) {
  // If already within bounds, just align to patch size
  if (width <= maxDim && height <= maxDim) {
    const aw = Math.floor(width / patchSize) * patchSize;
    const ah = Math.floor(height / patchSize) * patchSize;
    if (aw < patchSize || ah < patchSize) {
      return { rgb, width, height };
    }
    return { rgb: cropRgb(rgb, width, height, aw, ah), width: aw, height: ah };
  }

  // Scale down maintaining aspect ratio
  const scale = Math.min(maxDim / width, maxDim / height);
  let newW = Math.floor(width * scale);
  let newH = Math.floor(height * scale);

  // Align to patch size (floor to avoid exceeding maxDim)
  newW = Math.floor(newW / patchSize) * patchSize;
  newH = Math.floor(newH / patchSize) * patchSize;

  // Ensure minimum 1 patch per dimension
  if (newW < patchSize) newW = patchSize;
  if (newH < patchSize) newH = patchSize;

  // Bilinear interpolation
  const out = new Uint8Array(newW * newH * 3);
  const xRatio = width / newW;
  const yRatio = height / newH;

  for (let y = 0; y < newH; y++) {
    const srcY = y * yRatio;
    const y0 = Math.floor(srcY);
    const y1 = Math.min(y0 + 1, height - 1);
    const yFrac = srcY - y0;

    for (let x = 0; x < newW; x++) {
      const srcX = x * xRatio;
      const x0 = Math.floor(srcX);
      const x1 = Math.min(x0 + 1, width - 1);
      const xFrac = srcX - x0;

      const idx00 = (y0 * width + x0) * 3;
      const idx10 = (y0 * width + x1) * 3;
      const idx01 = (y1 * width + x0) * 3;
      const idx11 = (y1 * width + x1) * 3;
      const outIdx = (y * newW + x) * 3;

      for (let c = 0; c < 3; c++) {
        const v = (1 - xFrac) * (1 - yFrac) * rgb[idx00 + c]
                + xFrac * (1 - yFrac) * rgb[idx10 + c]
                + (1 - xFrac) * yFrac * rgb[idx01 + c]
                + xFrac * yFrac * rgb[idx11 + c];
        out[outIdx + c] = Math.round(v);
      }
    }
  }

  return { rgb: out, width: newW, height: newH };
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
 * @param {number} [maxLayers=0] - Use only the first N layers (0 = all).
 *   Only supported by the CPU backend (CpuDevice).  WASM backend ignores this.
 * @returns {Int32Array | Uint32Array}
 */
function invokeOcrTokens(backend, width, height, rgb, promptIds, maxNewTokens, maxLayers = 0) {
  if (backend.exports && typeof backend.exports.ocr_tokens === "function") {
    return backend.exports.ocr_tokens(width, height, rgb, promptIds, maxNewTokens);
  }
  if (typeof backend.ocrTokens === "function") {
    return backend.ocrTokens(width, height, rgb, promptIds, maxNewTokens, maxLayers);
  }
  throw new Error("Invalid inference backend: no ocr_tokens or ocrTokens method");
}

/**
 * Decode OCR token IDs to text with an explicit tokenizer requirement.
 *
 * Returning an empty string when token IDs exist loses model output silently.
 * Callers that need text must provide the loaded tokenizer; token-only
 * endpoints should return token IDs directly instead.
 *
 * @param {ArrayLike<number>} tokenIds
 * @param {import("./tokenizer.js").TokenizerDecoder | null} tokenizer
 * @returns {string}
 */
export function decodeOcrTokenArray(tokenIds, tokenizer) {
  if (!tokenizer || typeof tokenizer.decode !== "function") {
    throw new Error("Falcon OCR tokenizer is required to decode OCR token IDs");
  }
  const ids = Array.from(tokenIds);
  if (ids.length === 0) {
    throw new Error("Falcon OCR produced no tokens");
  }
  const text = tokenizer.decode(ids);
  if (typeof text !== "string") {
    throw new Error("Falcon OCR tokenizer returned non-string text");
  }
  if (text.trim().length === 0) {
    throw new Error("Falcon OCR decoded empty text");
  }
  return text;
}

/**
 * Validate the OCR success payload before serving or caching it.
 *
 * Blank pages need an explicit upstream signal before they can become a
 * successful OCR response; absent that signal, empty token/text payloads are
 * invalid model output and must fail closed.
 *
 * @param {unknown} payload
 * @param {string} source
 * @returns {object}
 */
export function normalizeOcrResultPayload(payload, source = "OCR result") {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    throw new Error(`${source} is not an object`);
  }
  if ("error" in payload) {
    throw new Error(`${source} contains error output`);
  }

  const record = /** @type {Record<string, unknown>} */ (payload);
  const text = record.text;
  if (typeof text !== "string") {
    throw new Error(`${source} is missing text`);
  }
  if (text.trim().length === 0) {
    throw new Error(`${source} contains empty text`);
  }

  const tokens = record.tokens;
  if (!Array.isArray(tokens)) {
    throw new Error(`${source} is missing token array`);
  }
  if (tokens.length === 0) {
    throw new Error(`${source} contains no tokens`);
  }
  for (const token of tokens) {
    if (!Number.isInteger(token) || token < 0) {
      throw new Error(`${source} contains invalid token id`);
    }
  }

  return {
    ...record,
    text,
    tokens: tokens.slice(),
  };
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
 * @param {import("./tokenizer.js").TokenizerDecoder | null} tokenizer - Optional tokenizer for decoding tokens to text
 * @returns {Promise<Response>}
 */
export async function handleOcrRequest(request, backend, env, cors, rid, device = "wasm", tokenizer = null) {
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

  // Downsample for inference speed: fewer patches = quadratically faster attention.
  // CPU mode uses aggressive 128px max to stay within Worker CPU budget.
  // WASM mode allows larger images since it runs faster.
  const maxDim = device === "cpu" ? 128 : 224;
  const resized = resizeForInference(decoded.rgb, decoded.width, decoded.height, maxDim);
  const { width: finalW, height: finalH, rgb } = resized;

  const promptIds = buildFalconOcrPromptIds(imageResult.category || "plain");

  // Respect max_tokens from request body if provided.
  // CPU mode default: 5 tokens (micro model ~60ms/step, fits Workers budget).
  // WASM mode default: 512 tokens (full generation).
  // LIMITATION: INT4 model (22 layers, dim=768) takes ~60s per forward pass,
  // so multi-step generation is only viable in Durable Objects or Containers.
  // The micro model (2 layers, dim=32) at ~60ms/step can generate 5+ tokens.
  let maxNewTokens = device === "cpu" ? 5 : 512;
  if (imageResult.maxTokens != null && imageResult.maxTokens > 0) {
    maxNewTokens = imageResult.maxTokens;
  }

  // For CPU mode with large models (>8 layers), use only the first 8 layers
  // to enable multi-token generation within the Workers wall-clock budget.
  // Speed vs quality tradeoff:
  //   - 8 of 22 layers: ~2.75x faster per step, moderate quality loss
  //   - All layers: full quality but ~60s per step (INT4 model)
  //   - Micro model (2 layers): maxLayers has no effect (all layers used)
  const maxLayers = device === "cpu" ? 8 : 0;
  const tokens = invokeOcrTokens(backend, finalW, finalH, rgb, promptIds, maxNewTokens, maxLayers);

  const tokenArray = Array.from(tokens);
  const text = decodeOcrTokenArray(tokenArray, tokenizer);
  const timMs = Date.now() - start;

  return new Response(
    JSON.stringify({
      text,
      tokens: tokenArray,
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
  } catch (err) {
    return new Response(
      JSON.stringify({
        error: "Structured OCR processing failed",
        detail: err.message,
        request_id: rid,
      }),
      {
        status: 502,
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

const DETAILED_BLOCK_TYPES = new Set([
  "heading", "field_label", "field_value", "paragraph", "table_cell", "footer",
]);


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
 * @param {import("./tokenizer.js").TokenizerDecoder | null} tokenizer - Optional tokenizer for decoding tokens to text
 * @returns {Promise<Response>}
 */
export async function handleBatchOcr(request, backend, env, cors, rid, device = "wasm", cacheOps = {}, tokenizer = null) {
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
  const promptIds = buildFalconOcrPromptIds(body.prompt || "plain");
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
            let cachedResult;
            try {
              cachedResult = normalizeOcrResultPayload(cached, "cached OCR result");
            } catch (err) {
              results.push({
                text: "",
                tokens: [],
                time_ms: Date.now() - imageStart,
                error: `Image at index ${idx}: invalid cached OCR result: ${err.message}`,
              });
              continue;
            }
            results.push({
              text: cachedResult.text,
              tokens: cachedResult.tokens,
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
      const tokenArray = Array.from(tokens);
      const timeMs = Date.now() - imageStart;
      const result = {
        text: decodeOcrTokenArray(tokenArray, tokenizer),
        tokens: tokenArray,
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
 * Extract a JSON object from a model response that was instructed to emit JSON.
 *
 * @param {string} responseText
 * @param {string} source
 * @returns {Record<string, unknown>}
 */
function parseModelJsonObject(responseText, source) {
  if (typeof responseText !== "string" || responseText.trim().length === 0) {
    throw new Error(`${source} returned empty response`);
  }
  const stripped = responseText.replace(/^```(?:json)?\s*\n?/m, "").replace(/\n?```\s*$/m, "");
  const jsonMatch = stripped.match(/\{[\s\S]*\}/);
  if (!jsonMatch) {
    throw new Error(`${source} did not return a JSON object`);
  }
  const parsed = JSON.parse(jsonMatch[0]);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${source} JSON root is not an object`);
  }
  return /** @type {Record<string, unknown>} */ (parsed);
}

/**
 * Normalize a table payload and reject shape inconsistencies.
 *
 * @param {unknown} table
 * @param {number} idx
 * @param {string} source
 * @returns {{rows: number, cols: number, headers: string[], data: string[][]}}
 */
function normalizeTablePayload(table, idx, source) {
  if (!table || typeof table !== "object" || Array.isArray(table)) {
    throw new Error(`${source} ${idx} is not an object`);
  }
  const record = /** @type {Record<string, unknown>} */ (table);
  if (!Array.isArray(record.headers)) {
    throw new Error(`${source} ${idx} is missing headers array`);
  }
  if (!Array.isArray(record.data)) {
    throw new Error(`${source} ${idx} is missing data array`);
  }
  const rows = Number(record.rows);
  const cols = Number(record.cols);
  if (!Number.isInteger(rows) || rows < 0) {
    throw new Error(`${source} ${idx} has invalid row count`);
  }
  if (!Number.isInteger(cols) || cols < 0) {
    throw new Error(`${source} ${idx} has invalid column count`);
  }
  const headers = record.headers.map(String);
  const data = record.data.map((row, rowIdx) => {
    if (!Array.isArray(row)) {
      throw new Error(`${source} ${idx} row ${rowIdx} is not an array`);
    }
    if (row.length !== cols) {
      throw new Error(`${source} ${idx} row ${rowIdx} width does not match cols`);
    }
    return row.map(String);
  });
  if (rows !== data.length) {
    throw new Error(`${source} ${idx} row count does not match data length`);
  }
  if (cols !== headers.length) {
    throw new Error(`${source} ${idx} column count does not match headers length`);
  }
  return { rows, cols, headers, data };
}

/**
 * @param {unknown} block
 * @param {number} idx
 * @param {string} source
 * @returns {{ text: string, confidence: number, bbox: {x: number, y: number, width: number, height: number}, type: string }}
 */
function normalizeDetailedBlockPayload(block, idx, source) {
  if (!block || typeof block !== "object" || Array.isArray(block)) {
    throw new Error(`${source} block ${idx} is not an object`);
  }
  const record = /** @type {Record<string, unknown>} */ (block);
  if (typeof record.text !== "string" || record.text.trim().length === 0) {
    throw new Error(`${source} block ${idx} contains empty text`);
  }
  const confidence = Number(record.confidence);
  if (!Number.isFinite(confidence) || confidence < 0 || confidence > 1) {
    throw new Error(`${source} block ${idx} has invalid confidence`);
  }
  const bbox = record.bbox;
  if (!bbox || typeof bbox !== "object" || Array.isArray(bbox)) {
    throw new Error(`${source} block ${idx} is missing bbox object`);
  }
  const bboxRecord = /** @type {Record<string, unknown>} */ (bbox);
  const normalizedBbox = {
    x: Number(bboxRecord.x),
    y: Number(bboxRecord.y),
    width: Number(bboxRecord.width),
    height: Number(bboxRecord.height),
  };
  for (const [key, value] of Object.entries(normalizedBbox)) {
    if (!Number.isFinite(value) || ((key === "width" || key === "height") && value < 0)) {
      throw new Error(`${source} block ${idx} has invalid bbox.${key}`);
    }
  }
  if (typeof record.type !== "string" || !DETAILED_BLOCK_TYPES.has(record.type)) {
    throw new Error(`${source} block ${idx} has invalid type`);
  }
  return {
    text: record.text,
    confidence,
    bbox: normalizedBbox,
    type: record.type,
  };
}

/**
 * @param {unknown} metadata
 * @param {string} source
 * @returns {{language: string, orientation: number, has_handwriting: boolean}}
 */
function normalizeDetailedMetadataPayload(metadata, source) {
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) {
    throw new Error(`${source} is missing metadata object`);
  }
  const record = /** @type {Record<string, unknown>} */ (metadata);
  if (typeof record.language !== "string" || record.language.trim().length === 0) {
    throw new Error(`${source} metadata is missing language`);
  }
  const orientation = Number(record.orientation);
  if (![0, 90, 180, 270].includes(orientation)) {
    throw new Error(`${source} metadata has invalid orientation`);
  }
  if (typeof record.has_handwriting !== "boolean") {
    throw new Error(`${source} metadata has invalid has_handwriting`);
  }
  return {
    language: record.language,
    orientation,
    has_handwriting: record.has_handwriting,
  };
}

/**
 * Validate and normalize a detailed OCR page payload before serving/caching.
 *
 * @param {unknown} payload
 * @param {string} source
 * @returns {{ text: string, blocks: Array, tables: Array, metadata: object }}
 */
export function normalizeDetailedOcrPagePayload(payload, source = "detailed OCR page") {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    throw new Error(`${source} is not an object`);
  }
  const record = /** @type {Record<string, unknown>} */ (payload);
  if (typeof record.text !== "string" || record.text.trim().length === 0) {
    throw new Error(`${source} contains empty text`);
  }
  if (!Array.isArray(record.blocks)) {
    throw new Error(`${source} is missing blocks array`);
  }
  if (!Array.isArray(record.tables)) {
    throw new Error(`${source} is missing tables array`);
  }
  if (!record.metadata || typeof record.metadata !== "object" || Array.isArray(record.metadata)) {
    throw new Error(`${source} is missing metadata object`);
  }
  return {
    text: record.text,
    blocks: record.blocks.map((block, idx) => normalizeDetailedBlockPayload(block, idx, source)),
    tables: record.tables.map((table, idx) => normalizeTablePayload(table, idx, `${source} table`)),
    metadata: normalizeDetailedMetadataPayload(record.metadata, source),
  };
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

  const parsed = parseModelJsonObject(responseText, "Detailed OCR model");
  if (!Array.isArray(parsed.blocks)) {
    throw new Error("Detailed OCR JSON is missing blocks array");
  }
  if (!Array.isArray(parsed.tables)) {
    throw new Error("Detailed OCR JSON is missing tables array");
  }
  if (!parsed.metadata || typeof parsed.metadata !== "object" || Array.isArray(parsed.metadata)) {
    throw new Error("Detailed OCR JSON is missing metadata object");
  }

  // Normalize blocks
  const blocks = parsed.blocks.map((b, idx) => normalizeDetailedBlockPayload(b, idx, "detailed OCR model output"));

  // Normalize tables
  const tables = parsed.tables.map((t, idx) => normalizeTablePayload(t, idx, "detailed OCR model table"));

  // Normalize metadata
  const metadata = normalizeDetailedMetadataPayload(parsed.metadata, "detailed OCR model output");

  const text = typeof parsed.text === "string" ? parsed.text
    : blocks.map((b) => b.text).join("\n");

  return normalizeDetailedOcrPagePayload(
    { text, blocks, tables, metadata },
    "detailed OCR model output",
  );
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
      try {
        pages.push(normalizeDetailedOcrPagePayload(cached, "cached detailed OCR page"));
      } catch (err) {
        return new Response(
          JSON.stringify({
            error: "Invalid cached detailed OCR page",
            detail: err.message,
            request_id: rid,
          }),
          { status: 502, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
    } else {
      let pageResult;
      try {
        pageResult = await runDetailedOcrPage(env, pageBytes[i]);
      } catch (err) {
        return new Response(
          JSON.stringify({
            error: "Invalid detailed OCR model output",
            detail: err.message,
            request_id: rid,
          }),
          { status: 502, headers: { ...cors, "Content-Type": "application/json" } },
        );
      }
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

  try {
    const tables = parseTableOcrResponse(responseText);
    return new Response(
      JSON.stringify({
        tables,
        table_count: tables.length,
        time_ms: Date.now() - start,
        request_id: rid,
      }),
      { status: 200, headers: { ...cors, "Content-Type": "application/json" } },
    );
  } catch (err) {
    return new Response(
      JSON.stringify({
        error: "Invalid table OCR model output",
        detail: err.message,
        request_id: rid,
      }),
      { status: 502, headers: { ...cors, "Content-Type": "application/json" } },
    );
  }
}

/**
 * Parse and normalize Workers AI table extraction output.
 *
 * @param {string} responseText
 * @returns {Array<{rows: number, cols: number, headers: string[], data: string[][]}>}
 */
export function parseTableOcrResponse(responseText) {
  if (typeof responseText !== "string" || responseText.trim().length === 0) {
    throw new Error("Table OCR model returned empty response");
  }

  const stripped = responseText.replace(/^```(?:json)?\s*\n?/m, "").replace(/\n?```\s*$/m, "");
  const jsonMatch = stripped.match(/\{[\s\S]*\}/);
  if (!jsonMatch) {
    throw new Error("Table OCR model did not return a JSON object");
  }

  const parsed = JSON.parse(jsonMatch[0]);
  if (!parsed || typeof parsed !== "object" || !Array.isArray(parsed.tables)) {
    throw new Error("Table OCR JSON is missing tables array");
  }

  return parsed.tables.map((t, idx) => normalizeTablePayload(t, idx, "Table OCR table"));
}

/**
 * Validate template extraction output before serving or caching it.
 *
 * @param {unknown} payload
 * @param {string} source
 * @returns {object}
 */
export function normalizeTemplateExtractionPayload(payload, source = "template extraction result") {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    throw new Error(`${source} is not an object`);
  }
  if ("error" in payload) {
    throw new Error(`${source} contains error output`);
  }

  const record = /** @type {Record<string, unknown>} */ (payload);
  if (!record.template || typeof record.template !== "object" || Array.isArray(record.template)) {
    throw new Error(`${source} is missing template object`);
  }
  const template = /** @type {Record<string, unknown>} */ (record.template);
  for (const field of ["id", "name", "type"]) {
    if (typeof template[field] !== "string" || String(template[field]).trim().length === 0) {
      throw new Error(`${source} template is missing ${field}`);
    }
  }
  if (!template.layout || typeof template.layout !== "object" || Array.isArray(template.layout)) {
    throw new Error(`${source} template is missing layout object`);
  }
  const layout = /** @type {Record<string, unknown>} */ (template.layout);
  if (!Array.isArray(layout.sections) || layout.sections.length === 0) {
    throw new Error(`${source} template layout is missing sections array`);
  }
  for (const [idx, section] of layout.sections.entries()) {
    if (!section || typeof section !== "object" || Array.isArray(section)) {
      throw new Error(`${source} template layout section ${idx} is not an object`);
    }
    const sectionRecord = /** @type {Record<string, unknown>} */ (section);
    if (typeof sectionRecord.type !== "string" || !SECTION_TYPES.includes(sectionRecord.type)) {
      throw new Error(`${source} template layout section ${idx} has invalid type`);
    }
    if (typeof sectionRecord.visible !== "boolean") {
      throw new Error(`${source} template layout section ${idx} has invalid visibility`);
    }
    if (!Number.isInteger(sectionRecord.order) || sectionRecord.order < 0) {
      throw new Error(`${source} template layout section ${idx} has invalid order`);
    }
  }
  if (!template.styles || typeof template.styles !== "object" || Array.isArray(template.styles)) {
    throw new Error(`${source} template is missing styles object`);
  }
  if (!Array.isArray(record.detected_sections)) {
    throw new Error(`${source} is missing detected_sections array`);
  }
  for (const section of record.detected_sections) {
    if (!SECTION_TYPES.includes(section)) {
      throw new Error(`${source} contains invalid section`);
    }
  }
  if (!Number.isFinite(record.confidence) || record.confidence < 0 || record.confidence > 1) {
    throw new Error(`${source} contains invalid confidence`);
  }

  return {
    ...record,
    template: record.template,
    detected_sections: record.detected_sections.slice(),
    confidence: record.confidence,
  };
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
    let cached = null;
    try {
      cached = await env.OCR_CACHE.get(cacheKey, "json");
    } catch (_e) { /* cache miss */ }
    if (cached) {
      return {
        ...normalizeTemplateExtractionPayload(cached, "cached template extraction result"),
        time_ms: Date.now() - start,
        cache: "hit",
      };
    }
  }

  if (!env.AI || typeof env.AI.run !== "function") {
    throw new Error("Template extraction requires Workers AI binding");
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

  const layoutInfo = parseModelJsonObject(responseText, "Template extraction model output");
  if (!Array.isArray(layoutInfo.sections)) {
    throw new Error("Template extraction JSON is missing sections array");
  }

  // Build template from AI layout analysis
  const detectedSections = layoutInfo.sections.map(String);
  for (const section of detectedSections) {
    if (!SECTION_TYPES.includes(section)) {
      throw new Error(`Template extraction JSON contains invalid section: ${section}`);
    }
  }

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

  const result = normalizeTemplateExtractionPayload({
    template,
    confidence: Math.round(confidence * 1000) / 1000,
    detected_sections: detectedSections,
    auto_filled: true,
    auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
    auto_fill_dismissable: true,
    time_ms: Date.now() - start,
  }, "template extraction result");

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
