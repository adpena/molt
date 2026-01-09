# STATUS (Canonical)

Last updated: 2026-01-09

This document is the source of truth for Molt's current capabilities and
limitations. Update this file whenever behavior or scope changes, and keep
README/ROADMAP in sync.

## Capabilities (Current)
- Tier 0 structification for typed classes (fixed layout).
- Native async/await lowering with state-machine poll loops.
- Call argument binding for Molt-defined functions: positional/keyword/`*args`/`**kwargs` with pos-only/kw-only enforcement.
- Async iteration: `__aiter__`/`__anext__`, `aiter`/`anext`, and `async for`.
- Async context managers: `async with` lowering for `__aenter__`/`__aexit__`.
- `anext(..., default)` awaitable creation outside `await`.
- AOT compilation via Cranelift for native targets.
- Differential testing vs CPython 3.12 for supported constructs.
- Molt packages for Rust-backed deps using MsgPack/CBOR and Arrow IPC.
- Sets: literals + constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- Numeric builtins: `int()`/`round()`/`math.trunc()` with `__int__`/`__index__`/`__round__`/`__trunc__` hooks and base parsing for string/bytes.
- BigInt heap fallback for ints beyond inline range (arithmetic/bitwise/shift parity for large ints).
- Format mini-language for ints/floats + f-string conversion flags (`!r`, `!s`, `!a`).
- memoryview exposes 1D `format`/`shape`/`strides`/`nbytes` for bytes/bytearray views.
- `str.count` supports start/end slices with Unicode-aware offsets.
- `str.lower`/`str.upper`, `list.clear`/`list.copy`/`list.reverse`, and `dict.setdefault`/`dict.update`.
- Dict/set key hashability parity for common unhashable types (list/dict/set/bytearray/memoryview).
- Importable `builtins` module binds supported builtins (see stdlib matrix).
- `enumerate` builtin returns an iterator over `(index, value)` with optional `start`.

## Limitations (Current)
- Classes/object model: C3 MRO + multiple inheritance + `super()` resolution for
  attribute lookup; no metaclasses or dynamic `type()` construction; descriptor
  precedence for `__get__`/`__set__`/`__delete__` is supported.
- Attributes: fixed struct fields with dynamic instance-dict fallback; no
  user-defined `__slots__` beyond dataclass lowering; no `__getattr__`/
  `__setattr__` hooks yet.
- Dataclasses: compile-time lowering for frozen/eq/repr/slots; no
  `default_factory`, `kw_only`, or `order`.
- Call binding: allowlisted module functions still reject keyword/variadic calls; binder supports up to 8 arguments before fallback work is added.
- Exceptions: `try/except/else/finally` + `raise`/reraise; partial BaseException
  semantics (see type coverage matrix).
- Imports: static module graph only; no dynamic import hooks or full package
  resolution.
- Asyncio: shim exposes `run`/`sleep` only; loop/task APIs still pending and no
  full event-loop/task surface.
- Async with: only a single context manager and simple name binding are supported.
- Matmul (`@`): supported only for `molt_buffer`/`buffer2d`; other types raise
  `TypeError` (TODO(type-coverage, owner:runtime, milestone:TC2): consider
  `__matmul__`/`__rmatmul__` fallback for custom types).
- Numeric tower: complex/decimal pending; `int` still missing full method surface
  (e.g., `bit_length`, `to_bytes`, `from_bytes`).
- Format protocol: no `__format__` fallback or named fields; locale-aware grouping
  still pending.
- memoryview: partial buffer protocol (no multidimensional shapes or advanced
  buffer exports).
- Cancellation: cooperative checks only; automatic cancellation injection into
  awaits and I/O still pending.

## Async + Concurrency Notes
- Awaitables that return pending now resume at a labeled state to avoid
  re-running pre-await side effects.
- Pending await resume targets are encoded in the state slot (negative, bitwise
  NOT of the resume op index) and decoded before dispatch.
- Channel send/recv yield on pending and resume at labeled states.
- `asyncio.sleep` honors delay/result and avoids busy-spin via scheduler sleep
  registration.
- Cancellation tokens are available with request-scoped defaults and task-scoped
  overrides; cancellation is cooperative via `molt.cancelled()` checks.

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
