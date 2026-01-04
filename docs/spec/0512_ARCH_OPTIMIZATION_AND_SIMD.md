# Architecture-Specific Optimization & SIMD Plan
**Spec ID:** 0512
**Status:** Draft (performance roadmap)
**Owner:** runtime + backend
**Goal:** Maximize runtime performance via architecture-specific SIMD kernels with safe, fast fallbacks.

## 1. Principles
- Prefer SIMD kernels where they exist; fall back to scalar checked kernels.
- Dispatch at runtime based on CPU feature detection (`avx2`, `sse2`, `sse4.1`, `neon`, `simd128`).
- Guarded fast paths must preserve Python semantics; failures must fall back safely.
- WASM parity is required for Tier 1 kernels (use `simd128` when enabled).

## 2. Kernel Multi-Versioning
Each hot kernel ships multiple variants:
- **Scalar (checked):** correctness-first, minimal assumptions.
- **Scalar (trusted):** skips per-element checks using trusted type facts.
- **SIMD (checked):** vectorized with per-element type validation.
- **SIMD (trusted):** vectorized without per-element checks.

Dispatch order (example):
1) AVX2 (x86_64) / NEON (aarch64)
2) SSE2/SSE4.1 (x86_64)
3) Scalar fallback

## 3. Target Kernels
- Integer reductions: `sum`, `prod`, `min`, `max`
- String/bytes search: `find`, `split`, `replace`, `count`
- Buffer kernels: typed reductions on contiguous memory

## 4. CPU-Specific Roadmap
### x86_64
- AVX2 vector reductions for 64-bit ints where supported.
- SSE2/SSE4.1 fallback paths.
- AVX2 lacks native 64-bit integer multiply and boxed list storage limits vectorization; `prod` stays scalar for general cases. We added an AVX2 "trivial scan" for unboxed int arrays that detects all-ones or zero early, and still fall back to scalar multiplication otherwise. SIMD reductions would re-associate multiplies, which changes wrap semantics once 64-bit overflow occurs, so any SIMD path must be guarded by overflow-safe bounds (or be documented as a semantics change). Evaluate 32-bit partials + overflow guards before any wider SIMD multiply.
- Prototype unboxed int arrays (`intarray_from_seq`) are permitted in fast paths to reduce pointer chasing ahead of wider SIMD support.
- Explore AVX-512 for wide reductions where stable/available.

### aarch64
- NEON reductions; use compare+blend for min/max where direct intrinsics are limited.
- Keep scalar fallbacks for unsupported operations (notably `prod`); NEON adds only trivial zero/all-ones scans until a safe multiply strategy is available.

### wasm32
- Use `simd128` for byte search and reductions.
- Provide non-SIMD fallback for runtime environments without SIMD.
- Short-needle search kernels should use `simd128` byte masks to accelerate `find`/`count`.

## 5. SIMD-Friendly String Strategy
- Use `memchr`/`memmem` for fast byte scanning on ASCII.
- For Unicode indexing (`find`, `count`), map byte offsets to codepoint offsets.
- Prefer SIMD UTF-8 counting when available; fall back to scalar prefix counting (current implementation uses `simdutf::count_utf8`).

## 6. Preferred Crates (Avoid Reinventing the Wheel)
- `memchr`/`memmem`: fast byte scanning (already used).
- `simdutf8` or `simdutf`: UTF-8 validation/counting, useful for codepoint mapping.
- `bytemuck`/`zerocopy`: safe, fast typed buffer casts.
- `cpufeatures` or `multiversion`: structured multi-ISA dispatch.
- `libm`: deterministic math intrinsics when needed.

## 7. Validation & Regression Gates
- Add microbenchmarks per kernel variant.
- Record regression thresholds in `README.md` and enforce on CI.
- Ensure differential tests cover Unicode index semantics and buffer writes.

## 8. TODOs
- TODO(optimizations, owner:runtime): consider AVX-512 or 32-bit specialization for vectorized `prod` reductions.
- TODO(optimizations, owner:runtime): consider cached UTF-8 index tables for repeated non-ASCII `find`/`count`.
- TODO(optimizations, owner:backend): add wasm `simd128` kernels for string scans.
