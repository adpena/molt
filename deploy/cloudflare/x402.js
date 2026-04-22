/**
 * x402 payment protocol middleware for Cloudflare Workers.
 *
 * Implements the x402 HTTP payment protocol:
 *   - Parses X-Payment-402 header containing a JSON payment proof
 *   - Verifies the payment signature against the expected wallet
 *   - Checks the payment amount meets the per-request price
 *   - Returns 402 Payment Required with payment instructions if invalid
 *   - Returns the original handler response with X-Payment-Receipt on success
 *
 * Protocol reference: https://www.x402.org/
 *
 * The payment proof header format (JSON, base64-encoded):
 * {
 *   "version": "1",
 *   "network": "base",
 *   "payload": {
 *     "sender": "0x...",
 *     "recipient": "0x...",
 *     "amount": "1000",         // in smallest unit (wei for ETH, etc.)
 *     "currency": "USDC",
 *     "timestamp": 1712000000,
 *     "nonce": "abc123"
 *   },
 *   "signature": "0x..."
 * }
 */

/**
 * Per-request price in USD. Matches MCP tool definition pricing.
 * @type {number}
 */
const PRICE_USD = 0.001;

/**
 * USDC has 6 decimal places. $0.001 = 1000 units.
 * @type {bigint}
 */
const PRICE_USDC_UNITS = 1000n;

/**
 * Maximum clock skew tolerance for payment timestamp (seconds).
 * @type {number}
 */
const MAX_TIMESTAMP_SKEW_SECONDS = 300;

/**
 * Parse and validate the X-Payment-402 header.
 *
 * @param {string} headerValue - Raw header value (base64-encoded JSON)
 * @returns {{ valid: boolean, proof?: PaymentProof, error?: string }}
 *
 * @typedef {{
 *   version: string,
 *   network: string,
 *   payload: {
 *     sender: string,
 *     recipient: string,
 *     amount: string,
 *     currency: string,
 *     timestamp: number,
 *     nonce: string,
 *   },
 *   signature: string,
 * }} PaymentProof
 */
function parsePaymentHeader(headerValue) {
  try {
    const decoded = atob(headerValue);
    const proof = JSON.parse(decoded);

    // Validate required fields
    if (proof.version !== "1") {
      return { valid: false, error: `Unsupported x402 version: ${proof.version}` };
    }

    if (!proof.payload) {
      return { valid: false, error: "Missing payment payload" };
    }

    const { sender, recipient, amount, currency, timestamp, nonce } = proof.payload;

    if (!sender || typeof sender !== "string") {
      return { valid: false, error: "Missing or invalid sender address" };
    }
    if (!recipient || typeof recipient !== "string") {
      return { valid: false, error: "Missing or invalid recipient address" };
    }
    if (!amount || typeof amount !== "string") {
      return { valid: false, error: "Missing or invalid payment amount" };
    }
    if (!currency || typeof currency !== "string") {
      return { valid: false, error: "Missing or invalid currency" };
    }
    if (typeof timestamp !== "number") {
      return { valid: false, error: "Missing or invalid timestamp" };
    }
    if (!nonce || typeof nonce !== "string") {
      return { valid: false, error: "Missing or invalid nonce" };
    }
    if (!proof.signature || typeof proof.signature !== "string") {
      return { valid: false, error: "Missing payment signature" };
    }

    return { valid: true, proof };
  } catch (err) {
    return { valid: false, error: `Failed to parse X-Payment-402 header: ${err.message}` };
  }
}

/**
 * Verify that the payment amount meets the per-request price.
 *
 * @param {PaymentProof} proof
 * @returns {{ valid: boolean, error?: string }}
 */
function verifyPaymentAmount(proof) {
  const { amount, currency } = proof.payload;

  if (currency !== "USDC") {
    return { valid: false, error: `Unsupported currency: ${currency}. Only USDC accepted.` };
  }

  try {
    const amountBigInt = BigInt(amount);
    if (amountBigInt < PRICE_USDC_UNITS) {
      return {
        valid: false,
        error: `Insufficient payment: ${amount} USDC units (minimum: ${PRICE_USDC_UNITS})`,
      };
    }
    return { valid: true };
  } catch {
    return { valid: false, error: `Invalid payment amount: ${amount}` };
  }
}

/**
 * Verify that the payment recipient matches the configured wallet.
 *
 * @param {PaymentProof} proof
 * @param {string} expectedWallet
 * @returns {{ valid: boolean, error?: string }}
 */
function verifyRecipient(proof, expectedWallet) {
  if (proof.payload.recipient.toLowerCase() !== expectedWallet.toLowerCase()) {
    return { valid: false, error: "Payment recipient does not match expected wallet" };
  }
  return { valid: true };
}

/**
 * Verify that the payment timestamp is within acceptable skew.
 *
 * @param {PaymentProof} proof
 * @returns {{ valid: boolean, error?: string }}
 */
function verifyTimestamp(proof) {
  const now = Math.floor(Date.now() / 1000);
  const skew = Math.abs(now - proof.payload.timestamp);
  if (skew > MAX_TIMESTAMP_SKEW_SECONDS) {
    return {
      valid: false,
      error: `Payment timestamp too far from server time (skew: ${skew}s, max: ${MAX_TIMESTAMP_SKEW_SECONDS}s)`,
    };
  }
  return { valid: true };
}

