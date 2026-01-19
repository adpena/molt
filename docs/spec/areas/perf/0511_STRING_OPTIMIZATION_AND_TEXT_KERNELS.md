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
- **Unicode-aware byte search:** for split/replace with non-empty needles, byte-wise `memmem` on UTF-8 is safe and avoids per-character scans (segments remain valid UTF-8).
- **Unicode fallback:** for ops that return indices (e.g., `find`, `count`), map byte offsets to codepoint indices when non-ASCII via block-wise SIMD prefix counting.
- **UTF-8 index cache:** for large non-ASCII strings, keep a lazy block-prefix cache (byte -> codepoint) to amortize repeated index translation; cap entries to avoid unbounded memory growth.
- **Vectorization:** hot kernels (find/split/replace) should use SIMD where available; fall back to portable implementations.
- **Short-needle SIMD:** for UTF-8 needles of 2–4 bytes, use `memchr`-driven prefix scans to accelerate `find`/`count` without per-codepoint decoding.
- SIMD dispatch and architecture-specific policies are detailed in `docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md`.
- **No hidden nondeterminism:** avoid randomized hashing or locale-sensitive behavior.

## 3. Runtime Intrinsics
- Provide dedicated intrinsics for string operations instead of routing through generic list/byte paths.
- Keep list materialization deterministic and avoid intermediate allocations when possible (builder APIs, pre-sized join buffers).
- Expose in WIT/ABI for WASM parity.

## 4. Benchmarks and Gates
- Maintain dedicated micro-benchmarks for `find`, `split`, and `replace` in `tests/benchmarks/`.
- CI should track Molt/CPython ratios and alert on regressions beyond the configured threshold.
- Prefer realistic text sizes (100KB–10MB) to capture cache effects.

## 5. Correctness Notes
- `str.find` returns codepoint indices, not byte offsets.
- Empty separator behavior must match CPython (ValueError for split; replace inserts between codepoints).
- All string outputs must remain valid UTF-8.
