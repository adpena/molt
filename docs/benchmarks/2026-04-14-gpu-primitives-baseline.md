# GPU Primitives Baseline Benchmarks — 2026-04-14

Host: darwin arm64 (Apple Silicon), CPU backend (reference implementation).

## Fusion Benchmarks

Warmup: 5 iters, Measurement: 100 iters.

| Composition | Unfused Kernels | Fused Kernels | Unfused Render (us) | Fused Render (us) | Speedup | Fusion Pass (us) |
|-------------|-----------------|---------------|--------------------:|------------------:|--------:|-----------------:|
| softmax_1024 | 6 | 2 | 4.13 | 2.30 | 1.79x | 3.55 |
| elem_chain_4x_add | 4 | 1 | 3.29 | 2.15 | 1.53x | 2.50 |

## Individual Op Benchmarks (1M elements)

| Operation | Elements | Avg (us) | GFLOPS | BW (GB/s) |
|-----------|----------|----------|--------|-----------|
| memcpy_baseline | 1000000 | 327.47 | 0.000 | 24.430 |
| add_f32 | 1000000 | 827.75 | 1.208 | 14.497 |
| mul_f32 | 1000000 | 704.10 | 1.420 | 17.043 |
| exp2_f32 | 1000000 | 1903.26 | 0.525 | 4.203 |
| sqrt_f32 | 1000000 | 733.50 | 1.363 | 10.907 |
| reduce_sum_f32 | 100000 | 47.25 | 2.116 | 8.466 |

## Composition Benchmarks

| Operation | Elements | Avg (us) | GFLOPS | BW (GB/s) |
|-----------|----------|----------|--------|-----------|
| softmax_f32 | 100000 | 232.64 | 2.579 | 10.316 |
| matmul_64x64 | 4096 | 110.55 | 4.742 | 0.445 |
| matmul_256x256 | 65536 | 9557.68 | 3.511 | 0.082 |
| rmsnorm_f32 | 100000 | 69.88 | 5.724 | 17.172 |

## Test Summary

- **207 unit tests**: all passing
- **WASM target**: `cargo check --target wasm32-unknown-unknown` passes (CPU backend)
- **Clippy**: clean (1 dead-code warning in `CompiledProgram.handle`)
- **Benchmark profiles**: `bench` (release with debuginfo)
