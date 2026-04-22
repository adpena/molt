/**
 * WebNN compute engine for molt-gpu tinygrad stack.
 *
 * Uses the Web Neural Network API (W3C Candidate Recommendation, Jan 2026)
 * for hardware-accelerated ML inference. WebNN delegates to the browser's
 * native ML backend — GPU (DirectML/Metal), NPU, or optimized CPU (XNNPACK)
 * — without writing shaders or managing buffers.
 *
 * Slot in the priority chain:
 *   WebNN > WebGPU > WebGL2 > WASM SIMD > scalar
 *
 * WebNN is preferred over WebGPU for ML workloads because:
 *   1. Native op fusion: the browser fuses matmul+add, conv2d+relu, etc.
 *      into single hardware dispatches automatically.
 *   2. NPU access: on devices with neural processing units (Qualcomm,
 *      Apple Neural Engine, Intel NPU), WebNN routes there. WebGPU cannot.
 *   3. No shader authoring: ops like softmax, layerNorm, conv2d are
 *      first-class — no workgroup size tuning or shared memory tiling.
 *   4. Quantized inference: quantizeLinear/dequantizeLinear support INT8
 *      and INT4 natively, enabling 2-4x memory reduction on edge devices.
 *
 * Tinygrad primitive mapping (26 ops):
 *
 *   Arithmetic:
 *     ADD        -> builder.add(a, b)
 *     SUB        -> builder.sub(a, b)
 *     MUL        -> builder.mul(a, b)
 *     IDIV       -> builder.div(a, b)  (with cast to int32 + back)
 *     MOD        -> a - builder.mul(builder.div(a, b), b)  (C-semantics)
 *     NEG        -> builder.neg(a)
 *
 *   Comparison:
 *     CMPLT      -> builder.lesser(a, b) + cast to float
 *     CMPEQ      -> builder.equal(a, b) + cast to float
 *     CMPNE      -> builder.notEqual(a, b) + cast to float
 *
 *   Bitwise:
 *     AND/OR/XOR -> not supported natively; fall through to CPU/WebGPU
 *     SHL/SHR    -> not supported natively; fall through to CPU/WebGPU
 *
 *   Math:
 *     EXP2       -> builder.pow(builder.constant('float32', 2.0), a)
 *     LOG2       -> builder.div(builder.log(a), builder.constant('float32', Math.LN2))
 *     SIN        -> builder.sin(a)
 *     SQRT       -> builder.sqrt(a)
 *     RECIPROCAL -> builder.reciprocal(a)
 *
 *   Other:
 *     TRUNC      -> builder.floor(builder.abs(a)) * builder.sign(a)  (toward zero)
 *     MAX        -> builder.max(a, b)
 *     WHERE      -> builder.where(cond, a, b)  (native ternary select)
 *     CAST       -> builder.cast(a, targetType)
 *     BITCAST    -> not supported; fall through to CPU/WebGPU
 *
 *   Specialized:
 *     REDUCE_SUM -> builder.reduceSum(a, { axes, keepDimensions })
 *     REDUCE_MAX -> builder.reduceMax(a, { axes, keepDimensions })
 *
 *   High-level fused ops (used by inference engine directly):
 *     MATMUL     -> builder.matmul(a, b)
 *     CONV2D     -> builder.conv2d(input, filter, options)
 *     SOFTMAX    -> builder.softmax(input, axis)
 *     RELU       -> builder.relu(input)
 *     GELU       -> builder.gelu(input)
 *     RMSNORM    -> manual: x * weight * rsqrt(reduceMean(x^2) + eps)
 *     LAYERNORM  -> builder.layerNormalization(input, scale, bias, options)
 *
 * Browser support (as of April 2026):
 *   - Chrome 130+: Origin trial, GPU backend via DirectML (Windows)
 *   - Chrome 146+: Origin trial expanded, Metal backend (macOS)
 *   - Edge 130+:   Same as Chrome (Chromium-based)
 *   - Firefox:     Not yet (tracking in Bugzilla)
 *   - Safari:      Not yet (no public signal)
 *
 * Data types: float32, float16, int8, uint8, int32, uint32, int64, uint64
 * Quantization: int4/uint4 via quantizeLinear/dequantizeLinear
 *
 * API pattern:
 *   1. navigator.ml.createContext({ deviceType: 'gpu' })
 *   2. new MLGraphBuilder(context) — construct op graph
 *   3. builder.build({ outputs }) — compile to MLGraph
 *   4. context.dispatch(graph, inputs, outputs) — async execute
 *   5. context.readTensor(outputTensor) — retrieve results
 */

