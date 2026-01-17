# Python Type Coverage Matrix
**Spec ID:** 0014
**Status:** Draft (implementation-tracking)
**Owner:** core-compiler + runtime + backend
**Goal:** Full CPython builtin type coverage with CPython semantics (including `PYTHONHASHSEED`) and production-grade performance.

## 1. Coverage Matrix (builtins)
### Types
| Type | Required Semantics (short) | Status | Priority | Milestone | Owner |
| --- | --- | --- | --- | --- | --- |
| NoneType | singleton, truthiness, repr | Supported | P0 | TC0 | runtime |
| bool | truthiness, ops, repr | Supported | P0 | TC0 | runtime |
| int | arithmetic, comparisons, hash | Supported | P0 | TC0 | runtime |
| float | arithmetic, comparisons, repr | Supported | P0 | TC0 | runtime |
| complex | arithmetic, comparisons, repr | Planned | P1 | TC2 | runtime |
| str | len, index/slice via `__index__`, iter, find/split/replace/startswith/endswith/count/join/strip/lower/upper/capitalize, concat, repr | Partial | P0 | TC1 | runtime/frontend |
| bytes | len, index/slice via `__index__`, iter, find (bytes-like/int needles)/count/startswith/endswith/split/replace (start/end slices), concat | Partial | P0 | TC1 | runtime |
| bytearray | mutability, index/slice via `__index__`, slice assign/delete, iter, find (bytes-like/int needles)/count/startswith/endswith/split/replace (start/end slices), concat, in-place concat/repeat | Partial | P0 | TC1 | runtime |
| list | literals, index/slice via `__index__`, slice assign/delete, append/extend/insert/remove/pop/count/index/clear/copy/reverse, iter, in-place add/mul | Partial | P0 | TC1 | runtime/frontend |
| tuple | literals, index/slice via `__index__`, hash, iter | Partial | P0 | TC1 | runtime/frontend |
| dict | literals, index/set, views, iter, basic methods (keys/values/items/get/pop/clear/copy/popitem/setdefault/update/fromkeys) | Partial | P0 | TC1 | runtime/frontend |
| set | literals, constructor, add/remove/contains/iter/len, algebra (`|`, `&`, `-`, `^`) + in-place updates (dict view RHS) | Partial | P1 | TC2 | runtime/frontend |
| frozenset | constructor, hash, contains/iter/len, algebra (`|`, `&`, `-`, `^`) | Partial | P1 | TC2 | runtime/frontend |
| range | len/index via `__index__`/iter; step==0 error | Partial | P0 | TC1 | runtime/frontend |
| slice | slice objects + normalization + step + `__index__` bounds | Partial | P1 | TC2 | runtime/frontend |
| memoryview | buffer protocol (format/shape/strides), cast, tuple scalar indexing, 1D slicing + slice assignment, writable views | Partial | P2 | TC3 | runtime |
| iterator | iter/next protocol, StopIteration | Partial | P0 | TC1 | runtime |
| generator/coroutine | send/throw/close, await | Partial | P0 | TC2 | runtime/frontend |
| exceptions | BaseException hierarchy, raise/try, chaining, `__traceback__` (names) | Partial | P0 | TC1 | frontend/runtime |
| function/method | callables, closures, descriptors | Partial | P1 | TC2 | frontend/runtime |
| code | `__code__` objects (`co_filename`, `co_name`, `co_firstlineno`) | Partial | P2 | TC2 | frontend/runtime |
| type/object | isinstance/issubclass, MRO | Partial | P2 | TC3 | runtime |
| module | imports, attributes, globals | Partial | P2 | TC3 | stdlib/frontend |
| descriptor protocol | @property, @classmethod | Partial | P1 | TC2 | runtime/frontend |

