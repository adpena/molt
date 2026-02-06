# Molt Roadmap (Active)

Canonical current status: `docs/spec/STATUS.md`. This roadmap is forward-looking.

## Legend
- **Status:** Implemented (done), Partial (some semantics missing), Planned (scoped but not started), Missing (no implementation), Divergent (intentional difference from CPython).
- **Priority:** P0 (blocker), P1 (high), P2 (medium), P3 (lower).
- **Tier/Milestone:** `TC*` (type coverage), `SL*` (stdlib), `DB*` (database), `DF*` (dataframe/pandas), `LF*` (language features), `RT*` (runtime), `TL*` (tooling), `M*` (syntax milestones).

## Parity-First Execution Plan
Guiding principle: lock CPython parity and robust test coverage before large optimizations or new higher-level surface area.

Parity gates (required before major optimizations that touch runtime, call paths, lowering, or object layout):
- Relevant matrix entries in `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md`, `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md`,
  `docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md`, `docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md`, and
  `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md` are updated to match the implementation status.
- Differential tests cover normal + edge-case behavior (exception type/messages, ordering, and protocol fallbacks).
- Native + WASM parity checks added or updated for affected behaviors.
- Runtime lifecycle plan tracked and up to date (`docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md`).

Plan (parity-first, comprehensive):
1) Matrix audit and coverage map: enumerate missing/partial cells in the matrices above, link each to at least one
   differential test, and ensure TODOs exist in code for remaining gaps.
2) Core object protocols: attribute access/descriptor binding, dunder fallbacks, container protocols
   (`__iter__`/`__len__`/`__contains__`/`__reversed__`), equality/ordering/hash/format parity, and strict exception behavior.
3) Call + iteration semantics: CALL_BIND/CALL_METHOD, `*args`/`**kwargs`, iterator error propagation, generators,
   coroutines, and async iteration; keep native + WASM parity in lockstep.
4) Stdlib core: builtins + `collections`/`functools`/`itertools`/`operator`/`heapq`/`bisect` to parity per
   `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md`, with targeted differential coverage.
5) Security + robustness tests: capability gating, invalid input handling, descriptor edge cases, and recursion/stack
   behavior to catch safety regressions early.

## Concurrency & Parallelism (Vision -> Plan)
- Default: CPython-correct asyncio semantics on a single-threaded event loop (deterministic ordering, structured cancellation).
- True parallelism is explicit: executors + isolated runtimes/actors with message passing.
- Shared-memory parallelism is opt-in, capability-gated, and limited to explicitly safe types.
- Current: runtime mutation is serialized by a GIL-like lock in the global runtime state; see `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`.

Planned milestones:
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P0, status:planned): Rust event loop + I/O poller with cancellation propagation and deterministic scheduling guarantees; expose as asyncio core.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P0, status:planned): full asyncio parity (tasks, task groups, streams, subprocess, executors) built on the runtime loop.
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define the per-runtime GIL strategy, runtime instance ownership model, and allowed cross-thread object sharing rules (see `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`).
- Implemented: explicit `PyToken` GIL token API and `with_gil`/`with_gil_entry` enforcement on runtime mutation entrypoints (see `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`).
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors, explicit message passing, and capability-gated shared-memory primitives.
- TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P1, status:planned): wasm host parity for the asyncio runtime loop, poller, sockets, and subprocess I/O.

