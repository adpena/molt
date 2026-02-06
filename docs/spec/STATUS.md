# STATUS (Canonical)

Last updated: 2026-02-06

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
- `molt build` supports sysroot overrides via `--sysroot` or `MOLT_SYSROOT` / `MOLT_CROSS_SYSROOT` for native linking.
- Differential testing vs CPython 3.12+ for supported constructs (PEP 649 annotation parity validated against CPython 3.14).
- PEP 649 lazy annotations: compiler emits `__annotate__` for module/class/function, `__annotations__` computed lazily and cached (formats 1/2: VALUE/STRING).
- PEP 585 generic aliases for builtin containers (`list`/`dict`/`tuple`/`set`/`frozenset`/`type`) with `__origin__`/`__args__`.
- PEP 584 dict union (`|`, `|=`) with mapping RHS parity.
- PEP 604 union types (`X | Y`) with `__args__`/`__origin__` and `types.UnionType` alias (`types.Union` on 3.14).
- Molt packages for Rust-backed deps using MsgPack/CBOR and Arrow IPC.
- `molt package` emits CycloneDX SBOM sidecars (`*.sbom.json`) and signature metadata (`*.sig.json`), embeds `sbom.json`/`signature.json` inside `.moltpkg`, can sign artifacts via cosign/codesign (signature sidecars `*.sig` when attached or produced by cosign), and `molt verify`/`molt publish` can enforce signature verification with trust policies.
- Sets: literals + constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`) over set/frozenset/dict view RHS; `frozenset` constructor + algebra; set/frozenset method attributes for union/intersection/difference/symmetric_difference, update variants, copy/clear, and isdisjoint/issubset/issuperset.
- Numeric builtins: `int()`/`abs()`/`divmod()`/`round()`/`math.trunc()` with `__int__`/`__index__`/`__round__`/`__trunc__` hooks and base parsing for string/bytes.
- `int()` accepts keyword arguments (`x`, `base`), and int subclasses preserve integer payloads for `__int__`/`__index__` (used by `IntEnum`/`IntFlag`).
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): advance native `re` engine to full syntax/flags/groups; native engine supports literals, `.`, char classes/ranges (`\\d`/`\\w`/`\\s`), groups/alternation, greedy + non-greedy quantifiers, and `IGNORECASE`/`MULTILINE`/`DOTALL` flags; advanced features/flags raise `NotImplementedError` (no host fallback).
- Builtin containers expose `__iter__`/`__len__`/`__contains__`/`__reversed__` (where defined) for list/dict/str/bytes/bytearray, including class-level access to builtin methods. Item dunder access via getattr is available for dict/list/bytearray/memoryview (`__getitem__`/`__setitem__`/`__delitem__`).
- Implemented: dict subclass storage is separate from instance `__dict__`, avoiding attribute leakage and matching CPython mapping/attribute separation.
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
- `bytes`/`bytearray` constructors accept int counts, iterable-of-ints, and str+encoding (`utf-8`/`utf-8-sig`/`latin-1`/`ascii`/`cp1252`/`cp437`/`cp850`/`cp860`/`cp862`/`cp863`/`cp865`/`cp866`/`cp874`/`cp1250`/`cp1251`/`cp1253`/`cp1254`/`cp1255`/`cp1256`/`cp1257`/`koi8-r`/`koi8-u`/`iso8859-2`/`iso8859-3`/`iso8859-4`/`iso8859-5`/`iso8859-6`/`iso8859-7`/`iso8859-8`/`iso8859-10`/`iso8859-15`/`mac-roman`/`utf-16`/`utf-32`) with basic error handlers (`strict`/`ignore`/`replace`) and parity errors for negative counts/range checks.
- `bytes`/`bytearray` methods `find`/`count` (bytes-like/int needles)/`split`/`rsplit`/`replace`/`startswith`/`endswith`/`strip`/`lstrip`/`rstrip` (including start/end slices and tuple prefixes) and indexing return int values with CPython-style bounds errors.
- `dict`/`dict.update` raise CPython parity errors for non-iterable elements and invalid pair lengths.
- `len()` falls back to `__len__` with CPython parity errors for negative, non-int, and overflow results.
- Dict/set key hashability parity for common unhashable types (list/dict/set/bytearray/memoryview).
- `errno` constants + `errorcode` mapping are generated from the host CPython errno table at build time for native targets (WASM keeps the minimal errno set).
- Importable `builtins` module binds supported builtins (see stdlib matrix).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): migrate all Python stdlib modules to Rust intrinsics-only implementations (Python files may only be thin intrinsic-forwarding wrappers); compiled binaries must reject Python-only stdlib modules. See `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`.
- Intrinsics audit is enforced by `tools/check_stdlib_intrinsics.py` (generated doc + lint), including `intrinsic-backed` / `intrinsic-partial` / `probe-only` / `python-only` status tracking.
- Core compiled-surface gate is enforced by `tools/check_core_lane_lowering.py`: modules imported (transitively) by `tests/differential/core/TESTS.txt` must be `intrinsic-backed` only.
- Execution program for complete Rust lowering is tracked in `docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md` (core blockers first, then socket -> threading -> asyncio, then full stdlib sweep).
- Implemented: `__future__` and `keyword` module data/queries are now sourced from Rust intrinsics (`molt_future_features`, `molt_keyword_lists`, `molt_keyword_iskeyword`, `molt_keyword_issoftkeyword`), removing probe-only status.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): remove `typing` fallback ABC scaffolding and lower protocol/ABC bootstrap helpers into Rust intrinsics-only paths.
- Implemented: `builtins` bootstrap no longer probes host `builtins`; descriptor constructors are intrinsic-backed (`molt_classmethod_new`, `molt_staticmethod_new`, `molt_property_new`) and fail fast when intrinsics are missing.
- Implemented: `pathlib` now routes core path algebra and filesystem operations through Rust intrinsics (`molt_path_join`, `molt_path_isabs`, `molt_path_dirname`, `molt_path_splitext`, `molt_path_abspath`, `molt_path_parts`, `molt_path_parents`, `molt_path_relative_to`, `molt_path_with_name`, `molt_path_with_suffix`, `molt_path_expanduser`, `molt_path_match`, `molt_path_glob`, `molt_path_exists`, `molt_path_listdir`, `molt_path_mkdir`, `molt_path_unlink`, `molt_path_rmdir`, `molt_file_open_ex`); targeted differential lane (`os`/`time`/`traceback`/`pathlib`/`threading`) ran `24/24` green with RSS caps enabled.
- `enumerate` builtin returns an iterator over `(index, value)` with optional `start`.
- `iter(callable, sentinel)`, `map`, `filter`, `zip(strict=...)`, and `reversed` return lazy iterator objects with CPython-style stop conditions.
- `iter(obj)` enforces that `__iter__` returns an iterator, raising `TypeError` with CPython-style messages for non-iterators.
- Builtin function objects for allowlisted builtins (`any`, `all`, `abs`, `ascii`, `bin`, `oct`, `hex`, `chr`, `ord`, `divmod`, `hash`, `callable`, `repr`, `format`, `getattr`, `hasattr`, `round`, `iter`, `next`, `anext`, `print`, `super`, `sum`, `min`, `max`, `sorted`, `map`, `filter`, `zip`, `reversed`).
- `sorted()` enforces keyword-only `key`/`reverse` arguments (CPython parity).
- Builtin reductions: `sum`, `min`, `max` with key/default support across core ordering types.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:partial): dynamic execution builtins: `compile` performs global/nonlocal checks and returns a stub code object; `eval`/`exec` and full compile (sandbox + codegen) remain missing; regrtest `test_future_stmt` depends on full `compile`.
- Differential parity probes for dynamic execution (`eval`/`exec`) are tracked in `tests/differential/planned/exec_*` and `tests/differential/planned/eval_*` and are **expected to fail** until sandboxed dynamic execution lands.
- `print` supports keyword arguments (`sep`, `end`, `file`, `flush`) with CPython-style type errors; `file=None` uses `sys.stdout`.
- Lexicographic ordering for `str`/`bytes`/`bytearray`/`list`/`tuple` (cross-type ordering raises `TypeError`).
- Ordering comparisons fall back to `__lt__`/`__le__`/`__gt__`/`__ge__` for user-defined objects
  (used by `sorted`/`list.sort`/`min`/`max`).
- Binary operators fall back to user-defined `__add__`/`__sub__`/`__or__`/`__and__` when builtin paths do not apply.
- Lambda expressions lower to function objects with closures, defaults, and varargs/kw-only args.
- Indexing honors user-defined `__getitem__`/`__setitem__` when builtin paths do not apply.
- CPython shim: minimal ASGI adapter for http/lifespan via `molt.asgi.asgi_adapter`.
- `molt_accel` client/decorator expose before/after hooks, metrics callbacks (including payload/response byte sizes), cancel-checks with auto-detection of request abort helpers, concurrent in-flight requests in the shared client, optional worker pooling via `MOLT_ACCEL_POOL_SIZE`, and raw-response pass-through; timeouts schedule a worker restart after in-flight requests drain; wire selection honors `MOLT_WORKER_WIRE`/`MOLT_WIRE`.
- `molt_accel.contracts` provides shared payload builders for demo endpoints (`list_items`, `compute`, `offload_table`), including JSON-body parsing for the offload table demo path.
- `molt_worker` supports sync/async runtimes (`MOLT_WORKER_RUNTIME` / `--runtime`), enforces cancellation/timeout checks in the fake DB path, compiled dispatch loops, pool waits, Postgres queries, and SQLite via interrupt handles; validates export manifests; reports queue/pool metrics per request (queue_us/handler_us/exec_us/decode_us plus ms rollups); fake DB decode cost can be simulated via `MOLT_FAKE_DB_DECODE_US_PER_ROW` and CPU work via `MOLT_FAKE_DB_CPU_ITERS`. Thread and queue tuning are available via `MOLT_WORKER_THREADS` and `MOLT_WORKER_MAX_QUEUE` (CLI overrides).
- `molt-db` provides a bounded pool, a feature-gated async pool primitive, a native-only SQLite connector (feature-gated in `molt-worker`), and an async Postgres connector (tokio-postgres + rustls) with per-connection statement caching.
- `molt_db_adapter` exposes a framework-agnostic DB IPC payload builder aligned with `docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md`; worker-side `db_query`/`db_exec` support SQLite (sync) and Postgres (async) with json/msgpack results (Arrow IPC for `db_query`), db-specific metrics, and structured decoding for Postgres arrays/ranges/intervals/multiranges in json/msgpack plus Arrow IPC struct/list encodings (including lower-bound metadata). WASM DB host intrinsics (`db_query`/`db_exec`) are defined with stream handles and `db.read`/`db.write` capability gating, and the Node/WASI host adapter is wired in `run_wasm.js`.
- WASM harness runs via `run_wasm.js` using linked outputs; direct-link is disabled due to shared-memory layout overlap. Async/channel benches still run on WASI.
- Wasmtime host runner (`molt-wasm-host`) uses linked outputs (direct-link disabled for correctness), supports shared memory/table wiring, non-blocking DB host delivery via `molt_db_host_poll` (stream semantics + cancellation checks), and can be used via `tools/bench_wasm.py --runner wasmtime` for perf comparisons.
- WASM parity tests cover strings, bytes/bytearray, memoryview, list/dict ops, control flow, generators, and async protocols.
- Instance `__getattr__`/`__getattribute__` fallback (AttributeError) plus `__setattr__`/`__delattr__` hooks for user-defined classes.
- Object-level `__getattribute__`/`__setattr__`/`__delattr__` builtins follow CPython raw attribute semantics.
- `__class__`/`__dict__` attribute access for instances, functions, modules, and classes (class `__dict__` returns a mutable dict).
- `**kwargs` expansion accepts dicts and mapping-like objects with `keys()` + `__getitem__`.
- `functools.partial`, `functools.reduce`, and `functools.lru_cache` accept `*args`/`**kwargs`, `functools.wraps`/`update_wrapper` honors assigned/updated, and `cmp_to_key`/`total_ordering` are available.
- `itertools` core iterators are available (`chain`, `islice`, `repeat`, `count`, `cycle`, `accumulate`, `pairwise`, `product`, `permutations`, `combinations`, `groupby`, `tee`).
- `heapq` includes `merge` plus max-heap helpers alongside runtime fast paths.
- `collections.deque` supports rotate/index/insert/remove; `Counter`/`defaultdict` are dict subclasses with arithmetic/default factories, `Counter` keys/values/items/total, repr/equality parity, and in-place arithmetic ops.
- Stdlib `linecache` supports `getline`/`getlines`/`checkcache`/`lazycache` with `fs.read` gating and loader-backed cache entries.
- Stdlib `pkgutil` supports filesystem `iter_modules`/`walk_packages` with `fs.read` gating.
- Stdlib `compileall` supports filesystem `compile_file`/`compile_dir`/`compile_path` with `fs.read` gating (no pyc emission).
- Stdlib `py_compile` supports `compile` with `fs.read`/`fs.write` gating (writes empty placeholder .pyc only).
- Stdlib `enum` provides minimal `Enum`/`IntEnum`/`Flag`/`IntFlag` support with `auto`, name/value accessors, and member maps.
- Stdlib `traceback` supports `format_exc`/`format_tb`/`format_list`/`format_stack`/`print_exception`/`print_list`/`print_stack`, `extract_tb`/`extract_stack`, `StackSummary` extraction, and basic `__cause__`/`__context__` chain formatting; full parity pending.
- Stdlib `abc` provides minimal `ABCMeta`/`ABC` and `abstractmethod` with instantiation guards.
- Stdlib `reprlib` provides `Repr`, `repr`, and `recursive_repr` parity.
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
- Core-lane strict lowering gate is green and enforced (`tools/check_core_lane_lowering.py`), and core-lane differential currently passes.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P0, status:partial): complete concurrency substrate lowering in strict order (`socket`/`select`/`selectors` -> `threading` -> `asyncio`) with intrinsic-only compiled semantics in native + wasm.
- Classes/object model: no metaclasses or dynamic `type()` construction.
- Implemented: `types.GenericAlias.__parameters__` derives `TypeVar`/`ParamSpec`/`TypeVarTuple` from `__args__`.
- Implemented: PEP 695 core-lane lowering uses Rust intrinsics for type parameter creation and GenericAlias construction/call dispatch (`molt_typing_type_param`, `molt_generic_alias_new`) for `typing`/frontend paths.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): finish PEP 695 type params (defaults + alias metadata/TypeAliasType; ParamSpec/TypeVarTuple + bounds/constraints now implemented).
- Attributes: fixed struct fields with dynamic instance-dict fallback; no
  user-defined `__slots__` beyond dataclass lowering; object-level
  class `__dict__` returns a mappingproxy view.
- Class instantiation bypasses user-defined `__new__` for non-exception classes (allocates instances directly before `__init__`).
  (TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.)
- Strings: `str.isdigit` now follows Unicode digit properties (ASCII + superscripts + non-ASCII digit sets).
- Dataclasses: compile-time lowering covers init/repr/eq/order/unsafe_hash/frozen/slots/match_args/kw_only,
  field flags, InitVar/ClassVar/KW_ONLY, __match_args__, and stdlib helpers.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): implement make_dataclass
  once dynamic class construction is allowed by the runtime contract.)
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance
  from non-dataclass bases without breaking layout guarantees.)
- Call binding: allowlisted stdlib modules now permit dynamic calls (keyword/variadic via `CALL_BIND`);
  direct-call fast paths still require allowlisted functions and positional-only calls. Non-allowlisted imports
  remain blocked unless the bridge policy is enabled.
- Builtin arity checks are still enforced at compile time for some constructors/methods (e.g., `bool`, `str`, `list`, `range`, `join`).
  (TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): lower builtin arity checks to runtime `TypeError` instead of compile-time rejection.)
- List membership/count/index snapshot list elements to guard against mutation during `__eq__`/`__contains__`, which allocates on hot paths.
  (TODO(perf, owner:runtime, milestone:TC1, priority:P2, status:planned): avoid list_snapshot allocations in membership/count/index by using a list mutation version or iterator guard.)
- `range()` lowering defers to runtime for non-int-like arguments and raises on step==0 before loop execution.
- Implemented: f-string conversion flags (`!r`, `!s`, `!a`) are supported in format placeholders, including nested format specs and debug expressions.
- Async generators (`async def` with `yield`) are not supported.
  (TODO(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:missing): implement async generator lowering and runtime parity.)
- `contextlib` parity is partial: `contextmanager`/`ContextDecorator` + `ExitStack`/`AsyncExitStack`, `suppress`, and `redirect_stdout`/`redirect_stderr` are supported; gaps remain for `aclosing`/`AbstractContextManager` and full parity.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): finish contextlib parity.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): finish abc registry + cache invalidation parity.
- Implemented: iterator/view helper types now map to concrete builtin classes so `collections.abc` imports and registrations work without fallback/guards.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): pkgutil loader/zipimport/iter_importers parity (filesystem-only discovery + store/deflate+zip64 zipimport today).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): compileall/py_compile parity (pyc output, invalidation modes, optimize levels).
- `str()` decoding with `encoding`/`errors` arguments is supported for bytes-like inputs (bytes/bytearray/memoryview), with the same codec/error-handler coverage as `bytes.decode` (utf-8/utf-8-sig/ascii/latin-1/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/utf-16/utf-32; strict/ignore/replace/backslashreplace/surrogateescape/surrogatepass).
- File I/O parity is partial: `open()` supports the full signature (mode/buffering/encoding/errors/newline/closefd/opener), fd-based `open`, and file objects now expose read/read1/readall/readinto/readinto1/write/writelines/seek/tell/fileno/readline(s)/truncate/iteration/flush/close + core attrs (name/mode/encoding/errors/newline/newlines/line_buffering/write_through, plus `closefd` on raw file handles and `buffer` on text wrappers). Remaining gaps include broader codec support (utf-8/utf-8-sig/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/ascii/latin-1/utf-16/utf-32 only; decode: strict/ignore/replace/backslashreplace/surrogateescape/surrogatepass; encode adds namereplace+xmlcharrefreplace) and Windows isatty accuracy.
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:partial): finish file/open parity per ROADMAP checklist + tests, with native/wasm lockstep.)
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): align file handle type names in error/AttributeError messages with CPython _io.* wrappers.)
- WASM `os.getpid()` uses a host-provided pid when available (0 in browser-like hosts).
- Generator introspection: `gi_code` is still stubbed and frame objects only expose `f_lasti`.
  (TODO(introspection, owner:runtime, milestone:TC3, priority:P2, status:missing): implement `gi_code` + full frame objects.)
- Comprehensions: list/set/dict comprehensions, generator expressions, and async comprehensions (async for/await) are supported.
- Differential tests: core-language basic includes pattern matching, async generator finalization, and `while`-`else` probes; failures are expected for pattern matching/async gen until the features are implemented.
- Augmented assignment: slice targets (`seq[a:b] += ...`) are supported, including extended-slice length checks.
- Exceptions: `try/except/else/finally` + `raise`/reraise + `except*` (ExceptionGroup matching/splitting/combining); `__traceback__` now returns
  traceback objects (`tb_frame`/`tb_lineno`/`tb_next`) with frame objects carrying `f_code`/`f_lineno` (see
  `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md`). Builtin exception hierarchy now matches CPython (BaseExceptionGroup,
  OSError/Warning trees, ExceptionGroup MRO).
  (TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame objects to full CPython parity fields.)
  (TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): exception `__init__` + subclass attribute parity (ExceptionGroup tree).)
- Code objects: `__code__` exposes `co_filename`/`co_name`/`co_firstlineno`, with `co_varnames` and
  arg counts (`co_argcount`/`co_posonlyargcount`/`co_kwonlyargcount`) populated; `co_linetable` remains minimal.
  (TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): expand remaining code object fields + linetable parity.)
- Runtime lifecycle: `molt_runtime_init()`/`molt_runtime_shutdown()` manage a `RuntimeState` that owns caches, pools, and async registries; TLS guard drains per-thread caches on thread exit, scheduler/sleep workers join on shutdown, and freed TYPE_ID_OBJECT headers return to the object pool with fallback deallocation for non-pooled types.
- Tooling: `molt clean --cargo-target` removes Cargo `target/` build artifacts when requested.
- Process-based concurrency is partial: spawn-based `multiprocessing` (Process/Pool/Queue/Pipe/SharedValue/SharedArray) is capability-gated and supports `maxtasksperchild`; `fork`/`forkserver` map to spawn semantics (no true fork yet). `subprocess` and `concurrent.futures` remain pending.
  (TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.)
  (TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures`.)
