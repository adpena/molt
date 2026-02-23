# Semantic Behavior Matrix
**Spec ID:** 0023
**Status:** Draft
**Owner:** language-lawyer
**Goal:** Track support and divergence of specific Python language semantics that are not captured by Types or Opcodes alone.

## 1. Evaluation Order
| Feature | Semantics | Status | Molt Behavior | Notes |
| --- | --- | --- | --- | --- |
| Call Arguments | Left-to-right | Supported | Matches CPython. | Crucial for side effects. |
| Assignment | RHS then LHS | Supported | Matches CPython. | `a[f()] = g()` -> `g` then `f` then assign. |
| Chained Compare | `a < b < c` | Supported | Matches CPython. | `b` evaluated once. |
| Slice Arguments | `start:stop:step` | Supported | Matches CPython. | Evaluated L-to-R. |
| Dictionary Literals | Key then Value | Supported | Matches CPython. | L-to-R pairs. |
| Generator Expressions | `iter` created immed. | Supported | Matches CPython. | First iterable evaluated at call site. |
| Annotations (PEP 649) | Lazy evaluation via `__annotate__` | Supported | Matches CPython 3.14. | Module/class `__annotations__` cached in `__dict__`; formats 1/2 (VALUE/STRING). |

## 2. Scoping & Namespaces
| Feature | Semantics | Status | Molt Behavior | Notes |
| --- | --- | --- | --- | --- |
| LEGB Rule | Local, Encl, Global, Builtin | Supported | Matches CPython. | - |
| `global` keyword | Write to module scope | Supported | Matches CPython. | - |
| `nonlocal` keyword | Write to enclosing scope | Supported | Matches CPython. | - |
| Class Body Scope | No closure access | Supported | Matches CPython. | Methods can't see class locals. |
| List Comp Scope | Function scope | Supported | Matches CPython. | Python 3 behavior (leak-free). |
| Loop Variable | Leaks to outer scope | Supported | Matches CPython. | `i` remains after loop. |
| Exception Variable | Cleared at `except` exit | Supported | Matches CPython. | `del e` implicit. |
| UnboundLocal | Access before assign | Supported | Guarded | Raises `UnboundLocalError`. |

## 3. Object Model Details
| Feature | Semantics | Status | Molt Behavior | Notes |
| --- | --- | --- | --- | --- |
| Identity | `id()` is unique/const | Supported | Deterministic ID | Not memory address. |
| Hashing | `hash()` | Supported | Matches CPython | SipHash13 + `PYTHONHASHSEED` (randomized by default; deterministic when seed=0). |
| Equality | `__eq__` reflexive | Supported | Matches CPython. | - |
| Truthiness | `__bool__` -> `__len__` | Supported | Matches CPython. | - |
| Descriptor Protocol | `__get__`/`__set__` | Supported | Matches CPython. | Data vs non-data priority; callable `__get__`/`__set__`/`__delete__` supported. |
| Metaclasses | Class creation hook | Partial | Static-only | No dynamic `metaclass=X` execution yet (TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): metaclass execution). |
| `__slots__` | Fixed layout | Supported | Struct-backed | Primary optimization target. |

## 4. Control Flow Nuances
| Feature | Semantics | Status | Molt Behavior | Notes |
| --- | --- | --- | --- | --- |
| `for` loop | `StopIteration` handling | Supported | Matches CPython. | Catch & stop. |
| `break` / `continue` | In `try/finally` | Supported | Matches CPython. | Executes `finally` before jump. |
| `return` | In `try/finally` | Supported | Matches CPython. | Executes `finally` before return. |
| `yield` | Pause/Resume | Supported | State Machine | Preserves locals. |
| `yield from` | Sub-generator delegation | Supported | Matches CPython. | Handles `send`/`throw`/`close`. |
| Async Task | Eager execution | Divergent | Lazy-start option | Molt may enable lazy tasks for perf (TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence). |

## 5. Runtime Environment
| Feature | Semantics | Status | Molt Behavior | Notes |
| --- | --- | --- | --- | --- |
| Recursion Limit | `sys.setrecursionlimit` | Supported | Fixed/Config | Checks depth. |
| Signal Handling | `KeyboardInterrupt` | Partial | Polling | Checks loop headers/calls (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:partial): signal handling parity). |
| Threading | GIL semantics | Divergent | No GIL | Molt is essentially single-threaded per isolate. |
| GC | Refcounting + Cycle Det | Partial | RC only (cycle collector pending) | Deterministic RC is key (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector). |
| Finalizers | `__del__` | Partial | Best-effort | Not guaranteed at exit (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:partial): finalizer guarantees). |
| Module Init | Once per process | Supported | Matches CPython. | Locks on import. |
| File I/O | `open` + file object semantics | Partial | Full `open()` signature + core file methods/attrs | utf-8/ascii/latin-1 only, partial text-mode seek/tell cookie semantics, and Windows fileno/isatty parity pending. |

## 6. Arithmetic & Numbers
| Feature | Semantics | Status | Molt Behavior | Notes |
| --- | --- | --- | --- | --- |
| Integer Overflow | Promotes to Arbitrary | Supported | SmallInt -> BigInt | Transparent. |
| Float Precision | IEEE 754 | Supported | Matches Host | - |
| Division | `//` floor | Supported | Matches CPython. | Rounds to -inf. |
| Modulo | `%` sign match | Supported | Matches Divisor | Different from C. |
| Power | `pow(a, b, m)` | Supported | Matches CPython. | Modular exponentiation. |

## 7. Divergences (Explicit)
Molt explicitly diverges from CPython in these specific areas for performance/determinism:

1.  **Memory Layout:** Objects do not have stable C-memory addresses. `id()` is a synthetic handle.
2.  **Refcounting:** Code may not rely on immediate destruction of cycles (only RC-reachable objects die immediately).
3.  **Bytecode:** Molt does not emulate `.pyc` files or `dis` output.
4.  **Stack Depth:** Exception tracebacks may not match CPython frame-for-frame due to inlining.

## 8. TODOs
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): Formalize "Lazy Task" divergence policy.
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): Implement cycle collector (currently pure RC).

## 9. Matrix Audit (2026-01-16)
Coverage evidence (selected):
- `tests/differential/basic/getattr_calls.py`, `tests/differential/basic/descriptor_delete.py` (descriptor call path + delete/set parity).
- `tests/differential/basic/object_dunder_builtins.py` (object-level attribute semantics).
- `tests/differential/basic/args_kwargs_eval_order.py` (evaluation order).
- `tests/differential/basic/iter_non_iterator.py` (iter non-iterator TypeError parity).
- `tests/differential/basic/recursion_limit.py` (recursion limit semantics).
- `tests/differential/basic/pep649_lazy_annotations.py` (PEP 649 lazy annotations + __annotate__ formats).

Gaps or missing coverage (audit findings):
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): add security-focused differential tests for attribute access edge cases (descriptor exceptions, `__getattr__` recursion traps).
