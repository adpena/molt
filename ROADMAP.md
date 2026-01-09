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
- Implemented: exception chaining with explicit `__cause__`, implicit `__context__`, and `__suppress_context__`.
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
- Partial: asyncio shim (`run`/`sleep` lowered to runtime with delay/result semantics); loop/task APIs still pending.
- Partial: shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pprint`, `string`, `typing`, `sys`, `os`, and `asyncio` (capability-gated env access).
- Import-only allowlist expanded for `base64`, `binascii`, `pickle`, `unittest`, `site`, `sysconfig`, `collections.abc`, `importlib`, and `importlib.util` (API parity pending).

## Offload / IPC
- Partial: `molt_accel` v0 scaffolding (stdio framing + client + decorator) + initial `molt_worker` stdio shell (built-in `list_items` demo handler).
- TODO(offload, owner:runtime, milestone:SL1): compiled entrypoint dispatch + cancellation propagation + queue/timeout metrics.

## DB
- Partial: `molt-db` pool skeleton (bounded, sync); async drivers + Postgres protocol + cancellation-aware queries still pending.

## Tooling
- Keep type facts + `ty` validation wired into build/lint flows and surface regressions early.
- Track complex performance work in `OPTIMIZATIONS_PLAN.md` before large refactors.
- TODO(runtime-provenance, owner:runtime, milestone:RT1): replace handle-table locks with sharded or lock-free lookups once handle migration lands.
