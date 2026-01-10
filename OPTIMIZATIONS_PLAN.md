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

### OPT-0004: Sharded/Lock-Free Handle Resolve Fast Path

**Problem**
- The current handle table uses a global lock on lookup, adding overhead to
  hot paths (attribute access, list/dict ops, method dispatch).
- Preliminary handle-table benchmarks show small but measurable deltas; we need
  a scalable fast path before widening the verified subset.

**Hypotheses**
- H1: Sharded tables with per-shard locks cut lookup contention to <2% overhead.
- H2: Lock-free reads with atomic generation checks bring lookup overhead under 1%.
- H3: A small thread-local handle cache reduces repeated lookups in tight loops.

**Alternatives**
1) Sharded `RwLock` table keyed by handle index (moderate complexity, good wins).
2) Lock-free slab with atomic generation + epoch GC (higher complexity, best perf).
3) Thread-local cache + fallback to global lock (low risk, partial win).

**Research References**
- Generational index slabs: https://cglab.ca/~abeinges/blah/slab-allocators/
- Crossbeam epoch-based reclamation: https://docs.rs/crossbeam-epoch/
- Folly AtomicHashMap (lock-free reads): https://github.com/facebook/folly

**Plan**
- Phase 0: Re-run handle-table benchmarks (`bench_handle_lock.json`) and expand to
  a wider suite to quantify overhead.
- Phase 1: Prototype sharded table with lock striping; measure deltas.
- Phase 2: Add optional lock-free read path + generation validation.
- Phase 3: Stabilize with correctness tests + Miri + fuzz.

**Benchmark Matrix**
- bench_sum.py: expected <=1% overhead vs baseline
- bench_bytes_find.py: expected <=1% overhead vs baseline
- bench_list_append.py: expected <=2% overhead
- bench_dict_set.py: expected <=2% overhead
- bench_attr_access.py: expected <=2% overhead

**Correctness Plan**
- New unit tests: generation mismatch, tombstone reuse, concurrent lookup.
- Differential cases: attribute-heavy class tests + list/dict ops.
- Guard/fallback behavior: invalid handle always raises or returns None; never
  dereference freed memory.

**Risk + Rollback**
- Risk: subtle data races or ABA bugs in lock-free path.
- Rollback: keep sharded locks only; disable lock-free reads behind feature flag.

**Success Criteria**
- Handle lookup overhead <=1% on hot-path benchmarks.
- Miri clean under strict provenance; fuzz targets green.
- Documented tradeoffs in `docs/spec/0020_RUNTIME_SAFETY_INVARIANTS.md`.

**Latest Results (2026-01-07, partial suite)**
- `bench_handle_lock.json` vs `bench_handle_sharded.json`:
  - bench_sum.py: 0.01177s -> 0.01084s (~8% faster)
  - bench_bytes_find.py: 0.01334s -> 0.01358s (~2% slower)
- Next: run full bench matrix to confirm net impact.

### OPT-0005: Monomorphic Direct-Call Fast Path for Recursion (bench_fib)

**Problem**
- `bench_fib.py` is ~4x slower than CPython, dominated by dynamic call dispatch
  for a simple self-recursive function.

**Hypotheses**
- H1: Emitting a direct call to a known local function with an identity guard
  reduces overhead enough to beat CPython on `bench_fib.py`.
- H2: A small inline-cache for function objects handles the common case without
  affecting dynamic semantics.

**Alternatives**
1) Direct-call lowering with function-identity guard + fallback to generic call.
2) Inline caching for local/global call sites (guarded, deopt to generic).
3) Inline expansion for self-recursive calls (higher complexity, code size risk).

**Research References**
- Deutsch & Schiffman (1984) inline caching (Smalltalk-80).
- PEP 659 (Specializing Adaptive Interpreter) for call-site specialization ideas.

**Plan**
- Phase 0: Profile call overhead in `bench_fib.py` and other call-heavy benches.
- Phase 1: Add IR op for direct call when callee is a known local/global symbol.
- Phase 2: Guard on function identity; fallback to generic call on mismatch.
- Phase 3: Evaluate optional inline-cache and recursion inlining.

**Benchmark Matrix**
- bench_fib.py: expected >=2x improvement
- bench_deeply_nested_loop.py: no regression (<=3%)
- bench_struct.py: no regression (<=3%)

