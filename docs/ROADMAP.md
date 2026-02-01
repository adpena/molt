# Molt Roadmap: The Evolution of Python

Molt compiles a verified subset of Python into extremely fast, single-file native binaries and WASM. This document tracks our progress from research prototype to production-grade systems runtime.

**Ultimate Goal:** A Go-like developer experience for Python, producing binaries that rival C/Rust in performance and safety, suitable for high-concurrency web services, databases, and data pipelines.

**Source of truth:** This file is the canonical status tracker. For near-term sequencing, see `ROADMAP_90_DAYS.md`. For historical milestone framing, see `docs/spec/areas/process/0006-roadmap.md`.

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
**Tracking doc:** `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md`

| Milestone | Focus | Owners | Status | Notes |
| :--- | :--- | :--- | :--- | :--- |
| **TC1** | Exceptions + full container semantics + range/slice polish | runtime, frontend, tests | 🚧 In Progress | TODO(type-coverage, owner:runtime, milestone:TC1, priority:P1, status:partial): exception object model + raise/try. |
| **TC2** | set/frozenset + generators/coroutines + callables | runtime, frontend, backend | 📅 Planned | TODO(type-coverage, owner:backend, milestone:TC2, priority:P2, status:planned): wasm ABI for generator state. |
| **TC3** | memoryview + type/object + modules/descriptors | runtime, stdlib | 📅 Planned | TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): module object + import rules. |

Type coverage TODOs tracked here for CI parity:
- TODO(semantics, owner:runtime, milestone:TC1, priority:P0, status:planned): audit negative-indexing parity across indexable types + add differential coverage for error messages.
- TODO(perf, owner:runtime, milestone:TC1, priority:P2, status:planned): avoid list_snapshot allocations in membership/count/index by using a list mutation version or iterator guard.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): `try/except/finally` lowering + raise paths.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `in`, chained comparisons) + lowering rules.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): builtin reductions (`sum/min/max`) and `len` parity.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): builtin constructors for `tuple`, `dict`, `bytes`, `bytearray`.
- TODO(type-coverage, owner:runtime, milestone:TC1, priority:P1, status:partial): exception objects + stack trace capture.
- TODO(type-coverage, owner:runtime, milestone:TC1, priority:P1, status:partial): recursion limits + `RecursionError` guard semantics.
- TODO(type-coverage, owner:tests, milestone:TC1, priority:P1, status:planned): add exception + set coverage to molt_diff.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): generator state objects + StopIteration.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): comprehension lowering to iterators.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): builtin iterators (`iter`, `next`, `reversed`, `enumerate`, `zip`, `map`, `filter`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): builtin numeric ops (`abs`, `round`, `pow`, `divmod`, `min`, `max`, `sum`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): builtin conversions (`int`, `float`, `complex`, `str`, `bool`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:partial): `int()` keyword arguments (`x`, `base`) parity.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:partial): lower unsupported `range()` arguments (e.g., oversized ints) to runtime parity errors.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:missing): complex literal lowering + runtime support.
- TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.
- TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P1, status:partial): expand errno constants + errorcode mapping to full CPython table.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): async iteration builtins (`aiter`, `anext`).
- TODO(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:missing): async generator lowering and runtime parity (`async def` with `yield`).
- TODO(perf, owner:compiler, milestone:TC2, priority:P1, status:planned): tighten async spill/restore to a CFG-based liveness pass to reduce closure traffic and shrink state_label reload sets.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): set/frozenset hashing + deterministic ordering.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): formatting builtins (`repr`, `ascii`, `bin`, `hex`, `oct`, `chr`, `ord`) + full `format` protocol (named fields, format specs, conversion flags).
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): f-string conversion flags (`!r`, `!s`, `!a`) parity.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): rounding intrinsics (`round`, `floor`, `ceil`, `trunc`) with deterministic semantics.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): identity builtins (`hash`, `id`, `callable`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): iterable unpacking + starred targets.
- TODO(type-coverage, owner:backend, milestone:TC2, priority:P2, status:planned): generator/iterator state in wasm ABI.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): type-hint specialization policy (`--type-hints=check` with runtime guards).
- TODO(type-coverage, owner:stdlib, milestone:TC2, priority:P2, status:planned): `builtins` module parity notes.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): buffer protocol + memoryview layout.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): project-root builds (package discovery, `__init__` handling, namespace packages, deterministic dependency graph caching).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): relative imports (explicit and implicit) with deterministic package resolution.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): reflection builtins (`type`, `isinstance`, `issubclass`, `getattr`, `setattr`, `hasattr`, `dir`, `vars`, `globals`, `locals`).
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:partial): dynamic execution builtins: `compile` performs global/nonlocal checks and returns a stub code object; `eval`/`exec` and full compile (sandbox + codegen) remain missing.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): descriptor builtins (`property`, `classmethod`, `staticmethod`, `super`).

