/**
 * CPU-only inference engine for Falcon-OCR micro model.
 *
 * Implements the minimal forward pass in pure JavaScript using
 * Float32Array for tensor operations.  This is NOT the production
 * path (WASM will be faster) but proves the Worker can serve real
 * inference from weights loaded from R2.
 *
 * Architecture matches falcon_ocr.py exactly:
 *   - RMSNorm (pre-norm)
 *   - Grouped-query attention with RoPE
 *   - SwiGLU feed-forward (squared ReLU gate, interleaved w13)
 *   - Greedy argmax decoding
 */

// ---------------------------------------------------------------------------
// SafeTensors parser
// ---------------------------------------------------------------------------

/**
 * Parse a SafeTensors file from an ArrayBuffer.
 * Returns a Map<string, { dtype: string, shape: number[], data: Float32Array }>.
 *
 * @param {ArrayBuffer} buffer
 * @returns {Map<string, { dtype: string, shape: number[], data: Float32Array }>}
 */
function parseSafetensors(buffer) {
  const view = new DataView(buffer);
  const headerLen = Number(view.getBigUint64(0, true));
  const headerBytes = new Uint8Array(buffer, 8, headerLen);
  const headerJson = new TextDecoder().decode(headerBytes);
  const header = JSON.parse(headerJson);
  const dataStart = 8 + headerLen;
  const tensors = new Map();

  for (const [name, meta] of Object.entries(header)) {
    if (name === "__metadata__") continue;
    const [start, end] = meta.data_offsets;
    const byteSlice = buffer.slice(dataStart + start, dataStart + end);

    let data;
    if (meta.dtype === "F32") {
      data = new Float32Array(byteSlice);
    } else if (meta.dtype === "F16") {
      // Convert F16 to F32
      const u16 = new Uint16Array(byteSlice);
      data = new Float32Array(u16.length);
      for (let i = 0; i < u16.length; i++) {
        data[i] = f16ToF32(u16[i]);
      }
    } else if (meta.dtype === "BF16") {
      const u16 = new Uint16Array(byteSlice);
      data = new Float32Array(u16.length);
      for (let i = 0; i < u16.length; i++) {
        data[i] = bf16ToF32(u16[i]);
      }
    } else {
      throw new Error(`Unsupported dtype: ${meta.dtype}`);
    }

    tensors.set(name, { dtype: meta.dtype, shape: meta.shape, data });
  }

  return tensors;
}

/** @param {number} bits */
function f16ToF32(bits) {
  const sign = (bits >> 15) & 1;
  const exp = (bits >> 10) & 0x1f;
  const frac = bits & 0x3ff;
  if (exp === 0) {
    return (sign ? -1 : 1) * Math.pow(2, -14) * (frac / 1024);
  }
  if (exp === 0x1f) {
    return frac === 0 ? (sign ? -Infinity : Infinity) : NaN;
  }
  return (sign ? -1 : 1) * Math.pow(2, exp - 15) * (1 + frac / 1024);
}

/** @param {number} bits */
function bf16ToF32(bits) {
  const buf = new ArrayBuffer(4);
  const u32 = new Uint32Array(buf);
  u32[0] = bits << 16;
  return new Float32Array(buf)[0];
}

// ---------------------------------------------------------------------------
// Tensor operations (all operate on flat Float32Arrays with shape metadata)
// ---------------------------------------------------------------------------

/**
 * Matrix multiply: [M, K] x [K, N] -> [M, N]
 * @param {Float32Array} a
 * @param {Float32Array} b
 * @param {number} M
 * @param {number} K
 * @param {number} N
 * @returns {Float32Array}
 */
function matmul(a, b, M, K, N) {
  const out = new Float32Array(M * N);
  for (let m = 0; m < M; m++) {
    const aBase = m * K;
    const oBase = m * N;
    for (let n = 0; n < N; n++) {
      let sum = 0;
      for (let k = 0; k < K; k++) {
        sum += a[aBase + k] * b[k * N + n];
      }
      out[oBase + n] = sum;
    }
  }
  return out;
}

/**
 * RMSNorm: x / sqrt(mean(x^2) + eps)
 * Operates on the last dimension of shape [..., D].
 *
 * @param {Float32Array} x
 * @param {number} D - last dimension size
 * @param {number} eps
 * @returns {Float32Array}
 */
