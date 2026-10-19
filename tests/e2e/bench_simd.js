// Matched source-built browser SIMD hot-kernel benchmark (no JS copy timings).
// node --no-liftoff tests/e2e/bench_simd.js [artifact.wasm]
'use strict';
const fs = require('node:fs');
const { performance } = require('node:perf_hooks');
const { DEFAULT_WASM, loadSimd, xorshift32, randArr } = require('./simd_test_support');
function benchmark(name, fn, iterations = 10000, warmup = 100) {
    for (let i = 0; i < warmup; i++) fn();
    const start = performance.now();
    for (let i = 0; i < iterations; i++) fn();
    const elapsed = performance.now() - start;
    return { name, elapsed, iterations, nsPerOp: elapsed * 1e6 / iterations };
}
async function main() {
    const artifact = process.argv[2] || DEFAULT_WASM;
    const wasm = await loadSimd(artifact), e = wasm.exports, rng = xorshift32(42), results = [];
    for (const size of [64, 16]) {
        wasm.reset();
        const a = wasm.alloc(randArr(rng, size * size)), b = wasm.alloc(randArr(rng, size * size)), out = wasm.alloc(size * size);
        results.push(benchmark(`matmul_f32_tiled ${size}x${size}`, () => e.matmul_f32_tiled(a, b, out, size, size, size), size === 64 ? 5000 : 10000, size === 64 ? 50 : 100));
    }
    wasm.reset();
    let a = wasm.alloc(randArr(rng, 1024, -5, 5)), out = wasm.alloc(1024);
    results.push(benchmark('softmax_f32_fused 1024', () => e.softmax_f32_fused(a, out, 1024)));
    wasm.reset();
    a = wasm.alloc(randArr(rng, 4096));
    const b = wasm.alloc(randArr(rng, 4096)); out = wasm.alloc(4096);
    results.push(benchmark('add_f32 4096', () => e.add_f32(a, b, out, 4096)));
    wasm.reset();
    a = wasm.alloc(randArr(rng, 1024, -10, 10)); out = wasm.alloc(1024);
    results.push(benchmark('exp2_f32 1024', () => e.exp2_f32(a, out, 1024)));
    wasm.reset();
    a = wasm.alloc(randArr(rng, 256));
    const w = wasm.alloc(randArr(rng, 256, 0.5, 1.5)); out = wasm.alloc(256);
    results.push(benchmark('rms_norm_f32 256', () => e.rms_norm_f32(a, w, out, 256, 1e-5)));
    wasm.reset();
    a = wasm.alloc(randArr(rng, 4096));
    results.push(benchmark('reduce_sum_f32 4096', () => e.reduce_sum_f32(a, 4096)));
    for (const result of results) console.log(`${result.name}: ${result.nsPerOp.toFixed(0)} ns/op`);
    console.log(`@@SIMD_BENCH_JSON@@ ${JSON.stringify({ version: 2, runtime: { node: process.version, v8: process.versions.v8, platform: process.platform, arch: process.arch, flags: process.execArgv }, benchmarks: results, binaryBytes: fs.statSync(artifact).size })}`);
}
main().catch(error => { console.error(error); process.exitCode = 2; });