### Builtin functions
| Builtin | Required Semantics (short) | Status | Priority | Milestone | Owner |
| --- | --- | --- | --- | --- | --- |
| abs | numeric absolute value | Partial | P1 | TC2 | frontend/runtime |
| aiter | async iterator protocol | Supported | P1 | TC2 | frontend/runtime |
| all | truthiness reduction | Partial | P1 | TC2 | frontend/runtime |
| anext | async next with default | Partial | P1 | TC2 | frontend/runtime |
| any | truthiness reduction | Partial | P1 | TC2 | frontend/runtime |
| ascii | ASCII repr escaping | Partial | P2 | TC3 | runtime |
| bin | integer to binary string | Partial | P2 | TC3 | runtime |
| bool | bool constructor | Partial | P1 | TC2 | frontend/runtime |
| breakpoint | debugger hook (gated) | Planned | P2 | TC3 | stdlib |
| bytearray | bytearray constructor | Partial | P0 | TC1 | frontend/runtime |
| bytes | bytes constructor | Partial | P1 | TC2 | frontend/runtime |
| callable | callable predicate | Partial | P2 | TC3 | runtime |
| chr | int to Unicode char | Supported | P2 | TC3 | runtime |
| classmethod | descriptor constructor | Partial | P1 | TC2 | runtime |
| compile | code object (restricted) | Planned | P2 | TC3 | stdlib |
| complex | complex constructor | Planned | P1 | TC2 | frontend/runtime |
| delattr | attribute deletion | Partial | P2 | TC3 | runtime |
| dict | dict constructor | Partial | P1 | TC2 | frontend/runtime |
| dir | attribute listing | Planned | P2 | TC3 | runtime |
| divmod | quotient/remainder | Partial | P1 | TC2 | frontend/runtime |
| enumerate | lazy iterator with index | Partial | P1 | TC2 | frontend/runtime |
| eval | eval (restricted) | Planned | P2 | TC3 | stdlib |
| exec | exec (restricted) | Planned | P2 | TC3 | stdlib |
| filter | lazy iterator predicate | Partial | P1 | TC2 | frontend/runtime |
| float | float constructor | Supported | P1 | TC2 | frontend/runtime |
| format | format protocol | Partial | P2 | TC3 | runtime |
| frozenset | frozenset constructor | Partial | P1 | TC2 | frontend/runtime |
| getattr | attribute lookup | Partial | P1 | TC2 | runtime |
| globals | globals dict | Planned | P2 | TC3 | stdlib |
| hasattr | attribute predicate | Partial | P1 | TC2 | runtime |
| hash | hash protocol | Partial | P2 | TC3 | runtime |
| help | help system (gated) | Planned | P2 | TC3 | stdlib |
| hex | integer to hex string | Partial | P2 | TC3 | runtime |
| id | identity (deterministic) | Partial | P2 | TC3 | runtime |
| input | stdin (gated) | Planned | P2 | TC3 | stdlib |
| int | int constructor | Partial | P1 | TC2 | frontend/runtime |
| isinstance | type check + tuple-of-types | Partial | P2 | TC3 | runtime |
| issubclass | type check + tuple-of-types | Partial | P2 | TC3 | runtime |
| iter | iterator construction | Partial | P1 | TC2 | frontend/runtime |
| len | container/sequence length | Supported | P0 | TC1 | frontend/runtime |
| list | list constructor | Partial | P0 | TC1 | frontend/runtime |
| locals | locals dict | Planned | P2 | TC3 | stdlib |
| map | lazy iterator calling callable | Partial | P1 | TC2 | frontend/runtime |
| max | reduction with key/default | Partial | P1 | TC2 | frontend/runtime |
| memoryview | memoryview constructor | Partial | P2 | TC3 | runtime |
| min | reduction with key/default | Partial | P1 | TC2 | frontend/runtime |
| next | iterator next with default | Partial | P1 | TC2 | frontend/runtime |
| object | base object constructor | Partial | P2 | TC3 | runtime |
| oct | integer to octal string | Partial | P2 | TC3 | runtime |
| open | file I/O (gated; buffering/text parity) | Partial | P2 | TC3 | stdlib |
| ord | char to int | Supported | P2 | TC3 | runtime |
| pow | power with mod | Partial | P1 | TC2 | frontend/runtime |
| print | output formatting | Supported | P0 | TC0 | runtime |
| property | descriptor constructor | Partial | P1 | TC2 | runtime |
| range | range object construction + errors | Partial | P0 | TC1 | frontend/runtime |
| repr | repr protocol | Partial | P1 | TC2 | runtime |
| reversed | reverse iterator | Partial | P1 | TC2 | frontend/runtime |
| round | rounding | Partial | P1 | TC2 | frontend/runtime |
| set | set constructor | Partial | P1 | TC2 | frontend/runtime |
| setattr | attribute set | Partial | P1 | TC2 | runtime |
| slice | slice constructor | Partial | P1 | TC2 | frontend/runtime |
| sorted | stable sort + key/reverse | Partial | P2 | TC3 | frontend/runtime |
| staticmethod | descriptor constructor | Partial | P1 | TC2 | runtime |
| str | str constructor | Partial | P1 | TC2 | frontend/runtime |
| sum | reduction with start | Partial | P1 | TC2 | frontend/runtime |
| super | super() resolution | Implemented | P2 | TC3 | runtime |
| tuple | tuple constructor | Partial | P1 | TC2 | frontend/runtime |
| type | type constructor (no metaclass) | Partial | P2 | TC3 | runtime |
| vars | vars dict | Planned | P2 | TC3 | runtime |
| zip | lazy iterator over iterables | Partial | P1 | TC2 | frontend/runtime |
| __import__ | import hook | Planned | P2 | TC3 | stdlib |

