# Molt Roadmap: The Evolution of Python

Molt compiles a verified subset of Python into extremely fast, single-file native binaries and WASM. This document tracks our progress from research prototype to production-grade systems runtime.

**Ultimate Goal:** A Go-like developer experience for Python, producing binaries that rival C/Rust in performance and safety, suitable for high-concurrency web services, databases, and data pipelines.

**Source of truth:** This file is the canonical status tracker. For near-term sequencing, see `ROADMAP_90_DAYS.md`. For historical milestone framing, see `docs/spec/0006-roadmap.md`.

---

## 🚀 Milestone Status

| Feature | Status | Date Completed | Notes |
| :--- | :--- | :--- | :--- |
| **0. Technical Specification** | ✅ Done | 2026-01-02 | Defined IR stack, tiers, and security model. |
| **1. NaN-boxed Object Model** | ✅ Done | 2026-01-02 | Efficient 64-bit tagged pointer representation. |
| **2. Tier 0 Structification** | ✅ Done | 2026-01-02 | Fixed-offset attribute access for typed classes. |
| **3. AOT Backend (Native)** | ✅ Done | 2026-01-02 | Cranelift-based machine code generation. |
| **4. AOT Backend (WASM)** | ✅ Done | 2026-01-02 | Direct WebAssembly bytecode generation. |
| **5. Tier 1 Guards** | ✅ Done | 2026-01-02 | Runtime type-check specializing hot paths. |
| **6. Molt Packages (Structured Codecs)** | ✅ Done | 2026-01-02 | Rust-backed packages with MsgPack/CBOR/Arrow IPC; JSON retained for compatibility. |
| **7. Differential Testing** | ✅ Done | 2026-01-02 | Automated verification against CPython 3.12. |
| **8. True Async Runtime** | ✅ Done | 2026-01-02 | State-machine lowering + Poll-based ABI. |
| **9. Closure Conversion** | ✅ Done | 2026-01-02 | Async locals stored in Task objects. |
| **10. WASM Host Interop** | ✅ Done | 2026-01-02 | Standardized host imports for async/memory. |
| **11. Garbage Collection** | 📅 Backlog | - | RC + Incremental Cycle Detection. |
| **12. Profile-Guided Opt (PGO)** | 📅 Backlog | - | Feedback-driven specialization. |
| **13. Performance Benchmarking** | ✅ Done | 2026-01-02 | Automated suites vs CPython 3.12. |
| **14. Multi-Version Compliance** | ✅ Done | 2026-01-02 | CI Matrix for Python 3.12, 3.13, 3.14. |
| **15. Compliance Scaffolding** | ✅ Done | 2026-01-02 | `tests/compliance/` structure for future specs. |
| **16. MLIR Pipeline** | 📅 Backlog | - | Domain-specific optimizations for data tasks. |
| **17. Loop Optimization & Vectorization** | 🚧 In Progress | - | Canonical loops, SIMD kernels, guard+fallback. |

---

## 🧭 Type Coverage Milestones
**Tracking doc:** `docs/spec/0014_TYPE_COVERAGE_MATRIX.md`

| Milestone | Focus | Owners | Status | Notes |
| :--- | :--- | :--- | :--- | :--- |
| **TC1** | Exceptions + full container semantics + range/slice polish | runtime, frontend, tests | 🚧 In Progress | TODO(type-coverage, owner:runtime, milestone:TC1): exception object model + raise/try. |
| **TC2** | set/frozenset + generators/coroutines + callables | runtime, frontend, backend | 📅 Planned | TODO(type-coverage, owner:backend, milestone:TC2): wasm ABI for generator state. |
| **TC3** | memoryview + type/object + modules/descriptors | runtime, stdlib | 📅 Planned | TODO(type-coverage, owner:stdlib, milestone:TC3): module object + import rules. |

