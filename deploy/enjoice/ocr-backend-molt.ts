/**
 * OCR backend implementation for enjoice using molt-compiled Falcon-OCR.
 *
 * Architecture:
 *   - On WebGPU/WASM-capable browsers: uses Falcon-OCR WASM for local inference
 *     (offline, private — no image data leaves the device).
 *   - Fallback: PaddleOCR via the Worker endpoint.
 *
 * Implements the OcrBackend interface expected by site/src/lib/ocr/index.ts.
 * Handles WebGPU detection, graceful fallback, and TTFB measurement.
 *
 * Usage:
 *   import { MoltOcrBackend } from "./ocr-backend-molt";
 *   const backend = new MoltOcrBackend(config);
 *   await backend.initialize();
 *   const result = await backend.recognize(imageData);
 */

import {
  createFalconOcrSession,
  type FalconOcrConfig,
  type FalconOcrSession,
  type OcrResult,
} from "./falcon-ocr-molt";

// Browser-side Falcon-OCR WASM loader for offline inference
import type { FalconOCR as FalconOCRLoader } from "../browser/falcon-ocr-loader.js";

// --------------------------------------------------------------------------
// Types matching enjoice's OcrBackend interface
// --------------------------------------------------------------------------

export interface OcrBackendResult {
  text: string;
  boundingBoxes: Array<{
    x: number;
    y: number;
    width: number;
    height: number;
    text: string;
    confidence: number;
  }>;
  confidence: number;
}

export interface OcrTimings {
  initMs: number;
  inferenceMs: number;
  totalMs: number;
  ttfbMs: number;
}

export interface OcrBackendStatus {
  available: boolean;
  name: string;
  device: "webgpu" | "wasm" | "none";
  reason?: string;
}

// --------------------------------------------------------------------------
// WebGPU detection
// --------------------------------------------------------------------------

async function detectWebGpu(): Promise<{
  available: boolean;
  adapter: GPUAdapter | null;
  reason?: string;
}> {
  if (typeof navigator === "undefined" || !("gpu" in navigator)) {
    return { available: false, adapter: null, reason: "navigator.gpu not available" };
  }

  try {
    const adapter = await navigator.gpu.requestAdapter({
      powerPreference: "high-performance",
    });
    if (!adapter) {
      return { available: false, adapter: null, reason: "No GPU adapter found" };
    }
    return { available: true, adapter };
  } catch (err) {
    return {
      available: false,
      adapter: null,
      reason: `GPU adapter request failed: ${(err as Error).message}`,
    };
  }
}

// --------------------------------------------------------------------------
// Backend implementation
// --------------------------------------------------------------------------

export class MoltOcrBackend {
  private session: FalconOcrSession | null = null;
  private browserOcr: FalconOCRLoader | null = null;
  private config: FalconOcrConfig;
  private initTimingMs = 0;
  private webGpuAvailable = false;
  private initError: string | null = null;
  private usingBrowserWasm = false;

  constructor(config: FalconOcrConfig) {
    this.config = config;
  }

  /**
   * Initialize the OCR backend. Attempts browser-side WASM inference first
   * (offline, private), falls back to the session-based approach.
   * Returns false if initialization failed (caller should use PaddleOCR fallback).
   */
  async initialize(): Promise<boolean> {
    const initStart = performance.now();

    try {
      // Check WebGPU availability
      const gpu = await detectWebGpu();
      this.webGpuAvailable = gpu.available;

      // On WebGPU/WASM-capable browsers, prefer local Falcon-OCR WASM inference.
      // This keeps all image data on-device (privacy-first).
      if (typeof WebAssembly !== "undefined") {
        try {
          const { FalconOCR } = await import("../browser/falcon-ocr-loader.js");
          const loader = new FalconOCR({
            baseUrl: this.config.workerUrl || "https://falcon-ocr.adpena.workers.dev",
            onProgress: this.config.onProgress,
          });
          await loader.init();
          this.browserOcr = loader as unknown as FalconOCRLoader;
          this.usingBrowserWasm = true;
          this.initTimingMs = performance.now() - initStart;
          return true;
        } catch (wasmErr) {
          // WASM init failed — fall through to session-based approach
          console.warn(
            `Falcon-OCR WASM init failed, falling back to session: ${(wasmErr as Error).message}`,
          );
        }
      }

      // Fallback: Create the WASM session (original path)
      this.session = await createFalconOcrSession(this.config);
      this.initTimingMs = performance.now() - initStart;
      return true;
    } catch (err) {
      this.initTimingMs = performance.now() - initStart;
      this.initError = (err as Error).message;
      return false;
    }
  }

