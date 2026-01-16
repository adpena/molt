# Loop Optimization & Vectorization

## Goals
- Treat loops as first-class native constructs (counted loops, induction variables, and guardable fast paths).
- Prefer vectorized kernels for numeric reductions and elementwise operations.
- Preserve Python semantics via guard + fallback, not by weakening behavior.
- Make tight loops the primary performance objective for Molt.

## Canonical Loop Form
Molt lowers supported `for` loops into a counted loop form:
- Establish an induction variable (`i`) and loop bound (`len`).
- Use native loop blocks with block parameters for the index (phi-like SSA).
- Hoist length and guard checks outside the loop body when safe.

This canonical form enables downstream optimization (bounds check hoisting, constant folding, and vectorization).

## Vectorizable Regions
A loop is considered vectorizable when:
- It is countable (induction variable + bounds known at runtime).
- The body is pure with respect to loop-carried dependencies (e.g., `acc += a[i]`).
- The data source is contiguous and stable during the loop (list/tuple views).
- Indexed reductions over `range(len(xs))` patterns are eligible when the subscript is `xs[i]`.
- Non-zero range starts (`range(k, len(xs))`, `k >= 0`) are eligible via range-aware kernels.
- Conditional min/max reductions (`if x < acc: acc = x` or `if x > acc: acc = x`) are eligible for vectorization.
- Type facts may assert homogeneous element types to enable trusted kernels without per-element guards.

Vectorization emits a fast path that uses a SIMD kernel with a guard. If any guard fails, Molt executes the scalar fallback to preserve semantics.
For architecture-specific dispatch and SIMD policy, see `docs/spec/0512_ARCH_OPTIMIZATION_AND_SIMD.md`.

## SIMD Kernels (Tier 1)
Initial kernels target:
- Integer reductions (`sum`, `prod`, `min`, `max`) over list/tuple of ints.
- Byte/string scans (`find`, `count`) using optimized search routines.
- Elementwise arithmetic on homogeneous containers (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): float + int mix kernels).
- Trusted integer reductions (`VecSumIntTrusted`, `VecProdIntTrusted`, `VecMinIntTrusted`, `VecMaxIntTrusted`) that skip per-element checks when type facts are trusted.
- Range-aware reductions (`Vec*IntRange`, `Vec*IntRangeTrusted`) for reductions that skip the first `k` elements.

Each kernel returns a `(result, ok)` tuple. `ok == false` triggers fallback.

## Guard & Deopt Strategy
- Guards validate container type, element type, and loop invariants.
- Fallback executes the canonical scalar loop.
- Profiling hooks should promote hot loops into vectorizable forms (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): profiling-driven vectorization).

## Benchmarks & Targets
- Tight loops: >300x over CPython for integer reductions.
- Memory bandwidth: approach `memcpy` throughput on contiguous byte ops.
- Regression gates in `tools/bench.py` for vectorization-sensitive workloads.