Type coverage TODOs tracked here for CI parity:
- TODO(type-coverage, owner:frontend, milestone:TC1): `try/except/finally` lowering + raise paths.
- TODO(type-coverage, owner:frontend, milestone:TC1): comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `in`, chained comparisons) + lowering rules.
- TODO(type-coverage, owner:frontend, milestone:TC1): builtin reductions (`sum/min/max`) and `len` parity.
- TODO(type-coverage, owner:frontend, milestone:TC1): builtin constructors for `tuple`, `dict`, `bytes`, `bytearray`.
- TODO(type-coverage, owner:runtime, milestone:TC1): exception objects + stack trace capture.
- TODO(type-coverage, owner:runtime, milestone:TC1): recursion limits + `RecursionError` guard semantics.
- TODO(type-coverage, owner:tests, milestone:TC1): add exception + set coverage to molt_diff.
- TODO(type-coverage, owner:runtime, milestone:TC2): generator state objects + StopIteration.
- TODO(type-coverage, owner:frontend, milestone:TC2): comprehension lowering to iterators.
- TODO(type-coverage, owner:frontend, milestone:TC2): builtin iterators (`iter`, `next`, `reversed`, `enumerate`, `zip`, `map`, `filter`).
- TODO(type-coverage, owner:frontend, milestone:TC2): builtin numeric ops (`abs`, `round`, `pow`, `divmod`, `min`, `max`, `sum`).
- TODO(type-coverage, owner:frontend, milestone:TC2): builtin conversions (`int`, `float`, `complex`, `str`, `bool`).
- TODO(type-coverage, owner:frontend, milestone:TC2): async iteration builtins (`aiter`, `anext`).
- TODO(type-coverage, owner:runtime, milestone:TC2): set/frozenset hashing + deterministic ordering.
- TODO(type-coverage, owner:runtime, milestone:TC2): formatting builtins (`repr`, `ascii`, `bin`, `hex`, `oct`, `chr`, `ord`) + full `format` protocol (named fields, format specs, conversion flags).
- TODO(type-coverage, owner:runtime, milestone:TC2): rounding intrinsics (`round`, `floor`, `ceil`, `trunc`) with deterministic semantics.
- TODO(type-coverage, owner:runtime, milestone:TC2): identity builtins (`hash`, `id`, `callable`).
- TODO(type-coverage, owner:frontend, milestone:TC2): iterable unpacking + starred targets.
- TODO(type-coverage, owner:backend, milestone:TC2): generator/iterator state in wasm ABI.
- TODO(type-coverage, owner:frontend, milestone:TC1): type-hint specialization policy (`--type-hints=check` with runtime guards).
- TODO(type-coverage, owner:stdlib, milestone:TC2): `builtins` module parity notes.
- TODO(type-coverage, owner:runtime, milestone:TC3): buffer protocol + memoryview layout.
- TODO(type-coverage, owner:stdlib, milestone:TC3): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
- TODO(type-coverage, owner:stdlib, milestone:TC3): reflection builtins (`type`, `isinstance`, `issubclass`, `getattr`, `setattr`, `hasattr`, `dir`, `vars`, `globals`, `locals`).
- TODO(type-coverage, owner:stdlib, milestone:TC3): dynamic execution builtins (`eval`, `exec`, `compile`) with sandboxing rules.
- TODO(type-coverage, owner:stdlib, milestone:TC3): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(type-coverage, owner:runtime, milestone:TC3): descriptor builtins (`property`, `classmethod`, `staticmethod`, `super`).

Stdlib compatibility TODOs tracked here for CI parity:
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `functools` fast paths (`lru_cache`, `partial`, `reduce`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `itertools` + `operator` core-adjacent intrinsics.
- TODO(stdlib-compat, owner:runtime, milestone:SL1): `array` + `struct` deterministic layouts and packing.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `re` engine + deterministic regex semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `json` parity plan (interop with `molt_json`).
- TODO(stdlib-compat, owner:frontend, milestone:SL2): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `dataclasses` transform (default_factory, kw-only, order, slots) + `__annotations__` propagation.
- TODO(stdlib-compat, owner:runtime, milestone:SL2): `hashlib` deterministic hashing policy.
- TODO(stdlib-compat, owner:runtime, milestone:SL3): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:runtime, milestone:SL3): PyO3 bridge phase 1 (dev-only embedded CPython; capability gate; no production).
- TODO(stdlib-compat, owner:runtime, milestone:SL3): PyO3 bridge phase 2 (embedded CPython feature flag + deterministic denylist + effect contracts).
- TODO(stdlib-compat, owner:runtime, milestone:SL3): PyO3 bridge phase 3 (worker-process default; Arrow IPC/MsgPack/CBOR batching; profiling warnings).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): `typing` runtime helpers + `__annotations__` preservation.

