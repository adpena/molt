// Test/benchmark support for the source-built browser SIMD ABI.
'use strict';
const fs = require('fs');
const path = require('path');
const DEFAULT_WASM = path.resolve(__dirname, '../../deploy/browser/simd-ops.wasm');
const KERNELS = [
    'add_f32', 'mul_f32', 'neg_f32', 'sqrt_f32', 'reciprocal_f32', 'max_f32',
    'exp2_f32', 'reduce_sum_f32', 'reduce_max_f32', 'softmax_f32_fused',
    'matmul_f32_tiled', 'rms_norm_f32', 'rope_f32',
];
async function loadSimd(wasmPath = DEFAULT_WASM) {
    const { instance } = await WebAssembly.instantiate(fs.readFileSync(wasmPath), {});
    const e = instance.exports;
    const missing = KERNELS.filter(name => typeof e[name] !== 'function');
    if (!(e.memory instanceof WebAssembly.Memory)) missing.push('memory');
    if (!(e.__heap_base instanceof WebAssembly.Global)) missing.push('__heap_base');
    if (missing.length) throw new Error(`Missing SIMD exports: ${missing.join(', ')}`);
    const base = e.__heap_base.value;
    if (!Number.isInteger(base) || base <= 0 || base % 16 !== 0) throw new Error('Invalid SIMD heap base');
    let next = base;
    return {
        exports: e, memory: e.memory, base,
        reset() { next = base; },
        alloc(values) {
            const count = typeof values === 'number' ? values : values.length;
            const ptr = next;
            next += Math.ceil(count * 4 / 16) * 16;
            if (next > e.memory.buffer.byteLength) e.memory.grow(Math.ceil((next - e.memory.buffer.byteLength) / 65536));
            if (typeof values !== 'number') new Float32Array(e.memory.buffer, ptr, count).set(values);
            return ptr;
        },
        read(ptr, count) { return new Float32Array(e.memory.buffer.slice(ptr, ptr + count * 4)); },
    };
}
function xorshift32(seed) {
    let s = seed >>> 0 || 1;
    return () => { s ^= s << 13; s ^= s >> 17; s ^= s << 5; return (s >>> 0) / 0x100000000; };
}
function randArr(rng, n, lo = -1, hi = 1) {
    return Float32Array.from({ length: n }, () => lo + rng() * (hi - lo));
}
module.exports = { DEFAULT_WASM, KERNELS, loadSimd, xorshift32, randArr };
