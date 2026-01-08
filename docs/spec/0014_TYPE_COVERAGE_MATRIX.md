# Python Type Coverage Matrix
**Spec ID:** 0014
**Status:** Draft (implementation-tracking)
**Owner:** core-compiler + runtime + backend
**Goal:** Full CPython builtin type coverage with deterministic semantics and production-grade performance.

## 1. Coverage Matrix (builtins)
### Types
| Type | Required Semantics (short) | Status | Priority | Milestone | Owner |
| --- | --- | --- | --- | --- | --- |
| NoneType | singleton, truthiness, repr | Supported | P0 | TC0 | runtime |
| bool | truthiness, ops, repr | Supported | P0 | TC0 | runtime |
| int | arithmetic, comparisons, hash | Supported | P0 | TC0 | runtime |
| float | arithmetic, comparisons, repr | Supported | P0 | TC0 | runtime |
| complex | arithmetic, comparisons, repr | Planned | P1 | TC2 | runtime |
| str | len, slice, find/split/replace/startswith/endswith/count/join, concat, repr | Partial | P0 | TC1 | runtime/frontend |
| bytes | len, slice, find/split/replace, concat | Partial | P0 | TC1 | runtime |
| bytearray | mutability, find/split/replace, concat | Partial | P0 | TC1 | runtime |
| list | literals, index/slice, methods, iter | Partial | P0 | TC1 | runtime/frontend |
| tuple | literals, index/slice, hash, iter | Partial | P0 | TC1 | runtime/frontend |
| dict | literals, index/set, views, iter | Partial | P0 | TC1 | runtime/frontend |
| set | literals, constructor, add/remove/contains/iter/len, algebra (`|`, `&`, `-`, `^`) | Partial | P1 | TC2 | runtime/frontend |
| frozenset | constructor, hash, contains/iter/len, algebra (`|`, `&`, `-`, `^`) | Partial | P1 | TC2 | runtime/frontend |
| range | len/index/iter; step==0 error | Partial | P0 | TC1 | runtime/frontend |
| slice | slice objects + normalization + step | Partial | P1 | TC2 | runtime/frontend |
| memoryview | buffer protocol (1D format/shape/strides), slicing, writable views | Partial | P2 | TC3 | runtime |
| iterator | iter/next protocol, StopIteration | Partial | P0 | TC1 | runtime |
| generator/coroutine | send/throw/close, await | Partial | P0 | TC2 | runtime/frontend |
| exceptions | BaseException hierarchy, raise/try, chaining | Partial | P0 | TC1 | frontend/runtime |
| function/method | callables, closures, descriptors | Partial | P1 | TC2 | frontend/runtime |
| type/object | isinstance/issubclass, MRO | Planned | P2 | TC3 | runtime |
| module | imports, attributes, globals | Partial | P2 | TC3 | stdlib/frontend |
| descriptor protocol | @property, @classmethod | Partial | P1 | TC2 | runtime/frontend |

