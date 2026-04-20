/**
 * WebGL2 compute engine for Falcon-OCR inference.
 *
 * Falls back to this when WebGPU is unavailable (iOS 15-25, older browsers).
 * Uses fragment shaders for GPGPU computation via render-to-texture.
 * 3-5x slower than WebGPU but 3-30x faster than CPU.
 *
 * Follows the same render-to-texture pattern as our GlslRenderer
 * (runtime/molt-gpu/src/render/glsl.rs):
 *   - Input data encoded as RGBA32F textures
 *   - Fragment shaders read via texelFetch (no interpolation)
 *   - gl_FragCoord.xy replaces global_invocation_id as work index
 *   - Output captured via framebuffer-attached textures
 *   - All dtypes narrowed to f32/i32/u32 (WebGL2 shader constraints)
 *
 * Usage:
 *   import { WebGL2Engine } from './webgl2-engine.js';
 *
 *   const engine = new WebGL2Engine();
 *   if (await engine.init()) {
 *     const c = await engine.matmul(aData, bData, 512, 768, 512);
 *     console.log('Result:', c);  // Float32Array of length M*N
 *   }
 */

// ---------------------------------------------------------------------------
// GLSL ES 3.0 vertex shader — fullscreen quad.
//
// Renders a fullscreen triangle (3 vertices, no index buffer) that covers
// the entire viewport. Each fragment corresponds to one output texel.
// ---------------------------------------------------------------------------
const FULLSCREEN_VS = /* glsl */ `#version 300 es
precision highp float;

// Fullscreen triangle: 3 hardcoded vertices covering [-1,1]^2.
// No vertex buffer needed — gl_VertexID drives the positions.
void main() {
    float x = float((gl_VertexID & 1) << 2) - 1.0;
    float y = float((gl_VertexID & 2) << 1) - 1.0;
    gl_Position = vec4(x, y, 0.0, 1.0);
}
`;

// ---------------------------------------------------------------------------
// GLSL ES 3.0 matmul fragment shader.
//
// Computes C[row, col] = sum_i(A[row, i] * B[i, col]) for each fragment.
// Input matrices packed into RGBA32F textures with 1 value per R channel.
// This is the single-value-per-texel variant for simplicity; the RGBA-packed
// variant (4 values per texel) would reduce texture fetches by 4x but
// complicates index arithmetic for non-aligned dimensions.
// ---------------------------------------------------------------------------
const MATMUL_FS = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

uniform sampler2D u_a;   // [M, K] row-major, 1 value per texel R channel
uniform sampler2D u_b;   // [K, N] row-major, 1 value per texel R channel
uniform int u_m;
uniform int u_k;
uniform int u_n;
uniform int u_a_width;   // texture width for A
uniform int u_b_width;   // texture width for B

out vec4 fragColor;

