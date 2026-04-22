/**
 * Browser capability detection for molt Falcon-OCR.
 *
 * Detects WebGPU, WebGL2, identifies browser, and determines the optimal
 * OCR backend path. Drop-in addition for enjoice's capabilities module.
 *
 * Usage:
 *   import { detectOcrCapabilities } from "./capabilities-update";
 *   const caps = await detectOcrCapabilities();
 *   if (caps.recommendedBackend === "molt-gpu") { ... }
 */

// --------------------------------------------------------------------------
// Types
// --------------------------------------------------------------------------

export interface BrowserInfo {
  name: "chrome" | "safari" | "edge" | "firefox" | "unknown";
  version: number;
  mobile: boolean;
}

export interface GpuCapabilities {
  webGpuAvailable: boolean;
  webGl2Available: boolean;
  adapterName: string | null;
  /** Approximate dispatch overhead in microseconds, null if unknown. */
  estimatedDispatchOverheadUs: number | null;
}

// Product-level backend choices consumed by enjoice's OCR factory. `molt-gpu`
// delegates to MoltOcrBackend, which selects its internal Falcon WebGPU/WASM
// engine lane separately.
export type OcrBackendChoice = "molt-gpu" | "paddle-wasm" | "server-side";

export interface OcrCapabilities {
  browser: BrowserInfo;
  gpu: GpuCapabilities;
  recommendedBackend: OcrBackendChoice;
  warnings: string[];
  /** If true, a feature flag overrides the automatic selection. */
  featureFlagOverride: boolean;
}

// --------------------------------------------------------------------------
// Browser detection
// --------------------------------------------------------------------------

export function detectBrowser(): BrowserInfo {
  if (typeof navigator === "undefined") {
    return { name: "unknown", version: 0, mobile: false };
  }

  const ua = navigator.userAgent;
  const mobile = /Mobile|Android|iPhone|iPad/.test(ua);

  // Order matters: Edge includes "Chrome" in UA, check Edge first.
  if (/Edg\/(\d+)/.test(ua)) {
    const version = parseInt(RegExp.$1, 10);
    return { name: "edge", version, mobile };
  }

  if (/Chrome\/(\d+)/.test(ua) && !/Edg/.test(ua)) {
    const version = parseInt(RegExp.$1, 10);
    return { name: "chrome", version, mobile };
  }

  if (/Version\/(\d+).*Safari/.test(ua) && !/Chrome/.test(ua)) {
    const version = parseInt(RegExp.$1, 10);
    return { name: "safari", version, mobile };
  }

  if (/Firefox\/(\d+)/.test(ua)) {
    const version = parseInt(RegExp.$1, 10);
    return { name: "firefox", version, mobile };
  }

  return { name: "unknown", version: 0, mobile };
}

// --------------------------------------------------------------------------
// GPU detection
// --------------------------------------------------------------------------

async function detectGpu(): Promise<GpuCapabilities> {
  const result: GpuCapabilities = {
    webGpuAvailable: false,
    webGl2Available: false,
    adapterName: null,
    estimatedDispatchOverheadUs: null,
  };

  // WebGL2 check
  if (typeof document !== "undefined") {
    try {
      const canvas = document.createElement("canvas");
      const gl = canvas.getContext("webgl2");
      result.webGl2Available = gl !== null;
    } catch {
      // WebGL2 not available
    }
  }

  // WebGPU check
  if (typeof navigator !== "undefined" && "gpu" in navigator) {
    try {
      const adapter = await navigator.gpu.requestAdapter({
        powerPreference: "high-performance",
      });
      if (adapter) {
        result.webGpuAvailable = true;
        // adapterInfo may not exist in all implementations
        if ("info" in adapter) {
          const info = (adapter as GPUAdapter & { info: { description?: string } }).info;
          result.adapterName = info.description ?? null;
        }
      }
    } catch {
      // WebGPU not available
    }
  }

  return result;
}

// --------------------------------------------------------------------------
// Dispatch overhead estimates (from GPU parallelism spec)
// --------------------------------------------------------------------------

/**
 * Estimated dispatch overhead per browser, sourced from spec
 * docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md.
 */
const DISPATCH_OVERHEAD_US: Record<string, number> = {
  chrome: 30,    // Dawn/Vulkan: 24-36 us
  safari: 32,    // Metal: ~32 us
  edge: 63,      // Dawn/D3D12: 59-67 us
  firefox: 1037, // wgpu: ~1037 us (rate-limited per Maczan 2026)
};

// --------------------------------------------------------------------------
// Feature flag
// --------------------------------------------------------------------------

/**
 * Check if a feature flag forces a specific backend.
 * Reads from window.__MOLT_OCR_BACKEND (set by enjoice's config).
 */
function readFeatureFlag(): OcrBackendChoice | null {
  if (typeof window === "undefined") return null;
  const flag = (window as unknown as Record<string, unknown>).__MOLT_OCR_BACKEND;
  if (flag === "molt-gpu" || flag === "paddle-wasm" || flag === "server-side") {
    return flag;
  }
  return null;
}

// --------------------------------------------------------------------------
// Main detection
// --------------------------------------------------------------------------

export async function detectOcrCapabilities(): Promise<OcrCapabilities> {
  const browser = detectBrowser();
  const gpu = await detectGpu();
  const warnings: string[] = [];

  // Set estimated dispatch overhead
  gpu.estimatedDispatchOverheadUs = DISPATCH_OVERHEAD_US[browser.name] ?? null;

  // Determine recommended backend
  let recommendedBackend: OcrBackendChoice;

  if (!gpu.webGpuAvailable) {
    recommendedBackend = "paddle-wasm";
    if (!gpu.webGl2Available) {
      warnings.push("Neither WebGPU nor WebGL2 available. Server-side OCR recommended.");
      recommendedBackend = "server-side";
    }
  } else if (browser.name === "firefox") {
    // Firefox WebGPU dispatch is rate-limited (~1037 us per dispatch).
    // Inference is functional but slow. Default to PaddleOCR for Firefox.
    recommendedBackend = "paddle-wasm";
    warnings.push(
      "Firefox WebGPU dispatch overhead is ~1037 us (rate-limited per Maczan 2026). " +
      "Using PaddleOCR fallback for better performance. " +
      "Set __MOLT_OCR_BACKEND = 'molt-gpu' to override.",
    );
  } else {
    recommendedBackend = "molt-gpu";
  }

  // Check feature flag override
  const flagOverride = readFeatureFlag();
  const featureFlagOverride = flagOverride !== null;
  if (featureFlagOverride) {
    recommendedBackend = flagOverride;
  }

  return {
    browser,
    gpu,
    recommendedBackend,
    warnings,
    featureFlagOverride,
  };
}