function rmsNorm(x, D, eps) {
  const n = x.length;
  const rows = n / D;
  const out = new Float32Array(n);
  for (let r = 0; r < rows; r++) {
    const base = r * D;
    let sumSq = 0;
    for (let i = 0; i < D; i++) {
      sumSq += x[base + i] * x[base + i];
    }
    const scale = 1.0 / Math.sqrt(sumSq / D + eps);
    for (let i = 0; i < D; i++) {
      out[base + i] = x[base + i] * scale;
    }
  }
  return out;
}

/**
 * RMSNorm with learned weight: rmsNorm(x) * weight
 * @param {Float32Array} x
 * @param {Float32Array} weight
 * @param {number} D
 * @param {number} eps
 * @returns {Float32Array}
 */
function rmsNormWeight(x, weight, D, eps) {
  const normed = rmsNorm(x, D, eps);
  const rows = normed.length / D;
  for (let r = 0; r < rows; r++) {
    const base = r * D;
    for (let i = 0; i < D; i++) {
      normed[base + i] *= weight[i];
    }
  }
  return normed;
}

/**
 * Softmax along the last dimension.
 * @param {Float32Array} x
 * @param {number} D - last dimension
 * @returns {Float32Array}
 */
function softmax(x, D) {
  const n = x.length;
  const rows = n / D;
  const out = new Float32Array(n);
  for (let r = 0; r < rows; r++) {
    const base = r * D;
    let maxVal = -Infinity;
    for (let i = 0; i < D; i++) {
      if (x[base + i] > maxVal) maxVal = x[base + i];
    }
    let sumExp = 0;
    for (let i = 0; i < D; i++) {
      out[base + i] = Math.exp(x[base + i] - maxVal);
      sumExp += out[base + i];
    }
    for (let i = 0; i < D; i++) {
      out[base + i] /= sumExp;
    }
  }
  return out;
}

/**
 * Apply RoPE to Q/K tensors.
 * x shape: [S, H, D] (batch=1 assumed)
 *
 * @param {Float32Array} x
 * @param {Float32Array} cosTable
 * @param {Float32Array} sinTable
 * @param {number} S
 * @param {number} H
 * @param {number} D
 * @param {number} freqDim - number of frequency pairs
 * @returns {Float32Array}
 */
function applyRope(x, cosTable, sinTable, S, H, D, freqDim) {
  const out = new Float32Array(x);
  const half = D >> 1;
  for (let s = 0; s < S; s++) {
    const freqBase = s * freqDim;
    for (let h = 0; h < H; h++) {
      const base = (s * H + h) * D;
      for (let i = 0; i < half && i < freqDim; i++) {
        const cosV = cosTable[freqBase + i];
        const sinV = sinTable[freqBase + i];
        const x0 = x[base + i];
        const x1 = i + half < D ? x[base + i + half] : 0;
        out[base + i] = x0 * cosV - x1 * sinV;
        if (i + half < D) {
          out[base + i + half] = x0 * sinV + x1 * cosV;
        }
      }
    }
  }
  return out;
}

/**
 * Repeat KV heads to match Q heads.
 * x shape: [S, n_kv_heads, D] -> [S, n_heads, D]
 *
 * @param {Float32Array} x
 * @param {number} S
 * @param {number} nKvHeads
 * @param {number} D
 * @param {number} nRep
 * @returns {Float32Array}
 */
function repeatKv(x, S, nKvHeads, D, nRep) {
  if (nRep === 1) return x;
  const nHeads = nKvHeads * nRep;
  const out = new Float32Array(S * nHeads * D);
  for (let s = 0; s < S; s++) {
    for (let kv = 0; kv < nKvHeads; kv++) {
      const srcBase = (s * nKvHeads + kv) * D;
      for (let r = 0; r < nRep; r++) {
        const dstBase = (s * nHeads + kv * nRep + r) * D;
        for (let d = 0; d < D; d++) {
          out[dstBase + d] = x[srcBase + d];
        }
      }
    }
  }
  return out;
}

/**
 * Squared ReLU gated feed-forward with interleaved w13.
 * Input: [S, ffn_dim*2] where even columns are gate, odd are up.
 * Output: [S, ffn_dim]
 *
 * @param {Float32Array} x
 * @param {number} S
 * @param {number} ffnDim
 * @returns {Float32Array}
 */
function squaredReluGateInterleaved(x, S, ffnDim) {
  const out = new Float32Array(S * ffnDim);
  for (let s = 0; s < S; s++) {
    const inBase = s * ffnDim * 2;
    const outBase = s * ffnDim;
    for (let i = 0; i < ffnDim; i++) {
      const gate = x[inBase + i * 2];
      const up = x[inBase + i * 2 + 1];
      const act = Math.max(0, gate);
      out[outBase + i] = act * act * up;
    }
  }
  return out;
}

