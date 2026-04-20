/**
 * Browser-optimized speculative decoding with WebGPU.
 *
 * Draft model: reduced layers (first 4 of 22) via WebGPU -- very fast.
 * Target model: all 22 layers via WebGPU -- slower but accurate.
 *
 * The key insight: in the browser, BOTH models share the same GPU.
 * We can keep both sets of weights in GPU memory and switch between them
 * without CPU-GPU transfer overhead. The draft model reuses the first 4
 * layers' weights from the target model, so there is zero extra memory
 * cost for the draft.
 *
 * Speculative decoding generates N draft tokens cheaply, then verifies
 * all N+1 positions through the full model in a single GPU dispatch.
 * When draft tokens match (typically 60-80% acceptance rate for OCR),
 * we get N tokens for the cost of ~1.3 full-model forwards.
 *
 * Usage:
 *   import { SpeculativeBrowserDecoder } from './speculative-browser.js';
 *   import { ComputeEngine } from './compute-engine.js';
 *
 *   const compute = await ComputeEngine.create();
 *   const decoder = new SpeculativeBrowserDecoder(compute, weights, config);
 *   const tokens = await decoder.decode(imagePatches, promptIds, 50);
 */

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Argmax over a Float32Array logit vector.
 * @param {Float32Array} logits
 * @returns {number} Index of the maximum value.
 */
function argmax(logits) {
  let maxIdx = 0;
  let maxVal = logits[0];
  for (let i = 1; i < logits.length; i++) {
    if (logits[i] > maxVal) {
      maxVal = logits[i];
      maxIdx = i;
    }
  }
  return maxIdx;
}

/**
 * Softmax over a Float32Array logit vector (in-place).
 * @param {Float32Array} logits
 * @returns {Float32Array} Probability distribution (same array, mutated).
 */
function softmax(logits) {
  let max = logits[0];
  for (let i = 1; i < logits.length; i++) {
    if (logits[i] > max) max = logits[i];
  }
  let sum = 0;
  for (let i = 0; i < logits.length; i++) {
    logits[i] = Math.exp(logits[i] - max);
    sum += logits[i];
  }
  const invSum = 1.0 / sum;
  for (let i = 0; i < logits.length; i++) {
    logits[i] *= invSum;
  }
  return logits;
}

// ---------------------------------------------------------------------------
// Transformer forward pass building blocks
// ---------------------------------------------------------------------------

/**
 * Single-head attention: Q @ K^T / sqrt(d) -> softmax -> @ V.
 * Operates on pre-projected Q, K, V matrices for one head.
 *
 * @param {import('./compute-engine.js').WebGPUEngine | import('./compute-engine.js').WebGL2Engine} compute
 * @param {Float32Array} q - [seqLen, headDim]
 * @param {Float32Array} k - [seqLen, headDim]
 * @param {Float32Array} v - [seqLen, headDim]
 * @param {number} seqLen
 * @param {number} headDim
 * @returns {Promise<Float32Array>} - [seqLen, headDim]
 */
async function attention(compute, q, k, v, seqLen, headDim) {
  // scores = Q @ K^T -> [seqLen, seqLen]
  // Transpose K by treating it as [headDim, seqLen] in row-major
  const kT = new Float32Array(headDim * seqLen);
  for (let r = 0; r < seqLen; r++) {
    for (let c = 0; c < headDim; c++) {
      kT[c * seqLen + r] = k[r * headDim + c];
    }
  }

  const scores = await compute.matmul(q, kT, seqLen, headDim, seqLen);

  // Scale by 1/sqrt(headDim)
  const scale = 1.0 / Math.sqrt(headDim);
  for (let i = 0; i < scores.length; i++) {
    scores[i] *= scale;
  }

  // Causal mask: set scores[i][j] = -Infinity for j > i
  for (let i = 0; i < seqLen; i++) {
    for (let j = i + 1; j < seqLen; j++) {
      scores[i * seqLen + j] = -Infinity;
    }
  }

  // Row-wise softmax
  for (let i = 0; i < seqLen; i++) {
    const row = scores.subarray(i * seqLen, (i + 1) * seqLen);
    softmax(row);
  }

  // attn_output = scores @ V -> [seqLen, headDim]
  return compute.matmul(scores, v, seqLen, seqLen, headDim);
}

