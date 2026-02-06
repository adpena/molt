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
| struct | Stdlib | Partial | P1 | SL1 | runtime | Runtime intrinsics cover full format table (including half-float) with endianness + alignment; intrinsics are required (no host fallback). Remaining: buffer protocol beyond bytes/bytearray and deterministic layout policy. (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): complete struct parity for buffer protocol + deterministic layout.) |
| re | Stdlib | Partial | P1 | SL2 | stdlib | Native engine supports literals, `.`, char classes/ranges (`\\d`/`\\w`/`\\s`), groups/alternation, greedy + non-greedy quantifiers, and `IGNORECASE`/`MULTILINE`/`DOTALL`; advanced features/flags raise `NotImplementedError` (no host fallback). Full parity pending. |
| decimal | Stdlib | Partial | P2 | SL2 | stdlib | libmpdec-backed intrinsics: constructor + context (prec/rounding/traps/flags), `as_tuple`, `str`/`repr`/float, quantize/compare/compare_total/normalize/exp/div; remaining: full arithmetic (add/sub/mul/pow/sqrt/log), formatting helpers (`to_eng_string`), NaN payloads + edge-case signaling parity. |
| fractions | Stdlib | Planned | P2 | SL2 | stdlib | Rational arithmetic. |
| statistics | Stdlib | Planned | P2 | SL2 | stdlib | Basic stats kernels. |
| random | Stdlib | Partial | P2 | SL2 | stdlib | Deterministic Mersenne Twister parity with `Random`/`seed`/`getstate`/`setstate`, `randrange`/`randint`/`shuffle`, `choice`/`choices`/`sample`, `randbytes`, `SystemRandom` (via `os.urandom`), and distribution methods (`uniform`, `triangular`, `normalvariate`, `gauss`, `lognormvariate`, `expovariate`, `vonmisesvariate`, `gammavariate`, `betavariate`, `paretovariate`, `weibullvariate`, `binomialvariate`). Remaining: extended test vectors. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand random distribution test vectors) |
| datetime | Stdlib | Planned | P2 | SL2 | stdlib | Time types + parsing. |
| zoneinfo | Stdlib | Planned | P3 | SL3 | stdlib | Timezone data handling. |
| pathlib | Stdlib | Partial | P2 | SL2 | stdlib | Basic `Path` wrapper with gated `open`/read/write/exists/unlink/iterdir plus `mkdir`/`rmdir`, `glob` (simple name patterns), `parts`/`parents`, `name`/`suffix`/`suffixes`/`stem`, `joinpath`, `with_name`, `with_suffix`, `relative_to`, `match` (basic patterns), `as_posix`, `expanduser`, `is_absolute`, `resolve` (abspath-only), and `PurePosixPath` mapped to `Path` with ordering/comparisons; richer Path ops pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): pathlib glob parity (recursive/segment patterns), resolve symlink/strict semantics, and full Path parity) |
| enum | Stdlib | Partial | P2 | SL2 | stdlib | Enum/IntEnum/Flag/IntFlag base types with `auto`, name/value access, and member maps; aliasing, functional API, and full Flag semantics pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): finish Enum/Flag/IntFlag parity.) |
| dataclasses | Stdlib | Partial | P2 | SL2 | stdlib | Dataclass lowering covers init/repr/eq/order/unsafe_hash/frozen/slots/match_args/kw_only, Field flags, InitVar/ClassVar/KW_ONLY, __match_args__, and stdlib helpers. make_dataclass + non-dataclass base inheritance pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): implement make_dataclass once dynamic class construction is allowed.) (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.) |
| typing | Stdlib | Supported | P2 | SL3 | stdlib | Deterministic runtime typing helpers: `Annotated`/`Literal`/`Union`/`Optional`/`Callable`, `TypeVar`/`ParamSpec`/`TypeVarTuple`, `NewType`/`TypedDict`, `Protocol` + `@runtime_checkable`, `get_origin`/`get_args`/`get_type_hints` (explicit eval only). |
| abc | Stdlib | Partial | P3 | SL3 | stdlib | Minimal `ABCMeta`/`ABC` + `abstractmethod` with instantiation guard; registration/caching pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): finish abc registry + cache invalidation parity.) |
| _abc | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; minimal cache helpers for CPython `abc`. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement full `_abc` cache/proxy parity.) |
| _py_abc | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; Python fallback for `abc`. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): implement `_py_abc` parity for abc fallback.) |
| contextlib | Stdlib | Partial | P2 | SL2 | stdlib | `contextmanager`/`ContextDecorator` + `ExitStack`/`AsyncExitStack`, `suppress`, `redirect_stdout`/`redirect_stderr`, `nullcontext`, `closing` implemented; `aclosing`/`AbstractContextManager`/full parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): contextlib pending parity) |
| contextvars | Stdlib | Partial | P2 | SL3 | stdlib/runtime | `ContextVar`/`Token`/`Context` + `copy_context`; task context propagation via cancel tokens; `Context.run` implemented. |
| gc | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Minimal `collect`/enable/disable shim for test support; cycle collector wiring + full API pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full gc module API + runtime cycle collector hook.) |
| weakref | Stdlib | Partial | P3 | SL3 | stdlib | Runtime-backed weakrefs + proxies + WeakKey/ValueDictionary + WeakSet + WeakMethod + finalize + getweakrefcount/refs; finalize atexit registry pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): wire finalize atexit registry once atexit hooks exist.) |
| _weakref | Stdlib | Supported | P3 | SL3 | stdlib | `_weakref` parity: exports ref/proxy types + weakref counts/refs. |
| _weakrefset | Stdlib | Supported | P3 | SL3 | stdlib | `_weakrefset.WeakSet` parity (runtime-backed weak semantics). |
| _intrinsics | Stdlib | Supported | P3 | SL3 | stdlib | Intrinsic loader used by stdlib modules to bind runtime helpers. |
| logging | Stdlib | Partial | P2 | SL2 | stdlib | Deterministic logging core (Logger/Handler/Formatter/LogRecord + Stream/File/Null handlers + basicConfig + `captureWarnings`); sinks gated by `fs.write`. |
| logging.config | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; config parsing pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.config pending parity) |
| logging.handlers | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; handler wiring pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.handlers pending parity) |
| json | Stdlib | Partial | P1 | SL2 | stdlib | Shim supports `loads`/`dumps`/`load`/`dump` with parse hooks, indent, separators, and `allow_nan`, plus `JSONEncoder`/`JSONDecoder` and `JSONDecodeError` details; runtime fast-path + full parity pending. |
| csv | Stdlib | Planned | P3 | SL3 | stdlib | Deterministic CSV parsing. |
| io | Capability-gated | Partial | P2 | SL3 | stdlib | Native `open` supports full signature + fd-based open; IOBase hierarchy (IOBase/RawIOBase/BufferedIOBase/TextIOBase) plus file objects expose core methods/attrs (read/read1/readall/readinto/readinto1/write/writelines/seek/tell/fileno/readline(s)/truncate/iteration/flush/close, newline/newlines/encoding/errors/line_buffering/write_through, `buffer` on text wrappers, `closefd` on raw handles). `io.UnsupportedOperation` exported; BytesIO/StringIO available. utf-8/utf-8-sig/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/ascii/latin-1/utf-16/utf-32 text encoding only (encode handlers include namereplace+xmlcharrefreplace), text-mode seek/tell cookies partial, and Windows isatty parity still pending. (TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:partial): io pending parity) |
| os | Capability-gated | Partial | P2 | SL3 | stdlib | Minimal shim: env access gated via `env.read`/`env.write`; path helpers plus `exists`/`unlink`/`remove`/`expandvars`/`close`; `urandom` gated by `rand`. |
| sys | Capability-gated | Partial | P2 | SL3 | stdlib | Minimal shim: argv/version/version_info/path/modules (synced from runtime module cache) + recursion limits; stdio uses runtime intrinsics or fd-based fallback and defaults to NullIO when unavailable; `sys.version` + `sys.version_info` are stamped by the toolchain intrinsic; `sys.exc_info()` reads the active exception context; compiled argv now sourced from runtime; host info gated via `env.read` (argv encoding parity TODO; `sys._getframe` uses partial frame objects). |
| errno | Stdlib | Full | P2 | SL2 | stdlib | Full CPython errno constants + errorcode mapping (native build-time generation; WASM keeps minimal errno set). |
| stat | Stdlib | Planned | P3 | SL3 | stdlib | Stat constants; import-only allowlist stub. |
| signal | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Signal handling; gated by `process.signal`. |
| select | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | `select.select` wired via `selectors` + io_poller for sockets; poll/epoll/kqueue objects and fd-based polling pending; wasmtime host parity implemented, Node/WASI/browser pending. |
| site | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; path config gated via `env.read`/`fs.read`. |
| sysconfig | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; host/path data gated via `env.read`/`fs.read`. |
| subprocess | Capability-gated | Planned | P3 | SL3 | stdlib | Process spawn control. |
| socket | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | Runtime-backed socket API (AF_INET/AF_INET6/AF_UNIX, connect/bind/listen/accept/send/recv, socketpair, shutdown/half-close, sendall, recv_into/peek, getaddrinfo/nameinfo/hostby*/fqdn, dup/fromfd, makefile, inet_pton/ntop, UDP echo/truncation, nonblocking connect SO_ERROR, dualstack, websocket handshakes); advanced options/full constant table, SSL, and Node/WASI/browser parity pending (wasmtime host implemented). |
| ssl | Capability-gated | Planned | P3 | SL3 | stdlib | TLS primitives. |
| asyncio | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | Shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `create_task`/`ensure_future`/`current_task`, `Event`, `wait_for`, and basic `gather`; advanced loop APIs and I/O adapters pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): asyncio pending parity) |
| _asyncio | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; CPython C-accelerated asyncio helpers. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement `_asyncio` parity or runtime hooks.) |
| asyncio.base_events/asyncio.coroutines/asyncio.events/asyncio.exceptions/asyncio.futures/asyncio.locks/asyncio.protocols/asyncio.runners/asyncio.queues/asyncio.streams/asyncio.subprocess/asyncio.tasks/asyncio.taskgroups/asyncio.timeouts/asyncio.threads/asyncio.transports/asyncio.unix_events/asyncio.windows_events | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stubs for asyncio submodules; parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): asyncio submodule parity.) |
| selectors | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | CPython-style selector API backed by io_wait; poller variants map to the same readiness path. Remaining: OS poller parity (poll/epoll/kqueue/devpoll), fd inheritance, and error mapping; Node/WASI/browser parity pending. |
| threading | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Isolate-backed `Thread` by default (spawn/join/is_alive/ident/native_id) with serialized payloads; enable `thread.shared` for shared-runtime globals (still serialized targets/args). Cross-thread shared objects and full sync primitives remain unsupported. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.) |
| multiprocessing | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Spawn-based Process/Pool/Queue/Pipe/SharedValue/SharedArray; `maxtasksperchild` supported; `fork`/`forkserver` map to spawn semantics (TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): implement true fork support). |
| concurrent.futures | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Executor interfaces. |
| sqlite3 | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | DB integration. |
| http | Capability-gated | Planned | P3 | SL3 | stdlib | HTTP parsing/client. |
| http.client | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; HTTP client sockets pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): http.client pending parity) |
| http.server | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; HTTP server stack pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): http.server pending parity) |
| http.cookies | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; cookie parsing/quoting pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): http.cookies pending parity) |
| urllib | Capability-gated | Partial | P3 | SL3 | stdlib | Package surface exposes `urllib.parse`; request/error/robotparser pending. |
| urllib.parse | Stdlib | Partial | P3 | SL3 | stdlib | Core URL parsing (`urlparse`/`urlsplit`/`urljoin`/`urldefrag`) + `quote`/`unquote` + `urlencode`; encoding/idna/params parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): urllib.parse parity gaps.) |
| urllib.request | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; URL fetching requires `net`. |
| urllib.error | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; error types pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): urllib.error pending parity) |
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
| hashlib/hmac | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Rust intrinsics for guaranteed algorithms (md5/sha1/sha2/sha3/shake/blake2) + `pbkdf2_hmac`/`scrypt` + `compare_digest`; optional OpenSSL algorithms (sha512_224/sha512_256, ripemd160, md4) are not yet implemented (no host fallback). (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement optional OpenSSL algorithms + parity tests for advanced digestmod usage.) |
| secrets | Stdlib | Planned | P3 | SL3 | stdlib | Crypto RNG (gated). |
| uuid | Stdlib | Full | P2 | SL2 | stdlib | `UUID` surface with fields/bytes_le/variant/urn accessors, `uuid1`/`uuid3`/`uuid4`/`uuid5`, namespace constants, and `SafeUUID` (uuid1 requires `time.wall`). |
| base64 | Stdlib | Full | P2 | SL2 | stdlib | b16/b32/b32hex/b64/b85/a85/z85 encode/decode, urlsafe variants, and legacy encode/decode helpers. |
| binascii | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; low-level codecs pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): binascii pending parity) |
| pickle | Stdlib | Partial | P2 | SL2 | stdlib | Protocol 0 only (basic builtins + slice). (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement protocol 1+, bytes/bytearray, memo cycles, and broader type coverage) |
| unittest | Stdlib | Partial | P3 | SL3 | stdlib | Minimal TestCase/runner stubs for CPython regrtest; full assertions/loader parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest pending parity) |
| doctest | Stdlib | Partial | P3 | SL3 | stdlib | Stub that rejects eval/exec/compile usage for now. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): doctest pending parity once eval/exec/compile are gated.) |
| test | Stdlib | Partial | P3 | SL3 | stdlib | Minimal `test.support` helpers (`captured_output`/`captured_stdout`/`captured_stderr`, `check_syntax_error`, `findfile`, `run_with_tz`, `warnings_helper` utilities: `check_warnings`/`check_no_warnings`/`check_no_resource_warning`/`check_syntax_warning`/`ignore_warnings`/`import_deprecated`/`save_restore_warnings_filters`/`WarningsRecorder`, `cpython_only`, `requires`, `swap_attr`/`swap_item`, `import_helper` basics: `import_module`/`import_fresh_module`/`make_legacy_pyc`/`ready_to_import`/`frozen_modules`/`multi_interp_extensions_check`/`DirsOnSysPath`/`isolated_modules`/`modules_setup`/`modules_cleanup`, `os_helper` basics: `temp_dir`/`temp_cwd`/`unlink`/`rmtree`/`rmdir`/`make_bad_fd`/`can_symlink`/`skip_unless_symlink` + TESTFN constants); full CPython harness pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): test package pending parity) |
| argparse | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): argparse pending parity) |
| getopt | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): getopt pending parity) |
| locale | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; locale data pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): locale pending parity) |
| ast | Stdlib | Partial | P3 | SL3 | stdlib | Minimal `parse`/`walk`/`get_docstring`; full node classes + visitor helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): ast parity gaps.) |
| atexit | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; exit handler semantics pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): atexit pending parity) |
| collections.abc | Stdlib | Full | P3 | SL3 | stdlib | Full ABCs with structural checks, registrations, and mixin helpers (Set/Mapping/Sequence, async ABCs, Callable generics). |
| _collections_abc | Stdlib | Full | P3 | SL3 | stdlib | Full collections ABC implementation (re-exported by `collections.abc`). |
| importlib | Stdlib | Partial | P2 | SL3 | stdlib | File-based module loading + `import_module`/`reload`/`find_spec` for sys.path; meta_path/path_hooks/namespace packages pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib meta_path/namespace/extension loader parity) |
| importlib.util | Stdlib | Partial | P2 | SL3 | stdlib | `find_spec`, `module_from_spec`, `spec_from_file_location` for filesystem sources; meta_path/zip/extension loaders pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.util parity for non-filesystem loaders) |
| importlib.machinery | Stdlib | Partial | P2 | SL3 | stdlib | `ModuleSpec` + filesystem `SourceFileLoader`; extension/zip/namespace loaders pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery loader/finder parity) |
| importlib.resources | Capability-gated | Partial | P2 | SL3 | stdlib | Filesystem-backed `files`/`open_text`/`read_binary`/traversable APIs; namespace/zip/loader resource readers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.resources loader/namespace/zip parity) |
| importlib.metadata | Capability-gated | Partial | P2 | SL3 | stdlib | `distribution`/`version`/`entry_points` via dist-info scan (requires `fs.read`); full metadata + dependency semantics pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata parity) |
| queue | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; threading integration pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): queue pending parity) |
| shlex | Stdlib | Partial | P3 | SL3 | stdlib | `shlex.quote` implemented without regex dependency to unblock subprocess shell quoting; full lexer/split parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement shlex lexer/split/parsing parity beyond `quote`.) |
| shutil | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read`/`fs.write` gating. |
| textwrap | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; wrapping/indent parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): textwrap pending parity) |
| time | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | `monotonic`/`perf_counter` + `process_time` + `sleep` + `get_clock_info`; `time`/`time_ns` gated by `time.wall` (or legacy `time`); `localtime`/`gmtime`/`strftime` + `struct_time` + `asctime`/`ctime` + `timezone`/`tzname` wired to runtime intrinsics. WASM localtime/timezone currently uses UTC (TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale on wasm hosts). (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand time module surface + deterministic clock policy) |
| tomllib | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): tomllib pending parity) |
| platform | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; host info gated. |
| ctypes | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Minimal `c_int`/`c_uint`, `Structure`, `pointer`, `sizeof`, and array multiplier; `ffi.unsafe` gating. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand ctypes surface + data model parity.) |
| cgi | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; form parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): cgi pending parity) |
| html | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; escape helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html pending parity) |
| html.parser | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parser parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html.parser pending parity) |
| ipaddress | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; IPv4/IPv6 parsing pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): ipaddress pending parity) |
| mimetypes | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; filesystem lookups require `fs.read`. |
| socketserver | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; server sockets require `net`. |
| wsgiref | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; WSGI server stack requires `net`. |
| xml | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parser/tree APIs pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): xml pending parity) |
| warnings | Stdlib | Partial | P3 | SL3 | stdlib | `warn` + filters/once/capture shim; regex filters + full registry pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): warnings pending parity) |
| traceback | Stdlib | Partial | P3 | SL3 | stdlib | `format_exc`/`format_tb`/`format_list`/`format_stack`/`print_exception`/`print_list`/`print_stack`, `extract_tb`/`extract_stack`, `StackSummary.extract`/`StackSummary.from_list`, and `TracebackException.from_exception` with basic `__cause__`/`__context__` chain formatting. Best-effort frames (CPython traceback objects or Molt (filename, line, name) tuples); full parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): traceback pending parity) |
| types | Stdlib | Partial | P3 | SL3 | stdlib | Type objects for module/function/code/frame/traceback + generator/coroutine/asyncgen, `SimpleNamespace` (repr/equality parity), `MappingProxyType`, `MethodType`, `coroutine`, `NoneType`/`EllipsisType`, descriptor/helper types (`BuiltinFunctionType`, `CellType`, etc), `DynamicClassAttribute`, `CapsuleType`, and `new_class` helpers; remaining helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): types pending parity) |
| inspect | Stdlib | Partial | P3 | SL3 | stdlib | `getdoc`/`cleandoc` + `signature` (Molt metadata/`__code__`); broader introspection pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): inspect pending parity) |
| tempfile | Capability-gated | Partial | P3 | SL3 | stdlib | `gettempdir`/`gettempdirb`, `mkdtemp`, `TemporaryDirectory`, and `NamedTemporaryFile` (basic delete-on-close); full tempfile parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): tempfile parity) |
| glob | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read` gating. |
| fnmatch | Stdlib | Partial | P3 | SL3 | stdlib | `*`/`?` + bracket class/range matching; literal `[]`/`[[]`/`[]]` escapes (no backslash quoting). |
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
| copyreg | Stdlib | Partial | P3 | SL3 | stdlib | Pickle registry helpers (`dispatch_table`, `pickle`, `constructor`, extension registry helpers); full parity pending. |
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
| gettext | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
| grp | Capability-gated | Planned | P3 | SL3 | runtime | Platform-specific (posix); `env.read` gating. |
| idlelib | Capability-gated | Planned | P3 | SL3 | stdlib | GUI; Tk required; `process.spawn` gating. |
| imaplib | Capability-gated | Planned | P3 | SL3 | stdlib | `net` gating. |
| linecache | Capability-gated | Supported | P3 | SL3 | stdlib | `fs.read` gating. |
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
| runpy | Capability-gated | Planned | P3 | SL3 | stdlib | `fs.read` gating. |
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): implement make_dataclass once dynamic class construction is allowed.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand hashlib/hmac coverage for optional OpenSSL algorithms (sha512_224/sha512_256, ripemd160, md4) and add parity tests for advanced digestmod usage.
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
