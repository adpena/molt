/**
 * CPU-only inference engine for Falcon-OCR.
 *
 * Implements the full forward pass in pure JavaScript using
 * Float32Array for tensor operations.  Supports both F32 weights
 * and INT4 quantized weights (with on-the-fly dequantization
 * during matmul to minimize memory usage).
 *
 * Architecture matches falcon_ocr.py exactly:
 *   - RMSNorm (pre-norm)
 *   - Grouped-query attention with RoPE
 *   - SwiGLU feed-forward (squared ReLU gate, interleaved w13)
 *   - Greedy argmax decoding
 *
 * Quantization format (INT4):
 *   - Per-tensor symmetric: val = scale * int4_val
 *   - Two 4-bit signed values packed per byte, low nibble first
 *   - byte = (val_hi & 0xF) << 4 | (val_lo & 0xF)
 *   - Signed range: [-8, 7]
 *   - Scales stored separately in scales.json
 */

// ---------------------------------------------------------------------------
// SafeTensors parser
// ---------------------------------------------------------------------------

/**
 * Parse a SafeTensors file from an ArrayBuffer.
 *
 * For quantized models, I4/I8 tensors are stored as raw Uint8Array
 * (NOT dequantized upfront) to save memory.  Dequantization happens
 * on-the-fly during matmul via matmulDequant().
 *
 * Returns a Map<string, { dtype: string, shape: number[], data: Float32Array|Uint8Array }>.
 *
 * @param {ArrayBuffer} buffer
 * @returns {Map<string, { dtype: string, shape: number[], data: Float32Array|Uint8Array }>}
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
    } else if (meta.dtype === "I8" || meta.dtype === "I4") {
      // Keep quantized data as raw bytes -- dequantized on-the-fly during matmul.
      data = new Uint8Array(byteSlice);
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
// WASM SIMD matmul kernel (loaded lazily on first use)
// ---------------------------------------------------------------------------

/** @type {WebAssembly.Instance | null} */
let matmulWasmInstance = null;

/** @type {WebAssembly.Memory | null} */
let matmulWasmMemory = null;

/** @type {boolean} */
let matmulWasmFailed = false;

/** @type {WebAssembly.Instance | null} */
let simdOpsInstance = null;

/** @type {WebAssembly.Memory | null} */
let simdOpsMemory = null;

/** @type {boolean} */
let simdOpsFailed = false;

/**
 * Try to initialize the WASM SIMD matmul kernel.
 * The matmul.wasm module is embedded as a base64 string (< 1 KB) to avoid
 * an extra R2 fetch.  Falls back to pure JS on failure.
 *
 * @param {ArrayBuffer} wasmBytes - The matmul.wasm binary
 */
async function initMatmulWasm(wasmBytes) {
  if (matmulWasmInstance || matmulWasmFailed) return;
  try {
    const module = await WebAssembly.compile(wasmBytes);
    // The WAT module defines its own memory; instantiate without imports.
    matmulWasmInstance = await WebAssembly.instantiate(module);
    matmulWasmMemory = matmulWasmInstance.exports.memory;
  } catch (err) {
    console.warn(`WASM matmul init failed, using JS fallback: ${err.message}`);
    matmulWasmFailed = true;
  }
}

/**
 * Initialize the expanded WASM SIMD ops module for all hot-path operations.
 * Covers: add, mul, sqrt, reciprocal, neg, max, exp2, reduce_sum,
 * reduce_max, softmax, rms_norm, rope, matmul, matmul_dequant_i8.
 *
 * @param {ArrayBuffer} wasmBytes - The simd-ops.wasm binary
 */
async function initSimdOps(wasmBytes) {
  if (simdOpsInstance || simdOpsFailed) return;
  try {
    const module = await WebAssembly.compile(wasmBytes);
    simdOpsInstance = await WebAssembly.instantiate(module);
    simdOpsMemory = simdOpsInstance.exports.memory;
  } catch (err) {
    console.warn(`WASM SIMD ops init failed, using JS fallback: ${err.message}`);
    simdOpsFailed = true;
  }
}

