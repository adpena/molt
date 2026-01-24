# STATUS (Canonical)

Last updated: 2026-01-22

This document is the source of truth for Molt's current capabilities and
limitations. Update this file whenever behavior or scope changes, and keep
README/ROADMAP in sync.

## Capabilities (Current)
- Tier 0 structification for typed classes (fixed layout).
- Native async/await lowering with state-machine poll loops.
- Unified task ABI for futures/generators with kind-tagged allocation shared across native and wasm backends.
- Call argument binding for Molt-defined functions: positional/keyword/`*args`/`**kwargs` with pos-only/kw-only enforcement.
- Call argument evaluation matches CPython ordering (positional/`*` left-to-right, then keyword/`**` left-to-right).
- Compiled call dispatch supports arbitrary positional arity via a variadic trampoline (native + wasm).
- Function decorators (non-contextmanager) are lowered for sync/async/generator functions; free-var closures and `nonlocal` rebinding are captured via closure tuples.
- Class decorators are lowered after class creation (dataclass remains compile-time), including stacked decorator factories and callable-object decorators with CPython evaluation order.
- `for`/`while`/`async for` `else` blocks are supported with break-aware lowering (async flags persist across awaits).
- Local/closure function calls (decorators, `__call__`) lower through dynamic call paths when not allowlisted; bound method/descriptor calls route through `CALL_BIND`/`CALL_METHOD` with builtin default binding.
- Async iteration: `__aiter__`/`__anext__`, `aiter`/`anext`, and `async for`.
- Async context managers: `async with` lowering for `__aenter__`/`__aexit__`.
- `anext(..., default)` awaitable creation outside `await`.
- AOT compilation via Cranelift for native targets.
- Differential testing vs CPython 3.12 for supported constructs (PEP 649 annotation parity validated against CPython 3.14).
- PEP 649 lazy annotations: compiler emits `__annotate__` for module/class/function, `__annotations__` computed lazily and cached (formats 1/2: VALUE/STRING).
- PEP 585 generic aliases for builtin containers (`list`/`dict`/`tuple`/`set`/`frozenset`/`type`) with `__origin__`/`__args__`.
- Molt packages for Rust-backed deps using MsgPack/CBOR and Arrow IPC.
- Sets: literals + constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`) over set/frozenset/dict view RHS; `frozenset` constructor + algebra; set/frozenset method attributes for union/intersection/difference/symmetric_difference, update variants, copy/clear, and isdisjoint/issubset/issuperset.
- Numeric builtins: `int()`/`abs()`/`divmod()`/`round()`/`math.trunc()` with `__int__`/`__index__`/`__round__`/`__trunc__` hooks and base parsing for string/bytes.
- Formatting builtins: `ascii()`/`bin()`/`oct()`/`hex()` with `__index__` fallback and CPython parity errors for non-integers.
- `chr()` and `ord()` parity errors for type/range checks; `chr()` accepts `__index__` and `ord()` enforces length-1 for `str`/`bytes`/`bytearray`.
- BigInt heap fallback for ints beyond inline range (arithmetic/bitwise/shift parity for large ints).
- Bitwise invert (`~`) supported for ints/bools/bigints (bool returns int result).
- Format mini-language for ints/floats + `__format__` dispatch + `str.format` field resolution (positional/keyword, attr/index, conversion flags, nested format specs).
- memoryview exposes `format`/`shape`/`strides`/`nbytes`, `cast`, tuple scalar indexing, and 1D slicing/assignment for bytes/bytearray-backed views.
- `str.find`/`str.count`/`str.startswith`/`str.endswith` support start/end slices with Unicode-aware offsets; `str.split`/`str.rsplit` support `None` separators and `maxsplit` for str/bytes/bytearray; `str.replace` supports `count`; `str.strip`/`str.lstrip`/`str.rstrip` support default whitespace and `chars` argument; `str.join` accepts arbitrary iterables.
- `str.lower`/`str.upper`/`str.capitalize`, list methods (`append`/`extend`/`insert`/`remove`/`pop`/`count`/`index` with start/stop + parity errors, `clear`/`copy`/`reverse`/`sort`),
  and `dict.clear`/`dict.copy`/`dict.popitem`/`dict.setdefault`/`dict.update`/`dict.fromkeys`.
- List dunder arithmetic methods (`__add__`/`__mul__`/`__rmul__`/`__iadd__`/`__imul__`) are available for dynamic access and follow CPython error behavior.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full `re` syntax/flags + group semantics (literal-only `search`/`match`/`fullmatch` are currently supported).
- Builtin containers expose `__iter__`/`__len__`/`__contains__`/`__reversed__` (where defined) for list/dict/str/bytes/bytearray, including class-level access to builtin methods. Item dunder access via getattr is available for dict/list/bytearray/memoryview (`__getitem__`/`__setitem__`/`__delitem__`).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): move dict subclass storage out of instance `__dict__` (currently uses `__molt_dict_data__` key) to avoid attribute leakage and match CPython mapping/attribute separation.
- Membership tests (`in`) honor `__contains__` and iterate via `__iter__`/`__getitem__` fallbacks for user-defined objects.
- `list.extend` accepts iterable inputs (range/generator/etc.) via the iter protocol.
- Iterable unpacking in assignment/loop targets (including starred targets) with CPython-style error messages.
- `for`/`async for` `else` blocks execute when loops exhaust without `break`.
- Indexing and slicing honor `__index__` for integer indices (including slice bounds/steps).
- `slice` objects expose `start`/`stop`/`step`, `indices`, and hash/eq parity.
- Slice assignment/deletion parity for list/bytearray/memoryview (including `__index__` errors; memoryview delete raises `TypeError`).
- Augmented assignment (`+=`, `*=`, `|=`, `&=`, `^=`, `-=`) uses in-place list/bytearray/set semantics for name/attribute/subscript targets.
- `dict()` supports positional mapping/iterable inputs (keys/`__getitem__` mapping fallback) plus keyword/`**` expansion
  (string key enforcement for `**`); `dict.update` mirrors the mapping fallback.
- `bytes`/`bytearray` constructors accept int counts, iterable-of-ints, and str+encoding (`utf-8`/`latin-1`/`ascii`/`utf-16`/`utf-32`) with basic error handlers (`strict`/`ignore`/`replace`) and parity errors for negative counts/range checks.
- `bytes`/`bytearray` methods `find`/`count` (bytes-like/int needles)/`split`/`rsplit`/`replace`/`startswith`/`endswith`/`strip`/`lstrip`/`rstrip` (including start/end slices and tuple prefixes) and indexing return int values with CPython-style bounds errors.
- `dict`/`dict.update` raise CPython parity errors for non-iterable elements and invalid pair lengths.
- `len()` falls back to `__len__` with CPython parity errors for negative, non-int, and overflow results.
- Dict/set key hashability parity for common unhashable types (list/dict/set/bytearray/memoryview).
- Importable `builtins` module binds supported builtins (see stdlib matrix).
- `enumerate` builtin returns an iterator over `(index, value)` with optional `start`.
- `iter(callable, sentinel)`, `map`, `filter`, `zip`, and `reversed` return lazy iterator objects with CPython-style stop conditions.
- `iter(obj)` enforces that `__iter__` returns an iterator, raising `TypeError` with CPython-style messages for non-iterators.
- Builtin function objects for allowlisted builtins (`any`, `all`, `abs`, `ascii`, `bin`, `oct`, `hex`, `chr`, `ord`, `divmod`, `hash`, `callable`, `repr`, `format`, `getattr`, `hasattr`, `round`, `iter`, `next`, `anext`, `print`, `super`, `sum`, `min`, `max`, `sorted`, `map`, `filter`, `zip`, `reversed`).
- TODO(semantics, owner:frontend, milestone:LF1, priority:P2, status:partial): allow positional `key`/`reverse` arguments for `sorted()` to match CPython; currently treated as kw-only.
- Builtin reductions: `sum`, `min`, `max` with key/default support across core ordering types.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): dynamic execution builtins (`eval`, `exec`, `compile`) with sandboxing rules; regrtest `test_future_stmt` depends on `compile`.
- Differential parity probes for dynamic execution (`eval`/`exec`) are tracked in `tests/differential/planned/exec_*` and `tests/differential/planned/eval_*` and are **expected to fail** until sandboxed dynamic execution lands.
- `print` supports keyword arguments (`sep`, `end`, `file`, `flush`) with CPython-style type errors; `file=None` uses `sys.stdout` when initialized.
- Lexicographic ordering for `str`/`bytes`/`bytearray`/`list`/`tuple` (cross-type ordering raises `TypeError`).
- Ordering comparisons fall back to `__lt__`/`__le__`/`__gt__`/`__ge__` for user-defined objects
  (used by `sorted`/`list.sort`/`min`/`max`).
- Binary operators fall back to user-defined `__add__`/`__sub__`/`__or__`/`__and__` when builtin paths do not apply.
- Lambda expressions lower to function objects with closures, defaults, and varargs/kw-only args.
- Indexing honors user-defined `__getitem__`/`__setitem__` when builtin paths do not apply.
- CPython shim: minimal ASGI adapter for http/lifespan via `molt.asgi.asgi_adapter`.
- `molt_accel` client/decorator expose before/after hooks, metrics callbacks (including payload/response byte sizes), cancel-checks with auto-detection of request abort helpers, concurrent in-flight requests in the shared client, optional worker pooling via `MOLT_ACCEL_POOL_SIZE`, and raw-response pass-through; timeouts schedule a worker restart after in-flight requests drain; wire selection honors `MOLT_WORKER_WIRE`/`MOLT_WIRE`.
- `molt_accel.contracts` provides shared payload builders for demo endpoints (`list_items`, `compute`, `offload_table`), including JSON-body parsing for the offload table demo path.
- `molt_worker` supports sync/async runtimes (`MOLT_WORKER_RUNTIME` / `--runtime`), enforces cancellation/timeout checks in the fake DB path, compiled dispatch loops, pool waits, and Postgres queries; validates export manifests; reports queue/pool metrics per request (queue_us/handler_us/exec_us/decode_us plus ms rollups); fake DB decode cost can be simulated via `MOLT_FAKE_DB_DECODE_US_PER_ROW` and CPU work via `MOLT_FAKE_DB_CPU_ITERS`. Thread and queue tuning are available via `MOLT_WORKER_THREADS` and `MOLT_WORKER_MAX_QUEUE` (CLI overrides).
- `molt-db` provides a bounded pool, a feature-gated async pool primitive, a native-only SQLite connector (feature-gated in `molt-worker`), and an async Postgres connector (tokio-postgres + rustls) with per-connection statement caching.
- `molt_db_adapter` exposes a framework-agnostic DB IPC payload builder aligned with `docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md`; worker-side `db_query`/`db_exec` support SQLite (sync) and Postgres (async) with json/msgpack results (Arrow IPC for `db_query`), db-specific metrics, and structured decoding for Postgres arrays/ranges/intervals/multiranges in json/msgpack plus Arrow IPC struct/list encodings (including lower-bound metadata). WASM DB host intrinsics (`db_query`/`db_exec`) are defined with stream handles and `db.read`/`db.write` capability gating, and the Node/WASI host adapter is wired in `run_wasm.js`.
- WASM harness runs via `run_wasm.js` with shared memory/table and direct runtime imports, including async/channel benches on WASI.
- WASM parity tests cover strings, bytes/bytearray, memoryview, list/dict ops, control flow, generators, and async protocols.
- Instance `__getattr__`/`__getattribute__` fallback (AttributeError) plus `__setattr__`/`__delattr__` hooks for user-defined classes.
- Object-level `__getattribute__`/`__setattr__`/`__delattr__` builtins follow CPython raw attribute semantics.
- `__class__`/`__dict__` attribute access for instances, functions, modules, and classes (class `__dict__` returns a mutable dict).
- `**kwargs` expansion accepts dicts and mapping-like objects with `keys()` + `__getitem__`.
- `functools.partial`, `functools.reduce`, and `functools.lru_cache` accept `*args`/`**kwargs`, `functools.wraps`/`update_wrapper` honors assigned/updated, and `cmp_to_key`/`total_ordering` are available.
- `itertools` core iterators are available (`chain`, `islice`, `repeat`, `count`, `cycle`, `accumulate`, `pairwise`, `product`, `permutations`, `combinations`, `groupby`, `tee`).
- `heapq` includes `merge` plus max-heap helpers alongside runtime fast paths.
- `collections.deque` supports rotate/index/insert/remove; `Counter`/`defaultdict` are dict subclasses with arithmetic/default factories, `Counter` keys/values/items/total, repr/equality parity, and in-place arithmetic ops.
- C3 MRO + multiple inheritance for attribute lookup, `super()` resolution, and descriptor precedence for
  `__get__`/`__set__`/`__delete__`.
- Descriptor protocol supports callable non-function `__get__`/`__set__`/`__delete__` implementations (callable objects).
- Exceptions: BaseException root, non-string messages lowered through `str()`, StopIteration.value propagated across
  iter/next and `yield from`, `__traceback__` captured as traceback objects (`tb_frame`/`tb_lineno`/`tb_next`) with frame
  objects carrying `f_code`/`f_lineno` line markers backed by global code slots across the module graph, unhandled
  exceptions render traceback frames with file/line/function metadata, and `sys.exc_info()` reads the active exception
  context.
- Generator introspection: `gi_running`, `gi_frame` (with `f_lasti`), `gi_yieldfrom`, and `inspect.getgeneratorstate`.
- Recursion limits enforced via call dispatch guards with `sys.getrecursionlimit`/`sys.setrecursionlimit` wired to runtime limits.
- `molt_accel` is packaged as an optional dependency group (`[project.optional-dependencies].accel`) with a packaged default exports manifest; the decorator falls back to `molt-worker` in PATH when `MOLT_WORKER_CMD` is unset. A demo Django app/worker scaffold lives under `demo/`.
- `molt_worker` compiled-entry dispatch is wired for demo handlers (`list_items`/`compute`/`offload_table`/`health`) using codec_in/codec_out; other exported names still return a clear error until compiled handlers exist.
  (TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): compiled handler coverage beyond demo exports.)
- `asyncio.CancelledError` follows CPython inheritance (BaseException subclass), so cancellation bypasses `except Exception`.

## Limitations (Current)
- Classes/object model: no metaclasses or dynamic `type()` construction.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): derive `types.GenericAlias.__parameters__` from `TypeVar`/`ParamSpec`/`TypeVarTuple` once typing metadata lands.
- Attributes: fixed struct fields with dynamic instance-dict fallback; no
  user-defined `__slots__` beyond dataclass lowering; object-level
  class `__dict__` uses a mutable dict (mappingproxy pending).
  (TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:missing): mappingproxy view for class `__dict__`.)
- Class instantiation bypasses user-defined `__new__` for non-exception classes (allocates instances directly before `__init__`).
  (TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.)
- Strings: `str.isdigit` is missing, and `str.startswith`/`str.endswith` ignore start/end parameters.
  (TODO(type-coverage, owner:runtime, milestone:TC2, priority:P1, status:partial): implement `str.isdigit` and start/end support for string prefix/suffix checks.)
- Dataclasses: compile-time lowering for frozen/eq/repr/slots with defaults and
  `default_factory`; missing `kw_only` and `order`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement kw-only fields
  and ordering parity.)
- Call binding: allowlisted stdlib modules now permit dynamic calls (keyword/variadic via `CALL_BIND`);
  direct-call fast paths still require allowlisted functions and positional-only calls. Non-allowlisted imports
  remain blocked unless the bridge policy is enabled.
- Builtin arity checks are still enforced at compile time for some constructors/methods (e.g., `bool`, `str`, `list`, `range`, `join`).
  (TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): lower builtin arity checks to runtime `TypeError` instead of compile-time rejection.)
- List membership/count/index snapshot list elements to guard against mutation during `__eq__`/`__contains__`, which allocates on hot paths.
  (TODO(perf, owner:runtime, milestone:TC1, priority:P2, status:planned): avoid list_snapshot allocations in membership/count/index by using a list mutation version or iterator guard.)
- `int()` keyword arguments (`x`, `base`) are not accepted.
  (TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:partial): support keyword arguments for `int()` with CPython parity.)
- `range()` rejects some arguments that fail lowering (e.g., oversized ints) instead of deferring to runtime errors.
  (TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:partial): lower unsupported `range()` arguments to runtime parity errors.)
- f-string conversion flags (`!r`, `!s`, `!a`) are not supported in format placeholders.
  (TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): implement f-string conversion flags and parity tests.)
- Async generators (`async def` with `yield`) are not supported.
  (TODO(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:missing): implement async generator lowering and runtime parity.)
- `contextlib.contextmanager` decorators are still unsupported.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): implement `contextmanager` lowering.)
- `str()` decoding with `encoding`/`errors` arguments is not supported; only 0/1-arg `str(obj)` conversion is available.
  (TODO(stdlib-compat, owner:runtime, milestone:TC2, priority:P2, status:missing): implement `str(bytes, encoding, errors)` parity.)
- `print(file=None)` falls back to the host stdout when the `sys` module is not initialized, rather than always using `sys.stdout`.
  (TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): ensure `sys.stdout` bootstrap for print.)
- WASM `str_from_obj` does not invoke `__str__` for non-primitive objects, so `print()`/`str()` may show placeholders for custom types.
  (TODO(wasm-parity, owner:runtime, milestone:TC1, priority:P2, status:partial): call `__str__` for non-primitive objects in wasm host bindings.)
- WASM `string_format`/`format()` only handle empty format specs; non-empty specs raise `TypeError`.
  (TODO(wasm-parity, owner:runtime, milestone:TC1, priority:P2, status:partial): implement full format spec parsing + rendering in the wasm host.)
- File I/O parity is partial: `open()` supports the full signature (mode/buffering/encoding/errors/newline/closefd/opener), fd-based `open`, and file objects now expose read/readinto/write/writelines/seek/tell/fileno/readline(s)/truncate/iteration/flush/close + core attrs. Remaining gaps include broader codec support + full error handlers (utf-8/ascii/latin-1 only), partial text-mode seek/tell cookie semantics, and Windows fileno/isatty accuracy.
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:partial): finish file/open parity per ROADMAP checklist + tests, with native/wasm lockstep.)
  (TODO(wasm-parity, owner:runtime, milestone:SL1, priority:P1, status:partial): extend wasm host hooks for remaining file methods (readinto1) and parity coverage.)
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): align file handle type names in error/AttributeError messages with CPython _io.* wrappers.)
- WASM `os.getpid()` currently returns a 0 placeholder.
  (TODO(wasm-parity, owner:runtime, milestone:SL2, priority:P2, status:partial): add a host-backed getpid or document the placeholder semantics.)
- Generator introspection: `gi_code` is still stubbed and frame objects only expose `f_lasti`.
  (TODO(introspection, owner:runtime, milestone:TC3, priority:P2, status:missing): implement `gi_code` + full frame objects.)
- Comprehensions: list/set/dict comprehensions, generator expressions, and async comprehensions (async for/await) are supported.
- Control flow: `while`-`else` is still unsupported.
  (TODO(syntax, owner:frontend, milestone:TC2, priority:P2, status:missing): `while`-`else` lowering and tests.)
- Differential tests: core-language basic now includes pattern matching, async generator finalization, and `while`-`else` probes; failures are expected until the features are implemented.
- Augmented assignment: slice targets (`seq[a:b] += ...`) are not supported yet.
  (TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:missing): augassign slice targets.)
- Exceptions: `try/except/else/finally` + `raise`/reraise; `__traceback__` now returns traceback objects
  (`tb_frame`/`tb_lineno`/`tb_next`) with frame objects carrying `f_code`/`f_lineno` (see `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md`).
  Builtin exception hierarchy now matches CPython (BaseExceptionGroup, OSError/Warning trees, ExceptionGroup MRO).
  (TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame objects to full CPython parity fields.)
  (TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): exception `__init__` + subclass attribute parity (UnicodeError fields, ExceptionGroup tree).)
- Code objects: `__code__` exposes `co_filename`/`co_name`/`co_firstlineno`; `co_varnames`, arg counts, and
  `co_linetable` remain minimal.
  (TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): fill out code object fields for parity.)
- Runtime lifecycle: `molt_runtime_init()`/`molt_runtime_shutdown()` manage a `RuntimeState` that owns caches, pools, and async registries; TLS guard drains per-thread caches on thread exit, scheduler/sleep workers join on shutdown, and freed TYPE_ID_OBJECT headers return to the object pool with fallback deallocation for non-pooled types.
- Tooling: `molt clean --cargo-target` removes Cargo `target/` build artifacts when requested.
- Process-based concurrency is unsupported: `multiprocessing`, `subprocess`, and `concurrent.futures` await capability-gated process model integration (start-method policy, IPC primitives, worker lifecycle).
  (TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:planned): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures`.)
