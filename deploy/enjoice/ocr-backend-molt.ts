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

export interface FalconOcrResult {
  text: string;
  tokens: number[];
  engine: "falcon-ocr";
  model: string;
  model_used: string;
  backend: string;
  auto_filled: boolean;
  auto_fill_warning: string;
  auto_fill_dismissable: boolean;
  confidence: number;
  time_ms: number;
}

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
  autoFilled?: boolean;
  autoFillWarning?: string;
  autoFillDismissable?: boolean;
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
  device: "webgpu" | "webgl2" | "wasm-simd" | "wasm" | "none";
  computeBackend: string;
  reason?: string;
}

/**
 * Progress phases the ScanButton UI should display:
 *
 *   detecting    - "Detecting GPU..." (~100ms)
 *   wasm         - "Loading WASM runtime..." (~200ms)
 *   config       - "Reading model config..." (instant)
 *   weights      - "Downloading model (45 MB / 257 MB)..." (progressive)
 *   gpu          - "Initializing GPU compute..." (1-3s)
 *   init         - "Preparing inference engine..." (~500ms)
 *   inferring    - "Running OCR inference..." (1-30s depending on GPU)
 *   decoding     - "Decoding text..." (instant)
 *   done         - "Done - extracted in 2.3s"
 */
export type InitProgressPhase =
  | "detecting"
  | "wasm"
  | "config"
  | "weights"
  | "gpu"
  | "init"
  | "inferring"
  | "decoding"
  | "done";

export interface InitProgressDetail {
  phase?: string;
  backend?: string;
  message?: string;
  bytes?: number;
  totalBytes?: number;
  shard?: number;
  totalShards?: number;
  fromCache?: boolean;
}

/**
 * Unified progress callback for the full OCR lifecycle (init + inference).
 * The ScanButton component subscribes to this to drive its progress bar
 * and status text through all phases.
 */
export interface OcrProgress {
  phase: InitProgressPhase;
  percent: number;
  detail?: string;
  timing?: { elapsed_ms: number };
}

export type OnInitProgress = (
  phase: InitProgressPhase,
  percent: number,
  detail?: InitProgressDetail,
) => void;

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

// --------------------------------------------------------------------------
// Auto-fill warning management
// --------------------------------------------------------------------------

const AUTOFILL_WARNING_DISMISSED_KEY = "molt_autofill_warning_dismissed";

/**
 * Check whether the auto-fill warning has been permanently dismissed.
 */
function isAutoFillWarningDismissed(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(AUTOFILL_WARNING_DISMISSED_KEY) === "true";
}

/**
 * Permanently dismiss the auto-fill warning for this browser.
 * Call this from a "Don't show again" button handler.
 */
export function dismissAutoFillWarning(): void {
  if (typeof localStorage !== "undefined") {
    localStorage.setItem(AUTOFILL_WARNING_DISMISSED_KEY, "true");
  }
}

/**
 * Returns true if an auto-fill warning banner should be shown for the given
 * result. Checks both the result flags and the localStorage dismissal state.
 */