**Correctness Plan**
- Differential tests for function reassignment and recursion correctness.
- Guard/fallback behavior: identity mismatch falls back to generic call.

**Risk + Rollback**
- Risk: invalid specialization when functions are rebound.
- Rollback: keep generic call path as default; gate direct-call behind a flag.

**Success Criteria**
- `bench_fib.py` >=1.0x vs CPython without regressions elsewhere.

**Latest Profile (2026-01-07)**
- `bench_fib.py` with `MOLT_PROFILE=1`:
  - call_dispatch=0 (direct calls are already used)
  - string_count_cache_hit=0 / miss=0
  - struct_field_store=0
- Next: `CALL_GUARDED` op landed to guard direct name calls by fn_ptr; re-benchmark
  fib and consider arity-checked fallback for mismatched globals.
- Re-run (2026-01-07): call_dispatch=0 / string_count_cache_hit=0 / miss=0 / struct_field_store=0.
- Execution checklist:
  - [x] Re-run `bench_fib.py` with `MOLT_PROFILE=1` and record call_dispatch deltas.
  - [x] Add differential coverage for rebinding a guarded local/global and verify fallback.
  - [ ] Decide whether to add arity checks on the guarded fast path.
- Update (2026-01-07): added stable-module direct-call lowering (no guard) when a function name
  is only bound once at module scope and not declared global elsewhere; see `call_rebind.py`.
- Update (2026-01-08): benchmark regression persists despite direct calls:
  - `bench_fib.py`: 0.9290s (Molt) vs 0.2168s (CPython), 0.23x.
  - Priority: inspect call dispatch and recursion prologue/epilogue costs; verify IR emits fast
    path for local recursion and avoid tuple/list allocations for argument passing.

### OPT-0006: Unicode Count Warm-Cache Fast Path

**Problem**
- `bench_str_count_unicode_warm.py` is ~4x slower than CPython even with a warm
  cache, indicating cached index map reuse is not amortizing correctly.

**Hypotheses**
- H1: Store a prefix codepoint-count table in the cache to make `count` O(n)
  over matches rather than O(n) over translation each call.
- H2: Avoid reallocating or recomputing byte->codepoint maps on warm paths.

**Alternatives**
1) Prefix count table stored alongside the UTF-8 index cache.
2) SIMD codepoint counting for each call (simpler, more CPU).
3) Cache per-slice metadata to reuse byte->codepoint offsets.

