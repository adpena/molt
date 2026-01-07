# Optimization Plan Spec

## Purpose
Capture complex, high-impact optimizations that require focused research, multi-week effort,
or careful risk management. Use this file as the source of truth for exploration, evaluation,
and regression control.

## When to Add an Entry
- The optimization is complex, risky, or time-intensive.
- It may affect semantics or performance across multiple benchmarks.
- It requires research, alternative algorithm evaluations, or new primitives.

## Required Fields (Checklist)
- [ ] Problem statement and current bottleneck
- [ ] Hypotheses and expected performance deltas
- [ ] Alternatives (at least 2), with tradeoffs
- [ ] Research references (papers, blog posts, standard algorithms)
- [ ] Implementation plan (phases, owners, dependencies)
- [ ] Benchmark matrix (affected benches + expected changes)
- [ ] Correctness plan (tests, differential cases, guard strategy)
- [ ] Risk assessment + rollback plan
- [ ] Success criteria + exit gates

## Template
```
### OPT-XXXX: <Short Title>

**Problem**
- What is slow or missing? Where is it measured?

**Hypotheses**
- H1:
- H2:

**Alternatives**
1) <Approach A> (pros/cons, complexity, risk)
2) <Approach B> (pros/cons, complexity, risk)
3) <Approach C> (pros/cons, complexity, risk)

**Research References**
- Paper/Link: summary of relevance
- Paper/Link: summary of relevance

**Plan**
- Phase 0: discovery and microbench harness
- Phase 1: prototype fast path
- Phase 2: integrate + guard + fallback
- Phase 3: stabilize + perf gates

**Benchmark Matrix**
- bench_sum_list.py: expected +X%
- bench_str_split.py: expected +Y%
- bench_str_replace.py: expected +Z%
- cross-bench risks: <list>

**Correctness Plan**
- New unit tests:
- Differential cases:
- Guard/fallback behavior:

**Risk + Rollback**
- Risk: <risk>
- Rollback steps:

**Success Criteria**
- Target speedups and regression thresholds
- Documentation updates
```

### OPT-0001: Unicode-Safe SIMD String Search + Index Mapping Cache

**Problem**
- Non-ASCII `str.find` and `str.count` require byte-to-codepoint translation, which is O(n) per query and can dominate large text workloads.

**Hypotheses**
- H1: A cached byte->codepoint index map (built lazily) will cut non-ASCII `find/count` overhead by >2x on large inputs.
- H2: SIMD UTF-8 validation/counting (simdutf-style) will reduce translation cost even without caching.

**Alternatives**
1) Lazy prefix index table (byte offset -> codepoint index) with amortized reuse across multiple calls.
2) SIMD UTF-8 counting for every call (no cache, lower memory overhead).
3) Rope/substring metadata caching (track codepoint offsets at slices) to avoid recomputing in hot loops.

**Research References**
- simdutf (fast UTF-8 validation and counting): https://github.com/simdutf/simdutf
- PEP 393 (CPython flexible string representation): https://peps.python.org/pep-0393/

**Plan**
- Phase 0: Add microbench for non-ASCII `find/count` on 1MB-10MB inputs.
- Phase 1: Prototype cached index table with eviction policy and measure memory overhead.
- Phase 2: Integrate SIMD UTF-8 counter for fast byte->codepoint mapping.
- Phase 3: Add guard + fallback; document tradeoffs in specs.

**Benchmark Matrix**
- bench_str_find.py: expected +2x on non-ASCII inputs.
- bench_str_count.py: expected +1.5x on non-ASCII inputs.
- bench_str_replace.py: no regression (<=2% change).

**Correctness Plan**
- New unit tests for mixed ASCII/Unicode `find`/`count` offsets.
- Differential cases with multi-byte codepoints and combining characters.
- Guard/fallback behavior for malformed UTF-8 (should be unreachable).

**Risk + Rollback**
- Risk: memory overhead from cached index tables.
- Rollback: disable cache, keep SIMD counting only.

**Success Criteria**
- >=2x speedup on non-ASCII `find`/`count` without regressions >5% on ASCII benchmarks.
- Spec + README updates with new perf gates.

**Latest Results (2026-01-04)**
- `bench_str_count_unicode.py`: 1.81x (steady vs prior run).
- `bench_str_find_unicode.py`: 4.82x (no regression observed).
- `bench_str_count_unicode_warm.py`: 0.25x (regression; warm cache path still dominated by index translation).
- Memory tradeoff: cache uses 8 bytes per 4KB block (≈2KB per 1MB string, ≈20KB per 10MB string); capped at 128 entries (~2.5MB worst-case) and only enabled for strings >=16KB.