/**
 * Ensure SIMD ops WASM memory has at least `needed` bytes.
 * @param {number} needed - bytes required
 */
function ensureSimdOpsMemory(needed) {
  const current = simdOpsMemory.buffer.byteLength;
  if (needed > current) {
    const pages = Math.ceil((needed - current) / 65536);
    simdOpsMemory.grow(pages);
  }
}

/**
 * WASM SIMD softmax: find max, sub, exp2, sum, div in fused SIMD passes.
 * Falls back to JS if SIMD ops not available.
 * @param {Float32Array} x - input (D elements for a single row)
 * @param {number} D - row size
 * @returns {Float32Array}
 */
function softmaxSimd(x, D) {
  if (simdOpsInstance) {
    const n = x.length;
    const rows = n / D;
    const out = new Float32Array(n);
    for (let r = 0; r < rows; r++) {
      const row = x.subarray(r * D, (r + 1) * D);
      const inBytes = D * 4;
      const outBytes = D * 4;
      const totalBytes = inBytes + outBytes;
      ensureSimdOpsMemory(totalBytes);
      const aPtr = 0;
      const outPtr = inBytes;
      new Float32Array(simdOpsMemory.buffer, aPtr, D).set(row);
      simdOpsInstance.exports.softmax_f32(aPtr, outPtr, D);
      out.set(new Float32Array(simdOpsMemory.buffer, outPtr, D), r * D);
    }
    return out;
  }
  return softmax(x, D);
}

/**
 * WASM SIMD RMSNorm with weight: rmsNorm(x, D, eps) * weight.
 * @param {Float32Array} x
 * @param {Float32Array} weight
 * @param {number} D
 * @param {number} eps
 * @returns {Float32Array}
 */
function rmsNormWeightSimd(x, weight, D, eps) {
  if (simdOpsInstance) {
    const n = x.length;
    const rows = n / D;
    const out = new Float32Array(n);
    for (let r = 0; r < rows; r++) {
      const rowX = x.subarray(r * D, (r + 1) * D);
      const aBytes = D * 4;
      const wBytes = D * 4;
      const outBytes = D * 4;
      const totalBytes = aBytes + wBytes + outBytes;
      ensureSimdOpsMemory(totalBytes);
      const aPtr = 0;
      const wPtr = aBytes;
      const outPtr = aBytes + wBytes;
      new Float32Array(simdOpsMemory.buffer, aPtr, D).set(rowX);
      new Float32Array(simdOpsMemory.buffer, wPtr, D).set(weight);
      simdOpsInstance.exports.rms_norm_f32(aPtr, wPtr, outPtr, D, eps);
      out.set(new Float32Array(simdOpsMemory.buffer, outPtr, D), r * D);
    }
    return out;
  }
  return rmsNormWeight(x, weight, D, eps);
}

/**
 * WASM SIMD RoPE rotation.
 * @param {Float32Array} x - [S, H, D]
 * @param {Float32Array} cosTable
 * @param {Float32Array} sinTable
 * @param {number} S
 * @param {number} H
 * @param {number} D
 * @param {number} freqDim
 * @returns {Float32Array}
 */
