# STATUS (Canonical)

Last updated: 2026-01-13

This document is the source of truth for Molt's current capabilities and
limitations. Update this file whenever behavior or scope changes, and keep
README/ROADMAP in sync.

## Capabilities (Current)
- Tier 0 structification for typed classes (fixed layout).
- Native async/await lowering with state-machine poll loops.
- Call argument binding for Molt-defined functions: positional/keyword/`*args`/`**kwargs` with pos-only/kw-only enforcement.
- Call argument evaluation matches CPython ordering (positional/`*` left-to-right, then keyword/`**` left-to-right).
- Function decorators (non-contextmanager) are lowered; sync/async free-var closures are captured via closure tuples.
- Local/closure function calls (decorators, `__call__`) lower through dynamic call paths when not allowlisted.
- Async iteration: `__aiter__`/`__anext__`, `aiter`/`anext`, and `async for`.
- Async context managers: `async with` lowering for `__aenter__`/`__aexit__`.
- `anext(..., default)` awaitable creation outside `await`.
- AOT compilation via Cranelift for native targets.
- Differential testing vs CPython 3.12 for supported constructs.
- Molt packages for Rust-backed deps using MsgPack/CBOR and Arrow IPC.
- Sets: literals + constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- Numeric builtins: `int()`/`abs()`/`divmod()`/`round()`/`math.trunc()` with `__int__`/`__index__`/`__round__`/`__trunc__` hooks and base parsing for string/bytes.
- Formatting builtins: `ascii()`/`bin()`/`oct()`/`hex()` with `__index__` fallback and CPython parity errors for non-integers.
- `chr()` and `ord()` parity errors for type/range checks; `chr()` accepts `__index__`.
- BigInt heap fallback for ints beyond inline range (arithmetic/bitwise/shift parity for large ints).
- Format mini-language for ints/floats + f-string conversion flags (`!r`, `!s`, `!a`).
- memoryview exposes 1D `format`/`shape`/`strides`/`nbytes` for bytes/bytearray views.
- `str.count` supports start/end slices with Unicode-aware offsets.
- `str.lower`/`str.upper`, `list.clear`/`list.copy`/`list.reverse`, and
  `dict.clear`/`dict.copy`/`dict.popitem`/`dict.setdefault`/`dict.update`.
- `list.extend` accepts iterable inputs (range/generator/etc.) via the iter protocol.
- `dict()` supports positional mapping/iterable inputs (keys/`__getitem__` mapping fallback) plus keyword/`**` expansion
  (string key enforcement for `**`); `dict.update` mirrors the mapping fallback.
- `bytes`/`bytearray` constructors accept int counts, iterable-of-ints, and str+encoding (`utf-8`/`latin-1`/`ascii`/`utf-16`/`utf-32`) with basic error handlers (`strict`/`ignore`/`replace`) and parity errors for negative counts/range checks.
- `dict`/`dict.update` raise CPython parity errors for non-iterable elements and invalid pair lengths.
- `len()` falls back to `__len__` with CPython parity errors for negative, non-int, and overflow results.
- Dict/set key hashability parity for common unhashable types (list/dict/set/bytearray/memoryview).
- Importable `builtins` module binds supported builtins (see stdlib matrix).
- `enumerate` builtin returns an iterator over `(index, value)` with optional `start`.
- Builtin function objects for allowlisted builtins (`any`, `all`, `abs`, `ascii`, `bin`, `oct`, `hex`, `chr`, `ord`, `divmod`, `callable`, `repr`, `getattr`, `hasattr`, `round`, `next`, `anext`, `print`, `super`, `sum`, `min`, `max`).
- Builtin reductions: `sum`, `min`, `max` with key/default support for numeric comparisons.
- Indexing honors user-defined `__getitem__`/`__setitem__` when builtin paths do not apply.
- CPython shim: minimal ASGI adapter for http/lifespan via `molt.asgi.asgi_adapter`.
- `molt_accel` client/decorator expose before/after hooks, metrics callbacks, and cancel-checks; wire selection honors `MOLT_WORKER_WIRE`/`MOLT_WIRE`.
- `molt_worker` enforces cancellation/timeout checks in the fake DB path and compiled dispatch loops, validates export manifests, and reports queue/pool metrics per request.
- WASM harness runs via `run_wasm.js` with shared memory/table and direct runtime imports (legacy wrapper fallback via `MOLT_WASM_LEGACY=1`), including async/channel benches on WASI.
- Instance `__getattr__`/`__setattr__` hooks for user-defined classes.
- Instance `__getattribute__` hooks for user-defined classes.
- `**kwargs` expansion accepts dicts and mapping-like objects with `keys()` + `__getitem__`.
- `functools.partial` and `functools.lru_cache` accept `*args`/`**kwargs`, and `functools.wraps` honors assigned/updated.
- C3 MRO + multiple inheritance for attribute lookup, `super()` resolution, and descriptor precedence for
  `__get__`/`__set__`/`__delete__`.
