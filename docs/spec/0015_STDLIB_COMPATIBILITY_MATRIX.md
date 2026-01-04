# Stdlib Compatibility Matrix
**Spec ID:** 0015
**Status:** Draft (implementation-tracking)
**Owner:** stdlib + runtime + frontend
**Goal:** Provide a production-grade, deterministic subset of the CPython standard library with clear import rules and capability gating.

## 0. Principles
- **Explicit imports:** everything outside builtins requires `import` (no implicit injection).
- **Minimal core:** core runtime stays lean; stdlib modules live outside the core unless explicitly promoted.
- **Capability gating:** any OS, I/O, network, or process control requires explicit capability grants.
- **Determinism first:** hashing, ordering, and file/system APIs must preserve deterministic output.
- **Enforcement:** use `molt.capabilities` to check or require capability tokens in stdlib shims.

## 1. Policy: Core vs Import vs Gated
- **Core (always available):** builtins and compiler/runtime intrinsics only.
- **Core-adjacent (import required, fast path):** modules with compiler/runtime intrinsics for hot paths.
- **Stdlib (import required):** pure-Python or Rust-backed modules with deterministic semantics.
- **Capability-gated:** modules that read/write the host or open network/process boundaries.

## 2. Decorators, Dataclasses, and Typing Policy
- **Decorator whitelist:** only approved decorators are allowed in Tier 0 (e.g., `@dataclass`, `@lru_cache`); other decorators must be pure and deterministic or are rejected.
- **Compile-time lowering:** known decorators are lowered into explicit IR (no dynamic `exec`/metaclass evaluation).
- **Typing:** annotations are preserved in `__annotations__` and used at compile time; runtime typing helpers are scoped and deterministic.
- **Type-hints policy:** `--type-hints=trust` applies annotations to lowering; `--type-hints=check` inserts runtime guards.
- **Safety bound:** decorators or typing helpers that mutate globals/types at runtime require explicit opt-in and Tier 1 guards.

