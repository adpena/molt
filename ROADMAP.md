# Molt Roadmap (Active)

Canonical current status: `docs/spec/STATUS.md`. This roadmap is forward-looking.

## Performance
- Vector reduction kernels now cover `sum`/`prod`/`min`/`max` with trusted fast paths; next up: float reductions and typed-buffer kernels.
- String kernel SIMD paths cover find/split/replace with Unicode-safe index translation; next: Unicode index caches and wider SIMD.

## Type Coverage
- memoryview (Partial): constructor, slicing, `tobytes`, writable views, strides, buffer export.
- TODO(type-coverage, owner:runtime, milestone:TC3): memoryview format codes and multidimensional shapes.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`) + `__set_name__` hook.
- Implemented: C3 MRO + multiple inheritance for attribute lookup + `super()` resolution + data descriptor precedence.
- Implemented: reflection builtins (`type`, `isinstance`, `issubclass`, `object`) for base chains (no metaclasses).
- Implemented: exception chaining with explicit `__cause__`, implicit `__context__`, and `__suppress_context__`.
- Implemented: descriptor deleter semantics (`__delete__`, property deleter) + attribute deletion wiring.
- Partial: wasm generator state machines + closure slot intrinsics + channel send/recv intrinsics + async pending/block_on parity landed; remaining generator state object and scheduler semantics.
- Implemented: async iterator protocol (`__aiter__`/`__anext__`) with `aiter`/`anext` lowering and `async for` support; sync-iter fallback remains for now (awaitable `anext` outside `await` still pending).

## Stdlib
- Partial: asyncio shim (`run`/`sleep` lowered to runtime); loop/task APIs and delay semantics still pending.
- Partial: shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pprint`, `string`, `typing`, `sys`, and `os` (capability-gated env access).
- Import-only allowlist expanded for `base64`, `binascii`, `pickle`, `unittest`, `site`, `sysconfig`, `collections.abc`, and `importlib` (API parity pending).

## Offload / IPC
- Partial: `molt_accel` v0 scaffolding (stdio framing + client + decorator) + initial `molt_worker` stdio shell (built-in `list_items` demo handler).
- TODO(offload, owner:runtime, milestone:SL1): compiled entrypoint dispatch + cancellation propagation + queue/timeout metrics.

## DB
- Partial: `molt-db` pool skeleton (bounded, sync); async drivers + Postgres protocol + cancellation-aware queries still pending.

## Tooling
- Keep type facts + `ty` validation wired into build/lint flows and surface regressions early.
- Track complex performance work in `OPTIMIZATIONS_PLAN.md` before large refactors.
- TODO(runtime-provenance, owner:runtime, milestone:RT1): replace handle-table locks with sharded or lock-free lookups once handle migration lands.