- `sys.argv` is initialized from compiled argv (native + wasm harness); decoding currently uses lossy UTF-8/UTF-16 until surrogateescape/fs-encoding parity lands.
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): decode argv via filesystem encoding + surrogateescape once Molt strings can represent surrogate escapes.)
- `sys.executable` now honors `MOLT_SYS_EXECUTABLE` when set (the diff harness pins it to the host Python to avoid recursive `-c` subprocess spawns); otherwise it falls back to the compiled argv[0].
- `sys.modules` mirrors the runtime module cache for compiled code; `sys._getframe` is available in compiled runtimes with partial frame objects (see introspection TODOs).
- `globals()` can be referenced as a first-class callable (module-bound) and returns the defining module globals; `locals()`/`vars()`/`dir()` remain lowered as direct calls,
  and no-arg callable parity for these builtins is still limited.
  (TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.)
- Runtime safety: NaN-boxed pointer conversions resolve through a pointer registry to avoid int->ptr casts in Rust; host pointer args now use raw pointer ABI in native + wasm; strict-provenance Miri is green.
- Hashing: SipHash13 + `PYTHONHASHSEED` parity (randomized by default; deterministic when seed=0); see `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md`.
- GC: reference counting only; cycle collector pending (see `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md`).
  (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): implement cycle collector.)