## 3. Compatibility Matrix
| Module | Tier | Status | Priority | Milestone | Owner | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| functools | Core-adjacent | Planned | P1 | SL1 | stdlib/runtime | Promote `lru_cache`, `partial`, `reduce` fast paths. |
| itertools | Core-adjacent | Planned | P1 | SL1 | stdlib/runtime | Iterator kernels; tie into vectorization. |
| operator | Core-adjacent | Planned | P1 | SL1 | stdlib/runtime | Low-level op dispatch helpers. |
| math | Core-adjacent | Planned | P1 | SL1 | stdlib/runtime | SIMD-friendly numeric intrinsics. |
| collections | Stdlib | Planned | P1 | SL1 | stdlib | `deque`, `Counter`, `defaultdict`. |
| heapq | Stdlib | Planned | P1 | SL1 | stdlib | Heap primitives. |
| bisect | Stdlib | Planned | P1 | SL1 | stdlib | Binary search helpers. |
| array | Stdlib | Planned | P1 | SL1 | runtime | Typed array storage, interop-ready. |
| struct | Stdlib | Planned | P1 | SL1 | runtime | Binary packing for codecs/I/O. |
| re | Stdlib | Planned | P1 | SL2 | stdlib | Deterministic regex engine. |
| decimal | Stdlib | Planned | P2 | SL2 | stdlib | Precise decimal math. |
| fractions | Stdlib | Planned | P2 | SL2 | stdlib | Rational arithmetic. |
| statistics | Stdlib | Planned | P2 | SL2 | stdlib | Basic stats kernels. |
| random | Stdlib | Planned | P2 | SL2 | stdlib | Deterministic RNG seeds. |
| datetime | Stdlib | Planned | P2 | SL2 | stdlib | Time types + parsing. |
| zoneinfo | Stdlib | Planned | P3 | SL3 | stdlib | Timezone data handling. |
| pathlib | Stdlib | Planned | P2 | SL2 | stdlib | Path abstraction (gated I/O). |
| enum | Stdlib | Planned | P2 | SL2 | stdlib | Enum base types. |
| dataclasses | Stdlib | Partial | P2 | SL2 | stdlib | Dataclass lowering (frozen/eq/repr/field order); defaults/kw-only/slots pending. |
| typing | Stdlib | Planned | P3 | SL3 | stdlib | Runtime typing helpers + annotations. |
| abc | Stdlib | Planned | P3 | SL3 | stdlib | Abstract base classes. |
| contextlib | Stdlib | Partial | P2 | SL2 | stdlib | `nullcontext` + `closing` lowered; `contextmanager` pending. |
| weakref | Stdlib | Planned | P3 | SL3 | runtime | Weak references (GC-aware). |
| logging | Stdlib | Planned | P2 | SL2 | stdlib | Structured logging; gated sinks. |
| json | Stdlib | Planned | P2 | SL2 | stdlib | Keep `molt_json` as fast-path. |
| csv | Stdlib | Planned | P3 | SL3 | stdlib | Deterministic CSV parsing. |
| io | Capability-gated | Partial | P2 | SL3 | stdlib | Native `open`/`read`/`write`/`close` with `fs.read`/`fs.write` gating; streams pending. |
| os | Capability-gated | Planned | P2 | SL3 | stdlib | Filesystem and env access. |
| sys | Capability-gated | Planned | P2 | SL3 | stdlib | Runtime info + argv. |
| subprocess | Capability-gated | Planned | P3 | SL3 | stdlib | Process spawn control. |
| socket | Capability-gated | Planned | P2 | SL3 | stdlib | Network sockets. |
| ssl | Capability-gated | Planned | P3 | SL3 | stdlib | TLS primitives. |
| asyncio | Capability-gated | Planned | P2 | SL3 | stdlib/runtime | Align with Molt async runtime. |
| selectors | Capability-gated | Planned | P3 | SL3 | stdlib | Event loop primitives. |
| threading | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Thread model integration. |
| multiprocessing | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Process model integration. |
| concurrent.futures | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Executor interfaces. |
| sqlite3 | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | DB integration. |
| http | Capability-gated | Planned | P3 | SL3 | stdlib | HTTP parsing/client. |
| urllib | Capability-gated | Planned | P3 | SL3 | stdlib | URL parsing + I/O. |
| email | Stdlib | Planned | P3 | SL3 | stdlib | Email parsing. |
| gzip/bz2/lzma | Stdlib | Planned | P3 | SL3 | stdlib | Compression modules. |
| zipfile/tarfile | Stdlib | Planned | P3 | SL3 | stdlib | Archive tooling. |
| hashlib/hmac | Stdlib | Planned | P2 | SL2 | stdlib/runtime | Hashing primitives (deterministic). |
| secrets | Stdlib | Planned | P3 | SL3 | stdlib | Crypto RNG (gated). |
| uuid | Stdlib | Planned | P2 | SL2 | stdlib | UUID generation (deterministic seed). |

### 3.1 Ten-Item Stdlib Parity Plan (Fully Specced)
Scope is based on the compatibility matrix and current stubs in `src/molt/stdlib/`.

#### 3.1.1 functools (Core-adjacent)
- Goal: Provide high-performance `lru_cache`, `partial`, `reduce` with deterministic behavior; add compiler/runtime fast paths.
- Scope: `functools.lru_cache`, `functools.partial`, `functools.reduce`; public API parity for basic usage (no weakref, no C ext).
- Semantics: Deterministic eviction (LRU), strict argument normalization; `partial` preserves func/args/keywords. `reduce` matches CPython edge cases (empty input, initializer).
- Runtime/IR: Add intrinsics for cache key hashing and lookup. Cache entries use deterministic hash policy. Lower `@lru_cache` at compile time into cache wrapper.
- Frontend: Decorator whitelist update and lowering in `src/molt/frontend/__init__.py`.
- Tests: Unit tests for argument order, eviction, `cache_info`, `cache_clear`; differential tests for `reduce` behavior.
- Docs: Update stdlib matrix: `functools` to Partial/SL1; note deterministic hashing policy.
- Acceptance: 95% parity for covered APIs, no nondeterministic behavior; all tests green.

