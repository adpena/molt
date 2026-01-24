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
- **Import allowlist:** modules listed in the matrix are importable; missing implementations load empty stubs for dependency tracking.
- **Import-only stubs:** allowlisted modules may load as empty module objects; attribute access raises `AttributeError` and signals missing coverage rather than crashing the compiler.

## 0.1 Tier-0 Direct-Call Rule
- **Direct-call allowlist:** Tier 0 compiles module-level calls to a static `CALL` only for allowlisted functions from allowlisted modules (this matrix + frontend allowlist).
- **No monkey-patching:** rebinding or monkey-patching allowlisted functions is not observed in Tier 0; the call target is fixed at compile time.
- **Fallbacks:** non-allowlisted module-level calls raise a compiler error unless `--fallback=bridge` is enabled, in which case a bridge warning is emitted.
- **Warnings control:** set `MOLT_COMPAT_WARNINGS=0` to suppress bridge warnings during compilation.

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
| Module | Tier | Status | Priority | Milestone | Owner | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| builtins | Core | Partial | P0 | SL1 | runtime/frontend | Importable module binds supported builtins (including function objects for allowlisted builtins); missing names raise `AttributeError`. |
| __future__ | Stdlib | Supported | P3 | SL3 | stdlib | Feature metadata (`_Feature`, `all_feature_names`, compiler flags) synced to CPython 3.12. |
| functools | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | `partial`, `reduce`, `lru_cache`, `wraps`/`update_wrapper`, `cmp_to_key`, `total_ordering`; `partial`/`lru_cache` accept `*args`/`**kwargs` (no fast paths yet). |
| itertools | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | `chain`, `islice`, `repeat`, `count`, `cycle`, `accumulate`, `pairwise`, `product`, `permutations`, `combinations`, `groupby`, `tee` (product/permutations/combinations are eager; no generators yet). |
| operator | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | Basic helpers (`add`, `mul`, `eq`, `itemgetter`, `attrgetter`, `methodcaller`). |
| math | Core-adjacent | Partial | P1 | SL1 | stdlib/runtime | Minimal constants + `isfinite`/`isnan`/`isinf`; intrinsics pending. |
| collections | Stdlib | Partial | P1 | SL1 | stdlib | `deque` core ops + rotate/index/insert/remove; `Counter`/`defaultdict` dict subclasses with arithmetic, in-place ops, and Counter keys/values/items/total + dict-style clear/pop/popitem/setdefault parity. |
| heapq | Stdlib | Partial | P1 | SL1 | stdlib | `heapify`/`heappush`/`heappop`/`heapreplace`/`heappushpop` + `nlargest`/`nsmallest`, `merge` (eager; full materialization/sort), max-heap helpers, runtime fast paths. |
| bisect | Stdlib | Partial | P1 | SL1 | stdlib | `bisect_left`/`bisect_right` + `insort_left`/`insort_right` with `key` support; aliases `bisect`/`insort`. |
| array | Stdlib | Planned | P1 | SL1 | runtime | Typed array storage, interop-ready. |
| struct | Stdlib | Planned | P1 | SL1 | runtime | Binary packing for codecs/I/O. |
| re | Stdlib | Partial | P1 | SL2 | stdlib | Literal-only `search`/`match`/`fullmatch` implemented; full regex engine pending. |
| decimal | Stdlib | Planned | P2 | SL2 | stdlib | Precise decimal math. |
| fractions | Stdlib | Planned | P2 | SL2 | stdlib | Rational arithmetic. |
| statistics | Stdlib | Planned | P2 | SL2 | stdlib | Basic stats kernels. |
| random | Stdlib | Partial | P2 | SL2 | stdlib | Deterministic `seed` + `randrange`/`shuffle` helpers; full Random API + Mersenne Twister parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement full random API + CPython-compatible algorithm) |
| datetime | Stdlib | Planned | P2 | SL2 | stdlib | Time types + parsing. |
| zoneinfo | Stdlib | Planned | P3 | SL3 | stdlib | Timezone data handling. |
| pathlib | Stdlib | Partial | P2 | SL2 | stdlib | Basic `Path` wrapper with gated `open`/read/write/exists/unlink; `iterdir` + richer Path ops pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): pathlib iterdir + full Path parity) |
| enum | Stdlib | Planned | P2 | SL2 | stdlib | Enum base types. |
| dataclasses | Stdlib | Partial | P2 | SL2 | stdlib | Dataclass lowering (frozen/eq/repr/field order, slots flag, defaults + default_factory); kw-only/order pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): dataclasses pending parity) |
| typing | Stdlib | Partial | P3 | SL3 | stdlib | Minimal shim: `Any`/`Union`/`Optional`/`Callable` + `TypeVar`/`Generic`/`Protocol` + `cast`/`get_origin`/`get_args`. |
| abc | Stdlib | Planned | P3 | SL3 | stdlib | Abstract base classes. |
| contextlib | Stdlib | Partial | P2 | SL2 | stdlib | `nullcontext` + `closing` lowered; `contextmanager` pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): contextlib pending parity) |
| contextvars | Stdlib | Partial | P2 | SL3 | stdlib/runtime | `ContextVar`/`Token`/`Context` + `copy_context`; task context propagation via cancel tokens; `Context.run` implemented. |
| gc | Stdlib | Partial | P2 | SL2 | stdlib/runtime | Minimal `collect`/enable/disable shim for test support; cycle collector wiring + full API pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full gc module API + runtime cycle collector hook.) |
| weakref | Stdlib | Partial | P3 | SL3 | stdlib | Minimal `ref` shim (cleared on `gc.collect`); GC-aware semantics pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement GC-aware weakrefs + full weakref API.) |
| logging | Stdlib | Planned | P2 | SL2 | stdlib | Structured logging; gated sinks. |
| logging.config | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; config parsing pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.config pending parity) |
| logging.handlers | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; handler wiring pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.handlers pending parity) |
| json | Stdlib | Partial | P1 | SL2 | stdlib | Shim supports `loads`/`dumps`/`load`/`dump` with parse hooks, indent, separators, and `allow_nan`; runtime fast-path + full encoder/decoder parity pending. |
| csv | Stdlib | Planned | P3 | SL3 | stdlib | Deterministic CSV parsing. |
| io | Capability-gated | Partial | P2 | SL3 | stdlib | Native `open` supports full signature + fd-based open; file objects expose core methods/attrs (read/readinto/write/writelines/seek/tell/fileno/readline(s)/truncate/iteration/flush/close). `io.UnsupportedOperation` exported. utf-8/ascii/latin-1 text encoding only, text-mode seek/tell cookies partial, and Windows fileno/isatty parity still pending. (TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:partial): io pending parity) |
| os | Capability-gated | Partial | P2 | SL3 | stdlib | Minimal shim: env access gated via `env.read`/`env.write`; path helpers plus `exists`/`unlink`. |
| sys | Capability-gated | Partial | P2 | SL3 | stdlib | Minimal shim: argv/version/path/modules (synced from runtime module cache) + recursion limits; `sys.exc_info()` reads the active exception context; compiled argv now sourced from runtime; host info gated via `env.read` (argv encoding parity TODO, `sys._getframe` TODO). |
| errno | Stdlib | Partial | P2 | SL2 | stdlib | Core errno constants + errorcode mapping (expand to full CPython table). |
| stat | Stdlib | Planned | P3 | SL3 | stdlib | Stat constants; import-only allowlist stub. |
| signal | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Signal handling; gated by `process.signal`. |
| select | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | `select.select` wired via `selectors` + io_poller for sockets; poll/epoll/kqueue objects and fd-based polling pending; wasm parity missing. |
| site | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; path config gated via `env.read`/`fs.read`. |
| sysconfig | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; host/path data gated via `env.read`/`fs.read`. |
| subprocess | Capability-gated | Planned | P3 | SL3 | stdlib | Process spawn control. |
| socket | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | Runtime-backed socket API (AF_INET/AF_INET6/AF_UNIX, connect/bind/listen/accept/send/recv, getaddrinfo/nameinfo, inet_pton/ntop); advanced options, full constant table, SSL, and wasm host parity pending. |
| ssl | Capability-gated | Planned | P3 | SL3 | stdlib | TLS primitives. |
| asyncio | Capability-gated | Partial | P2 | SL3 | stdlib/runtime | Shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `create_task`/`ensure_future`/`current_task`, `Event`, `wait_for`, and basic `gather`; advanced loop APIs and I/O adapters pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): asyncio pending parity) |
| selectors | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Selector shim backed by io_poller; poller variants and fd-based registrations pending; wasm parity missing. |
| threading | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | Minimal `Thread` stub (start/join) for import compatibility; runtime thread model integration pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): threading pending parity) |
| multiprocessing | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Process model integration. |
| concurrent.futures | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Executor interfaces. |
| sqlite3 | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | DB integration. |
| http | Capability-gated | Planned | P3 | SL3 | stdlib | HTTP parsing/client. |
| http.client | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; HTTP client sockets pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): http.client pending parity) |
| http.server | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; HTTP server stack pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): http.server pending parity) |
| http.cookies | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; cookie parsing/quoting pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): http.cookies pending parity) |
| urllib | Capability-gated | Planned | P3 | SL3 | stdlib | URL parsing + I/O. |
| urllib.parse | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; URL parsing helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): urllib.parse pending parity) |
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
| zlib | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; compression primitives pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): zlib pending parity) |
| zipfile/tarfile | Stdlib | Planned | P3 | SL3 | stdlib | Archive tooling. |
| hashlib/hmac | Stdlib | Planned | P2 | SL2 | stdlib/runtime | Hashing primitives (deterministic). |
| secrets | Stdlib | Planned | P3 | SL3 | stdlib | Crypto RNG (gated). |
| uuid | Stdlib | Planned | P2 | SL2 | stdlib | UUID generation (deterministic seed). |
| base64 | Stdlib | Planned | P2 | SL2 | stdlib | Import-only allowlist stub; encoding/decoding parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): base64 pending parity) |
| binascii | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; low-level codecs pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): binascii pending parity) |
| pickle | Stdlib | Partial | P2 | SL2 | stdlib | Protocol 0 only (basic builtins + slice). (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement protocol 1+, bytes/bytearray, memo cycles, and broader type coverage) |
| unittest | Stdlib | Partial | P3 | SL3 | stdlib | Minimal TestCase/runner stubs for CPython regrtest; full assertions/loader parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest pending parity) |
| doctest | Stdlib | Partial | P3 | SL3 | stdlib | Stub that rejects eval/exec/compile usage for now. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): doctest pending parity once eval/exec/compile are gated.) |
| test | Stdlib | Partial | P3 | SL3 | stdlib | Minimal `test.support` helpers (`captured_output`/`captured_stdout`/`captured_stderr`, `warnings_helper.check_warnings`, `cpython_only`, `requires`, `swap_attr`/`swap_item`, `import_helper.import_module`/`import_fresh_module`, `os_helper.temp_dir`/`unlink`); full CPython harness pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): test package pending parity) |
| argparse | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): argparse pending parity) |
| getopt | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; CLI parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): getopt pending parity) |
| locale | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; locale data pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): locale pending parity) |
| ast | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; AST API parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): ast pending parity) |
| atexit | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; exit handler semantics pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): atexit pending parity) |
| collections.abc | Stdlib | Partial | P3 | SL3 | stdlib | Import-only stub with minimal ABC shells; registration pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): collections.abc pending parity) |
| importlib | Stdlib | Partial | P3 | SL3 | stdlib | Import-only stub; dynamic import hooks pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): importlib pending parity) |
| importlib.util | Stdlib | Partial | P3 | SL3 | stdlib | Import-only stub; loader helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): importlib.util pending parity) |
| importlib.metadata | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; distribution metadata requires `fs.read`. |
| queue | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; threading integration pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): queue pending parity) |
| shlex | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; POSIX parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): shlex pending parity) |
| shutil | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read`/`fs.write` gating. |
| textwrap | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; wrapping/indent parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): textwrap pending parity) |
| time | Capability-gated | Partial | P3 | SL3 | stdlib/runtime | `monotonic`/`perf_counter` + `sleep` wired; `time`/`time_ns` gated by `time.wall` (or legacy `time`). Missing timezone/tzname/struct_time/get_clock_info/process_time. (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand time module surface + deterministic clock policy) |
| tomllib | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): tomllib pending parity) |
| platform | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; host info gated. |
| ctypes | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; `ffi.unsafe` gating. |
| cgi | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; form parsing parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): cgi pending parity) |
| html | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; escape helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html pending parity) |
| html.parser | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parser parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html.parser pending parity) |
| ipaddress | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; IPv4/IPv6 parsing pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): ipaddress pending parity) |
| mimetypes | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; filesystem lookups require `fs.read`. |
| socketserver | Capability-gated | Planned | P3 | SL3 | stdlib/runtime | Import-only allowlist stub; server sockets require `net`. |
| wsgiref | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; WSGI server stack requires `net`. |
| xml | Stdlib | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; parser/tree APIs pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): xml pending parity) |
| warnings | Stdlib | Partial | P3 | SL3 | stdlib | `warn` + filters/once/capture shim; regex filters + full registry pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): warnings pending parity) |
| traceback | Stdlib | Partial | P3 | SL3 | stdlib | `format_exc`/`format_tb`/`print_exception` with best-effort frames (CPython traceback objects or Molt (filename, line, name) tuples); full parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): traceback pending parity) |
| types | Stdlib | Partial | P3 | SL3 | stdlib | `SimpleNamespace` + mapping proxy shim; full type helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): types pending parity) |
| inspect | Stdlib | Partial | P3 | SL3 | stdlib | `getdoc`/`cleandoc` + `signature` (Molt metadata/`__code__`); broader introspection pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): inspect pending parity) |
| tempfile | Capability-gated | Partial | P3 | SL3 | stdlib | `gettempdir`/`gettempdirb` shim; full tempfile parity pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): tempfile parity) |
| glob | Capability-gated | Planned | P3 | SL3 | stdlib | Import-only allowlist stub; `fs.read` gating. |
| fnmatch | Stdlib | Partial | P3 | SL3 | stdlib | `*`/`?` + bracket class/range matching; literal `[]`/`[[]`/`[]]` escapes (no backslash quoting). |
| copy | Stdlib | Partial | P3 | SL3 | stdlib | Shallow/deep copy helpers + `__copy__`/`__deepcopy__` hooks. |
| pprint | Stdlib | Partial | P3 | SL3 | stdlib | `pprint`/`pformat` wrapper; layout rules pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): pprint pending parity) |
| string | Stdlib | Partial | P3 | SL3 | stdlib | ASCII constants + `capwords`; locale/formatter helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): string pending parity) |
| numbers | Stdlib | Planned | P3 | SL3 | stdlib | ABCs + numeric tower registration pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): numbers pending parity) |
| unicodedata | Stdlib | Planned | P3 | SL3 | stdlib/runtime | Unicode database helpers pending. (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): unicodedata pending parity) |

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
- Scope: `pack`, `unpack`, `calcsize`, basic format strings (`bBhHiIlLqQfd`, endianness).
- Semantics: Strict format parsing; error messages align with CPython.
- Runtime/IR: Implement parser + packer in Rust for speed; expose to Python layer.
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill out `math` intrinsics (trig/log/exp/floor/ceil/prod/sum) + float determinism policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` runtime `deque` type + O(1) ops + view parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `heapq` randomized stress + perf tracking.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `bisect` fast paths on list/array types.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): `array` + `struct` deterministic layouts and packing.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `re` engine + deterministic regex semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): finish `json` parity plan (Encoder/Decoder classes, JSONDecodeError details, full cls support) and add a runtime fast-path parser for dynamic strings.
- TODO(stdlib-compat, owner:frontend, milestone:SL1, priority:P2, status:planned): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:frontend, milestone:SL2, priority:P2, status:missing): `contextlib.contextmanager` lowering (generator-based context managers).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `dataclasses` transform (kw-only, order, `__annotations__`).
- TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P2, status:planned): `hashlib` deterministic hashing policy.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): expand `io` to buffered/text wrappers and streaming helpers.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:missing): full `open`/file object parity (modes/buffering/text/encoding/newline/fileno/seek/tell/iter/context manager) with differential + wasm coverage.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand `asyncio` shim to full loop/task APIs (task groups, wait, shields) and I/O adapters.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): `typing` runtime helpers + `__annotations__` preservation.

## 7. Matrix Audit (2026-01-16)
Coverage evidence (selected):
- `tests/differential/stdlib/heapq_basic.py`, `tests/differential/stdlib/heapq_more.py` (heapq core + merge/max-heap helpers).
- `tests/differential/basic/bisect_basic.py` (bisect/insort + key support).
- `tests/differential/stdlib/itertools_core.py` (core itertools iterators and combinatorics).
- `tests/differential/basic/collections_basic.py`, `tests/differential/stdlib/collections_deque.py` (collections shims + deque core).
- `tests/differential/stdlib/functools_more.py` (cmp_to_key + total_ordering parity).
- `tests/differential/stdlib/operator_basic.py` (itemgetter/attrgetter/methodcaller).
- `tests/differential/stdlib/fnmatch_basic.py`, `tests/test_stdlib_shims.py` (fnmatch core patterns + shim).

Gaps or missing coverage (audit findings):
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): add wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
