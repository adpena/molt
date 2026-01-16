# Molt Roadmap (Active)

Canonical current status: `docs/spec/STATUS.md`. This roadmap is forward-looking.

## Legend
- **Status:** Implemented (done), Partial (some semantics missing), Planned (scoped but not started), Missing (no implementation), Divergent (intentional difference from CPython).
- **Priority:** P0 (blocker), P1 (high), P2 (medium), P3 (lower).
- **Tier/Milestone:** `TC*` (type coverage), `SL*` (stdlib), `DB*` (database), `DF*` (dataframe/pandas), `LF*` (language features), `RT*` (runtime), `TL*` (tooling), `M*` (syntax milestones).

## Performance
- Vector reduction kernels now cover `sum`/`prod`/`min`/`max` with trusted fast paths; next up: float reductions and typed-buffer kernels (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): float reductions + typed-buffer kernels).
- String kernel SIMD paths cover find/split/replace with Unicode-safe index translation; next: Unicode index caches and wider SIMD (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): Unicode index caches + wider SIMD).

## Type Coverage
- memoryview (Partial): multi-dimensional `format`/`shape`/`strides`/`nbytes` + `cast`, tuple scalar indexing, 1D slicing/assignment for bytes/bytearray-backed views.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): memoryview multi-dimensional slicing + sub-views (C-order parity).
- Implemented: BigInt heap fallback + arithmetic parity beyond 47-bit inline ints.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`) + `__set_name__` hook.
- Implemented: C3 MRO + multiple inheritance for attribute lookup + `super()` resolution + data descriptor precedence.
- Implemented: reflection builtins (`type`, `isinstance`, `issubclass`, `object`) for base chains (no metaclasses).
- Implemented: BaseException root + exception chaining (`__cause__`, `__context__`, `__suppress_context__`) + `__traceback__` name tuples + StopIteration.value propagation.
- Implemented: descriptor deleter semantics (`__delete__`, property deleter) + attribute deletion wiring.
- Implemented: set literals/constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- Implemented: format mini-language for ints/floats + f-string conversion flags (`!r`, `!s`, `!a`).
- Implemented: call argument binding for Molt functions (positional/keyword/`*args`/`**kwargs`) with pos-only/kw-only enforcement.
- Implemented: lambda lowering with closures, defaults, and kw-only/varargs support.
- Implemented: `sorted()` builtin with stable ordering + key/reverse (core ordering types).
- Implemented: `list.sort` with key/reverse and rich-compare fallback for user-defined types.
- Implemented: `str.lower`/`str.upper`, `list.clear`/`list.copy`/`list.reverse`, and `dict.setdefault`/`dict.update`.
- Implemented: container dunder/membership fallbacks (`__contains__`/`__iter__`/`__getitem__`) and builtin class method access for list/dict/str/bytes/bytearray.
- Implemented: dynamic call binding for bound methods/descriptors with builtin defaults + expanded class decorator parity coverage.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:missing): async comprehensions (async for/await in comprehensions).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): matmul dunder hooks (`__matmul__`/`__rmatmul__`) with buffer2d fast path.
- Partial: wasm generator state machines + closure slot intrinsics + channel send/recv intrinsics + async pending/block_on parity landed; remaining scheduler semantics (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): wasm scheduler semantics).
- Implemented: wasm async state dispatch uses encoded resume targets to avoid state-id collisions and keeps state/poll locals distinct (prevents pending-state corruption on resume).
- Implemented: async iterator protocol (`__aiter__`/`__anext__`) with `aiter`/`anext` lowering and `async for` support; sync-iter fallback remains for now.
- Implemented: `anext(..., default)` awaitable creation outside `await`.
- Implemented: `async with` lowering for `__aenter__`/`__aexit__` (single manager, simple name binding).
- Implemented: cancellation token plumbing with request-default inheritance and task override; automatic cancellation injection into awaits still pending (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): cancellation injection on await).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): native-only tokio host adapter for compiled async tasks with determinism guard + capability gating (no WASM impact).
- TODO(syntax, owner:frontend, milestone:M3, priority:P2, status:missing): structural pattern matching (`match`/`case`) lowering and semantics (see `docs/spec/0021_SYNTACTIC_FEATURES_MATRIX.md`).
- TODO(opcode-matrix, owner:frontend, milestone:M3, priority:P2, status:missing): `MATCH_*` opcode coverage for pattern matching (see `docs/spec/0019_BYTECODE_LOWERING_MATRIX.md`).
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): f-string format specifiers and debug spec (`f"{x:.2f}"`, `f"{x=}"`) parity (see `docs/spec/0021_SYNTACTIC_FEATURES_MATRIX.md`).
- TODO(syntax, owner:frontend, milestone:M3, priority:P3, status:missing): type alias statement (`type X = ...`) and generic class syntax (`class C[T]: ...`) coverage (see `docs/spec/0021_SYNTACTIC_FEATURES_MATRIX.md`).
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator opcode coverage and lowering gaps (see `docs/spec/0019_BYTECODE_LOWERING_MATRIX.md`).
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector implementation (see `docs/spec/0023_SEMANTIC_BEHAVIOR_MATRIX.md`).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence policy (see `docs/spec/0023_SEMANTIC_BEHAVIOR_MATRIX.md`).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define and implement `libmolt` C API shim + `Py_LIMITED_API` target (see `docs/spec/0212_C_API_SYMBOL_MATRIX.md`).

## Stdlib
- Partial: importable `builtins` module binding supported builtins (attribute gaps tracked in the matrix).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill `builtins` module attribute coverage.)
- Partial: asyncio shim (`run`/`sleep` lowered to runtime with delay/result semantics; `set_event_loop`/`new_event_loop` stubs); loop/task APIs still pending (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task API parity).
- Partial: shims for `warnings`, `traceback`, `types`, `inspect`, `fnmatch`, `copy`, `pprint`, `string`, `typing`, `sys`, `os`, `asyncio`, `threading`, `bisect`, `heapq`, `functools`, `itertools`, and `collections` (capability-gated env access).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- Import-only allowlist expanded for `base64`, `binascii`, `pickle`, `unittest`, `site`, `sysconfig`, `collections.abc`, `importlib`, and `importlib.util`; planned additions: `importlib.metadata`, `html`, `html.parser`, `http.cookies`, `http.client`, `http.server`, `ipaddress`, `mimetypes`, `socketserver`, `wsgiref`, `xml`, `email.policy`, `email.message`, `email.parser`, `email.utils`, `email.header`, `urllib.parse`, `urllib.request`, `urllib.error`, `urllib.robotparser`, `logging.config`, `logging.handlers`, `cgi`, and `zlib` (API parity pending; TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + tests).

## Compatibility Matrix Execution Plan (Next 8 Steps)
1) Done: TC2 iterable unpacking + starred targets in assignment/for targets (tests + spec/status updates).
2) TC2: remaining StopIteration semantics (sync/async) with differential coverage (StopIteration.value propagation done).
3) TC2: builtin conversions (`bool`, `str`, `complex`) with hook/error parity.
   TODO(stdlib-compat, owner:runtime, milestone:TC2, priority:P2, status:missing): `str(bytes, encoding, errors)` decoding parity for bytes-like inputs.
4) TC2: async comprehensions lowering + runtime support with parity tests.
5) TC2/TC3: object-level `__getattr__`/`__getattribute__`/`__setattr__`, reflection builtins, CPython `hash` parity (`PYTHONHASHSEED`) + `format`/rounding; update tests + docs.
6) SL1: `functools` (`lru_cache`, `partial`, `reduce`) with compile-time lowering and deterministic cache keys; `cmp_to_key`/`total_ordering` landed.
7) SL1: `itertools` + `operator` intrinsics plus `heapq` fast paths; `bisect`/`heapq` shims landed (fast paths now wired).
8) SL1: `math` intrinsics + deterministic `array`/`struct` layouts with wasm/native parity tests.

## Offload / IPC
- Partial: `molt_accel` v0 scaffolding (stdio framing + client + decorator) with auto cancel-check detection, payload/response byte metrics, and shared demo payload builders; `molt_worker` stdio shell with demo handlers and compiled dispatch (`list_items`/`compute`/`offload_table`/`health`), plus optional worker pooling via `MOLT_ACCEL_POOL_SIZE`.
  (TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): finalize accel retry/backoff + non-demo handler coverage.)
- Implemented: compiled export loader + manifest validation (schema, reserved-name filtering, error mapping) with queue/timeout metrics.
- Implemented: worker tuning via `MOLT_WORKER_THREADS` and `MOLT_WORKER_MAX_QUEUE` (CLI overrides).
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): propagate cancellation into real DB tasks; extend compiled handlers beyond demo coverage.

## DB
- Partial: `molt-db` pool skeleton (bounded, sync), feature-gated async pool primitive, SQLite connector (native-only; wasm parity pending), and async Postgres connector with statement cache; `molt_worker` exposes `db_query`/`db_exec` for SQLite + Postgres (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity).
- Top priority: wasm parity for DB connectors before expanding DB adapters or query-builder ergonomics.
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:missing): ship wasm DB client shims + parity tests; Node/WASI host adapter now forwards `db_query`/`db_exec` to `molt-worker` via `run_wasm.js`.

## Parity Cluster Plan (Next)
- 1) Async runtime core: Task/Future APIs, scheduler, contextvars, and cancellation injection into awaits/I/O. Key files: `runtime/molt-runtime/src/lib.rs`, `src/molt/stdlib/asyncio.py`, `src/molt/stdlib/contextvars.py`, `docs/spec/STATUS.md`. Outcome: asyncio loop/task parity for core patterns. Validation: new unit + differential tests; `tools/dev.py test`.
- 2) Capability-gated async I/O: sockets/SSL/selectors/time primitives with cancellation propagation. Key files: `docs/spec/0900_HTTP_SERVER_RUNTIME.md`, `docs/spec/0505_IO_ASYNC_AND_CONNECTORS.md`, `runtime/molt-runtime/src/lib.rs`. Outcome: async I/O primitives usable by DB/HTTP stacks. Validation: I/O unit tests + fuzzed parser tests + wasm/native parity checks.
- 3) DB semantics expansion: implement `db_exec`, transactions, typed param mapping; add multirange + array lower-bound decoding. Key files: `runtime/molt-db/src/postgres.rs`, `runtime/molt-worker/src/main.rs`, `docs/spec/0700_MOLT_DB_LAYER_VISION.md`, `docs/spec/0701_ASYNC_PG_POOL_AND_PROTOCOL.md`, `docs/spec/0915_MOLT_DB_IPC_CONTRACT.md`. Outcome: production-ready DB calls with explicit write gating and full type decoding. Validation: dockerized Postgres integration + cancellation tests.
- 4) WASM DB parity: define WIT/host calls for DB access and implement wasm connectors in molt-db. Key files: `wit/molt-runtime.wit`, `runtime/molt-runtime/src/lib.rs`, `runtime/molt-db/src/lib.rs`, `docs/spec/0400_WASM_PORTABLE_ABI.md`. Outcome: wasm builds can execute DB queries behind capability gates. Validation: wasm harness tests + native/wasm result parity.
- 5) Framework-agnostic adapters: finalize `molt_db_adapter` + helper APIs for Django/Flask/FastAPI with shared payload builders. Key files: `src/molt_db_adapter/`, `docs/spec/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md`, `demo/`, `tests/`. Outcome: same IPC contract across frameworks with consistent error mapping. Validation: integration tests in sample Django/Flask/FastAPI apps.
- 6) Production hardening: propagate cancellation into compiled entrypoints/DB tasks, add pool/queue metrics, run bench harness. Key files: `runtime/molt-worker/src/main.rs`, `bench/scripts/`, `docs/spec/0910_REPRO_BENCH_VERTICAL_SLICE.md`. Outcome: stable P99/P999 and reliable cancellation/backpressure. Validation: `bench/scripts/run_stack.sh` + stored JSON results.

## Tooling
- Keep type facts + `ty` validation wired into build/lint flows and surface regressions early.
- Implemented: CLI wrappers for `run`/`test`/`diff`/`bench`/`profile`/`lint`/`doctor`/`package`/`publish`/`verify`,
  plus determinism/capability checks and vendoring materialization; next: registry auth, SBOM/signing, and remote publish flows.
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): registry auth + SBOM/signing + remote publish flows.)
- Implemented: initial cross-target native builds (Cranelift target + zig link); next: cross-linker configuration,
  target capability manifests, and runtime cross-build caching (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:partial): cross-target ergonomics).
- CLI Roadmap (plan):
  - Build cache clarity: `--cache-report` by default in `--json`, `molt clean --cache`, and cache hit/miss summaries with input fingerprints.
  - Build UX polish: stable `--out-dir` defaults (`.molt/build/<module>`), explicit `--emit` artifacts, and `--emit-ir` + `--emit-json` dumps.
  - Profiles + metadata: `--profile {dev,release}` consistency across backend/runtime, and JSON metadata with toolchain hashes.
  - Config introspection: `molt config` shows merged `molt.toml`/`pyproject.toml` plus resolved build settings.
  - Cross-target ergonomics: cache-aware runtime builds, target flag presets, and capability manifest helpers.
- Track complex performance work in `OPTIMIZATIONS_PLAN.md` before large refactors.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:planned): replace handle-table locks with sharded or lock-free lookups once handle migration lands.
- TODO(tooling, owner:release, milestone:TL2, priority:P2, status:planned): formalize release tagging (start at `v0.0.001`, increment thousandth) and require super-bench stats for README performance summaries.

## Django Demo Path (Draft, 5-Step)
- Step 1 (Core semantics): close TC1/TC2 gaps in `docs/spec/0014_TYPE_COVERAGE_MATRIX.md` for Django-heavy types (dict/list/tuple/set/str, iter/len, mapping protocol, kwargs/varargs ordering per docs/spec/0016_ARGS_KWARGS.md, descriptor hooks, class `__getattr__`/`__setattr__`).
- Step 2 (Import/module system): package resolution + module objects, `__import__`, and a deterministic `sys.path` policy; unblock `importlib` basics.
- Step 3 (Stdlib essentials): advance `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` for `functools`, `itertools`, `operator`, `collections`, `contextlib`, `inspect`, `typing`, `dataclasses`, `enum`, `re`, and `datetime` to Partial with tests.
- Step 4 (Async/runtime): production-ready asyncio loop/task APIs, contextvars, cancellation injection, and long-running workload hardening.
- Step 5 (I/O + web/DB): capability-gated `os`, `sys`, `pathlib`, `logging`, `time`, `selectors`, `socket`, `ssl`; ASGI/WSGI surface, HTTP parsing, and DB client + pooling/transactions (start sqlite3 + minimal async driver), plus deterministic template rendering.
- Cross-framework note: DB IPC payloads and adapters must remain framework-agnostic to support Django/Flask/FastAPI.