## Performance
- Vector reduction kernels now cover `sum`/`prod`/`min`/`max` with trusted fast paths; next up: float reductions and typed-buffer kernels (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): float reductions + typed-buffer kernels).
- String kernel SIMD paths cover find/split/replace with Unicode-safe index translation; next: Unicode index caches and wider SIMD (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): Unicode index caches + wider SIMD).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): implement sharded/lock-free handle resolution and track lock-sensitive benchmark deltas (attr access, container ops).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): stream print writes to avoid building intermediate output strings for large payloads.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): pre-size `dict.fromkeys` using iterable length hints to reduce rehashing.
- Implemented: websocket readiness integration via io_poller for native + wasm (`molt_ws_wait_new`) to avoid busy-polling and enable batch wakeups.
- TODO(perf, owner:runtime, milestone:RT3, priority:P2, status:planned): cache mio websocket poll streams/registrations to avoid per-wait `TcpStream` clones.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): re-enable safe direct-linking by relocating the runtime heap base or enforcing non-overlapping memory layouts to avoid wasm-ld in hot loops.
- Implemented: removed linked-wasm static intrinsic dispatch workaround for channel intrinsics by canonicalizing the runtime channel-handle ABI to 64-bit bits values, restoring stable dynamic intrinsic call dispatch.
- Implemented: use i32 locals for wasm pointer temporaries in the backend to trim wrap/extend churn.
- Wasmtime host runner is available (`molt-wasm-host`) with shared memory/table wiring and a `tools/bench_wasm.py --runner wasmtime` path for perf comparison against Node.
- Implemented: Wasmtime DB host delivery is non-blocking via `molt_db_host_poll` with stream semantics + cancellation checks; parity coverage still pending.

