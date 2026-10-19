/**
 * Compute backend abstraction for Falcon-OCR browser inference.
 *
 * Detects the best available compute backend in priority order:
 *   1. WebNN   -- W3C Neural Network API, hardware-accelerated ML (GPU/NPU/CPU)
 *   2. WebGPU  -- tiled matmul via compute shaders (10-100x over CPU)
 *   3. WebGL2  -- fragment shader GPGPU via render-to-texture (3-30x over CPU)
 *   4. WASM SIMD -- 128-bit SIMD intrinsics in WASM (2-4x over scalar)
 *   5. Scalar JS -- baseline, always available
 *
 * Each backend implements the same interface:
 *   - matmul(a, b, m, k, n)         -> Promise<Float32Array>
 *   - softmax(input, n, rows?)      -> Promise<Float32Array>
 *   - rmsNorm(input, weight, n, rows?, eps?) -> Promise<Float32Array>
 *   - add(a, b, size)               -> Promise<Float32Array>
 *   - mul(a, b, size)               -> Promise<Float32Array>
 *   - uploadWeights(buffer)          -> void (GPU backends pre-upload weight matrices)
 *   - destroy()                      -> void
 *   - backendName: string
 *
 * Usage:
 *   import { ComputeEngine } from './compute-engine.js';
 *   const engine = await ComputeEngine.create();
 *   console.log(engine.backendName);  // 'webnn' | 'webgpu' | 'webgl2' | 'wasm-simd' | 'scalar'
 */

import { createWebGPUMatmul, cpuMatmul } from './webgpu-matmul.js';
import { WebGL2Engine as WebGL2GPGPUEngine } from './webgl2-engine.js';
import { WebNNEngine as WebNNComputeEngine } from './webnn-engine.js';

const AVAILABLE_BACKENDS = new Set(['webnn', 'webgpu', 'webgl2', 'wasm-simd', 'scalar']);

// ---------------------------------------------------------------------------
// Shared CPU implementations for ops not covered by all backends.
// ---------------------------------------------------------------------------

/**
 * CPU softmax with numerical stability (subtract max before exp).
 *
 * @param {Float32Array} input - Input data, [rows * n] elements.
 * @param {number} n - Row length.
 * @param {number} rows - Number of rows (default 1).
 * @returns {Float32Array}
 */
function _cpuSoftmax(input, n, rows = 1) {
    const out = new Float32Array(rows * n);
    if (out.length === 0) return out;
    for (let r = 0; r < rows; r++) {
        const offset = r * n;
        let maxVal = -Infinity;
        for (let i = 0; i < n; i++) {
            if (input[offset + i] > maxVal) maxVal = input[offset + i];
        }
        let sum = 0;
        for (let i = 0; i < n; i++) {
            const v = Math.exp(input[offset + i] - maxVal);
            out[offset + i] = v;
            sum += v;
        }
        const invSum = 1.0 / sum;
        for (let i = 0; i < n; i++) {
            out[offset + i] *= invSum;
        }
    }
    return out;
}

/**
 * CPU RMSNorm: out[i] = input[i] * weight[i % n] / sqrt(mean(input_row^2) + eps).
 *
 * @param {Float32Array} input - Input data, [rows * n] elements.
 * @param {Float32Array} weight - Weight vector, [n] elements.
 * @param {number} n - Hidden dimension.
 * @param {number} rows - Number of rows (default 1).
 * @param {number} eps - Epsilon (default 1e-6).
 * @returns {Float32Array}
 */
function _cpuRmsNorm(input, weight, n, rows = 1, eps = 1e-6) {
    const out = new Float32Array(rows * n);
    if (out.length === 0) return out;
    for (let r = 0; r < rows; r++) {
        const offset = r * n;
        let sumSq = 0;
        for (let i = 0; i < n; i++) {
            sumSq += input[offset + i] * input[offset + i];
        }
        const scale = 1.0 / Math.sqrt(sumSq / n + eps);
        for (let i = 0; i < n; i++) {
            out[offset + i] = input[offset + i] * weight[i] * scale;
        }
    }
    return out;
}

// ---------------------------------------------------------------------------
// WebGPU backend — delegates to the WebGPU compute engine for all ops.
// Uses tiled 16x16 matmul compute shader with workgroup shared memory.
// ---------------------------------------------------------------------------

class WebGPUEngine {
    /** @type {{ matmul: Function, matmulBatch: Function, destroy: Function, device: GPUDevice }} */
    #gpu;