- `sys.argv` is initialized from compiled argv (native + wasm harness); decoding currently uses lossy UTF-8/UTF-16 until surrogateescape/fs-encoding parity lands.
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): decode argv via filesystem encoding + surrogateescape once Molt strings can represent surrogate escapes.)
- `sys.modules` mirrors the runtime module cache for compiled code; `sys._getframe` is still unavailable in compiled runtimes.
  (TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): implement `sys._getframe` for compiled runtimes.)
- `globals()`/`locals()`/`vars()`/`dir()` are lowered as direct calls; `locals()` snapshots the current locals,
  and first-class builtin objects (especially no-arg `dir`/`vars` passed around) are still limited.
  (TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.)
- Runtime safety: NaN-boxed pointer conversions resolve through a pointer registry to avoid int->ptr casts in Rust; host pointer args now use raw pointer ABI in native + wasm; strict-provenance Miri is green.
- Hashing: SipHash13 + `PYTHONHASHSEED` parity (randomized by default; deterministic when seed=0); see `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md`.
- GC: reference counting only; cycle collector pending (see `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md`).
  (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): implement cycle collector.)
- Imports: static module graph only; no dynamic import hooks or full package
  resolution.
  (TODO(import-system, owner:stdlib, milestone:TC3, priority:P2, status:missing): dynamic import hooks + package resolution.)