#### 3.1.2 itertools (Core-adjacent)
- Goal: Implement core iterator kernels and tie into vectorization where possible.
- Scope: `chain`, `islice`, `product`, `permutations`, `combinations`, `groupby`, `repeat`.
- Semantics: Iterator protocol compliance; no eager materialization; deterministic iteration order identical to CPython.
- Runtime/IR: Add iterator ops for product/permutations and shared iteration state; optimize repeat/islice to avoid allocations.
- Frontend: Recognize common patterns and lower to specialized ops when safe.
- Tests: Differential tests for each iterator; edge cases (empty input, step, large lengths).
- Docs: Update stdlib matrix to Partial/SL1 and add usage note.
- Acceptance: Parity for covered APIs, memory behavior within 10% of CPython on small inputs.

#### 3.1.3 operator (Core-adjacent)
- Goal: Provide low-overhead operator helpers.
- Scope: `itemgetter`, `attrgetter`, `methodcaller`, plus basic `add`, `mul`, `eq` wrappers.
- Semantics: Attribute resolution uses Molt rules but mirrors CPython error messages for missing attributes/keys.
- Runtime/IR: Implement `GETATTR_FAST`, `GETITEM_FAST` ops for getter functions; cache attribute lookups when safe.
- Tests: Differential tests for attribute/key errors and tuple/multi-getter behavior.
- Docs: Matrix update: `operator` Partial/SL1.
- Acceptance: No semantic regressions; simple getters 2x faster than Python fallback.

#### 3.1.4 math (Core-adjacent)
- Goal: Deterministic numeric intrinsics with CPython edge-case behavior.
- Scope: `sqrt`, `sin`, `cos`, `log`, `exp`, `floor`, `ceil`, `isfinite`, `isnan`, `isinf`, `prod`, `sum` fast paths.
- Semantics: IEEE-754 behavior; handle NaN/inf exactly like CPython where possible.
- Runtime/IR: Use Rust libm or platform intrinsics; guard on float types; integer fast paths.
- Tests: Differential tests for edge inputs; property tests for invariants (e.g., `isfinite`).
- Docs: Matrix update: `math` Partial/SL1; note floating point determinism policy.
- Acceptance: Parity on core functions; no UB; stable outputs across runs.

#### 3.1.5 collections (Stdlib)
- Goal: Implement `deque`, `Counter`, `defaultdict`.
- Scope: Basic operations and methods; omit advanced views initially.
- Semantics: `deque` O(1) append/pop; `Counter` arithmetic and `most_common`.
- Runtime/IR: New runtime types for `deque`; `Counter` backed by dict with deterministic ordering.
- Tests: Unit + differential tests; deterministic iteration and repr.
- Docs: Matrix update: `collections` Partial/SL1.
- Acceptance: Behavior parity for common methods; deterministic iteration.

#### 3.1.6 heapq (Stdlib)
- Goal: Provide heap operations used widely in algorithms.
- Scope: `heapify`, `heappush`, `heappop`, `heapreplace`, `heappushpop`, `nlargest`, `nsmallest`.
- Semantics: Ordering matches CPython; stable for equal elements when applicable.
- Runtime/IR: Use list-backed binary heap; expose efficient ops to frontend.
- Tests: Differential tests with random inputs; heap invariants.
- Docs: Matrix update: `heapq` Partial/SL1.
- Acceptance: Correctness on randomized stress, no regressions in benchmarks.

#### 3.1.7 bisect (Stdlib)
- Goal: Provide deterministic binary search helpers.
- Scope: `bisect_left`, `bisect_right`, `insort_left`, `insort_right`.
- Semantics: Comparable to CPython for list-like sequences; stable insertion.
- Runtime/IR: Optional fast path on list/array types.
- Tests: Differential tests across sorted inputs, edge cases with duplicates.
- Docs: Matrix update: `bisect` Partial/SL1.
- Acceptance: Parity for covered functions; perf within 1.5x CPython.

#### 3.1.8 array (Stdlib)
- Goal: Typed array storage with deterministic layout and buffer interop.
- Scope: `array('b','i','f','d', ...)`, basic operations, slicing, `.tobytes()`.
- Semantics: Endianness and item size consistent with CPython for supported types.
- Runtime/IR: Add array object to runtime with explicit layout; implement buffer protocol for interop (future).
- Tests: Unit + differential; pack/unpack round-trip; size/endianness.
- Docs: Matrix update: `array` Partial/SL1; note supported typecodes.
- Acceptance: Parity for supported types; deterministic bytes output.

