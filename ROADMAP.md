# Molt Roadmap (Active)

Canonical current status: `docs/spec/STATUS.md`. This roadmap is forward-looking.

## Performance
- Vector reduction kernels now cover `sum`/`prod`/`min`/`max` with trusted fast paths; next up: float reductions and typed-buffer kernels.
- String kernel SIMD paths cover find/split/replace with Unicode-safe index translation; next: Unicode index caches and wider SIMD.

## Type Coverage
- memoryview (Partial): 1D `format`/`shape`/`strides`/`nbytes` for bytes/bytearray views.
- TODO(type-coverage, owner:runtime, milestone:TC3): memoryview multidimensional shapes + advanced buffer exports.
- Implemented: BigInt heap fallback + arithmetic parity beyond 47-bit inline ints.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`) + `__set_name__` hook.
- Implemented: C3 MRO + multiple inheritance for attribute lookup + `super()` resolution + data descriptor precedence.
- Implemented: reflection builtins (`type`, `isinstance`, `issubclass`, `object`) for base chains (no metaclasses).
- Implemented: BaseException root + exception chaining (`__cause__`, `__context__`, `__suppress_context__`) + `__traceback__` name tuples.
- Implemented: descriptor deleter semantics (`__delete__`, property deleter) + attribute deletion wiring.
- Implemented: set literals/constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- Implemented: format mini-language for ints/floats + f-string conversion flags (`!r`, `!s`, `!a`).
- Implemented: call argument binding for Molt functions (positional/keyword/`*args`/`**kwargs`) with pos-only/kw-only enforcement.
- Implemented: `str.lower`/`str.upper`, `list.clear`/`list.copy`/`list.reverse`, and `dict.setdefault`/`dict.update`.
- TODO(type-coverage, owner:runtime, milestone:TC2): full format protocol (`__format__`, named fields, locale-aware grouping).
- TODO(type-coverage, owner:runtime, milestone:TC2): matmul dunder hooks (`__matmul__`/`__rmatmul__`) with buffer2d fast path.
- Partial: wasm generator state machines + closure slot intrinsics + channel send/recv intrinsics + async pending/block_on parity landed; remaining generator state object and scheduler semantics.
- Implemented: wasm async state dispatch uses encoded resume targets to avoid state-id collisions and keeps state/poll locals distinct (prevents pending-state corruption on resume).
- Implemented: async iterator protocol (`__aiter__`/`__anext__`) with `aiter`/`anext` lowering and `async for` support; sync-iter fallback remains for now.
- Implemented: `anext(..., default)` awaitable creation outside `await`.
- Implemented: `async with` lowering for `__aenter__`/`__aexit__` (single manager, simple name binding).
- Implemented: cancellation token plumbing with request-default inheritance and task override; automatic cancellation injection into awaits still pending.

## Stdlib
- Partial: importable `builtins` module binding supported builtins (attribute gaps tracked in the matrix).
- Partial: asyncio shim (`run`/`sleep` lowered to runtime with delay/result semantics; `set_event_loop`/`new_event_loop` stubs); loop/task APIs still pending.
- Partial: shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pprint`, `string`, `typing`, `sys`, `os`, `asyncio`, and `threading` (capability-gated env access).
- Import-only allowlist expanded for `base64`, `binascii`, `pickle`, `unittest`, `site`, `sysconfig`, `collections.abc`, `importlib`, and `importlib.util` (API parity pending).

## Offload / IPC
- Partial: `molt_accel` v0 scaffolding (stdio framing + client + decorator) + `molt_worker` stdio shell with demo handlers and compiled dispatch (`list_items`/`compute`/`offload_table`/`health`).
- Implemented: compiled export loader + manifest validation (schema, reserved-name filtering, error mapping) with queue/timeout metrics.
- TODO(offload, owner:runtime, milestone:SL1): propagate cancellation into pool waits and real DB tasks; extend compiled handlers beyond demo coverage.

## DB
- Partial: `molt-db` pool skeleton (bounded, sync); async drivers + Postgres protocol + cancellation-aware queries still pending.

## Tooling
- Keep type facts + `ty` validation wired into build/lint flows and surface regressions early.
- Track complex performance work in `OPTIMIZATIONS_PLAN.md` before large refactors.
- TODO(runtime-provenance, owner:runtime, milestone:RT1): replace handle-table locks with sharded or lock-free lookups once handle migration lands.

## Django Demo Path (Draft, 5-Step)
- Step 1 (Core semantics): close TC1/TC2 gaps in `docs/spec/0014_TYPE_COVERAGE_MATRIX.md` for Django-heavy types (dict/list/tuple/set/str, iter/len, mapping protocol, kwargs/varargs ordering per docs/spec/0016_ARGS_KWARGS.md, descriptor hooks, class `__getattr__`/`__setattr__`).
- Step 2 (Import/module system): package resolution + module objects, `__import__`, and a deterministic `sys.path` policy; unblock `importlib` basics.
- Step 3 (Stdlib essentials): advance `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` for `functools`, `itertools`, `operator`, `collections`, `contextlib`, `inspect`, `typing`, `dataclasses`, `enum`, `re`, and `datetime` to Partial with tests.
- Step 4 (Async/runtime): production-ready asyncio loop/task APIs, contextvars, cancellation injection, and long-running workload hardening.
- Step 5 (I/O + web/DB): capability-gated `os`, `sys`, `pathlib`, `logging`, `time`, `selectors`, `socket`, `ssl`; ASGI/WSGI surface, HTTP parsing, and DB client + pooling/transactions (start sqlite3 + minimal async driver), plus deterministic template rendering.
