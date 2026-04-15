/**
 * Cloudflare Workers AI fallback for OCR inference.
 *
 * When Workers AI is available (via env.AI binding), uses GPU-accelerated
 * vision models for OCR instead of CPU-bound local inference.  This is the
 * most sustainable production architecture:
 *
 *   - Zero CPU cost on the Worker (inference runs on Cloudflare's GPU fleet)
 *   - No model loading, no memory pressure, no cold start penalty
 *   - Scales with Cloudflare's infrastructure, not Worker limits
 *
 * Fallback chain:
 *   1. Workers AI (GPU) — if AI binding is available
 *   2. Local CPU inference — Falcon-OCR INT8 sharded model
 *   3. Error response with fallback URL to PaddleOCR
 *
 * Supported Workers AI vision models (ranked by OCR suitability):
 *   - @cf/google/gemma-4-26b-a4b-it  (best: native OCR + multilingual)
 *   - @cf/meta/llama-3.2-11b-vision-instruct  (good: general vision)
 *   - @cf/mistralai/mistral-small-3.1-24b-instruct  (good: vision + long context)
 *
 * Usage in wrangler.toml:
 *   [ai]
 *   binding = "AI"
 */

/**
 * @typedef {Object} AiFallbackResult
 * @property {string} text - Extracted text
 * @property {number} confidence - Confidence score [0, 1]
 * @property {string} model - Model identifier used
 * @property {string} backend - "workers-ai" | "local-cpu"
 * @property {number} time_ms - Inference time
 */

/**
 * OCR prompt template for vision models.
 * Structured to extract maximum text with formatting preservation.
 */
const OCR_PROMPT = `Extract ALL text from this document image exactly as it appears.
Preserve the layout, line breaks, and formatting.
Include all numbers, dates, currency amounts, and special characters.
Do not interpret or summarize — output the raw text only.`;

/**
 * Invoice-specific prompt for higher accuracy on structured documents.
 */
const INVOICE_OCR_PROMPT = `Extract ALL text from this invoice/receipt image exactly as written.
Include: company name, invoice number, dates, all line items with amounts,
subtotals, taxes, and totals. Preserve exact formatting and numbers.
Output raw text only, no interpretation.`;

/**
 * Preferred Workers AI models for OCR, in order of quality.
 * Each entry includes the model ID and a fallback flag.
 *
 * @type {Array<{id: string, maxTokens: number}>}
 */
const AI_MODELS = [
  { id: "@cf/google/gemma-4-26b-a4b-it", maxTokens: 2048 },
  { id: "@cf/meta/llama-3.2-11b-vision-instruct", maxTokens: 2048 },
  { id: "@cf/mistralai/mistral-small-3.1-24b-instruct", maxTokens: 2048 },
];

/**
 * Check if Workers AI binding is available and functional.
 *
 * @param {object} env - Worker environment bindings
 * @returns {boolean}
 */
export function isWorkersAiAvailable(env) {
  return !!(env && env.AI && typeof env.AI.run === "function");
}

/**
 * Run OCR inference via Cloudflare Workers AI.
 *
 * Tries each model in AI_MODELS order until one succeeds.
 * Converts image bytes to base64 for the API.
 *
 * @param {object} env - Worker environment with AI binding
 * @param {Uint8Array} imageBytes - Raw image file bytes (JPEG/PNG)
 * @param {object} options - Optional configuration
 * @param {string} [options.prompt] - Custom OCR prompt
 * @param {boolean} [options.isInvoice] - Use invoice-specific prompt
 * @param {number} [options.maxTokens] - Max output tokens
 * @returns {Promise<AiFallbackResult>}
 */
export async function runWorkersAiOcr(env, imageBytes, options = {}) {
  if (!isWorkersAiAvailable(env)) {
    throw new Error("Workers AI binding not available");
  }

  const prompt = options.prompt || (options.isInvoice ? INVOICE_OCR_PROMPT : OCR_PROMPT);
  const maxTokens = options.maxTokens || 2048;
  const start = Date.now();

  // Convert image to base64 for the AI API
  const base64Image = uint8ArrayToBase64(imageBytes);

  let lastError = null;

  for (const model of AI_MODELS) {
    try {
      const result = await env.AI.run(model.id, {
        messages: [
          {
            role: "user",
            content: [
              {
                type: "image",
                image: base64Image,
              },
              {
                type: "text",
                text: prompt,
              },
            ],
          },
        ],
        max_tokens: Math.min(maxTokens, model.maxTokens),
        temperature: 0.0,  // Deterministic for OCR
      });

      const text = extractTextFromAiResult(result);
      if (text && text.length > 0) {
        return {
          text,
          confidence: estimateConfidence(text),
          model: model.id,
          backend: "workers-ai",
          time_ms: Date.now() - start,
        };
      }
    } catch (err) {
      lastError = err;
      console.warn(`Workers AI model ${model.id} failed: ${err.message}`);
      // Try next model
    }
  }

  throw new Error(
    `All Workers AI models failed. Last error: ${lastError?.message || "unknown"}`
  );
}