## Type Coverage
- memoryview (Partial): multi-dimensional `format`/`shape`/`strides`/`nbytes` + `cast`, tuple scalar indexing, 1D slicing/assignment for bytes/bytearray-backed views.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): memoryview multi-dimensional slicing + sub-views (C-order parity).
- Implemented: BigInt heap fallback + arithmetic parity beyond 47-bit inline ints.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`) + `__set_name__` hook.
- Implemented: C3 MRO + multiple inheritance for attribute lookup + `super()` resolution + data descriptor precedence.
- Implemented: reflection builtins (`type`, `isinstance`, `issubclass`, `object`) for base chains (no metaclasses).
- Implemented: BaseException root + exception chaining (`__cause__`, `__context__`, `__suppress_context__`) + `__traceback__` objects with line markers + StopIteration.value propagation.
- Implemented: ExceptionGroup/except* semantics (match/split/derive/combine) with BaseExceptionGroup hierarchy + try/except* lowering (native + wasm).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): tighten exception `__init__` + subclass attribute parity (ExceptionGroup tree).
- Implemented: dict subclass storage lives outside instance `__dict__`, matching CPython attribute/mapping separation.
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame/traceback objects to CPython parity (`f_back`, `f_globals`, `f_locals`, live `f_lasti`/`f_lineno`).
- Implemented: descriptor deleter semantics (`__delete__`, property deleter) + attribute deletion wiring.
- Implemented: set literals/constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- Implemented: augassign slice targets (`seq[a:b] += ...`) with extended-slice length checks.
- Implemented: format mini-language for ints/floats + f-string conversion flags (`!r`, `!s`, `!a`) + `str.format` field parsing (positional/keyword, attr/index, conversion flags, nested specs).
- Implemented: call argument binding for Molt functions (positional/keyword/`*args`/`**kwargs`) with pos-only/kw-only enforcement.
- Implemented: variadic call trampoline lifts compiled call-arity ceiling beyond 12 (native + wasm).
- Implemented: PEP 649 lazy annotations (`__annotate__` + lazy `__annotations__` cache for module/class/function; VALUE/STRING formats).
- Implemented: PEP 585 generic aliases for builtin containers (`list`/`dict`/`tuple`/`set`/`frozenset`/`type`) with `__origin__`/`__args__`.
- Implemented: PEP 584 dict union (`|`, `|=`), PEP 604 union types (`X | Y`), and zip(strict) (PEP 618).
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): derive `types.GenericAlias.__parameters__` from `TypeVar`/`ParamSpec`/`TypeVarTuple` once typing metadata lands.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): implement full PEP 695 type params (bounds/constraints/defaults, ParamSpec/TypeVarTuple, alias metadata).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P1, status:partial): implement `str.isdigit`.
- Implemented: lambda lowering with closures, defaults, and kw-only/varargs support.
- Implemented: `sorted()` builtin with stable ordering + key/reverse (core ordering types).
- Implemented: `sorted()` enforces keyword-only `key`/`reverse` arguments (CPython parity).
- Implemented: `list.sort` with key/reverse and rich-compare fallback for user-defined types.
- Implemented: `str.lower`/`str.upper`, `list.clear`/`list.copy`/`list.reverse`, and `dict.setdefault`/`dict.update`.
- Implemented: container dunder/membership fallbacks (`__contains__`/`__iter__`/`__getitem__`) and builtin class method access for list/dict/str/bytes/bytearray.
- Implemented: dynamic call binding for bound methods/descriptors with builtin defaults + expanded class decorator parity coverage.
- Implemented: print keyword-argument parity tests (`sep`, `end`, `file`, `flush`) for native + wasm.
- Implemented: compiled `sys.argv` initialization for native + wasm harness; TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): filesystem-encoding + surrogateescape decoding parity.
- Implemented: `sys.executable` override via `MOLT_SYS_EXECUTABLE` (diff harness pins it to the host Python to avoid recursive `-c` subprocess spawns).
- TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): fill out code object fields (`co_varnames`, arg counts, `co_linetable`) for parity.
- TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): importlib.machinery pending parity
- Implemented: iterator/view helper types now map to concrete builtin classes so `collections.abc` imports and registers without fallback/guards.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): bootstrap `sys.stdout` so print(file=None) always honors the sys stream.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:missing): expose file handle `flush()` and wire wasm parity for file flushing.
- TODO(tests, owner:frontend, milestone:TC2, priority:P2, status:planned): KW_NAMES error-path coverage (duplicate keywords, positional-only violations) in differential tests.
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): security-focused attribute access tests (descriptor exceptions, `__getattr__` recursion traps).
- Implemented: async comprehensions (async for/await) with nested + await-in-comprehension coverage.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): matmul dunder hooks (`__matmul__`/`__rmatmul__`) with buffer2d fast path.
- Partial: wasm generator state machines + closure slot intrinsics + channel send/recv intrinsics + async pending/block_on parity landed; remaining scheduler semantics (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): wasm scheduler semantics).
- Implemented: wasm async state dispatch uses encoded resume targets to avoid state-id collisions and keeps state/poll locals distinct (prevents pending-state corruption on resume).
- Implemented: async iterator protocol (`__aiter__`/`__anext__`) with `aiter`/`anext` lowering and `async for` support; sync-iter fallback remains for now.
- Implemented: `anext(..., default)` awaitable creation outside `await`.
- Implemented: `async with` lowering for `__aenter__`/`__aexit__`.
- Implemented: cancellation token plumbing with request-default inheritance and task override; automatic cancellation injection into awaits still pending (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): cancellation injection on await).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): native-only tokio host adapter for compiled async tasks with determinism guard + capability gating (no WASM impact).
- TODO(syntax, owner:frontend, milestone:M3, priority:P2, status:missing): structural pattern matching (`match`/`case`) lowering and semantics (see `docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md`).
- TODO(opcode-matrix, owner:frontend, milestone:M3, priority:P2, status:missing): `MATCH_*` opcode coverage for pattern matching (see `docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md`).
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): f-string format specifiers and debug spec (`f"{x:.2f}"`, `f"{x=}"`) parity (see `docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md`).
- TODO(syntax, owner:frontend, milestone:M3, priority:P3, status:missing): type alias statement (`type X = ...`) and generic class syntax (`class C[T]: ...`) coverage (see `docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md`).
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator opcode coverage and lowering gaps (see `docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md`).
- TODO(compiler, owner:compiler, milestone:TC2, priority:P0, status:partial): fix async lowering/back-end verifier for `asyncio.gather` poll paths (dominance issues) and wasm stack-balance errors; async protocol parity tests currently fail.
- Implemented: generator/async poll trampolines are task-aware (generator/coroutine/asyncgen) so wasm no longer relies on arity overrides.
- TODO(perf, owner:compiler, milestone:TC2, priority:P2, status:planned): optimize wasm trampolines with bulk payload initialization and shared helpers to cut code size and call overhead.
- Implemented: cached task-trampoline eligibility on function headers to avoid per-call attribute lookups.
- Implemented: coroutine trampolines reuse the current cancellation token to avoid per-call token allocations.
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector implementation (see `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md`).
- Implemented: runtime lifecycle refactor moved caches/pools/async registries into `RuntimeState`, removed lazy_static globals, and added TLS guard cleanup for user threads (see `docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md`).
- Implemented: host pointer args use raw pointer ABI; strict-provenance Miri stays green (pointer registry remains for NaN-boxed handles).
- TODO(runtime-provenance, owner:runtime, milestone:RT2, priority:P2, status:planned): bound or evict transient const-pointer registrations in the pointer registry.
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence policy (see `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md`).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define and implement `libmolt` C API shim + `Py_LIMITED_API` target (see `docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md`).

## File/Open Parity Checklist (Production)
Checklist:
- `open()` signature: file/mode/buffering/encoding/errors/newline/closefd/opener + path-like + fd-based open (done; utf-8/utf-8-sig/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/ascii/latin-1 only, opener error text, and wasm parity still tracked below).
- Mode parsing: validate combinations (`r/w/a/x`, `b/t`, `+`), default mode behavior, and text/binary exclusivity (done).
- Buffering: `buffering=0/1/n/-1` semantics (binary-only unbuffered, line buffering in text, default sizes, flush behavior) (partial: line buffering + unbuffered text guard in place; default size + buffering strategy pending).
- Text layer: encoding/errors/newline handling, universal newlines, and `newline=None/'\\n'/'\\r'/'\\r\\n'` parity (partial: newline handling + utf-8/utf-8-sig/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/ascii/latin-1/utf-16/utf-32 decode/encode; other codecs pending; encode error handlers include namereplace+xmlcharrefreplace).
- File object API: `read`, `readinto`, `write`, `writelines`, `readline(s)`, `seek`, `tell`, `truncate`, `flush`, `close`, `fileno`, `isatty`, `readable`, `writable`, `seekable`, `name`, `mode`, `closed`, `__iter__`/`__next__` (partial: core methods/attrs implemented; Windows isatty pending).
- Context manager: `__enter__`/`__exit__` semantics, close-on-exit, exception propagation, idempotent close (done).
- Capability gating: enforce `fs.read`/`fs.write` and error surfaces per operation (done).
- Native + WASM parity: file APIs and error messages aligned across hosts (pending: open parity tests + wasm host parity coverage).
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): align file handle type names in error/AttributeError messages with CPython _io.* wrappers.)

Test plan (sign-off):
- Differential tests: `tests/differential/planned/file_open_modes.py`, `file_buffering_text.py`,
  `file_text_encoding_newline.py`, `file_iteration_context.py`, `file_seek_tell_fileno.py` (move to verified subset on parity).
- Pytest unit tests: invalid mode/buffering/encoding/newline combos, fd-based `open`, `closefd`/`opener` errors, path-like objects.
- WASM parity: harness tests for read/write/line iteration using temp files via Node/WASI host I/O.
- Security/robustness: fuzz mode strings + newline values, and validate close/idempotency + leak-free handles.
- Windows parity: newline translation + path handling coverage in CI.
- Scaffolded tests live in `tests/differential/planned/` + `tests/wasm_planned/` until file/open parity lands.

Sign-off criteria:
- All above tests pass on 3.12/3.13/3.14 + wasm parity runs; matrices + STATUS updated; no capability bypass.

## Stdlib
- Partial: importable `builtins` module binding supported builtins (attribute gaps tracked in the matrix).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill `builtins` module attribute coverage.)
- Partial: asyncio shim (`run`/`sleep` lowered to runtime with delay/result semantics; `wait`/`wait_for`/`shield` + basic `gather` supported; `set_event_loop`/`new_event_loop` stubs); loop/task APIs still pending (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task API parity).
- Partial: shims for `warnings`, `traceback`, `types`, `inspect`, `ast`, `ctypes`, `uuid`, `urllib.parse`, `fnmatch`, `copy`, `pickle` (protocol 0 only), `pprint`, `string`, `struct`, `typing`, `sys`, `os`, `json`, `asyncio`, `shlex` (`quote`), `threading`, `weakref`, `bisect`, `heapq`, `functools`, `itertools`, `zipfile`, `zipimport`, and `collections` (capability-gated env access).
- Partial: `decimal` shim backed by libmpdec intrinsics (contexts/traps/flags, quantize/compare/normalize/exp/div, `as_tuple`, `str`/`repr`/float conversions).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Decimal arithmetic + formatting parity (add/sub/mul/pow/sqrt/log/ln, quantize edge cases, NaN payloads).)
- Implemented: strict intrinsics registry + removal of CPython shim fallbacks in tooling/tests; JSON/MsgPack helpers now use runtime intrinsics only.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close parity gaps for `ast`, `ctypes`, `urllib.parse`, and `uuid` (see stdlib matrix).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand zipfile/zipimport with bytecode caching + broader archive support.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures` (spawn-based partial; IPC + lifecycle parity pending).
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.
- Partial: capability-gated `socket`/`select`/`selectors` backed by runtime sockets + io_poller (native + wasmtime host implemented); Node/WASI host bindings now wired in `run_wasm.js`, browser host supports WebSocket-backed stream sockets + io_poller readiness while UDP/listen/server sockets remain unsupported.
  (TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): expand browser socket coverage (UDP/listen/server sockets) + parity tests.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (Encoder/Decoder classes, JSONDecodeError details, runtime fast-path parser).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand hashlib/hmac coverage for optional OpenSSL algorithms (sha512_224/sha512_256, ripemd160, md4) and add parity tests for advanced digestmod usage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `struct` shim supports `pack`/`unpack`/`calcsize` for `i`/`d` only; expand to full format/alignment parity plus `pack_into`/`unpack_from`/`iter_unpack`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `time` module surface (altzone/daylight + timegm/mktime) + deterministic clock policy.
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale data for `time.localtime`/`time.strftime` on wasm hosts.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): `weakref.finalize` atexit registry pending until atexit hooks are available.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/backslashreplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (registry/lookup + encodings package + incremental/stream codecs + error-handler registration); base encode/decode intrinsics are present.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `pickle` protocol 1+ and broader type coverage (bytes/bytearray, memo cycles).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): finish remaining `math` intrinsics (determinism policy); predicates, `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc` are now wired in Rust.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): fill out `types` shims (TracebackType, FrameType, FunctionType, coroutine/asyncgen types, etc).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:missing): implement `types.new_class`, `types.prepare_class`, `types.resolve_bases`, and `types.get_original_bases`, plus `DynamicClassAttribute` descriptor parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): replace placeholder iterator/view types (`object`/`type`) so ABC registration doesn't need guards.
- TODO(tests, owner:runtime, milestone:SL1, priority:P1, status:partial): expand native+wasm codec parity coverage for binary/floats/large ints/tagged values + deeper container shapes.
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
- Import-only allowlist expanded for `binascii`, `unittest`, `site`, `sysconfig`, `collections.abc`, `importlib`, and `importlib.util`; planned additions now cover the remaining CPython 3.12+ stdlib surface (see `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` Section 3.0b), including `annotationlib`, `compileall`, `configparser`, `difflib`, `dis`, `encodings`, `tokenize`, `trace`, `xmlrpc`, and `zipapp` (API parity pending; TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + tests).

