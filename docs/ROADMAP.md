# Molt Roadmap: The Evolution of Python

Molt compiles a verified subset of Python into extremely fast, single-file native binaries and WASM. This document tracks our progress from research prototype to production-grade systems runtime.

**Ultimate Goal:** A Go-like developer experience for Python, producing binaries that rival C/Rust in performance and safety, suitable for high-concurrency web services, databases, and data pipelines.

**Source of truth:** This file is the canonical status tracker. For near-term sequencing, see `ROADMAP_90_DAYS.md`. For historical milestone framing, see `docs/spec/areas/process/0006-roadmap.md`.

**Version policy:** Molt targets **Python 3.12+** semantics only. When 3.12/3.13/3.14 diverge, document the chosen target in specs/tests.

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
| **7. Differential Testing** | ✅ Done | 2026-01-02 | Automated verification against CPython 3.12+. |
| **8. True Async Runtime** | ✅ Done | 2026-01-02 | State-machine lowering + Poll-based ABI. |
| **9. Closure Conversion** | ✅ Done | 2026-01-02 | Async locals stored in Task objects. |
| **10. WASM Host Interop** | ✅ Done | 2026-01-02 | Standardized host imports for async/memory. |
| **11. Garbage Collection** | 📅 Backlog | - | RC + Incremental Cycle Detection. |
| **12. Profile-Guided Opt (PGO)** | 🚧 In Progress | - | Profile ingestion/IR plumbing done; feedback-driven specialization in progress. |
| **13. Performance Benchmarking** | ✅ Done | 2026-01-02 | Automated suites vs CPython 3.12+. |
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
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): `try/except/finally` lowering + raise paths.
- Implemented: comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `in`, chained comparisons) + lowering rules.
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
- Implemented: `range()` lowering defers to runtime for non-int-like arguments and raises on step==0 before loop execution.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:missing): complex literal lowering + runtime support.
- TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): async iteration builtins (`aiter`, `anext`).
- TODO(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:missing): async generator lowering and runtime parity (`async def` with `yield`).
- TODO(perf, owner:compiler, milestone:TC2, priority:P1, status:planned): tighten async spill/restore to a CFG-based liveness pass to reduce closure traffic and shrink state_label reload sets.
- TODO(perf, owner:compiler, milestone:TC2, priority:P2, status:planned): optimize wasm trampolines with bulk payload initialization and shared helpers to cut code size and call overhead.
- Implemented: cached task-trampoline eligibility on function headers to avoid per-call attribute lookups.
- Implemented: coroutine trampolines reuse the current cancellation token to avoid per-call token allocations.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): set/frozenset hashing + deterministic ordering.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): formatting builtins (`repr`, `ascii`, `bin`, `hex`, `oct`, `chr`, `ord`) + full `format` protocol (named fields, format specs, conversion flags).
- Implemented: f-string conversion flags (`!r`, `!s`, `!a`) parity (including format specifiers and debug expressions).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): rounding intrinsics (`round`, `floor`, `ceil`, `trunc`) with deterministic semantics.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): identity builtins (`hash`, `id`, `callable`).
- Implemented: iterable unpacking + starred targets for assignment/loop targets with CPython-style error semantics (PEP 3132 + PEP 448 coverage).
- TODO(type-coverage, owner:backend, milestone:TC2, priority:P2, status:planned): generator/iterator state in wasm ABI.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): type-hint specialization policy (`--type-hints=check` with runtime guards).
- TODO(type-coverage, owner:stdlib, milestone:TC2, priority:P2, status:planned): `builtins` module parity notes.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): buffer protocol + memoryview layout.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
- Implemented: runtime-owned sys bootstrap intrinsics now provide deterministic `sys.path`/module-root/env bootstrap fields (`PYTHONPATH`, `MOLT_MODULE_ROOTS`, `VIRTUAL_ENV` site-packages, `PWD` policy), and stdlib shims consume those intrinsic payloads directly.
- Implemented: import-path bootstrap wiring now reaches `runpy`/`importlib.machinery` + `importlib.util` filesystem resolution surfaces via runtime payload intrinsics (`molt_runpy_resolve_path`, `molt_importlib_source_loader_payload`, `molt_importlib_read_file`, `molt_importlib_bootstrap_payload`, `molt_importlib_cache_from_source`, `molt_importlib_find_in_path`, `molt_importlib_spec_from_file_location_payload`) with no Python `os.path` fallback in those code paths.
- Implemented: `importlib.resources` namespace package path discovery and `importlib.metadata` dist-info path scanning now lower through runtime intrinsics (`molt_importlib_namespace_paths`, `molt_importlib_metadata_dist_paths`), and `importlib.metadata` text reads now use intrinsic file IO (`molt_importlib_read_file`) instead of Python-side file-open fallbacks.
- Implemented: `importlib.resources` traversable stat/listdir shaping now lowers through runtime payload intrinsic (`molt_importlib_resources_path_payload`), and `open_text`/`open_binary`/`read_text`/`read_binary` paths use intrinsic-backed reads (`molt_importlib_read_file`) without Python file-open fallback.
- Implemented: `importlib.metadata` name/version/header and entry-point parsing now lower through runtime payload intrinsic (`molt_importlib_metadata_payload`), and aggregated entry-point discovery now lowers through `molt_importlib_metadata_entry_points_payload`, with Python wrappers handling cache/object shaping only.
- Implemented: `importlib.machinery.SourceFileLoader` restricted source execution now lowers through Rust intrinsic (`molt_importlib_exec_restricted_source`) instead of Python-side statement/literal parsing.
- Implemented: `runpy.run_path` now executes via runtime intrinsic (`molt_runpy_run_path`) after bootstrap-aware resolution, removing Python-side `NotImplementedError` fallback for supported source payloads.
- Implemented: traceback exception-chain formatting is runtime-lowered through `molt_traceback_format_exception`; `traceback.py` delegates `format_exception`/`TracebackException.format` chain shaping, `extract_tb` payload shaping, and `TracebackException.from_exception` component extraction (`molt_traceback_exception_components`) to Rust intrinsics.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): project-root builds (package discovery, `__init__` handling, namespace packages, deterministic dependency graph caching).
- Implemented: relative import resolution honors `__package__`/`__spec__` metadata (including `__main__`), namespace packages, and CPython-matching missing/beyond-top-level errors.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): reflection builtins (`type`, `isinstance`, `issubclass`, `getattr`, `setattr`, `hasattr`, `dir`, `vars`, `globals`, `locals`).
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:partial): dynamic execution builtins: `compile` now performs Rust parser-backed syntax/scope validation for `exec`/`eval`/`single` modes and returns a runtime code object, but `eval`/`exec` and full compile codegen/sandboxing remain missing.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): descriptor builtins (`property`, `classmethod`, `staticmethod`, `super`).

