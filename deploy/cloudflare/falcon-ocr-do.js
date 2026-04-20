/**
 * Durable Object for Falcon-OCR inference with persistent model state.
 *
 * Advantages over plain Workers:
 *   - Model loaded ONCE, persists across requests (no cold-start penalty)
 *   - Same 256 MB memory limit, but no repeated allocation/deallocation
 *   - Single-threaded: no concurrent model initialization races
 *   - Hibernation: can sleep between requests without losing model state
 *
 * Model loading priority: INT8-sharded > INT4-sharded > micro.
 * INT8 uses all layers (no skip) and 224x224 input for maximum quality.
 * INT4 is faster but produces degraded output with 8/22 layers.
 *
 * The Worker routes /ocr requests here when X-Use-Backend: durable.
 */

import { createModel, createModelFromTensors, parseSafetensorsToMap } from "./inference-cpu.js";
import { MICRO_MODEL_B64, MICRO_MODEL_CONFIG } from "./micro-model-data.js";
import { handleOcrRequest } from "./ocr_api.js";
import { TokenizerDecoder } from "./tokenizer.js";

/**
 * CpuDevice adapter that wraps a FalconOCRMicro model instance
 * to match the interface expected by handleOcrRequest.
 */
class DOCpuDevice {
  constructor(model) {
    this.model = model;
    this.initialized = true;
  }

  ocrTokens(width, height, rgb, promptIds, maxNewTokens) {
    if (!this.model) return new Int32Array(0);
    return this.model.ocrTokens(width, height, rgb, promptIds, maxNewTokens);
  }
}

export class FalconOCRInference {
  /**
   * @param {DurableObjectState} state
   * @param {object} env
   */
  constructor(state, env) {
    this.state = state;
    this.env = env;
    this.backend = null;
    this.tokenizer = null;
    this.modelReady = false;
    this.modelVariant = "none";
    this.loadError = null;
    this.initPromise = null;
  }

  /**
   * Handle incoming requests.
   *
   * @param {Request} request
   * @returns {Promise<Response>}
   */
  async fetch(request) {
    const url = new URL(request.url);

    // Health check
    if (url.pathname === "/health") {
      return new Response(JSON.stringify({
        status: this.modelReady ? "ready" : "loading",
        model_variant: this.modelVariant,
        error: this.loadError,
      }), {
        headers: { "Content-Type": "application/json" },
      });
    }

    // Load model on first request
    if (!this.modelReady) {
      try {
        await this.loadModel();
      } catch (err) {
        return new Response(JSON.stringify({
          error: "Model loading failed in Durable Object",
          reason: err.message,
          fallback_available: true,
          fallback_url: "/api/ocr/paddle",
        }), {
          status: 503,
          headers: { "Content-Type": "application/json" },
        });
      }
    }

    // Delegate to the same handler the Worker uses
    const cors = {
      "Access-Control-Allow-Origin": this.env.CORS_ORIGIN || "https://freeinvoicemaker.app",
      "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
      "Access-Control-Allow-Headers": "Content-Type, X-Payment-402, X-Request-ID, X-Use-Backend, X-Document-Type",
    };
    const rid = request.headers.get("X-Request-ID") || `do-${Date.now().toString(36)}`;

    return handleOcrRequest(request, this.backend, this.env, cors, rid, "cpu", this.tokenizer);
  }