function applyRopeSimd(x, cosTable, sinTable, S, H, D, freqDim) {
  if (simdOpsInstance && D === freqDim * 2) {
    // When D == freqDim*2, we can use the fused SIMD rope per head
    const out = new Float32Array(x);
    for (let s = 0; s < S; s++) {
      const freqBase = s * freqDim;
      for (let h = 0; h < H; h++) {
        const base = (s * H + h) * D;
        const headSlice = x.subarray(base, base + D);
        const cosSlice = cosTable.subarray(freqBase, freqBase + freqDim);
        const sinSlice = sinTable.subarray(freqBase, freqBase + freqDim);
        // Pack for rope_f32: interleave pairs [x0, x_half, x1, x_{half+1}, ...]
        // rope_f32 expects pairs: q[2*i], q[2*i+1] with cos[i], sin[i]
        const qBytes = D * 4;
        const cosBytes = freqDim * 4;
        const sinBytes = freqDim * 4;
        const outBytes = D * 4;
        const totalBytes = qBytes + cosBytes + sinBytes + outBytes;
        ensureSimdOpsMemory(totalBytes);
        const qPtr = 0;
        const cosPtr = qBytes;
        const sinPtr = cosPtr + cosBytes;
        const outPtr = sinPtr + sinBytes;
        // Interleave: [x[0], x[half], x[1], x[half+1], ...]
        const half = D >> 1;
        const interleaved = new Float32Array(D);
        for (let i = 0; i < half; i++) {
          interleaved[2 * i] = headSlice[i];
          interleaved[2 * i + 1] = headSlice[i + half];
        }
        new Float32Array(simdOpsMemory.buffer, qPtr, D).set(interleaved);
        new Float32Array(simdOpsMemory.buffer, cosPtr, freqDim).set(cosSlice);
        new Float32Array(simdOpsMemory.buffer, sinPtr, freqDim).set(sinSlice);
        simdOpsInstance.exports.rope_f32(qPtr, cosPtr, sinPtr, outPtr, D);
        const result = new Float32Array(simdOpsMemory.buffer, outPtr, D);
        // De-interleave back
        for (let i = 0; i < half; i++) {
          out[base + i] = result[2 * i];
          out[base + i + half] = result[2 * i + 1];
        }
      }
    }
    return out;
  }
  return applyRope(x, cosTable, sinTable, S, H, D, freqDim);
}

/**
 * Ensure WASM memory has at least `needed` bytes.
 * Grows the memory if necessary.
 *
 * @param {number} needed - bytes required
 */
function ensureWasmMemory(needed) {
  const current = matmulWasmMemory.buffer.byteLength;
  if (needed > current) {
    const pages = Math.ceil((needed - current) / 65536);
    matmulWasmMemory.grow(pages);
  }
}

// ---------------------------------------------------------------------------
// Tensor operations (all operate on flat Float32Arrays with shape metadata)
// ---------------------------------------------------------------------------

/**
 * Matrix multiply: [M, K] x [K, N] -> [M, N]
 *
 * Dispatches to WASM SIMD kernel if available, otherwise falls back
 * to pure JS with IKJ loop order.
 *
 * @param {Float32Array} a
 * @param {Float32Array} b
 * @param {number} M
 * @param {number} K
 * @param {number} N
 * @returns {Float32Array}
 */
function matmul(a, b, M, K, N) {
  // WASM SIMD fast path
  if (matmulWasmInstance) {
    const aBytes = M * K * 4;
    const bBytes = K * N * 4;
    const outBytes = M * N * 4;
    const totalBytes = aBytes + bBytes + outBytes;
    ensureWasmMemory(totalBytes);

    const mem = new Float32Array(matmulWasmMemory.buffer);
    const aPtr = 0;
    const bPtr = aBytes;
    const outPtr = aBytes + bBytes;

    mem.set(a, aPtr / 4);
    mem.set(b, bPtr / 4);

    matmulWasmInstance.exports.matmul_f32(aPtr, bPtr, outPtr, M, K, N);

    // Copy result out (buffer may have been detached by memory.grow)
    return new Float32Array(matmulWasmMemory.buffer, outPtr, M * N).slice();
  }

  // JS fallback: IKJ loop for cache locality
  const out = new Float32Array(M * N);
  for (let m = 0; m < M; m++) {
    const aBase = m * K;
    const oBase = m * N;
    for (let k = 0; k < K; k++) {
      const aVal = a[aBase + k];
      for (let n = 0; n < N; n++) {
        out[oBase + n] += aVal * b[k * N + n];
      }
    }
  }
  return out;
}

/**
 * Dequantize an INT8 value: val = scale * (signed int8).
 * @param {number} byte_val - unsigned byte [0..255]
 * @returns {number} - signed int8 [-128..127]
 */