#### 3.1.9 struct (Stdlib)
- Goal: Binary pack/unpack for codecs/I/O.
- Scope: `pack`, `unpack`, `calcsize`, basic format strings (`bBhHiIlLqQfd`, endianness).
- Semantics: Strict format parsing; error messages align with CPython.
- Runtime/IR: Implement parser + packer in Rust for speed; expose to Python layer.
- Tests: Differential tests for formats; fuzz small formats; exact byte matches.
- Docs: Matrix update: `struct` Partial/SL1; supported format list.
- Acceptance: Parity on supported formats; deterministic output.

#### 3.1.10 re (Stdlib)
- Goal: Deterministic regex engine with core CPython semantics.
- Scope: `compile`, `match`, `search`, `findall`, `sub`, `split` and flags `IGNORECASE`, `MULTILINE`, `DOTALL`.
- Semantics: No backtracking nondeterminism; deterministic error handling.
- Runtime/IR: Use Rust regex crate or custom engine with explicit semantics; ensure Python-compatible groups.
- Tests: Differential tests for common patterns; coverage for groups/flags; backtracking stress tests.
- Docs: Matrix update: `re` Partial/SL2; note unsupported features.
- Acceptance: Parity for supported features; deterministic runtime.

### 3.2 Cross-cutting Requirements
- Update `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` and `docs/ROADMAP.md` as each module lands.
- Add `TODO(stdlib-compat, ...)` markers for interim gaps.
- Ensure capability gating for I/O-adjacent pieces (if introduced).
- Add differential tests in `tests/differential/` as required.

## 4. Milestones
- **SL1:** core-adjacent and data-structure modules (functools/itertools/operator/math + collections/heapq/bisect/array/struct).
- **SL2:** regex + time + numeric libs + logging + json + hashing.
- **SL3:** I/O/network/system modules with capability gating and stronger sandboxing.

## 5. Capability Tokens (appendix)
Modules that touch the host require explicit capabilities. Tokens are additive and must be declared by the binary.

| Capability | Scope | Example modules |
| --- | --- | --- |
| `fs.read` | Read-only filesystem access | `io`, `pathlib`, `os`, `open` |
| `fs.write` | Write filesystem access | `io`, `pathlib`, `os`, `open` |
| `env.read` | Environment variable read | `os`, `sys` |
| `env.write` | Environment variable write | `os` |
| `net.outbound` | Outbound network access | `socket`, `http`, `urllib`, `ssl` |
| `net.listen` | Bind/listen access | `socket`, `asyncio` |
| `proc.spawn` | Process creation | `subprocess`, `multiprocessing` |
| `time.wall` | Real time access | `datetime`, `time` |
| `rand.secure` | Cryptographic RNG | `secrets`, `ssl` |
| `ffi.unsafe` | Native extension/FFI | `ctypes` |

## 6. TODOs (tracked in ROADMAP.md)
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `functools` fast paths (`lru_cache`, `partial`, `reduce`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `itertools` + `operator` core-adjacent intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `math` intrinsics + float determinism policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `collections` (`deque`, `Counter`, `defaultdict`) parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `heapq` primitives + invariants.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1): `bisect` helpers + fast paths.
- TODO(stdlib-compat, owner:runtime, milestone:SL1): `array` + `struct` deterministic layouts and packing.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `re` engine + deterministic regex semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `json` parity plan (interop with `molt_json`).
- TODO(stdlib-compat, owner:frontend, milestone:SL1): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:frontend, milestone:SL2): `contextlib.contextmanager` lowering (generator-based context managers).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `dataclasses` transform (default_factory, kw-only, order, slots, `__annotations__`).
- TODO(stdlib-compat, owner:runtime, milestone:SL2): `hashlib` deterministic hashing policy.
- TODO(stdlib-compat, owner:runtime, milestone:SL3): expand `io` to buffered/text wrappers and streaming helpers.
- TODO(stdlib-compat, owner:runtime, milestone:SL3): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): `typing` runtime helpers + `__annotations__` preservation.