**Research References**
- PEP 393 (flexible string representation).
- simdutf UTF-8 counting/validation (https://github.com/simdutf/simdutf).

**Plan**
- Phase 0: Inspect cache hit/miss behavior and measure translation overhead.
- Phase 1: Extend cache entries with prefix count metadata.
- Phase 2: Add fast path for repeated `count` on same string/haystack.
- Phase 3: Validate against Unicode differential cases.

**Benchmark Matrix**
- bench_str_count_unicode_warm.py: expected >=2x improvement
- bench_str_count_unicode.py: no regression (<=5%)
- bench_str_find_unicode_warm.py: no regression (<=5%)

**Correctness Plan**
- Differential tests with mixed-width Unicode and combining characters.
- Validate count semantics on overlapping patterns.

**Risk + Rollback**
- Risk: cache memory overhead grows on large strings.
- Rollback: disable prefix-count caching behind size threshold.

**Success Criteria**
- `bench_str_count_unicode_warm.py` >=1.0x vs CPython.

**Latest Profile (2026-01-08)**
- `bench_str_count_unicode_warm.py` with `MOLT_PROFILE=1`:
  - string_count_cache_hit=25
  - string_count_cache_miss=1
  - call_dispatch=0 / struct_field_store=0
- Update (2026-01-07): added a thread-local fast path for count cache hits to avoid
  global lock overhead on warm loops.
- Update (2026-01-08): sharded UTF-8 count cache store to reduce lock contention.
- Update (2026-01-07): aligned UTF-8 cache block offsets to codepoint boundaries and
  added `str.count` start/end slicing support with Unicode combining tests.
- Targeted bench run (2026-01-08, `bench/results/bench_tls_struct.json`):
  - `bench_str_count_unicode.py`: 0.0296s (Molt) vs 0.0287s (CPython), 1.03x.
  - `bench_str_count_unicode_warm.py`: 0.2359s (Molt) vs 0.0289s (CPython), 8.16x.
  - `bench_struct.py`: 0.2188s (Molt) vs 1.6611s (CPython), 0.13x.
- Execution checklist:
  - [x] Run unicode count benches and capture warm vs cold delta with `MOLT_PROFILE=1`.
  - [x] Prototype prefix-count metadata in cache entries and record memory overhead.
  - [ ] Add Unicode differential cases that exercise combining characters.
- Update (2026-01-08): full bench run shows warm count ahead of CPython:
  - `bench_str_count_unicode_warm.py`: 0.0228s (Molt) vs 0.0954s (CPython), 4.18x.
  - `bench_str_count_unicode.py`: 0.0223s (Molt) vs 0.0449s (CPython), 2.02x.
- Update (2026-01-08): added prefix-count metadata to `Utf8CountCache` with lazy
  promotion on slice paths to avoid penalizing full-count hot paths.
- Update (2026-01-08): current bench run confirms warm/cold wins:
  - `bench_str_count_unicode_warm.py`: 0.0334s (Molt) vs 0.1453s (CPython), 4.36x.
  - `bench_str_count_unicode.py`: 0.0347s (Molt) vs 0.0660s (CPython), 1.90x.

### OPT-0007: Structified Class Fast Path + Optional Scalar Replacement

**Problem**
- `bench_struct.py` shows attribute store and object allocation overhead for a
  simple annotated class, suggesting struct field stores are not hitting a
  direct slot path.

**Hypotheses**
- H1: Precompute a fixed layout for classes with field annotations and lower
  attribute stores to direct slot writes.
- H2: If objects do not escape the loop, scalar replacement can eliminate
  allocations entirely.

**Alternatives**
1) Force structification for annotated classes without dynamic features.
2) Escape analysis + scalar replacement (higher complexity, best perf).
3) Inline cache on attribute store for monomorphic classes.

**Research References**
- "Escape Analysis for Java" (Choi et al.) and similar scalar replacement work.
- HotSpot scalar replacement notes (public JVM documentation).

**Plan**
- Phase 0: Confirm class layout inference for annotated classes without __init__.
- Phase 1: Lower SETATTR to direct slot stores when layout is fixed.
- Phase 2: Add escape analysis prototype to elide allocations in loops.
- Phase 3: Validate correctness + update structification rules in docs/spec.

**Latest Profile (2026-01-07)**
- `bench_struct.py` with `MOLT_PROFILE=1`:
  - struct_field_store=4,000,000
  - call_dispatch=0
- Next: reduce store overhead (guard cost vs direct slot writes) and consider
  escape-analysis gating for allocation removal.
- Update: exact-local class tracking now skips guarded setattr for constructor-bound
  locals with fixed layouts; measure impact on `bench_struct.py`.
- Re-run (2026-01-07): call_dispatch=0 / struct_field_store=2,000,000 (after store_init defaults).
- Execution checklist:
  - [ ] Measure guard vs direct-slot store cost on `bench_struct.py` and record.
  - [x] Add differential tests for dynamic class mutation that must deopt.
  - [x] Gate structified stores behind a layout-stability guard.
- Update (2026-01-07): added `store_init` lowering for immediate defaults to avoid
  refcount work on freshly allocated objects.
- Update (2026-01-08): regression still severe after latest benches:
  - `bench_struct.py`: 1.7825s (Molt) vs 0.5453s (CPython), 0.31x.
  - Priority: eliminate guard overhead on hot struct stores or move to monomorphic
    slot stores with layout-stability checks.
- Update (2026-01-08): added class layout version tracking and guarded slot
  loads/stores; class mutations now bump version and deopt to generic paths.
- Update (2026-01-08): fused guarded field ops reduced guard+store calls, but
  `bench_struct.py` is still 0.07x and `bench_attr_access.py` 0.13x vs CPython.
- Update (2026-01-09): re-bench shows `bench_struct.py` ~0.12x and
  `bench_attr_access.py` ~0.20x; guard cost is still dominating.