- Entry modules are aliased as `__main__` in the module cache; importing the entry module shares the same module object.
  (TODO(import-system, owner:frontend, milestone:TC3, priority:P2, status:partial): split `__main__` and importable entry modules when the entry module is imported elsewhere.)
- Module metadata: compiled modules set `__file__`/`__package__` and package `__path__`.
  (TODO(import-system, owner:frontend, milestone:TC3, priority:P1, status:partial): populate module `__spec__` for compiled modules.)
- Imports: module-level `from x import *` honors `__all__` (with strict name checks) and otherwise skips underscore-prefixed names.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root builds (namespace packages + PYTHONPATH roots supported; remaining: package discovery hardening, `__init__` edge cases, deterministic dependency graph caching).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): relative imports (explicit and implicit) with deterministic package resolution.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:planned): method-binding safety pass (guard/deopt on method lookup + cache invalidation rules for call binding).
- Asyncio: shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `Event`, `wait`, `wait_for`, `shield`, basic `gather`,
  stream helpers (`open_connection`/`start_server`), and `add_reader`/`add_writer`; advanced loop APIs, task groups, and full
  transport/protocol adapters remain pending. Event-loop semantics target a single-threaded, deterministic scheduler;
  true parallelism is explicit via executors or isolated runtimes.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task APIs + task groups + I/O adapters + executor semantics.)
  (TODO(async-runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors and explicit message passing; shared-memory parallelism only via opt-in safe types.)
- C API: no `libmolt` C-extension surface yet; `docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md` is target-only
  (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define and implement the initial C API shim).
- Matmul (`@`): supported only for `molt_buffer`/`buffer2d`; other types raise
  `TypeError` (TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): consider
  `__matmul__`/`__rmatmul__` fallback for custom types).
