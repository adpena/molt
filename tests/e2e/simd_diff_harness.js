// Differential testing: Zig vs Rust WASM SIMD kernels.
//
// Both implementations must produce identical results for identical inputs.
// Tests every shared exported function with random, adversarial, and edge-case
// inputs. Compares outputs BIT-FOR-BIT using DataView.getUint32 on f32 memory.
//
// Run: node tests/e2e/simd_diff_harness.js
//
// Exit code 0 = all tests pass (bit-identical).
// Exit code 1 = at least one mismatch found.

'use strict';

const fs = require('fs');
const path = require('path');
const { performance } = require('perf_hooks');

const ROOT = path.resolve(__dirname, '../..');
const RUST_WASM = path.join(ROOT, 'deploy/browser/simd-ops-rs/target/wasm32-unknown-unknown/release/simd_ops.wasm');
const ZIG_WASM = path.join(ROOT, 'deploy/browser/simd-ops-zig/simd.wasm');

// ---------------------------------------------------------------------------
// WASM module loader
// ---------------------------------------------------------------------------

async function loadModule(wasmPath) {
    const bytes = fs.readFileSync(wasmPath);
    const memory = new WebAssembly.Memory({ initial: 256 });
    let instance;
    try {
        // Try with imported memory first (Zig may export its own)
        ({ instance } = await WebAssembly.instantiate(bytes, {
            env: { memory },
        }));
    } catch {
        // Module exports its own memory (Rust does this)
        ({ instance } = await WebAssembly.instantiate(bytes, {}));
    }
    const mem = instance.exports.memory || memory;
    return { exports: instance.exports, memory: mem };
}

// ---------------------------------------------------------------------------
// Memory helpers
// ---------------------------------------------------------------------------

function writeF32(memory, offset, arr) {
    const view = new Float32Array(memory.buffer, offset, arr.length);
    view.set(arr);
}

function readF32(memory, offset, n) {
    return new Float32Array(memory.buffer, offset, n);
}

function readBitsU32(memory, offset, n) {
    return new Uint32Array(memory.buffer, offset, n);
}

// Compare two f32 arrays bit-for-bit. Returns { pass, mismatches }.
function compareBits(memA, offsetA, memB, offsetB, n, label) {
    const bitsA = readBitsU32(memA, offsetA, n);
    const bitsB = readBitsU32(memB, offsetB, n);
    const floatsA = readF32(memA, offsetA, n);
    const floatsB = readF32(memB, offsetB, n);
    const mismatches = [];
    for (let i = 0; i < n; i++) {
        if (bitsA[i] !== bitsB[i]) {
            // Allow both being NaN (any NaN bit pattern)
            const isNanA = (bitsA[i] & 0x7F800000) === 0x7F800000 && (bitsA[i] & 0x007FFFFF) !== 0;
            const isNanB = (bitsB[i] & 0x7F800000) === 0x7F800000 && (bitsB[i] & 0x007FFFFF) !== 0;
            if (isNanA && isNanB) continue; // Both NaN — acceptable
            mismatches.push({
                index: i,
                rustBits: '0x' + bitsA[i].toString(16).padStart(8, '0'),
                zigBits: '0x' + bitsB[i].toString(16).padStart(8, '0'),
                rustFloat: floatsA[i],
                zigFloat: floatsB[i],
            });
        }
    }
    return { pass: mismatches.length === 0, mismatches };
}

// Generate n random f32 in [lo, hi].
function randomF32(n, lo = -10, hi = 10) {
    const arr = new Float32Array(n);
    for (let i = 0; i < n; i++) {
        arr[i] = lo + Math.random() * (hi - lo);
    }
    return arr;
}

// Seeded PRNG for reproducibility (xorshift32).
function xorshift32(seed) {
    let s = seed >>> 0 || 1;
    return function () {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        return (s >>> 0) / 0x100000000;
    };
}

// ---------------------------------------------------------------------------
// Test framework
// ---------------------------------------------------------------------------

let totalTests = 0;
let passedTests = 0;
let failedTests = 0;

function report(name, result) {
    totalTests++;
    if (result.pass) {
        passedTests++;
        console.log(`  PASS  ${name}`);
    } else {
        failedTests++;
        console.log(`  FAIL  ${name} — ${result.mismatches.length} mismatch(es):`);
        for (const m of result.mismatches.slice(0, 5)) {
            console.log(`        [${m.index}] Rust=${m.rustBits} (${m.rustFloat}) vs Zig=${m.zigBits} (${m.zigFloat})`);
        }
        if (result.mismatches.length > 5) {
            console.log(`        ... and ${result.mismatches.length - 5} more`);
        }
    }
}

