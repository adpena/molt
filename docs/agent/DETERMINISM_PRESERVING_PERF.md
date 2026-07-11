# Determinism-Preserving Performance

This document is the canonical rule for accelerating parity-gated floating-point
compute paths. Performance work may change where independent work runs, but it
must not change the ordered floating-point operation sequence that produces a
parity-sensitive output unless a contract-owned margins gate explicitly admits
the change.

## Numeric basis

IEEE 754 makes the result depend on the input values, destination format, and
operation sequence. Reassociation therefore changes the program, not merely its
schedule. A fused multiply-add rounds once, while separate multiply and add
operations round twice. The WebAssembly core specification defines ordinary
floating-point arithmetic from IEEE 754 and states that relaxed multiply-add may
produce either fused or unfused results, making its rounding implementation
dependent.

Primary sources:

- IEEE 754-2019 overview: https://standards.ieee.org/ieee/754/6210/
- WebAssembly core numeric semantics: https://www.w3.org/TR/wasm-core/
- Clang floating-point controls: https://clang.llvm.org/docs/UsersManual.html
- GCC floating-point contraction controls: https://gcc.gnu.org/onlinedocs/gcc/Optimize-Options.html
- SciPy Gaussian weights and axis-by-axis filter: https://github.com/scipy/scipy/blob/main/scipy/ndimage/_filters.py
- SciPy serial `NI_Correlate1D`: https://github.com/scipy/scipy/blob/main/scipy/ndimage/src/ni_filters.c

## BIT-SAFE optimization classes

An optimization is BIT-SAFE only when every parity-gated output observes the
same typed operands and the same ordered primitive operations.

| Class | Rule | Why it is safe |
|---|---|---|
| Across-output parallelism | Run independent output elements concurrently. SIMD lanes may hold different pixels while each lane executes taps in the original serial order. | No output's dependency chain changes. SciPy's line correlation is serial within one pixel but pixels are independent. |
| Order-preserving tiling | Tile lines, rows, planes, or output blocks without changing each output's tap or reduction order. | Address order and cache residency change; arithmetic order does not. |
| Deterministic SIMD | Use `simd128` lane-wise add/multiply with no horizontal reduction or relaxed operation. | Each lane has ordinary IEEE-defined operations in the same sequence as the scalar output. |
| Exact representation changes | Change layout, indexing, dead code, scheduling, or integer address arithmetic when the floating operands and operations are unchanged. | The numeric dataflow is identical. |
| Proven exact strength reduction | Apply only when a domain proof covers NaNs, infinities, signed zero, exceptions, and every result bit. | Algebra alone is insufficient. Removing `x + 0` can change `-0`; removing `x * 1` can change signaling-NaN behavior or payload propagation. Treat these rewrites as unsafe without a domain-specific bit proof. |

## BIT-UNSAFE optimization classes

These are forbidden on parity-gated paths unless the pact/operator-owned
contract includes and passes a margins gate for the changed observable.

| Class | Forbidden change |
|---|---|
| Reduction reassociation | Serial sum to tree, pairwise, blocked, horizontal SIMD, warp, or workgroup reduction. |
| Contraction | FMA generation, `-ffp-contract=on/fast`, relaxed-SIMD fused multiply-add, or equivalent backend contraction. |
| Fast math | `-ffast-math`, reassociation, reciprocal approximation, no-signed-zero, finite-only, approximate functions, or unsafe-math bundles. |
| Libm substitution | Replacing the sealed stack's `exp`, `sin`, `cos`, `tanh`, `pow`, or other libm implementation without a range-specific proof. |
| Precision changes | Changing accumulator/intermediate width, early casts, excess precision, flush-to-zero behavior, or denormal handling. |
| Algorithm substitution | Different BLAS/LAPACK kernels, convolution algorithms, approximate EDT/JFA, or any numerically different implementation. |

## Kernel A binding constraint

`docs/agent/E1_PARITY_FEASIBILITY.md` proves that Kernel A is achievable as
constructed and identifies one sharp constraint: `m_smooth` must be bit
identical because the keep-120 `crit_min` cut crosses a 630-pixel exact-tie
group. Any accelerated `gaussian_filter` must preserve SciPy's serial
per-pixel accumulation rounding.

The safe acceleration lever is output-lane SIMD. For a symmetric radius-eight
kernel, lane `n` still evaluates center multiply followed by tap pairs
`0..7`, each as separate add, multiply, and accumulator add. Lanes contain
different pixels; there is no horizontal reduction.

## Enforced toolchain contract

Molt previously relied on compiler defaults for sealed upstream source
extensions. That was insufficient: current Clang defaults permit contraction
within an expression, and GCC defaults vary by language mode. The shared
wasm32 source-extension compile authority now appends both:

- `-fno-fast-math`
- `-ffp-contract=off`

`tools/determinism_perf_gate.py` attests those flags and the target-feature
manifest contract:

- `wasm.simd128` remains `deterministic`.
- `wasm.relaxed_simd` remains `non_bit_exact`.
- every wasm target keeps relaxed SIMD behind
  `explicit_non_bit_exact_profile`.

Teeth:

```powershell
python tools/determinism_perf_gate.py
python tools/determinism_perf_gate.py --probe-unsafe-flag=-ffp-contract=fast
python tools/determinism_perf_gate.py --probe-unsafe-flag=-ffast-math
```

The first command must pass. Each probe command must fail. The probes append an
unsafe flag to the same compile-argument attestation used by the gate; they are
not source-text mocks.

## First reference implementation and attestation

`tools/native/deterministic_correlate1d.c` implements scalar and SSE2 128-bit
output-lane kernels. `tools/benchmark_deterministic_gaussian.py` applies both
axis passes to the real Kernel A `m12` field from the parity microscope and
requires bit identity against SciPy before timing.

Attestation on AMD Ryzen 9 3900X, Clang 22.1.7, NumPy 2.5.1, SciPy 1.18.0,
Windows x86-64, median of 21:

| Path | Median |
|---|---:|
| Scalar fixed-order reference | 3.1745 ms |
| SSE2 across two output pixels | 2.7598 ms |
| Speedup | **1.1503x** |

Both results are bit-identical to SciPy `gaussian_filter` over all 196,608
float32 outputs. The machine-readable evidence is
`docs/agent/evidence/determinism_perf/gaussian_sse2_attestation.json`.

This is a reference implementation, not a Molt-owned SciPy replacement. The
integration follow-on is to route the technique through upstream SciPy source
recompilation or an existing native-callable substitution whose ABI and package
custody are already authoritative. Admission requires the same bit proof on the
wasm object, target-feature attestation, and a before/after witness-loop
measurement. Copying SciPy semantics into a shipped Molt package is forbidden.

## Review checklist

1. Identify every parity-gated output downstream of the optimized operation.
2. Draw the old and new per-output operation sequence.
3. Reject contraction, reassociation, libm, precision, and algorithm drift by default.
4. Run the determinism gate and its unsafe probes.
5. Run bitwise comparison on the real fixture before timing.
6. Record median-of-N before/after evidence on the same machine.
7. Escalate any margins-based contract decision to the pact/operator owner.
