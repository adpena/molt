/**
 * WebGPU matmul for Falcon-OCR browser inference.
 *
 * Replaces CPU matmul with GPU compute shader for 10-100x speedup on the
 * GEMM operations that dominate transformer inference (~90% of compute).
 *
 * The WGSL shader follows the same conventions as our WgslRenderer
 * (runtime/molt-gpu/src/render/wgsl.rs):
 *   - Entry point: molt_kernel
 *   - @builtin(global_invocation_id) for thread indexing
 *   - @group(0) @binding(N) for storage buffers
 *   - f32 dtype (narrowed via DType::narrow_webgpu)
 *   - fma() for fused multiply-add where profitable
 *
 * Usage:
 *   import { createWebGPUMatmul } from './webgpu-matmul.js';
 *
 *   const gpu = await createWebGPUMatmul();
 *   if (gpu) {
 *     // C = A @ B where A is [M, K] and B is [K, N]
 *     const c = await gpu.matmul(aData, bData, 512, 768, 512);
 *     console.log('Result:', c);  // Float32Array of length M*N
 *   }
 */

// ---------------------------------------------------------------------------
// WGSL matmul compute shader — tiled 16x16.
//
// This is the WGSL that our WgslRenderer would produce for a fused
// MUL + REDUCE_SUM kernel with 2D tiling. The tiled approach uses
// workgroup shared memory to amortize global memory reads:
//   - Each workgroup loads a 16x16 tile of A and B into shared memory
//   - Each thread computes one element of the output tile
//   - The loop over the K dimension processes 16 columns at a time
//
// Buffer layout matches WgslRenderer conventions:
//   buf0 (binding 0): output C [M * N], read_write
//   buf1 (binding 1): input A  [M * K], read
//   buf2 (binding 2): input B  [K * N], read
//   buf3 (binding 3): dimensions uniform [M, K, N], read
// ---------------------------------------------------------------------------
const TILE_SIZE = 16;

const MATMUL_WGSL = /* wgsl */ `
// Dimensions: M, K, N passed as uniform buffer.
@group(0) @binding(0) var<storage, read_write> buf0: array<f32>;
@group(0) @binding(1) var<storage, read> buf1: array<f32>;
@group(0) @binding(2) var<storage, read> buf2: array<f32>;
@group(0) @binding(3) var<uniform> dims: vec4<u32>;

var<workgroup> tile_a: array<f32, ${TILE_SIZE * TILE_SIZE}>;
var<workgroup> tile_b: array<f32, ${TILE_SIZE * TILE_SIZE}>;

@compute @workgroup_size(${TILE_SIZE}, ${TILE_SIZE}, 1)
fn molt_kernel(
    @builtin(global_invocation_id) gid_vec: vec3<u32>,
    @builtin(local_invocation_id) lid_vec: vec3<u32>,
    @builtin(workgroup_id) wg_vec: vec3<u32>
) {
    let M = dims.x;
    let K = dims.y;
    let N = dims.z;

    let row = wg_vec.y * ${TILE_SIZE}u + lid_vec.y;
    let col = wg_vec.x * ${TILE_SIZE}u + lid_vec.x;

    let local_idx = lid_vec.y * ${TILE_SIZE}u + lid_vec.x;

    var acc: f32 = f32(0);

    let num_tiles = (K + ${TILE_SIZE - 1}u) / ${TILE_SIZE}u;

    for (var t: u32 = 0u; t < num_tiles; t = t + 1u) {
        // Load tile of A into shared memory.
        let a_col = t * ${TILE_SIZE}u + lid_vec.x;
        if (row < M && a_col < K) {
            tile_a[local_idx] = buf1[row * K + a_col];
        } else {
            tile_a[local_idx] = f32(0);
        }

        // Load tile of B into shared memory.
        let b_row = t * ${TILE_SIZE}u + lid_vec.y;
        if (b_row < K && col < N) {
            tile_b[local_idx] = buf2[b_row * N + col];
        } else {
            tile_b[local_idx] = f32(0);
        }

        workgroupBarrier();

        // Accumulate dot product for this tile.
        for (var k: u32 = 0u; k < ${TILE_SIZE}u; k = k + 1u) {
            acc = fma(
                tile_a[lid_vec.y * ${TILE_SIZE}u + k],
                tile_b[k * ${TILE_SIZE}u + lid_vec.x],
                acc
            );
        }

        workgroupBarrier();
    }

    // Write output — bounds check for edge tiles.
    if (row < M && col < N) {
        buf0[row * N + col] = acc;
    }
}
`;

/**
 * Create a WebGPU-accelerated matmul engine.
 *
 * Returns null if WebGPU is not available (browser too old, no GPU, etc.).
 * The caller should fall back to CPU matmul in that case.
 *
 * @returns {Promise<{matmul: Function, destroy: Function} | null>}
 */
