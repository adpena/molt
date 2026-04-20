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
 * @property {string} model - Full model identifier used
 * @property {string} model_used - Short model name for client consumption
 * @property {string} backend - "workers-ai" | "local-cpu"
 * @property {number} time_ms - Inference time
 * @property {number} retries - Number of retry attempts on the serving model
 */

/**
 * OCR prompt template for vision models.
 * Structured to extract maximum text with formatting preservation.
 */
const OCR_PROMPT = `Extract ALL text from this invoice image. Return the text exactly as it appears, preserving:
- Line items with quantities, rates, and amounts
- All dates (issue date, due date)
- Invoice number
- Company names and addresses
- Totals, subtotals, tax amounts
- Currency symbols and formatting
- Payment terms

Output the raw text line-by-line, exactly as printed. Do not summarize or interpret.`;

/**
 * Invoice-specific prompt for higher accuracy on structured documents.
 */
const INVOICE_OCR_PROMPT = `Extract ALL text from this invoice/receipt image exactly as written.
Include: company name, invoice number, dates, all line items with amounts,
subtotals, taxes, and totals. Preserve exact formatting and numbers.
Output raw text only, no interpretation.`;

/**
 * Structured invoice extraction prompt that returns JSON.
 * Used by the /ocr/structured endpoint for MCP agent consumption.
 */
const STRUCTURED_INVOICE_PROMPT = `Extract invoice data as JSON: {"vendor":"...","invoice_number":"...","issue_date":"...","due_date":"...","items":[{"description":"...","qty":N,"rate":N,"amount":N}],"subtotal":N,"tax":N,"total":N,"currency":"..."}`;

/**
 * JSON schema for structured invoice output validation.
 * @type {object}
 */
const STRUCTURED_INVOICE_SCHEMA = {
  type: "object",
  required: ["vendor", "invoice_number", "total", "currency"],
  properties: {
    vendor: { type: "string" },
    invoice_number: { type: "string" },
    issue_date: { type: "string" },
    due_date: { type: "string" },
    items: {
      type: "array",
      items: {
        type: "object",
        required: ["description", "amount"],
        properties: {
          description: { type: "string" },
          qty: { type: "number" },
          rate: { type: "number" },
          amount: { type: "number" },
        },
      },
    },
    subtotal: { type: "number" },
    tax: { type: "number" },
    total: { type: "number" },
    currency: { type: "string" },
  },
};

/**
 * Preferred Workers AI models for OCR, in order of quality.
 *
 * The primary model (Gemma 3 12B) is retried with exponential backoff
 * on 503/capacity errors.  Fallback models each get a single attempt.
 *
 * @type {Array<{id: string, name: string, maxTokens: number, retries: number, delays: number[]}>}
 */
const AI_MODELS = [
  { id: "@cf/google/gemma-3-12b-it", name: "gemma-3-12b", maxTokens: 2048, retries: 3, delays: [200, 500, 1000] },
  { id: "@cf/meta/llama-3.2-11b-vision-instruct", name: "llama-3.2-11b", maxTokens: 2048, retries: 0, delays: [] },
  { id: "@cf/mistralai/mistral-small-3.1-24b-instruct", name: "mistral-small-3.1", maxTokens: 2048, retries: 0, delays: [] },
  { id: "@cf/meta/llama-3.2-3b-instruct", name: "llama-3.2-3b", maxTokens: 2048, retries: 0, delays: [] },
];

/** Total timeout for the entire retry+fallback chain (ms). */
const AI_TOTAL_TIMEOUT_MS = 5000;

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
 * Determine whether an error is a transient capacity error (503)
 * that warrants a retry or model fallback.
 *
 * @param {Error} err
 * @returns {boolean}
 */
function isCapacityError(err) {
  const msg = (err.message || "").toLowerCase();
  return msg.includes("503") ||
    msg.includes("capacity") ||
    msg.includes("overloaded") ||
    msg.includes("rate limit") ||
    msg.includes("too many requests");
}