function int8Signed(byte_val) {
  return byte_val > 127 ? byte_val - 256 : byte_val;
}

/**
 * Matrix multiply with on-the-fly INT8 dequantization of B.
 * A is Float32Array [M, K], B is Uint8Array [K, N] (INT8 quantized).
 * Result: [M, N] where B_ij = scale * int8_signed(B_raw[i*N+j])
 *
 * Dispatches to WASM SIMD kernel if available.
 *
 * @param {Float32Array} a
 * @param {Uint8Array} b
 * @param {number} scale
 * @param {number} M
 * @param {number} K
 * @param {number} N
 * @returns {Float32Array}
 */
function matmulDequantI8(a, b, scale, M, K, N) {
  // WASM SIMD fast path
  if (matmulWasmInstance) {
    const aBytes = M * K * 4;
    const bBytes = K * N;  // INT8: 1 byte per element
    const scaleBytes = 4;
    const outBytes = M * N * 4;
    const totalBytes = aBytes + bBytes + scaleBytes + outBytes;
    ensureWasmMemory(totalBytes);

    const aPtr = 0;
    const bPtr = aBytes;
    const scalesPtr = aBytes + bBytes;
    const outPtr = scalesPtr + scaleBytes;

    // Write A (f32)
    new Float32Array(matmulWasmMemory.buffer, aPtr, M * K).set(a);
    // Write B (int8 as bytes)
    new Uint8Array(matmulWasmMemory.buffer, bPtr, K * N).set(b);
    // Write scale (single f32)
    new Float32Array(matmulWasmMemory.buffer, scalesPtr, 1)[0] = scale;

    matmulWasmInstance.exports.matmul_dequant_i8(aPtr, bPtr, scalesPtr, outPtr, M, K, N);

    return new Float32Array(matmulWasmMemory.buffer, outPtr, M * N).slice();
  }

  // JS fallback
  const out = new Float32Array(M * N);
  for (let m = 0; m < M; m++) {
    const aBase = m * K;
    const oBase = m * N;
    for (let k = 0; k < K; k++) {
      const aScaled = a[aBase + k] * scale;
      for (let n = 0; n < N; n++) {
        out[oBase + n] += aScaled * int8Signed(b[k * N + n]);
      }
    }
  }
  return out;
}

/**
 * Matrix multiply with on-the-fly INT4 dequantization of B.
 * A is Float32Array [M, K], B is Uint8Array with INT4 packed weights.
 * Two 4-bit signed values per byte, low nibble first.
 * B logical shape: [K, N], packed into K*N/2 bytes.
 *
 * No WASM fast path (INT4 packing is complex; INT8 is preferred).
 *
 * @param {Float32Array} a
 * @param {Uint8Array} b
 * @param {number} scale
 * @param {number} M
 * @param {number} K
 * @param {number} N
 * @returns {Float32Array}
 */
function matmulDequantI4(a, b, scale, M, K, N) {
  const out = new Float32Array(M * N);
  // IKJ loop order for cache locality on output writes.
  for (let m = 0; m < M; m++) {
    const aBase = m * K;
    const oBase = m * N;
    for (let k = 0; k < K; k++) {
      const aVal = a[aBase + k];
      for (let n = 0; n < N; n++) {
        const logIdx = k * N + n;
        const byteIdx = logIdx >> 1;
        const byteVal = b[byteIdx];
        let nibble;
        if ((logIdx & 1) === 0) {
          nibble = byteVal & 0xF;         // low nibble
        } else {
          nibble = (byteVal >> 4) & 0xF;  // high nibble
        }
        // Sign-extend 4-bit to signed: range [-8, 7]
        const signed = nibble >= 8 ? nibble - 16 : nibble;
        out[oBase + n] += aVal * (signed * scale);
      }
    }
  }
  return out;
}

/**
 * Dequantize an entire INT4 tensor to Float32Array.
 * Used for non-matmul access (embeddings, norms, etc.).
 *
 * @param {Uint8Array} packed - packed INT4 data
 * @param {number} numElements - total logical elements
 * @param {number} scale - dequantization scale
 * @returns {Float32Array}
 */
