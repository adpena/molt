# Browser SIMD measurements

Scope: standalone browser inference kernels, advisory macOS arm64 Node/V8
measurements on 2026-10-19. This is not release performance acceptance.
The [source/ABI contract](../design/browser_simd.md) owns the implementation,
numerical tolerances and proof procedure.

## Reproduce

```bash
uv run --python 3.12 python tools/build_browser_simd.py
node tests/e2e/bench_simd.js deploy/browser/simd-ops.wasm
uv run --python 3.12 python -m pytest -q tests/e2e/test_simd_differential.py
```

The benchmark accepts any explicit candidate artifact and emits
`@@SIMD_BENCH_JSON@@` with per-operation timings and binary size. Inputs use a
fixed PRNG seed. Setup and JS input/output copies are outside kernel timings;
matmul64 uses 50 warmups/5,000 iterations, other operations 100/10,000. The owning
numerical test builds once and checks the published bytes before execution.

## Consolidation and correctness cost

Pinned Rust 1.99.0, wasm32-unknown-unknown, release opt-level=z, Node 26.11.1.
The baseline is a fresh build of the Rust crate at bb0fd253bfbc; its incorrect
subnormal and special-value exp2 behavior is retained as a failing oracle control.
Three alternating baseline/candidate samples use the same benchmark and memory
arena address. Median ns/op:

| Operation | Baseline | Corrected |
|---|---:|---:|
| matmul_f32_tiled 64x64 | 11858 | 11933 |
| matmul_f32_tiled 16x16 | 211 | 209 |
| softmax_f32_fused 1024 | 1769 | 1986 |
| add_f32 4096 | 312 | 314 |
| exp2_f32 1024 | 310 | 388 |
| rms_norm_f32 256 | 102 | 103 |
| reduce_sum_f32 4096 | 523 | 520 |

The required exp2 range classification costs about 78 ns per 1,024 elements in
this probe; softmax's range checks and consistent nonfinite-row policy cost about
217 ns per 1,024 elements. These are explicit correctness costs, not speedups.
Tiled matmul, arithmetic and the other reductions remain within this probe's
noise. Exceptional vectors share out-of-line handling: the first corrected
implementation was 8,020 bytes and measured exp2 ~427 ns/softmax ~2,531 ns;
the consolidated implementation is 5,524 bytes. This is not an exhaustive accuracy
or platform benchmark and does not establish optimality.

The two old shipped binaries totaled 8,987 bytes (Rust 3,508, Zig 5,479). One
source-built artifact now totals 5,524 bytes, 38.5% less. Its SHA-256 is
`5b7513fb546b9a086f54fef6aeefeb49d477be31058244c47eb1fbcb5450f64d`.
The numerical fixes add 812 bytes over the previously fresh Rust build (4,712),
while retiring the entire duplicate language, binary, build and loader path.
A matched initial Rust/Zig probe found approximately 1% differences on the seven
workloads and only 259 bytes between fresh modules; it supplied no distinct
capability that justified retaining both.

Users fetch and instantiate one module. Development builds, ABI validation,
numerical oracles and artifact synchronization add no checks to inference loops.
Real browser/OS/architecture and end-to-end OCR qualification remain with the
release matrices and V1 ledger; the local artifact does not attest remote R2 assets.