/**
 * Extract text content from Workers AI response.
 *
 * The response format varies by model — this normalizes it.
 *
 * @param {object} result - AI.run() response
 * @returns {string}
 */
function extractTextFromAiResult(result) {
  if (typeof result === "string") {
    return result.trim();
  }
  if (result && typeof result.response === "string") {
    return result.response.trim();
  }
  if (result && result.choices && result.choices.length > 0) {
    const choice = result.choices[0];
    if (choice.message && typeof choice.message.content === "string") {
      return choice.message.content.trim();
    }
    if (typeof choice.text === "string") {
      return choice.text.trim();
    }
  }
  if (result && typeof result.text === "string") {
    return result.text.trim();
  }
  return "";
}

/**
 * Estimate OCR confidence from extracted text heuristics.
 *
 * A rough confidence score based on text characteristics:
 *   - Length (very short = low confidence)
 *   - Contains expected patterns (numbers, dates, currency)
 *   - Character distribution (mostly printable ASCII = higher)
 *
 * @param {string} text - Extracted text
 * @returns {number} Confidence score [0, 1]
 */
function estimateConfidence(text) {
  if (!text || text.length === 0) return 0.0;

  let score = 0.0;

  // Length factor: very short text is suspicious
  if (text.length >= 20) score += 0.3;
  else if (text.length >= 5) score += 0.1;

  // Contains numbers (expected in invoices)
  if (/\d/.test(text)) score += 0.2;

  // Contains currency patterns
  if (/[\$\u20AC\u00A3\u00A5]|USD|EUR|GBP|JPY/.test(text)) score += 0.15;

  // Contains date-like patterns
  if (/\d{2,4}[-/]\d{1,2}[-/]\d{1,4}/.test(text)) score += 0.15;

  // Mostly printable characters
  const printable = text.replace(/[^\x20-\x7E\n\r\t]/g, "").length;
  const printableRatio = printable / text.length;
  score += printableRatio * 0.2;

  return Math.min(1.0, score);
}

/**
 * Convert Uint8Array to base64 string.
 *
 * Uses btoa in Workers runtime (available since 2023).
 *
 * @param {Uint8Array} bytes
 * @returns {string}
 */
function uint8ArrayToBase64(bytes) {
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
 * Hybrid OCR handler: tries Workers AI first, falls back to local CPU.
 *
 * This is the recommended production entry point. It provides the best
 * balance of quality, speed, and sustainability:
 *
 *   - Workers AI: GPU inference, zero CPU cost, highest quality
 *   - Local CPU: Falcon-OCR INT8, fallback only, bounded by Worker limits
 *
 * @param {object} env - Worker environment
 * @param {Uint8Array} imageBytes - Raw image bytes
 * @param {function|null} localInferenceFn - Local CPU inference fallback
 * @param {object} options - Configuration options
 * @returns {Promise<AiFallbackResult>}
 */
export async function hybridOcr(env, imageBytes, localInferenceFn, options = {}) {
  // Try Workers AI first (GPU, zero CPU cost)
  if (isWorkersAiAvailable(env)) {
    try {
      return await runWorkersAiOcr(env, imageBytes, options);
    } catch (err) {
      console.warn(`Workers AI OCR failed, falling back to local: ${err.message}`);
    }
  }

  // Fall back to local CPU inference
  if (typeof localInferenceFn === "function") {
    const start = Date.now();
    const result = await localInferenceFn(imageBytes);
    return {
      text: result.text || "",
      confidence: result.confidence || 0.0,
      model: "falcon-ocr-int8",
      backend: "local-cpu",
      time_ms: Date.now() - start,
    };
  }

  throw new Error("No OCR backend available: Workers AI not bound and no local inference");
}