    constructor(gpu) {
        this.#gpu = gpu;
    }

    static async probe() {
        if (typeof navigator === 'undefined' || !navigator.gpu) return null;
        try {
            const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
            return adapter || null;
        } catch {
            return null;
        }
    }

    static async create() {
        const gpu = await createWebGPUMatmul();
        if (!gpu) return null;
        return new WebGPUEngine(gpu);
    }

    /**
     * Pre-upload weight matrices to GPU memory as persistent GPUBuffers.
     * @param {Float32Array} weights
     * @returns {void}
     */
    uploadWeights(weights) {
        // WebGPU matmul creates per-dispatch buffers. Future optimization:
        // persistent weight buffers to eliminate per-inference upload overhead.
        this._weightsCache = weights;
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
        return this.#gpu.matmul(a, b, m, k, n);
    }

    /**
     * Batched matmul — encode multiple dispatches in a single command buffer.
     * @param {Array<{a: Float32Array, b: Float32Array, m: number, k: number, n: number}>} ops
     * @returns {Promise<Float32Array[]>}
     */
    async matmulBatch(ops) {
        return this.#gpu.matmulBatch(ops);
    }

    /**
     * Softmax over rows. CPU fallback — small vectors in inference.
     * @param {Float32Array} input - [rows * n]
     * @param {number} n - Row length
     * @param {number} [rows=1]
     * @returns {Promise<Float32Array>}
     */
    async softmax(input, n, rows = 1) {
        return _cpuSoftmax(input, n, rows);
    }

    /**
     * RMSNorm. CPU fallback — small vectors in inference.
     * @param {Float32Array} input - [rows * n]
     * @param {Float32Array} weight - [n]
     * @param {number} n
     * @param {number} [rows=1]
     * @param {number} [eps=1e-6]
     * @returns {Promise<Float32Array>}
     */
    async rmsNorm(input, weight, n, rows = 1, eps = 1e-6) {
        return _cpuRmsNorm(input, weight, n, rows, eps);
    }

    /**
     * Elementwise add.
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async add(a, b, size) {
        const out = new Float32Array(size);
        for (let i = 0; i < size; i++) out[i] = a[i] + b[i];
        return out;
    }

    /**
     * Elementwise mul.
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async mul(a, b, size) {
        const out = new Float32Array(size);
        for (let i = 0; i < size; i++) out[i] = a[i] * b[i];
        return out;
    }

    get backendName() {
        return 'webgpu';
    }

    destroy() {
        this.#gpu.destroy();
    }
}

// ---------------------------------------------------------------------------
// WebGL2 backend — GPGPU via fragment shaders (render-to-texture).
//
// Uses the WebGL2Engine from webgl2-engine.js which implements matmul,
// softmax, rmsNorm, add, and mul via GLSL ES 3.0 fragment shaders.
// This follows the same render-to-texture pattern as our GlslRenderer
// (runtime/molt-gpu/src/render/glsl.rs).
// ---------------------------------------------------------------------------

class WebGL2Engine {
    /** @type {WebGL2GPGPUEngine} */
    #engine;

    constructor(engine) {
        this.#engine = engine;
    }

    static async probe() {
        try {
            const canvas = new OffscreenCanvas(1, 1);
            const gl = canvas.getContext('webgl2');
            if (!gl) return false;
            // Check float texture support required for GPGPU
            return !!gl.getExtension('EXT_color_buffer_float');
        } catch {
            return false;
        }
    }

    static async create() {
        const engine = new WebGL2GPGPUEngine();
        const ok = await engine.init();
        if (!ok) return null;
        return new WebGL2Engine(engine);
    }

    /**
     * No-op for WebGL2 — weights stay in JS heap, uploaded per-dispatch
     * as textures.
     */
    async uploadWeights(_weightsBuffer) {}

    /**
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} m
     * @param {number} k
     * @param {number} n
     * @returns {Promise<Float32Array>}
     */
    async matmul(a, b, m, k, n) {
        return this.#engine.matmul(a, b, m, k, n);
    }

