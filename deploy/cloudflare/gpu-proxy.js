/**
 * GPU inference proxy — forwards OCR requests to an external GPU service
 * for bfloat16-quality Falcon-OCR inference.
 *
 * Supported backends (in priority order):
 *   1. HuggingFace Inference Endpoints (custom endpoint with tiiuae/Falcon-OCR)
 *   2. Replicate (push Falcon-OCR as a Cog model)
 *   3. RunPod serverless (A100/H100, lowest latency for sustained load)
 *   4. Modal (A100/H100, per-second billing, cold start ~30s)
 *   5. Fly.io GPU Machines (A100/H100, $2.50/hr, persistent)
 *
 * Required env vars:
 *   GPU_INFERENCE_URL    — Full URL of the GPU inference endpoint
 *   GPU_INFERENCE_KEY    — Bearer token / API key for the endpoint
 *   GPU_INFERENCE_PROVIDER — One of: huggingface, replicate, runpod, modal, flyio
 *
 * Docker image for self-hosted backends: ghcr.io/tiiuae/falcon-ocr:latest
 *
 * GPU requirements:
 *   - Minimum: NVIDIA T4 (16 GB VRAM) for INT8 inference
 *   - Recommended: A100 40 GB for bfloat16 full-precision inference
 *   - bfloat16 requires: CUDA compute capability >= 8.0 (A100, H100, L40S)
 */

const SUPPORTED_GPU_PROVIDERS = new Set(["huggingface", "replicate", "runpod", "modal", "flyio"]);

function normalizeProvider(provider) {
    const normalized = String(provider || "").trim().toLowerCase();
    if (!SUPPORTED_GPU_PROVIDERS.has(normalized)) {
        throw new Error(
            `Unsupported GPU_INFERENCE_PROVIDER '${provider}'. Expected one of: ${Array.from(SUPPORTED_GPU_PROVIDERS).join(", ")}`
        );
    }
    return normalized;
}

function normalizeEndpoint(endpoint) {
    try {
        const url = new URL(String(endpoint || ""));
        if (url.protocol !== "https:") {
            throw new Error("GPU_INFERENCE_URL must use https");
        }
        return url.href;
    } catch (err) {
        throw new Error(`Invalid GPU_INFERENCE_URL: ${err.message}`);
    }
}

function buildProviderRequest(provider, gpuEndpoint, gpuKey, imageBase64, options = {}) {
    provider = normalizeProvider(provider);
    gpuEndpoint = normalizeEndpoint(gpuEndpoint);
    const maxTokens = options.maxTokens || 512;
    const category = options.category || "plain";

    switch (provider) {
        case "huggingface":
            return {
                url: gpuEndpoint,
                headers: {
                    "Content-Type": "application/json",
                    "Authorization": `Bearer ${gpuKey}`,
                },
                body: JSON.stringify({
                    inputs: imageBase64,
                    parameters: { max_new_tokens: maxTokens, task: "image-to-text" },
                }),
            };
        case "replicate":
            return {
                url: gpuEndpoint,
                headers: {
                    "Content-Type": "application/json",
                    "Authorization": `Bearer ${gpuKey}`,
                    "Prefer": "wait",
                },
                body: JSON.stringify({
                    version: options.replicateVersion || undefined,
                    input: {
                        image: `data:image/png;base64,${imageBase64}`,
                        category,
                        max_tokens: maxTokens,
                    },
                }),
            };
        case "runpod":
            return {
                url: gpuEndpoint,
                headers: {
                    "Content-Type": "application/json",
                    "Authorization": `Bearer ${gpuKey}`,
                },
                body: JSON.stringify({
                    input: { image: imageBase64, category, max_tokens: maxTokens },
                }),
            };
        case "modal":
        case "flyio":
            return {
                url: gpuEndpoint,
                headers: {
                    "Content-Type": "application/json",
                    "Authorization": `Bearer ${gpuKey}`,
                },
                body: JSON.stringify({
                    model: "tiiuae/Falcon-OCR",
                    image: imageBase64,
                    category,
                    max_tokens: maxTokens,
                }),
            };
    }
}

function parseProviderResponse(provider, responseData) {
    provider = normalizeProvider(provider);
    switch (provider) {
        case "huggingface":
            if (Array.isArray(responseData) && responseData.length > 0) {
                return {
                    text: responseData[0].generated_text || "",
                    confidence: 0.95,
                    backend: "gpu-huggingface",
                    model: "tiiuae/Falcon-OCR",
                };
            }
            return { text: responseData.generated_text || "", confidence: 0.95, backend: "gpu-huggingface", model: "tiiuae/Falcon-OCR" };
        case "replicate": {
            const output = Array.isArray(responseData.output)
                ? responseData.output.join("")
                : (responseData.output || "");
            return { text: output, confidence: 0.95, backend: "gpu-replicate", model: "tiiuae/Falcon-OCR" };
        }
        case "runpod":
            return {
                text: responseData.output?.text || responseData.output || "",
                confidence: 0.95,
                backend: "gpu-runpod",
                model: "tiiuae/Falcon-OCR",
            };
        case "modal":
        case "flyio":
            return {
                text: responseData.text || "",
                confidence: responseData.confidence || 0.95,
                backend: `gpu-${provider}`,
                model: responseData.model || "tiiuae/Falcon-OCR",
            };
    }
}

export async function proxyToGPU(env, imageBase64, options = {}) {
    const gpuEndpoint = env.GPU_INFERENCE_URL;
    const gpuKey = env.GPU_INFERENCE_KEY;
    const status = gpuInferenceStatus(env);
    const provider = status.provider;

    if (!status.configured) return null;

    const timeoutMs = options.timeout || 30_000;
    const startMs = Date.now();
    const req = buildProviderRequest(provider, gpuEndpoint, gpuKey, imageBase64, options);
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), timeoutMs);

    try {
        const response = await fetch(req.url, {
            method: "POST",
            headers: req.headers,
            body: req.body,
            signal: controller.signal,
        });
        clearTimeout(timeoutId);
        if (!response.ok) {
            console.error(`GPU proxy error: ${response.status} ${response.statusText} [provider=${provider}]`);
            return null;
        }
        const responseData = await response.json();
        const result = parseProviderResponse(provider, responseData);
        result.latency_ms = Date.now() - startMs;
        return result;
    } catch (err) {
        clearTimeout(timeoutId);
        if (err.name === "AbortError") {
            console.error(`GPU proxy timeout after ${timeoutMs}ms [provider=${provider}]`);
        } else {
            console.error(`GPU proxy error: ${err.message} [provider=${provider}]`);
        }
        return null;
    }
}

export function gpuInferenceStatus(env) {
    const providerRaw = env.GPU_INFERENCE_PROVIDER;
    const endpointRaw = env.GPU_INFERENCE_URL;
    if (!endpointRaw || !env.GPU_INFERENCE_KEY || !providerRaw) {
        return {
            configured: false,
            provider: providerRaw || "none",
            endpoint: "none",
            error: "GPU_INFERENCE_URL, GPU_INFERENCE_KEY, and GPU_INFERENCE_PROVIDER are required",
        };
    }
    try {
        const provider = normalizeProvider(providerRaw);
        const endpoint = new URL(normalizeEndpoint(endpointRaw));
        return {
            configured: true,
            provider,
            endpoint: endpoint.hostname,
        };
    } catch (err) {
        return {
            configured: false,
            provider: providerRaw || "none",
            endpoint: "none",
            error: err.message,
        };
    }
}

export { buildProviderRequest, parseProviderResponse, normalizeProvider };