Stdlib compatibility TODOs tracked here for CI parity:
Ten-item parity plan details live in `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` (section 3.1).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `functools` fast paths (`lru_cache`, `partial`, `reduce`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `itertools` + `operator` core-adjacent intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` intrinsics + float determinism policy (non-transcendentals covered; trig/log/exp parity pending).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` (`deque`, `Counter`, `defaultdict`) parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `heapq` randomized stress + perf tracking.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `bisect` helpers + fast paths.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): `array` + `struct` deterministic layouts and packing (struct intrinsics cover the CPython 3.12 format table with alignment + half-float support, and C-contiguous nested-memoryview buffer windows; remaining struct gap is exact CPython diagnostic-text parity on selected edge cases).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): advance native `re` engine to full syntax/flags/groups; native engine covers core syntax (literals, `.`, classes/ranges, groups/alternation, greedy + non-greedy quantifiers) and `IGNORECASE`/`MULTILINE`/`DOTALL`; advanced features/flags raise `NotImplementedError` (no host fallback).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `json` parity plan (runtime fast-path + performance tuning + full cls/callback parity).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `enum` parity (aliases, functional API, Flag/IntFlag edge cases).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `random` distributions + extended test vectors.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module API + runtime cycle collector hook.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` ancillary-data parity (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`) while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty control messages.
- Implemented: wasm host ABI plumbs `recvmsg` `msg_flags` end-to-end for `socket.recvmsg`/`socket.recvmsg_into` (no longer hardcoded to `0` in wasm runtime paths).
- logging core implemented (Logger/Handler/Formatter/LogRecord + basicConfig); `logging.config`/`logging.handlers` pending.
- TODO(stdlib-compat, owner:frontend, milestone:SL1, priority:P2, status:planned): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): implement `make_dataclass` once dynamic class construction is allowed.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.
- TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P2, status:planned): `hashlib` deterministic hashing policy.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P1, status:planned): define the minimal `libmolt` C-API subset (buffer, numerics, sequence/mapping, errors, GIL mapping) as the primary C-extension compatibility path.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): extension rebuild pipeline (headers, build helpers, audit tooling) for `libmolt`-compiled wheels.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract (IPC/ABI, capability gating, deterministic denylist for C extensions) as an explicit, opt-in compatibility layer only.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): Bridge phase 1 (worker-process bridge default when enabled; Arrow IPC/MsgPack/CBOR batching; profiling warnings).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): Bridge phase 2 (embedded CPython feature flag + deterministic denylist + effect contracts; never default).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- Implemented: `os.environ` backend is now intrinsic-owned (`molt_env_snapshot`/`molt_env_set`/`molt_env_unset`) and `os.putenv`/`os.unsetenv` are lowered to dedicated runtime intrinsics (`molt_env_putenv`/`molt_env_unsetenv`) with CPython-visible mapping separation.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete socket/select/selectors parity after intrinsic-backed object lowering (`poll`/`epoll`/`kqueue`/`devpoll` + backend selector classes); remaining work is OS-flag/error fidelity, fd inheritance corners, and wasm/browser host parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib meta_path/path_hooks + namespace/extension/zip loader parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.resources loader-backed readers + namespace/zip parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata full parsing + dependency/entry point semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python stdlib modules with Rust intrinsics-only implementations (thin wrappers only); compiled binaries must reject Python-only stdlib modules. See `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): remove `typing` fallback ABC scaffolding and lower protocol/ABC bootstrap helpers into Rust intrinsics-only paths.
- Implemented: `builtins` bootstrap no longer probes host `builtins`; descriptor constructors are intrinsic-backed (`molt_classmethod_new`, `molt_staticmethod_new`, `molt_property_new`) with fail-fast missing-intrinsic behavior.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P0, status:partial): complete concurrency substrate lowering in strict order (`socket`/`select`/`selectors` -> `threading` -> `asyncio`) with intrinsic-only compiled semantics in native + wasm.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): import-only allowlisted stdlib modules (`argparse`, `ast`, `atexit`, `collections.abc`, `_collections_abc`, `_abc`, `_asyncio`, `_bz2`, `_weakref`, `_weakrefset`, `platform`, `queue`, `shlex`, `shutil`, `textwrap`, `time`, `tomllib`, `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `copyreg`, `string`, `numbers`, `unicodedata`, `glob`, `tempfile`, `ctypes`) to minimal parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): pkgutil loader/zipimport/iter_importers parity (filesystem-only iter_modules/walk_packages today).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): compileall/py_compile parity (pyc output, invalidation modes, optimize levels).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): finish abc registry + cache invalidation parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs for regrtest (support: captured_output/captured_stdout/captured_stderr, check_syntax_error, findfile, run_with_tz, warnings_helper utilities: check_warnings/check_no_warnings/check_no_resource_warning/check_syntax_warning/ignore_warnings/import_deprecated/save_restore_warnings_filters/WarningsRecorder, cpython_only, requires, swap_attr/swap_item, import_helper basics: import_module/import_fresh_module/make_legacy_pyc/ready_to_import/frozen_modules/multi_interp_extensions_check/DirsOnSysPath/isolated_modules/modules_setup/modules_cleanup, os_helper basics: temp_dir/temp_cwd/unlink/rmtree/rmdir/make_bad_fd/can_symlink/skip_unless_symlink + TESTFN constants); doctest blocked on eval/exec/compile gating, full unittest parity pending.
- Implemented: `__future__`/`keyword` module bootstrap now loads feature metadata and keyword tables/checks from Rust intrinsics (`molt_future_features`, `molt_keyword_lists`, `molt_keyword_iskeyword`, `molt_keyword_issoftkeyword`), eliminating probe-only status.
- linecache module implemented (`getline`, `getlines`, `checkcache`, `lazycache`) with `fs.read` gating.
- reprlib module implemented (`Repr`, `repr`, `recursive_repr` parity).

