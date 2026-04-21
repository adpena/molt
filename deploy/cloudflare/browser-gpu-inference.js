// Cloudflare Browser Rendering — GPU inference on the edge.
//
// Uses Cloudflare's headless Chrome instances to run Falcon-OCR with REAL
// WebGPU acceleration on the edge. This is the only path to GPU inference
// on Cloudflare without dedicated GPU containers (which are not yet
// customer-facing).
//
// Architecture:
//   1. Worker receives OCR request with image data
//   2. Worker spawns a headless Chrome via Browser Rendering binding
//   3. Chrome loads the Falcon-OCR WebGPU inference page
//   4. Image is injected into the page via evaluate()
//   5. WebGPU-accelerated inference runs in the browser context
//   6. Results are extracted and returned to the caller
//
// Requirements:
//   - Browser Rendering binding in wrangler.toml: [browser] binding = "BROWSER"
//   - Falcon-OCR test page deployed (serves WASM + WebGPU shaders)
//   - Workers Paid plan (Browser Rendering is a paid feature)
//
// Limitations:
//   - Cold start: ~2-5 seconds to launch Chrome instance
//   - Session limit: ~2 concurrent browser sessions per worker
//   - GPU availability depends on Cloudflare's edge hardware
//   - Not all edge locations have GPU-capable Chrome instances
//
// Usage in worker.js:
//   import { inferWithBrowserGPU, isBrowserRenderingAvailable } from './browser-gpu-inference.js';
//
//   // In request handler:
//   if (isBrowserRenderingAvailable(env)) {
//       const result = await inferWithBrowserGPU(env, imageBase64);
//   }

// Cloudflare Browser Rendering uses @cloudflare/puppeteer.
// The BROWSER binding is passed to puppeteer.launch() as the connection.
import puppeteer from '@cloudflare/puppeteer';

/**
 * Check if Browser Rendering is available in the current environment.
 * @param {object} env - Worker environment bindings
 * @returns {boolean}
 */
export function isBrowserRenderingAvailable(env) {
    return env.BROWSER !== undefined;
}

/**
 * Probe WebGPU availability in Browser Rendering without loading the full model.
 * Launches Chrome, checks navigator.gpu, and returns adapter info.
 * Used to verify that GPU inference is possible before committing to a full run.
 *
 * @param {object} env - Worker environment bindings
 * @returns {Promise<object>} GPU availability info
 */
export async function probeGPU(env) {
    let browser;
    try {
        browser = await puppeteer.launch(env.BROWSER);
        const page = await browser.newPage();

        // Navigate to a minimal page (about:blank works for GPU detection)
        await page.goto('about:blank');

        const gpuInfo = await page.evaluate(async () => {
            if (!navigator.gpu) return { available: false, reason: 'no navigator.gpu' };
            try {
                const adapter = await navigator.gpu.requestAdapter();
                if (!adapter) return { available: false, reason: 'no adapter' };
                const info = await adapter.requestAdapterInfo();
                return {
                    available: true,
                    vendor: info.vendor,
                    architecture: info.architecture,
                    device: info.device,
                    description: info.description,
                };
            } catch (err) {
                return { available: false, reason: err.message };
            }
        });

        return gpuInfo;
    } finally {
        if (browser) {
            try { await browser.close(); } catch { /* best-effort */ }
        }
    }
}

/**
 * Run Falcon-OCR inference using Browser Rendering with WebGPU.
 *
 * @param {object} env - Worker environment bindings (must include BROWSER)
 * @param {string} imageBase64 - Base64-encoded image data
 * @param {object} options - Optional configuration
 * @param {string} options.inferenceUrl - URL of the Falcon-OCR inference page
 * @param {number} options.timeoutMs - Maximum inference time in ms (default: 30000)
 * @returns {Promise<{text: string, confidence: number, backend: string, latencyMs: number}>}
 */