- Imports: file-based sys.path resolution and `spec_from_file_location` are supported;
  meta_path/path_hooks/namespace packages remain unsupported.
  (TODO(import-system, owner:stdlib, milestone:TC3, priority:P2, status:partial): meta_path/path_hooks + namespace packages + extension loaders.)
- Entry modules execute under `__main__` while remaining importable under their real module name (distinct module objects).
- Module metadata: compiled modules set `__file__`/`__package__`/`__spec__` (ModuleSpec + filesystem loader) and package `__path__`.
  TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery loader parity (namespace/extension/zip).
- Imports: module-level `from x import *` honors `__all__` (with strict name checks) and otherwise skips underscore-prefixed names.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root builds (namespace packages + PYTHONPATH roots supported; remaining: package discovery hardening, `__init__` edge cases, deterministic dependency graph caching).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:planned): method-binding safety pass (guard/deopt on method lookup + cache invalidation rules for call binding).
- Asyncio: shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `Event`, `wait`, `wait_for`, `shield`, basic `gather`,
  stream helpers (`open_connection`/`start_server`), and `add_reader`/`add_writer`; advanced loop APIs, task groups, and full
  transport/protocol adapters remain pending. Asyncio subprocess stdio now supports `stderr=STDOUT` and fd-based redirection.
  Event-loop semantics target a single-threaded, deterministic scheduler; true parallelism is explicit via executors or isolated
  runtimes.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task APIs + task groups + I/O adapters + executor semantics.)
  Logging core is implemented (Logger/Handler/Formatter/LogRecord + basicConfig) with deterministic formatting and
  capability-gated sinks; `logging.config` and `logging.handlers` remain pending.
  (TODO(async-runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors and explicit message passing; shared-memory parallelism only via opt-in safe types.)
- C API: no `libmolt` C-extension surface yet; `docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md` is target-only.
- Policy: Molt binaries never fall back to CPython; C-extension compatibility is planned via `libmolt` (primary) with an explicit, capability-gated bridge as a non-default escape hatch.
  (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define and implement the initial C API shim).
- Intrinsics registry is runtime-owned and strict; CPython shims have been removed from tooling/tests. `molt_json` and `molt_msgpack` now require runtime intrinsics (no Python-library fallback).
- Matmul (`@`): supported only for `molt_buffer`/`buffer2d`; other types raise
  `TypeError` (TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): consider
  `__matmul__`/`__rmatmul__` fallback for custom types).