## Rust Lowering Program (Core -> Stdlib)
Execution contract: `docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md`

Program board:
- Phase 0 (enforcement spine): completed, keep strict in CI + local lint.
- Phase 1 (core-lane blockers): completed; core lane closure is now intrinsic-backed only.
- Phase 2 (concurrency substrate): active P0 queue (`socket`/`select`/`selectors` -> `threading` -> `asyncio`).
- Phase 3 (core-adjacent stdlib): queued behind phase 2.
- Phase 4 (long tail + capability-gated families): queued behind phase 3.

1. Phase 0 (enforcement spine)
- Keep `tools/check_stdlib_intrinsics.py` as generated-audit + lint gate in CI.
- Keep strict core-lane gate in CI (`tools/check_core_lane_lowering.py`), requiring `intrinsic-backed` status only for the core-lane import closure.
- Keep core differential lane as the first green gate before broader stdlib sweeps.

2. Phase 1 (core-lane blockers, P0)
- Lower core closure bootstrap modules to intrinsic-backed: `types`, `abc` stack (`abc`, `_abc`, `collections.abc`, `_collections_abc`), `weakref` stack (`weakref`, `_weakrefset`), `typing`, `traceback`, `__future__`, and core-used `asyncio` surface.
- Exit only when core-lane closure contains zero `probe-only`, `intrinsic-partial`, and `python-only` modules.

