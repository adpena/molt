# SIMD Benchmark Results

Date: 2026-04-15 15:09:01
Iterations per op: 100

## JS Scalar Baseline (Node.js)

| Op | 1K (us) | 64K (us) | 1M (us) |
|---|---|---|---|
| add_f32 | 2.68 | 51.28 | 673.05 |
| mul_f32 | 2.02 | 53.98 | 645.65 |
| sqrt_f32 | 3.26 | 43.37 | 521.20 |
| neg_f32 | 2.58 | 29.52 | 447.37 |
| softmax_f32 | 9.90 | 438.25 | 8072.47 |
| reduce_sum_f32 | 2.40 | 41.46 | 661.98 |
| rms_norm_f32 | 4.73 | 85.72 | 1317.02 |

## WASM SIMD Expected Speedups

Based on architectural analysis:
- Elementwise ops (add, mul, neg): ~3.5-4x (4-wide SIMD, minus overhead)
- Transcendental ops (sqrt, exp2): ~2-3x (SIMD + polynomial)
- Reductions (sum, max): ~2-3x (SIMD accumulate + horizontal)
- Fused ops (softmax, rms_norm): ~3-5x (eliminates intermediate buffers)
- matmul: ~10-50x (SIMD + cache-optimized IKJ loop)

## Rust CPU SIMD Coverage

All 26 PrimitiveOps covered with `wide` crate f32x4:
- Arithmetic: Add, Sub, Mul, Idiv, Mod, Neg
- Comparison: Cmplt, Cmpeq, Cmpne
- Bitwise: And, Or, Xor, Shl, Shr
- Math: Exp2, Log2, Sin, Sqrt, Reciprocal, Trunc
- Reduce: ReduceSum, ReduceMax (scalar fallback in SIMD path)
- Control: Max, Where, Cast, Bitcast