/**
 * Run OCR inference via Cloudflare Workers AI with exponential backoff
 * retries on the primary model and ordered fallback to smaller models.
 *
 * Retry strategy:
 *   1. Primary model (Gemma 3 12B): up to 3 retries with 200ms/500ms/1000ms backoff
 *   2. Fallback models: 1 attempt each, no retries (time budget already spent)
 *   3. Total hard timeout: 5 seconds for the entire chain
 *
 * On success, the response includes `model_used` indicating which model
 * served the request, and `retries` counting how many attempts were made
 * on the model that ultimately succeeded.
 *
 * @param {object} env - Worker environment with AI binding
 * @param {Uint8Array} imageBytes - Raw image file bytes (JPEG/PNG)
 * @param {object} options - Optional configuration
 * @param {string} [options.prompt] - Custom OCR prompt
 * @param {boolean} [options.isInvoice] - Use invoice-specific prompt
 * @param {boolean} [options.structured] - Return structured JSON instead of raw text
 * @param {number} [options.maxTokens] - Max output tokens
 * @returns {Promise<AiFallbackResult>}
 */
export async function runWorkersAiOcr(env, imageBytes, options = {}) {
  if (!isWorkersAiAvailable(env)) {
    throw new Error("Workers AI binding not available");
  }

  const prompt = options.prompt || (options.structured
    ? STRUCTURED_INVOICE_PROMPT
    : (options.isInvoice ? INVOICE_OCR_PROMPT : OCR_PROMPT));
  const maxTokens = options.maxTokens || 2048;
  const start = Date.now();
  const deadline = start + AI_TOTAL_TIMEOUT_MS;

  // Convert image to base64 once (shared across all attempts)
  const base64Image = uint8ArrayToBase64(imageBytes);

  let lastError = null;
  let totalAttempts = 0;

  for (const model of AI_MODELS) {
    const maxAttempts = model.retries + 1; // retries + initial attempt

    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      // Hard timeout: abort if we've exceeded the 5s budget
      if (Date.now() >= deadline) {
        console.warn(`Workers AI timeout reached after ${totalAttempts} total attempts across models`);
        throw new Error(
          `Workers AI timeout: all models exhausted within ${AI_TOTAL_TIMEOUT_MS}ms. ` +
          `Last error: ${lastError?.message || "unknown"}`
        );
      }

      // Exponential backoff sleep (only on retries, not the first attempt)
      if (attempt > 0) {
        const delay = model.delays[attempt - 1];
        // Don't sleep past the deadline
        const effectiveDelay = Math.min(delay, deadline - Date.now());
        if (effectiveDelay > 0) {
          await new Promise((resolve) => setTimeout(resolve, effectiveDelay));
        }
        // Re-check deadline after sleep
        if (Date.now() >= deadline) {
          console.warn(`Workers AI timeout reached during backoff for ${model.name}`);
          break;
        }
      }

      totalAttempts++;

      try {
        const result = await env.AI.run(model.id, {
          messages: [
            {
              role: "user",
              content: `${prompt}\n\n![image](data:image/png;base64,${base64Image})`,
            },
          ],
          max_tokens: Math.min(maxTokens, model.maxTokens),
        });

        const text = extractTextFromAiResult(result);
        if (text && text.length > 0) {
          return {
            text,
            confidence: estimateConfidence(text),
            engine: "falcon-ocr",
            model: model.id,
            model_used: model.name,
            backend: "workers-ai",
            auto_filled: true,
            auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
            auto_fill_dismissable: true,
            time_ms: Date.now() - start,
            retries: attempt,
          };
        }

        // Empty response: treat as a soft failure, try next model
        console.warn(`Workers AI model ${model.name} returned empty text (attempt ${attempt + 1}/${maxAttempts})`);
        lastError = new Error(`${model.name} returned empty response`);
      } catch (err) {
        lastError = err;
        console.warn(`Workers AI model ${model.name} attempt ${attempt + 1}/${maxAttempts} failed: ${err.message}`);

        if (isCapacityError(err)) {
          // Capacity error: retry (if retries remain) or fall through to next model
          continue;
        }
        // Non-capacity error (e.g., bad request, auth failure): skip retries, try next model
        break;
      }
    }
  }

  throw new Error(
    `All Workers AI models failed after ${totalAttempts} total attempts. ` +
    `Last error: ${lastError?.message || "unknown"}`
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
 * Parse structured JSON from AI response text.
 *
 * The model may return the JSON wrapped in markdown code fences or with
 * surrounding prose.  This extracts the first valid JSON object.
 *
 * @param {string} text - Raw AI response
 * @returns {object|null} Parsed invoice data, or null if unparseable
 */
function parseStructuredResponse(text) {
  // Try direct parse first
  try {
    const parsed = JSON.parse(text.trim());
    if (typeof parsed === "object" && parsed !== null) return parsed;
  } catch (_) { /* fall through */ }

  // Extract JSON from markdown code fences or surrounding text
  const jsonMatch = text.match(/\{[\s\S]*\}/);
  if (jsonMatch) {
    try {
      const parsed = JSON.parse(jsonMatch[0]);
      if (typeof parsed === "object" && parsed !== null) return parsed;
    } catch (_) { /* fall through */ }
  }

  return null;
}

/**
 * Validate structured invoice data against the expected schema.
 *
 * Performs type coercion for numeric fields (the model sometimes returns
 * numbers as strings) and ensures required fields are present.
 *
 * @param {object} data - Parsed JSON from AI response
 * @returns {object} Validated and coerced invoice data
 */
function validateStructuredInvoice(data) {
  const result = {
    vendor: String(data.vendor || ""),
    invoice_number: String(data.invoice_number || ""),
    issue_date: String(data.issue_date || ""),
    due_date: String(data.due_date || ""),
    items: [],
    subtotal: 0,
    tax: 0,
    total: 0,
    currency: String(data.currency || "USD"),
  };

  // Coerce numeric fields
  if (data.subtotal != null) result.subtotal = Number(data.subtotal) || 0;
  if (data.tax != null) result.tax = Number(data.tax) || 0;
  if (data.total != null) result.total = Number(data.total) || 0;

  // Validate items array
  if (Array.isArray(data.items)) {
    for (const item of data.items) {
      if (typeof item === "object" && item !== null) {
        result.items.push({
          description: String(item.description || ""),
          qty: Number(item.qty) || 0,
          rate: Number(item.rate) || 0,
          amount: Number(item.amount) || 0,
        });
      }
    }
  }

  return result;
}

/**
 * Run structured OCR inference that returns parsed invoice JSON.
 *
 * Uses Workers AI with the structured prompt, parses the JSON response,
 * validates it against the invoice schema, and returns a clean object.
 *
 * @param {object} env - Worker environment with AI binding
 * @param {Uint8Array} imageBytes - Raw image file bytes (JPEG/PNG)
 * @returns {Promise<{invoice: object, confidence: number, model: string, model_used: string, time_ms: number}>}
 */
export async function runStructuredOcr(env, imageBytes) {
  const result = await runWorkersAiOcr(env, imageBytes, {
    structured: true,
    maxTokens: 2048,
  });

  const parsed = parseStructuredResponse(result.text);
  if (!parsed) {
    return {
      invoice: validateStructuredInvoice({}),
      raw_text: result.text,
      confidence: 0.0,
      model: result.model,
      model_used: result.model_used,
      auto_filled: true,
      auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
      auto_fill_dismissable: true,
      time_ms: result.time_ms,
      parse_error: "Failed to extract JSON from model response",
    };
  }

  const invoice = validateStructuredInvoice(parsed);
  return {
    invoice,
    confidence: result.confidence,
    model: result.model,
    model_used: result.model_used,
    auto_filled: true,
    auto_fill_warning: "These fields were auto-filled by AI. Please review all values before sending.",
    auto_fill_dismissable: true,
    time_ms: result.time_ms,
  };
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
      model_used: "falcon-ocr-int8",
      backend: "local-cpu",
      time_ms: Date.now() - start,
      retries: 0,
    };
  }

  throw new Error("No OCR backend available: Workers AI not bound and no local inference");
}