export async function inferWithBrowserGPU(env, imageBase64, options = {}) {
    const {
        inferenceUrl = 'https://falcon-ocr.adpena.workers.dev/test',
        timeoutMs = 30000,
    } = options;

    if (!isBrowserRenderingAvailable(env)) {
        throw new Error('Browser Rendering binding not available');
    }


    const startTime = Date.now();
    let browser;
    try {
        // Cloudflare Browser Rendering: pass the BROWSER binding to puppeteer.launch()
        browser = await puppeteer.launch(env.BROWSER);
        const page = await browser.newPage();

        // Navigate to the Falcon-OCR inference page
        await page.goto(inferenceUrl, { waitUntil: 'domcontentloaded', timeout: 15000 });

        // Check if WebGPU is available BEFORE waiting for model init
        const gpuInfo = await page.evaluate(async () => {
            if (!navigator.gpu) return { available: false, reason: 'no navigator.gpu' };
            const adapter = await navigator.gpu.requestAdapter();
            if (!adapter) return { available: false, reason: 'no adapter' };
            const info = await adapter.requestAdapterInfo();
            return {
                available: true,
                vendor: info.vendor,
                architecture: info.architecture,
                device: info.device,
                description: info.description,
            };
        });

        // Wait for the OCR engine to initialize (WebGPU shader compilation, etc.)
        // Timeout is generous because model + weights loading from R2 can take 10-20s.
        await page.waitForFunction(
            () => window.__falconOCR && window.__falconOCR.ready === true,
            { timeout: timeoutMs }
        );

        // Inject the image and run inference
        const result = await page.evaluate(async (imgData) => {
            const ocr = window.__falconOCR;
            if (!ocr) throw new Error('Falcon-OCR not loaded');

            const startInference = performance.now();
            const recognition = await ocr.recognize(imgData);
            const inferenceMs = performance.now() - startInference;

            return {
                text: recognition.text,
                confidence: recognition.confidence,
                inferenceMs,
                backend: recognition.backend || 'webgpu',
            };
        }, imageBase64);

        const totalMs = Date.now() - startTime;
        return {
            text: result.text,
            confidence: result.confidence,
            backend: result.backend,
            latencyMs: totalMs,
            inferenceMs: result.inferenceMs,
            gpuInfo,
        };
    } finally {
        if (browser) {
            try {
                await browser.close();
            } catch {
                // Best-effort cleanup
            }
        }
    }
}

/**
 * Batch inference: process multiple images in a single browser session.
 * More efficient than one session per image due to Chrome startup cost.
 *
 * @param {object} env - Worker environment bindings
 * @param {string[]} images - Array of base64-encoded images
 * @param {object} options - Configuration
 * @returns {Promise<Array<{text: string, confidence: number, latencyMs: number}>>}
 */
export async function batchInferWithBrowserGPU(env, images, options = {}) {
    const {
        inferenceUrl = 'https://falcon-ocr.adpena.workers.dev/test',
        timeoutMs = 60000,
    } = options;

    if (!isBrowserRenderingAvailable(env)) {
        throw new Error('Browser Rendering binding not available');
    }


    let browser;
    try {
        browser = await puppeteer.launch(env.BROWSER);
        const page = await browser.newPage();
        await page.goto(inferenceUrl, { waitUntil: 'networkidle0' });
        await page.waitForFunction(
            () => window.__falconOCR && window.__falconOCR.ready === true,
            { timeout: timeoutMs }
        );

        const results = [];
        for (const imgData of images) {
            const result = await page.evaluate(async (img) => {
                const start = performance.now();
                const r = await window.__falconOCR.recognize(img);
                return {
                    text: r.text,
                    confidence: r.confidence,
                    latencyMs: performance.now() - start,
                };
            }, imgData);
            results.push(result);
        }
        return results;
    } finally {
        if (browser) {
            try { await browser.close(); } catch { /* best-effort */ }
        }
    }
}