- Roadmap focus: async runtime core (Task/Future scheduler, contextvars, cancellation injection), capability-gated async I/O,
  DB semantics expansion, WASM DB parity, framework adapters, and production hardening (see ROADMAP).
- Numeric tower: complex/decimal pending; `int` still missing full method surface
  (e.g., `bit_length`, `to_bytes`, `from_bytes`).
  (TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): complex/decimal + `int` method parity.)
  (TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:missing): complex literals + lowering support.)
- errno: basic constants + errorcode mapping to support OSError mapping; full table pending.
  (TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P1, status:partial): expand errno constants + errorcode mapping to full CPython table.)
- Format protocol: WASM `n` formatting uses host locale separators via
  `MOLT_WASM_LOCALE_*` (set by `run_wasm.js` when available).
- memoryview: multi-dimensional slicing/sub-views remain pending; slice assignments
  are restricted to ndim = 1.
  (TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): multi-dimensional slicing/sub-views.)
- WASM parity: codec parity tests cover baseline + mixed schema payloads and invalid payload errors via harness
  overrides; advanced schema coverage (binary/float/large ints/tags) is still expanding.
  (TODO(tests, owner:runtime, milestone:SL1, priority:P1, status:partial): expand codec parity coverage for
  binary/floats/large ints/tagged values/deeper container shapes.)