- Update (2026-01-09): loop guard caching landed, but `bench_struct.py` remains
  ~0.12x and `bench_attr_access.py` ~0.21x vs CPython; next step is to hoist
  class layout guards outside hot loops or memoize per loop iteration to remove
  redundant guard overhead.
- Update (2026-01-09): hoisted layout guards outside loop bodies; re-bench still
  shows `bench_struct.py` ~0.12x and `bench_attr_access.py` ~0.19x vs CPython,
  so the bottleneck is now direct slot store/load cost rather than guard checks.
- Update (2026-01-09): inlined struct field load/store in the native backend
  (skip refcount calls for immediates, conditional profile) and re-bench still
  shows `bench_struct.py` ~0.12x and `bench_attr_access.py` ~0.20x; likely dominated
  by allocation + raw-object tracking costs (mutex/HashSet) rather than slot ops.

**Benchmark Matrix**
- bench_struct.py: expected >=2x improvement
- bench_sum_list.py: no regression (<=3%)
- bench_deeply_nested_loop.py: no regression (<=3%)

**Correctness Plan**
- Differential tests for attribute assignment order and class mutation.
- Guard/fallback behavior: dynamic class changes fall back to generic store.

**Risk + Rollback**
- Risk: misclassification of dynamic classes leading to wrong attribute behavior.
- Rollback: require explicit "frozen layout" flag or dataclass lowering.

**Success Criteria**
- `bench_struct.py` >=1.0x vs CPython with no semantic regressions.

### OPT-0008: Descriptor/Property Access Fast Path (bench_descriptor_property)

**Problem**
- `bench_descriptor_property.py` is ~4x slower than CPython, dominated by repeated
  descriptor lookup and attribute resolution overhead.

**Hypotheses**
- H1: Cache descriptor resolution for monomorphic classes and inline property getter calls.
- H2: Split data vs non-data descriptor paths earlier to avoid extra dictionary probes.

**Alternatives**
1) Inline cache for attribute lookup with descriptor classification (data vs non-data).
2) Pre-resolved descriptor slots on class creation (update on class mutation).
3) Guarded direct-call lowering for `property.__get__` on known fields.

**Research References**
- CPython descriptor protocol and attribute lookup order (Objects/typeobject.c).
- Inline caching for attribute lookup (PIC/IC literature).

**Plan**
- Phase 0: Profile attribute resolution in `bench_descriptor_property.py`.
- Phase 1: Add an attribute lookup IC keyed by (class, attr_name) with descriptor kind.
- Phase 2: Add guarded direct-call for property getters.
- Phase 3: Deopt on class mutation (mro/attrs change).
- Update (2026-01-08): added TLS attribute-name cache and descriptor IC keyed by
   (class bits, attr bits, layout version), with class mutation version bumping.
 - Update (2026-01-08): bench still regresses (`bench_descriptor_property.py` 0.12x);
   need direct-call lowering and IC that avoids repeated generic lookups.
 - Update (2026-01-09): added a guarded property-get fast path (layout guard +
   direct getter call) to bypass descriptor lookup in hot loops; re-bench pending.
 - Update (2026-01-09): re-bench shows `bench_descriptor_property.py` ~0.10x;
   guarded property get is not enough without call overhead reduction.
 - Update (2026-01-09): loop guard caching + trivial property inline
   (`return self.<field>`) improved `bench_descriptor_property.py` to ~0.26x, but
   call overhead is still dominating; target direct field loads or getter inline
   without call overhead.
 - Update (2026-01-09): hoisted loop guards did not move the needle; still
   ~0.27x vs CPython, so we need a non-call, direct-slot property fast path for
   common `property` patterns or inlined getter bodies.
 - Update (2026-01-09): backend inline load/store did not materially improve
   `bench_descriptor_property.py` (~0.27x); call overhead is not the only bottleneck.

**Benchmark Matrix**
- bench_descriptor_property.py: expected >=2x improvement
- bench_attr_access.py: expected >=1.5x improvement
- bench_struct.py: no regression (<=3%)

**Correctness Plan**
- Differential tests for data vs non-data descriptors.
- Class mutation tests to ensure deopt correctness.

**Risk + Rollback**
- Risk: stale IC entries after class mutation.
- Rollback: guard IC behind class version and disable on mutation.

**Success Criteria**
- `bench_descriptor_property.py` >=1.0x vs CPython without regressions.