---

## 🛠 Feature Checklist & Implementation Details

### Core Compiler
- [x] Python AST to Molt TIR (Typed IR) Lowering
- [x] Invariant Mining (Stable Class Layouts)
- [x] Monomorphization (Function Specialization)
- [x] Global Data Support (String Constants)
- [x] Position Independent Code (PIC) for macOS/Linux
- [x] WASM backend lowering for `if`/`else` control flow (parity with native backend)
- [ ] Closure Conversion (for lambdas and inner functions)
- [ ] List/Dict Comprehension Lowering
- [x] Full `range()` semantics (start/stop/step + negative ranges; step==0 raises ValueError)
- [ ] Type coverage matrix execution (see `docs/spec/0014_TYPE_COVERAGE_MATRIX.md`)

### Runtime & Performance
- [x] NaN-Boxing (Inline Ints, Bools, None)
- [x] Static Dispatch for Tier 0
- [x] Guarded Dispatch for Tier 1
- [x] External Rust FFI
- [x] List/Dict literals + indexing/assignment (MVP, deterministic dict table + insertion order)
- [x] List/Dict methods (append/pop/get/keys/values) hardening (list growth + dict view types + RC return semantics)
- [x] Tuple literals + tuple hashing/equality for composite dict keys
- [x] Iterator protocol + for-loop lowering for list/tuple/dict views
- [x] Temp arena allocation for parse-time containers (arrays/maps)
- [ ] Canonical loop lowering (counted loops + induction variables)
- [ ] Vectorizable region detection (guarded fast paths with scalar fallback)
- [ ] SIMD kernels for reductions + byte/string scans
- [ ] Production wire codecs (MsgPack/CBOR) as default over JSON
- [x] Loop-scoped RC cleanup for temporary values (dominance-safe dec_ref in control flow)
- [x] Dominance-safe cleanup for non-entry temporaries (block-level cleanup gated by last-use)
- [ ] Bytes semantics beyond literals (ops, comparisons, slicing)
- [ ] Bytes/bytearray find/split/replace fast paths (partial: no empty-sep/maxsplit; str methods pending)
- [ ] Biased Reference Counting (Single-thread optimization)
- [ ] Incremental Mark-and-Sweep GC
- [ ] Zero-copy String passing for WASM

### Concurrency & I/O
- [x] Async/Await Syntax Support
- [x] CPython fallback wrappers for channels/spawn (`molt.channel`, `molt.spawn`)
- [ ] Task-based Concurrency (No GIL)
- [ ] Rust Executor Integration (Tokio/Smol)
- [ ] Native HTTP Package (`molt_http`)
- [ ] Native WebSocket + streaming I/O (`molt_ws` or equivalent)
- [ ] WebSocket host connect hook + capability registry integration
- [ ] Native Database Drivers (`molt_sqlite`, `molt_postgres`)

### Tooling & DX
- [x] `molt build` CLI
- [x] Cross-compilation to WASM
- [x] `molt-diff` Harness (CPython Semantics Matcher)
- [ ] `molt run` (JIT-like execution)
- [ ] SBOM Generation (SPDX/CycloneDX)
- [ ] Integrated Benchmarking Regression Gates

---

## 🔬 Research & Innovation Areas
1. **Semantic Reduction via Invariant Mining:** Automatically identifying which parts of a Python app are "frozen" vs "guarded".
2. **AI-Assisted Guard Synthesis:** Using dev-time traces to generate optimal guards for dynamic sites.
3. **WASM Capability Boundaries:** Defining strict security manifests for third-party Molt Packages.
4. **Deterministic WASM:** Ensuring identical execution for database triggers or smart contracts.

---

*Last Updated: Friday, January 2, 2026 - 19:11 CST*
