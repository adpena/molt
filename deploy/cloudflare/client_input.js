/**
 * Client input parsing for the Falcon-OCR Worker.
 *
 * Request bodies and base64 fields come from the client, so a parse failure
 * is the client's error: it answers 400, never the 500 of an unhandled throw
 * or the 503 "backend unavailable" fallback. Each parser returns either its
 * value or `{ error, status }`, as `extractImage` in ocr_api.js does, and the
 * route returns `clientErrorResponse(failure, rid, cors)`.
 */

/**
 * Read the request body as a JSON object.
 *
 * @param {Request} request
 * @returns {Promise<{ value: Record<string, unknown> } | { error: string, status: number }>}
 */
export async function readJsonObject(request) {
  let value;
  try {
    value = await request.json();
  } catch {
    return { error: "Request body is not valid JSON", status: 400 };
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return { error: "Request body must be a JSON object", status: 400 };
  }
  return { value };
}

/**
 * Decode a base64 field to bytes.
 *
 * @param {unknown} value
 * @param {string} field - The field's name, for the error message.
 * @returns {{ bytes: Uint8Array } | { error: string, status: number }}
 */
export function decodeBase64Field(value, field) {
  if (typeof value !== "string" || value.length === 0) {
    return { error: `Missing '${field}' field (base64 string)`, status: 400 };
  }
  let raw;
  try {
    raw = atob(value);
  } catch {
    return { error: `'${field}' is not valid base64`, status: 400 };
  }
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) {
    bytes[i] = raw.charCodeAt(i);
  }
  return { bytes };
}

/**
 * The JSON response for a client input failure.
 *
 * @param {{ error: string, status: number }} failure
 * @param {string} rid
 * @param {Record<string, string>} cors
 * @returns {Response}
 */
export function clientErrorResponse(failure, rid, cors) {
  return new Response(JSON.stringify({ error: failure.error, request_id: rid }), {
    status: failure.status,
    headers: { ...cors, "Content-Type": "application/json", "X-Request-ID": rid },
  });
}
