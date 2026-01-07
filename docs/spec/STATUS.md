# STATUS (Canonical)

Last updated: 2026-01-07

This document is the source of truth for Molt's current capabilities and
limitations. Update this file whenever behavior or scope changes, and keep
README/ROADMAP in sync.

## Capabilities (Current)
- Tier 0 structification for typed classes (fixed layout).
- Native async/await lowering with state-machine poll loops.
- Async iteration: `__aiter__`/`__anext__`, `aiter`/`anext`, and `async for`.
- AOT compilation via Cranelift for native targets.
- Differential testing vs CPython 3.12 for supported constructs.
- Molt packages for Rust-backed deps using MsgPack/CBOR and Arrow IPC.

## Limitations (Current)
- Classes/object model: C3 MRO + multiple inheritance + `super()` resolution for
  attribute lookup; no metaclasses or dynamic `type()` construction; descriptor
  precedence for `__get__`/`__set__`/`__delete__` is supported.
- Attributes: fixed struct fields with dynamic instance-dict fallback; no
  user-defined `__slots__` beyond dataclass lowering; no `__getattr__`/
  `__setattr__` hooks yet.
- Dataclasses: compile-time lowering for frozen/eq/repr/slots; no
  `default_factory`, `kw_only`, or `order`.
- Exceptions: `try/except/else/finally` + `raise`/reraise; partial BaseException
  semantics (see type coverage matrix).
- Imports: static module graph only; no dynamic import hooks or full package
  resolution.
- Asyncio: shim exposes `run`/`sleep` only; loop/task APIs and delay semantics
  still pending.

## Async + Concurrency Notes
- Awaitables that return pending now resume at a labeled state to avoid
  re-running pre-await side effects.
- Channel send/recv yield on pending and resume at labeled states.

## Stdlib Coverage
- Partial shims: `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`,
  `pprint`, `string`, `typing`, `sys`, `os`, `asyncio`.
- Import-only stubs: `collections.abc`, `importlib`, `importlib.util`.
- See `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` for the full matrix.

## Tooling + Verification
- CI enforces lint, type checks, Rust fmt/clippy, differential tests, and perf
  smoke gates.
- Use `tools/dev.py lint` and `tools/dev.py test` for local validation.
- WIT interface contract lives at `wit/molt-runtime.wit` (WASM runtime intrinsics).

## Known Gaps
- uv-managed Python 3.14 hangs on arm64; system Python 3.14 used as workaround.
- TODO(runtime-provenance, owner:runtime, milestone:RT1): remove handle-table lock overhead via sharded or lock-free lookups.