export async function createWebGPUMatmul() {
    if (typeof navigator === 'undefined' || !navigator.gpu) {
        return null;
    }

    const adapter = await navigator.gpu.requestAdapter({
        powerPreference: 'high-performance',
    });
    if (!adapter) {
        return null;
    }

    const device = await adapter.requestDevice({
        requiredLimits: {
            maxStorageBufferBindingSize: 512 * 1024 * 1024,  // 512 MB for large weight matrices
            maxBufferSize: 512 * 1024 * 1024,
            maxComputeWorkgroupsPerDimension: 65535,
        },
    });

    // Compile the matmul WGSL shader.
    const shaderModule = device.createShaderModule({ code: MATMUL_WGSL });

    // Create bind group layout: 3 storage buffers + 1 uniform.
    const bindGroupLayout = device.createBindGroupLayout({
        entries: [
            { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'storage' } },
            { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'read-only-storage' } },
            { binding: 2, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'read-only-storage' } },
            { binding: 3, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'uniform' } },
        ],
    });

    const pipelineLayout = device.createPipelineLayout({
        bindGroupLayouts: [bindGroupLayout],
    });

    const pipeline = device.createComputePipeline({
        layout: pipelineLayout,
        compute: { module: shaderModule, entryPoint: 'molt_kernel' },
    });

    /**
     * Perform C = A @ B where A is [M, K] and B is [K, N].
     *
     * @param {Float32Array} a - Input matrix A, row-major, length M*K.
     * @param {Float32Array} b - Input matrix B, row-major, length K*N.
     * @param {number} m - Number of rows in A.
     * @param {number} k - Shared dimension (columns of A, rows of B).
     * @param {number} n - Number of columns in B.
     * @returns {Promise<Float32Array>} - Result matrix C, row-major, length M*N.
     */
    async function matmul(a, b, m, k, n) {
        const aBytes = m * k * 4;
        const bBytes = k * n * 4;
        const cBytes = m * n * 4;

        // Create GPU buffers.
        const bufC = device.createBuffer({
            size: cBytes,
            usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
        });
        const bufA = device.createBuffer({
            size: aBytes,
            usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
        });
        const bufB = device.createBuffer({
            size: bBytes,
            usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
        });
        const bufDims = device.createBuffer({
            size: 16,  // vec4<u32> = 16 bytes
            usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
        });

        // Upload data to GPU.
        device.queue.writeBuffer(bufA, 0, a);
        device.queue.writeBuffer(bufB, 0, b);
        device.queue.writeBuffer(bufDims, 0, new Uint32Array([m, k, n, 0]));

        // Create bind group.
        const bindGroup = device.createBindGroup({
            layout: bindGroupLayout,
            entries: [
                { binding: 0, resource: { buffer: bufC } },
                { binding: 1, resource: { buffer: bufA } },
                { binding: 2, resource: { buffer: bufB } },
                { binding: 3, resource: { buffer: bufDims } },
            ],
        });

        // Dispatch compute.
        const workgroupsX = Math.ceil(n / TILE_SIZE);
        const workgroupsY = Math.ceil(m / TILE_SIZE);

        const encoder = device.createCommandEncoder();
        const pass = encoder.beginComputePass();
        pass.setPipeline(pipeline);
        pass.setBindGroup(0, bindGroup);
        pass.dispatchWorkgroups(workgroupsX, workgroupsY, 1);
        pass.end();

        // Staging buffer for readback.
        const staging = device.createBuffer({
            size: cBytes,
            usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST,
        });
        encoder.copyBufferToBuffer(bufC, 0, staging, 0, cBytes);

        device.queue.submit([encoder.finish()]);

        // Read results back to CPU.
        await staging.mapAsync(GPUMapMode.READ);
        const result = new Float32Array(staging.getMappedRange().slice(0));
        staging.unmap();

        // Clean up GPU buffers.
        bufA.destroy();
        bufB.destroy();
        bufC.destroy();
        bufDims.destroy();
        staging.destroy();

        return result;
    }

    /**
     * Batched matmul — encode multiple dispatches in a single command buffer.
     *
     * Per Maczan 2026, command submission accounts for ~40% of per-dispatch
     * overhead. For batch inference (multiple images), encoding all matmuls
     * into one submit() call eliminates redundant submission overhead.
     *
     * @param {Array<{a: Float32Array, b: Float32Array, m: number, k: number, n: number}>} ops
     * @returns {Promise<Float32Array[]>}
     */
    async function matmulBatch(ops) {
        if (ops.length === 0) return [];

        const buffers = [];
        const stagingBuffers = [];
        const encoder = device.createCommandEncoder();

        for (const { a, b, m, k, n } of ops) {
            const aBytes = m * k * 4;
            const bBytes = k * n * 4;
            const cBytes = m * n * 4;

            const bufC = device.createBuffer({
                size: cBytes,
                usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
            });
            const bufA = device.createBuffer({
                size: aBytes,
                usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
            });
            const bufB = device.createBuffer({
                size: bBytes,
                usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
            });
            const bufDims = device.createBuffer({
                size: 16,
                usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
            });

            device.queue.writeBuffer(bufA, 0, a);
            device.queue.writeBuffer(bufB, 0, b);
            device.queue.writeBuffer(bufDims, 0, new Uint32Array([m, k, n, 0]));

            const bindGroup = device.createBindGroup({
                layout: bindGroupLayout,
                entries: [
                    { binding: 0, resource: { buffer: bufC } },
                    { binding: 1, resource: { buffer: bufA } },
                    { binding: 2, resource: { buffer: bufB } },
                    { binding: 3, resource: { buffer: bufDims } },
                ],
            });

            const pass = encoder.beginComputePass();
            pass.setPipeline(pipeline);
            pass.setBindGroup(0, bindGroup);
            pass.dispatchWorkgroups(Math.ceil(n / TILE_SIZE), Math.ceil(m / TILE_SIZE), 1);
            pass.end();

            const staging = device.createBuffer({
                size: cBytes,
                usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST,
            });
            encoder.copyBufferToBuffer(bufC, 0, staging, 0, cBytes);

            buffers.push(bufA, bufB, bufC, bufDims);
            stagingBuffers.push({ staging, cBytes });
        }

        // Single submit for all dispatches.
        device.queue.submit([encoder.finish()]);

        // Read back all results.
        const results = [];
        for (const { staging, cBytes } of stagingBuffers) {
            await staging.mapAsync(GPUMapMode.READ);
            results.push(new Float32Array(staging.getMappedRange().slice(0)));
            staging.unmap();
        }

        // Clean up.
        for (const buf of buffers) buf.destroy();
        for (const { staging } of stagingBuffers) staging.destroy();

        return results;
    }

    /**
     * Release GPU resources. Call when done with matmul operations.
     */
    function destroy() {
        device.destroy();
    }

    return { matmul, matmulBatch, destroy, device };
}

