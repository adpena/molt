# Browser SIMD measurements

Scope: standalone browser inference kernels, advisory macOS arm64 Node/V8
measurements on 2026-10-19. This is not release performance acceptance.
The [source/ABI contract](../design/browser_simd.md) owns the implementation,
numerical tolerances and proof procedure.

## Reproduce

```bash
uv run --python 3.12 python tools/build_browser_simd.py
node --no-liftoff tests/e2e/bench_simd.js deploy/browser/simd-ops.wasm
uv run --python 3.12 python -m pytest -q tests/e2e/test_simd_differential.py
```

The benchmark accepts any explicit candidate artifact and emits
`@@SIMD_BENCH_JSON@@` with per-operation timings, binary size and runtime identity.
Inputs use a fixed PRNG seed. Setup and JS input/output copies are outside kernel
timings; matmul64 uses 50 warmups/5,000 iterations, other operations 100/10,000.
`--no-liftoff` selects optimized V8 compilation before measurement; it is a
benchmark setting, not a product runtime requirement. The owning numerical test
builds once and checks the published bytes before execution.

## Consolidation, numerical correctness and kernel cost

Pinned Rust 1.99.0, wasm32-unknown-unknown, release opt-level=z, Node 26.11.1.
The original baseline is a fresh build of the Rust crate at bb0fd253bfbc; its
incorrect subnormal and special-value exp2 behavior is retained as a failing
oracle control. The intermediate candidate corrected exp2 classification while
retaining the degree-six polynomial and online softmax. The final implementation
uses a monotone degree-four polynomial and stable SIMD softmax. Three round-robin
samples per artifact use the same benchmark and memory arena address. Median ns/op:

| Operation | Original | Intermediate | Final |
|---|---:|---:|---:|
| matmul_f32_tiled 64x64 | 11517 | 11501 | 11552 |
| matmul_f32_tiled 16x16 | 204 | 204 | 208 |
| softmax_f32_fused 1024 | 1713 | 1955 | 976 |
| add_f32 4096 | 304 | 302 | 300 |
| exp2_f32 1024 | 300 | 379 | 320 |
| rms_norm_f32 256 | 78 | 81 | 81 |
| reduce_sum_f32 4096 | 510 | 515 | 507 |

Stable softmax is about 2.00 times faster than the intermediate candidate and
1.75 times faster than the original in this probe. A gradually increasing
1,024-element row exposed 0.35% relative error in the old online normalizer,
above the 2e-5 contract. The final kernel uses a maximum pass, a vector
exponent/store/sum pass with f64 lane accumulation, and output normalization.
One extra result read/write removes repeated scalar exponent evaluation and
rescaling error without allocating scratch storage. The regression oracle checks
increasing, decreasing and shifted rows against an independent stable reference.

The degree-four exp2 polynomial recovers most classification cost and reduces
arithmetic, while preserving the full-domain error bound and monotonic integer
boundaries. It costs about 20 ns per 1,024 elements over the incorrect original
in this probe. Other kernel timings differ by at most about 4%; these samples
cannot establish small regressions or speedups. This is not exhaustive platform
qualification and does not establish optimality.

The two old shipped binaries totaled 8,987 bytes (Rust 3,508, Zig 5,479). One
source-built artifact now totals 5,180 bytes, 42.4% less. Its SHA-256 is
`85d45e377e82a9cec63a29bb3d373ce926ffd1a7f3668a7a39bb8c8b5008fbfc`.
The numerical fixes add 468 bytes over the previously fresh Rust build (4,712),
while retiring the duplicate language, binary, build and loader path.
A matched initial Rust/Zig probe found approximately 1% differences on the seven
workloads and only 259 bytes between fresh modules; it supplied no distinct
capability that justified retaining both.

Users fetch and instantiate one module. Development builds, ABI validation,
numerical oracles and artifact synchronization add no checks to inference loops.
Real browser/OS/architecture and end-to-end OCR qualification remain with the
release matrices and V1 ledger; the local artifact does not attest remote R2 assets.