export function shouldShowAutoFillWarning(result: OcrBackendResult): boolean {
  if (!result.autoFilled) return false;
  return !isAutoFillWarningDismissed();
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
  private _computeBackend: string = "none";
  private _onInitProgress: OnInitProgress | null = null;

  constructor(config: FalconOcrConfig, onInitProgress?: OnInitProgress) {
    this.config = config;
    this._onInitProgress = onInitProgress ?? null;
  }

  /**
   * The active compute backend name reported by the browser loader.
   * Available after initialize() completes.
   */
  get computeBackend(): string {
    return this._computeBackend;
  }

  /**
   * Initialize the OCR backend. Attempts browser-side WASM inference first
   * (offline, private), falls back to the session-based approach.
   *
   * Reports progress through the onInitProgress callback (if provided)
   * and through config.onProgress (forwarded to the browser loader).
   *
   * Returns false if initialization failed (caller should use PaddleOCR fallback).
   */
  async initialize(): Promise<boolean> {
    const initStart = performance.now();
    const progress = this._onInitProgress;

    try {
      // Check WebGPU availability
      progress?.("detecting", 0, { phase: "GPU detection" });
      const gpu = await detectWebGpu();
      this.webGpuAvailable = gpu.available;
      progress?.("detecting", 100, {
        backend: gpu.available ? "webgpu" : "none",
        message: gpu.available
          ? `WebGPU available (${gpu.adapter?.name ?? "unknown adapter"})`
          : `WebGPU unavailable: ${gpu.reason}`,
      });

      // On WebGPU/WASM-capable browsers, prefer local Falcon-OCR WASM inference.
      // This keeps all image data on-device (privacy-first).
      if (typeof WebAssembly !== "undefined") {
        try {
          const { FalconOCR } = await import("../browser/falcon-ocr-loader.js");

          // Merge progress callbacks: forward both to config.onProgress and our
          // onInitProgress so the caller gets unified progress reporting.
          const mergedProgress = (
            phase: string,
            pct: number,
            detail?: Record<string, unknown>,
          ) => {
            this.config.onProgress?.(phase, pct, detail);
            progress?.(phase as InitProgressPhase, pct, detail as InitProgressDetail);
          };

          const loader = new FalconOCR({
            baseUrl: this.config.workerUrl || "https://falcon-ocr.adpena.workers.dev",
            onProgress: mergedProgress,
          });
          await loader.init();
          this.browserOcr = loader as unknown as FalconOCRLoader;
          this.usingBrowserWasm = true;
          // Capture the compute backend from the loader
          this._computeBackend = (loader as any).computeBackend ?? "wasm";
          this.initTimingMs = performance.now() - initStart;

          progress?.("init", 100, {
            backend: this._computeBackend,
            message: `Ready (${this._computeBackend}, ${this.initTimingMs.toFixed(0)}ms)`,
          });
          return true;
        } catch (wasmErr) {
          // WASM init failed — fall through to session-based approach
          console.warn(
            `Falcon-OCR WASM init failed, falling back to session: ${(wasmErr as Error).message}`,
          );
          progress?.("init", 0, {
            message: `WASM init failed: ${(wasmErr as Error).message}, falling back`,
          });
        }
      }

      // Fallback: Create the WASM session (original path)
      progress?.("init", 50, { phase: "Creating session (fallback path)" });
      this.session = await createFalconOcrSession(this.config);
      this._computeBackend = "wasm";
      this.initTimingMs = performance.now() - initStart;
      progress?.("init", 100, {
        backend: "wasm",
        message: `Ready (session fallback, ${this.initTimingMs.toFixed(0)}ms)`,
      });
      return true;
    } catch (err) {
      this.initTimingMs = performance.now() - initStart;
      this.initError = (err as Error).message;
      progress?.("init", 0, {
        message: `Initialization failed: ${this.initError}`,
      });
      return false;
    }
  }

  /**
   * Run OCR on an ImageData (from canvas) or raw RGB buffer.
   *
   * Returns the recognized text, bounding boxes, timing breakdown, and
   * which compute backend was used for inference.
   */
  async recognize(
    image: ImageData | { width: number; height: number; rgb: Uint8Array },
  ): Promise<{ result: OcrBackendResult; timings: OcrTimings; backend: string }> {
    // Browser WASM path: all inference runs locally, no network
    if (this.usingBrowserWasm && this.browserOcr) {
      const totalStart = performance.now();
      const progress = this._onInitProgress;

      progress?.("inferring", 0, {
        message: "Running OCR inference...",
      });

      const wasmResult = await (this.browserOcr as any).recognize(image);

      progress?.("decoding", 90, {
        message: "Decoding text...",
      });

      const backend = wasmResult.backend ?? this._computeBackend;
      const totalMs = performance.now() - totalStart;
      const textPreview = wasmResult.text.length > 40
        ? wasmResult.text.slice(0, 40) + "..."
        : wasmResult.text;

      progress?.("done", 100, {
        message: "Done - '" + textPreview + "' extracted in " + (totalMs / 1000).toFixed(1) + "s",
      });

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
        backend,
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

    const ocrAny = ocrResult as OcrResult & {
      auto_filled?: boolean;
      auto_fill_warning?: string;
      auto_fill_dismissable?: boolean;
    };

    return {
      result: {
        text: ocrResult.text,
        boundingBoxes: ocrResult.boundingBoxes,
        confidence: ocrResult.confidence,
        autoFilled: ocrAny.auto_filled ?? false,
        autoFillWarning: ocrAny.auto_fill_warning ?? "",
        autoFillDismissable: ocrAny.auto_fill_dismissable ?? false,
      },
      timings: {
        initMs: this.initTimingMs,
        inferenceMs: ocrResult.timeMs,
        totalMs,
        ttfbMs,
      },
      backend: this._computeBackend,
    };
  }

  /**
   * Query backend availability and status.
   * Includes the specific compute backend (webgpu, webgl2, wasm-simd, wasm)
   * detected during initialization.
   */
  status(): OcrBackendStatus {
    if (this.initError) {
      return {
        available: false,
        name: "molt-falcon-ocr",
        device: "none",
        computeBackend: "none",
        reason: this.initError,
      };
    }

    if (this.usingBrowserWasm && this.browserOcr) {
      const device = this._computeBackend === "webgpu" ? "webgpu"
        : this._computeBackend === "webgl2" ? "webgl2"
        : this._computeBackend === "wasm-simd" ? "wasm-simd"
        : "wasm";
      return {
        available: true,
        name: "molt-falcon-ocr-browser",
        device,
        computeBackend: this._computeBackend,
      };
    }

    if (!this.session) {
      return {
        available: false,
        name: "molt-falcon-ocr",
        device: "none",
        computeBackend: "none",
        reason: "Not initialized",
      };
    }

    return {
      available: this.session.ready,
      name: "molt-falcon-ocr",
      device: this.webGpuAvailable ? "webgpu" : "wasm",
      computeBackend: this._computeBackend,
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
