# Stdlib Compatibility Matrix
**Spec ID:** 0015
**Status:** Draft (implementation-tracking)
**Owner:** stdlib + runtime + frontend
**Goal:** Provide a production-grade, deterministic subset of the CPython standard library with clear import rules and capability gating.

## 0. Principles
- **Explicit imports:** everything outside builtins requires `import` (no implicit injection).
- **Minimal core:** core runtime stays lean; stdlib modules live outside the core unless explicitly promoted.
- **Capability gating:** any OS, I/O, network, or process control requires explicit capability grants.
- **Trusted override:** `MOLT_TRUSTED=1` disables capability checks for trusted native deployments.
- **Determinism first:** hashing, ordering, and file/system APIs must preserve deterministic output.
- **Enforcement:** use `molt.capabilities` to check or require capability tokens in stdlib shims.
- **Intrinsic-only enforcement:** compiled binaries may only import stdlib modules that are intrinsic-backed; Python-only stdlib modules must fail fast (compile-time error or immediate `RuntimeError`) until lowered.
- **Import-only stubs (tooling-only):** stubs may be used for dependency tracking in tooling, but are forbidden in compiled binaries.
- **CPython union coverage:** Molt must include one top-level stdlib module/package for each CPython stdlib entry and one `.py` submodule/subpackage for each CPython stdlib submodule entry in the 3.12/3.13/3.14 union baseline (`tools/stdlib_module_union.py`), enforced by `tools/check_stdlib_intrinsics.py`. Update process: `docs/spec/areas/compat/0027_STDLIB_TOP_LEVEL_UNION_BASELINE.md`.
- **Intrinsic-partial ratchet:** `intrinsic-partial` count must remain at or below `tools/stdlib_intrinsics_ratchet.json` (`max_intrinsic_partial`), enforced by `tools/check_stdlib_intrinsics.py`.
- **Execution sequencing:** blocker-first lowering order and tranche acceptance criteria are tracked in `docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md`.

## 0.1 Tier-0 Direct-Call Rule
- **Direct-call allowlist:** Tier 0 compiles module-level calls to a static `CALL` only for allowlisted functions from allowlisted modules (this matrix + frontend allowlist).
- **No monkey-patching:** rebinding or monkey-patching allowlisted functions is not observed in Tier 0; the call target is fixed at compile time.
- **Fallbacks:** non-allowlisted module-level calls raise a compiler error unless `--fallback=bridge` is enabled, in which case a bridge warning is emitted.
- **Warnings control:** set `MOLT_COMPAT_WARNINGS=0` to suppress bridge warnings during compilation.

## 0.2 Submodule Policy
- **Explicit tracking:** submodules are listed explicitly in this matrix (or noted under the parent) before they are considered importable.
- **Deterministic registration:** submodules must be created as module objects, registered in `sys.modules`, and attached to their parent package; avoid dynamic attribute-based imports.
- **Capability parity:** submodules that touch I/O/OS/process boundaries inherit the same capability gates as their parent module.

## 0.3 Coverage Notes
- Differential coverage includes a Click/Trio stdlib surface pack (see `tests/differential/COVERAGE_INDEX.yaml` and `tests/differential/planned/*_basic.py`).

## 0.4 Version Policy
- This matrix targets CPython **3.12+** semantics.
- When behavior differs across 3.12/3.13/3.14, record the chosen target in the
  Notes column (e.g., "3.14-only") and align tests/runtime to that version.

## 1. Policy: Core vs Import vs Gated
- **Core (always available):** builtins and compiler/runtime intrinsics only.
- **Core-adjacent (import required, fast path):** modules with compiler/runtime intrinsics for hot paths.
- **Stdlib (import required):** pure-Python or Rust-backed modules with deterministic semantics.
- **Capability-gated:** modules that read/write the host or open network/process boundaries.

## 2. Decorators, Dataclasses, and Typing Policy
- **Decorator whitelist:** only approved decorators are allowed in Tier 0 (e.g., `@dataclass`, `@lru_cache`); other decorators must be pure and deterministic or are rejected.
- **Compile-time lowering:** known decorators are lowered into explicit IR (no dynamic `exec`/metaclass evaluation).
- **Typing:** annotations are preserved in `__annotations__` and used at compile time; runtime typing helpers are scoped and deterministic.
- **Type-hints policy:** `--type-hints=trust` applies annotations to lowering; `--type-hints=check` inserts runtime guards.
- **Safety bound:** decorators or typing helpers that mutate globals/types at runtime require explicit opt-in and Tier 1 guards.

## 3. Compatibility Matrix
Program sequencing for full Rust lowering (core first, then stdlib) is tracked
in `docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md`.