- WASM parity: socket/select/selector I/O is native-only; wasm host networking and readiness integration are missing.
  (TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:missing): implement wasm socket + io_poller host bindings with capability enforcement.)
- Structured codecs: MsgPack is the production default while JSON remains for compatibility/debug.
- Cancellation: cooperative checks plus automatic cancellation injection on await
  boundaries; async I/O cancellation propagation still pending.
  (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): async I/O cancellation propagation.)
- `db_query` Arrow IPC uses best-effort type inference; mixed-type columns error without a declared schema; wasm-side client shims remain pending (Node/WASI host adapter is implemented in `run_wasm.js`).
  (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:missing): wasm client shims for DB intrinsics.)
- collections: `deque` remains list-backed (left ops are O(n)); no runtime deque type yet.
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:missing): runtime deque type.)
- itertools: `product`/`permutations`/`combinations` are eager (materialize inputs/outputs), so infinite iterables are not supported
  (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): make these iterators lazy and streaming).

## Async + Concurrency Notes
- Core async scheduling lives in `molt-runtime` (custom poll/sleep loop); tokio is used only in service crates (`molt-worker`, `molt-db`) for host I/O.
- Awaitables that return pending now resume at a labeled state to avoid
  re-running pre-await side effects.
- Pending await resume targets are encoded in the state slot (negative, bitwise
  NOT of the resume op index) and decoded before dispatch.
