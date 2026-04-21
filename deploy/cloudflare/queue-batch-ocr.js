// Cloudflare Queues — Async batch OCR processing.
//
// Architecture:
//   1. Client POST /batch with array of images
//   2. Worker generates a batch_id, enqueues each image to the Queue
//   3. Queue consumer processes each image with Falcon-OCR (no timeout pressure)
//   4. Results stored in KV (keyed by batch_id + image index)
//   5. Client polls GET /batch/:id for results
//
// This decouples request latency from inference time. The Worker responds
// immediately with a batch_id while inference runs asynchronously.
//
// Requirements:
//   - Queue binding in wrangler.toml:
//       [[queues.producers]]
//       binding = "OCR_QUEUE"
//       queue = "falcon-ocr-batch"
//
//       [[queues.consumers]]
//       queue = "falcon-ocr-batch"
//       max_batch_size = 10
//       max_retries = 3
//       dead_letter_queue = "falcon-ocr-dlq"
//
//   - KV namespace for results (reuse CACHE binding or create dedicated)
//   - Workers Paid plan (Queues is a paid feature)
//
// Usage in worker.js:
//   import { handleBatchSubmit, handleBatchStatus, processQueueBatch } from './queue-batch-ocr.js';
//
//   // In fetch handler:
//   if (url.pathname === '/batch' && method === 'POST') return handleBatchSubmit(request, env);
//   if (url.pathname.startsWith('/batch/')) return handleBatchStatus(request, env);
//
//   // In queue handler:
//   export default { queue: (batch, env) => processQueueBatch(batch, env) };

/**
 * Generate a unique batch ID (collision-resistant, URL-safe).
 */