// ---------------------------------------------------------------------------
// Graph cache key generation
// ---------------------------------------------------------------------------

/**
 * Generate a cache key for a graph configuration. WebNN graphs are compiled
 * once and dispatched many times — caching avoids recompilation overhead.
 *
 * @param {string} op - Operation name (e.g., 'matmul', 'softmax').
 * @param  {...number} dims - Dimension parameters.
 * @returns {string}
 */
function _graphKey(op, ...dims) {
    return `${op}:${dims.join(',')}`;
}

/**
 * WebNN compute engine implementing the ComputeEngine interface.
 *
 * Builds and caches WebNN graphs for each (op, shape) combination.
 * Graphs are immutable once compiled, so shape changes produce new graphs.
 * For inference workloads, shapes are typically fixed per model, so the
 * cache hit rate approaches 100% after warmup.
 */
export class WebNNEngine {
    /** @type {MLContext} */
    #context;

    /** @type {Map<string, { graph: MLGraph, inputNames: string[], outputNames: string[] }>} */
    #graphCache;

    constructor(context) {
        this.#context = context;
        this.#graphCache = new Map();
    }

    /**
     * Probe whether WebNN is available in this browser.
     *
     * @returns {Promise<boolean>}
     */
    static async probe() {
        if (typeof navigator === 'undefined') return false;
        if (!('ml' in navigator)) return false;
        try {
            // Attempt to create a context — this validates that the backend
            // (DirectML, CoreML, XNNPACK) is actually functional, not just
            // that the API surface exists.
            const ctx = await navigator.ml.createContext({
                powerPreference: 'high-performance',
            });
            // Context creation succeeded — WebNN is usable.
            // We don't hold this context; create() will make the real one.
            return !!ctx;
        } catch {
            return false;
        }
    }

    /**
     * Create and initialize a WebNN engine.
     *
     * @returns {Promise<WebNNEngine | null>} null if WebNN is unavailable.
     */
    static async create() {
        if (typeof navigator === 'undefined' || !('ml' in navigator)) {
            return null;
        }
        try {
            const context = await navigator.ml.createContext({
                powerPreference: 'high-performance',
            });
            if (!context) return null;
            return new WebNNEngine(context);
        } catch {
            return null;
        }
    }

    // -----------------------------------------------------------------------
    // Graph construction helpers
    // -----------------------------------------------------------------------

    /**
     * Build, compile, and cache a WebNN graph.
     *
     * @param {string} key - Cache key.
     * @param {function(MLGraphBuilder): { inputs: Object, outputs: Object }} buildFn
     *   Function that constructs the graph using the builder. Must return
     *   { inputs: { name: MLOperand }, outputs: { name: MLOperand } }.
     * @returns {Promise<{ graph: MLGraph, inputNames: string[], outputNames: string[] }>}
     */
    async #getOrBuildGraph(key, buildFn) {
        const cached = this.#graphCache.get(key);
        if (cached) return cached;

        const builder = new MLGraphBuilder(this.#context);
        const { inputs, outputs } = buildFn(builder);

        const graph = await builder.build(outputs);
        const inputNames = Object.keys(inputs);
        const outputNames = Object.keys(outputs);
        const entry = { graph, inputNames, outputNames };
        this.#graphCache.set(key, entry);
        return entry;
    }