| Module | Tier | Status | Priority | Milestone | Owner | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| builtins | Core | Partial | P0 | SL1 | runtime/frontend | Importable module binds supported builtins (including function objects for allowlisted builtins); missing names raise `AttributeError`. |
| __future__ | Stdlib | Supported | P3 | SL3 | stdlib | Feature metadata (`_Feature`, `all_feature_names`, compiler flags) synced to CPython 3.12. |
| functools | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | `partial`, `reduce`, `lru_cache`, `wraps`/`update_wrapper`, `cmp_to_key`, `total_ordering`; `partial`/`lru_cache` accept `*args`/`**kwargs` (no fast paths yet). |
| itertools | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | `chain`, `islice`, `repeat`, `count`, `cycle`, `accumulate`, `pairwise`, `product`, `permutations`, `combinations`, `groupby`, `tee` (product/permutations/combinations are eager; no generators yet). |
| operator | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | Basic helpers (`add`, `mul`, `eq`, `itemgetter`, `attrgetter`, `methodcaller`). |
| math | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | Constants (`pi`/`e`/`tau`/`inf`/`nan`) plus `isfinite`/`isnan`/`isinf`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`/`fmod`/`modf`, `frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`, and `sqrt`; Rust intrinsics cover predicates, `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc`; remaining: determinism policy. |
| collections | Stdlib | Partial | P1 | SL1 | stdlib | `deque` core ops + rotate/index/insert/remove; `Counter`/`defaultdict` dict subclasses with arithmetic, in-place ops, and Counter keys/values/items/total + dict-style clear/pop/popitem/setdefault parity. |
| keyword | Stdlib | Supported | P3 | SL3 | stdlib | Hard/soft keyword tables (`kwlist`, `softkwlist`) + `iskeyword`/`issoftkeyword` for CPython 3.12+. |
| heapq | Stdlib | Partial | P1 | SL1 | stdlib | `heapify`/`heappush`/`heappop`/`heapreplace`/`heappushpop` + `nlargest`/`nsmallest`, `merge` (eager; full materialization/sort), max-heap helpers, runtime fast paths. |
| bisect | Stdlib | Partial | P1 | SL1 | stdlib | `bisect_left`/`bisect_right` + `insort_left`/`insort_right` with `key` support; aliases `bisect`/`insort`. |
| array | Stdlib | Planned | P1 | SL1 | runtime | Typed array storage, interop-ready. |
| struct | Stdlib | Partial | P1 | SL1 | runtime | Runtime intrinsics cover the CPython 3.12 format table (including half-float) with endianness + alignment; pack/unpack paths support bytes/bytearray/C-contiguous memoryview (including nested memoryview windows) and remain intrinsic-only (no host fallback). Remaining: exact CPython diagnostic-text parity on selected edge cases. (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): align remaining struct edge-case error text with CPython.) |
| re | Stdlib | Partial | P1 | SL2 | stdlib | Native engine supports literals, `.`, char classes/ranges (`\\d`/`\\w`/`\\s`), groups/alternation, greedy + non-greedy quantifiers, and `IGNORECASE`/`MULTILINE`/`DOTALL`; advanced features/flags raise `NotImplementedError` (no host fallback). Full parity pending. |
| decimal | Stdlib | Partial | P2 | SL2 | stdlib | Rust intrinsic-backed constructor + context (prec/rounding/traps/flags), `as_tuple`, `str`/`repr`/float, quantize/compare/compare_total/normalize/exp/div. Runtime uses vendored libmpdec when available and a native Rust backend otherwise; no Python fallback path. Remaining: full arithmetic (add/sub/mul/pow/sqrt/log), formatting helpers (`to_eng_string`), NaN payloads + edge-case signaling parity. |
| fractions | Stdlib | Planned | P2 | SL2 | stdlib | Rational arithmetic. |
| statistics | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Intrinsic-backed function surface (`mean`, `fmean`, `stdev`, `variance`, `pvariance`, `pstdev`, `median`, `median_low`, `median_high`, `median_grouped`, `mode`, `multimode`, `quantiles`, `harmonic_mean`, `geometric_mean`, `covariance`, `correlation`, `linear_regression`) with shim-level `StatisticsError` mapping; remaining parity is broader 3.12+ API/PEP coverage (for example `NormalDist`) and edge-case message alignment. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete full statistics 3.12+ API/PEP parity beyond function surface lowering.) |
| random | Stdlib | Partial | P2 | SL2 | stdlib | Deterministic Mersenne Twister parity with `Random`/`seed`/`getstate`/`setstate`, `randrange`/`randint`/`shuffle`, `choice`/`choices`/`sample`, `randbytes`, `SystemRandom` (via `os.urandom`), and distribution methods (`uniform`, `triangular`, `normalvariate`, `gauss`, `lognormvariate`, `expovariate`, `vonmisesvariate`, `gammavariate`, `betavariate`, `paretovariate`, `weibullvariate`, `binomialvariate`). Remaining: extended test vectors. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand random distribution test vectors) |
| datetime | Stdlib | Planned | P2 | SL2 | stdlib | Time types + parsing. |
| zoneinfo | Stdlib | Planned | P3 | SL3 | stdlib | Timezone data handling. |
| pathlib | Stdlib | Partial | P2 | SL2 | stdlib | Basic `Path` wrapper with gated `open`/read/write/exists/unlink/iterdir plus `mkdir`/`rmdir`, intrinsic-backed `glob`/`rglob` segment matching (`*`, `?`, `[]`, `**`), `parts`/`parents`, `name`/`suffix`/`suffixes`/`stem`, `joinpath`/`__truediv__`/`__rtruediv__`, `with_name`, `with_suffix`, `relative_to`, `match` (basic patterns), `as_posix`, `expanduser`, `is_absolute`, and intrinsic-backed `resolve(strict=...)` (`molt_path_resolve`); runtime path shaping now includes splitroot-aware Windows drive/UNC absolute handling (`molt_path_isabs`/`molt_path_parts`/`molt_path_parents`) with no Python fallback. `PurePosixPath` remains mapped to `Path`; richer Path ops still pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining pathlib glob edge parity (`root_dir`/hidden semantics, full Windows flavor/symlink nuances) and full Path parity) |
| enum | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Enum/IntEnum/Flag/IntFlag base types with `auto`, name/value access, and member maps; member initialization now routes through runtime intrinsic `molt_enum_init_member`. Aliasing, functional API, and full Flag semantics remain pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): finish Enum/Flag/IntFlag parity.) |
| dataclasses | Stdlib | Partial | P2 | SL2 | stdlib | Dataclass lowering covers init/repr/eq/order/unsafe_hash/frozen/slots/match_args/kw_only, Field flags, InitVar/ClassVar/KW_ONLY, __match_args__, stdlib helpers, and `make_dataclass` (runtime class construction + decorator parity path). Remaining gap: non-dataclass base inheritance guarantees. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.) |
| typing | Stdlib | Supported | P2 | SL3 | stdlib | Deterministic runtime typing helpers: `Annotated`/`Literal`/`Union`/`Optional`/`Callable`, `TypeVar`/`ParamSpec`/`TypeVarTuple`, `NewType`/`TypedDict`, `Protocol` + `@runtime_checkable`, `get_origin`/`get_args`/`get_type_hints` (explicit eval only). |
| abc | Stdlib | Partial | P3 | SL3 | stdlib | `ABCMeta`/`ABC` + `abstractmethod` with intrinsic-backed `_abc` registry/caches (`register`, `__instancecheck__`, `__subclasscheck__`, cache token + reset helpers). Remaining: edge-case parity for subclasshook/cache invalidation behavior. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining abc edge-case parity around subclasshook/cache invalidation.) |
| _abc | Stdlib | Partial | P3 | SL3 | stdlib | Intrinsic-backed `_abc` bootstrap exports cache token/init/register/instancecheck/subclasscheck/get_dump/reset_registry/reset_caches; no host-Python fallback path. Remaining: CPython edge-case parity for cache/version corner cases. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close `_abc` edge-case cache/version parity.) |
| contextlib | Stdlib | Supported | P2 | SL2 | stdlib/runtime | Runtime intrinsics cover `contextmanager`/`ContextDecorator` + `ExitStack`/`AsyncExitStack`, `asynccontextmanager`, `aclosing`, `suppress`, `redirect_stdout`/`redirect_stderr`, `nullcontext`, `closing`, `AbstractContextManager`, `AbstractAsyncContextManager`, and `chdir` (including intrinsic-backed abstract subclasshook checks and cwd enter/exit plumbing). |
| contextvars | Stdlib | Partial | P2 | SL3 | stdlib/runtime | `ContextVar`/`Token`/`Context` + `copy_context`; task context propagation via cancel tokens; `Context.run` implemented. |
| gc | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Minimal `collect`/enable/disable shim for test support; cycle collector wiring + full API pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full gc module API + runtime cycle collector hook.) |
| weakref | Stdlib | Partial | P3 | SL3 | stdlib | Runtime-backed weakrefs + proxies + WeakKey/ValueDictionary + WeakSet + WeakMethod + finalize + getweakrefcount/refs (runtime registry-backed, including callback refs and no-callback dedupe semantics); finalize registry is tracked in runtime and drained during runtime shutdown. Remaining: tighten `finalize.atexit` shutdown-order edge-case parity against CPython. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close weakref finalize shutdown-order edge-case parity.) |
| _weakref | Stdlib | Supported | P3 | SL3 | stdlib | `_weakref` parity: exports ref/proxy types + weakref counts/refs. |
| _weakrefset | Stdlib | Supported | P3 | SL3 | stdlib | `_weakrefset.WeakSet` parity (runtime-backed weak semantics). |
| _intrinsics | Stdlib | Supported | P3 | SL3 | stdlib | Intrinsic loader used by stdlib modules to bind runtime helpers. |
| logging | Stdlib | Partial | P2 | SL2 | stdlib | Deterministic logging core (Logger/Handler/Formatter/LogRecord + Stream/File/Null handlers + basicConfig + `captureWarnings`); sinks gated by `fs.write`. |
| logging.config | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; config parsing pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.config pending parity) |
| logging.handlers | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; handler wiring pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.handlers pending parity) |
| json | Stdlib | Partial | P1 | SL2 | stdlib | Shim supports `loads`/`dumps`/`load`/`dump` with parse hooks, indent, separators, and `allow_nan`, plus `JSONEncoder`/`JSONDecoder` and `JSONDecodeError` details; runtime fast-path + full parity pending. |
| csv | Stdlib | Planned | P3 | SL3 | stdlib | Deterministic CSV parsing. |
| io | Capability-gated | Partial | P2 | SL3 | stdlib | Native `open` supports full signature + fd-based open; IOBase hierarchy (IOBase/RawIOBase/BufferedIOBase/TextIOBase) plus file objects expose core methods/attrs (read/read1/readall/readinto/readinto1/write/writelines/seek/tell/fileno/readline(s)/truncate/iteration/flush/close, newline/newlines/encoding/errors/line_buffering/write_through, `buffer` on text wrappers, `closefd` on raw handles). `io.UnsupportedOperation` exported; BytesIO/StringIO available. utf-8/utf-8-sig/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/ascii/latin-1/utf-16/utf-32 text encoding only (encode handlers include namereplace+xmlcharrefreplace), text-mode seek/tell cookies partial, and Windows isatty parity still pending. (TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:partial): io pending parity) |
| os | Capability-gated | Partial | P2 | SL3 | stdlib | Intrinsic-backed shim: env access gated via `env.read`/`env.write` with runtime-backed `os.environ` state (`molt_env_snapshot`/`molt_env_set`/`molt_env_unset`) and dedicated `os.putenv`/`os.unsetenv` intrinsics (`molt_env_putenv`/`molt_env_unsetenv`) matching CPython-visible mapping separation; path helpers (`basename`/`split`/`relpath`/`expandvars`) + fs helpers (`exists`/`isdir`/`isfile`/`unlink`/`remove`) + fd ops (`close`/`pipe`/`dup`/`read`/`write`/`get_inheritable`/`set_inheritable`); `urandom` gated by `rand`. Remaining: full process/signal/stat surface + broader parity long tail. |
| sys | Capability-gated | Partial | P2 | SL3 | stdlib | Minimal shim: argv/version/version_info/path/modules (synced from runtime module cache) + recursion limits; stdio and bootstrap state are intrinsic-only via `molt_sys_bootstrap_payload` (`sys.path` + module roots + `PYTHONPATH`/`VIRTUAL_ENV` site-packages/`PWD`/stdlib-root/include-cwd policy) with no host-Python fallback path; `sys.version` + `sys.version_info` are stamped by the toolchain intrinsic; `sys.exc_info()` reads the active exception context; compiled argv now sourced from runtime; host info gated via `env.read` (argv encoding parity TODO; `sys._getframe` uses partial frame objects). |
| errno | Stdlib | Full | P2 | SL2 | stdlib | Full CPython errno constants + errorcode mapping (native build-time generation; WASM keeps minimal errno set). |
| stat | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Intrinsic-backed stat constants/mode helpers now include file-type constants (`S_IFSOCK`/`S_IFLNK`/`S_IFREG`/`S_IFBLK`/`S_IFDIR`/`S_IFCHR`/`S_IFIFO`), permission/set-id bits, `ST_*` indexes, and helpers (`S_IFMT`, `S_IMODE`, `S_ISDIR`, `S_ISREG`, `S_ISCHR`, `S_ISBLK`, `S_ISFIFO`, `S_ISLNK`, `S_ISSOCK`) via `molt_stat_constants` + `molt_stat_*` intrinsics. Remaining: broader POSIX/macOS/BSD constant long-tail parity. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand stat constant/mode helper parity.) |
| signal | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Signal handling; gated by `process.signal`. |
| select | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | `select.select` plus selector-object backends (`poll`/`epoll`/`kqueue`/`devpoll`) are Rust-intrinsic-backed via runtime selector registries (`molt_select_selector_*`) and readiness polling; Python shims now only normalize signatures/events and platform-gate exported constructors. Remaining gaps: full OS-flag/error fidelity and broader non-socket fd/device parity. |
| site | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; path config gated via `env.read`/`fs.read`. |
| sysconfig | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; host/path data gated via `env.read`/`fs.read`. |
| subprocess | Capability-gated | Planned | P3 | SL3 | stdlib | Process spawn control. |
| socket | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | Runtime-backed socket API (AF_INET/AF_INET6/AF_UNIX, connect/bind/listen/accept/send/recv, socketpair, shutdown/half-close, sendall, recv_into/peek, `sendmsg`/`recvmsg`/`recvmsg_into`, getaddrinfo/nameinfo/hostby*/fqdn, dup/fromfd, makefile, inet_pton/ntop, UDP echo/truncation, nonblocking connect SO_ERROR, dualstack, websocket handshakes); `dup` uses runtime socket-handle cloning and default-timeout validation is CPython-shaped. Unix ancillary control-plane coverage is now intrinsic-backed for `cmsghdr` tuple encode/decode (including `SCM_RIGHTS` fd-passing lane). WASM/non-Unix host ABI now carries ancillary payloads + `msg_flags` end-to-end for `sendmsg`/`recvmsg`/`recvmsg_into`, including ancillary transport on wasm-managed stream peer paths (for example `socketpair`); unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty ancillary control messages. Advanced options/full constant table, SSL, and Node/WASI/browser parity pending (wasmtime host implemented). |
| ssl | Capability-gated | Planned | P3 | SL3 | stdlib | TLS primitives. |
| asyncio | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | Shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `create_task`/`ensure_future`/`current_task`, `wait`/`wait_for`/`gather`, and stream/socket adapters; TaskGroup/Runner cancellation fanout is intrinsic-backed (`molt_asyncio_cancel_pending` + gather path), synchronization waiter hot paths lower through Rust intrinsics (`molt_asyncio_waiters_notify`, `molt_asyncio_waiters_notify_exception`, `molt_asyncio_waiters_remove`, `molt_asyncio_barrier_release`), task/future callback transfer + event waiter teardown are intrinsic-backed (`molt_asyncio_future_transfer`, `molt_asyncio_event_waiters_cleanup`), task registry + event-waiter token maps are runtime-owned (`molt_asyncio_task_registry_set`/`get`/`current`/`pop`/`move`/`values`, `molt_asyncio_event_waiters_register`/`unregister`/`cleanup_token`), running/event-loop state is intrinsic-backed (`molt_asyncio_running_loop_get`/`set`, `molt_asyncio_event_loop_get`/`set`, `molt_asyncio_event_loop_policy_get`/`set`), TaskGroup done-callback fanout + ready-queue dispatch are intrinsic-backed (`molt_asyncio_taskgroup_on_task_done`, `molt_asyncio_ready_queue_drain`), coroutine predicates route through inspect intrinsics (`molt_inspect_iscoroutine`, `molt_inspect_iscoroutinefunction`), and TLS execution is runtime-owned for client and server lanes (`molt_asyncio_tls_client_connect_new`, `molt_asyncio_tls_client_from_fd_new`, `molt_asyncio_tls_server_payload`, `molt_asyncio_tls_server_from_fd_new`) across `open_connection`/`create_connection`, `open_unix_connection`/`create_unix_connection`, client/server-side `start_tls`, and `start_server`/`start_unix_server`. Advanced loop APIs and full submodule parity remain pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): asyncio pending parity) |
| _asyncio | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Minimal intrinsic-backed running/event-loop hooks (`_get_running_loop`/`_set_running_loop`/`get_running_loop`, intrinsic-backed `get_event_loop` path via runtime loop/policy state); broader CPython C-accelerated helper parity remains pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement `_asyncio` parity or runtime hooks.) |
| asyncio.base_events/asyncio.coroutines/asyncio.events/asyncio.exceptions/asyncio.futures/asyncio.locks/asyncio.protocols/asyncio.runners/asyncio.queues/asyncio.streams/asyncio.subprocess/asyncio.tasks/asyncio.taskgroups/asyncio.timeouts/asyncio.threads/asyncio.transports/asyncio.unix_events/asyncio.windows_events | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stubs for asyncio submodules; parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): asyncio submodule parity.) |
| selectors | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | CPython-shaped `SelectSelector`/`PollSelector`/`EpollSelector`/`KqueueSelector`/`DevpollSelector` now route through intrinsic-backed `select` object backends (`molt_select_selector_*`) instead of Python async wait fan-out. Remaining: full OS-specific flag/error fidelity, fd inheritance corners, and broader wasm/browser host parity. |
| threading | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Shared-runtime intrinsic-backed `Thread` lifecycle by default (`spawn_shared`/join/is_alive/ident/native_id + runtime registry), plus intrinsic-backed lock/rlock/condition/event/semaphore/barrier/local primitives and runtime-owned `stack_size` control (`molt_thread_stack_size_get`/`molt_thread_stack_size_set`). `RLock` ownership + recursion save/restore are runtime-owned (`molt_rlock_is_owned`, `molt_rlock_release_save`, `molt_rlock_acquire_restore`) with Python wrappers limited to API shaping. Current differential basic lane (`threading_*.py`) is green (`24/24`) under intrinsic-only compiled runs; remaining work is broader CPython surface/regrtest parity and determinism-sensitive scheduling corners. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.) |
| multiprocessing | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Spawn-based Process/Pool/Queue/Pipe/SharedValue/SharedArray; `maxtasksperchild` supported; `fork`/`forkserver` map to spawn semantics (TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): implement true fork support). |
| concurrent.futures | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Executor interfaces. |
| sqlite3 | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | DB integration. |
| http | Capability-gated | Planned | P3 | SL3 | stdlib | HTTP parsing/client. |
| http.client | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned request/response lane via `molt_http_client_*` intrinsics (`execute`, response `read`/`close`/`getstatus`/`getreason`/`getheader`/`getheaders`) with Python shim reduced to request-state wiring (`putrequest`/`putheader`/`endheaders`/`send`/`request`/`getresponse`). Remaining: full CPython `HTTPConnection` state-machine parity (persistent connection reuse/chunked/CONNECT/proxy edge flows). (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.client connection/chunked/proxy parity on top of intrinsic execute/response core.) |
| http.server | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned serve loop/shutdown lane via `molt_socketserver_serve_forever`/`molt_socketserver_shutdown` and intrinsic-backed socketserver dispatch queue. Remaining: full CPython HTTP parser/handler edge semantics, richer server subclasses, and complete error/connection lifecycle parity. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.server parser/handler lifecycle parity.) |
| http.cookies | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; cookie parsing/quoting pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): http.cookies pending parity) |
| urllib | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Intrinsic-backed package root imports `urllib.parse`, `urllib.error`, and `urllib.request` through required runtime intrinsics; `robotparser` and broader HTTP/network handler parity remain pending. |
| urllib.parse | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned URL parsing/quoting/query helpers via `molt_urllib_*` intrinsics (`quote`/`quote_plus`/`unquote`/`unquote_plus`, `parse_qs`/`parse_qsl`, `urlencode`, `urlsplit`/`urlparse`/`urlunsplit`/`urlunparse`, `urljoin`, `urldefrag`); Python shim is reduced to result-object shaping + minimal argument normalization. Remaining: full CPython edge semantics (`params`/IDNA/bytes interfaces/validation details). (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.parse parity gaps.) |
| urllib.request | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned opener core via `molt_urllib_request_*` intrinsics (`Request` init, `OpenerDirector` init/add_handler/open dispatch with handler ordering + `data:` URL fallback); Python shim is reduced to class shells + result adaptation (`bytes` -> `io.BytesIO`). Remaining: full handler stack parity (`HTTPHandler`/`HTTPSHandler`/proxy/cookie/auth/redirect), richer response objects (`addinfourl`), and complete network capability-gated flows. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish urllib.request handler/response/network parity on top of intrinsic opener core.) |
| urllib.error | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned exception construction/formatting via `molt_urllib_error_*` intrinsics (`URLError`, `HTTPError`, `ContentTooShortError`) with Python shim reduced to class shell + attribute/property wiring. Remaining: full `urllib.response.addinfourl` integration and complete `urllib.request` interaction semantics. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.error/request integration parity.) |
| urllib.robotparser | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; network fetch requires `net`. |
| email | Stdlib | Planned | P3 | SL3 | stdlib | Email parsing. |
| email.policy | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; policy objects pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.policy pending parity) |
| email.message | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; message objects pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.message pending parity) |
| email.parser | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parser helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.parser pending parity) |
| email.utils | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; formatting helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.utils pending parity) |
| email.header | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; header encoding pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.header pending parity) |
| gzip/bz2/lzma | Stdlib | Planned | P3 | SL3 | stdlib | Compression modules. |
| _bz2 | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; bz2 codec backend. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement `_bz2` compression/decompression parity.) |
| zlib | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; compression primitives pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): zlib pending parity) |
| zipfile/tarfile | Stdlib | Partial | P3 | SL3 | stdlib | `zipfile` store/deflate + zip64 read/write; tarfile parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement tarfile parity.) |
| hashlib/hmac | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Rust intrinsics now cover guaranteed algorithms plus optional OpenSSL-family algorithms used by CPython (`sha512_224`/`sha512_256`, `ripemd160`, `md4`) with intrinsic-backed `pbkdf2_hmac`/`scrypt` + `compare_digest`; no host fallback lane. Remaining: broaden advanced digestmod parity coverage. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.) |
| secrets | Stdlib | Planned | P3 | SL3 | stdlib | Crypto RNG (gated). |
| uuid | Stdlib | Full | P2 | SL2 | stdlib | `UUID` surface with fields/bytes_le/variant/urn accessors, `uuid1`/`uuid3`/`uuid4`/`uuid5`, namespace constants, and `SafeUUID` (uuid1 requires `time.wall`). |
| base64 | Stdlib | Full | P2 | SL2 | stdlib | b16/b32/b32hex/b64/b85/a85/z85 encode/decode, urlsafe variants, and legacy encode/decode helpers. |
| binascii | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; low-level codecs pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): binascii pending parity) |
| pickle | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Protocol 0 only (basic builtins + slice) with dumps byte assembly lowered through runtime intrinsic `molt_pickle_encode_protocol0`. Protocol 1+, bytes/bytearray memo cycles, and broader type coverage remain pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement protocol 1+, bytes/bytearray, memo cycles, and broader type coverage) |
| unittest | Stdlib | Partial | P3 | SL3 | stdlib | Minimal TestCase/runner stubs for CPython regrtest; full assertions/loader parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest pending parity) |
| doctest | Stdlib | Partial | P3 | SL3 | stdlib | Stub that rejects eval/exec/compile usage for now. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): doctest pending parity once eval/exec/compile are gated.) |
| test | Stdlib | Partial | P3 | SL3 | stdlib | Minimal `test.support` helpers (`captured_output`/`captured_stdout`/`captured_stderr`, `check_syntax_error`, `findfile`, `run_with_tz`, `warnings_helper` utilities: `check_warnings`/`check_no_warnings`/`check_no_resource_warning`/`check_syntax_warning`/`ignore_warnings`/`import_deprecated`/`save_restore_warnings_filters`/`WarningsRecorder`, `cpython_only`, `requires`, `swap_attr`/`swap_item`, `import_helper` basics: `import_module`/`import_fresh_module`/`make_legacy_pyc`/`ready_to_import`/`frozen_modules`/`multi_interp_extensions_check`/`DirsOnSysPath`/`isolated_modules`/`modules_setup`/`modules_cleanup`, `os_helper` basics: `temp_dir`/`temp_cwd`/`unlink`/`rmtree`/`rmdir`/`make_bad_fd`/`can_symlink`/`skip_unless_symlink` + TESTFN constants); full CPython harness pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): test package pending parity) |
| argparse | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): argparse pending parity) |
| getopt | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): getopt pending parity) |
| locale | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Intrinsic-backed runtime locale state for `setlocale`/`getpreferredencoding`/`getlocale` (deterministic shim lane with `C`/`POSIX` normalization and UTF-8/US-ASCII encoding labels). Remaining: full host locale catalog/parsing parity. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics.) |
| ast | Stdlib | Partial | P3 | SL3 | stdlib | Minimal `parse`/`walk`/`get_docstring`; full node classes + visitor helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): ast parity gaps.) |
| atexit | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; exit handler semantics pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): atexit pending parity) |
| collections.abc | Stdlib | Full | P3 | SL3 | stdlib | Full ABCs with structural checks, registrations, and mixin helpers (Set/Mapping/Sequence, async ABCs, Callable generics). |
| _collections_abc | Stdlib | Full | P3 | SL3 | stdlib | Full collections ABC implementation (re-exported by `collections.abc`). |
| importlib | Stdlib | Partial | P2 | SL3 | stdlib | File-based module loading + `import_module`/`reload`/`find_spec` for sys.path; `import_module` dispatch lowers via `molt_module_import` (no Python `__import__` fallback). `find_spec` now routes meta/path finder execution, namespace package discovery, zip-source spec discovery, extension-module spec discovery, sourceless bytecode spec discovery, and `path_importer_cache` finder reuse through runtime intrinsics. Remaining: full extension/sourceless execution parity beyond capability-gated intrinsic shim lanes. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib extension/sourceless execution parity) |
| importlib.util | Stdlib | Partial | P2 | SL3 | stdlib/runtime | `find_spec`/`module_from_spec`/`spec_from_file_location` for filesystem + namespace + zip-source paths; sys bootstrap/path resolution and spec/cache shaping lower through runtime payload intrinsics (`molt_importlib_bootstrap_payload`, `molt_importlib_runtime_state_payload`, `molt_importlib_find_spec_payload`, `molt_importlib_cache_from_source`, `molt_importlib_spec_from_file_location_payload`) with extension/sourceless-bytecode spec discovery plus `meta_path`/`path_hooks`/`path_importer_cache` finder reuse lowered in runtime payloads. Remaining: loader execution parity for extension/sourceless paths. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.util non-source loader execution parity) |
| importlib.machinery | Stdlib | Partial | P2 | SL3 | stdlib/runtime | `ModuleSpec` + filesystem `SourceFileLoader` + `ZipSourceLoader`; package/module shaping lowers via `molt_importlib_source_loader_payload`/`molt_importlib_zip_source_exec_payload`, module spec package detection lowers via `molt_importlib_module_spec_is_package`, file reads lower via `molt_importlib_read_file`, and restricted module source execution lowers via `molt_importlib_exec_restricted_source` (no Python-side fallback parser). `ExtensionFileLoader`/`SourcelessFileLoader` execution paths are intrinsic-owned and capability-gated, with explicit intrinsic execution candidate lanes (`*.molt.py` + `*.py`) and continue-on-probe behavior for unsupported restricted-shim candidates before deterministic `ImportError`; restricted shim execution now includes runtime-owned `from ... import *` handling (`__all__` + underscore fallback). (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery full native extension/pyc execution parity beyond restricted source shim lanes) |
| importlib.resources | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | `files`/`open_text`/`read_binary`/traversable APIs with package root + namespace discovery lowered via `molt_importlib_resources_package_payload`, traversable stat/listdir payloads lowered via `molt_importlib_resources_path_payload` (filesystem + zip/whl/egg namespace/resource roots), direct read/open helpers using intrinsic file IO (`molt_importlib_read_file`) for filesystem and archive entries, loader reader bootstrap lowered through `molt_importlib_resources_module_name`/`molt_importlib_resources_loader_reader` (with fallback from `module.__spec__.loader` to `module.__loader__`), and custom loader reader contract support lowered through dedicated runtime intrinsics (`molt_importlib_resources_reader_roots`, `molt_importlib_resources_reader_contents`, `molt_importlib_resources_reader_resource_path`, `molt_importlib_resources_reader_is_resource`, `molt_importlib_resources_reader_open_resource_bytes`, `molt_importlib_resources_reader_child_names`). Zip archive-member paths are explicitly tagged in runtime payloads so `resource_path()` stays filesystem-only across direct + traversable + roots fallback lanes, while `open_resource()` remains intrinsic-backed for archive reads. |
| importlib.metadata | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | `distribution`/`version`/`entry_points` via dist-info scan; bootstrap + scan lower via `molt_importlib_bootstrap_payload` and `molt_importlib_metadata_dist_paths`, aggregated and filtered entry points lower via `molt_importlib_metadata_entry_points_payload`/`molt_importlib_metadata_entry_points_select_payload`, distribution name canonicalization lowers via `molt_importlib_metadata_normalize_name`, file reads lower via `molt_importlib_read_file`, and metadata header + dependency/extra/entry-point payload parsing lowers via `molt_importlib_metadata_payload` (requires `fs.read`). Remaining gaps are advanced metadata/version semantics. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata parity) |
| queue | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Intrinsic-backed `Queue`/`SimpleQueue` core operations plus intrinsic-backed `LifoQueue` and `PriorityQueue` constructors/ordering (`molt_queue_lifo_new`, `molt_queue_priority_new`) are landed; remaining parity is edge-case/API alignment (task accounting corner cases, comparator/error-path fidelity, and broader behavioral coverage). (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete queue edge-case/API parity) |
| shlex | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned `quote`/`split`/`join` and intrinsic-backed lexer tokenization (`shlex.shlex` delegates tokenization to `molt_shlex_split_ex` with comments/whitespace/punctuation controls); remaining gaps are full CPython parser surface (`sourcehook`, `wordchars`/escape fine-grain parity, stream incremental semantics). (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining shlex parser/state parity.) |
| shutil | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read`/`fs.write` gating. |
| textwrap | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Intrinsic-backed `TextWrapper.wrap`/`TextWrapper.fill` and `indent` via `molt_textwrap_wrap`/`molt_textwrap_fill`/`molt_textwrap_indent`; full CPython `TextWrapper` option surface (drop_whitespace/expand_tabs/initial/subsequent indent/predicate hooks) remains pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining textwrap option/edge-case parity.) |
| time | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | `monotonic`/`perf_counter` + `process_time` + `sleep` + `get_clock_info`; `time`/`time_ns` gated by `time.wall` (or legacy `time`); `localtime`/`gmtime`/`strftime` + `struct_time` + `asctime`/`ctime` + `timezone`/`daylight`/`altzone`/`tzname` + `mktime` + `timegm` wired to runtime intrinsics. WASM localtime/timezone currently uses UTC (TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale on wasm hosts). (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): deterministic clock policy) |
| tomllib | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): tomllib pending parity) |
| platform | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; host info gated. |
| ctypes | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Minimal `c_int`/`c_uint`, `Structure`, `pointer`, `sizeof`, and array multiplier; `ffi.unsafe` gating. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand ctypes surface + data model parity.) |
| cgi | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; form parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): cgi pending parity) |
| html | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; escape helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html pending parity) |
| html.parser | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parser parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html.parser pending parity) |
| ipaddress | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; IPv4/IPv6 parsing pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): ipaddress pending parity) |
| mimetypes | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; filesystem lookups require `fs.read`. |
| socketserver | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned dispatch/serve lanes via `molt_socketserver_*` intrinsics (`register`/`unregister`, request queue dispatch begin/poll/cancel, response set, `serve_forever`, `shutdown`); Python shim is focused on handler class shaping and in-memory transport adaptation. Remaining: full CPython server mixin/subclass surface and edge lifecycle parity. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close remaining socketserver class/lifecycle parity gaps.) |
| wsgiref | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; WSGI server stack requires `net`. |
| xml | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parser/tree APIs pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): xml pending parity) |
| warnings | Stdlib | Partial | P3 | SL3 | stdlib | `warn` + filters/once/capture shim; regex filters + full registry pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): warnings pending parity) |
| traceback | Stdlib | Partial | P3 | SL3 | stdlib | `format_exc`/`format_tb`/`format_list`/`format_stack`/`print_exception`/`print_list`/`print_stack`, `extract_tb`/`extract_stack`, `StackSummary.extract`/`StackSummary.from_list`, and `TracebackException.from_exception`; exception chain formatting (`__cause__`/`__context__`/`__suppress_context__`) is runtime-lowered via `molt_traceback_format_exception`, extraction routes through `molt_traceback_extract_tb`/`molt_traceback_payload`, suppress-context probing lowers through `molt_traceback_exception_suppress_context`, stack frame entry retrieval lowers through `molt_getframe`, and `TracebackException.from_exception` chain extraction is runtime-lowered via `molt_traceback_exception_chain_payload`. Best-effort frames (CPython traceback objects or Molt payload tuples); full parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): traceback pending parity) |
| types | Stdlib | Partial | P3 | SL3 | stdlib | Type objects for module/function/code/frame/traceback + generator/coroutine/asyncgen, `SimpleNamespace` (repr/equality parity), `MappingProxyType`, `MethodType`, `coroutine`, `NoneType`/`EllipsisType`, descriptor/helper types (`BuiltinFunctionType`, `CellType`, etc), `CapsuleType`, intrinsic-backed `new_class`/`prepare_class`/`resolve_bases`/`get_original_bases`, and a dedicated `DynamicClassAttribute` descriptor implementation; remaining gaps are broader helper/introspection parity. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): types pending parity) |
| inspect | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Rust-intrinsic-backed `cleandoc`/`currentframe`/`getdoc`, generator+asyncgen+coroutine state helpers, and predicate helpers (`isfunction`/`isclass`/`ismodule`/`iscoroutine`/`iscoroutinefunction`/`isawaitable`/generator-coroutine classifiers); `signature` object shaping still uses Python metadata glue and broader introspection parity remains pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): inspect pending parity) |
| tempfile | Capability-gated | Partial | P3 | SL3 | stdlib | `gettempdir`/`gettempdirb`, `mkdtemp`, `TemporaryDirectory`, and `NamedTemporaryFile` (basic delete-on-close); full tempfile parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): tempfile parity) |
| glob | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Intrinsic-backed `glob`/`iglob`/`has_magic` via `molt_glob` + `molt_glob_has_magic` (`fs.read` gated for filesystem expansion); full CPython parity (`root_dir`, `recursive`/`**` nuances, `include_hidden`) remains pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining glob option/edge-case parity.) |
| fnmatch | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Runtime-owned wildcard engine via `molt_fnmatch`/`molt_fnmatchcase` with intrinsic-backed `filter` and `translate`; supports `*`, `?`, bracket classes, class negation, and range matching. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining CPython bytes/normcase/cache parity details.) |
| copy | Stdlib | Partial | P3 | SL3 | stdlib | Shallow/deep copy helpers + `__copy__`/`__deepcopy__` hooks, dispatch table support, reduce-based reconstruction, and slot copying. |
| pprint | Stdlib | Supported | P3 | SL3 | stdlib | CPython-style PrettyPrinter (`pprint`/`pformat`/`saferepr` + width/indent/compact/sort_dicts parity). |
| string | Stdlib | Partial | P3 | SL3 | stdlib | ASCII constants + `capwords` + `Template` + `Formatter`; locale/Template pattern customization pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining string parity gaps.) |
| numbers | Stdlib | Planned | P3 | SL3 | stdlib | ABCs + numeric tower registration pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): numbers pending parity) |
| unicodedata | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Unicode database helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): unicodedata pending parity) |