    async matmulBatch(ops) {
        const results = [];
        for (const { a, b, m, k, n } of ops) {
            results.push(await this.#engine.matmul(a, b, m, k, n));
        }
        return results;
    }

    /**
     * Softmax via WebGL2 fragment shader.
     * CPU pass finds max+sum, GPU pass normalizes.
     *
     * @param {Float32Array} input
     * @param {number} n
     * @param {number} [rows=1]
     * @returns {Promise<Float32Array>}
     */
    async softmax(input, n, rows = 1) {
        if (n === 0 || rows === 0) return new Float32Array();
        if (rows === 1) {
            return this.#engine.softmax(input, n);
        }
        // Multi-row: process each row independently
        const out = new Float32Array(rows * n);
        for (let r = 0; r < rows; r++) {
            const row = input.subarray(r * n, (r + 1) * n);
            const result = await this.#engine.softmax(row, n);
            out.set(result, r * n);
        }
        return out;
    }

    /**
     * RMSNorm via WebGL2 fragment shader.
     * CPU pass computes scale, GPU pass applies scale * weight.
     *
     * @param {Float32Array} input
     * @param {Float32Array} weight
     * @param {number} n
     * @param {number} [rows=1]
     * @param {number} [eps=1e-6]
     * @returns {Promise<Float32Array>}
     */
    async rmsNorm(input, weight, n, rows = 1, eps = 1e-6) {
        if (n === 0 || rows === 0) return new Float32Array();
        if (rows === 1) {
            return this.#engine.rmsNorm(input, weight, n, eps);
        }
        const out = new Float32Array(rows * n);
        for (let r = 0; r < rows; r++) {
            const row = input.subarray(r * n, (r + 1) * n);
            const result = await this.#engine.rmsNorm(row, weight, n, eps);
            out.set(result, r * n);
        }
        return out;
    }

    /**
     * Elementwise add via WebGL2 fragment shader.
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async add(a, b, size) {
        return this.#engine.add(a, b, size);
    }

    /**
     * Elementwise mul via WebGL2 fragment shader.
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async mul(a, b, size) {
        return this.#engine.mul(a, b, size);
    }

    get backendName() {
        return 'webgl2';
    }

    destroy() {
        this.#engine.destroy();
    }
}

// ---------------------------------------------------------------------------
// WASM SIMD backend. One Rust crate owns the binary and its memory ABI.
// See docs/design/browser_simd.md for numerical and allocation contracts.
// ---------------------------------------------------------------------------

class WasmSimdEngine {
    #instance;
    #memory;
    #heapBase;

    constructor(instance, memory, heapBase) {
        this.#instance = instance;
        this.#memory = memory;
        this.#heapBase = heapBase;
    }

    static requiredExports() {
        return Object.freeze([
            'matmul_f32_tiled', 'softmax_f32_fused', 'rms_norm_f32',
            'add_f32', 'mul_f32',
        ]);
    }

    static async probe() {
        if (typeof WebAssembly === 'undefined') return false;
        // (module (func (result v128) (v128.const i32x4 0 0 0 0)))
        return WebAssembly.validate(new Uint8Array([
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7b,
            0x03, 0x02, 0x01, 0x00,
            0x0a, 0x16, 0x01, 0x14, 0x00, 0xfd, 0x0c,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0x0b,
        ]));
    }

    static async create(wasmUrl) {
        if (!await WasmSimdEngine.probe() || typeof fetch === 'undefined') return null;
        const url = wasmUrl || new URL('./simd-ops.wasm', import.meta.url).href;
        const response = await fetch(url);
        if (!response.ok) throw new Error(`WASM SIMD fetch failed: ${response.status}`);
        const { instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {});
        const missing = WasmSimdEngine.requiredExports().filter(
            name => typeof instance.exports[name] !== 'function',
        );
        if (!(instance.exports.memory instanceof WebAssembly.Memory)) missing.push('memory');
        if (!(instance.exports.__heap_base instanceof WebAssembly.Global)) missing.push('__heap_base');
        if (missing.length) throw new Error(`WASM SIMD module missing exports: ${missing.join(', ')}`);
        const heapBase = instance.exports.__heap_base.value;
        if (!Number.isSafeInteger(heapBase) || heapBase <= 0 || heapBase % 16 !== 0
            || heapBase > instance.exports.memory.buffer.byteLength) {
            throw new Error('WASM SIMD module has invalid __heap_base');
        }
        return new WasmSimdEngine(instance, instance.exports.memory, heapBase);
    }

    // Validate the unsafe kernel's dimensions before converting them to wasm32.
    #count(...dimensions) {
        let count = 1;
        for (const size of dimensions) {
            if (!Number.isSafeInteger(size) || size < 0 || size > 0xffffffff) {
                throw new RangeError('WASM SIMD dimensions must be nonnegative wasm32 integers');
            }
            count *= size;
        }
        if (!Number.isSafeInteger(count) || count > 0x3fffffff) {
            throw new RangeError('WASM SIMD array exceeds wasm32 memory');
        }
        return count;
    }

    #input(array, size) {
        if (!(array instanceof Float32Array) || array.length !== size) {
            throw new RangeError(`WASM SIMD input requires ${size} float32 elements`);
        }
    }

    // Reserve the complete call once; no partial growth or mutable arena cursor.
    #buffers(...counts) {
        let end = this.#heapBase;
        for (let i = 0; i < counts.length; i++) {
            const bytes = Math.ceil(counts[i] / 4) * 16;
            counts[i] = end;
            end += bytes;
        }
        if (!Number.isSafeInteger(end) || end > 0x100000000) {
            throw new RangeError('WASM SIMD allocation exceeds wasm32 memory');
        }
        const currentBytes = this.#memory.buffer.byteLength;
        if (end > currentBytes) this.#memory.grow(Math.ceil((end - currentBytes) / 65536));
        return counts;
    }

    async uploadWeights(_weightsBuffer) {}

    async matmul(a, b, m, k, n) {
        const aSize = this.#count(m, k), bSize = this.#count(k, n), outSize = this.#count(m, n);
        this.#input(a, aSize);
        this.#input(b, bSize);
        if (outSize === 0) return new Float32Array();
        const [aPtr, bPtr, outPtr] = this.#buffers(aSize, bSize, outSize);
        new Float32Array(this.#memory.buffer, aPtr, aSize).set(a);
        new Float32Array(this.#memory.buffer, bPtr, bSize).set(b);
        this.#instance.exports.matmul_f32_tiled(aPtr, bPtr, outPtr, m, k, n);
        return new Float32Array(this.#memory.buffer.slice(outPtr, outPtr + outSize * 4));
    }

    async matmulBatch(ops) {
        const results = [];
        for (const { a, b, m, k, n } of ops) results.push(await this.matmul(a, b, m, k, n));
        return results;
    }

    async softmax(input, n, rows = 1) {
        const size = this.#count(n, rows);
        this.#input(input, size);
        if (size === 0) return new Float32Array();
        const [inPtr, outPtr] = this.#buffers(size, size);
        new Float32Array(this.#memory.buffer, inPtr, size).set(input);
        for (let r = 0; r < rows; r++) {
            this.#instance.exports.softmax_f32_fused(inPtr + r * n * 4, outPtr + r * n * 4, n);
        }
        return new Float32Array(this.#memory.buffer.slice(outPtr, outPtr + size * 4));
    }

    async rmsNorm(input, weight, n, rows = 1, eps = 1e-6) {
        const size = this.#count(n, rows);
        this.#input(input, size);
        this.#input(weight, n);
        if (size === 0) return new Float32Array();
        const [inPtr, wPtr, outPtr] = this.#buffers(size, n, size);
        new Float32Array(this.#memory.buffer, inPtr, size).set(input);
        new Float32Array(this.#memory.buffer, wPtr, n).set(weight);
        for (let r = 0; r < rows; r++) {
            this.#instance.exports.rms_norm_f32(inPtr + r * n * 4, wPtr, outPtr + r * n * 4, n, eps);
        }
        return new Float32Array(this.#memory.buffer.slice(outPtr, outPtr + size * 4));
    }

    #binary(name, a, b, size) {
        this.#count(size);
        this.#input(a, size);
        this.#input(b, size);
        const [aPtr, bPtr, outPtr] = this.#buffers(size, size, size);
        new Float32Array(this.#memory.buffer, aPtr, size).set(a);
        new Float32Array(this.#memory.buffer, bPtr, size).set(b);
        this.#instance.exports[name](aPtr, bPtr, outPtr, size);
        return new Float32Array(this.#memory.buffer.slice(outPtr, outPtr + size * 4));
    }

    async add(a, b, size) { return this.#binary('add_f32', a, b, size); }
    async mul(a, b, size) { return this.#binary('mul_f32', a, b, size); }
    get backendName() { return 'wasm-simd'; }
    destroy() { this.#instance = null; }
}

// ---------------------------------------------------------------------------
// Scalar JS fallback — always available, no dependencies.
// ---------------------------------------------------------------------------

class ScalarEngine {
    async uploadWeights(_weightsBuffer) {}

    async matmul(a, b, m, k, n) {
        return cpuMatmul(a, b, m, k, n);
    }

    async matmulBatch(ops) {
        return ops.map(({ a, b, m, k, n }) => cpuMatmul(a, b, m, k, n));
    }

    async softmax(input, n, rows = 1) {
        return _cpuSoftmax(input, n, rows);
    }

    async rmsNorm(input, weight, n, rows = 1, eps = 1e-6) {
        return _cpuRmsNorm(input, weight, n, rows, eps);
    }

    async add(a, b, size) {
        const out = new Float32Array(size);
        for (let i = 0; i < size; i++) out[i] = a[i] + b[i];
        return out;
    }

    async mul(a, b, size) {
        const out = new Float32Array(size);
        for (let i = 0; i < size; i++) out[i] = a[i] * b[i];
        return out;
    }

    get backendName() {
        return 'scalar';
    }

    destroy() {}
}

// ---------------------------------------------------------------------------
// Factory: detect best backend and create engine
// ---------------------------------------------------------------------------

/**
 * Unified compute engine — selects the best available backend.
 *
 * Priority: WebNN > WebGPU > WebGL2 > WASM SIMD > scalar JS.
 *
 * WebNN is preferred over WebGPU for ML workloads because the browser
 * can route to GPU, NPU, or optimized CPU (XNNPACK) transparently,
 * with native op fusion (matmul+bias, conv2d+relu) that custom shaders
 * cannot achieve. WebGPU remains the fallback for general compute and
 * browsers that lack WebNN support.
 *
 * All backends expose the same async interface. The caller does not need
 * to know which backend is active.
 */
export class ComputeEngine {
    /**
     * Create the best available compute engine.
     *
     * @param {object} [options]
     * @param {string} [options.wasmUrl] - URL to simd-ops.wasm for WASM backend.
     * @param {string} [options.forceBackend] - Force: 'webnn' | 'webgpu' | 'webgl2' | 'wasm-simd' | 'scalar'.
     * @returns {Promise<WebNNComputeEngine | WebGPUEngine | WebGL2Engine | WasmSimdEngine | ScalarEngine>}
     */
    static async create(options = {}) {
        const { forceBackend, wasmUrl } = options;

        if (forceBackend != null && !AVAILABLE_BACKENDS.has(forceBackend)) {
            throw new Error(
                `ComputeEngine: unknown requested backend '${forceBackend}'`,
            );
        }

        if (forceBackend === 'scalar') {
            return new ScalarEngine();
        }

        // 1. WebNN (best for ML: native op fusion, NPU access, no shaders)
        //    Chrome 130+/Edge 130+ with origin trial. Routes to DirectML,
        //    CoreML, or XNNPACK depending on platform.
        if (!forceBackend || forceBackend === 'webnn') {
            try {
                const engine = await WebNNComputeEngine.create();
                if (engine) return engine;
            } catch {
                // WebNN not available
            }
            if (forceBackend === 'webnn') {
                throw new Error('ComputeEngine: WebNN requested but not available');
            }
        }

        // 2. WebGPU (great: tiled compute shaders, 10-100x over CPU)
        if (!forceBackend || forceBackend === 'webgpu') {
            try {
                const engine = await WebGPUEngine.create();
                if (engine) return engine;
            } catch {
                // WebGPU not available
            }
            if (forceBackend === 'webgpu') {
                throw new Error('ComputeEngine: WebGPU requested but not available');
            }
        }

        // 3. WebGL2 (good: fragment shader GPGPU, 3-30x over CPU)
        if (!forceBackend || forceBackend === 'webgl2') {
            try {
                const engine = await WebGL2Engine.create();
                if (engine) return engine;
            } catch {
                // WebGL2 not available
            }
            if (forceBackend === 'webgl2') {
                throw new Error('ComputeEngine: WebGL2 requested but not available');
            }
        }

        // 4. WASM SIMD (fallback: CPU with 128-bit SIMD, 2-4x over scalar)
        if (!forceBackend || forceBackend === 'wasm-simd') {
            try {
                const engine = await WasmSimdEngine.create(wasmUrl);
                if (engine) return engine;
            } catch (error) {
                if (forceBackend === 'wasm-simd') throw error;
            }
            if (forceBackend === 'wasm-simd') {
                throw new Error('ComputeEngine: WASM SIMD requested but not available');
            }
        }

        // 5. Scalar JS (always available)
        return new ScalarEngine();
    }
}

export { WebNNComputeEngine, WebGPUEngine, WebGL2Engine, WasmSimdEngine, ScalarEngine };