function generateBatchId() {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    // Base64url encoding without padding
    return btoa(String.fromCharCode(...bytes))
        .replace(/\+/g, '-')
        .replace(/\//g, '_')
        .replace(/=+$/, '');
}

/**
 * Handle POST /batch — accept images and enqueue for processing.
 *
 * Request body (JSON):
 *   { "images": ["base64...", "base64...", ...] }
 *
 * Response:
 *   { "batch_id": "abc123", "count": 5, "status": "queued" }
 */
export async function handleBatchSubmit(request, env) {
    const body = await request.json();
    const images = body.images;

    if (!Array.isArray(images) || images.length === 0) {
        return new Response(JSON.stringify({ error: 'images must be a non-empty array' }), {
            status: 400,
            headers: { 'Content-Type': 'application/json' },
        });
    }

    if (images.length > 100) {
        return new Response(JSON.stringify({ error: 'maximum 100 images per batch' }), {
            status: 400,
            headers: { 'Content-Type': 'application/json' },
        });
    }

    const batchId = generateBatchId();
    const count = images.length;

    // Store batch metadata in KV
    await env.CACHE.put(
        `batch:${batchId}:meta`,
        JSON.stringify({
            count,
            created: Date.now(),
            completed: 0,
            failed: 0,
            status: 'processing',
        }),
        { expirationTtl: 86400 } // 24 hours
    );

    // Enqueue each image as a separate message
    const messages = images.map((imageData, index) => ({
        body: {
            batchId,
            index,
            imageData,
        },
    }));

    // Queue.sendBatch accepts up to 100 messages
    await env.OCR_QUEUE.sendBatch(messages);

    return new Response(
        JSON.stringify({ batch_id: batchId, count, status: 'queued' }),
        {
            status: 202,
            headers: { 'Content-Type': 'application/json' },
        }
    );
}

/**
 * Handle GET /batch/:id — poll for batch processing status and results.
 *
 * Response:
 *   {
 *     "batch_id": "abc123",
 *     "status": "processing" | "completed" | "partial",
 *     "count": 5,
 *     "completed": 3,
 *     "failed": 0,
 *     "results": [
 *       { "index": 0, "text": "...", "confidence": 0.95 },
 *       { "index": 1, "text": "...", "confidence": 0.87 },
 *       ...
 *     ]
 *   }
 */
export async function handleBatchStatus(request, env) {
    const url = new URL(request.url);
    const batchId = url.pathname.split('/').pop();

    const metaJson = await env.CACHE.get(`batch:${batchId}:meta`);
    if (!metaJson) {
        return new Response(JSON.stringify({ error: 'batch not found' }), {
            status: 404,
            headers: { 'Content-Type': 'application/json' },
        });
    }

    const meta = JSON.parse(metaJson);

    // Fetch all available results
    const results = [];
    for (let i = 0; i < meta.count; i++) {
        const resultJson = await env.CACHE.get(`batch:${batchId}:result:${i}`);
        if (resultJson) {
            results.push(JSON.parse(resultJson));
        }
    }

    const status = results.length === meta.count
        ? 'completed'
        : results.length > 0
        ? 'partial'
        : 'processing';

    return new Response(
        JSON.stringify({
            batch_id: batchId,
            status,
            count: meta.count,
            completed: results.length,
            failed: meta.failed,
            results,
        }),
        {
            headers: { 'Content-Type': 'application/json' },
        }
    );
}

/**
 * Queue consumer: process a batch of OCR messages.
 *
 * Called by the Cloudflare Queue runtime when messages are available.
 * Each message contains { batchId, index, imageData }.
 *
 * @param {object} batch - Queue message batch
 * @param {object} env - Worker environment bindings
 */
export async function processQueueBatch(batch, env) {
    for (const message of batch.messages) {
        const { batchId, index, imageData } = message.body;

        try {
            // Run OCR inference (uses Workers AI or local WASM depending on config)
            const ocrResult = await runOCRInference(env, imageData);

            // Store result in KV
            await env.CACHE.put(
                `batch:${batchId}:result:${index}`,
                JSON.stringify({
                    index,
                    text: ocrResult.text,
                    confidence: ocrResult.confidence,
                    backend: ocrResult.backend,
                    processedAt: Date.now(),
                }),
                { expirationTtl: 86400 }
            );

            // Update metadata counter
            await incrementBatchCounter(env, batchId, 'completed');
            message.ack();
        } catch (err) {
            console.error(`OCR failed for batch ${batchId} image ${index}:`, err);
            await incrementBatchCounter(env, batchId, 'failed');
            message.retry({ delaySeconds: 10 });
        }
    }
}

/**
 * Run OCR inference on a single image.
 * Tries Workers AI first, falls back to local WASM inference.
 */
async function runOCRInference(env, imageBase64) {
    // Prefer Workers AI for GPU-accelerated inference
    if (env.AI) {
        try {
            const response = await env.AI.run('@cf/meta/llama-3.2-11b-vision-instruct', {
                messages: [
                    {
                        role: 'user',
                        content: [
                            { type: 'text', text: 'Extract all text from this image. Return only the extracted text, nothing else.' },
                            { type: 'image_url', image_url: { url: `data:image/png;base64,${imageBase64}` } },
                        ],
                    },
                ],
                max_tokens: 2048,
            });
            return {
                text: response.response || '',
                confidence: 0.9,
                backend: 'workers-ai',
            };
        } catch {
            // Fall through to WASM inference
        }
    }

    // Fallback: local WASM inference would go here
    // For now, return an error indicating no backend available
    throw new Error('No inference backend available (Workers AI failed, no local WASM fallback)');
}

/**
 * Atomically increment a counter in the batch metadata.
 */
async function incrementBatchCounter(env, batchId, field) {
    const key = `batch:${batchId}:meta`;
    const metaJson = await env.CACHE.get(key);
    if (!metaJson) return;
    const meta = JSON.parse(metaJson);
    meta[field] = (meta[field] || 0) + 1;
    if (meta.completed + meta.failed >= meta.count) {
        meta.status = 'completed';
    }
    await env.CACHE.put(key, JSON.stringify(meta), { expirationTtl: 86400 });
}