### 3.0b Additional stdlib modules (3.12+ coverage)
This list fills out CPython 3.12+ stdlib modules not yet tracked above.
Default status is Planned and import-only stubs unless noted; capability-gated
entries must obey the policy in Section 0.

#### 3.0b.1 Stdlib (planned/import-only)
| Module | Tier | Status | Priority | Milestone | Owner | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| annotationlib | Stdlib | Planned | P3 | SL3 | stdlib | 3.14+; annotation helpers; import-only stub. |
| cProfile | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Profiling hooks; runtime integration pending. |
| calendar | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| cmath | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Math intrinsics; parity pending. |
| codecs | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Intrinsic encode/decode for bytes-like/str; registry/lookup + minimal encodings/aliases now available; incremental/stream codecs + error-handler registration still pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement incremental/stream codecs, full encodings import hooks, and error-handler registration.) |
| codeop | Stdlib | Planned | P3 | SL3 | stdlib | Compilation helpers; parity pending. |
| colorsys | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| compression | Stdlib | Planned | P3 | SL3 | stdlib | 3.14+; import-only allowlist stub. |
| configparser | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| copyreg | Stdlib | Partial | P3 | SL3 | stdlib | Intrinsic-backed pickle registry core (`dispatch_table`, `pickle`, `constructor`, extension registry helpers) with runtime-owned state; full parity pending. |
| difflib | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| dis | Stdlib | Planned | P3 | SL3 | stdlib | Bytecode disassembly; parity pending. |
| encodings | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Minimal package + aliases present; encoding package import hooks pending. |
| faulthandler | Stdlib | Planned | P3 | SL3 | runtime | Runtime hooks pending; import-only stub. |
| graphlib | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| marshal | Stdlib | Planned | P3 | SL3 | runtime | Marshal format parity pending. |
| opcode | Stdlib | Planned | P3 | SL3 | stdlib | Opcode tables; import-only stub. |
| optparse | Stdlib | Planned | P3 | SL3 | stdlib | Legacy CLI parser; parity pending. |
| pickletools | Stdlib | Planned | P3 | SL3 | stdlib | Pickle analysis helpers; import-only stub. |
| profile | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Profiling hooks; parity pending. |
| quopri | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| reprlib | Stdlib | Supported | P3 | SL3 | stdlib | `Repr`, `repr`, and `recursive_repr` parity. |
| sched | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| sre_compile | Stdlib | Planned | P3 | SL3 | stdlib | Internal regex compiler; import-only stub. |
| sre_constants | Stdlib | Planned | P3 | SL3 | stdlib | Internal regex constants; import-only stub. |
| sre_parse | Stdlib | Planned | P3 | SL3 | stdlib | Internal regex parser; import-only stub. |
| stringprep | Stdlib | Planned | P3 | SL3 | stdlib | Unicode stringprep tables; import-only stub. |
| symtable | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Symbol table introspection; parity pending. |
| this | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub. |
| timeit | Stdlib | Planned | P3 | SL3 | stdlib | Timing helpers; parity pending. |
| token | Stdlib | Planned | P3 | SL3 | stdlib | Token constants; import-only stub. |
| tokenize | Stdlib | Planned | P3 | SL3 | stdlib | Tokenizer helpers; parity pending. |
| tracemalloc | Stdlib | Planned | P3 | SL3 | runtime | Runtime hooks pending; import-only stub. |
| pyexpat | Stdlib | Planned | P3 | SL3 | runtime | Expat bindings pending. |