- Roadmap focus: async runtime core (Task/Future scheduler, contextvars, cancellation injection), capability-gated async I/O,
  DB semantics expansion, WASM DB parity, framework adapters, and production hardening (see ROADMAP).
- Numeric tower: complex supported; decimal is backed by libmpdec intrinsics with context (prec/rounding/traps/flags),
  quantize/compare/compare_total/normalize/exp/div/as_tuple + `str`/`repr`/float conversions; `int` still missing
  full method surface (e.g., `bit_length`, `to_bytes`, `from_bytes`).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Decimal arithmetic + formatting
  parity (add/sub/mul/pow/sqrt/log/ln/exp variants, quantize edge cases, to_eng_string, NaN payloads).)
  (TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): decimal + `int` method parity.)
- errno: basic constants + errorcode mapping to support OSError mapping; full table pending.
- Format protocol: WASM `n` formatting uses host locale separators via
  `MOLT_WASM_LOCALE_*` (set by `run_wasm.js` when available).
- memoryview: multi-dimensional slicing/sub-views remain pending; slice assignments
  are restricted to ndim = 1.
  (TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): multi-dimensional slicing/sub-views.)
- WASM parity: codec parity tests cover baseline + mixed schema payloads and invalid payload errors via harness
  overrides; advanced schema coverage (binary/float/large ints/tags) is still expanding.
  (TODO(tests, owner:runtime, milestone:SL1, priority:P1, status:partial): expand codec parity coverage for
  binary/floats/large ints/tagged values/deeper container shapes.)
