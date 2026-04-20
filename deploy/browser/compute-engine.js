/**
 * Compute backend abstraction for Falcon-OCR browser inference.
 *
 * Detects the best available compute backend in priority order:
 *   1. WebGPU  -- tiled matmul via compute shaders (10-100x over CPU)
 *   2. WebGL2  -- matmul encoded as texture passes (5-20x over CPU)
 *   3. WASM SIMD -- 128-bit SIMD intrinsics in WASM (2-4x over scalar)
 *
 * Each backend implements the same interface:
 *   - matmul(a, b, m, k, n)  -> Float32Array
 *   - uploadWeights(buffer)   -> void (GPU backends pre-upload weight matrices)
 *   - destroy()               -> void
 *
 * Usage:
 *   import { ComputeEngine } from './compute-engine.js';
 *   const engine = await ComputeEngine.create();
 *   console.log(engine.constructor.name);  // "WebGPUEngine" | "WebGL2Engine" | "WasmSimdEngine"
 */

import { createWebGPUMatmul, cpuMatmul } from './webgpu-matmul.js';
import { WebGPUEngine as WebGPUComputeEngine } from './webgpu-engine.js';

// ---------------------------------------------------------------------------
// WebGPU backend — delegates to the full WebGPU compute engine for all ops
// (matmul, softmax, RMSNorm, RoPE, elementwise add/mul).
// ---------------------------------------------------------------------------

class WebGPUEngine {
  /** @type {WebGPUComputeEngine} */
  #engine;

  constructor(engine) {
    this.#engine = engine;
  }

  static async probe() {
    if (typeof navigator === 'undefined' || !navigator.gpu) return null;
    try {
      const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
      if (!adapter) return null;
      return adapter;
    } catch {
      return null;
    }
  }

  static async create() {
    const engine = new WebGPUComputeEngine();
    const ok = await engine.init();
    if (!ok) return null;
    return new WebGPUEngine(engine);
  }

  /**
   * Pre-upload weight matrices to GPU memory as persistent GPUBuffers.
   * Returns an opaque handle map for use during inference.
   *
   * @param {Float32Array} weights - Weight data as f32 array
   * @returns {GPUBuffer} Persistent GPU buffer handle
   */
  uploadWeights(weights) {
    return this.#engine.uploadWeights(weights);
  }

  /**
   * @param {Float32Array|GPUBuffer} a
   * @param {Float32Array|GPUBuffer} b
   * @param {number} m
   * @param {number} k
   * @param {number} n
   * @returns {Promise<Float32Array>}
   */
  async matmul(a, b, m, k, n) {
    return this.#engine.matmul(a, b, m, k, n);
  }

  /**
   * GPU-resident matmul — returns GPUBuffer, no CPU readback.
   * Use for chaining ops without round-trips.
   *
   * @param {Float32Array|GPUBuffer} a
   * @param {Float32Array|GPUBuffer} b
   * @param {number} m
   * @param {number} k
   * @param {number} n
   * @returns {GPUBuffer}
   */
  matmulGPU(a, b, m, k, n) {
    return this.#engine.matmulGPU(a, b, m, k, n);
  }