// Memory layout: we allocate regions in linear memory for inputs/outputs.
// Region A: 0x10000..0x20000  (input a / q)
// Region B: 0x20000..0x30000  (input b / freqs_cos)
// Region C: 0x30000..0x40000  (output for Rust)
// Region D: 0x40000..0x50000  (output for Zig / freqs_sin)
// Region E: 0x50000..0x60000  (weights for rms_norm)
const REGION_A = 0x10000;
const REGION_B = 0x20000;
const REGION_C = 0x30000;
const REGION_D = 0x40000;
const REGION_E = 0x50000;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

async function main() {
    console.log('Loading WASM modules...');
    const rust = await loadModule(RUST_WASM);
    const zig = await loadModule(ZIG_WASM);
    console.log(`  Rust: ${Object.keys(rust.exports).filter(k => typeof rust.exports[k] === 'function').length} functions`);
    console.log(`  Zig:  ${Object.keys(zig.exports).filter(k => typeof zig.exports[k] === 'function').length} functions`);
    console.log('');

    const rng = xorshift32(42);

    // Helper: generate reproducible random f32 array
    function randArr(n, lo = -10, hi = 10) {
        const arr = new Float32Array(n);
        for (let i = 0; i < n; i++) arr[i] = lo + rng() * (hi - lo);
        return arr;
    }

    // Helper: generate positive random f32 array
    function randArrPos(n, lo = 0.01, hi = 10) {
        const arr = new Float32Array(n);
        for (let i = 0; i < n; i++) arr[i] = lo + rng() * (hi - lo);
        return arr;
    }

    // -----------------------------------------------------------------------
    // Test: add_f32
    // -----------------------------------------------------------------------
    console.log('--- add_f32 ---');
    for (const n of [1, 4, 7, 16, 63, 256, 1024]) {
        const a = randArr(n);
        const b = randArr(n);
        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        rust.exports.add_f32(REGION_A, REGION_B, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        zig.exports.add_f32(REGION_A, REGION_B, REGION_D, n);

        // Copy Zig output to Rust memory for comparison
        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`add_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: mul_f32
    // -----------------------------------------------------------------------
    console.log('--- mul_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArr(n);
        const b = randArr(n);
        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        rust.exports.mul_f32(REGION_A, REGION_B, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        zig.exports.mul_f32(REGION_A, REGION_B, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`mul_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: neg_f32
    // -----------------------------------------------------------------------
    console.log('--- neg_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArr(n);
        writeF32(rust.memory, REGION_A, a);
        rust.exports.neg_f32(REGION_A, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        zig.exports.neg_f32(REGION_A, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`neg_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: sqrt_f32
    // -----------------------------------------------------------------------
    console.log('--- sqrt_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArrPos(n);
        writeF32(rust.memory, REGION_A, a);
        rust.exports.sqrt_f32(REGION_A, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        zig.exports.sqrt_f32(REGION_A, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`sqrt_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: reciprocal_f32
    // -----------------------------------------------------------------------
    console.log('--- reciprocal_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArrPos(n, 0.1, 100);
        writeF32(rust.memory, REGION_A, a);
        rust.exports.reciprocal_f32(REGION_A, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        zig.exports.reciprocal_f32(REGION_A, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`reciprocal_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: max_f32 (with NaN propagation)
    // -----------------------------------------------------------------------
    console.log('--- max_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArr(n);
        const b = randArr(n);
        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        rust.exports.max_f32(REGION_A, REGION_B, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        zig.exports.max_f32(REGION_A, REGION_B, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`max_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: exp2_f32
    // -----------------------------------------------------------------------
    console.log('--- exp2_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArr(n, -10, 10);
        writeF32(rust.memory, REGION_A, a);
        rust.exports.exp2_f32(REGION_A, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        zig.exports.exp2_f32(REGION_A, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`exp2_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: reduce_sum_f32
    // -----------------------------------------------------------------------
    console.log('--- reduce_sum_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArr(n);
        writeF32(rust.memory, REGION_A, a);
        const rustResult = rust.exports.reduce_sum_f32(REGION_A, n);

        writeF32(zig.memory, REGION_A, a);
        const zigResult = zig.exports.reduce_sum_f32(REGION_A, n);

        const rustBits = new Uint32Array(new Float32Array([rustResult]).buffer)[0];
        const zigBits = new Uint32Array(new Float32Array([zigResult]).buffer)[0];
        const pass = rustBits === zigBits ||
            (Number.isNaN(rustResult) && Number.isNaN(zigResult));
        report(`reduce_sum_f32 n=${n}`, {
            pass,
            mismatches: pass ? [] : [{
                index: 0,
                rustBits: '0x' + rustBits.toString(16).padStart(8, '0'),
                zigBits: '0x' + zigBits.toString(16).padStart(8, '0'),
                rustFloat: rustResult,
                zigFloat: zigResult,
            }],
        });
    }

    // -----------------------------------------------------------------------
    // Test: reduce_max_f32
    // -----------------------------------------------------------------------
    console.log('--- reduce_max_f32 ---');
    for (const n of [1, 4, 7, 16, 256]) {
        const a = randArr(n);
        writeF32(rust.memory, REGION_A, a);
        const rustResult = rust.exports.reduce_max_f32(REGION_A, n);

        writeF32(zig.memory, REGION_A, a);
        const zigResult = zig.exports.reduce_max_f32(REGION_A, n);

        const rustBits = new Uint32Array(new Float32Array([rustResult]).buffer)[0];
        const zigBits = new Uint32Array(new Float32Array([zigResult]).buffer)[0];
        const pass = rustBits === zigBits ||
            (Number.isNaN(rustResult) && Number.isNaN(zigResult));
        report(`reduce_max_f32 n=${n}`, {
            pass,
            mismatches: pass ? [] : [{
                index: 0,
                rustBits: '0x' + rustBits.toString(16).padStart(8, '0'),
                zigBits: '0x' + zigBits.toString(16).padStart(8, '0'),
                rustFloat: rustResult,
                zigFloat: zigResult,
            }],
        });
    }

    // -----------------------------------------------------------------------
    // Test: softmax_f32_fused
    // -----------------------------------------------------------------------
    console.log('--- softmax_f32_fused ---');
    for (const n of [1, 4, 7, 16, 64, 256]) {
        const a = randArr(n, -5, 5);
        writeF32(rust.memory, REGION_A, a);
        rust.exports.softmax_f32_fused(REGION_A, REGION_C, n);

        writeF32(zig.memory, REGION_A, a);
        zig.exports.softmax_f32_fused(REGION_A, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`softmax_f32_fused n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: matmul_f32_tiled
    // -----------------------------------------------------------------------
    console.log('--- matmul_f32_tiled ---');
    for (const [m, k, n] of [[4, 4, 4], [8, 8, 8], [5, 7, 3], [16, 16, 16], [1, 1, 1], [64, 64, 64]]) {
        const a = randArr(m * k);
        const b = randArr(k * n);

        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        rust.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, m, k, n);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        zig.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_D, m, k, n);

        const zigOut = readF32(zig.memory, REGION_D, m * n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`matmul_f32_tiled ${m}x${k}x${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, m * n));
    }

    // -----------------------------------------------------------------------
    // Test: rms_norm_f32
    // -----------------------------------------------------------------------
    console.log('--- rms_norm_f32 ---');
    for (const n of [4, 7, 16, 64, 256]) {
        const a = randArr(n);
        const w = randArr(n, 0.5, 1.5);
        const eps = 1e-5;

        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_E, w);
        rust.exports.rms_norm_f32(REGION_A, REGION_E, REGION_C, n, eps);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_E, w);
        zig.exports.rms_norm_f32(REGION_A, REGION_E, REGION_D, n, eps);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`rms_norm_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Test: rope_f32
    // -----------------------------------------------------------------------
    console.log('--- rope_f32 ---');
    for (const n of [4, 8, 16, 64]) {
        const q = randArr(n);
        const freqs_cos = randArr(n / 2, -1, 1);
        const freqs_sin = randArr(n / 2, -1, 1);

        writeF32(rust.memory, REGION_A, q);
        writeF32(rust.memory, REGION_B, freqs_cos);
        writeF32(rust.memory, REGION_D, freqs_sin);
        rust.exports.rope_f32(REGION_A, REGION_B, REGION_D, REGION_C, n);

        writeF32(zig.memory, REGION_A, q);
        writeF32(zig.memory, REGION_B, freqs_cos);
        writeF32(zig.memory, REGION_D, freqs_sin);
        // Use REGION_E for Zig output to avoid clobbering REGION_D (freqs_sin)
        zig.exports.rope_f32(REGION_A, REGION_B, REGION_D, REGION_E, n);

        const zigOut = readF32(zig.memory, REGION_E, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`rope_f32 n=${n}`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));
    }

    // -----------------------------------------------------------------------
    // Adversarial inputs: NaN, inf, -0.0, subnormals
    // -----------------------------------------------------------------------
    console.log('--- adversarial inputs ---');
    {
        const edges = new Float32Array([
            0, -0, Infinity, -Infinity, NaN,
            1e-38,  // subnormal-ish
            -1e-38,
            1e38,
            -1e38,
            3.4028235e+38,  // f32 max
            1.1754944e-38,  // f32 min normal
            1.4e-45,        // f32 min subnormal
        ]);
        const n = edges.length;
        const b = randArr(n);

        // add_f32 with edge cases
        writeF32(rust.memory, REGION_A, edges);
        writeF32(rust.memory, REGION_B, b);
        rust.exports.add_f32(REGION_A, REGION_B, REGION_C, n);

        writeF32(zig.memory, REGION_A, edges);
        writeF32(zig.memory, REGION_B, b);
        zig.exports.add_f32(REGION_A, REGION_B, REGION_D, n);

        const zigOut = readF32(zig.memory, REGION_D, n);
        writeF32(rust.memory, REGION_D, zigOut);
        report(`add_f32 adversarial`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, n));

        // max_f32 with NaN inputs
        const nanArr = new Float32Array([NaN, 1.0, NaN, NaN, 0, -0, Infinity, -Infinity]);
        const nanB = new Float32Array([1.0, NaN, NaN, 0, NaN, 0, -Infinity, Infinity]);
        writeF32(rust.memory, REGION_A, nanArr);
        writeF32(rust.memory, REGION_B, nanB);
        rust.exports.max_f32(REGION_A, REGION_B, REGION_C, nanArr.length);

        writeF32(zig.memory, REGION_A, nanArr);
        writeF32(zig.memory, REGION_B, nanB);
        zig.exports.max_f32(REGION_A, REGION_B, REGION_D, nanArr.length);

        const zigMaxOut = readF32(zig.memory, REGION_D, nanArr.length);
        writeF32(rust.memory, REGION_D, zigMaxOut);
        report(`max_f32 NaN propagation`, compareBits(rust.memory, REGION_C, rust.memory, REGION_D, nanArr.length));
    }

    // -----------------------------------------------------------------------
    // Determinism test: same inputs 100 times, verify all outputs identical
    // -----------------------------------------------------------------------
    console.log('--- determinism (100 runs) ---');
    {
        const n = 64;
        const a = randArr(n);
        const b = randArr(n);

        // Get reference output
        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        rust.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, 8, 8, 8);
        const refBits = new Uint32Array(rust.memory.buffer, REGION_C, n).slice();

        let allMatch = true;
        for (let run = 0; run < 100; run++) {
            writeF32(rust.memory, REGION_A, a);
            writeF32(rust.memory, REGION_B, b);
            rust.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, 8, 8, 8);
            const bits = new Uint32Array(rust.memory.buffer, REGION_C, n);
            for (let i = 0; i < n; i++) {
                if (bits[i] !== refBits[i]) {
                    allMatch = false;
                    break;
                }
            }
            if (!allMatch) break;
        }
        report(`matmul_f32_tiled deterministic x100 (Rust)`, { pass: allMatch, mismatches: allMatch ? [] : [{ index: -1, rustBits: 'varies', zigBits: 'N/A', rustFloat: NaN, zigFloat: NaN }] });

        // Same for Zig
        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        zig.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, 8, 8, 8);
        const refBitsZig = new Uint32Array(zig.memory.buffer, REGION_C, n).slice();

        let allMatchZig = true;
        for (let run = 0; run < 100; run++) {
            writeF32(zig.memory, REGION_A, a);
            writeF32(zig.memory, REGION_B, b);
            zig.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, 8, 8, 8);
            const bits = new Uint32Array(zig.memory.buffer, REGION_C, n);
            for (let i = 0; i < n; i++) {
                if (bits[i] !== refBitsZig[i]) {
                    allMatchZig = false;
                    break;
                }
            }
            if (!allMatchZig) break;
        }
        report(`matmul_f32_tiled deterministic x100 (Zig)`, { pass: allMatchZig, mismatches: allMatchZig ? [] : [{ index: -1, rustBits: 'N/A', zigBits: 'varies', rustFloat: NaN, zigFloat: NaN }] });
    }

    // -----------------------------------------------------------------------
    // Summary
    // -----------------------------------------------------------------------
    console.log('');
    console.log(`=== RESULTS: ${passedTests}/${totalTests} passed, ${failedTests} failed ===`);
    process.exit(failedTests > 0 ? 1 : 0);
}

main().catch(err => {
    console.error('Fatal error:', err);
    process.exit(2);
});