- WASM parity: wasmtime host wires sockets + io_poller readiness with capability checks; Node/WASI host bindings (sockets + readiness, detach, sockopts) live in `run_wasm.js`; browser harness under `wasm/browser_host.html` supports WebSocket-backed stream sockets + io_poller readiness plus the DB host adapter (fetch/JS adapter + cancellation polling). WASM websocket host intrinsics (`molt_ws_*_host`) are available in Node, browser, and wasmtime hosts. WASM process host is wired for Node/wasmtime (spawn + stdin/out/err pipes + cancellation hooks); browser process host remains unavailable. UDP/listen/server sockets remain unsupported in the browser host.
  (TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): expand browser socket coverage (UDP/listen/server sockets) + add more parity tests.)
- Structured codecs: MsgPack is the production default while JSON remains for compatibility/debug.
- Cancellation: cooperative checks plus automatic cancellation injection on await
  boundaries; async I/O cancellation propagation still pending.
  (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): async I/O cancellation propagation.)
- `db_query` Arrow IPC uses best-effort type inference; mixed-type columns error without a declared schema; wasm client shims now consume DB response streams into bytes/Arrow IPC via `molt_db` (async) using MsgPack header parsing (Node/WASI host adapter is implemented in `run_wasm.js`).
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
- Implemented: generator/async poll trampolines are task-aware (generator/coroutine/asyncgen) so wasm no longer relies on arity overrides.
- TODO(perf, owner:compiler, milestone:TC2, priority:P2, status:planned): optimize wasm trampolines with bulk payload initialization and shared helpers to cut code size and call overhead.
- Implemented: cached task-trampoline eligibility on function headers to avoid per-call attribute lookups.
- Implemented: coroutine trampolines reuse the current cancellation token to avoid per-call token allocations.
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
- Implemented: native websocket connect uses the built-in tungstenite host hook (ws/wss, nonblocking) with capability gating; wasm hosts wire `molt_ws_*_host` for browser/Node (wasmtime stubs).
- Implemented: websocket readiness integration via io_poller for native + wasm (`molt_ws_wait_new`) to avoid busy-polling and enable batch wakeups.
- TODO(perf, owner:runtime, milestone:RT3, priority:P2, status:planned): cache mio websocket poll streams/registrations to avoid per-wait `TcpStream` clones.