### Builtin functions
| Builtin | Required Semantics (short) | Status | Priority | Milestone | Owner |
| --- | --- | --- | --- | --- | --- |
| abs | numeric absolute value | Planned | P1 | TC2 | frontend/runtime |
| aiter | async iterator protocol | Supported | P1 | TC2 | frontend/runtime |
| all | truthiness reduction | Planned | P1 | TC2 | frontend/runtime |
| anext | async next with default | Partial | P1 | TC2 | frontend/runtime |
| any | truthiness reduction | Planned | P1 | TC2 | frontend/runtime |
| ascii | ASCII repr escaping | Planned | P2 | TC3 | runtime |
| bin | integer to binary string | Planned | P2 | TC3 | runtime |
| bool | bool constructor | Planned | P1 | TC2 | frontend/runtime |
| breakpoint | debugger hook (gated) | Planned | P2 | TC3 | stdlib |
| bytearray | bytearray constructor | Partial | P0 | TC1 | frontend/runtime |
| bytes | bytes constructor | Planned | P1 | TC2 | frontend/runtime |
| callable | callable predicate | Planned | P2 | TC3 | runtime |
| chr | int to Unicode char | Planned | P2 | TC3 | runtime |
| classmethod | descriptor constructor | Partial | P1 | TC2 | runtime |
| compile | code object (restricted) | Planned | P2 | TC3 | stdlib |
| complex | complex constructor | Planned | P1 | TC2 | frontend/runtime |
| delattr | attribute deletion | Partial | P2 | TC3 | runtime |
| dict | dict constructor | Planned | P1 | TC2 | frontend/runtime |
| dir | attribute listing | Planned | P2 | TC3 | runtime |
| divmod | quotient/remainder | Planned | P1 | TC2 | frontend/runtime |
| enumerate | lazy iterator with index | Planned | P1 | TC2 | frontend/runtime |
| eval | eval (restricted) | Planned | P2 | TC3 | stdlib |
| exec | exec (restricted) | Planned | P2 | TC3 | stdlib |
| filter | lazy iterator predicate | Planned | P1 | TC2 | frontend/runtime |
| float | float constructor | Supported | P1 | TC2 | frontend/runtime |
| format | format protocol | Partial | P2 | TC3 | runtime |
| frozenset | frozenset constructor | Partial | P1 | TC2 | frontend/runtime |
| getattr | attribute lookup | Partial | P1 | TC2 | runtime |
| globals | globals dict | Planned | P2 | TC3 | stdlib |
| hasattr | attribute predicate | Partial | P1 | TC2 | runtime |
| hash | hash protocol | Planned | P2 | TC3 | runtime |
| help | help system (gated) | Planned | P2 | TC3 | stdlib |
| hex | integer to hex string | Planned | P2 | TC3 | runtime |
| id | identity (deterministic) | Planned | P2 | TC3 | runtime |
| input | stdin (gated) | Planned | P2 | TC3 | stdlib |
| int | int constructor | Partial | P1 | TC2 | frontend/runtime |
| isinstance | type check + tuple-of-types | Partial | P2 | TC3 | runtime |
| issubclass | type check + tuple-of-types | Partial | P2 | TC3 | runtime |
| iter | iterator construction | Planned | P1 | TC2 | frontend/runtime |
| len | container/sequence length | Partial | P0 | TC1 | frontend/runtime |
| list | list constructor | Partial | P0 | TC1 | frontend/runtime |
| locals | locals dict | Planned | P2 | TC3 | stdlib |
| map | lazy iterator calling callable | Planned | P1 | TC2 | frontend/runtime |
| max | reduction with key/default | Planned | P1 | TC2 | frontend/runtime |
| memoryview | memoryview constructor | Partial | P2 | TC3 | runtime |
| min | reduction with key/default | Planned | P1 | TC2 | frontend/runtime |
| next | iterator next with default | Planned | P1 | TC2 | frontend/runtime |
| object | base object constructor | Partial | P2 | TC3 | runtime |
| oct | integer to octal string | Planned | P2 | TC3 | runtime |
| open | file I/O (gated) | Planned | P2 | TC3 | stdlib |
| ord | char to int | Planned | P2 | TC3 | runtime |
| pow | power with mod | Partial | P1 | TC2 | frontend/runtime |
| print | output formatting | Supported | P0 | TC0 | runtime |
| property | descriptor constructor | Partial | P1 | TC2 | runtime |
| range | range object construction + errors | Partial | P0 | TC1 | frontend/runtime |
| repr | repr protocol | Partial | P1 | TC2 | runtime |
| reversed | reverse iterator | Planned | P1 | TC2 | frontend/runtime |
| round | rounding | Partial | P1 | TC2 | frontend/runtime |
| set | set constructor | Partial | P1 | TC2 | frontend/runtime |
| setattr | attribute set | Partial | P1 | TC2 | runtime |
| slice | slice constructor | Partial | P1 | TC2 | frontend/runtime |
| sorted | stable sort + key/reverse | Planned | P2 | TC3 | frontend/runtime |
| staticmethod | descriptor constructor | Partial | P1 | TC2 | runtime |
| str | str constructor | Partial | P1 | TC2 | frontend/runtime |
| sum | reduction with start | Planned | P1 | TC2 | frontend/runtime |
| super | super() resolution | Implemented | P2 | TC3 | runtime |
| tuple | tuple constructor | Planned | P1 | TC2 | frontend/runtime |
| type | type constructor (no metaclass) | Partial | P2 | TC3 | runtime |
| vars | vars dict | Planned | P2 | TC3 | runtime |
| zip | lazy iterator over iterables | Planned | P1 | TC2 | frontend/runtime |
| __import__ | import hook | Planned | P2 | TC3 | stdlib |

## 2. Milestones
- **TC0 (Now):** ints/bools/None/float + core containers in MVP.
- **TC1 (Near):** exceptions, full container semantics, range/slice polish.
  - Implemented: `try/except/else/finally` lowering + exception chaining (explicit `__cause__`, implicit `__context__`, `__suppress_context__`).
  - Implemented: exception type objects for `type()`/`__name__` via kind-based classes (base `Exception`).
  - TODO(type-coverage, owner:runtime, milestone:TC1): BaseException hierarchy + typed matching (beyond kind-name classes).
- Implemented: comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `in`, chained comparisons) + lowering rules for core types (list/tuple/dict/str/bytes/bytearray/range).
  - TODO(type-coverage, owner:frontend, milestone:TC1): builtin reductions (`sum/min/max`) and `len` parity.
  - TODO(type-coverage, owner:frontend, milestone:TC1): builtin constructors for `tuple`, `dict`, `bytes`, `bytearray`.