### OPT-0009: String Split/Join Builder Fast Path (bench_str_split, bench_str_join)

**Problem**
- `bench_str_split.py` and `bench_str_join.py` are <1.0x vs CPython, indicating
  list builder and allocation overhead for common split/join patterns.

**Hypotheses**
- H1: Pre-size list/byte buffers and use a single allocation per split.
- H2: Fast-path ASCII delimiter and whitespace splitting to avoid UTF-8 scanning.

**Alternatives**
1) Pre-scan delimiter positions + single allocation list builder.
2) Use memchr/memmem for ASCII delimiter and fall back for Unicode.
3) Specialized whitespace split (CPython-like) for the default separator.

**Research References**
- CPython `unicode_split` and `unicode_join` implementations.
- memchr/memmem optimized substring search techniques.

**Plan**
- Phase 0: Profile allocation counts in split/join benches.
- Phase 1: Pre-scan and reserve list capacity; avoid per-element reallocs.
- Phase 2: ASCII fast paths using memchr/memmem.
- Phase 3: Validate whitespace split semantics.
 - Update (2026-01-08): removed positions vector in split; now two-pass count +
   split with memchr/memmem to reduce allocations.
 - Update (2026-01-08): `bench_str_split.py` remains 0.43x and `bench_str_join.py`
   0.75x; new plan needed for string builder fast paths.
 - Update (2026-01-08): restored single-scan delimiter index capture for split
   (avoid double memmem pass) and cache join element pointers/lengths for a
   single copy loop.
 - Update (2026-01-08): `bench_str_join.py` improved to 0.52x; `bench_str_split.py`
   still 0.27x after keeping the single-byte separator on the count+split path.
 - Update (2026-01-09): added split token reuse cache for short repeated pieces
   and a join fast path for repeated identical elements using a doubling copy
   fill strategy; re-bench pending.
 - Update (2026-01-09): re-bench shows `bench_str_split.py` ~2.04x and
   `bench_str_join.py` ~0.95x; split success, join near parity but still <1.0x.
 - Update (2026-01-09): re-bench shows `bench_str_split.py` ~2.03x and
   `bench_str_join.py` ~0.91x; join remains below parity, investigate repeated-
   element fast path thresholds and allocation behavior.

**Benchmark Matrix**
- bench_str_split.py: expected >=2x improvement
- bench_str_join.py: expected >=1.5x improvement
- bench_str_count.py: no regression (<=3%)

**Correctness Plan**
- Differential tests for whitespace vs explicit separator.
- Unicode edge cases with multi-byte separators.

**Risk + Rollback**
- Risk: incorrect split behavior on Unicode or empty separators.
- Rollback: fall back to generic path for non-ASCII separators.

**Success Criteria**
- `bench_str_split.py` >=1.0x and `bench_str_join.py` >=1.0x vs CPython.

### OPT-0010: Vector Reduction Regressions (bench_min_list/bench_max_list/bench_sum_list)

**Problem**
- Recent benches show regressions on vector reductions vs CPython, indicating
  that reduction fast paths are not triggering or have overhead regressions.

**Hypotheses**
- H1: Reduction fast path disabled by type guards or iterator conversions.
- H2: Avoid boxing/unboxing in hot loops to restore expected speedups.

**Alternatives**
1) Rework reduction IR to specialize on list/tuple of ints with tight loops.
2) Add guarded fast path in runtime for homogeneous int vectors.
3) Inline reduction in frontend with static specialization.

**Research References**
- CPython `sum`/`min`/`max` C implementations.
- Loop vectorization and unboxing techniques in JITs.

**Plan**
- Phase 0: Instrument reduction path to confirm guard hits.
- Phase 1: Restore direct int vector fast path (no iterator allocations).
- Phase 2: Add fast path for small tuples/ranges.
- Phase 3: Update perf gates and regression tests.

**Benchmark Matrix**
- bench_sum_list.py: expected >=1.5x improvement
- bench_min_list.py: expected >=1.5x improvement
- bench_max_list.py: expected >=1.5x improvement

**Correctness Plan**
- Differential tests for NaN/None comparisons and mixed types.
- Guard/fallback for non-int sequences.

**Risk + Rollback**
- Risk: incorrect behavior on mixed-type sequences.
- Rollback: guard fast path to int-only lists/tuples.

