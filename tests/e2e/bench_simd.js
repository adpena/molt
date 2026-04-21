// Benchmark: Zig vs Rust WASM SIMD implementations.
//
// Profiles matmul, softmax, add, exp2, rms_norm, reduce_sum across
// both implementations with realistic sizes (Falcon-OCR inference shapes).
//
// Run: node tests/e2e/bench_simd.js

'use strict';

const fs = require('fs');
const path = require('path');
const { performance } = require('perf_hooks');

const ROOT = path.resolve(__dirname, '../..');
const RUST_WASM = path.join(ROOT, 'deploy/browser/simd-ops-rs/target/wasm32-unknown-unknown/release/simd_ops.wasm');
const ZIG_WASM = path.join(ROOT, 'deploy/browser/simd-ops-zig/simd.wasm');

async function loadModule(wasmPath) {
    const bytes = fs.readFileSync(wasmPath);
    const memory = new WebAssembly.Memory({ initial: 256 });
    let instance;
    try {
        ({ instance } = await WebAssembly.instantiate(bytes, { env: { memory } }));
    } catch {
        ({ instance } = await WebAssembly.instantiate(bytes, {}));
    }
    const mem = instance.exports.memory || memory;
    return { exports: instance.exports, memory: mem };
}

function writeF32(memory, offset, arr) {
    new Float32Array(memory.buffer, offset, arr.length).set(arr);
}

// Seeded PRNG for reproducible data.
function xorshift32(seed) {
    let s = seed >>> 0 || 1;
    return function () {
        s ^= s << 13; s ^= s >> 17; s ^= s << 5;
        return (s >>> 0) / 0x100000000;
    };
}

function randArr(rng, n, lo = -1, hi = 1) {
    const arr = new Float32Array(n);
    for (let i = 0; i < n; i++) arr[i] = lo + rng() * (hi - lo);
    return arr;
}

const REGION_A = 0x10000;
const REGION_B = 0x20000;
const REGION_C = 0x30000;
const REGION_E = 0x50000;

// Warm-up + measure pattern: run warmup iterations, then timed iterations.
function benchmark(name, fn, warmup = 100, iterations = 10000) {
    for (let i = 0; i < warmup; i++) fn();
    const start = performance.now();
    for (let i = 0; i < iterations; i++) fn();
    const elapsed = performance.now() - start;
    const opsPerSec = iterations / (elapsed / 1000);
    const nsPerOp = (elapsed * 1e6) / iterations;
    return { name, elapsed, iterations, opsPerSec, nsPerOp };
}

function printResult(r) {
    const opsStr = r.opsPerSec >= 1e6
        ? (r.opsPerSec / 1e6).toFixed(2) + 'M'
        : r.opsPerSec >= 1e3
        ? (r.opsPerSec / 1e3).toFixed(1) + 'K'
        : r.opsPerSec.toFixed(0);
    console.log(`  ${r.name.padEnd(40)} ${r.elapsed.toFixed(1).padStart(8)}ms  ${opsStr.padStart(10)} ops/s  ${r.nsPerOp.toFixed(0).padStart(8)} ns/op`);
}