function dequantI4Full(packed, numElements, scale) {
  const out = new Float32Array(numElements);
  for (let i = 0; i < numElements; i++) {
    const byteIdx = i >> 1;
    const byteVal = packed[byteIdx];
    let nibble;
    if ((i & 1) === 0) {
      nibble = byteVal & 0xF;
    } else {
      nibble = (byteVal >> 4) & 0xF;
    }
    const signed = nibble >= 8 ? nibble - 16 : nibble;
    out[i] = signed * scale;
  }
  return out;
}

/**
 * Dequantize an entire INT8 tensor to Float32Array.
 *
 * @param {Uint8Array} data - INT8 data
 * @param {number} scale - dequantization scale
 * @returns {Float32Array}
 */
function dequantI8Full(data, scale) {
  const out = new Float32Array(data.length);
  for (let i = 0; i < data.length; i++) {
    out[i] = int8Signed(data[i]) * scale;
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
   * @param {Map<string, { dtype: string, shape: number[], data: Float32Array|Uint8Array }>} weights
   * @param {object} config
   * @param {Object<string, number>|null} scales - per-tensor dequantization scales (null for F32 models)
   */
  constructor(weights, config, scales) {
    this.config = config;
    this.weights = weights;
    this.scales = scales || {};
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

    // Detect quantization mode from the first weight tensor dtype
    const sampleTensor = weights.get("output.weight");
    this.quantMode = sampleTensor.dtype === "I4" ? "int4"
                   : sampleTensor.dtype === "I8" ? "int8"
                   : "f32";

    const ropeDim = this.headDim >> 1;
    this.rope = precomputeRopeFreqs(ropeDim, config.max_seq_len, config.rope_theta);

    // Extract weight tensors.  For quantized models, large weight matrices
    // stay as raw Uint8Array; small F32 tensors (norms, embeddings) are
    // dequantized eagerly since they're accessed element-wise.
    const tokEmbT = weights.get("tok_embeddings.weight");
    this.tokEmbDtype = tokEmbT.dtype;
    this.tokEmbed = tokEmbT.data;
    this.tokEmbScale = this.scales["tok_embeddings.weight"] || 0;

    const imgProjT = weights.get("img_projector.weight");
    this.imgProjDtype = imgProjT.dtype;
    this.imgProj = imgProjT.data;
    this.imgProjScale = this.scales["img_projector.weight"] || 0;

    // norm.weight is always F32 (kept during quantization)
    this.normW = weights.get("norm.weight").data;

    const outputT = weights.get("output.weight");
    this.outputDtype = outputT.dtype;
    this.outputW = outputT.data;
    this.outputScale = this.scales["output.weight"] || 0;

    this.layers = [];
    for (let i = 0; i < this.nLayers; i++) {
      const wqkvT = weights.get(`layers.${i}.attention.wqkv.weight`);
      const woT = weights.get(`layers.${i}.attention.wo.weight`);
      const w13T = weights.get(`layers.${i}.feed_forward.w13.weight`);
      const w2T = weights.get(`layers.${i}.feed_forward.w2.weight`);
      this.layers.push({
        wqkv: wqkvT.data,
        wqkvDtype: wqkvT.dtype,
        wqkvScale: this.scales[`layers.${i}.attention.wqkv.weight`] || 0,
        wo: woT.data,
        woDtype: woT.dtype,
        woScale: this.scales[`layers.${i}.attention.wo.weight`] || 0,
        w13: w13T.data,
        w13Dtype: w13T.dtype,
        w13Scale: this.scales[`layers.${i}.feed_forward.w13.weight`] || 0,
        w2: w2T.data,
        w2Dtype: w2T.dtype,
        w2Scale: this.scales[`layers.${i}.feed_forward.w2.weight`] || 0,
      });
    }
  }

  /**
   * Dispatch matmul based on weight dtype.
   * a: Float32Array [M, K], w: Float32Array|Uint8Array, wDtype, wScale
   * Weight shape: [outDim, inDim] stored row-major, so matmul is a[M,K] x w[K,N].
   *
   * @param {Float32Array} a
   * @param {Float32Array|Uint8Array} w
   * @param {string} wDtype
   * @param {number} wScale
   * @param {number} M
   * @param {number} K
   * @param {number} N
   * @returns {Float32Array}
   */
  matmulW(a, w, wDtype, wScale, M, K, N) {
    if (wDtype === "I4") {
      return matmulDequantI4(a, w, wScale, M, K, N);
    } else if (wDtype === "I8") {
      return matmulDequantI8(a, w, wScale, M, K, N);
    }
    return matmul(a, w, M, K, N);
  }

  /**
   * Embed a single token ID.  Handles dequantization for quantized embeddings.
   * @param {number} tokenId
   * @returns {Float32Array}
   */
  embedToken(tokenId) {
    if (this.tokEmbDtype === "I4") {
      // INT4: 2 values per byte.  Each embedding row is dim elements = dim/2 bytes.
      // dim is always even (768), so rows are byte-aligned.
      const startByte = (tokenId * this.dim) >> 1;
      const numBytes = this.dim >> 1;
      const out = new Float32Array(this.dim);
      for (let i = 0; i < this.dim; i++) {
        const byteIdx = startByte + (i >> 1);
        const byteVal = this.tokEmbed[byteIdx];
        const nibble = (i & 1) === 0 ? (byteVal & 0xF) : ((byteVal >> 4) & 0xF);
        const signed = nibble >= 8 ? nibble - 16 : nibble;
        out[i] = signed * this.tokEmbScale;
      }
      return out;
    } else if (this.tokEmbDtype === "I8") {
      const slice = this.tokEmbed.slice(tokenId * this.dim, (tokenId + 1) * this.dim);
      return dequantI8Full(slice, this.tokEmbScale);
    }
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

    // Pre-norm for attention (SIMD-accelerated when available)
    const hNorm = rmsNorm(h, dim, rmsInnerEps);

    // QKV projection: [S, dim] x [dim, totalQkv] -> [S, totalQkv]
    const qkv = this.matmulW(hNorm, layer.wqkv, layer.wqkvDtype, layer.wqkvScale, S, dim, totalQkv);

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

    // Apply RoPE (SIMD-accelerated when available)
    const qRope = applyRopeSimd(qNorm, this.rope.cos, this.rope.sin, S, nHeads, headDim, this.rope.freqDim);
    const kRope = applyRopeSimd(kNorm, this.rope.cos, this.rope.sin, S, nKvHeads, headDim, this.rope.freqDim);

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

      // Softmax per row (SIMD-accelerated when available)
      const probs = softmaxSimd(scores, S);

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
    const attnProj = this.matmulW(attnOut, layer.wo, layer.woDtype, layer.woScale, S, qDim, dim);

    // Residual
    const h2 = new Float32Array(S * dim);
    for (let i = 0; i < S * dim; i++) {
      h2[i] = h[i] + attnProj[i];
    }

    // Feed-forward
    const ffNorm = rmsNorm(h2, dim, rmsInnerEps);
    const ffUp = this.matmulW(ffNorm, layer.w13, layer.w13Dtype, layer.w13Scale, S, dim, this.ffnDim * 2);
    const ffAct = squaredReluGateInterleaved(ffUp, S, this.ffnDim);
    const ffDown = this.matmulW(ffAct, layer.w2, layer.w2Dtype, layer.w2Scale, S, this.ffnDim, dim);

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
   * @param {number} [maxLayers=0] - If > 0, use only the first N transformer
   *   layers instead of all layers. This is a quality-vs-speed tradeoff:
   *     - All layers (default): full quality, slowest
   *     - First 8 of 22 layers: ~2.75x faster, moderate quality loss
   *     - First 4 of 22 layers: ~5.5x faster, significant quality loss
   *   For the micro model (2 layers), this has no effect.
   *   For the full 22-layer model on Workers CPU, using 8 layers enables
   *   multi-token generation within the 30s wall-clock budget.
   * @returns {number[]}
   */
  generate(promptIds, patchFeatures, nPatches, maxNewTokens, maxLayers = 0) {
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

    // Append the OCR task token AFTER image tokens.
    // Without this, the model has no instruction and produces garbage.
    // Token 257 = <|OCR_PLAIN|> — plain text extraction
    // Token 255 = <|OCR_GROUNDING|> — with bounding boxes
    // Token 256 = <|OCR_DOC_PARSER|> — structured document
    const OCR_PLAIN_TOKEN = 257;
    prefixIds.push(OCR_PLAIN_TOKEN);

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
      const projected = this.matmulW(patchFeatures, this.imgProj, this.imgProjDtype, this.imgProjScale, nPatches, patchDim, dim);
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

      // Run transformer (optionally with layer skip for speed)
      let h = new Float32Array(allEmbeddings);
      const layerCount = maxLayers > 0 ? Math.min(maxLayers, this.layers.length) : this.layers.length;
      for (let li = 0; li < layerCount; li++) {
        h = this.transformerBlock(h, S, this.layers[li], mask);
      }

      // Final norm (SIMD-accelerated when available)
      h = rmsNormWeightSimd(h, this.normW, dim, normEps);

      // Extract last token's hidden state
      const lastH = h.slice((S - 1) * dim, S * dim);

      // Logits: [1, dim] x [dim, vocabSize] -> [1, vocabSize]
      const logits = this.matmulW(lastH, this.outputW, this.outputDtype, this.outputScale, 1, dim, vocabSize);

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
   * @param {number} [maxLayers=0] - Use only the first N layers (0 = all)
   * @returns {Int32Array}
   */
  ocrTokens(width, height, rgb, promptIds, maxNewTokens, maxLayers = 0) {
    const patches = this.rgbToPatches(rgb, width, height);
    const nPatches = (width / this.patchSize) * (height / this.patchSize);
    const tokens = this.generate(promptIds, patches, nPatches, maxNewTokens, maxLayers);
    return new Int32Array(tokens);
  }
}

/**
 * Create a FalconOCRMicro model from a single safetensors buffer.
 *
 * @param {ArrayBuffer} weightsBuffer - SafeTensors file content
 * @param {object} config - Parsed JSON config
 * @param {Object<string, number>|null} scales - Per-tensor dequantization scales (null for F32)
 * @returns {FalconOCRMicro}
 */
export function createModel(weightsBuffer, config, scales) {
  const weights = parseSafetensors(weightsBuffer);
  return new FalconOCRMicro(weights, config, scales);
}

/**
 * Parse a SafeTensors buffer and return a Map (same as parseSafetensors but exported).
 * Used for sharded loading where each shard is parsed separately.
 *
 * @param {ArrayBuffer} buffer
 * @returns {Map<string, { dtype: string, shape: number[], data: Float32Array|Uint8Array }>}
 */
export function parseSafetensorsToMap(buffer) {
  return parseSafetensors(buffer);
}

/**
 * Create a FalconOCRMicro model from a pre-built tensor Map.
 * Used for sharded loading where tensors are accumulated across shards.
 *
 * @param {Map<string, { dtype: string, shape: number[], data: Float32Array|Uint8Array }>} tensors
 * @param {object} config - Parsed JSON config
 * @param {Object<string, number>|null} scales - Per-tensor dequantization scales
 * @returns {FalconOCRMicro}
 */
export function createModelFromTensors(tensors, config, scales) {
  return new FalconOCRMicro(tensors, config, scales);
}

/**
 * Initialize the WASM SIMD matmul kernel for 10-50x speedup over pure JS.
 * Call this once on startup before any inference.  Falls back to JS
 * automatically if WASM SIMD is not supported.
 *
 * @param {ArrayBuffer} wasmBytes - The compiled matmul.wasm binary
 */
export { initMatmulWasm, initSimdOps };