/**
 * Argmax of a Float32Array.
 * @param {Float32Array} x
 * @returns {number}
 */
function argmax(x) {
  let maxIdx = 0;
  let maxVal = x[0];
  for (let i = 1; i < x.length; i++) {
    if (x[i] > maxVal) {
      maxVal = x[i];
      maxIdx = i;
    }
  }
  return maxIdx;
}

// ---------------------------------------------------------------------------
// Precompute RoPE tables
// ---------------------------------------------------------------------------

/**
 * @param {number} dim
 * @param {number} maxLen
 * @param {number} theta
 * @returns {{ cos: Float32Array, sin: Float32Array, freqDim: number }}
 */
function precomputeRopeFreqs(dim, maxLen, theta) {
  const freqs = new Float32Array(dim);
  const invDim = 1.0 / dim;
  for (let i = 0; i < dim; i++) {
    freqs[i] = 1.0 / Math.pow(theta, i * invDim);
  }
  const cos = new Float32Array(maxLen * dim);
  const sin = new Float32Array(maxLen * dim);
  for (let pos = 0; pos < maxLen; pos++) {
    for (let i = 0; i < dim; i++) {
      const angle = pos * freqs[i];
      cos[pos * dim + i] = Math.cos(angle);
      sin[pos * dim + i] = Math.sin(angle);
    }
  }
  return { cos, sin, freqDim: dim };
}

// ---------------------------------------------------------------------------
// Model class
// ---------------------------------------------------------------------------

export class FalconOCRMicro {
  /**
   * @param {Map<string, { dtype: string, shape: number[], data: Float32Array }>} weights
   * @param {object} config
   */
  constructor(weights, config) {
    this.config = config;
    this.weights = weights;
    this.dim = config.dim;
    this.nLayers = config.n_layers;
    this.nHeads = config.n_heads;
    this.headDim = config.head_dim;
    this.nKvHeads = config.n_kv_heads;
    this.nRep = this.nHeads / this.nKvHeads;
    this.ffnDim = config.ffn_dim;
    this.vocabSize = config.vocab_size;
    this.normEps = config.norm_eps;
    this.rmsInnerEps = config.rms_inner_eps;
    this.patchSize = config.spatial_patch_size;
    this.eosId = config.eos_id;

    const ropeDim = this.headDim >> 1;
    this.rope = precomputeRopeFreqs(ropeDim, config.max_seq_len, config.rope_theta);

    // Extract weight tensors
    this.tokEmbed = weights.get("tok_embeddings.weight").data;
    this.imgProj = weights.get("img_projector.weight").data;
    this.normW = weights.get("norm.weight").data;
    this.outputW = weights.get("output.weight").data;

    this.layers = [];
    for (let i = 0; i < this.nLayers; i++) {
      this.layers.push({
        wqkv: weights.get(`layers.${i}.attention.wqkv.weight`).data,
        wo: weights.get(`layers.${i}.attention.wo.weight`).data,
        w13: weights.get(`layers.${i}.feed_forward.w13.weight`).data,
        w2: weights.get(`layers.${i}.feed_forward.w2.weight`).data,
      });
    }
  }

  /**
   * Embed a single token ID.
   * @param {number} tokenId
   * @returns {Float32Array}
   */
  embedToken(tokenId) {
    return this.tokEmbed.slice(tokenId * this.dim, (tokenId + 1) * this.dim);
  }