void main() {
    int texel = int(gl_FragCoord.x) + int(gl_FragCoord.y) * u_n;
    int row = texel / u_n;
    int col = texel - row * u_n;

    if (row >= u_m || col >= u_n) {
        fragColor = vec4(0.0);
        return;
    }

    float sum = 0.0;

    // Process 4 iterations at a time for ILP (instruction-level parallelism).
    // The GPU fragment shader compiler will interleave the texture fetches
    // with ALU ops, hiding latency.
    int k4 = (u_k / 4) * 4;
    for (int i = 0; i < k4; i += 4) {
        int a_base = row * u_k;
        int b_col = col;

        float a0 = texelFetch(u_a, ivec2((a_base + i) % u_a_width, (a_base + i) / u_a_width), 0).r;
        float a1 = texelFetch(u_a, ivec2((a_base + i + 1) % u_a_width, (a_base + i + 1) / u_a_width), 0).r;
        float a2 = texelFetch(u_a, ivec2((a_base + i + 2) % u_a_width, (a_base + i + 2) / u_a_width), 0).r;
        float a3 = texelFetch(u_a, ivec2((a_base + i + 3) % u_a_width, (a_base + i + 3) / u_a_width), 0).r;

        int b0_idx = i * u_n + b_col;
        int b1_idx = (i + 1) * u_n + b_col;
        int b2_idx = (i + 2) * u_n + b_col;
        int b3_idx = (i + 3) * u_n + b_col;

        float b0 = texelFetch(u_b, ivec2(b0_idx % u_b_width, b0_idx / u_b_width), 0).r;
        float b1 = texelFetch(u_b, ivec2(b1_idx % u_b_width, b1_idx / u_b_width), 0).r;
        float b2 = texelFetch(u_b, ivec2(b2_idx % u_b_width, b2_idx / u_b_width), 0).r;
        float b3 = texelFetch(u_b, ivec2(b3_idx % u_b_width, b3_idx / u_b_width), 0).r;

        sum += a0 * b0 + a1 * b1 + a2 * b2 + a3 * b3;
    }

    // Scalar tail
    for (int i = k4; i < u_k; i++) {
        int a_idx = row * u_k + i;
        int b_idx = i * u_n + col;
        float a_val = texelFetch(u_a, ivec2(a_idx % u_a_width, a_idx / u_a_width), 0).r;
        float b_val = texelFetch(u_b, ivec2(b_idx % u_b_width, b_idx / u_b_width), 0).r;
        sum += a_val * b_val;
    }

    fragColor = vec4(sum, 0.0, 0.0, 1.0);
}
`;

// ---------------------------------------------------------------------------
// Softmax fragment shader.
//
// Two-pass approach:
//   Pass 1: Compute max(x) and sum(exp(x - max)) in a single shader
//           by writing max to R channel and sum to G channel.
//   Pass 2: Read back max + sum, then compute exp(x - max) / sum.
//
// For browser inference the vector lengths are small (vocab 32000 max),
// so the loop-in-fragment approach is practical without multi-pass ping-pong.
//
// This shader implements Pass 2 (the normalize pass). Pass 1 is done
// as a CPU reduction since the vector is small.
// ---------------------------------------------------------------------------
const SOFTMAX_FS = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

uniform sampler2D u_input;
uniform int u_input_width;
uniform int u_n;
uniform float u_max_val;
uniform float u_inv_sum;

out vec4 fragColor;

void main() {
    int idx = int(gl_FragCoord.x) + int(gl_FragCoord.y) * u_n;

    if (idx >= u_n) {
        fragColor = vec4(0.0);
        return;
    }

    float x = texelFetch(u_input, ivec2(idx % u_input_width, idx / u_input_width), 0).r;
    // softmax(x_i) = exp(x_i - max) / sum(exp(x_j - max))
    // Use exp2(z * log2(e)) since GLSL has exp2 as a native builtin.
    float shifted = x - u_max_val;
    float exp_val = exp(shifted);
    fragColor = vec4(exp_val * u_inv_sum, 0.0, 0.0, 1.0);
}
`;

// ---------------------------------------------------------------------------
// RMSNorm fragment shader.
//
// out[i] = a[i] * w[i] * scale
// where scale = 1 / sqrt(mean(a^2) + eps)
//
// The sum-of-squares reduction is done CPU-side (small vectors).
// This shader applies the per-element scale + weight multiply.
// ---------------------------------------------------------------------------
const RMSNORM_FS = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

uniform sampler2D u_a;
uniform sampler2D u_w;
uniform int u_tex_width;
uniform int u_n;
uniform float u_scale;  // precomputed: 1 / sqrt(mean(a^2) + eps)

out vec4 fragColor;

void main() {
    int idx = int(gl_FragCoord.x) + int(gl_FragCoord.y) * u_n;

    if (idx >= u_n) {
        fragColor = vec4(0.0);
        return;
    }

    float a_val = texelFetch(u_a, ivec2(idx % u_tex_width, idx / u_tex_width), 0).r;
    float w_val = texelFetch(u_w, ivec2(idx % u_tex_width, idx / u_tex_width), 0).r;
    fragColor = vec4(a_val * w_val * u_scale, 0.0, 0.0, 1.0);
}
`;

// ---------------------------------------------------------------------------
// Elementwise add fragment shader.
// ---------------------------------------------------------------------------
const ADD_FS = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

uniform sampler2D u_a;
uniform sampler2D u_b;
uniform int u_tex_width;
uniform int u_n;

out vec4 fragColor;

void main() {
    int idx = int(gl_FragCoord.x) + int(gl_FragCoord.y) * u_n;

    if (idx >= u_n) {
        fragColor = vec4(0.0);
        return;
    }

    float a_val = texelFetch(u_a, ivec2(idx % u_tex_width, idx / u_tex_width), 0).r;
    float b_val = texelFetch(u_b, ivec2(idx % u_tex_width, idx / u_tex_width), 0).r;
    fragColor = vec4(a_val + b_val, 0.0, 0.0, 1.0);
}
`;