Stdlib compatibility TODOs tracked here for CI parity:
Ten-item parity plan details live in `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` (section 3.1).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `functools` fast paths (`lru_cache`, `partial`, `reduce`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `itertools` + `operator` core-adjacent intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `math` intrinsics + float determinism policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` (`deque`, `Counter`, `defaultdict`) parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `heapq` randomized stress + perf tracking.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `bisect` helpers + fast paths.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): `array` + `struct` deterministic layouts and packing.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full `re` syntax/flags + group semantics (literal-only `search`/`match`/`fullmatch` are supported).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `json` parity plan (interop with `molt_json`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `random` module API + CPython-compatible RNG parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module API + runtime cycle collector hook.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.
- logging core implemented (Logger/Handler/Formatter/LogRecord + basicConfig); `logging.config`/`logging.handlers` pending.
- TODO(stdlib-compat, owner:frontend, milestone:SL1, priority:P2, status:planned): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `dataclasses` transform (kw-only, order, richer `__init__` signatures) + `__annotations__` propagation.
- TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P2, status:planned): `hashlib` deterministic hashing policy.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): PyO3 bridge phase 1 (dev-only embedded CPython; capability gate; no production).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): PyO3 bridge phase 2 (embedded CPython feature flag + deterministic denylist + effect contracts).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): PyO3 bridge phase 3 (worker-process default; Arrow IPC/MsgPack/CBOR batching; profiling warnings).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): os.environ parity (mapping methods + backend).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): socket/select shims expose error classes only; implement full capability-gated APIs.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): import-only allowlisted stdlib modules (`argparse`, `ast`, `atexit`, `collections.abc`, `importlib`, `platform`, `queue`, `shlex`, `shutil`, `textwrap`, `time`, `tomllib`, `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pprint`, `string`, `numbers`, `unicodedata`, `glob`, `tempfile`, `ctypes`) to minimal parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs for regrtest (support: captured_output/captured_stdout/captured_stderr, warnings_helper.check_warnings, cpython_only, requires, swap_attr/swap_item, import_helper.import_module/import_fresh_module, os_helper.temp_dir/unlink); doctest blocked on eval/exec/compile gating, full unittest parity pending.

---

## 🧩 Language Features Roadmap
**Goal:** Sequence the language/runtime features that unlock file I/O, context managers, and Python class patterns without sacrificing determinism.

| Milestone | Focus | Owners | Status | Notes |
| :--- | :--- | :--- | :--- | :--- |
| **LF1** | Exceptions + context manager protocol | runtime, frontend | 🚧 In Progress | TODO(semantics, owner:runtime, milestone:LF1, priority:P1, status:partial): exception objects + last-exception plumbing. |
| **LF2** | Classes, inheritance, descriptors, factory patterns | runtime, frontend, stdlib | 📅 Planned | TODO(type-coverage, owner:runtime, milestone:LF2, priority:P2, status:planned): type/object + MRO + descriptor protocol. |
| **LF3** | Capability-gated file I/O + pathlib | stdlib, runtime | 📅 Planned | TODO(stdlib-compat, owner:stdlib, milestone:LF3, priority:P2, status:planned): io/pathlib stubs + capability enforcement. |

Language feature TODOs tracked here for parity:
- TODO(syntax, owner:frontend, milestone:LF1, priority:P1, status:partial): `with` lowering for async/multi-context managers + try/finally lowering in IR.
- TODO(semantics, owner:runtime, milestone:LF1, priority:P1, status:partial): exception propagation + suppression semantics for context manager exit paths.
- TODO(stdlib-compat, owner:stdlib, milestone:LF1, priority:P1, status:missing): `contextlib.contextmanager` lowering and generator-based manager support.
- TODO(type-coverage, owner:runtime, milestone:LF2, priority:P2, status:planned): `type`/`object` layout, `isinstance`/`issubclass`.
- TODO(type-coverage, owner:runtime, milestone:LF2, priority:P2, status:planned): descriptor builtins (`property`, `classmethod`, `staticmethod`, `super`).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:planned): method-binding safety pass (guard/deopt on method lookup + cache invalidation rules for call binding).
- TODO(syntax, owner:frontend, milestone:LF2, priority:P2, status:planned): class lowering for `__init__` and factory classmethods (dataclass defaults now wired in stdlib).
- TODO(stdlib-compat, owner:stdlib, milestone:LF3, priority:P2, status:planned): expand `io`/`pathlib` to buffered + streaming wrappers with capability gates.

---

## 🛠 Feature Checklist & Implementation Details

### Core Compiler
- [x] Python AST to Molt TIR (Typed IR) Lowering
- [x] Invariant Mining (Stable Class Layouts)
- [x] Monomorphization (Function Specialization)
- [x] Global Data Support (String Constants)
- [x] Position Independent Code (PIC) for macOS/Linux
- [x] WASM backend lowering for `if`/`else` control flow (parity with native backend)
- [x] Closure Conversion (for lambdas and inner functions)
- [x] List/Dict Comprehension Lowering
- [x] Full `range()` semantics (start/stop/step + negative ranges; step==0 raises ValueError)
- [ ] Type coverage matrix execution (see `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md`) (TODO(type-coverage, owner:tests, milestone:TC2, priority:P2, status:partial): execute matrix end-to-end).

### Runtime & Performance
- [x] NaN-Boxing (Inline Ints, Bools, None)
- [x] Static Dispatch for Tier 0
- [x] Guarded Dispatch for Tier 1
- [x] External Rust FFI
- [x] List/Dict literals + indexing/assignment (MVP, deterministic dict table + insertion order)
- [x] List/Dict methods (append/pop/get/keys/values) hardening (list growth + dict view types + RC return semantics)
- [x] PEP 584 dict union + PEP 604 union types + zip(strict) (PEP 618) parity
- [x] Tuple literals + tuple hashing/equality for composite dict keys
- [x] Iterator protocol + for-loop lowering for list/tuple/dict views
- [x] Temp arena allocation for parse-time containers (arrays/maps)
- [ ] Canonical loop lowering (counted loops + induction variables) (TODO(compiler, owner:compiler, milestone:RT2, priority:P2, status:planned): canonical loop lowering).
- [ ] Vectorizable region detection (guarded fast paths with scalar fallback) (TODO(perf, owner:compiler, milestone:RT2, priority:P2, status:planned): vectorizable region detection).
- [ ] SIMD kernels for reductions + byte/string scans (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:partial): SIMD kernels for reductions + scans).
- [ ] Production wire codecs (MsgPack/CBOR) as default over JSON (TODO(packaging, owner:tooling, milestone:SL2, priority:P2, status:partial): default wire codecs to MsgPack/CBOR).
- [x] Loop-scoped RC cleanup for temporary values (dominance-safe dec_ref in control flow)
- [x] Dominance-safe cleanup for non-entry temporaries (block-level cleanup gated by last-use)
- [ ] Bytes semantics beyond literals (ops, comparisons, slicing) (TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): bytes semantics beyond literals).
- [ ] Bytes/bytearray find/split/replace fast paths (partial: no empty-sep/maxsplit; str methods pending) (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:partial): bytes/bytearray fast paths).
- [ ] Sharded/lock-free handle resolution + pointer registry lock-scope reduction (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): reduce handle/registry lock scope and measure lock-sensitive benchmarks).
- [ ] Biased Reference Counting (Single-thread optimization) (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): biased RC).
- [ ] Incremental Mark-and-Sweep GC (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): incremental mark-and-sweep GC).
- [ ] Zero-copy String passing for WASM (TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P2, status:planned): zero-copy string passing for WASM).

### Concurrency & I/O
- [x] Async/Await Syntax Support
- [x] Unified Task ABI for futures/generators across native + WASM backends
- [x] CPython fallback wrappers for channels/spawn (`molt.channel`, `molt.spawn`)
- [ ] Task-based Concurrency (No GIL) (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): task-based concurrency).
- [ ] Per-runtime GIL strategy + runtime instance ownership model (TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define per-runtime GIL strategy and runtime instance ownership model).
- [ ] PyToken enforcement across runtime mutation entrypoints (TODO(concurrency, owner:runtime, milestone:RT2, priority:P1, status:partial): thread PyToken through runtime mutation entrypoints).
- [~] Process model integration for `multiprocessing`/`subprocess`/`concurrent.futures` (capability-gated spawn, IPC primitives, worker lifecycle). Spawn-based `multiprocessing` is now partial; `fork`/`forkserver` map to spawn semantics and need true fork support; `subprocess`/`concurrent.futures` remain pending.
  (TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.)
  (TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures`.)