  /**
   * Run one transformer layer.
   * h shape: [S, dim]
   *
   * @param {Float32Array} h
   * @param {number} S
   * @param {object} layer
   * @param {Float32Array} mask - [S, S] causal mask
   * @returns {Float32Array}
   */
  transformerBlock(h, S, layer, mask) {
    const { dim, nHeads, nKvHeads, headDim, nRep, rmsInnerEps } = this;

    // Attention
    const qDim = nHeads * headDim;
    const kvDim = nKvHeads * headDim;
    const totalQkv = qDim + 2 * kvDim;

    // Pre-norm for attention
    const hNorm = rmsNorm(h, dim, rmsInnerEps);

    // QKV projection: [S, dim] x [dim, totalQkv] -> [S, totalQkv]
    const qkv = matmul(hNorm, layer.wqkv, S, dim, totalQkv);

    // Split into Q, K, V
    const q = new Float32Array(S * qDim);
    const k = new Float32Array(S * kvDim);
    const v = new Float32Array(S * kvDim);
    for (let s = 0; s < S; s++) {
      const src = s * totalQkv;
      q.set(qkv.subarray(src, src + qDim), s * qDim);
      k.set(qkv.subarray(src + qDim, src + qDim + kvDim), s * kvDim);
      v.set(qkv.subarray(src + qDim + kvDim, src + totalQkv), s * kvDim);
    }

    // RMSNorm Q and K
    const qNorm = rmsNorm(q, headDim, rmsInnerEps);
    const kNorm = rmsNorm(k, headDim, rmsInnerEps);

    // Apply RoPE
    const qRope = applyRope(qNorm, this.rope.cos, this.rope.sin, S, nHeads, headDim, this.rope.freqDim);
    const kRope = applyRope(kNorm, this.rope.cos, this.rope.sin, S, nKvHeads, headDim, this.rope.freqDim);

    // Repeat KV
    const kRep = repeatKv(kRope, S, nKvHeads, headDim, nRep);
    const vRep = repeatKv(v, S, nKvHeads, headDim, nRep);

    // Scaled dot-product attention per head
    const scale = 1.0 / Math.sqrt(headDim);
    const attnOut = new Float32Array(S * qDim);

    for (let hd = 0; hd < nHeads; hd++) {
      // Extract Q[s, hd, :], K[s, hd, :], V[s, hd, :] for this head
      // Q shape: [S, nHeads, headDim] stored as [S * nHeads * headDim]
      // Compute attention scores: [S, S]
      const scores = new Float32Array(S * S);
      for (let qi = 0; qi < S; qi++) {
        for (let ki = 0; ki < S; ki++) {
          let dot = 0;
          for (let d = 0; d < headDim; d++) {
            dot += qRope[(qi * nHeads + hd) * headDim + d] *
                   kRep[(ki * nHeads + hd) * headDim + d];
          }
          scores[qi * S + ki] = dot * scale + mask[qi * S + ki];
        }
      }

      // Softmax per row
      const probs = softmax(scores, S);

      // Weighted sum of V
      for (let qi = 0; qi < S; qi++) {
        for (let d = 0; d < headDim; d++) {
          let sum = 0;
          for (let ki = 0; ki < S; ki++) {
            sum += probs[qi * S + ki] * vRep[(ki * nHeads + hd) * headDim + d];
          }
          attnOut[(qi * nHeads + hd) * headDim + d] = sum;
        }
      }
    }

    // Reshape attention output: [S, nHeads * headDim] -> [S, dim]
    // Output projection: [S, qDim] x [qDim, dim] -> [S, dim]
    const attnProj = matmul(attnOut, layer.wo, S, qDim, dim);

    // Residual
    const h2 = new Float32Array(S * dim);
    for (let i = 0; i < S * dim; i++) {
      h2[i] = h[i] + attnProj[i];
    }

    // Feed-forward
    const ffNorm = rmsNorm(h2, dim, rmsInnerEps);
    const ffUp = matmul(ffNorm, layer.w13, S, dim, this.ffnDim * 2);
    const ffAct = squaredReluGateInterleaved(ffUp, S, this.ffnDim);
    const ffDown = matmul(ffAct, layer.w2, S, this.ffnDim, dim);

    // Residual
    const h3 = new Float32Array(S * dim);
    for (let i = 0; i < S * dim; i++) {
      h3[i] = h2[i] + ffDown[i];
    }

    return h3;
  }

  /**
   * Build causal mask [S, S].
   * @param {number} S
   * @returns {Float32Array}
   */
  buildCausalMask(S) {
    const mask = new Float32Array(S * S);
    for (let q = 0; q < S; q++) {
      for (let k = 0; k < S; k++) {
        mask[q * S + k] = k <= q ? 0 : -1e9;
      }
    }
    return mask;
  }

  /**
   * Convert RGB bytes to patch embeddings.
   * @param {Uint8Array} rgb
   * @param {number} width
   * @param {number} height
   * @returns {Float32Array} - [nPatches, patchDim]
   */
  rgbToPatches(rgb, width, height) {
    const p = this.patchSize;
    const c = 3;
    const nW = width / p;
    const nH = height / p;
    const nPatches = nW * nH;
    const patchDim = p * p * c;
    const out = new Float32Array(nPatches * patchDim);

    for (let ph = 0; ph < nH; ph++) {
      for (let pw = 0; pw < nW; pw++) {
        const patchIdx = ph * nW + pw;
        let outIdx = 0;
        for (let py = 0; py < p; py++) {
          for (let px = 0; px < p; px++) {
            const imgY = ph * p + py;
            const imgX = pw * p + px;
            const rgbIdx = (imgY * width + imgX) * c;
            for (let ch = 0; ch < c; ch++) {
              out[patchIdx * patchDim + outIdx] = (rgb[rgbIdx + ch] / 255.0) * 2.0 - 1.0;
              outIdx++;
            }
          }
        }
      }
    }

    return out;
  }

