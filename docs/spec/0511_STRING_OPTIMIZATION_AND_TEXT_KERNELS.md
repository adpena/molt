# String Optimization and Text Kernels
**Spec ID:** 0511
**Status:** Draft (performance roadmap)
**Owner:** runtime + backend
**Goal:** Make string-heavy workloads first-class with deterministic, SIMD-friendly kernels and correct Unicode semantics.

## 1. Scope
String operations must be fast by default without sacrificing correctness for Unicode. The runtime provides optimized intrinsics for:
- `find`, `split`, `replace`, `startswith`, `endswith`, `count`, `join`
- slicing/indexing with Unicode codepoint semantics
- byte-level fast paths when inputs are ASCII

## 2. Performance Strategy
- **ASCII fast path:** if all inputs are ASCII, use byte-wise kernels (`memchr`, `memmem`) and return byte indices.
- **Unicode fallback:** for non-ASCII inputs, use UTF-8 aware operations and map byte offsets to codepoint indices.
- **Vectorization:** hot kernels (find/split/replace) should use SIMD where available; fall back to portable implementations.
- **No hidden nondeterminism:** avoid randomized hashing or locale-sensitive behavior.

## 3. Runtime Intrinsics
- Provide dedicated intrinsics for string operations instead of routing through generic list/byte paths.
- Keep list materialization deterministic and avoid intermediate allocations when possible (builder APIs).
- Expose in WIT/ABI for WASM parity.

## 4. Benchmarks and Gates
- Maintain dedicated micro-benchmarks for `find`, `split`, and `replace` in `tests/benchmarks/`.
- CI should track Molt/CPython ratios and alert on regressions beyond the configured threshold.
- Prefer realistic text sizes (100KB–10MB) to capture cache effects.

## 5. Correctness Notes
- `str.find` returns codepoint indices, not byte offsets.
- Empty separator behavior must match CPython (ValueError for split; replace inserts between codepoints).
- All string outputs must remain valid UTF-8.