    /**
     * Create an MLTensor from a Float32Array.
     *
     * @param {Float32Array} data
     * @param {number[]} shape
     * @returns {Promise<MLTensor>}
     */
    async #createInputTensor(data, shape) {
        const tensor = await this.#context.createTensor({
            dataType: 'float32',
            shape,
            writable: true,
        });
        this.#context.writeTensor(tensor, data);
        return tensor;
    }

    /**
     * Create an output MLTensor (readable, for result retrieval).
     *
     * @param {number[]} shape
     * @returns {Promise<MLTensor>}
     */
    async #createOutputTensor(shape) {
        return this.#context.createTensor({
            dataType: 'float32',
            shape,
            readable: true,
        });
    }

    /**
     * Read results from an MLTensor into a Float32Array.
     *
     * @param {MLTensor} tensor
     * @returns {Promise<Float32Array>}
     */
    async #readTensor(tensor) {
        const buffer = await this.#context.readTensor(tensor);
        return new Float32Array(buffer);
    }

    // -----------------------------------------------------------------------
    // ComputeEngine interface: matmul, softmax, rmsNorm, add, mul
    // -----------------------------------------------------------------------

    /**
     * No-op for WebNN — the framework manages device memory internally.
     * Weight tensors are created as constants in the graph.
     *
     * @param {Float32Array} _weightsBuffer
     */
    uploadWeights(_weightsBuffer) {
        // WebNN manages memory via MLTensor. Constants are embedded in the
        // compiled graph. No manual upload needed.
    }

    /**
     * Matrix multiply: C = A @ B where A is [M, K] and B is [K, N].
     *
     * Uses builder.matmul() which maps to hardware-optimized GEMM:
     *   - DirectML: uses DirectX's optimized GEMM kernel
     *   - CoreML: uses Apple's Accelerate/BNNS framework
     *   - XNNPACK: uses SIMD-optimized CPU GEMM
     *
     * @param {Float32Array} a - Input matrix A, row-major, length M*K.
     * @param {Float32Array} b - Input matrix B, row-major, length K*N.
     * @param {number} m - Number of rows in A.
     * @param {number} k - Shared dimension.
     * @param {number} n - Number of columns in B.
     * @returns {Promise<Float32Array>} Result C, row-major, length M*N.
     */
    async matmul(a, b, m, k, n) {
        const key = _graphKey('matmul', m, k, n);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const inputA = builder.input('a', { dataType: 'float32', shape: [m, k] });
            const inputB = builder.input('b', { dataType: 'float32', shape: [k, n] });
            const result = builder.matmul(inputA, inputB);
            return {
                inputs: { a: inputA, b: inputB },
                outputs: { result },
            };
        });

        const tensorA = await this.#createInputTensor(a, [m, k]);
        const tensorB = await this.#createInputTensor(b, [k, n]);
        const tensorOut = await this.#createOutputTensor([m, n]);

        this.#context.dispatch(graph, { a: tensorA, b: tensorB }, { result: tensorOut });
        return this.#readTensor(tensorOut);
    }

    /**
     * Batched matmul — process multiple matmuls.
     *
     * Each op in the batch may have different dimensions, so each gets
     * its own compiled graph (cached by shape).
     *
     * @param {Array<{a: Float32Array, b: Float32Array, m: number, k: number, n: number}>} ops
     * @returns {Promise<Float32Array[]>}
     */
    async matmulBatch(ops) {
        const results = [];
        for (const { a, b, m, k, n } of ops) {
            results.push(await this.matmul(a, b, m, k, n));
        }
        return results;
    }

    /**
     * Softmax over rows.
     *
     * Uses builder.softmax() which applies the numerically stable
     * softmax(x) = exp(x - max(x)) / sum(exp(x - max(x))) as a single
     * fused operation. The browser backend handles numerical stability
     * internally — no manual max-subtract needed.
     *
     * @param {Float32Array} input - [rows * n] elements.
     * @param {number} n - Row length.
     * @param {number} [rows=1] - Number of rows.
     * @returns {Promise<Float32Array>}
     */
    async softmax(input, n, rows = 1) {
        const key = _graphKey('softmax', rows, n);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', { dataType: 'float32', shape: [rows, n] });
            // axis=1: softmax along the last dimension (each row independently)
            const result = builder.softmax(x, 1);
            return {
                inputs: { x },
                outputs: { result },
            };
        });

        const tensorIn = await this.#createInputTensor(input, [rows, n]);
        const tensorOut = await this.#createOutputTensor([rows, n]);

        this.#context.dispatch(graph, { x: tensorIn }, { result: tensorOut });
        return this.#readTensor(tensorOut);
    }

    /**
     * RMSNorm: out[i] = input[i] * weight[i] / sqrt(mean(input^2) + eps).
     *
     * WebNN does not have a native rmsNorm op, so we compose it from
     * primitives. The browser's op fusion pass will typically fuse the
     * mul+reduceSum+div+sqrt+mul chain into a single kernel dispatch.
     *
     * Decomposition:
     *   1. sq = mul(x, x)              — elementwise square
     *   2. mean_sq = reduceMean(sq)     — mean of squares per row
     *   3. sum_eps = add(mean_sq, eps)  — add epsilon
     *   4. scale = reciprocal(sqrt(sum_eps))  — 1/sqrt(mean+eps)
     *   5. result = mul(mul(x, w), scale)     — apply weight and scale
     *
     * @param {Float32Array} input - [rows * n] elements.
     * @param {Float32Array} weight - [n] elements (shared across rows).
     * @param {number} n - Hidden dimension.
     * @param {number} [rows=1] - Number of rows.
     * @param {number} [eps=1e-6] - Epsilon for numerical stability.
     * @returns {Promise<Float32Array>}
     */
    async rmsNorm(input, weight, n, rows = 1, eps = 1e-6) {
        const key = _graphKey('rmsnorm', rows, n, eps);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', { dataType: 'float32', shape: [rows, n] });
            const w = builder.input('w', { dataType: 'float32', shape: [1, n] });

            // RMSNorm decomposition into WebNN primitives
            const sq = builder.mul(x, x);
            const meanSq = builder.reduceMean(sq, { axes: [1], keepDimensions: true });
            const epsConst = builder.constant('float32', eps);
            const sumEps = builder.add(meanSq, epsConst);
            const scale = builder.reciprocal(builder.sqrt(sumEps));
            const weighted = builder.mul(x, w);
            const result = builder.mul(weighted, scale);

            return {
                inputs: { x, w },
                outputs: { result },
            };
        });

        const tensorX = await this.#createInputTensor(input, [rows, n]);
        // Weight is 1D [n], but we pass as [1, n] for broadcasting
        const tensorW = await this.#createInputTensor(weight, [1, n]);
        const tensorOut = await this.#createOutputTensor([rows, n]);

        this.#context.dispatch(
            graph,
            { x: tensorX, w: tensorW },
            { result: tensorOut },
        );
        return this.#readTensor(tensorOut);
    }

    /**
     * Elementwise add: out[i] = a[i] + b[i].
     *
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async add(a, b, size) {
        const key = _graphKey('add', size);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const inputA = builder.input('a', { dataType: 'float32', shape: [size] });
            const inputB = builder.input('b', { dataType: 'float32', shape: [size] });
            const result = builder.add(inputA, inputB);
            return {
                inputs: { a: inputA, b: inputB },
                outputs: { result },
            };
        });

        const tensorA = await this.#createInputTensor(a, [size]);
        const tensorB = await this.#createInputTensor(b, [size]);
        const tensorOut = await this.#createOutputTensor([size]);

        this.#context.dispatch(
            graph,
            { a: tensorA, b: tensorB },
            { result: tensorOut },
        );
        return this.#readTensor(tensorOut);
    }

    /**
     * Elementwise mul: out[i] = a[i] * b[i].
     *
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async mul(a, b, size) {
        const key = _graphKey('mul', size);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const inputA = builder.input('a', { dataType: 'float32', shape: [size] });
            const inputB = builder.input('b', { dataType: 'float32', shape: [size] });
            const result = builder.mul(inputA, inputB);
            return {
                inputs: { a: inputA, b: inputB },
                outputs: { result },
            };
        });

        const tensorA = await this.#createInputTensor(a, [size]);
        const tensorB = await this.#createInputTensor(b, [size]);
        const tensorOut = await this.#createOutputTensor([size]);

        this.#context.dispatch(
            graph,
            { a: tensorA, b: tensorB },
            { result: tensorOut },
        );
        return this.#readTensor(tensorOut);
    }

    // -----------------------------------------------------------------------
    // Extended ops — tinygrad primitive coverage beyond the base interface.
    // These are available for direct use by the tinygrad lowering layer.
    // -----------------------------------------------------------------------

    /**
     * Conv2d: output = conv2d(input, filter, options).
     *
     * Native WebNN conv2d with optional bias, stride, padding, dilation,
     * and groups. Maps directly to hardware-accelerated convolution.
     *
     * @param {Float32Array} input - [N, C_in, H, W] (NCHW layout).
     * @param {Float32Array} filter - [C_out, C_in/groups, kH, kW].
     * @param {Float32Array|null} bias - [C_out] or null.
     * @param {object} params - { n, cIn, h, w, cOut, kH, kW, strideH, strideW,
     *                            padTop, padBottom, padLeft, padRight,
     *                            dilationH, dilationW, groups }.
     * @returns {Promise<Float32Array>}
     */
    async conv2d(input, filter, bias, params) {
        const {
            n: batchSize, cIn, h, w, cOut, kH, kW,
            strideH = 1, strideW = 1,
            padTop = 0, padBottom = 0, padLeft = 0, padRight = 0,
            dilationH = 1, dilationW = 1, groups = 1,
        } = params;

        const key = _graphKey('conv2d', batchSize, cIn, h, w, cOut, kH, kW,
            strideH, strideW, padTop, padBottom, padLeft, padRight,
            dilationH, dilationW, groups, bias ? 1 : 0);

        const outH = Math.floor((h + padTop + padBottom - dilationH * (kH - 1) - 1) / strideH) + 1;
        const outW = Math.floor((w + padLeft + padRight - dilationW * (kW - 1) - 1) / strideW) + 1;

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', {
                dataType: 'float32',
                shape: [batchSize, cIn, h, w],
            });
            const f = builder.input('f', {
                dataType: 'float32',
                shape: [cOut, cIn / groups, kH, kW],
            });

            const convOpts = {
                padding: [padTop, padBottom, padLeft, padRight],
                strides: [strideH, strideW],
                dilations: [dilationH, dilationW],
                groups,
            };

            if (bias) {
                const b = builder.input('b', {
                    dataType: 'float32',
                    shape: [cOut],
                });
                convOpts.bias = b;
                const result = builder.conv2d(x, f, convOpts);
                return {
                    inputs: { x, f, b },
                    outputs: { result },
                };
            }

            const result = builder.conv2d(x, f, convOpts);
            return {
                inputs: { x, f },
                outputs: { result },
            };
        });

        const tensorX = await this.#createInputTensor(input, [batchSize, cIn, h, w]);
        const tensorF = await this.#createInputTensor(filter, [cOut, cIn / groups, kH, kW]);
        const tensorOut = await this.#createOutputTensor([batchSize, cOut, outH, outW]);

        const inputs = { x: tensorX, f: tensorF };
        if (bias) {
            inputs.b = await this.#createInputTensor(bias, [cOut]);
        }

        this.#context.dispatch(graph, inputs, { result: tensorOut });
        return this.#readTensor(tensorOut);
    }

    /**
     * ReLU activation: out = max(0, input).
     *
     * @param {Float32Array} input
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async relu(input, size) {
        const key = _graphKey('relu', size);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', { dataType: 'float32', shape: [size] });
            const result = builder.relu(x);
            return {
                inputs: { x },
                outputs: { result },
            };
        });

        const tensorIn = await this.#createInputTensor(input, [size]);
        const tensorOut = await this.#createOutputTensor([size]);

        this.#context.dispatch(graph, { x: tensorIn }, { result: tensorOut });
        return this.#readTensor(tensorOut);
    }

    /**
     * GELU activation: out = x * 0.5 * (1 + erf(x / sqrt(2))).
     *
     * Native WebNN op — no manual approximation needed.
     *
     * @param {Float32Array} input
     * @param {number} size
     * @returns {Promise<Float32Array>}
     */
    async gelu(input, size) {
        const key = _graphKey('gelu', size);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', { dataType: 'float32', shape: [size] });
            const result = builder.gelu(x);
            return {
                inputs: { x },
                outputs: { result },
            };
        });

        const tensorIn = await this.#createInputTensor(input, [size]);
        const tensorOut = await this.#createOutputTensor([size]);

        this.#context.dispatch(graph, { x: tensorIn }, { result: tensorOut });
        return this.#readTensor(tensorOut);
    }

    /**
     * Layer normalization.
     *
     * Native WebNN layerNormalization — fused into a single kernel.
     *
     * @param {Float32Array} input - [rows, n] elements.
     * @param {Float32Array} scale - [n] gamma weights.
     * @param {Float32Array} bias - [n] beta weights.
     * @param {number} n - Feature dimension.
     * @param {number} [rows=1]
     * @param {number} [eps=1e-5]
     * @returns {Promise<Float32Array>}
     */
    async layerNorm(input, scale, bias, n, rows = 1, eps = 1e-5) {
        const key = _graphKey('layernorm', rows, n, eps);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', { dataType: 'float32', shape: [rows, n] });
            const s = builder.input('s', { dataType: 'float32', shape: [n] });
            const b = builder.input('b', { dataType: 'float32', shape: [n] });
            const result = builder.layerNormalization(x, s, b, {
                axes: [1],
                epsilon: eps,
            });
            return {
                inputs: { x, s, b },
                outputs: { result },
            };
        });

        const tensorX = await this.#createInputTensor(input, [rows, n]);
        const tensorS = await this.#createInputTensor(scale, [n]);
        const tensorB = await this.#createInputTensor(bias, [n]);
        const tensorOut = await this.#createOutputTensor([rows, n]);

        this.#context.dispatch(
            graph,
            { x: tensorX, s: tensorS, b: tensorB },
            { result: tensorOut },
        );
        return this.#readTensor(tensorOut);
    }

    /**
     * Reduce sum along specified axis.
     *
     * @param {Float32Array} input
     * @param {number[]} shape - Input shape.
     * @param {number} axis - Axis to reduce.
     * @param {boolean} [keepDims=false]
     * @returns {Promise<Float32Array>}
     */
    async reduceSum(input, shape, axis, keepDims = false) {
        const key = _graphKey('reducesum', ...shape, axis, keepDims ? 1 : 0);

        const outShape = shape.slice();
        if (keepDims) {
            outShape[axis] = 1;
        } else {
            outShape.splice(axis, 1);
        }
        if (outShape.length === 0) outShape.push(1);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', { dataType: 'float32', shape });
            const result = builder.reduceSum(x, { axes: [axis], keepDimensions: keepDims });
            return {
                inputs: { x },
                outputs: { result },
            };
        });

        const tensorIn = await this.#createInputTensor(input, shape);
        const tensorOut = await this.#createOutputTensor(outShape);

        this.#context.dispatch(graph, { x: tensorIn }, { result: tensorOut });
        return this.#readTensor(tensorOut);
    }

    /**
     * Reduce max along specified axis.
     *
     * @param {Float32Array} input
     * @param {number[]} shape - Input shape.
     * @param {number} axis - Axis to reduce.
     * @param {boolean} [keepDims=false]
     * @returns {Promise<Float32Array>}
     */
    async reduceMax(input, shape, axis, keepDims = false) {
        const key = _graphKey('reducemax', ...shape, axis, keepDims ? 1 : 0);

        const outShape = shape.slice();
        if (keepDims) {
            outShape[axis] = 1;
        } else {
            outShape.splice(axis, 1);
        }
        if (outShape.length === 0) outShape.push(1);

        const { graph } = await this.#getOrBuildGraph(key, (builder) => {
            const x = builder.input('x', { dataType: 'float32', shape });
            const result = builder.reduceMax(x, { axes: [axis], keepDimensions: keepDims });
            return {
                inputs: { x },
                outputs: { result },
            };
        });

        const tensorIn = await this.#createInputTensor(input, shape);
        const tensorOut = await this.#createOutputTensor(outShape);

        this.#context.dispatch(graph, { x: tensorIn }, { result: tensorOut });
        return this.#readTensor(tensorOut);
    }

    // -----------------------------------------------------------------------
    // Metadata
    // -----------------------------------------------------------------------

    get backendName() {
        return 'webnn';
    }

    /**
     * Release all cached graphs. The engine can still be used after this
     * (graphs will be rebuilt on next dispatch), but call this when done
     * to free compiled graph resources.
     */
    destroy() {
        this.#graphCache.clear();
        // MLContext has no explicit destroy — relies on GC.
    }
}