- Channel send/recv yield on pending and resume at labeled states.
- `asyncio.sleep` honors delay/result and avoids busy-spin via scheduler sleep
  registration (sleep queue + block_on integration); `asyncio.gather` and
  `asyncio.Event` are supported for core patterns; `asyncio.wait_for` now
  supports timeout + cancellation propagation across task boundaries.
- TODO(compiler, owner:compiler, milestone:TC2, priority:P0, status:partial): fix async lowering/back-end verifier for `asyncio.gather` poll paths (dominance issues) and wasm stack-balance errors; async protocol parity tests currently fail.
- TODO(perf, owner:compiler, milestone:TC2, priority:P1, status:planned): tighten async spill/restore to a CFG-based liveness pass to reduce closure traffic and shrink state_label reload sets.
- `asyncio.Event` prunes cancelled waiters during task teardown and cooperates
  with cancellation propagation.
- Raising non-exception objects raises `TypeError` with BaseException checks (CPython parity); subclass-specific attributes remain pending.
- Cancellation tokens are available with request-scoped defaults and task-scoped
  overrides; awaits inject `CancelledError`, and cooperative checks via
  `molt.cancelled()` remain available.
- Await lowering now consults `__await__` when present to bridge stdlib `Task`/`Future` shims.
- WASM runs a single-threaded scheduler loop (no background workers); pending
  sleeps are handled by blocking registration in the same task loop.
  (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): wasm scheduler background workers.)