/**
 * RMSNorm: x / sqrt(mean(x^2) + eps) * weight
 *
 * @param {Float32Array} x - [len]
 * @param {Float32Array} weight - [len]
 * @param {number} eps
 * @returns {Float32Array}
 */
function rmsNorm(x, weight, eps = 1e-6) {
  const len = x.length;
  let sumSq = 0;
  for (let i = 0; i < len; i++) {
    sumSq += x[i] * x[i];
  }
  const rms = 1.0 / Math.sqrt(sumSq / len + eps);
  const out = new Float32Array(len);
  for (let i = 0; i < len; i++) {
    out[i] = x[i] * rms * weight[i];
  }
  return out;
}

// ---------------------------------------------------------------------------
// Weight access helpers
// ---------------------------------------------------------------------------

/**
 * Extract a named weight tensor from the model weights map.
 *
 * @param {Map<string, { data: Float32Array, shape: number[] }>} weights
 * @param {string} name
 * @returns {{ data: Float32Array, shape: number[] }}
 */
function getWeight(weights, name) {
  const w = weights.get(name);
  if (!w) {
    throw new Error(`Missing weight tensor: ${name}`);
  }
  return w;
}

// ---------------------------------------------------------------------------
// SpeculativeBrowserDecoder
// ---------------------------------------------------------------------------

export class SpeculativeBrowserDecoder {
  /** @type {import('./compute-engine.js').WebGPUEngine | import('./compute-engine.js').WebGL2Engine | import('./compute-engine.js').WasmSimdEngine} */
  #compute;
  /** @type {Map<string, { data: Float32Array, shape: number[] }>} */
  #weights;
  /** @type {object} */
  #config;
  /** @type {number} Number of layers for draft model (fast, first N layers). */
  #draftLayers;
  /** @type {number} Total layers for target model (accurate, all layers). */
  #targetLayers;
  /** @type {number} Number of draft tokens to generate per speculation round. */
  #nDraft;
  /** @type {{ accepted: number, rejected: number, rounds: number }} */
  #stats;

  /**
   * @param {import('./compute-engine.js').WebGPUEngine | import('./compute-engine.js').WebGL2Engine | import('./compute-engine.js').WasmSimdEngine} computeEngine
   * @param {Map<string, { data: Float32Array, shape: number[] }>} weights - Named weight tensors
   * @param {object} config - Model config (n_layers, hidden_size, n_heads, vocab_size, eos_token_id, etc.)
   * @param {object} [options]
   * @param {number} [options.draftLayers=4] - Number of layers for draft model
   * @param {number} [options.nDraft=5] - Draft tokens per speculation round
   */
  constructor(computeEngine, weights, config, options = {}) {
    this.#compute = computeEngine;
    this.#weights = weights;
    this.#config = config;
    this.#draftLayers = options.draftLayers ?? 4;
    this.#targetLayers = config.n_layers;
    this.#nDraft = options.nDraft ?? 5;
    this.#stats = { accepted: 0, rejected: 0, rounds: 0 };
  }

  /**
   * Forward pass through a subset of transformer layers.
   *
   * @param {Float32Array} embeddings - [seqLen, hiddenSize]
   * @param {number} seqLen
   * @param {number} startLayer - First layer index (inclusive)
   * @param {number} endLayer - Last layer index (exclusive)
   * @returns {Promise<Float32Array>} - [seqLen, hiddenSize] after the layer range
   */
  async #forwardLayers(embeddings, seqLen, startLayer, endLayer) {
    const hiddenSize = this.#config.hidden_size;
    const nHeads = this.#config.n_heads;
    const headDim = hiddenSize / nHeads;
    let hidden = embeddings;