### OPT-0002: Typed Buffer Protocol + SIMD Kernels for MemoryView

**Problem**
- Memoryview currently supports only 1D byte-level semantics; no typed formats, multidimensional views, or SIMD-friendly reductions.

**Hypotheses**
- H1: Introducing typed buffer views (e.g., int32/float64) unlocks fast zero-copy reductions and elementwise kernels.
- H2: A limited format subset (PEP 3118 core) can deliver most wins with lower implementation risk.

**Alternatives**
1) Full PEP 3118 format parser + multidimensional strides (highest compatibility, complex).
2) Core subset (bBhHiIlLqQfd) with 1D/2D only (faster, less risk).
3) External typed array wrapper (Rust-side) with explicit constructors (simpler, lower CPython parity).

**Research References**
- PEP 3118 (buffer protocol): https://peps.python.org/pep-3118/
- NumPy ndarray strides and buffer interface docs: https://numpy.org/doc/

**Plan**
- Phase 0: Define supported format subset + shape/stride rules in spec.
- Phase 1: Implement typed buffer metadata + safe export hooks in runtime.
- Phase 2: Add SIMD reductions for typed views (sum/min/max/prod).
- Phase 3: Add frontend guards and lowering for trusted typed buffers.

**Benchmark Matrix**
- bench_sum_list.py: expected +1.5x on typed buffers.
- bench_matrix_math.py: expected +1.2x from zero-copy views.
- bench_deeply_nested_loop.py: no regression (<=3%).

**Correctness Plan**
- Differential tests for typed buffer slicing and endianness.
- Property tests for stride correctness and bounds checks.
- Guard/fallback behavior for unsupported formats.

**Risk + Rollback**
- Risk: format/stride bugs causing data corruption.
- Rollback: restrict to read-only typed views until stabilized.

**Success Criteria**
- >=1.5x speedup on typed reductions with zero-copy interop.
- Spec + roadmap updates and regression gates in README.

### OPT-0003: Provenance-Safe Handle Table for NaN-Boxed Objects

**Problem**
- NaN-boxed heap pointers currently lose provenance by packing raw addresses into 48 bits.
- Strict provenance tooling (Miri) flags integer-to-pointer casts and can miss real bugs.
- We need a production-grade representation that preserves correctness without slowing hot paths.

**Hypotheses**
- H1: A handle table with generation checks removes provenance UB and prevents stale handle reuse.
- H2: Sharded or lock-free handle lookup keeps overhead <2% on attribute/collection hot paths.
- H3: Storing handles (not raw addresses) unlocks future GC/compaction without ABI churn.

**Alternatives**
1) Status quo pointer tagging + `with_exposed_provenance` (fast but provenance-unsafe; Miri warnings remain).
2) Global handle table with locking (simple, safe, but likely slower on hot path).
3) Sharded handle table with lock-free reads + generation checks (more complex, best perf).
4) Arena + offset scheme (bounds-checked offsets; high complexity and migration cost).

**Research References**
- Rust strict provenance docs: https://doc.rust-lang.org/std/ptr/index.html#strict-provenance
- Miri provenance model notes: https://github.com/rust-lang/miri
- Generational indices: https://cglab.ca/~abeinges/blah/slab-allocators/
- CHERI capability pointers overview: https://www.cl.cam.ac.uk/research/security/ctsrd/cheri/

**Plan**
- Phase 0: Define handle encoding (index + generation) and table invariants in spec.
- Phase 1: Implement handle table + pointer map in `molt-obj-model`, wire `MoltObject` through it.
- Phase 2: Add unregister hooks on object free; validate with Miri strict provenance.
- Phase 3: Optimize lookup (sharding/lock-free read path) and profile against CPython/Molt baselines.

**Benchmark Matrix**
- bench_deeply_nested_loop.py: expected <=2% change after lock-free read path.
- bench_sum_list.py: expected <=2% change (handle lookup on list elements).
- bench_str_find.py: expected <=2% change (string object access).
- cross-bench risks: attribute access, dict lookup, and method dispatch regressions.

**Correctness Plan**
- New unit tests for handle reuse (generation mismatch => None).
- Differential cases: handle-heavy list/dict operations and attribute access.
- Guard/fallback behavior: invalid handle returns `None`/error, never dereference freed memory.

**Risk + Rollback**
- Risk: lookup contention or handle table growth hurting perf/memory.
- Rollback: keep handle table behind a feature flag and revert to pointer tagging.

**Success Criteria**
- Miri strict provenance passes with `-Zmiri-strict-provenance`.
- <2% overhead on hot-path microbenchmarks after lock-free/sharded lookup.
- Updated runtime spec, README/ROADMAP, and CI gates reflect the new object model.