- Exceptions: BaseException root, non-string messages lowered through `str()`, and `__traceback__` captured as a
  tuple of function names.
- Recursion limits enforced via call dispatch guards with `sys.getrecursionlimit`/`sys.setrecursionlimit` wired to runtime limits.
- `molt_accel` is packaged as an optional dependency group (`[project.optional-dependencies].accel`) with a packaged default exports manifest; the decorator falls back to `molt-worker` in PATH when `MOLT_WORKER_CMD` is unset. A demo Django app/worker scaffold lives under `demo/`.
- `molt_worker` compiled-entry dispatch is wired for demo handlers (`list_items`/`compute`/`offload_table`/`health`) using codec_in/codec_out; other exported names still return a clear error until compiled handlers exist.

## Limitations (Current)
- Classes/object model: no metaclasses or dynamic `type()` construction.
- Attributes: fixed struct fields with dynamic instance-dict fallback; no
  user-defined `__slots__` beyond dataclass lowering; object-level
  `__getattr__`/`__getattribute__`/`__setattr__` are not exposed as builtins.
- Dataclasses: compile-time lowering for frozen/eq/repr/slots; no
  `default_factory`, `kw_only`, or `order`.
- Call binding: allowlisted stdlib modules now permit dynamic calls (keyword/variadic via `CALL_BIND`);
  direct-call fast paths still require allowlisted functions and positional-only calls. Non-allowlisted imports
  remain blocked unless the bridge policy is enabled.
- Closures for generator functions and generator decorators are still pending.
- Exceptions: `try/except/else/finally` + `raise`/reraise; `__traceback__` lacks full
  traceback objects/line info and exception args remain message-only (see type coverage matrix).
- Imports: static module graph only; no dynamic import hooks or full package
  resolution.
- Asyncio: shim exposes `run`/`sleep` plus `set_event_loop`/`new_event_loop` stubs; loop/task APIs still pending and no
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
- collections: shim `Counter`/`defaultdict` are wrapper implementations (not dict subclasses); `defaultdict`
  default_factory is only fast-pathed for `list`.

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
- WASM runs a single-threaded scheduler loop (no background workers); pending
  sleeps are handled by blocking registration in the same task loop.

## Stdlib Coverage
- Partial shims: `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`,
  `pprint`, `string`, `typing`, `sys`, `os`, `asyncio`, `threading`,
  `functools`, `itertools`, `operator`, `collections`.
- Import-only stubs: `collections.abc`, `importlib`, `importlib.util`.
- See `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` for the full matrix.

## Django Demo Blockers (Current)
- Missing `functools`/`itertools`/`operator`/`collections` parity needed for common Django internals.
- Async loop/task APIs + `contextvars` are incomplete; cancellation injection and long-running workload hardening are pending.
- Capability-gated I/O/runtime modules (`os`, `sys`, `pathlib`, `logging`, `time`, `selectors`) need deterministic parity.
- HTTP/ASGI runtime surface and DB driver/pool integration are not implemented (shim adapter exists).
- Descriptor hooks still lack metaclass behaviors, limiting idiomatic Django patterns.

## Tooling + Verification
- CI enforces lint, type checks, Rust fmt/clippy, differential tests, and perf
  smoke gates.
- Use `tools/dev.py lint` and `tools/dev.py test` for local validation.
- WIT interface contract lives at `wit/molt-runtime.wit` (WASM runtime intrinsics).
- Experimental single-module wasm link attempt via `tools/wasm_link.py` (requires `wasm-ld`); run via `MOLT_WASM_LINKED=1`.

## Known Gaps
- uv-managed Python 3.14 hangs on arm64; system Python 3.14 used as workaround.
- Browser host for WASM is still pending; current harness targets WASI via
  `run_wasm.js` and uses a single-threaded scheduler.
- True single-module WASM link (no JS boundary) is still pending; current direct-link harness still uses a JS stub for `molt_call_indirect1`.
- TODO(runtime-provenance, owner:runtime, milestone:RT1): remove handle-table lock overhead via sharded or lock-free lookups.
- Single-module wasm linking remains experimental; wasm-ld now links relocatable output when `MOLT_WASM_LINK=1`, but broader coverage + table/element relocation validation and removal of the JS `molt_call_indirect1` stub are still pending.