async function main() {
    console.log('Loading WASM modules...\n');
    const rust = await loadModule(RUST_WASM);
    const zig = await loadModule(ZIG_WASM);
    const rng = xorshift32(42);

    const results = [];

    // -----------------------------------------------------------------------
    // Matmul 64x64 (simulates one layer's projection)
    // -----------------------------------------------------------------------
    console.log('=== matmul_f32_tiled 64x64 ===');
    {
        const m = 64, k = 64, n = 64;
        const a = randArr(rng, m * k);
        const b = randArr(rng, k * n);

        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        const rr = benchmark('Rust matmul_f32_tiled 64x64', () => {
            rust.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, m, k, n);
        }, 50, 5000);
        printResult(rr);
        results.push(rr);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        const zr = benchmark('Zig  matmul_f32_tiled 64x64', () => {
            zig.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, m, k, n);
        }, 50, 5000);
        printResult(zr);
        results.push(zr);
    }

    // -----------------------------------------------------------------------
    // Matmul 64x64 — FAST variant (4x16 register blocking)
    // -----------------------------------------------------------------------
    if (rust.exports.matmul_f32_fast) {
        console.log('=== matmul_f32_fast 64x64 ===');
        const m = 64, k = 64, n = 64;
        const a = randArr(rng, m * k);
        const b = randArr(rng, k * n);

        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        const rr = benchmark('Rust matmul_f32_fast 64x64', () => {
            rust.exports.matmul_f32_fast(REGION_A, REGION_B, REGION_C, m, k, n);
        }, 50, 5000);
        printResult(rr);
    }

    // -----------------------------------------------------------------------
    // Matmul 16x16 (small projection)
    // -----------------------------------------------------------------------
    console.log('=== matmul_f32_tiled 16x16 ===');
    {
        const m = 16, k = 16, n = 16;
        const a = randArr(rng, m * k);
        const b = randArr(rng, k * n);

        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        const rr = benchmark('Rust matmul_f32_tiled 16x16', () => {
            rust.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, m, k, n);
        });
        printResult(rr);
        results.push(rr);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        const zr = benchmark('Zig  matmul_f32_tiled 16x16', () => {
            zig.exports.matmul_f32_tiled(REGION_A, REGION_B, REGION_C, m, k, n);
        });
        printResult(zr);
        results.push(zr);
    }

    // -----------------------------------------------------------------------
    // Softmax 1024 elements (simulates one attention head)
    // -----------------------------------------------------------------------
    console.log('=== softmax_f32_fused 1024 ===');
    {
        const n = 1024;
        const a = randArr(rng, n, -5, 5);

        writeF32(rust.memory, REGION_A, a);
        const rr = benchmark('Rust softmax_f32_fused 1024', () => {
            rust.exports.softmax_f32_fused(REGION_A, REGION_C, n);
        });
        printResult(rr);
        results.push(rr);

        writeF32(zig.memory, REGION_A, a);
        const zr = benchmark('Zig  softmax_f32_fused 1024', () => {
            zig.exports.softmax_f32_fused(REGION_A, REGION_C, n);
        });
        printResult(zr);
        results.push(zr);
    }

    // -----------------------------------------------------------------------
    // Add 4096 elements (simulates residual connection)
    // -----------------------------------------------------------------------
    console.log('=== add_f32 4096 ===');
    {
        const n = 4096;
        const a = randArr(rng, n);
        const b = randArr(rng, n);

        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_B, b);
        const rr = benchmark('Rust add_f32 4096', () => {
            rust.exports.add_f32(REGION_A, REGION_B, REGION_C, n);
        });
        printResult(rr);
        results.push(rr);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_B, b);
        const zr = benchmark('Zig  add_f32 4096', () => {
            zig.exports.add_f32(REGION_A, REGION_B, REGION_C, n);
        });
        printResult(zr);
        results.push(zr);
    }

    // -----------------------------------------------------------------------
    // exp2 1024 elements
    // -----------------------------------------------------------------------
    console.log('=== exp2_f32 1024 ===');
    {
        const n = 1024;
        const a = randArr(rng, n, -10, 10);

        writeF32(rust.memory, REGION_A, a);
        const rr = benchmark('Rust exp2_f32 1024', () => {
            rust.exports.exp2_f32(REGION_A, REGION_C, n);
        });
        printResult(rr);
        results.push(rr);

        writeF32(zig.memory, REGION_A, a);
        const zr = benchmark('Zig  exp2_f32 1024', () => {
            zig.exports.exp2_f32(REGION_A, REGION_C, n);
        });
        printResult(zr);
        results.push(zr);
    }

    // -----------------------------------------------------------------------
    // rms_norm 256 elements
    // -----------------------------------------------------------------------
    console.log('=== rms_norm_f32 256 ===');
    {
        const n = 256;
        const a = randArr(rng, n);
        const w = randArr(rng, n, 0.5, 1.5);

        writeF32(rust.memory, REGION_A, a);
        writeF32(rust.memory, REGION_E, w);
        const rr = benchmark('Rust rms_norm_f32 256', () => {
            rust.exports.rms_norm_f32(REGION_A, REGION_E, REGION_C, n, 1e-5);
        });
        printResult(rr);
        results.push(rr);

        writeF32(zig.memory, REGION_A, a);
        writeF32(zig.memory, REGION_E, w);
        const zr = benchmark('Zig  rms_norm_f32 256', () => {
            zig.exports.rms_norm_f32(REGION_A, REGION_E, REGION_C, n, 1e-5);
        });
        printResult(zr);
        results.push(zr);
    }

    // -----------------------------------------------------------------------
    // reduce_sum 4096 elements
    // -----------------------------------------------------------------------
    console.log('=== reduce_sum_f32 4096 ===');
    {
        const n = 4096;
        const a = randArr(rng, n);

        writeF32(rust.memory, REGION_A, a);
        const rr = benchmark('Rust reduce_sum_f32 4096', () => {
            rust.exports.reduce_sum_f32(REGION_A, n);
        });
        printResult(rr);
        results.push(rr);

        writeF32(zig.memory, REGION_A, a);
        const zr = benchmark('Zig  reduce_sum_f32 4096', () => {
            zig.exports.reduce_sum_f32(REGION_A, n);
        });
        printResult(zr);
        results.push(zr);
    }

    // -----------------------------------------------------------------------
    // Summary: compare Rust vs Zig for each pair
    // -----------------------------------------------------------------------
    console.log('\n=== SUMMARY ===');
    console.log('Operation'.padEnd(40) + 'Rust ns/op'.padStart(12) + 'Zig ns/op'.padStart(12) + 'Winner'.padStart(10) + 'Speedup'.padStart(10));
    console.log('-'.repeat(84));
    for (let i = 0; i < results.length; i += 2) {
        const r = results[i];
        const z = results[i + 1];
        const opName = r.name.replace(/^Rust\s+/, '');
        const winner = r.nsPerOp < z.nsPerOp ? 'Rust' : 'Zig';
        const speedup = r.nsPerOp < z.nsPerOp
            ? (z.nsPerOp / r.nsPerOp).toFixed(2) + 'x'
            : (r.nsPerOp / z.nsPerOp).toFixed(2) + 'x';
        console.log(
            opName.padEnd(40) +
            r.nsPerOp.toFixed(0).padStart(12) +
            z.nsPerOp.toFixed(0).padStart(12) +
            winner.padStart(10) +
            speedup.padStart(10)
        );
    }

    // Binary size comparison
    console.log('\n=== BINARY SIZE ===');
    const rustSize = fs.statSync(RUST_WASM).size;
    const zigSize = fs.statSync(ZIG_WASM).size;
    console.log(`  Rust: ${(rustSize / 1024).toFixed(1)} KB`);
    console.log(`  Zig:  ${(zigSize / 1024).toFixed(1)} KB`);
    console.log(`  Ratio: Zig is ${(rustSize / zigSize).toFixed(1)}x smaller`);
}

main().catch(err => {
    console.error('Fatal error:', err);
    process.exit(2);
});