## 2. Milestones
- **TC0 (Now):** ints/bools/None/float + core containers in MVP.
- **TC1 (Near):** exceptions, full container semantics, range/slice polish.
  - Implemented: `try/except/else/finally` lowering + exception chaining (explicit `__cause__`, implicit `__context__`, `__suppress_context__`).
  - Implemented: exception type objects for `type()`/`__name__` via kind-based classes (base `BaseException`).
  - Implemented: `BaseException` root class + `SystemExit`/`KeyboardInterrupt`/`GeneratorExit` base selection.
  - TODO(type-coverage, owner:runtime, milestone:TC1, priority:P2, status:partial): typed exception matching beyond kind-name classes.
- Implemented: comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `in`, chained comparisons) for numbers + str/bytes/bytearray/list/tuple; ordering for custom objects uses `__lt__`/`__le__`/`__gt__`/`__ge__` with `NotImplemented` semantics.
  - Implemented: builtin reductions (`sum`/`min`/`max`) and `len` parity.
  - Implemented: `list.sort` with key/reverse (stable).
  - Implemented: `tuple`/`dict` constructor arg-count + sequence-length error parity.
  - Partial: `bytes`/`bytearray` constructors accept int/iterables + str with `utf-8`/`latin-1`/`ascii`/`utf-16`/`utf-32` encodings and basic error handlers (`strict`/`ignore`/`replace`).
  - TODO(type-coverage, owner:runtime, milestone:TC1, priority:P2, status:partial): expand `bytes`/`bytearray` encoding coverage (additional codecs + full error handlers).
  - TODO(stdlib-compat, owner:runtime, milestone:TC2, priority:P2, status:missing): `str(bytes, encoding, errors)` decoding parity for bytes-like inputs.
- **TC2 (Mid):** set/frozenset, generators/coroutines, callable objects.
  - Implemented: generator protocol (`send`/`throw`/`close`, `yield from`) + closure slot load/store intrinsics across native + wasm backends.
  - Implemented: generator function closures capture free vars in generator frames.
  - Implemented: `nonlocal` rebinding in nested sync/async closures.
- Implemented: async state machine (`await`, `asyncio.run`/`asyncio.sleep`) with delay/result semantics and pending sentinel across native + wasm harness.
  - Implemented: `async with` lowering for `__aenter__`/`__aexit__` (single manager, simple name binding).
  - Implemented: StopIteration.value propagation for generator returns, explicit raises, and iterator next.
  - Implemented: generator state objects (`gi_running`, `gi_frame` stub, `gi_yieldfrom`) and `inspect.getgeneratorstate`.
  - TODO(introspection, owner:runtime, milestone:TC3, priority:P2, status:missing): full frame objects + `gi_code` parity.
  - Implemented: comprehension lowering to iterators (list/set/dict comprehensions + generator expressions).
  - TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:missing): async comprehensions (async for/await in comprehensions).
  - Implemented (partial): builtin iterators (`iter` with sentinel, `next`, `reversed`, `zip`, `map`, `filter`).
  - Implemented (partial): builtin numeric ops (`abs`, `divmod`, `min`, `max`, `sum`) for numeric types.
  - TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:partial): builtin conversions (`complex`, `str`, `bool`).
  - Implemented (partial): `round`/`trunc` lowering with `__round__`/`__trunc__` hooks.
  - Implemented (partial): `int` conversion from int/float/str/bytes + `__int__`/`__index__` hooks.
- Implemented: `aiter`/`anext` lowering + async-for parity with `__aiter__`/`__anext__` support (sync-iter fallback retained for now).
- Implemented: `anext` default handling outside `await` expressions.
- **TC3 (Late):** memoryview, type/object, modules, descriptors.
  - TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): memoryview multi-dimensional slicing + sub-views (retain C-order semantics + parity errors).
  - TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
  - TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:partial): reflection builtins (`type`, `isinstance`, `issubclass`, `dir`, `vars`, `globals`, `locals`).
  - TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): dynamic execution builtins (`eval`, `exec`, `compile`) with sandboxing rules.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:partial): finish `open`/file object parity (non-UTF-8 encodings, readinto/writelines/reconfigure, text-mode seek/tell cookies, Windows fileno/isatty) with differential + wasm coverage.
  - Implemented: descriptor deleter support (`__delete__`, property deleter) + attribute deletion wiring.