/**
 * CPU reference matmul for validation and fallback.
 *
 * @param {Float32Array} a - Input matrix A [M, K], row-major.
 * @param {Float32Array} b - Input matrix B [K, N], row-major.
 * @param {number} m
 * @param {number} k
 * @param {number} n
 * @returns {Float32Array}
 */
export function cpuMatmul(a, b, m, k, n) {
    const c = new Float32Array(m * n);
    for (let i = 0; i < m; i++) {
        for (let j = 0; j < n; j++) {
            let sum = 0;
            for (let p = 0; p < k; p++) {
                sum += a[i * k + p] * b[p * n + j];
            }
            c[i * n + j] = sum;
        }
    }
    return c;
}

/**
 * Validate GPU matmul against CPU reference.
 *
 * @param {number} m
 * @param {number} k
 * @param {number} n
 * @param {number} tolerance - Max absolute error per element (default 1e-3 for f32).
 * @returns {Promise<{ok: boolean, maxError: number, gpuMs: number, cpuMs: number}>}
 */
export async function validateMatmul(m = 512, k = 768, n = 512, tolerance = 1e-3) {
    const gpu = await createWebGPUMatmul();
    if (!gpu) {
        return { ok: false, maxError: Infinity, gpuMs: 0, cpuMs: 0, error: 'no WebGPU' };
    }

    // Generate random test data.
    const a = new Float32Array(m * k);
    const b = new Float32Array(k * n);
    for (let i = 0; i < a.length; i++) a[i] = Math.random() * 2 - 1;
    for (let i = 0; i < b.length; i++) b[i] = Math.random() * 2 - 1;

    // CPU reference.
    const cpuStart = performance.now();
    const cpuResult = cpuMatmul(a, b, m, k, n);
    const cpuMs = performance.now() - cpuStart;

    // GPU.
    const gpuStart = performance.now();
    const gpuResult = await gpu.matmul(a, b, m, k, n);
    const gpuMs = performance.now() - gpuStart;

    // Compare.
    let maxError = 0;
    for (let i = 0; i < cpuResult.length; i++) {
        const err = Math.abs(cpuResult[i] - gpuResult[i]);
        if (err > maxError) maxError = err;
    }

    gpu.destroy();

    return {
        ok: maxError <= tolerance,
        maxError,
        gpuMs,
        cpuMs,
        speedup: cpuMs / gpuMs,
        size: `${m}x${k} @ ${k}x${n}`,
    };
}