// ---------------------------------------------------------------------------
// Elementwise mul fragment shader.
// ---------------------------------------------------------------------------
const MUL_FS = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

uniform sampler2D u_a;
uniform sampler2D u_b;
uniform int u_tex_width;
uniform int u_n;

out vec4 fragColor;

void main() {
    int idx = int(gl_FragCoord.x) + int(gl_FragCoord.y) * u_n;

    if (idx >= u_n) {
        fragColor = vec4(0.0);
        return;
    }

    float a_val = texelFetch(u_a, ivec2(idx % u_tex_width, idx / u_tex_width), 0).r;
    float b_val = texelFetch(u_b, ivec2(idx % u_tex_width, idx / u_tex_width), 0).r;
    fragColor = vec4(a_val * b_val, 0.0, 0.0, 1.0);
}
`;

/**
 * Compute the texture width for packing N float values into a 2D texture.
 * Uses a power-of-2 width capped at the GL max texture size.
 * Height is computed as ceil(n / width).
 *
 * @param {number} n - Number of float values.
 * @param {number} maxSize - GL_MAX_TEXTURE_SIZE.
 * @returns {{ width: number, height: number }}
 */
function computeTexDims(n, maxSize) {
    // Use a width that is a power of 2 for efficient integer division
    // in shaders (compiler turns x / POT into x >> log2(POT)).
    let width = 1;
    while (width * width < n && width < maxSize) {
        width *= 2;
    }
    // Cap at max texture size
    if (width > maxSize) width = maxSize;
    const height = Math.ceil(n / width);
    return { width, height };
}

/**
 * WebGL2 GPGPU compute engine for Falcon-OCR inference.
 *
 * All computation uses the render-to-texture pattern:
 *   1. Pack input data into RGBA32F textures (1 value per texel R channel)
 *   2. Bind input textures as sampler2D uniforms
 *   3. Draw fullscreen triangle — fragment shader performs computation
 *   4. Read output from framebuffer via readPixels
 *
 * This matches the architecture of our GlslRenderer
 * (runtime/molt-gpu/src/render/glsl.rs) but operates at the JS API level
 * rather than generating GLSL source from FusedKernel IR.
 */
export class WebGL2Engine {
    constructor() {
        /** @type {WebGL2RenderingContext | null} */
        this.gl = null;
        /** @type {Record<string, { program: WebGLProgram, uniforms: Record<string, WebGLUniformLocation> }>} */
        this.programs = {};
        /** @type {WebGLVertexArrayObject | null} */
        this.emptyVAO = null;
        /** @type {number} */
        this.maxTexSize = 0;
    }

    /**
     * Initialize WebGL2 context and compile all shaders.
     *
     * @returns {Promise<boolean>} true if WebGL2 + float textures are available.
     */
    async init() {
        const canvas = new OffscreenCanvas(1, 1);
        this.gl = canvas.getContext('webgl2');
        if (!this.gl) return false;

        // EXT_color_buffer_float is required for rendering to RGBA32F textures.
        // Without it, framebuffer-attached float textures are incomplete.
        const ext = this.gl.getExtension('EXT_color_buffer_float');
        if (!ext) return false;

        this.maxTexSize = this.gl.getParameter(this.gl.MAX_TEXTURE_SIZE);

        // Empty VAO for fullscreen triangle draws (no vertex attributes needed).
        this.emptyVAO = this.gl.createVertexArray();

        await this._compileShaders();
        return true;
    }

    /**
     * Compile all GLSL ES 3.0 shader programs.
     */
    async _compileShaders() {
        this.programs.matmul = this._createProgram('matmul', FULLSCREEN_VS, MATMUL_FS, [
            'u_a', 'u_b', 'u_m', 'u_k', 'u_n', 'u_a_width', 'u_b_width',
        ]);
        this.programs.softmax = this._createProgram('softmax', FULLSCREEN_VS, SOFTMAX_FS, [
            'u_input', 'u_input_width', 'u_n', 'u_max_val', 'u_inv_sum',
        ]);
        this.programs.rmsNorm = this._createProgram('rmsNorm', FULLSCREEN_VS, RMSNORM_FS, [
            'u_a', 'u_w', 'u_tex_width', 'u_n', 'u_scale',
        ]);
        this.programs.add = this._createProgram('add', FULLSCREEN_VS, ADD_FS, [
            'u_a', 'u_b', 'u_tex_width', 'u_n',
        ]);
        this.programs.mul = this._createProgram('mul', FULLSCREEN_VS, MUL_FS, [
            'u_a', 'u_b', 'u_tex_width', 'u_n',
        ]);
    }

    /**
     * Compile and link a vertex + fragment shader pair.
     *
     * @param {string} name - Human-readable name for error messages.
     * @param {string} vsSrc - Vertex shader GLSL ES 3.0 source.
     * @param {string} fsSrc - Fragment shader GLSL ES 3.0 source.
     * @param {string[]} uniformNames - Names of uniforms to look up.
     * @returns {{ program: WebGLProgram, uniforms: Record<string, WebGLUniformLocation> }}
     */
    _createProgram(name, vsSrc, fsSrc, uniformNames) {
        const gl = this.gl;

        const vs = gl.createShader(gl.VERTEX_SHADER);
        gl.shaderSource(vs, vsSrc);
        gl.compileShader(vs);
        if (!gl.getShaderParameter(vs, gl.COMPILE_STATUS)) {
            const log = gl.getShaderInfoLog(vs);
            gl.deleteShader(vs);
            throw new Error(`WebGL2Engine: vertex shader compile failed (${name}): ${log}`);
        }

        const fs = gl.createShader(gl.FRAGMENT_SHADER);
        gl.shaderSource(fs, fsSrc);
        gl.compileShader(fs);
        if (!gl.getShaderParameter(fs, gl.COMPILE_STATUS)) {
            const log = gl.getShaderInfoLog(fs);
            gl.deleteShader(vs);
            gl.deleteShader(fs);
            throw new Error(`WebGL2Engine: fragment shader compile failed (${name}): ${log}`);
        }

        const program = gl.createProgram();
        gl.attachShader(program, vs);
        gl.attachShader(program, fs);
        gl.linkProgram(program);
        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
            const log = gl.getProgramInfoLog(program);
            gl.deleteProgram(program);
            gl.deleteShader(vs);
            gl.deleteShader(fs);
            throw new Error(`WebGL2Engine: program link failed (${name}): ${log}`);
        }

        // Shaders are now linked into the program; delete source objects.
        gl.deleteShader(vs);
        gl.deleteShader(fs);

        const uniforms = {};
        for (const uname of uniformNames) {
            uniforms[uname] = gl.getUniformLocation(program, uname);
        }

        return { program, uniforms };
    }

    /**
     * Create an RGBA32F texture from a Float32Array.
     * Each value occupies the R channel of one texel (G=B=A=0).
     *
     * @param {Float32Array} data - Source data.
     * @param {number} n - Number of elements in data.
     * @returns {{ tex: WebGLTexture, width: number, height: number }}
     */
    _createDataTexture(data, n) {
        const gl = this.gl;
        const { width, height } = computeTexDims(n, this.maxTexSize);

        // Pad data to fill the full texture (width * height texels).
        // Each texel is 4 floats (RGBA), but we store 1 value per texel in R.
        const padded = new Float32Array(width * height * 4);
        for (let i = 0; i < n; i++) {
            padded[i * 4] = data[i];  // R channel
        }

        const tex = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.texImage2D(
            gl.TEXTURE_2D, 0, gl.RGBA32F,
            width, height, 0,
            gl.RGBA, gl.FLOAT, padded,
        );
        // NEAREST filtering — no interpolation for GPGPU.
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

        return { tex, width, height };
    }

    /**
     * Create a framebuffer with an RGBA32F texture attachment for output.
     *
     * @param {number} width
     * @param {number} height
     * @returns {{ fbo: WebGLFramebuffer, tex: WebGLTexture, width: number, height: number }}
     */
    _createOutputFBO(width, height) {
        const gl = this.gl;

        const tex = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.texImage2D(
            gl.TEXTURE_2D, 0, gl.RGBA32F,
            width, height, 0,
            gl.RGBA, gl.FLOAT, null,
        );
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

        const fbo = gl.createFramebuffer();
        gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
        gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);

        const status = gl.checkFramebufferStatus(gl.FRAMEBUFFER);
        if (status !== gl.FRAMEBUFFER_COMPLETE) {
            gl.deleteFramebuffer(fbo);
            gl.deleteTexture(tex);
            throw new Error(`WebGL2Engine: framebuffer incomplete (status=${status})`);
        }

        gl.bindFramebuffer(gl.FRAMEBUFFER, null);

        return { fbo, tex, width, height };
    }

    /**
     * Draw a fullscreen triangle and read back the result.
     *
     * @param {number} width - Output texture width.
     * @param {number} height - Output texture height.
     * @param {number} n - Number of output elements to read.
     * @param {WebGLFramebuffer} fbo - Framebuffer to render into.
     * @returns {Float32Array} - Output values (R channel of each texel).
     */
    _drawAndReadback(width, height, n, fbo) {
        const gl = this.gl;

        gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
        gl.viewport(0, 0, width, height);

        gl.bindVertexArray(this.emptyVAO);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
        gl.bindVertexArray(null);

        // Readback via readPixels — synchronous but simpler than PBO.
        // For inference workloads the output is typically small (< 1M elements).
        const raw = new Float32Array(width * height * 4);
        gl.readPixels(0, 0, width, height, gl.RGBA, gl.FLOAT, raw);

        gl.bindFramebuffer(gl.FRAMEBUFFER, null);

        // Extract R channel values.
        const result = new Float32Array(n);
        for (let i = 0; i < n; i++) {
            result[i] = raw[i * 4];
        }
        return result;
    }

    /**
     * Perform C = A @ B where A is [M, K] and B is [K, N].
     *
     * @param {Float32Array} a - Input matrix A, row-major, length M*K.
     * @param {Float32Array} b - Input matrix B, row-major, length K*N.
     * @param {number} m - Number of rows in A.
     * @param {number} k - Shared dimension.
     * @param {number} n - Number of columns in B.
     * @returns {Promise<Float32Array>} - Result C, row-major, length M*N.
     */
    async matmul(a, b, m, k, n) {
        const gl = this.gl;
        const outputN = m * n;

        const texA = this._createDataTexture(a, m * k);
        const texB = this._createDataTexture(b, k * n);
        const { width: outW, height: outH } = computeTexDims(outputN, this.maxTexSize);
        const out = this._createOutputFBO(outW, outH);

        const { program, uniforms } = this.programs.matmul;
        gl.useProgram(program);

        // Bind input textures to texture units.
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, texA.tex);
        gl.uniform1i(uniforms.u_a, 0);

        gl.activeTexture(gl.TEXTURE1);
        gl.bindTexture(gl.TEXTURE_2D, texB.tex);
        gl.uniform1i(uniforms.u_b, 1);

        gl.uniform1i(uniforms.u_m, m);
        gl.uniform1i(uniforms.u_k, k);
        gl.uniform1i(uniforms.u_n, n);
        gl.uniform1i(uniforms.u_a_width, texA.width);
        gl.uniform1i(uniforms.u_b_width, texB.width);

        const result = this._drawAndReadback(outW, outH, outputN, out.fbo);

        // Clean up.
        gl.deleteTexture(texA.tex);
        gl.deleteTexture(texB.tex);
        gl.deleteTexture(out.tex);
        gl.deleteFramebuffer(out.fbo);

        return result;
    }

    /**
     * Softmax over a 1D vector.
     *
     * Pass 1 (CPU): Find max and sum(exp(x - max)) — small vectors.
     * Pass 2 (GPU): Normalize exp(x - max) / sum.
     *
     * @param {Float32Array} input - Input vector of length n.
     * @param {number} n - Vector length.
     * @returns {Promise<Float32Array>}
     */
    async softmax(input, n) {
        const gl = this.gl;

        // CPU pass: find max and exp sum (vectors are small for inference).
        let maxVal = -Infinity;
        for (let i = 0; i < n; i++) {
            if (input[i] > maxVal) maxVal = input[i];
        }
        let expSum = 0;
        for (let i = 0; i < n; i++) {
            expSum += Math.exp(input[i] - maxVal);
        }
        const invSum = 1.0 / expSum;

        // GPU pass: normalize.
        const texIn = this._createDataTexture(input, n);
        const { width: outW, height: outH } = computeTexDims(n, this.maxTexSize);
        const out = this._createOutputFBO(outW, outH);

        const { program, uniforms } = this.programs.softmax;
        gl.useProgram(program);

        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, texIn.tex);
        gl.uniform1i(uniforms.u_input, 0);

        gl.uniform1i(uniforms.u_input_width, texIn.width);
        gl.uniform1i(uniforms.u_n, n);
        gl.uniform1f(uniforms.u_max_val, maxVal);
        gl.uniform1f(uniforms.u_inv_sum, invSum);

        const result = this._drawAndReadback(outW, outH, n, out.fbo);

        gl.deleteTexture(texIn.tex);
        gl.deleteTexture(out.tex);
        gl.deleteFramebuffer(out.fbo);

        return result;
    }

    /**
     * RMSNorm: out[i] = a[i] * w[i] / sqrt(mean(a^2) + eps).
     *
     * Sum-of-squares is computed CPU-side (small vectors).
     * Per-element scale + weight multiply done on GPU.
     *
     * @param {Float32Array} a - Input vector, length n.
     * @param {Float32Array} w - Weight vector, length n.
     * @param {number} n - Vector length.
     * @param {number} eps - Epsilon (default 1e-6).
     * @returns {Promise<Float32Array>}
     */
    async rmsNorm(a, w, n, eps = 1e-6) {
        const gl = this.gl;

        // CPU: compute scale = 1 / sqrt(mean(a^2) + eps).
        let sumSq = 0;
        for (let i = 0; i < n; i++) {
            sumSq += a[i] * a[i];
        }
        const scale = 1.0 / Math.sqrt(sumSq / n + eps);

        const texA = this._createDataTexture(a, n);
        const texW = this._createDataTexture(w, n);
        const { width: outW, height: outH } = computeTexDims(n, this.maxTexSize);
        const out = this._createOutputFBO(outW, outH);

        const { program, uniforms } = this.programs.rmsNorm;
        gl.useProgram(program);

        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, texA.tex);
        gl.uniform1i(uniforms.u_a, 0);

        gl.activeTexture(gl.TEXTURE1);
        gl.bindTexture(gl.TEXTURE_2D, texW.tex);
        gl.uniform1i(uniforms.u_w, 1);

        gl.uniform1i(uniforms.u_tex_width, texA.width);
        gl.uniform1i(uniforms.u_n, n);
        gl.uniform1f(uniforms.u_scale, scale);

        const result = this._drawAndReadback(outW, outH, n, out.fbo);

        gl.deleteTexture(texA.tex);
        gl.deleteTexture(texW.tex);
        gl.deleteTexture(out.tex);
        gl.deleteFramebuffer(out.fbo);

        return result;
    }

    /**
     * Elementwise add: out[i] = a[i] + b[i].
     *
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} n
     * @returns {Promise<Float32Array>}
     */
    async add(a, b, n) {
        return this._elementwiseBinop('add', a, b, n);
    }

    /**
     * Elementwise mul: out[i] = a[i] * b[i].
     *
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} n
     * @returns {Promise<Float32Array>}
     */
    async mul(a, b, n) {
        return this._elementwiseBinop('mul', a, b, n);
    }

    /**
     * Generic elementwise binary op via GPU fragment shader.
     *
     * @param {string} opName - Key into this.programs.
     * @param {Float32Array} a
     * @param {Float32Array} b
     * @param {number} n
     * @returns {Float32Array}
     */
    _elementwiseBinop(opName, a, b, n) {
        const gl = this.gl;

        const texA = this._createDataTexture(a, n);
        const texB = this._createDataTexture(b, n);
        const { width: outW, height: outH } = computeTexDims(n, this.maxTexSize);
        const out = this._createOutputFBO(outW, outH);

        const { program, uniforms } = this.programs[opName];
        gl.useProgram(program);

        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, texA.tex);
        gl.uniform1i(uniforms.u_a, 0);

        gl.activeTexture(gl.TEXTURE1);
        gl.bindTexture(gl.TEXTURE_2D, texB.tex);
        gl.uniform1i(uniforms.u_b, 1);

        gl.uniform1i(uniforms.u_tex_width, texA.width);
        gl.uniform1i(uniforms.u_n, n);

        const result = this._drawAndReadback(outW, outH, n, out.fbo);

        gl.deleteTexture(texA.tex);
        gl.deleteTexture(texB.tex);
        gl.deleteTexture(out.tex);
        gl.deleteFramebuffer(out.fbo);

        return result;
    }

    /**
     * Release all GPU resources. The engine cannot be reused after this.
     */
    destroy() {
        const gl = this.gl;
        if (!gl) return;

        for (const { program } of Object.values(this.programs)) {
            gl.deleteProgram(program);
        }
        this.programs = {};

        if (this.emptyVAO) {
            gl.deleteVertexArray(this.emptyVAO);
            this.emptyVAO = null;
        }

        // WebGL2 context is released when the canvas is GC'd.
        this.gl = null;
    }
}