- **TC2 (Mid):** set/frozenset, generators/coroutines, callable objects.
  - Implemented: generator protocol (`send`/`throw`/`close`, `yield from`) + closure slot load/store intrinsics across native + wasm backends.
  - Implemented: async state machine (`await`, `asyncio.run`/`asyncio.sleep`) with pending sentinel across native + wasm harness.
  - TODO(type-coverage, owner:runtime, milestone:TC2): generator state objects + StopIteration.
  - TODO(type-coverage, owner:frontend, milestone:TC2): comprehension lowering to iterators.
  - TODO(type-coverage, owner:frontend, milestone:TC2): builtin iterators (`iter`, `next`, `reversed`, `enumerate`, `zip`, `map`, `filter`).
  - TODO(type-coverage, owner:frontend, milestone:TC2): builtin numeric ops (`abs`, `divmod`, `min`, `max`, `sum`).
  - TODO(type-coverage, owner:frontend, milestone:TC2): builtin conversions (`complex`, `str`, `bool`).
  - Implemented (partial): `round`/`trunc` lowering with `__round__`/`__trunc__` hooks.
  - Implemented (partial): `int` conversion from int/float/str/bytes + `__int__`/`__index__` hooks.
- Implemented: `aiter`/`anext` lowering + async-for parity with `__aiter__`/`__anext__` support (sync-iter fallback retained for now).
- TODO(type-coverage, owner:frontend, milestone:TC2): `anext` awaitable support outside `await` expressions.
- **TC3 (Late):** memoryview, type/object, modules, descriptors.
  - TODO(type-coverage, owner:runtime, milestone:TC3): memoryview multidimensional shapes + advanced buffer exports.
  - TODO(type-coverage, owner:stdlib, milestone:TC3): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
  - TODO(type-coverage, owner:stdlib, milestone:TC3): reflection builtins (`type`, `isinstance`, `issubclass`, `dir`, `vars`, `globals`, `locals`).
  - TODO(type-coverage, owner:stdlib, milestone:TC3): dynamic execution builtins (`eval`, `exec`, `compile`) with sandboxing rules.
  - TODO(type-coverage, owner:stdlib, milestone:TC3): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
  - Implemented: descriptor deleter support (`__delete__`, property deleter) + attribute deletion wiring.

## 3. Runtime Object Model Expansion
- Deterministic layouts for all new heap objects (stable header + payload).
- RC/GC hooks for all container edges and iterator state.
- Implemented: instance dict fallback for structified objects + dynamic attrs on non-slot dataclasses.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`).
- Implemented: C3 MRO + multiple inheritance for attribute lookup + `super()` resolution + data descriptor precedence.
- Implemented: frozenset hashing (order-insensitive) + set/frozenset algebra intrinsics.
- Implemented: exception objects with cause/context/suppress fields.
  - Implemented: exception class objects derived from `Exception` for typed `type(exc)`.
- TODO(type-coverage, owner:runtime, milestone:TC1): exception stack trace capture.
- TODO(type-coverage, owner:runtime, milestone:TC2): formatting builtins (`repr`, `ascii`, `bin`, `hex`, `oct`, `chr`, `ord`) + full `format` protocol (`__format__`, named fields, locale-aware grouping).
- TODO(type-coverage, owner:runtime, milestone:TC2): rounding intrinsics (`floor`, `ceil`) + full deterministic semantics for edge cases.
- TODO(type-coverage, owner:runtime, milestone:TC2): identity builtins (`hash`, `id`, `callable`).
- Implemented: BigInt heap fallback + arithmetic parity beyond 47-bit inline ints.
- TODO(type-coverage, owner:runtime, milestone:TC1): recursion limits + `RecursionError` guard semantics.
- Implemented: descriptor deleter semantics (`__delete__`, property deleter) + attribute deletion wiring.

## 4. Frontend + IR Coverage
- Lower set literals/constructors + set algebra + frozenset; complex and exceptions remain.
- Add IR ops for raise, try/except, unpacking, and dunder dispatch.
- TODO(type-coverage, owner:frontend, milestone:TC2): iterable unpacking + starred targets.

## 5. Backend + WIT/ABI
- Implement ops in native + WASM backends and add WIT intrinsics.
- Add parity tests per new type (native vs wasm).
- Partial: wasm backend covers generator state machines, closure slot intrinsics, channel send/recv intrinsics, and basic async pending semantics; remaining async parity gaps include async iteration/scheduler semantics.

## 6. Stdlib + Interop
- Expand builtins (e.g., `range`, `slice`, exceptions).
- Document staged/unsupported behaviors explicitly.
- TODO(type-coverage, owner:stdlib, milestone:TC2): `builtins` module parity notes.

## 7. Correctness + Perf Gates
- Differential tests per type with edge cases (errors, hashing, iteration).
- Hypothesis‑driven generators for container/exception semantics.
- Perf gates for container hot paths + memory churn.
- TODO(type-coverage, owner:tests, milestone:TC1): add exception coverage to molt_diff.