## Thread Safety + GIL Notes
- Runtime mutation is serialized by a GIL-like lock; only one host thread may
  execute Python/runtime code at a time within the process.
- Runtime state and object headers are not thread-safe; `Value` and heap objects
  are not `Send`/`Sync` unless explicitly documented otherwise.
- Cross-thread sharing of live Python objects is unsupported by default; serialize or
  freeze data before crossing threads.
- `threading.Thread` defaults to isolated runtimes with serialized targets/args; when the
  `thread.shared` capability is enabled, threads share module globals but still use
  serialized targets/args (no arbitrary object passing yet).
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
- Partial shims: `warnings`, `traceback`, `types`, `inspect`, `ast`, `ctypes`, `urllib.parse`, `fnmatch` (`*`/`?`
  + bracket class/range matching; literal `[]`/`[[]`/`[]]` escapes (no backslash
  quoting)), `copy`, `string`, `struct`, `typing`, `sys`, `os`, `pathlib`,
  `tempfile`, `gc`, `weakref`, `random` (Random API + MT parity: `seed`/`getstate`/`setstate`, `randrange`/`randint`/`shuffle`, `choice`/`choices`/`sample`, `randbytes`, `SystemRandom` via `os.urandom`, plus distributions: `uniform`, `triangular`, `normalvariate`, `gauss`, `lognormvariate`, `expovariate`, `vonmisesvariate`, `gammavariate`, `betavariate`, `paretovariate`, `weibullvariate`, `binomialvariate`), `time` (`monotonic`, `perf_counter`, `process_time`, `sleep`, `get_clock_info`, `time`/`time_ns` gated by `time.wall`, plus `localtime`/`gmtime`/`strftime` + `struct_time` + `asctime`/`ctime` + `timezone`/`tzname`), `json` (loads/dumps with parse hooks, indent, separators, allow_nan, `JSONEncoder`/`JSONDecoder`, `JSONDecodeError` details), `base64` (b16/b32/b32hex/b64/b85/a85/z85 encode/decode + urlsafe + legacy helpers), `hashlib`/`hmac` (Rust intrinsics for guaranteed algorithms + `pbkdf2_hmac`/`scrypt`; unsupported algorithms raise), `pickle` (protocol 0 only),
  `socket` (runtime-backed, capability-gated; advanced options + wasm parity pending), `select` (selectors-backed for sockets only),
  `selectors` (io_wait-backed readiness), `asyncio`, `contextvars`, `contextlib`, `threading`, `zipfile`, `zipimport`,
  `functools`, `itertools`, `operator`, `bisect`, `heapq`, `collections`.
  Supported shims: `keyword` (`kwlist`/`softkwlist`, `iskeyword`, `issoftkeyword`), `pprint` (PrettyPrinter/pformat/pprint parity).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand zipfile/zipimport with bytecode caching + broader archive support.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close parity gaps for `ast`, `ctypes`, and `urllib.parse` per matrix coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): complete socket/select/selectors parity (poll/epoll/select objects, fd inheritance, error mapping, cancellation) and align with asyncio adapters.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs exist for regrtest (support: captured_output/captured_stdout/captured_stderr, check_syntax_error, findfile, run_with_tz, warnings_helper utilities: check_warnings/check_no_warnings/check_no_resource_warning/check_syntax_warning/ignore_warnings/import_deprecated/save_restore_warnings_filters/WarningsRecorder, cpython_only, requires, swap_attr/swap_item, import_helper basics: import_module/import_fresh_module/make_legacy_pyc/ready_to_import/frozen_modules/multi_interp_extensions_check/DirsOnSysPath/isolated_modules/modules_setup/modules_cleanup, os_helper basics: temp_dir/temp_cwd/unlink/rmtree/rmdir/make_bad_fd/can_symlink/skip_unless_symlink + TESTFN constants); doctest is blocked on eval/exec/compile gating and full unittest parity is pending.