**Success Criteria**
- Each reduction bench >=1.0x vs CPython with no semantic regressions.

### OPT-0011: Aggressive Monomorphization + Specialization Pipeline

**Problem**
- Generic lowering leaves hot call sites and arithmetic paths boxed and indirect.
- We lack a systematic pipeline for cloning specialized versions based on stable types.

**Hypotheses**
- H1: Monomorphic specializations with guards will reduce dispatch overhead by 2x+ in hot loops.
- H2: Cloning a small number of specialized variants is cheaper than inline caches for pure numeric code.

**Alternatives**
1) Call-site driven specialization (guard + fallback, clone per dominant type set).
2) Whole-function specialization driven by type facts/annotations.
3) Profile-guided multi-versioning (only for hot functions).

**Research References**
- Chambers, Ungar, Lee: "An Efficient Implementation of SELF: A Dynamically-Typed Object-Oriented Language Based on Prototypes."
- CPython PEP 659: Specializing Adaptive Interpreter.

**Plan**
- Phase 0: Define specialization policy in docs/spec/0017_TYPE_SYSTEM_AND_SPECIALIZATION.md.
- Phase 1: Emit guarded monomorphic clones for numeric-heavy locals and known globals.
- Phase 2: Add multi-version cache keyed by type vector at call sites.
- Phase 3: Add perf gates and regression detection on type-unstable workloads.

**Benchmark Matrix**
- tests/benchmarks/bench_fib.py: expected +2x
- tests/benchmarks/bench_matrix_math.py: expected +1.5x
- tests/benchmarks/bench_sum_list.py: expected +1.5x

**Correctness Plan**
- Differential tests for mixed-type arithmetic and deopt fallback.
- Guarded fallback path for unknown or unstable types.

**Risk + Rollback**
- Risk: code size growth and slower cold-start.
- Rollback: cap specialization count per function and fall back to generic path.

**Success Criteria**
- >=1.5x on numeric-heavy benches without regressions >5% on mixed-type benches.

### OPT-0012: Inline Caches for Attribute Access + Call Dispatch

**Problem**
- Attribute lookup and call dispatch dominate in object-heavy workloads.
- Layout guards are repeated per access with no reuse across sites.

**Hypotheses**
- H1: Monomorphic inline caches remove repeated layout checks and dict lookups.
- H2: Polymorphic inline caches (PICs) reduce cost for small type sets.

**Alternatives**
1) Monomorphic IC per site with class version tags.
2) PIC with 2-4 entries and a megamorphic fallback.
3) Global method cache keyed by (type, name) with epoch invalidation.

**Research References**
- Holzle, Ungar: "Optimizing Dynamically-Typed Object-Oriented Languages with Polymorphic Inline Caches."
- CPython method cache and type version tags (Objects/typeobject.c).

**Plan**
- Phase 0: Add IC counters and invalidation plumbing (type versioning).
- Phase 1: Monomorphic IC for attribute get/set and call dispatch.
- Phase 2: PIC expansion + megamorphic fallback.
- Phase 3: Stabilize guards, document in specs, and add perf gates.

**Benchmark Matrix**
- tests/benchmarks/bench_attr_access.py: expected +2x
- tests/benchmarks/bench_descriptor_property.py: expected +1.5x
- tests/benchmarks/bench_struct.py: expected +1.5x

**Correctness Plan**
- Differential tests for dynamic class mutation and descriptor precedence.
- Guard/fallback for changes in class dicts or MRO.

**Risk + Rollback**
- Risk: invalidation bugs causing stale reads.
- Rollback: disable ICs under debug flag and keep layout guard path.

**Success Criteria**
- Attribute-heavy benches >=1.5x, no correctness regressions.

### OPT-0013: Layout-Guard Elimination via Shape Stabilization

**Problem**
- Layout guards are costly and repeated, even when class layout is stable.

**Hypotheses**
- H1: Dominating guard hoisting can eliminate repeated layout checks in loops.
- H2: Shape-stable classes can skip guards entirely after validation.

**Alternatives**
1) Guard hoisting in SSA (loop-invariant checks).
2) Class version stamps + global shape table (guard once per function).
3) User-declared "final" classes with static layout guarantees.

