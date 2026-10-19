// Independent numerical and real browser-consumer oracles for source-built SIMD.
// node tests/e2e/simd_diff_harness.js [artifact.wasm]
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { pathToFileURL } = require('node:url');
const path = require('node:path');
const { DEFAULT_WASM, loadSimd, xorshift32, randArr } = require('./simd_test_support');
const f = Math.fround;
const MIN_SUBNORMAL = 2 ** -149;
function exact(actual, expected) {
    assert.equal(actual.length, expected.length);
    for (let i = 0; i < expected.length; i++) {
        assert.ok(Object.is(actual[i], f(expected[i])), `element ${i}: ${actual[i]} != ${expected[i]}`);
    }
}
function close(actual, expected, relative = 1e-5, absolute = MIN_SUBNORMAL) {
    assert.equal(actual.length, expected.length);
    for (let i = 0; i < expected.length; i++) {
        const a = actual[i], b = expected[i];
        if (!Number.isFinite(b) || b === 0) assert.ok(Object.is(a, b), `special ${i}: ${a} != ${b}`);
        else assert.ok(Number.isFinite(a) && Math.abs(a - b) <= absolute + relative * Math.abs(b),
            `element ${i}: ${a} != ${b} (relative ${Math.abs((a - b) / b)})`);
    }
}
function matmulReference(a, b, m, k, n) {
    const out = new Float32Array(m * n);
    for (let i = 0; i < m; i++) for (let j = 0; j < n; j++) {
        let sum = 0;
        for (let p = 0; p < k; p++) sum = f(sum + f(a[i * k + p] * b[p * n + j]));
        out[i * n + j] = sum;
    }
    return out;
}
function softmaxReference(a) {
    const max = Math.max(...a);
    const exp = Array.from(a, x => Math.exp(x - max));
    const sum = exp.reduce((a, b) => a + b, 0);
    return Float32Array.from(exp, x => x / sum);
}
function rmsReference(a, w, eps) {
    const scale = 1 / Math.sqrt(Array.from(a).reduce((s, x) => s + x * x, 0) / a.length + eps);
    return Float32Array.from(a, (x, i) => x * w[i] * scale);
}
async function main() {
    const artifact = process.argv[2] || DEFAULT_WASM;
    const wasm = await loadSimd(artifact), e = wasm.exports, rng = xorshift32(42);
    const groups = new Map();
    async function test(group, name, run) {
        const cases = groups.get(group) || [];
        groups.set(group, cases);
        wasm.reset();
        try { await run(); cases.push({ name, passed: true }); }
        catch (error) { cases.push({ name, passed: false, error: error.stack }); console.error(`${group}/${name}: ${error.message}`); }
    }
    function unary(name, a) {
        const ptr = wasm.alloc(a), out = wasm.alloc(a.length);
        e[name](ptr, out, a.length); return wasm.read(out, a.length);
    }
    function binary(name, a, b) {
        const ap = wasm.alloc(a), bp = wasm.alloc(b), out = wasm.alloc(a.length);
        e[name](ap, bp, out, a.length); return wasm.read(out, a.length);
    }
    const sizes = [0, 1, 3, 4, 5, 16, 257, 4096];
    for (const n of sizes) {
        const a = randArr(rng, n), b = randArr(rng, n);
        for (const [name, op] of [['add_f32', (a, b) => f(a + b)], ['mul_f32', (a, b) => f(a * b)], ['max_f32', Math.max]]) {
            await test(name, `n=${n}`, () => exact(binary(name, a, b), Float32Array.from(a, (x, i) => op(x, b[i]))));
        }
        for (const [name, op] of [['neg_f32', x => -x], ['sqrt_f32', Math.sqrt], ['reciprocal_f32', x => 1 / x]]) {
            await test(name, `n=${n}`, () => exact(unary(name, a), Float32Array.from(a, op)));
        }
        await test('exp2_f32', `n=${n}`, () => close(unary('exp2_f32', a), Float32Array.from(a, x => 2 ** x)));
        await test('reduce_max_f32', `n=${n}`, () => exact([e.reduce_max_f32(wasm.alloc(a), n)], [Math.max(...a)]));
        await test('reduce_sum_f32', `integer n=${n}`, () => {
            const ints = Float32Array.from(a, x => Math.trunc(x * 8));
            exact([e.reduce_sum_f32(wasm.alloc(ints), n)], [Array.from(ints).reduce((s, x) => s + x, 0)]);
        });
        await test('softmax_f32_fused', `n=${n}`, () => close(unary('softmax_f32_fused', a), n ? softmaxReference(a) : [], 2e-5));
        await test('rms_norm_f32', `n=${n}`, () => {
            const ap = wasm.alloc(a), wp = wasm.alloc(b), out = wasm.alloc(n), eps = f(1e-5);
            e.rms_norm_f32(ap, wp, out, n, eps);
            close(wasm.read(out, n), n ? rmsReference(a, b, eps) : [], 2e-6);
        });
    }
    // Each value appears in vector bodies and scalar tails, including each lane.
    const edges = [NaN, -Infinity, Infinity, -0, 0, MIN_SUBNORMAL, -MIN_SUBNORMAL, -150, -149.9, -149, -127, -126, 127.99, 128, 129];
    for (let shift = 0; shift < 4; shift++) {
        const a = Float32Array.from([...edges.slice(shift), ...edges.slice(0, shift)]);
        await test('exp2_f32', `IEEE boundaries rotation=${shift}`, () => close(unary('exp2_f32', a), Float32Array.from(a, x => 2 ** x)));
        for (const [name, op] of [['neg_f32', x => -x], ['sqrt_f32', Math.sqrt], ['reciprocal_f32', x => 1 / x]]) {
            await test(name, `IEEE boundaries rotation=${shift}`, () => exact(unary(name, a), Float32Array.from(a, op)));
        }
    }
    for (const [name, op] of [['add_f32', (a, b) => a + b], ['mul_f32', (a, b) => a * b]]) {
        await test(name, 'IEEE values in vector bodies and tails', () => {
            const a = Float32Array.from(edges), b = Float32Array.from([...edges].reverse());
            exact(binary(name, a, b), Float32Array.from(a, (x, i) => op(x, b[i])));
        });
    }
    await test('exp2_f32', 'normal and subnormal domain grid', () => {
        const a = Float32Array.from({ length: 32769 }, (_, i) => -150 + i * 278 / 32768);
        close(unary('exp2_f32', a), Float32Array.from(a, x => 2 ** x));
    });
    await test('exp2_f32', 'half-integer and exponent boundaries', () => {
        const a = Float32Array.from([-149.5, -148.5, -127.5, -126.5, -125.5, -0.5, 0.5,
            125.5, 126.5, 127.5 - 2 ** -17, 127.5, 127.5 + 2 ** -17, 128 - 2 ** -17]);
        close(unary('exp2_f32', a), Float32Array.from(a, x => 2 ** x));
        for (const x of a) {
            wasm.reset();
            const scalar = unary('exp2_f32', Float32Array.of(x));
            exact(scalar, [unary('exp2_f32', Float32Array.of(x, NaN, 0, 0))[0]]);
        }
    });
    await test('exp2_f32', 'monotone adjacent floats at every integer and half-integer boundary', () => {
        function neighbor(x, direction) {
            if (x === 0) return direction * MIN_SUBNORMAL;
            const value = Float32Array.of(x), bits = new Uint32Array(value.buffer);
            bits[0] += x > 0 ? direction : -direction;
            return value[0];
        }
        for (let x = -150; x <= 128; x += 0.5) {
            wasm.reset();
            const a = Float32Array.of(neighbor(x, -1), x, neighbor(x, 1), neighbor(x, 1));
            const output = unary('exp2_f32', a);
            assert.ok(output[0] <= output[1] && output[1] <= output[2], `exp2 decreased at ${x}: ${[...output]}`);
            exact(unary('exp2_f32', a.subarray(0, 3)), output.subarray(0, 3));
        }
    });
    await test('exp2_f32', 'exact underflow and powers-of-two goldens', () => {
        const a = Float32Array.from([-Infinity, -150, -149.9, -149, -148, -127, -126, -MIN_SUBNORMAL, 0, MIN_SUBNORMAL, 1, 127, 128, Infinity, NaN]);
        exact(unary('exp2_f32', a), [0, 0, MIN_SUBNORMAL, MIN_SUBNORMAL, 2 * MIN_SUBNORMAL, 2 ** -127, 2 ** -126, 1, 1, 1, 2, 2 ** 127, Infinity, Infinity, NaN]);
    });
    for (const x of [-149, -127.5, -126, -0.25, 0.5, 126.5, 127]) {
        await test('exp2_f32', `same bits in scalar/normal/mixed lanes x=${x}`, () => {
            const scalar = unary('exp2_f32', Float32Array.of(x));
            for (const peers of [[0, 0, 0], [NaN, 0, 0]]) {
                const vector = unary('exp2_f32', Float32Array.of(x, ...peers));
                assert.equal(new Uint32Array(vector.buffer)[0], new Uint32Array(scalar.buffer)[0]);
            }
        });
    }
    await test('softmax_f32_fused', 'finite shifted, monotone and wide rows', () => {
        const inputs = [
            randArr(rng, 1024, -20, 20),
            Float32Array.from({length:1024}, (_,i) => i * 0.001),
            Float32Array.from({length:1024}, (_,i) => i * 0.01),
            Float32Array.from({length:1024}, (_,i) => (1023 - i) * 0.01),
            Float32Array.from({length: 257}, (_, i) => -1000 + i * 2000 / 256),
            Float32Array.from({length: 17}, (_, i) => i % 2 ? 100 : -100),
        ];
        for (const a of inputs) {
            for (const shift of [-1000, 0, 1000]) {
                wasm.reset();
                const shifted = Float32Array.from(a, x => x + shift);
                close(unary('softmax_f32_fused', shifted), softmaxReference(shifted), 2e-5);
            }
        }
    });
    for (const a of [[1, Infinity], [Infinity, 1, -1, 0, 2], [-Infinity], [NaN, 0], [-Infinity, -Infinity, -Infinity, -Infinity], [-Infinity, 1, 0, 2, -Infinity]]) {
        await test('softmax_f32_fused', `IEEE row ${a}`, () => close(unary('softmax_f32_fused', Float32Array.from(a)), softmaxReference(a), 2e-5));
    }
    for (let position = 0; position < 9; position++) {
        await test('max_f32', `NaN body/tail ${position}`, () => {
            const a = new Float32Array(9).fill(-0), b = new Float32Array(9).fill(0);
            new Uint32Array(a.buffer)[position] = 0xffc12345;
            const result = binary('max_f32', a, b);
            assert.equal(new Uint32Array(result.buffer)[position], 0x7fc00000);
            exact(binary('max_f32', a, b), Float32Array.from(a, (x, i) => Math.max(x, b[i])));
            exact(binary('max_f32', b, a), Float32Array.from(a, (x, i) => Math.max(b[i], x)));
        });
        await test('reduce_max_f32', `NaN body/tail ${position}`, () => {
            const a = new Float32Array(9).fill(1); a[position] = NaN;
            exact([e.reduce_max_f32(wasm.alloc(a), a.length)], [NaN]);
        });
    }
    for (const [a, expected] of [[[-0, -0, -0, -0, -0], -0], [[-0, -0, 0, -0, -0], 0], [[], -Infinity]]) {
        await test('reduce_max_f32', `signed zero/empty ${a.map(x => Object.is(x, -0) ? '-0' : x)}`, () => exact([e.reduce_max_f32(wasm.alloc(a), a.length)], [expected]));
    }
    // Goldens distinguish the specified pairwise order from the deleted left fold.
    for (const [a, expected] of [ [[1e20, 1, -1e20, 1], 2], [Array(4).fill([1e20, 1, -1e20, 1]).flat(), 8], [[1e20, 1, -1e20, 1, 3], 5] ]) {
        await test('reduce_sum_f32', `cancellation n=${a.length}`, () => exact([e.reduce_sum_f32(wasm.alloc(a), a.length)], [expected]));
    }
    for (const [m, k, n] of [[0, 3, 4], [1, 1, 1], [4, 4, 4], [5, 7, 9], [8, 0, 8], [16, 16, 16], [64, 64, 64]]) {
        await test('matmul_f32_tiled', `${m}x${k}x${n}`, () => {
            const a = randArr(rng, m * k), b = randArr(rng, k * n);
            const ap = wasm.alloc(a), bp = wasm.alloc(b), out = wasm.alloc(m * n);
            e.matmul_f32_tiled(ap, bp, out, m, k, n);
            exact(wasm.read(out, m * n), matmulReference(a, b, m, k, n));
        });
    }
    for (const n of [0, 2, 6, 16, 256]) {
        await test('rope_f32', `pairs n=${n}`, () => {
            const a = randArr(rng, n), c = randArr(rng, n / 2), s = randArr(rng, n / 2);
            const ap = wasm.alloc(a), cp = wasm.alloc(c), sp = wasm.alloc(s), out = wasm.alloc(n);
            e.rope_f32(ap, cp, sp, out, n);
            exact(wasm.read(out, n), Float32Array.from(a, (_, i) => i % 2
                ? f(f(a[i - 1] * s[i >> 1]) + f(a[i] * c[i >> 1]))
                : f(f(a[i] * c[i >> 1]) - f(a[i + 1] * s[i >> 1]))));
        });
    }
    await test('determinism', 'repeated special vectors and reductions', () => {
        const a = Float32Array.from(edges);
        const expected = unary('exp2_f32', a);
        for (let i = 0; i < 100; i++) {
            wasm.reset();
            assert.deepEqual(new Uint32Array(unary('exp2_f32', a).buffer), new Uint32Array(expected.buffer));
        }
    });
    const { ComputeEngine } = await import(pathToFileURL(path.resolve(__dirname, '../../deploy/browser/compute-engine.js')).href);
    let fetches = 0;
    globalThis.fetch = async url => {
        assert.equal(url, 'test://source-built-simd'); fetches++;
        const content = fs.readFileSync(artifact);
        return { ok: true, arrayBuffer: async () => content.buffer.slice(content.byteOffset, content.byteOffset + content.byteLength) };
    };
    const engine = await ComputeEngine.create({ forceBackend: 'wasm-simd', wasmUrl: 'test://source-built-simd' });
    await test('browser_consumer', 'one source artifact and persistent result copies', async () => {
        assert.equal(engine.backendName, 'wasm-simd'); assert.equal(fetches, 1);
        const a = randArr(rng, 300001), b = randArr(rng, a.length);
        const originalGrow = WebAssembly.Memory.prototype.grow;
        let growths = 0, sum;
        WebAssembly.Memory.prototype.grow = function(pages) { growths++; return originalGrow.call(this, pages); };
        try { sum = await engine.add(a, b, a.length); }
        finally { WebAssembly.Memory.prototype.grow = originalGrow; }
        assert.equal(growths, 1, 'all call buffers must reserve with one memory growth');
        exact(sum, Float32Array.from(a, (x, i) => f(x + b[i])));
        exact(await engine.mul(a, b, a.length), Float32Array.from(a, (x, i) => f(x * b[i])));
        exact(sum, Float32Array.from(a, (x, i) => f(x + b[i])));
    });
    await test('browser_consumer', 'tiled matmul and batches', async () => {
        const a = randArr(rng, 5 * 7), b = randArr(rng, 7 * 9), expected = matmulReference(a, b, 5, 7, 9);
        exact(await engine.matmul(a, b, 5, 7, 9), expected);
        const batch = await engine.matmulBatch([{ a, b, m: 5, k: 7, n: 9 }, { a, b, m: 5, k: 7, n: 9 }]);
        batch.forEach(x => exact(x, expected));
    });
    await test('browser_consumer', 'multirow softmax and RMSNorm', async () => {
        const n = 7, rows = 3, a = randArr(rng, n * rows), w = randArr(rng, n);
        const softmax = await engine.softmax(a, n, rows), rms = await engine.rmsNorm(a, w, n, rows, f(1e-5));
        for (let r = 0; r < rows; r++) {
            close(softmax.subarray(r * n, (r + 1) * n), softmaxReference(a.subarray(r * n, (r + 1) * n)), 2e-5);
            close(rms.subarray(r * n, (r + 1) * n), rmsReference(a.subarray(r * n, (r + 1) * n), w, f(1e-5)), 2e-6);
        }
    });
    await test('browser_consumer', 'nonfinite softmax rows follow the scalar oracle', async () => {
        const a = Float32Array.from([1, Infinity, 0, -Infinity, -Infinity, -Infinity]);
        const output = await engine.softmax(a, 3, 2);
        close(output.subarray(0, 3), softmaxReference(a.subarray(0, 3)));
        close(output.subarray(3, 6), softmaxReference(a.subarray(3, 6)));
    });
    await test('browser_consumer', 'empty dimensions do no work regardless of row count', async () => {
        const empty = new Float32Array();
        exact(await engine.softmax(empty, 0, 0xffffffff), []);
        exact(await engine.rmsNorm(empty, empty, 0, 0xffffffff), []);
        exact(await engine.matmul(empty, empty, 0xffffffff, 0, 0), []);
    });
    await test('browser_consumer', 'unsafe shape rejection before entering WASM', async () => {
        for (const size of [-1, 1.5, 2 ** 32, 2 ** 53]) await assert.rejects(engine.add(new Float32Array(), new Float32Array(), size), RangeError);
        await assert.rejects(engine.matmul(new Float32Array(1), new Float32Array(4), 2, 2, 2), RangeError);
        await assert.rejects(engine.softmax(new Float32Array(3), 2, 2), RangeError);
        await assert.rejects(engine.rmsNorm(new Float32Array(4), new Float32Array(1), 2, 2), RangeError);
    });
    const report = Array.from(groups, ([name, cases]) => ({ name, cases, failed: cases.filter(x => !x.passed).length }));
    const all = report.flatMap(x => x.cases), failed = all.filter(x => !x.passed).length;
    console.log(`@@SIMD_DIFF_JSON@@ ${JSON.stringify({ version: 2, groups: report, summary: { total: all.length, passed: all.length - failed, failed } })}`);
    process.exitCode = failed ? 1 : 0;
}
main().catch(error => { console.error(error.stack); process.exitCode = 2; });