  /**
   * Run OCR on an ImageData (from canvas) or raw RGB buffer.
   */
  async recognize(
    image: ImageData | { width: number; height: number; rgb: Uint8Array },
  ): Promise<{ result: OcrBackendResult; timings: OcrTimings }> {
    // Browser WASM path: all inference runs locally, no network
    if (this.usingBrowserWasm && this.browserOcr) {
      const totalStart = performance.now();
      const wasmResult = await (this.browserOcr as any).recognize(image);
      const totalMs = performance.now() - totalStart;

      return {
        result: {
          text: wasmResult.text,
          boundingBoxes: [], // WASM model returns raw text only; bounding boxes require post-processing
          confidence: 0.95, // INT4 accuracy estimate
        },
        timings: {
          initMs: this.initTimingMs,
          inferenceMs: wasmResult.timeMs,
          totalMs,
          ttfbMs: 0, // No network — zero TTFB
        },
      };
    }

    // Session-based path (original)
    if (!this.session || !this.session.ready) {
      throw new Error(
        this.initError
          ? `MoltOcrBackend not initialized: ${this.initError}`
          : "MoltOcrBackend not initialized. Call initialize() first.",
      );
    }

    const totalStart = performance.now();
    const ttfbStart = performance.now();

    const ocrResult: OcrResult = await this.session.ocr(image);

    const ttfbMs = performance.now() - ttfbStart;
    const totalMs = performance.now() - totalStart;

    return {
      result: {
        text: ocrResult.text,
        boundingBoxes: ocrResult.boundingBoxes,
        confidence: ocrResult.confidence,
      },
      timings: {
        initMs: this.initTimingMs,
        inferenceMs: ocrResult.timeMs,
        totalMs,
        ttfbMs,
      },
    };
  }

  /**
   * Query backend availability and status.
   */
  status(): OcrBackendStatus {
    if (this.initError) {
      return {
        available: false,
        name: "molt-falcon-ocr",
        device: "none",
        reason: this.initError,
      };
    }

    if (this.usingBrowserWasm && this.browserOcr) {
      return {
        available: true,
        name: "molt-falcon-ocr-browser",
        device: this.webGpuAvailable ? "webgpu" : "wasm",
      };
    }

    if (!this.session) {
      return {
        available: false,
        name: "molt-falcon-ocr",
        device: "none",
        reason: "Not initialized",
      };
    }

    return {
      available: this.session.ready,
      name: "molt-falcon-ocr",
      device: this.webGpuAvailable ? "webgpu" : "wasm",
    };
  }

  /**
   * Extract a reusable template definition from an invoice image.
   *
   * Calls the falcon-ocr Worker's /template/extract endpoint which uses
   * Workers AI for section classification and style inference.
   *
   * @param imageBase64 - Base64-encoded image bytes
   * @param options - Extraction options
   * @returns Template definition with confidence score and detected sections
   */
  async extractTemplate(
    imageBase64: string,
    options: {
      documentType?: string;
      preserveLogo?: boolean;
      detectColors?: boolean;
    } = {},
  ): Promise<{
    template: Record<string, unknown>;
    confidence: number;
    detected_sections: string[];
    time_ms: number;
  }> {
    const endpoint = this.config.workerUrl
      ? `${this.config.workerUrl.replace(/\/$/, "")}/template/extract`
      : "https://falcon-ocr.adpena.workers.dev/template/extract";

    const res = await fetch(endpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        image: imageBase64,
        document_type: options.documentType ?? "invoice",
        preserve_logo: options.preserveLogo !== false,
        detect_colors: options.detectColors !== false,
      }),
    });

    if (!res.ok) {
      const body = (await res.json().catch(() => ({}))) as { error?: string };
      throw new Error(
        body.error ?? `Template extraction failed with status ${res.status}`,
      );
    }

    return res.json() as Promise<{
      template: Record<string, unknown>;
      confidence: number;
      detected_sections: string[];
      time_ms: number;
    }>;
  }

  /**
   * Release all resources held by the backend.
   */
  dispose(): void {
    if (this.browserOcr) {
      (this.browserOcr as any).dispose();
      this.browserOcr = null;
    }
    if (this.session) {
      this.session.dispose();
      this.session = null;
    }
  }
}