- Websocket connect is blocked when no host hook is registered.
  (TODO(runtime, owner:runtime, milestone:RT2, priority:P2, status:missing): add a host-level websocket connect hook and enforce production socket capabilities.)

## Thread Safety + GIL Notes
- Runtime mutation is serialized by a GIL-like lock; only one host thread may
  execute Python/runtime code at a time within the process.
- Runtime state and object headers are not thread-safe; `Value` and heap objects
  are not `Send`/`Sync` unless explicitly documented otherwise.
- Cross-thread sharing of live Python objects is unsupported; serialize or
  freeze data before crossing threads.
- Handle table and pointer registry may use internal locks; lock ordering rules
  are defined in `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`.
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define
  the per-runtime GIL strategy, runtime instance ownership model, and allowed
  cross-thread object sharing rules.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): implement
  sharded/lock-free handle resolution and measure lock-sensitive benchmark deltas
  (attr access, container ops).
- Runtime mutation entrypoints require a `PyToken`; only `molt_handle_resolve` is
  GIL-exempt by contract (see `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`).

## Performance Notes
- `print` builds a single intermediate string before writing.
  (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): stream print writes to avoid large intermediate allocations.)
- `dict.fromkeys` does not pre-size using iterable length hints.
  (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): pre-size `dict.fromkeys` to reduce rehashing.)