**Research References**
- Self/Strongtalk shape-based optimization techniques.
- PyPy map/shadow object layout techniques.

**Plan**
- Phase 0: Add IR dominance analysis for guard hoisting candidates.
- Phase 1: Hoist guards to block entry and reuse cached results.
- Phase 2: Add shape-stable class annotation and skip guards under conditions.
- Phase 3: Perf validation, guard auditing, and spec updates.

**Benchmark Matrix**
- tests/benchmarks/bench_struct.py: expected +1.5x
- tests/benchmarks/bench_attr_access.py: expected +1.3x
- tests/benchmarks/bench_descriptor_property.py: expected +1.2x

**Correctness Plan**
- Differential tests for class mutation after guard.
- Fallback to guarded path on any shape change.

**Risk + Rollback**
- Risk: incorrect guard elimination on dynamic mutation.
- Rollback: keep guard checks and disable hoisting in debug builds.

**Success Criteria**
- >=1.3x on layout-heavy benches with zero semantic regressions.

### OPT-0014: Escape Analysis + Scalar Replacement

**Problem**
- Short-lived objects (tuples, lists, small structs) still allocate on the heap.

**Hypotheses**
- H1: Escape analysis can identify non-escaping allocations for stack or scalar replacement.
- H2: Scalar replacement will reduce allocation pressure and improve cache locality.

**Alternatives**
1) SSA-based escape analysis with stack allocation for non-escaping objects.
2) Region-based allocation for short-lived temps.
3) Annotated allocation hints (compiler-assisted) for common patterns.

**Research References**
- "Escape Analysis for Object-Oriented Languages" (Choi et al.).
- JVM HotSpot scalar replacement and escape analysis docs.

**Plan**
- Phase 0: Add allocation site IDs and liveness tracing.
- Phase 1: SSA escape analysis and stack allocation for simple objects.
- Phase 2: Scalar replacement for tuples/structs in tight loops.
- Phase 3: Extend to lists/dicts where safe; add perf gates.

**Benchmark Matrix**
- tests/benchmarks/bench_tuple_pack.py: expected +2x
- tests/benchmarks/bench_struct.py: expected +1.5x
- tests/benchmarks/bench_list_ops.py: expected +1.3x

**Correctness Plan**
- Differential tests for object identity, aliasing, and mutation semantics.
- Guard/fallback when escaping or captured by closures.

**Risk + Rollback**
- Risk: aliasing bugs or lifetime mismanagement.
- Rollback: disable escape analysis pass and keep heap allocation.

**Success Criteria**
- >=1.5x on allocation-heavy benches with no behavioral regressions.

### OPT-0015: PGO/LTO/BOLT Pipeline for Runtime + Stubs

**Problem**
- Runtime and stubs are compiled without profile-guided or link-time optimization.

**Hypotheses**
- H1: PGO will improve branch prediction and inline choices in runtime hot paths.
- H2: LTO/BOLT will reduce call overhead and improve I-cache locality.

**Alternatives**
1) LLVM PGO for runtime crates and C stubs; keep Cranelift for generated code.
2) LTO-only for runtime (thin-LTO), no PGO.
3) BOLT post-link optimization for release artifacts.

**Research References**
- LLVM PGO and ThinLTO documentation.
- BOLT optimization tooling: https://github.com/llvm/llvm-project/tree/main/bolt

**Plan**
- Phase 0: Add scripts to collect profiles on bench suite.
- Phase 1: Enable thin-LTO in Cargo release profile for runtime crates.
- Phase 2: Integrate PGO builds for runtime + C stubs.
- Phase 3: Evaluate BOLT on final artifacts; document tradeoffs.

**Benchmark Matrix**
- tests/benchmarks/bench_dict_ops.py: expected +1.1x
- tests/benchmarks/bench_str_count.py: expected +1.1x
- tests/benchmarks/bench_attr_access.py: expected +1.1x

**Correctness Plan**
- Ensure identical functional output across PGO/LTO builds.
- Run differential suite after PGO config changes.

**Risk + Rollback**
- Risk: build complexity and non-reproducible binaries if profiles drift.
- Rollback: disable PGO/LTO flags and revert to baseline release builds.

**Success Criteria**
- >=1.1x on multiple benches with no determinism regressions.