## 3. Runtime Object Model Expansion
- Deterministic layouts for all new heap objects (stable header + payload).
- RC/GC hooks for all container edges and iterator state.
- Implemented: instance dict fallback for structified objects + dynamic attrs on non-slot dataclasses.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`).
- Implemented: instance `__getattr__`/`__getattribute__` fallback plus `__setattr__`/`__delattr__` hooks for user-defined classes.
- Implemented: `__class__`/`__dict__` accessors for instances/functions/modules/classes (class `__dict__` is mutable; mappingproxy pending).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:missing): mappingproxy view for class `__dict__`.
- Implemented: C3 MRO + multiple inheritance for attribute lookup + `super()` resolution + data descriptor precedence.
- Implemented: frozenset hashing (order-insensitive) + set/frozenset algebra intrinsics with CPython mixing parity.
- Implemented: exception objects with cause/context/suppress fields.
  - Implemented: exception class objects derived from `BaseException` for typed `type(exc)`.
- Implemented: `__traceback__` capture as a tuple of (filename, line, name) entries with line markers from the last executed statement in each frame and global code slots across the module graph.
- Implemented: object-level `__getattribute__`/`__setattr__`/`__delattr__` builtins with CPython-style raw semantics.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): rounding intrinsics (`floor`, `ceil`) + full deterministic semantics for edge cases.
- Implemented: BigInt heap fallback + arithmetic parity beyond 47-bit inline ints.
- Implemented: recursion limits + `RecursionError` guard semantics.
- Implemented: descriptor deleter semantics (`__delete__`, property deleter) + attribute deletion wiring.

## 4. Frontend + IR Coverage
- Lower set literals/constructors + set algebra + frozenset; complex and exceptions remain.
- Add IR ops for raise, try/except, unpacking, and dunder dispatch.
- Implemented: `list.extend` consumes generic iterables via the iter protocol (range/generator/etc.).
- Implemented: augmented assignment lowers to in-place list/bytearray/set ops (`+=`, `*=`, `|=`, `&=`, `^=`, `-=`) with attribute/subscript targets.
- Implemented: iterable unpacking + starred targets for assignment and loop targets.

## 5. Backend + WIT/ABI
- Implement ops in native + WASM backends and add WIT intrinsics.
- Add parity tests per new type (native vs wasm).
- Partial: wasm backend covers generator state machines, closure slot intrinsics, channel send/recv intrinsics, and basic async pending semantics; remaining async parity gaps include async iteration/scheduler semantics.
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): wasm async iteration/scheduler parity.

## 6. Stdlib + Interop
- Expand builtins (e.g., `range`, `slice`, exceptions).
- Document staged/unsupported behaviors explicitly.
- TODO(type-coverage, owner:stdlib, milestone:TC2, priority:P3, status:planned): `builtins` module parity notes.

## 7. Correctness + Perf Gates
- Differential tests per type with edge cases (errors, hashing, iteration).
- Implemented: container dunder/membership fallback coverage (`__contains__`, `__iter__`, `__getitem__`) plus getattr-based method calls and class decorator evaluation order.
- Hypothesis‑driven generators for container/exception semantics.
- Perf gates for container hot paths + memory churn.
- Partial: exception coverage in `molt_diff` (add traceback/args/line info cases as runtime grows).
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): expand exception differential coverage.

## 8. Matrix Audit (2026-01-16)
Coverage evidence (selected):
- `tests/differential/basic/container_dunders.py` (list/dict/str dunder container ops + type-level calls).
- `tests/differential/basic/object_dunder_builtins.py` (object-level `__getattribute__`/`__setattr__`/`__delattr__` raw semantics).
- `tests/differential/basic/list_dict.py` (list/dict core methods, dict views, `dict.fromkeys`).
- `tests/differential/basic/str_methods.py`, `tests/differential/basic/bytes_ranges.py` (string/bytes/bytearray methods + slicing).
- `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/set_view_ops.py`, `tests/differential/basic/frozenset_basic.py` (set/frozenset algebra + views).
- `tests/differential/basic/builtin_iterators.py`, `tests/differential/basic/iter_non_iterator.py` (iter/next protocol + non-iterator TypeError parity).
- `tests/differential/basic/generator_protocol.py`, `tests/differential/basic/generator_state.py`, `tests/test_native_async_protocol.py`, `tests/test_wasm_async_protocol.py` (generator/coroutine/async protocol parity).

Gaps or missing coverage (audit findings):
- Implemented: print keyword-argument parity coverage (`sep`, `end`, `file`, `flush`) in `tests/differential/basic/print_keywords.py` + `tests/test_wasm_print_keywords.py`.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:missing): lower classes defining `__next__` without `__iter__` without backend panics.