## Stdlib Coverage
- Partial shims: `warnings`, `traceback`, `types`, `inspect`, `fnmatch` (`*`/`?`
  + bracket class/range matching; literal `[]`/`[[]`/`[]]` escapes (no backslash
  quoting)), `copy`, `pprint`, `string`, `typing`, `sys`, `os`, `pathlib`,
  `tempfile`, `gc`, `random` (`seed`, `randrange`, `shuffle`), `time` (`monotonic`, `perf_counter`, `sleep`, `time`/`time_ns` gated by `time.wall`), `json` (loads/dumps with parse hooks, indent, separators, allow_nan), `pickle` (protocol 0 only),
  `socket` (runtime-backed, capability-gated; advanced options + wasm parity pending), `select` (selectors-backed for sockets only),
  `selectors` (io_wait-backed readiness), `asyncio`, `contextvars`, `contextlib`, `threading`,
  `functools`, `itertools`, `operator`, `bisect`, `heapq`, `collections`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): complete socket/select/selectors parity (poll/epoll/select objects, fd inheritance, error mapping, cancellation) and align with asyncio adapters.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs exist for regrtest (support: captured_output/captured_stdout/captured_stderr, warnings_helper.check_warnings, cpython_only, requires, swap_attr/swap_item, import_helper.import_module/import_fresh_module, os_helper.temp_dir/unlink); doctest is blocked on eval/exec/compile gating and full unittest parity is pending.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): os.environ parity (mapping methods + backend).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (Encoder/Decoder classes, JSONDecodeError details, runtime fast-path parser).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module exposes only minimal toggles/collect; wire to runtime cycle collector and implement full API.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `random` shim exposes seed/randrange/shuffle; expand to CPython-compatible Random API + algorithm.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `time` module surface (timezone/tzname/struct_time/get_clock_info/process_time) + deterministic clock policy.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/backslashreplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `pickle` protocol 1+ and broader type coverage (bytes/bytearray, memo cycles).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` shim only covers constants + `isfinite`/`isnan`/`isinf`; intrinsics pending.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand `types` shims (TracebackType, FrameType, FunctionType, MethodType, etc).
- Import-only stubs: `collections.abc`, `importlib`, `importlib.util`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement core importlib/collections.abc surfaces.)
- Planned import-only stubs: `importlib.metadata`, `html`, `html.parser`, `http.cookies`, `http.client`, `http.server`,
  `ipaddress`, `mimetypes`, `socketserver`, `wsgiref`, `xml`, `email.policy`, `email.message`, `email.parser`,
  `email.utils`, `email.header`, `urllib.parse`, `urllib.request`, `urllib.error`, `urllib.robotparser`,
  `logging.config`, `logging.handlers`, `cgi`, `zlib`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + coverage smoke tests.)
- See `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` for the full matrix.

## Django Demo Blockers (Current)
- Remaining stdlib gaps for Django internals: `operator` intrinsics, richer `collections` perf (runtime deque), and `re`/`datetime`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): operator intrinsics + runtime deque + `re`/`datetime` parity.)
- Async loop/task APIs + `contextvars` cover Task/Future/gather/Event/`wait_for`;
  task groups/wait/shield plus async I/O cancellation propagation and long-running
  workload hardening are pending.
  (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): task groups/wait/shield + I/O cancellation + hardening.)
- Top priority: finish wasm parity for DB connectors before full DB adapter expansion (see `docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md`).
  (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB connector parity.)
- Capability-gated I/O/runtime modules (`os`, `sys`, `pathlib`, `logging`, `time`, `selectors`) need deterministic parity.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O parity.)
- HTTP/ASGI runtime surface is not implemented (shim adapter exists); DB driver/pool integration is partial (`db_query` only; wasm parity pending).
  (TODO(http-runtime, owner:runtime, milestone:SL3, priority:P1, status:missing): HTTP/ASGI runtime + DB driver parity.)
- Descriptor hooks still lack metaclass behaviors, limiting idiomatic Django patterns.
  (TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): metaclass behavior for descriptor hooks.)

## Tooling + Verification
- CI enforces lint, type checks, Rust fmt/clippy, differential tests, and perf
  smoke gates.
- Trusted mode is available via `MOLT_TRUSTED=1` (disables capability checks for
  trusted native deployments).
- CLI commands now cover `run`, `test`, `diff`, `bench`, `profile`, `lint`,
  `doctor`, `package`, `publish`, `verify`, and `config` as initial wrappers
  (local-only package publish; manifest/checksum verification only).
- `molt build` enforces lockfiles in deterministic mode, accepts capability
  manifests, and can target non-host triples via Cranelift + zig linking.
- `molt vendor` materializes Tier A sources into `vendor/` with a manifest.
- Use `tools/dev.py lint` and `tools/dev.py test` for local validation.
- WIT interface contract lives at `wit/molt-runtime.wit` (WASM runtime intrinsics).
- Experimental single-module wasm link attempt via `tools/wasm_link.py` (requires `wasm-ld`); run via `MOLT_WASM_LINKED=1`.

## Known Gaps
- uv-managed Python 3.14 hangs on arm64; system Python 3.14 used as workaround.
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): resolve uv-managed 3.14 arm64 hang.)
- Browser host for WASM is still pending; current harness targets WASI via
  `run_wasm.js` and uses a single-threaded scheduler.
  (TODO(wasm-host, owner:runtime, milestone:RT3, priority:P2, status:missing): browser-hosted WASM harness.)
- Cross-target native builds (non-host triples/architectures) are not yet wired into
  the CLI/build pipeline.
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): wire cross-target builds into CLI.)
- SQLite/Postgres connectors remain native-only; wasm DB host adapters and client shims are still pending.
  (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:missing): wasm DB adapters + client shims.)
- True single-module WASM link (no JS boundary) is still pending; current direct-link harness still uses a JS stub for `molt_call_indirect1`.
  (TODO(wasm-link, owner:runtime, milestone:RT3, priority:P2, status:partial): single-module link without JS stub.)
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): OPT-0003 phase 1 landed (sharded pointer registry); benchmark and evaluate lock-free alternatives next (see `OPTIMIZATIONS_PLAN.md`).
- Single-module wasm linking remains experimental; wasm-ld now links relocatable output when `MOLT_WASM_LINK=1`, but broader coverage + table/element relocation validation and removal of the JS `molt_call_indirect1` stub are still pending.
  (TODO(wasm-link, owner:runtime, milestone:RT3, priority:P2, status:planned): relocation validation + JS stub removal.)
