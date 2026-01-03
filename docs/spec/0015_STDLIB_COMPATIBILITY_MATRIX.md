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
| contextlib | Stdlib | Planned | P2 | SL2 | stdlib | Context manager helpers. |
| weakref | Stdlib | Planned | P3 | SL3 | runtime | Weak references (GC-aware). |
| logging | Stdlib | Planned | P2 | SL2 | stdlib | Structured logging; gated sinks. |
| json | Stdlib | Planned | P2 | SL2 | stdlib | Keep `molt_json` as fast-path. |
| csv | Stdlib | Planned | P3 | SL3 | stdlib | Deterministic CSV parsing. |
| io | Capability-gated | Planned | P2 | SL3 | stdlib | File and in-memory streams. |
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
- TODO(stdlib-compat, owner:runtime, milestone:SL1): `array` + `struct` deterministic layouts and packing.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `re` engine + deterministic regex semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `json` parity plan (interop with `molt_json`).
- TODO(stdlib-compat, owner:frontend, milestone:SL2): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2): `dataclasses` transform (default_factory, kw-only, order, slots, `__annotations__`).
- TODO(stdlib-compat, owner:runtime, milestone:SL2): `hashlib` deterministic hashing policy.
- TODO(stdlib-compat, owner:runtime, milestone:SL3): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3): `typing` runtime helpers + `__annotations__` preservation.
