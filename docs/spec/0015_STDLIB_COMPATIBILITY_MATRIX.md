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
- **Import allowlist:** modules listed in the matrix are importable; missing implementations load empty stubs for dependency tracking.
- **Import-only stubs:** allowlisted modules may load as empty module objects; attribute access raises `AttributeError` and signals missing coverage rather than crashing the compiler.

## 0.1 Tier-0 Direct-Call Rule
- **Direct-call allowlist:** Tier 0 compiles module-level calls to a static `CALL` only for allowlisted functions from allowlisted modules (this matrix + frontend allowlist).
- **No monkey-patching:** rebinding or monkey-patching allowlisted functions is not observed in Tier 0; the call target is fixed at compile time.
- **Fallbacks:** non-allowlisted module-level calls raise a compiler error unless `--fallback=bridge` is enabled, in which case a bridge warning is emitted.
- **Warnings control:** set `MOLT_COMPAT_WARNINGS=0` to suppress bridge warnings during compilation.

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
| builtins | Core | Partial | P0 | SL1 | runtime/frontend | Importable module binds supported builtins (including function objects for allowlisted builtins); missing names raise `AttributeError`. |
| functools | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | `partial`, `reduce`, `lru_cache`, `wraps` shim; `partial`/`lru_cache` accept `*args`/`**kwargs` (no fast paths yet). |
| itertools | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | `chain`, `islice`, `repeat` shim; vectorized kernels pending. |
| operator | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | Basic helpers (`add`, `mul`, `eq`, `itemgetter`, `attrgetter`, `methodcaller`). |
| math | Core-adjacent | Planned | P1 | SL1 | stdlib/runtime | SIMD-friendly numeric intrinsics. |
| collections | Stdlib | Partial | P1 | SL1 | stdlib | `deque`, `Counter`, `defaultdict` basics (wrapper shims; `Counter`/`defaultdict` are not dict subclasses). |
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
| dataclasses | Stdlib | Partial | P2 | SL2 | stdlib | Dataclass lowering (frozen/eq/repr/field order, slots flag); defaults/kw-only/order/default_factory pending. |
| typing | Stdlib | Partial | P3 | SL3 | stdlib | Minimal shim: `Any`/`Union`/`Optional`/`Callable` + `TypeVar`/`Generic`/`Protocol` + `cast`/`get_origin`/`get_args`. |
| abc | Stdlib | Planned | P3 | SL3 | stdlib | Abstract base classes. |
| contextlib | Stdlib | Partial | P2 | SL2 | stdlib | `nullcontext` + `closing` lowered; `contextmanager` pending. |
| contextvars | Stdlib | Partial | P2 | SL3 | stdlib/runtime | `ContextVar`/`Token`/`Context` + `copy_context`; task context propagation via cancel tokens; `Context.run` implemented. |
| weakref | Stdlib | Planned | P3 | SL3 | runtime | Weak references (GC-aware). |
| logging | Stdlib | Planned | P2 | SL2 | stdlib | Structured logging; gated sinks. |
| json | Stdlib | Planned | P2 | SL2 | stdlib | Keep `molt_json` as fast-path. |
| csv | Stdlib | Planned | P3 | SL3 | stdlib | Deterministic CSV parsing. |
| io | Capability-gated | Partial | P2 | SL3 | stdlib | Native `open`/`read`/`write`/`close` with `fs.read`/`fs.write` gating; streams pending. |
| os | Capability-gated | Partial | P2 | SL3 | stdlib | Minimal shim: env access gated via `env.read`/`env.write`; path helpers only. |
| sys | Capability-gated | Partial | P2 | SL3 | stdlib | Minimal shim: argv/version/path/modules + recursion limits; host info gated via `env.read`. |
| errno | Stdlib | Planned | P2 | SL2 | stdlib | Errno constants; import-only allowlist stub. |
| stat | Stdlib | Planned | P3 | SL3 | stdlib | Stat constants; import-only allowlist stub. |
| signal | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Signal handling; gated by `process.signal`. |
| select | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | I/O multiplexing; gated by `io.poll`/`net.poll`. |
| site | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; path config gated via `env.read`/`fs.read`. |
| sysconfig | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; host/path data gated via `env.read`/`fs.read`. |
| subprocess | Capability-gated | Planned | P3 | SL3 | stdlib | Process spawn control. |
| socket | Capability-gated | Planned | P2 | SL3 | stdlib | Network sockets. |
| ssl | Capability-gated | Planned | P3 | SL3 | stdlib | TLS primitives. |
| asyncio | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | Shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `create_task`/`ensure_future`/`current_task`, `Event`, `wait_for`, and basic `gather`; advanced loop APIs and I/O adapters pending. |
| selectors | Capability-gated | Planned | P3 | SL3 | stdlib | Event loop primitives. |
| threading | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Minimal `Thread` stub (start/join) for import compatibility; runtime thread model integration pending. |
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
| base64 | Stdlib | Planned | P2 | SL2 | stdlib | Import-only allowlist stub; encoding/decoding parity pending. |
| binascii | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; low-level codecs pending. |
| pickle | Stdlib | Planned | P2 | SL2 | stdlib | Import-only allowlist stub; deterministic serialization pending. |
| unittest | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; runner/assertions parity pending. |
| argparse | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. |
| getopt | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. |
| locale | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; locale data pending. |
| ast | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; AST API parity pending. |
| atexit | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; exit handler semantics pending. |
| collections.abc | Stdlib | Partial | P3 | SL3 | stdlib | Import-only stub with minimal ABC shells; registration pending. |
| importlib | Stdlib | Partial | P3 | SL3 | stdlib | Import-only stub; dynamic import hooks pending. |
| importlib.util | Stdlib | Partial | P3 | SL3 | stdlib | Import-only stub; loader helpers pending. |
| queue | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; threading integration pending. |
| shlex | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; POSIX parsing parity pending. |
| shutil | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read`/`fs.write` gating. |
| textwrap | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; wrapping/indent parity pending. |
| time | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; `time.wall` gating. |
| tomllib | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parsing parity pending. |
| platform | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; host info gated. |
| ctypes | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; `ffi.unsafe` gating. |
| warnings | Stdlib | Partial | P3 | SL3 | stdlib | `warn` + filters/once/capture shim; regex filters + full registry pending. |
| traceback | Stdlib | Partial | P3 | SL3 | stdlib | `format_exc`/`format_tb`/`print_exception`; rich stack/frame formatting pending. |
| types | Stdlib | Partial | P3 | SL3 | stdlib | `SimpleNamespace` + mapping proxy shim; full type helpers pending. |
| inspect | Stdlib | Partial | P3 | SL3 | stdlib | `getdoc`/`cleandoc` + `signature` (Molt metadata/`__code__`); broader introspection pending. |
| tempfile | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read`/`fs.write` gating. |
| glob | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read` gating. |
| fnmatch | Stdlib | Partial | P3 | SL3 | stdlib | `*`/`?` + bracket class/range matching; escape semantics pending. |
| copy | Stdlib | Partial | P3 | SL3 | stdlib | Shallow/deep copy helpers + `__copy__`/`__deepcopy__` hooks. |
| pprint | Stdlib | Partial | P3 | SL3 | stdlib | `pprint`/`pformat` wrapper; layout rules pending. |
| string | Stdlib | Partial | P3 | SL3 | stdlib | ASCII constants + `capwords`; locale/formatter helpers pending. |
| numbers | Stdlib | Planned | P3 | SL3 | stdlib | ABCs + numeric tower registration pending. |
| unicodedata | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Unicode database helpers pending. |

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
- Semantics: `deque` basic append/pop/iter; `Counter` arithmetic and `most_common`.
- Runtime/IR: Current shim uses list-backed `deque` + dict-backed wrappers; TODO(stdlib-compat, owner:stdlib, milestone:SL1): move to runtime types and dict-subclass parity.
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `dataclasses` transform (default_factory, kw-only, order, `__annotations__`).
- TODO(stdlib-compat, owner:runtime, milestone:SL2): `hashlib` deterministic hashing policy.
- TODO(stdlib-compat, owner:runtime, milestone:SL3): expand `io` to buffered/text wrappers and streaming helpers.
- TODO(stdlib-compat, owner:runtime, milestone:SL3): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): expand `asyncio` shim to full loop/task APIs (task groups, wait, shields) and I/O adapters.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): `typing` runtime helpers + `__annotations__` preservation.