  /**
   * Batched matmul — sequential dispatch (use matmulGPU + single submit
   * for true batching in the forward pass).
   *
   * @param {Array<{a: Float32Array, b: Float32Array, m: number, k: number, n: number}>} ops
   * @returns {Promise<Float32Array[]>}
   */
  async matmulBatch(ops) {
    const results = [];
    for (const { a, b, m, k, n } of ops) {
      results.push(await this.#engine.matmul(a, b, m, k, n));
    }
    return results;
  }

  /**
   * Fused softmax over rows.
   *
   * @param {Float32Array|GPUBuffer} input - [rows * n]
   * @param {number} n - Row length
   * @param {number} rows - Number of rows
   * @returns {Promise<Float32Array>}
   */
  async softmax(input, n, rows) {
    return this.#engine.softmax(input, n, rows);
  }

  /**
   * GPU-resident softmax — returns GPUBuffer.
   * @param {Float32Array|GPUBuffer} input
   * @param {number} n
   * @param {number} rows
   * @returns {GPUBuffer}
   */
  softmaxGPU(input, n, rows) {
    return this.#engine.softmaxGPU(input, n, rows);
  }

  /**
   * Fused RMSNorm.
   *
   * @param {Float32Array|GPUBuffer} input - [rows * n]
   * @param {Float32Array|GPUBuffer} weight - [n]
   * @param {number} n - Hidden dimension
   * @param {number} rows
   * @param {number} [eps=1e-6]
   * @returns {Promise<Float32Array>}
   */
  async rmsNorm(input, weight, n, rows, eps = 1e-6) {
    return this.#engine.rmsNorm(input, weight, n, rows, eps);
  }

  /**
   * GPU-resident RMSNorm — returns GPUBuffer.
   * @param {Float32Array|GPUBuffer} input
   * @param {Float32Array|GPUBuffer} weight
   * @param {number} n
   * @param {number} rows
   * @param {number} [eps=1e-6]
   * @returns {GPUBuffer}
   */
  rmsNormGPU(input, weight, n, rows, eps = 1e-6) {
    return this.#engine.rmsNormGPU(input, weight, n, rows, eps);
  }

  /**
   * RoPE — rotary position embedding on Q and K.
   *
   * @param {Float32Array|GPUBuffer} q - [seq_len * dim]
   * @param {Float32Array|GPUBuffer} k - [seq_len * dim]
   * @param {Float32Array|GPUBuffer} freqsCos - [seq_len * dim/2]
   * @param {Float32Array|GPUBuffer} freqsSin - [seq_len * dim/2]
   * @param {number} seqLen
   * @param {number} dim
   * @returns {Promise<{q: Float32Array, k: Float32Array}>}
   */
  async rope(q, k, freqsCos, freqsSin, seqLen, dim) {
    return this.#engine.rope(q, k, freqsCos, freqsSin, seqLen, dim);
  }

  /**
   * GPU-resident RoPE — modifies Q/K buffers in-place, no readback.
   * @param {GPUBuffer} q
   * @param {GPUBuffer} k
   * @param {Float32Array|GPUBuffer} freqsCos
   * @param {Float32Array|GPUBuffer} freqsSin
   * @param {number} seqLen
   * @param {number} dim
   */
  ropeGPU(q, k, freqsCos, freqsSin, seqLen, dim) {
    this.#engine.ropeGPU(q, k, freqsCos, freqsSin, seqLen, dim);
  }

  /**
   * Elementwise add.
   * @param {Float32Array|GPUBuffer} a
   * @param {Float32Array|GPUBuffer} b
   * @param {number} size
   * @returns {Promise<Float32Array>}
   */
  async add(a, b, size) {
    return this.#engine.add(a, b, size);
  }

  /**
   * GPU-resident elementwise add — returns GPUBuffer.
   * @param {Float32Array|GPUBuffer} a
   * @param {Float32Array|GPUBuffer} b
   * @param {number} size
   * @returns {GPUBuffer}
   */
  addGPU(a, b, size) {
    return this.#engine.addGPU(a, b, size);
  }

  /**
   * Elementwise mul.
   * @param {Float32Array|GPUBuffer} a
   * @param {Float32Array|GPUBuffer} b
   * @param {number} size
   * @returns {Promise<Float32Array>}
   */
  async mul(a, b, size) {
    return this.#engine.mul(a, b, size);
  }

  /**
   * GPU-resident elementwise mul — returns GPUBuffer.
   * @param {Float32Array|GPUBuffer} a
   * @param {Float32Array|GPUBuffer} b
   * @param {number} size
   * @returns {GPUBuffer}
   */
  mulGPU(a, b, size) {
    return this.#engine.mulGPU(a, b, size);
  }

  /**
   * Read a GPUBuffer back to CPU.
   * @param {GPUBuffer} gpuBuffer
   * @param {number} byteLength
   * @returns {Promise<Float32Array>}
   */
  async readBuffer(gpuBuffer, byteLength) {
    return this.#engine.readBuffer(gpuBuffer, byteLength);
  }

  get backendName() {
    return 'webgpu';
  }

  destroy() {
    this.#engine.destroy();
  }
}

// ---------------------------------------------------------------------------
// WebGL2 backend -- matmul via texture render passes
// ---------------------------------------------------------------------------

class WebGL2Engine {
  /** @type {WebGL2RenderingContext} */
  #gl;
  /** @type {WebGLProgram} */
  #program;
  /** @type {ArrayBuffer | null} */
  #weightsBuffer = null;