## Compatibility Matrix Execution Plan (Next 8 Steps)
1) Done: TC2 iterable unpacking + starred targets in assignment/for targets (tests + spec/status updates).
2) TC2: remaining StopIteration semantics (sync/async) with differential coverage (StopIteration.value propagation done).
3) TC2: builtin conversions (`bool`, `str`) with hook/error parity.
- Implemented: `str(bytes, encoding, errors)` decoding for bytes-like inputs (matches `bytes.decode` codec/handler coverage).
4) Done: TC2 async comprehensions lowering + runtime support with parity tests.
5) TC2/TC3: reflection builtins, CPython `hash` parity (`PYTHONHASHSEED`) + `format`/rounding; update tests + docs.
   Implemented: object-level `__getattribute__`/`__setattr__`/`__delattr__` builtins.
6) SL1: `functools` (`lru_cache`, `partial`, `reduce`) with compile-time lowering and deterministic cache keys; `cmp_to_key`/`total_ordering` landed.
7) SL1: `itertools` + `operator` intrinsics plus `heapq` fast paths; `bisect`/`heapq` shims landed (fast paths now wired).
8) SL1: finish `math` intrinsics beyond `log`/`log2`/`exp`/`sin`/`cos`/`acos`/`lgamma` and trig/hyperbolic (remaining: determinism policy), plus deterministic `array`/`struct` layouts with wasm/native parity tests.