  /**
   * Load sharded model from R2 by variant prefix.
   *
   * @param {string} variant - R2 directory name (e.g. "falcon-ocr-int8-sharded")
   * @param {object} configOverrides - config fields to override (e.g. maxLayers, imageSize)
   * @returns {Promise<boolean>} true if loaded successfully
   */
  async loadShardedModel(variant, configOverrides = {}) {
    const prefix = `models/${variant}`;
    const indexObj = await this.env.WEIGHTS.get(`${prefix}/model.safetensors.index.json`);
    const configObj = await this.env.WEIGHTS.get(`${prefix}/config.json`);
    const scalesObj = await this.env.WEIGHTS.get(`${prefix}/scales.json`);

    if (!indexObj || !configObj || !scalesObj) return false;

    const indexJson = JSON.parse(await indexObj.text());
    const config = JSON.parse(await configObj.text());
    const scales = JSON.parse(await scalesObj.text());

    // Apply overrides: maxLayers=null means use all layers, imageSize for input resolution.
    if (configOverrides.maxLayers !== undefined) config.max_layers = configOverrides.maxLayers;
    if (configOverrides.imageSize !== undefined) config.image_size = configOverrides.imageSize;

    const shardNames = [];
    const seen = new Set();
    for (const shardName of Object.values(indexJson.weight_map)) {
      if (!seen.has(shardName)) {
        seen.add(shardName);
        shardNames.push(shardName);
      }
    }

    const allTensors = new Map();
    for (const shardName of shardNames) {
      const shardObj = await this.env.WEIGHTS.get(`${prefix}/${shardName}`);
      if (!shardObj) throw new Error(`Shard not found: ${prefix}/${shardName}`);
      const shardBuffer = await shardObj.arrayBuffer();
      const shardTensors = parseSafetensorsToMap(shardBuffer);
      for (const [name, tensor] of shardTensors) {
        allTensors.set(name, tensor);
      }
    }

    const model = createModelFromTensors(allTensors, config, scales);
    this.backend = new DOCpuDevice(model);
    this.modelVariant = variant.replace("falcon-ocr-", "");
    this.modelReady = true;
    this.loadError = null;
    console.log(`[DO] Loaded ${variant} model: ${allTensors.size} tensors, config overrides: ${JSON.stringify(configOverrides)}`);
    return true;
  }

  /**
   * Load model weights.  Priority: INT8-sharded > INT4-sharded > micro.
   *
   * INT8-sharded: all 22 layers, 224x224 input, best quality (~22 min/token).
   * INT4-sharded: 8/22 layers, 128x128 input, fast but degraded.
   * Micro: embedded 65K-param model, always fits, lowest quality.
   */
  async loadModel() {
    if (this.initPromise) {
      await this.initPromise;
      return;
    }

    this.initPromise = (async () => {
      // Priority 1: INT8-sharded (best quality, all layers, 224x224 input)
      // Slow (~22 * 60s per token) but the question is: does quality improve?
      try {
        const loaded = await this.loadShardedModel("falcon-ocr-int8-sharded", {
          maxLayers: null,   // Use ALL 22 layers (no skip)
          imageSize: 224,    // Larger input for more detail (vs 128)
        });
        if (loaded) {
          console.log("[DO] INT8 quality test: all 22 layers, 224x224 input, expect ~22 min/token");
        }
      } catch (err) {
        console.warn(`[DO] INT8-sharded load failed: ${err.message}`);
      }

      // Priority 2: INT4-sharded (faster, degraded quality)
      if (!this.modelReady) {
        try {
          await this.loadShardedModel("falcon-ocr-int4-sharded");
        } catch (err) {
          console.warn(`[DO] INT4-sharded load failed: ${err.message}`);
        }
      }

      // Priority 3: embedded micro model (263 KB, always fits)
      if (!this.modelReady) {
        const raw = atob(MICRO_MODEL_B64);
        const weightsBytes = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) {
          weightsBytes[i] = raw.charCodeAt(i);
        }
        const model = createModel(weightsBytes.buffer, MICRO_MODEL_CONFIG, null);
        this.backend = new DOCpuDevice(model);
        this.modelVariant = "micro";
        this.modelReady = true;
        this.loadError = null;
        console.log("[DO] Loaded embedded micro model");
      }

      // Load tokenizer from R2 for server-side token-to-text decoding.
      // Non-fatal: if unavailable, responses return empty text and the
      // browser-side tokenizer handles decoding.
      try {
        const tokObj = await this.env.WEIGHTS.get("models/falcon-ocr/tokenizer.json");
        if (tokObj) {
          this.tokenizer = TokenizerDecoder.fromJSON(await tokObj.text());
          console.log(`[DO] Tokenizer loaded: ${this.tokenizer.vocab.size} tokens`);
        }
      } catch (err) {
        console.warn(`[DO] Tokenizer load failed (non-fatal): ${err.message}`);
      }
    })();

    try {
      await this.initPromise;
    } catch (err) {
      this.initPromise = null;
      this.loadError = err.message;
      throw err;
    }
  }
}