  constructor(gl, program) {
    this.#gl = gl;
    this.#program = program;
  }

  static probe() {
    if (typeof document === 'undefined') return null;
    try {
      const canvas = document.createElement('canvas');
      const gl = canvas.getContext('webgl2');
      return gl;
    } catch {
      return null;
    }
  }

  static create() {
    const gl = WebGL2Engine.probe();
    if (!gl) return null;

    // Compile matmul fragment shader: encode matrices as RGBA float textures,
    // compute dot products in the fragment shader.
    const vertSrc = `#version 300 es
      in vec2 a_pos;
      out vec2 v_uv;
      void main() {
        v_uv = a_pos * 0.5 + 0.5;
        gl_Position = vec4(a_pos, 0.0, 1.0);
      }
    `;

    const fragSrc = `#version 300 es
      precision highp float;
      uniform sampler2D u_a;
      uniform sampler2D u_b;
      uniform int u_k;
      uniform int u_n;
      in vec2 v_uv;
      out vec4 fragColor;
      void main() {
        int row = int(gl_FragCoord.y);
        int col = int(gl_FragCoord.x);
        float acc = 0.0;
        for (int i = 0; i < 4096; i++) {
          if (i >= u_k) break;
          float aVal = texelFetch(u_a, ivec2(i, row), 0).r;
          float bVal = texelFetch(u_b, ivec2(col, i), 0).r;
          acc += aVal * bVal;
        }
        fragColor = vec4(acc, 0.0, 0.0, 1.0);
      }
    `;

    const vert = gl.createShader(gl.VERTEX_SHADER);
    gl.shaderSource(vert, vertSrc);
    gl.compileShader(vert);
    if (!gl.getShaderParameter(vert, gl.COMPILE_STATUS)) {
      gl.deleteShader(vert);
      return null;
    }

    const frag = gl.createShader(gl.FRAGMENT_SHADER);
    gl.shaderSource(frag, fragSrc);
    gl.compileShader(frag);
    if (!gl.getShaderParameter(frag, gl.COMPILE_STATUS)) {
      gl.deleteShader(vert);
      gl.deleteShader(frag);
      return null;
    }

    const program = gl.createProgram();
    gl.attachShader(program, vert);
    gl.attachShader(program, frag);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      gl.deleteProgram(program);
      return null;
    }

    gl.deleteShader(vert);
    gl.deleteShader(frag);

    return new WebGL2Engine(gl, program);
  }

  async uploadWeights(weightsBuffer) {
    this.#weightsBuffer = weightsBuffer;
  }