## Offload / IPC
- Partial: `molt_accel` v0 scaffolding (stdio framing + client + decorator) with auto cancel-check detection, payload/response byte metrics, and shared demo payload builders; `molt_worker` stdio shell with demo handlers and compiled dispatch (`list_items`/`compute`/`offload_table`/`health`), plus optional worker pooling via `MOLT_ACCEL_POOL_SIZE`.
  (TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): finalize accel retry/backoff + non-demo handler coverage.)
- Implemented: compiled export loader + manifest validation (schema, reserved-name filtering, error mapping) with queue/timeout metrics.
- Implemented: worker tuning via `MOLT_WORKER_THREADS` and `MOLT_WORKER_MAX_QUEUE` (CLI overrides).
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): propagate cancellation into real DB tasks; extend compiled handlers beyond demo coverage.

## DB
- Partial: `molt-db` pool skeleton (bounded, sync), feature-gated async pool primitive, SQLite connector (native-only; wasm parity pending), and async Postgres connector with statement cache; `molt_worker` exposes `db_query`/`db_exec` for SQLite + Postgres (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity).
- Top priority: wasm parity for DB connectors before expanding DB adapters or query-builder ergonomics.
- Implemented: wasm DB client shims + parity test (`molt_db` async helper) consume response streams and surface bytes/Arrow IPC; Node/WASI host adapter forwards `db_query`/`db_exec` to `molt-worker` via `run_wasm.js`.