- Implemented: os.environ mapping methods + backend parity (str-only keys/values, update/pop/setdefault/copy).
- Implemented: uuid module parity (UUID accessors, `uuid1`/`uuid3`/`uuid4`/`uuid5`, namespaces, SafeUUID).
- Implemented: collections.abc parity (ABC registration, structural checks, mixins).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (runtime fast-path parser + performance tuning).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand hashlib/hmac coverage for optional OpenSSL algorithms (sha512_224/sha512_256, ripemd160, md4) and add parity tests for advanced digestmod usage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module exposes only minimal toggles/collect; wire to runtime cycle collector and implement full API.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): `weakref.finalize` atexit registry pending until atexit hooks are available.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): `_abc` cache helpers parity (registry, caches, invalidation).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): replace placeholder iterator/view types (`object`/`type`) so ABC registration doesn't need guards.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): `_py_abc` fallback parity for ABC registry/caches.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `_asyncio` shim uses pure-Python loop helpers; C-accelerated parity pending.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): asyncio submodule parity (events/tasks/streams/etc) beyond import-only allowlisting.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `_bz2` compression backend parity for `bz2`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `random` distribution test vectors and edge-case coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `struct` intrinsics cover full format table (including half-float) with endianness + alignment and aligned error messages; remaining gaps: buffer protocol beyond bytes/bytearray and deterministic layout policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `time` module surface (altzone/daylight + timegm/mktime) + deterministic clock policy.
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale data for `time.localtime`/`time.strftime` on wasm hosts.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/surrogatepass/namereplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (incremental/stream codecs + full encodings import hooks + error-handler registration); base encode/decode intrinsics plus registry/lookup and minimal encodings/aliases are present.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `pickle` protocol 1+ and broader type coverage (bytes/bytearray, memo cycles).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` shim covers constants, predicates, `trunc`/`floor`/`ceil`, `fabs`/`copysign`/`fmod`/`modf`, `frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`, and `sqrt`; Rust intrinsics cover predicates (`isfinite`/`isinf`/`isnan`), `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc`; remaining: determinism policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish remaining `types` shims (CapsuleType + any missing helper/descriptor types).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:missing): implement `types.new_class`, `types.prepare_class`, `types.resolve_bases`, and `types.get_original_bases`, plus a dedicated `DynamicClassAttribute` descriptor.
- Import-only stubs: `collections.abc`, `_collections_abc`, `_abc`, `_py_abc`, `_asyncio`, `_bz2`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement core collections.abc surfaces.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib meta_path/namespace/extension loader parity.
- Implemented: relative import resolution now honors `__package__`/`__spec__` metadata (including `__main__`) and namespace packages, with CPython-matching errors for missing or over-deep parents.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.resources loader/namespace/zip resource readers.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata full parsing + dependency/entry point semantics.
- Planned import-only stubs: `html`, `html.parser`, `http.cookies`, `http.client`, `http.server`,
  `ipaddress`, `mimetypes`, `socketserver`, `wsgiref`, `xml`, `email.policy`, `email.message`, `email.parser`,
  `email.utils`, `email.header`, `urllib.request`, `urllib.error`, `urllib.robotparser`,
  `logging.config`, `logging.handlers`, `cgi`, `zlib`.
  Additional 3.12+ planned/import-only modules (e.g., `annotationlib`, `codecs`, `configparser`,
  `difflib`, `dis`, `encodings`, `tokenize`, `trace`, `xmlrpc`, `zipapp`) are tracked in
  `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` Section 3.0b.
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
  (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB connector parity with real backend coverage (browser host tests cover cancellation + Arrow IPC bytes).)
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
  (publish supports local + HTTP(S) registry targets with optional auth and
  enforces signature/trust policy for remote publishes; `verify` enforces
  manifest/checksum and optional signature/trust policy checks).
- `molt package` and `molt verify` enforce `abi_version` compatibility (currently `0.1`)
  alongside capability/effect allowlists.
- `molt build` enforces lockfiles in deterministic mode, accepts capability
  manifests (allow/deny/package/effects), and can target non-host triples via
  Cranelift + zig linking; `molt package`/`molt verify` enforce capability and
  effect allowlists.
- `molt build` accepts `--pgo-profile` (MPA v0.1) and threads hot-function
  hints into backend codegen ordering.
- `molt package` supports CycloneDX (default) and SPDX SBOM output.
- `molt vendor` materializes Tier A sources into `vendor/` with a manifest.
- `molt vendor` supports git sources when a pinned revision (or tag/branch that resolves
  to a commit) is present, recording resolved commit + tree hash in the manifest.
- Use `tools/dev.py lint` and `tools/dev.py test` for local validation.
- On macOS arm64, uv runs that target Python 3.14 force `--no-managed-python` and
  require a system `python3.14` to avoid uv-managed hangs.
- WIT interface contract lives at `wit/molt-runtime.wit` (WASM runtime intrinsics).
- Single-module wasm linking via `tools/wasm_link.py` (requires `wasm-ld`) is required for Node/wasmtime runs of runtime outputs; enable with `--linked`/`--require-linked` (or `MOLT_WASM_LINK=1`).

## Known Gaps
- Browser host harness is available under `wasm/browser_host.html` with
  DB host support, WebSocket-backed stream sockets, and websocket host intrinsics; production browser host I/O is still pending for storage + broader parity coverage.
  (TODO(wasm-host, owner:runtime, milestone:RT3, priority:P2, status:partial): add browser host I/O bindings + capability plumbing for storage and parity tests.)
- Cross-target native builds (non-host triples/architectures) are not yet wired into
  the CLI/build pipeline.
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): wire cross-target builds into CLI.)
- SQLite/Postgres connectors remain native-only; wasm DB host adapters exist (Node/WASI + browser), parity tests now cover browser host cancellation + Arrow IPC payload delivery, but real backend coverage is still pending.
  (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity with real backend coverage.)
- Single-module WASM link now rejects `molt_call_indirect*` imports, `reloc.*`/`linking`/`dylink.0` sections, and table/memory imports; element segments are validated to target table 0 with `ref.null`/`ref.func` init exprs. Linked runs no longer rely on JS call_indirect stubs (direct-link path still uses env wrappers by design).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): re-enable safe direct-linking by relocating the runtime heap base or enforcing non-overlapping memory layouts to avoid wasm-ld in hot loops.
- Implemented: linked-wasm dynamic intrinsic dispatch no longer requires Python static-dispatch shims for channel intrinsics; runtime uses a canonical 64-bit channel handle ABI so dynamic intrinsic calls and direct calls share the same call_indirect signature.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): OPT-0003 phase 1 landed (sharded pointer registry); benchmark and evaluate lock-free alternatives next (see `OPTIMIZATIONS_PLAN.md`).
- Single-module wasm linking remains experimental; wasm-ld links relocatable output when `MOLT_WASM_LINK=1`, but broader module coverage is still pending (direct-link runs are disabled for now).