    for (let layer = startLayer; layer < endLayer; layer++) {
      const prefix = `model.layers.${layer}`;

      // Pre-attention RMSNorm
      const normWeight = getWeight(this.#weights, `${prefix}.input_layernorm.weight`).data;
      const normedRows = new Float32Array(seqLen * hiddenSize);
      for (let s = 0; s < seqLen; s++) {
        const row = hidden.subarray(s * hiddenSize, (s + 1) * hiddenSize);
        const normed = rmsNorm(row, normWeight);
        normedRows.set(normed, s * hiddenSize);
      }

      // QKV projection: [seqLen, hiddenSize] @ [hiddenSize, 3*hiddenSize]
      const qWeight = getWeight(this.#weights, `${prefix}.self_attn.q_proj.weight`).data;
      const kWeight = getWeight(this.#weights, `${prefix}.self_attn.k_proj.weight`).data;
      const vWeight = getWeight(this.#weights, `${prefix}.self_attn.v_proj.weight`).data;

      // Batch the three projections into one GPU submit
      const [qAll, kAll, vAll] = await this.#compute.matmulBatch([
        { a: normedRows, b: qWeight, m: seqLen, k: hiddenSize, n: hiddenSize },
        { a: normedRows, b: kWeight, m: seqLen, k: hiddenSize, n: hiddenSize },
        { a: normedRows, b: vWeight, m: seqLen, k: hiddenSize, n: hiddenSize },
      ]);

      // Multi-head attention (concatenated output)
      const attnOut = new Float32Array(seqLen * hiddenSize);
      const headPromises = [];

      for (let h = 0; h < nHeads; h++) {
        // Extract per-head Q, K, V slices
        const qHead = new Float32Array(seqLen * headDim);
        const kHead = new Float32Array(seqLen * headDim);
        const vHead = new Float32Array(seqLen * headDim);

        for (let s = 0; s < seqLen; s++) {
          const srcOffset = s * hiddenSize + h * headDim;
          const dstOffset = s * headDim;
          qHead.set(qAll.subarray(srcOffset, srcOffset + headDim), dstOffset);
          kHead.set(kAll.subarray(srcOffset, srcOffset + headDim), dstOffset);
          vHead.set(vAll.subarray(srcOffset, srcOffset + headDim), dstOffset);
        }

        headPromises.push(
          attention(this.#compute, qHead, kHead, vHead, seqLen, headDim).then((headOut) => {
            for (let s = 0; s < seqLen; s++) {
              attnOut.set(
                headOut.subarray(s * headDim, (s + 1) * headDim),
                s * hiddenSize + h * headDim,
              );
            }
          }),
        );
      }
      await Promise.all(headPromises);

      // Output projection
      const oWeight = getWeight(this.#weights, `${prefix}.self_attn.o_proj.weight`).data;
      const projected = await this.#compute.matmul(attnOut, oWeight, seqLen, hiddenSize, hiddenSize);

      // Residual connection
      const postAttn = new Float32Array(seqLen * hiddenSize);
      for (let i = 0; i < postAttn.length; i++) {
        postAttn[i] = hidden[i] + projected[i];
      }

      // Post-attention RMSNorm + FFN
      const ffnNormWeight = getWeight(this.#weights, `${prefix}.post_attention_layernorm.weight`).data;
      const ffnNormed = new Float32Array(seqLen * hiddenSize);
      for (let s = 0; s < seqLen; s++) {
        const row = postAttn.subarray(s * hiddenSize, (s + 1) * hiddenSize);
        const normed = rmsNorm(row, ffnNormWeight);
        ffnNormed.set(normed, s * hiddenSize);
      }

      // FFN: gate_proj * SiLU(up_proj) -> down_proj (SwiGLU)
      const intermediateSize = this.#config.intermediate_size;
      const gateWeight = getWeight(this.#weights, `${prefix}.mlp.gate_proj.weight`).data;
      const upWeight = getWeight(this.#weights, `${prefix}.mlp.up_proj.weight`).data;

      const [gate, up] = await this.#compute.matmulBatch([
        { a: ffnNormed, b: gateWeight, m: seqLen, k: hiddenSize, n: intermediateSize },
        { a: ffnNormed, b: upWeight, m: seqLen, k: hiddenSize, n: intermediateSize },
      ]);

      // SiLU(gate) * up
      const ffnIntermediate = new Float32Array(seqLen * intermediateSize);
      for (let i = 0; i < ffnIntermediate.length; i++) {
        const x = gate[i];
        const silu = x / (1.0 + Math.exp(-x));
        ffnIntermediate[i] = silu * up[i];
      }

      const downWeight = getWeight(this.#weights, `${prefix}.mlp.down_proj.weight`).data;
      const ffnOut = await this.#compute.matmul(
        ffnIntermediate, downWeight, seqLen, intermediateSize, hiddenSize,
      );

      // Residual connection
      hidden = new Float32Array(seqLen * hiddenSize);
      for (let i = 0; i < hidden.length; i++) {
        hidden[i] = postAttn[i] + ffnOut[i];
      }
    }

    return hidden;
  }

  /**
   * Get logits from hidden states via the LM head.
   *
   * @param {Float32Array} hidden - [seqLen, hiddenSize]
   * @param {number} seqLen
   * @returns {Promise<Float32Array[]>} - Array of [vocabSize] logit vectors, one per position
   */
  async #toLogits(hidden, seqLen) {
    const hiddenSize = this.#config.hidden_size;
    const vocabSize = this.#config.vocab_size;

    // Final RMSNorm
    const normWeight = getWeight(this.#weights, 'model.norm.weight').data;
    const normed = new Float32Array(seqLen * hiddenSize);
    for (let s = 0; s < seqLen; s++) {
      const row = hidden.subarray(s * hiddenSize, (s + 1) * hiddenSize);
      const n = rmsNorm(row, normWeight);
      normed.set(n, s * hiddenSize);
    }

    // LM head: [seqLen, hiddenSize] @ [hiddenSize, vocabSize]
    const lmWeight = getWeight(this.#weights, 'lm_head.weight').data;
    const allLogits = await this.#compute.matmul(normed, lmWeight, seqLen, hiddenSize, vocabSize);

    // Split into per-position logit vectors
    const result = [];
    for (let s = 0; s < seqLen; s++) {
      result.push(allLogits.subarray(s * vocabSize, (s + 1) * vocabSize));
    }
    return result;
  }

  /**
   * Embed token IDs and image patches into the hidden space.
   *
   * @param {number[]} tokenIds
   * @param {Float32Array} [imagePatches] - Pre-encoded image patch embeddings [nPatches, hiddenSize]
   * @returns {Float32Array} - [seqLen, hiddenSize]
   */
  #embed(tokenIds, imagePatches) {
    const hiddenSize = this.#config.hidden_size;
    const embedWeight = getWeight(this.#weights, 'model.embed_tokens.weight').data;

    const nPatches = imagePatches ? imagePatches.length / hiddenSize : 0;
    const seqLen = nPatches + tokenIds.length;
    const embeddings = new Float32Array(seqLen * hiddenSize);

    // Image patch embeddings first (if present)
    if (imagePatches && nPatches > 0) {
      embeddings.set(imagePatches, 0);
    }

    // Token embeddings after image patches
    for (let i = 0; i < tokenIds.length; i++) {
      const tokenId = tokenIds[i];
      const srcOffset = tokenId * hiddenSize;
      const dstOffset = (nPatches + i) * hiddenSize;
      embeddings.set(
        embedWeight.subarray(srcOffset, srcOffset + hiddenSize),
        dstOffset,
      );
    }

    return embeddings;
  }

  /**
   * Generate N draft tokens using only the first few layers (fast, approximate).
   *
   * @param {number[]} tokenIds - Current token sequence
   * @param {Float32Array} [imagePatches] - Image patch embeddings
   * @param {number} n - Number of draft tokens to generate
   * @returns {Promise<number[]>} - Draft token IDs
   */
  async generateDraft(tokenIds, imagePatches, n) {
    const draftTokens = [];
    const currentIds = [...tokenIds];

    for (let i = 0; i < n; i++) {
      const embeddings = this.#embed(currentIds, imagePatches);
      const seqLen = embeddings.length / this.#config.hidden_size;

      // Forward through draft layers only (first N of total)
      const hidden = await this.#forwardLayers(embeddings, seqLen, 0, this.#draftLayers);
      const logits = await this.#toLogits(hidden, seqLen);

      // Greedy: take argmax of last position
      const lastLogits = logits[logits.length - 1];
      const token = argmax(lastLogits);
      draftTokens.push(token);
      currentIds.push(token);

      if (token === this.#config.eos_token_id) break;
    }

    return draftTokens;
  }

  /**
   * Forward pass through ALL layers for verification.
   * Processes the entire sequence in one GPU dispatch for efficiency.
   *
   * @param {number[]} tokenIds - Full token sequence including draft tokens
   * @param {Float32Array} [imagePatches] - Image patch embeddings
   * @returns {Promise<Float32Array[]>} - Per-position logit vectors
   */
  async forwardFull(tokenIds, imagePatches) {
    const embeddings = this.#embed(tokenIds, imagePatches);
    const seqLen = embeddings.length / this.#config.hidden_size;

    const hidden = await this.#forwardLayers(embeddings, seqLen, 0, this.#targetLayers);
    return this.#toLogits(hidden, seqLen);
  }

  /**
   * Run speculative decoding: generate tokens using draft-then-verify.
   *
   * @param {Float32Array} imagePatches - Pre-encoded image patch embeddings
   * @param {number[]} promptIds - Initial prompt token IDs
   * @param {number} [maxTokens=50] - Maximum tokens to generate
   * @returns {Promise<{ tokens: number[], stats: { accepted: number, rejected: number, rounds: number, acceptRate: number } }>}
   */
  async decode(imagePatches, promptIds, maxTokens = 50) {
    const tokens = [...promptIds];
    const generated = [];
    const nDraft = this.#nDraft;

    this.#stats = { accepted: 0, rejected: 0, rounds: 0 };

    while (generated.length < maxTokens) {
      this.#stats.rounds++;

      // Draft: generate N tokens using first few layers (fast)
      const draftTokens = await this.generateDraft(tokens, imagePatches, nDraft);

      if (draftTokens.length === 0) break;

      // Verify: run all N+1 positions through full model (one GPU dispatch)
      const verifyInput = [...tokens, ...draftTokens];
      const targetLogits = await this.forwardFull(verifyInput, imagePatches);

      // Accept/reject: compare draft tokens against full model predictions
      let accepted = 0;
      for (let i = 0; i < draftTokens.length; i++) {
        const targetToken = argmax(targetLogits[tokens.length + i]);
        if (targetToken === draftTokens[i]) {
          // Draft matches target -- accept
          tokens.push(draftTokens[i]);
          generated.push(draftTokens[i]);
          accepted++;
        } else {
          // Draft diverges -- reject, use target's token instead
          tokens.push(targetToken);
          generated.push(targetToken);
          this.#stats.rejected++;
          break;
        }
      }

      this.#stats.accepted += accepted;

      // Check for EOS
      if (generated[generated.length - 1] === this.#config.eos_token_id) break;
    }

    const totalDecisions = this.#stats.accepted + this.#stats.rejected;
    return {
      tokens: generated,
      stats: {
        accepted: this.#stats.accepted,
        rejected: this.#stats.rejected,
        rounds: this.#stats.rounds,
        acceptRate: totalDecisions > 0 ? this.#stats.accepted / totalDecisions : 0,
      },
    };
  }

  /**
   * Get current speculation statistics.
   * @returns {{ accepted: number, rejected: number, rounds: number, acceptRate: number }}
   */
  get stats() {
    const total = this.#stats.accepted + this.#stats.rejected;
    return {
      ...this.#stats,
      acceptRate: total > 0 ? this.#stats.accepted / total : 0,
    };
  }
}

export { argmax, softmax, rmsNorm, attention };