## Parity Cluster Plan (Next)
- 1) Async runtime core: Task/Future APIs, scheduler, contextvars, and cancellation injection into awaits/I/O. Key files: `runtime/molt-runtime/src/lib.rs`, `src/molt/stdlib/asyncio.py`, `src/molt/stdlib/contextvars.py`, `docs/spec/STATUS.md`. Outcome: asyncio loop/task parity for core patterns. Validation: new unit + differential tests; `tools/dev.py test`.
- 2) Capability-gated async I/O: sockets/SSL/selectors/time primitives with cancellation propagation. Key files: `docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md`, `docs/spec/areas/runtime/0505_IO_ASYNC_AND_CONNECTORS.md`, `runtime/molt-runtime/src/lib.rs`. Outcome: async I/O primitives usable by DB/HTTP stacks. Validation: I/O unit tests + fuzzed parser tests + wasm/native parity checks.
- Implemented: native host-level websocket connect hook for `molt_ws_connect` with capability gating for production socket usage.
- 3) DB semantics expansion: implement `db_exec`, transactions, typed param mapping; add multirange + array lower-bound decoding. Key files: `runtime/molt-db/src/postgres.rs`, `runtime/molt-worker/src/main.rs`, `docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md`, `docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md`, `docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md`. Outcome: production-ready DB calls with explicit write gating and full type decoding. Validation: dockerized Postgres integration + cancellation tests.
- 4) WASM DB parity: define WIT/host calls for DB access and implement wasm connectors in molt-db. Key files: `wit/molt-runtime.wit`, `runtime/molt-runtime/src/lib.rs`, `runtime/molt-db/src/lib.rs`, `docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md`. Outcome: wasm builds can execute DB queries behind capability gates. Validation: wasm harness tests + native/wasm result parity.
- 5) Framework-agnostic adapters: finalize `molt_db_adapter` + helper APIs for Django/Flask/FastAPI with shared payload builders. Key files: `src/molt_db_adapter/`, `docs/spec/areas/db/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md`, `demo/`, `tests/`. Outcome: same IPC contract across frameworks with consistent error mapping. Validation: integration tests in sample Django/Flask/FastAPI apps.
- 6) Production hardening: propagate cancellation into compiled entrypoints/DB tasks, add pool/queue metrics, run bench harness. Key files: `runtime/molt-worker/src/main.rs`, `bench/scripts/`, `docs/spec/areas/demos/0910_REPRO_BENCH_VERTICAL_SLICE.md`. Outcome: stable P99/P999 and reliable cancellation/backpressure. Validation: `bench/scripts/run_stack.sh` + stored JSON results.