#### 3.0b.2 Capability-gated (planned/import-only)
| Module | Tier | Status | Priority | Milestone | Owner | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| antigravity | Capability-gated | Planned | P3 | SL3 | stdlib | Launches browser; `process.spawn`/`net` gating. |
| bdb | Capability-gated | Planned | P3 | SL3 | stdlib | Debugger base; `fs.read`/`tty` gating. |
| cmd | Capability-gated | Planned | P3 | SL3 | stdlib | CLI loop; `tty` gating. |
| code | Capability-gated | Planned | P3 | SL3 | stdlib | Interactive console; `tty` gating. |
| compileall | Capability-gated | Partial | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating; compile_file/dir/path only, no pyc output. |
| curses | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `tty` gating. |
| dbm | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | `fs.read`/`fs.write` gating. |
| ensurepip | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` + `process.spawn` gating. |
| fcntl | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `fs.read` gating. |
| filecmp | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| fileinput | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| ftplib | Capability-gated | Planned | P3 | SL3 | stdlib | `net` gating. |
| genericpath | Capability-gated | Planned | P3 | SL3 | stdlib | Path helpers; `fs.read` gating. |
| getpass | Capability-gated | Planned | P3 | SL3 | stdlib | `tty` gating. |
| gettext | Stdlib | Partial | P3 | SL3 | stdlib/runtime | Intrinsic-backed `gettext`/`ngettext` core lane (identity translation + plural selection). Remaining: translation catalog loading, domains, and filesystem-backed localization features. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement full gettext translation catalog/domain parity.) |
| grp | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `env.read` gating. |
| idlelib | Capability-gated | Planned | P3 | SL3 | stdlib | GUI; Tk required; `process.spawn` gating. |
| imaplib | Capability-gated | Planned | P3 | SL3 | stdlib | `net` gating. |
| linecache | Capability-gated | Supported | P3 | SL3 | stdlib/runtime | `fs.read` gating; loader `get_source` lazy-cache lookups lower through `molt_linecache_loader_get_source` for deterministic runtime-owned error mapping (`ImportError`/`OSError` -> source unavailable). |
| mailbox | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating. |
| mmap | Capability-gated | Planned | P3 | SL3 | runtime | `fs.read`/`fs.write` gating. |
| modulefinder | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| msvcrt | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (windows). |
| netrc | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| nt | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (windows). |
| ntpath | Capability-gated | Planned | P3 | SL3 | stdlib | Platform-specific (windows); `fs.read` gating. |
| nturl2path | Capability-gated | Planned | P3 | SL3 | stdlib | Platform-specific (windows). |
| pdb | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`tty` gating. |
| pkgutil | Capability-gated | Partial | P3 | SL3 | stdlib | `fs.read` gating; filesystem `iter_modules` + `walk_packages` only. |
| plistlib | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating. |
| poplib | Capability-gated | Planned | P3 | SL3 | stdlib | `net` gating. |
| posix | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix). |
| posixpath | Capability-gated | Planned | P3 | SL3 | stdlib | Platform-specific (posix); `fs.read` gating. |
| pstats | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating. |
| pty | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `tty` gating. |
| pwd | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `env.read` gating. |
| py_compile | Capability-gated | Partial | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating; creates empty .pyc placeholder only. |
| pyclbr | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| pydoc | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`tty` gating. |
| pydoc_data | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| readline | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific; `tty` gating. |
| resource | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix). |
| rlcompleter | Capability-gated | Planned | P3 | SL3 | stdlib | `tty` gating. |
| runpy | Capability-gated | Partial | P3 | SL3 | stdlib | `fs.read` gating with runtime-lowered path resolution (`molt_runpy_resolve_path`) and intrinsic-backed `run_path` execution (`molt_runpy_run_path`); `run_module(alter_sys=True)` and full code-object/package execution parity still pending. |
| shelve | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating. |
| smtplib | Capability-gated | Planned | P3 | SL3 | stdlib | `net` gating. |
| syslog | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix). |
| tabnanny | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| termios | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `tty` gating. |
| tkinter | Capability-gated | Planned | P3 | SL3 | stdlib | GUI; Tk required; `process.spawn` gating. |
| trace | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating. |
| tty | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `tty` gating. |
| turtle | Capability-gated | Planned | P3 | SL3 | stdlib | GUI; Tk required; `process.spawn` gating. |
| turtledemo | Capability-gated | Planned | P3 | SL3 | stdlib | GUI; Tk required; `process.spawn` gating. |
| venv | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` + `process.spawn` gating. |
| wave | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating. |
| webbrowser | Capability-gated | Planned | P3 | SL3 | stdlib | `process.spawn`/`net` gating. |
| winreg | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (windows). |
| winsound | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (windows). |
| xmlrpc | Capability-gated | Planned | P3 | SL3 | stdlib | `net` gating. |
| zipapp | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read`/`fs.write` gating. |
| zipimport | Capability-gated | Partial | P3 | SL3 | stdlib | Minimal zipimporter for store/deflate+zip64 zipfile entries; `fs.read` gated. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement zipimporter bytecode/cache parity + broader archive support.) |

### 3.1 Ten-Item Stdlib Parity Plan (Fully Specced)
Scope is based on the compatibility matrix and current stubs in `src/molt/stdlib/`.

#### 3.1.1 functools (Core-adjacent)
- Goal: Provide high-performance `lru_cache`, `partial`, `reduce` with deterministic behavior; add compiler/runtime fast paths.
- Scope: `functools.lru_cache`, `functools.partial`, `functools.reduce`, `functools.cmp_to_key`, `functools.total_ordering`; public API parity for basic usage (no weakref, no C ext).
- Semantics: Deterministic eviction (LRU), strict argument normalization; `partial` preserves func/args/keywords. `reduce` matches CPython edge cases (empty input, initializer).
- Runtime/IR: Add intrinsics for cache key hashing and lookup. Cache entries use deterministic hash policy. Lower `@lru_cache` at compile time into cache wrapper.
- Frontend: Decorator whitelist update and lowering in `src/molt/frontend/__init__.py`.
- Tests: Unit tests for argument order, eviction, `cache_info`, `cache_clear`; differential tests for `reduce` behavior.
- Docs: Update stdlib matrix: `functools` to Partial/SL1; note deterministic hashing policy.
- Acceptance: 95% parity for covered APIs, no nondeterministic behavior; all tests green.

#### 3.1.2 itertools (Core-adjacent)
- Goal: Implement core iterator kernels and tie into vectorization where possible.
- Scope: `chain`, `islice`, `repeat`, `count`, `cycle`, `accumulate`, `pairwise`, `product`, `permutations`, `combinations`, `groupby`, `tee`.
- Semantics: Iterator protocol compliance; no eager materialization; deterministic iteration order identical to CPython.
- Runtime/IR: Add iterator ops for product/permutations and shared iteration state; optimize repeat/islice to avoid allocations.
- Frontend: Recognize common patterns and lower to specialized ops when safe.
- Tests: Differential tests for each iterator; edge cases (empty input, step, large lengths).
- Docs: Update stdlib matrix to Partial/SL1 and add usage note.
- Acceptance: Parity for covered APIs, memory behavior within 10% of CPython on small inputs.

#### 3.1.3 operator (Core-adjacent)
- Goal: Provide low-overhead operator helpers.
- Scope: `itemgetter`, `attrgetter`, `methodcaller`, plus basic `add`, `mul`, `eq` wrappers.
- Semantics: Attribute resolution uses Molt rules but mirrors CPython error messages for missing attributes/keys.
- Runtime/IR: Implement `GETATTR_FAST`, `GETITEM_FAST` ops for getter functions; cache attribute lookups when safe.
- Tests: Differential tests for attribute/key errors and tuple/multi-getter behavior.
- Docs: Matrix update: `operator` Partial/SL1.
- Acceptance: No semantic regressions; simple getters 2x faster than Python fallback.

#### 3.1.4 math (Core-adjacent)
- Goal: Deterministic numeric intrinsics with CPython edge-case behavior.
- Scope: `sqrt`, `sin`, `cos`, `log`, `exp`, `floor`, `ceil`, `isfinite`, `isnan`, `isinf`, `prod`, `sum` fast paths.
- Semantics: IEEE-754 behavior; handle NaN/inf exactly like CPython where possible.
- Runtime/IR: Use Rust libm or platform intrinsics; guard on float types; integer fast paths.
- Tests: Differential tests for edge inputs; property tests for invariants (e.g., `isfinite`).
- Docs: Matrix update: `math` Partial/SL1; note floating point determinism policy.
- Acceptance: Parity on core functions; no UB; stable outputs across runs.

#### 3.1.5 collections (Stdlib)
- Goal: Implement `deque`, `Counter`, `defaultdict`.
- Scope: Basic operations and methods; omit advanced views initially.
- Semantics: `deque` append/pop/iter plus rotate/index/insert/remove; `Counter` arithmetic/in-place ops + `most_common`/`total` + keys/values/items + clear/pop/popitem/setdefault; `defaultdict` default factories.
- Runtime/IR: Current shim uses list-backed `deque` + dict-subclass parity for `Counter`/`defaultdict`.
- Tests: Unit + differential tests; deterministic iteration and repr.
- Docs: Matrix update: `collections` Partial/SL1.
- Acceptance: Behavior parity for common methods; deterministic iteration.

#### 3.1.6 heapq (Stdlib)
- Goal: Provide heap operations used widely in algorithms.
- Scope: `heapify`, `heappush`, `heappop`, `heapreplace`, `heappushpop`, `nlargest`, `nsmallest`, `merge`, max-heap helpers.
- Semantics: Ordering matches CPython; stable for equal elements when applicable.
- Runtime/IR: Use list-backed binary heap; expose efficient ops to frontend.
- Tests: Differential tests with random inputs; heap invariants.
- Docs: Matrix update: `heapq` Partial/SL1.
- Acceptance: Correctness on randomized stress, no regressions in benchmarks.
- Status: Python shim + merge/max-heap helpers + runtime fast paths landed.

#### 3.1.7 bisect (Stdlib)
- Goal: Provide deterministic binary search helpers.
- Scope: `bisect_left`, `bisect_right`, `insort_left`, `insort_right`.
- Semantics: Comparable to CPython for list-like sequences; stable insertion.
- Runtime/IR: Optional fast path on list/array types.
- Tests: Differential tests across sorted inputs, edge cases with duplicates.
- Docs: Matrix update: `bisect` Partial/SL1.
- Acceptance: Parity for covered functions; perf within 1.5x CPython.
- Status: Python shim + key support landed; runtime fast paths pending (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): bisect runtime fast paths).

#### 3.1.8 array (Stdlib)
- Goal: Typed array storage with deterministic layout and buffer interop.
- Scope: `array('b','i','f','d', ...)`, basic operations, slicing, `.tobytes()`.
- Semantics: Endianness and item size consistent with CPython for supported types.
- Runtime/IR: Add array object to runtime with explicit layout; implement buffer protocol for interop (future) (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): array runtime layout + buffer protocol).
- Tests: Unit + differential; pack/unpack round-trip; size/endianness.
- Docs: Matrix update: `array` Partial/SL1; note supported typecodes.
- Acceptance: Parity for supported types; deterministic bytes output.

#### 3.1.9 struct (Stdlib)
- Goal: Binary pack/unpack for codecs/I/O.
- Scope: `pack`, `unpack`, `calcsize` with `i`/`d` (+ `x` padding) and byte order (`@`, `=`, `<`, `>`, `!`). Full format table pending.
- Semantics: Alignment and CPython-grade error strings pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): implement full struct format/alignment parity.)
- Runtime/IR: Rust parser + packer exposed to Python; extend to full format table and alignment rules.
- Tests: Differential tests for formats; fuzz small formats; exact byte matches.
- Docs: Matrix update: `struct` Partial/SL1; supported format list.
- Acceptance: Parity on supported formats; deterministic output.

#### 3.1.10 re (Stdlib)
- Goal: Deterministic regex engine with core CPython semantics.
- Scope: `compile`, `match`, `search`, `findall`, `sub`, `split` and flags `IGNORECASE`, `MULTILINE`, `DOTALL`.
- Semantics: No backtracking nondeterminism; deterministic error handling.
- Runtime/IR: Use Rust regex crate or custom engine with explicit semantics; ensure Python-compatible groups.
- Tests: Differential tests for common patterns; coverage for groups/flags; backtracking stress tests.
- Docs: Matrix update: `re` Partial/SL2; note unsupported features (TODO(docs, owner:docs, milestone:SL2, priority:P3, status:planned): document unsupported re features).
- Acceptance: Parity for supported features; deterministic runtime.

### 3.2 Cross-cutting Requirements
- Update `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` and `docs/ROADMAP.md` as each module lands.
- TODO(docs, owner:docs, milestone:SL1, priority:P3, status:planned): add `TODO(stdlib-compat, ...)` markers for interim gaps.
- Ensure capability gating for I/O-adjacent pieces (if introduced).
- Add differential tests in `tests/differential/` as required.

## 4. Milestones
- **SL1:** core-adjacent and data-structure modules (functools/itertools/operator/math + collections/heapq/bisect/array/struct).
- **SL2:** regex + time + numeric libs + logging + json + hashing.
- **SL3:** I/O/network/system modules with capability gating and stronger sandboxing.

## 5. Capability Tokens (appendix)
Modules that touch the host require explicit capabilities. Tokens are additive and must be declared by the binary.

| Capability | Scope | Example modules |
| --- | --- | --- |
| `fs.read` | Read-only filesystem access | `io`, `pathlib`, `os`, `open` |
| `fs.write` | Write filesystem access | `io`, `pathlib`, `os`, `open` |
| `env.read` | Environment variable read | `os`, `sys` |
| `env.write` | Environment variable write | `os` |
| `net.outbound` | Outbound network access | `socket`, `http`, `urllib`, `ssl` |
| `net.listen` | Bind/listen access | `socket`, `asyncio` |
| `net.poll` | Network readiness/polling | `select`, `selectors`, `asyncio` |
| `proc.spawn` | Process creation | `subprocess`, `multiprocessing` |
| `time.wall` | Real time access | `datetime`, `time` |
| `rand.secure` | Cryptographic RNG | `secrets`, `ssl` |
| `ffi.unsafe` | Native extension/FFI | `ctypes` |

## 6. TODOs (tracked in ROADMAP.md)
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `functools` fast paths (`lru_cache`, `partial`, `reduce`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `itertools` + `operator` core-adjacent intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill out remaining `math` intrinsics (determinism policy).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` runtime `deque` type + O(1) ops + view parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `heapq` randomized stress + perf tracking.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `bisect` fast paths on list/array types.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): `array` deterministic layout + buffer protocol.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `struct` alignment + full format table parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `re` engine + deterministic regex semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): finish `json` parity plan (performance tuning + full cls/callback parity) and add a runtime fast-path parser for dynamic strings.
- TODO(stdlib-compat, owner:frontend, milestone:SL1, priority:P2, status:planned): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:partial): finish `io` parity (codec coverage, Windows isatty).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:missing): full `open`/file object parity (modes/buffering/text/encoding/newline/fileno/seek/tell/iter/context manager) with differential + wasm coverage.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand `asyncio` shim to full loop/task APIs (task groups, wait, shields) and I/O adapters.

## 7. Matrix Audit (2026-01-16)
Coverage evidence (selected):
- `tests/differential/stdlib/heapq_basic.py`, `tests/differential/stdlib/heapq_more.py` (heapq core + merge/max-heap helpers).
- `tests/differential/basic/bisect_basic.py` (bisect/insort + key support).
- `tests/differential/stdlib/itertools_core.py` (core itertools iterators and combinatorics).
- `tests/differential/basic/collections_basic.py`, `tests/differential/stdlib/collections_deque.py` (collections shims + deque core).
- `tests/differential/stdlib/functools_more.py` (cmp_to_key + total_ordering parity).
- `tests/differential/stdlib/operator_basic.py` (itemgetter/attrgetter/methodcaller).
- `tests/differential/stdlib/fnmatch_basic.py` (fnmatch core patterns).

Gaps or missing coverage (audit findings):
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): add wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