/**
 * Verify the payment signature using the x402 verification endpoint.
 *
 * @param {PaymentProof} proof
 * @param {string} verificationUrl
 * @returns {Promise<{ valid: boolean, error?: string, receipt?: string }>}
 */
async function verifySignature(proof, verificationUrl) {
  try {
    const response = await fetch(verificationUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        version: proof.version,
        network: proof.network,
        payload: proof.payload,
        signature: proof.signature,
      }),
    });

    if (!response.ok) {
      return { valid: false, error: `Signature verification endpoint returned ${response.status}` };
    }

    const result = await response.json();
    if (!result.valid) {
      return { valid: false, error: result.error || "Signature verification failed" };
    }

    return { valid: true, receipt: result.receipt || null };
  } catch (err) {
    return { valid: false, error: `Signature verification error: ${err.message}` };
  }
}

/**
 * Build the 402 Payment Required response with payment instructions.
 *
 * @param {string} error - Human-readable error message
 * @param {string} rid - Request ID
 * @param {string} walletAddress - Recipient wallet address
 * @param {Record<string, string>} cors - CORS headers
 * @returns {Response}
 */
function paymentRequiredResponse(error, rid, walletAddress, cors) {
  return new Response(
    JSON.stringify({
      error,
      request_id: rid,
      payment_required: {
        version: "1",
        network: "base",
        recipient: walletAddress,
        amount: String(PRICE_USDC_UNITS),
        currency: "USDC",
        description: "Falcon-OCR inference: $0.001 per request",
        price_usd: PRICE_USD,
        quality: "GPU bfloat16 (highest quality)",
        speed: "~1s inference on A10G",
        engine: "falcon-ocr",
      },
    }),
    {
      status: 402,
      headers: {
        ...cors,
        "Content-Type": "application/json",
        "X-Payment-Version": "1",
        "X-Payment-Network": "base",
        "X-Payment-Currency": "USDC",
        "X-Payment-Amount": String(PRICE_USDC_UNITS),
        "X-Payment-Recipient": walletAddress,
      },
    },
  );
}

/**
 * Full x402 payment verification pipeline.
 *
 * @param {Request} request
 * @param {object} env - Worker environment bindings
 * @param {string} rid - Request ID for tracing
 * @param {Record<string, string>} cors - CORS headers
 * @returns {Promise<{ authorized: boolean, response?: Response, receipt?: string }>}
 */
export async function verifyX402(request, env, rid, cors) {
  const walletAddress = env.X402_WALLET_ADDRESS;

  // Dev mode: if no wallet configured, skip payment verification
  if (!walletAddress) {
    return { authorized: true };
  }

  // Browser bypass: skip x402 for same-origin requests from the production
  // app.  Browser users are already authenticated via the enjoice session.
  // x402 is only enforced for API/agent requests (no Origin or different origin).
  const origin = request.headers.get("Origin");
  const allowedOrigin = env.CORS_ORIGIN || "https://freeinvoicemaker.app";
  if (origin && origin === allowedOrigin) {
    return { authorized: true };
  }

  const headerValue = request.headers.get("X-Payment-402");
  if (!headerValue) {
    return {
      authorized: false,
      response: paymentRequiredResponse(
        "Missing X-Payment-402 header",
        rid,
        walletAddress,
        cors,
      ),
    };
  }

  // 1. Parse the payment header
  const parsed = parsePaymentHeader(headerValue);
  if (!parsed.valid) {
    return {
      authorized: false,
      response: paymentRequiredResponse(parsed.error, rid, walletAddress, cors),
    };
  }

  const proof = parsed.proof;

  // 2. Verify recipient
  const recipientCheck = verifyRecipient(proof, walletAddress);
  if (!recipientCheck.valid) {
    return {
      authorized: false,
      response: paymentRequiredResponse(recipientCheck.error, rid, walletAddress, cors),
    };
  }

  // 3. Verify amount
  const amountCheck = verifyPaymentAmount(proof);
  if (!amountCheck.valid) {
    return {
      authorized: false,
      response: paymentRequiredResponse(amountCheck.error, rid, walletAddress, cors),
    };
  }

  // 4. Verify timestamp
  const timestampCheck = verifyTimestamp(proof);
  if (!timestampCheck.valid) {
    return {
      authorized: false,
      response: paymentRequiredResponse(timestampCheck.error, rid, walletAddress, cors),
    };
  }

  // 5. Verify signature via the verification endpoint
  const verificationUrl = env.X402_VERIFICATION_URL;
  if (!verificationUrl) {
    return {
      authorized: false,
      response: paymentRequiredResponse(
        "x402 verification endpoint not configured",
        rid,
        walletAddress,
        cors,
      ),
    };
  }

  const sigCheck = await verifySignature(proof, verificationUrl);
  if (!sigCheck.valid) {
    return {
      authorized: false,
      response: paymentRequiredResponse(sigCheck.error, rid, walletAddress, cors),
    };
  }

  return { authorized: true, receipt: sigCheck.receipt };
}