## Tooling
- Keep type facts + `ty` validation wired into build/lint flows and surface regressions early.
- Implemented: CLI wrappers for `run`/`test`/`diff`/`bench`/`profile`/`lint`/`doctor`/`package`/`publish`/`verify`,
  plus determinism/capability checks and vendoring materialization (publish supports local + HTTP(S) registry targets).
- Implemented: initial cross-target native builds (Cranelift target + zig link); next: cross-linker configuration,
  target capability manifests, and runtime cross-build caching (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:partial): cross-target ergonomics).
- CLI Roadmap (plan):
  - Build cache clarity: `--cache-report` by default in `--json`, `molt clean --cache`, and cache hit/miss summaries with input fingerprints.
  - Build UX polish: stable `--out-dir` defaults (`$MOLT_HOME/build/<entry>`), explicit `--emit` artifacts, and `--emit-ir` + `--emit-json` dumps.
  - Profiles + metadata: `--profile {dev,release}` consistency across backend/runtime, and JSON metadata with toolchain hashes.
  - Config introspection: `molt config` shows merged `molt.toml`/`pyproject.toml` plus resolved build settings.
  - Cross-target ergonomics: cache-aware runtime builds, target flag presets, and capability manifest helpers.
- Track complex performance work in `OPTIMIZATIONS_PLAN.md` before large refactors.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:planned): replace pointer-registry locks with sharded or lock-free lookups once registry load is characterized.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): remove legacy `.molt/` clean-up path after MOLT_HOME/MOLT_CACHE migration is complete.
- TODO(tooling, owner:release, milestone:TL2, priority:P2, status:planned): formalize release tagging (start at `v0.0.001`, increment thousandth) and require super-bench stats for README performance summaries.

## Django Demo Path (Draft, 5-Step)
- Step 1 (Core semantics): close TC1/TC2 gaps in `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md` for Django-heavy types (dict/list/tuple/set/str, iter/len, mapping protocol, kwargs/varargs ordering per docs/spec/areas/compat/0016_ARGS_KWARGS.md, descriptor hooks, class `__getattr__`/`__setattr__`).
- Step 2 (Import/module system): package resolution + module objects, `__import__`, and a deterministic `sys.path` policy; unblock `importlib` basics.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root build discovery (namespace packages + PYTHONPATH roots done; remaining: deterministic graph caching + `__init__` edge cases).
- Step 3 (Stdlib essentials): advance `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` for `functools`, `itertools`, `operator`, `collections`, `contextlib`, `inspect`, `typing`, `dataclasses`, `enum`, `re`, and `datetime` to Partial with tests.
- Step 4 (Async/runtime): production-ready asyncio loop/task APIs, contextvars, cancellation injection, and long-running workload hardening.
- Step 5 (I/O + web/DB): capability-gated `os`, `sys`, `pathlib`, `logging`, `time`, `selectors`, `socket`, `ssl`; ASGI/WSGI surface, HTTP parsing, and DB client + pooling/transactions (start sqlite3 + minimal async driver), plus deterministic template rendering.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining `pathlib` parity gaps (glob edge cases, Windows drive/anchor/path flavor semantics, and broader PurePath/PurePosixPath API surface).
- Cross-framework note: DB IPC payloads and adapters must remain framework-agnostic to support Django/Flask/FastAPI.