  /**
   * Run greedy autoregressive generation.
   *
   * @param {number[]} promptIds
   * @param {Float32Array|null} patchFeatures - [nPatches, patchDim]
   * @param {number} nPatches
   * @param {number} maxNewTokens
   * @returns {number[]}
   */
  generate(promptIds, patchFeatures, nPatches, maxNewTokens) {
    const { dim, vocabSize, normEps, config } = this;
    const patchDim = this.patchSize * this.patchSize * 3;

    // Build full prefix: promptIds + image block IDs
    const prefixIds = [...promptIds];
    if (patchFeatures !== null && nPatches > 0) {
      prefixIds.push(config.image_cls_token_id);
      prefixIds.push(config.image_reg_1_token_id);
      prefixIds.push(config.image_reg_2_token_id);
      prefixIds.push(config.image_reg_3_token_id);
      prefixIds.push(config.image_reg_4_token_id);
      for (let i = 0; i < nPatches; i++) {
        prefixIds.push(config.img_id);
      }
      prefixIds.push(config.img_end_id);
    }

    const imageNoIncrease = new Set([
      config.img_id, config.image_reg_1_token_id,
      config.image_reg_2_token_id, config.image_reg_3_token_id,
      config.image_reg_4_token_id, config.img_end_id,
    ]);

    // Build embeddings for prefix
    const prefixLen = prefixIds.length;
    const embeddings = new Float32Array(prefixLen * dim);
    for (let i = 0; i < prefixLen; i++) {
      const emb = this.embedToken(prefixIds[i] % vocabSize);
      embeddings.set(emb, i * dim);
    }

    // Replace image patch positions with projected patch features
    if (patchFeatures !== null && nPatches > 0) {
      const projected = matmul(patchFeatures, this.imgProj, nPatches, patchDim, dim);
      let patchIdx = 0;
      for (let i = 0; i < prefixLen; i++) {
        if (prefixIds[i] === config.img_id && patchIdx < nPatches) {
          embeddings.set(projected.slice(patchIdx * dim, (patchIdx + 1) * dim), i * dim);
          patchIdx++;
        }
      }
    }

    const generated = [];
    let allIds = [...prefixIds];
    let allEmbeddings = new Float32Array(embeddings);

    for (let step = 0; step < maxNewTokens; step++) {
      const S = allIds.length;

      // Build causal mask
      const mask = this.buildCausalMask(S);

      // Run transformer
      let h = new Float32Array(allEmbeddings);
      for (const layer of this.layers) {
        h = this.transformerBlock(h, S, layer, mask);
      }

      // Final norm
      h = rmsNormWeight(h, this.normW, dim, normEps);

      // Extract last token's hidden state
      const lastH = h.slice((S - 1) * dim, S * dim);

      // Logits: [1, dim] x [dim, vocabSize] -> [1, vocabSize]
      const logits = matmul(lastH, this.outputW, 1, dim, vocabSize);

      // Greedy decode
      const nextId = argmax(logits);
      generated.push(nextId);

      if (nextId === this.eosId) break;

      // Append embedding for next step
      const nextEmb = this.embedToken(nextId % vocabSize);
      allIds.push(nextId);
      const newEmbeddings = new Float32Array(allIds.length * dim);
      newEmbeddings.set(allEmbeddings);
      newEmbeddings.set(nextEmb, allEmbeddings.length);
      allEmbeddings = newEmbeddings;
    }

    return generated;
  }

  /**
   * Run OCR on an image.
   *
   * @param {number} width
   * @param {number} height
   * @param {Uint8Array} rgb
   * @param {number[]} promptIds
   * @param {number} maxNewTokens
   * @returns {Int32Array}
   */
  ocrTokens(width, height, rgb, promptIds, maxNewTokens) {
    const patches = this.rgbToPatches(rgb, width, height);
    const nPatches = (width / this.patchSize) * (height / this.patchSize);
    const tokens = this.generate(promptIds, patches, nPatches, maxNewTokens);
    return new Int32Array(tokens);
  }
}

/**
 * Create a FalconOCRMicro model from R2 objects.
 *
 * @param {ArrayBuffer} weightsBuffer - SafeTensors file content
 * @param {object} config - Parsed JSON config
 * @returns {FalconOCRMicro}
 */
export function createModel(weightsBuffer, config) {
  const weights = parseSafetensors(weightsBuffer);
  return new FalconOCRMicro(weights, config);
}