  /**
   * @param {Float32Array} a
   * @param {Float32Array} b
   * @param {number} m
   * @param {number} k
   * @param {number} n
   * @returns {Promise<Float32Array>}
   */
  async matmul(a, b, m, k, n) {
    const gl = this.#gl;

    // Resize canvas to output dimensions for framebuffer readback
    gl.canvas.width = n;
    gl.canvas.height = m;
    gl.viewport(0, 0, n, m);

    // Upload A as [K x M] R32F texture (transposed for texelFetch row access)
    const texA = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, texA);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, k, m, 0, gl.RED, gl.FLOAT, a);

    // Upload B as [N x K] R32F texture
    const texB = gl.createTexture();
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, texB);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, n, k, 0, gl.RED, gl.FLOAT, b);

    gl.useProgram(this.#program);
    gl.uniform1i(gl.getUniformLocation(this.#program, 'u_a'), 0);
    gl.uniform1i(gl.getUniformLocation(this.#program, 'u_b'), 1);
    gl.uniform1i(gl.getUniformLocation(this.#program, 'u_k'), k);
    gl.uniform1i(gl.getUniformLocation(this.#program, 'u_n'), n);

    // Fullscreen quad
    const quadBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1,-1, 1,-1, -1,1, 1,1]), gl.STATIC_DRAW);
    const aPos = gl.getAttribLocation(this.#program, 'a_pos');
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

    // Render to framebuffer with R32F color attachment
    const fb = gl.createFramebuffer();
    const outTex = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, outTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, n, m, 0, gl.RED, gl.FLOAT, null);
    gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, outTex, 0);

    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

    // Read back
    const result = new Float32Array(m * n);
    gl.readPixels(0, 0, n, m, gl.RED, gl.FLOAT, result);

    // Cleanup
    gl.deleteTexture(texA);
    gl.deleteTexture(texB);
    gl.deleteTexture(outTex);
    gl.deleteFramebuffer(fb);
    gl.deleteBuffer(quadBuf);

    return result;
  }

  async matmulBatch(ops) {
    const results = [];
    for (const { a, b, m, k, n } of ops) {
      results.push(await this.matmul(a, b, m, k, n));
    }
    return results;
  }

  get backendName() {
    return 'webgl2';
  }

  destroy() {
    this.#weightsBuffer = null;
    const gl = this.#gl;
    gl.deleteProgram(this.#program);
    const ext = gl.getExtension('WEBGL_lose_context');
    if (ext) ext.loseContext();
  }
}

// ---------------------------------------------------------------------------
// WASM SIMD backend -- CPU fallback with SIMD detection
// ---------------------------------------------------------------------------

class WasmSimdEngine {
  #simdAvailable;

  constructor(simdAvailable) {
    this.#simdAvailable = simdAvailable;
  }

  static async probe() {
    if (typeof WebAssembly === 'undefined') return false;
    // Feature-detect WASM SIMD by attempting to compile a minimal SIMD module.
    // The magic bytes encode: (module (func (result v128) (v128.const i32x4 0 0 0 0)))
    try {
      const simdTest = new Uint8Array([
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
        0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7b,
        0x03, 0x02, 0x01, 0x00,
        0x0a, 0x17, 0x01, 0x15, 0x00,
        0xfd, 0x0c,
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
        0x0b,
      ]);
      await WebAssembly.compile(simdTest);
      return true;
    } catch {
      return false;
    }
  }

  static async create() {
    const simd = await WasmSimdEngine.probe();
    return new WasmSimdEngine(simd);
  }

  /**
   * No-op for CPU backend -- weights stay in JS heap.
   */
  async uploadWeights(_weightsBuffer) {
    // No GPU memory to upload to; weights are accessed directly from JS ArrayBuffers.
  }

  /**
   * CPU matmul (delegates to the reference implementation from webgpu-matmul.js).
   * WASM SIMD acceleration happens at the WASM module level, not here.
   *
   * @param {Float32Array} a
   * @param {Float32Array} b
   * @param {number} m
   * @param {number} k
   * @param {number} n
   * @returns {Promise<Float32Array>}
   */
  async matmul(a, b, m, k, n) {
    return cpuMatmul(a, b, m, k, n);
  }

  async matmulBatch(ops) {
    return ops.map(({ a, b, m, k, n }) => cpuMatmul(a, b, m, k, n));
  }

  get backendName() {
    return this.#simdAvailable ? 'wasm-simd' : 'wasm';
  }

  destroy() {
    // Nothing to release for CPU backend.
  }
}

// ---------------------------------------------------------------------------
// Factory: detect best backend and create engine
// ---------------------------------------------------------------------------

export class ComputeEngine {
  /**
   * Create the best available compute engine.
   * Probes backends in priority order: WebGPU > WebGL2 > WASM SIMD.
   *
   * @returns {Promise<WebGPUEngine | WebGL2Engine | WasmSimdEngine>}
   */
  static async create() {
    // 1. WebGPU (best: tiled compute shaders, 10-100x over CPU)
    const gpuAdapter = await WebGPUEngine.probe();
    if (gpuAdapter) {
      const engine = await WebGPUEngine.create();
      if (engine) return engine;
    }

    // 2. WebGL2 (good: texture-pass matmul, 5-20x over CPU)
    const gl2Engine = WebGL2Engine.create();
    if (gl2Engine) return gl2Engine;

    // 3. WASM SIMD (fallback: CPU with optional 128-bit SIMD)
    return WasmSimdEngine.create();
  }
}

export { WebGPUEngine, WebGL2Engine, WasmSimdEngine };