- [ ] Rust Executor Integration (Tokio/Smol) (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): executor integration).
- [ ] Native HTTP Package (`molt_http`) (TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:missing): native HTTP package).
- [ ] Native WebSocket + streaming I/O (`molt_ws` or equivalent) (TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:missing): native WebSocket + streaming I/O).
- [ ] WebSocket host connect hook + capability registry integration (TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:planned): WebSocket host connect hook + capability registry).
- [ ] Native Database Drivers (`molt_sqlite`, `molt_postgres`) (TODO(db, owner:runtime, milestone:DB2, priority:P1, status:partial): native database drivers).

### Tooling & DX
- [x] `molt build` CLI
- [x] Cross-compilation to WASM
- [x] `molt-diff` Harness (CPython Semantics Matcher)
- [ ] `molt run` (JIT-like execution) (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:partial): `molt run` JIT-like execution).
- [ ] SBOM Generation (SPDX/CycloneDX) (TODO(tooling, owner:release, milestone:TL2, priority:P2, status:planned): SBOM generation).
- [ ] Integrated Benchmarking Regression Gates (TODO(perf, owner:tooling, milestone:TL2, priority:P2, status:planned): benchmarking regression gates).

---

## 🔬 Research & Innovation Areas
1. **Semantic Reduction via Invariant Mining:** Automatically identifying which parts of a Python app are "frozen" vs "guarded".
2. **WASM Capability Boundaries:** Defining strict security manifests for third-party Molt Packages.
3. **Deterministic WASM:** Ensuring identical execution for database triggers or smart contracts.

---

*Last Updated: Friday, January 30, 2026 - 05:22 CST*
