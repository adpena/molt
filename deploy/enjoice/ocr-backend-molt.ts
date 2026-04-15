/**
 * OCR backend implementation for enjoice using molt-compiled Falcon-OCR.
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
  private config: FalconOcrConfig;
  private initTimingMs = 0;
  private webGpuAvailable = false;
  private initError: string | null = null;

  constructor(config: FalconOcrConfig) {
    this.config = config;
  }

  /**
   * Initialize the WASM module and model. Call once before recognize().
   * Returns false if initialization failed (caller should use fallback).
   */
  async initialize(): Promise<boolean> {
    const initStart = performance.now();

    try {
      // Check WebGPU availability (informational — the WASM module handles
      // GPU dispatch internally when available)
      const gpu = await detectWebGpu();
      this.webGpuAvailable = gpu.available;

      // Create the WASM session
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
   * Release all resources held by the backend.
   */
  dispose(): void {
    if (this.session) {
      this.session.dispose();
      this.session = null;
    }
  }
}