3. Phase 2 (concurrency substrate, P0)
- Lower in dependency order: `socket` -> `threading` -> `asyncio`.
- Require native + wasm differential parity for each module family before promoting to the next.

4. Phase 3 (core-adjacent stdlib, P1)
- Lower `builtins`, `math`, `re`, `struct`, `time`, `inspect`, `functools`, `itertools`, `operator`, `contextlib` to intrinsic-backed status.

5. Phase 4 (capability-gated + long tail, P2/P3)
- Lower import/tooling family (`pathlib`, `importlib.*`, `pkgutil`, `glob`, `shutil`, `py_compile`, `compileall`).
- Lower data/codec family (`json`, `csv`, `pickle`, `enum`, `ipaddress`, encodings family).
- Lower remaining network/process modules (`ssl`, `subprocess`, `concurrent.futures`, remaining `http.*`).

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
- [ ] Cache metaclass rich-compare dispatch for type objects to avoid repeated attribute resolution on hot equality paths (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): cache type comparison dispatch on type objects).
- [ ] Biased Reference Counting (Single-thread optimization) (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): biased RC).
- [ ] Incremental Mark-and-Sweep GC (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): incremental mark-and-sweep GC).
- [ ] Zero-copy String passing for WASM (TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P2, status:planned): zero-copy string passing for WASM).

### Concurrency & I/O
- [x] Async/Await Syntax Support
- [x] Unified Task ABI for futures/generators across native + WASM backends
- [x] Molt-native channel/spawn wrappers (`molt.channel`, `molt.spawn`) with no CPython fallback
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
- [ ] Explicit CPython parity runner (separate from `molt run`) (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add a CPython parity runner distinct from compiled `molt run`).
- [x] SBOM Generation + signing hooks (`molt package` CycloneDX/SPDX sidecars + cosign/codesign).
- [~] Signature verification + trust policy for packaged artifacts (publish/verify enforced; load-time enforcement pending).
  (TODO(tooling, owner:release, milestone:TL2, priority:P2, status:partial): enforce signature verification/trust policy during load.)
- [ ] Integrated Benchmarking Regression Gates (TODO(perf, owner:tooling, milestone:TL2, priority:P2, status:planned): benchmarking regression gates).

---

## 🔬 Research & Innovation Areas
1. **Semantic Reduction via Invariant Mining:** Automatically identifying which parts of a Python app are "frozen" vs "guarded".
2. **WASM Capability Boundaries:** Defining strict security manifests for third-party Molt Packages.
3. **Deterministic WASM:** Ensuring identical execution for database triggers or smart contracts.

---

*Last Updated: Friday, February 6, 2026 - 08:01 CST*
