<!-- Foundation audit 29. Architect: read-only research-granted agent, 2026-06-06.
Saved verbatim. SUPERVISOR SLOT REMAPPING (the doc's internal proposed numbers collide
with already-taken slots — 27=Perceus, 28=asyncio-frontier, 30=core-language portfolio):
its "Doc 28 Threading" -> slot 33; "Doc 29 Zero-Copy Spine" -> slot 34; "Doc 30 Web
Stack" -> slot 35; "Doc 31 mmap/File-IO" -> slot 36; "Doc 32 Regex" -> slot 37.
Top-5 commission order stands: 33 threading + 37 regex now (independent); 34 zero-copy
after/with #49; 36 mmap after 34; 35 web after docs 26/28 land. -->

# Domain-Critical Subsystem Portfolio Audit

## Codebase Inventory Summary

The audit covers molt at commit `951938075` (branch `main`). The runtime crate tree is `runtime/molt-runtime/` with its satellite crates (`molt-runtime-net`, `molt-runtime-regex`, `molt-runtime-zoneinfo`, `molt-runtime-http`, `molt-cpython-abi`, etc.). The stdlib tree is `src/molt/stdlib/` (~280 Python files, full CPython 3.12+ mirror structure). Feature gates live in `/Users/adpena/Projects/molt/runtime/molt-runtime/Cargo.toml`.

---

## SUBSYSTEM 1: Threading Ladder

### Current State

**GIL architecture.** Two GIL implementations coexist:

- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/concurrency/gil.rs` — the authoritative GIL: a single `static PREINIT_GIL: Mutex<()>` used throughout the entire program lifetime (pre-init through post-shutdown). Implements per-thread depth counting via TLS (`GIL_DEPTH`), a thread-count fastpath (`GIL_THREAD_COUNT <= 1` skips the mutex for reentrant acquires), and a TLS-destruction fallback path (`GIL_FALLBACK_OWNER/DEPTH`). `GilReleaseGuard` saves and restores depth for blocking I/O.

- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/gil.rs` — a Phase 1 GIL-removal stub: a `GIL_RELEASED: AtomicBool` flag for a future `@par` parallel-execution mode and a `ObjectLock` (per-object `Mutex<()>`) intended to replace global GIL for container mutations. This is infrastructure scaffolding only; nothing in production codegen activates it.

**Refcount ordering.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/refcount.rs:1-108` — `MoltRefCount` uses `AtomicU32` on native (with an explicit `acquire_fence()` on drop-to-zero) and `Cell<u32>` on WASM (single-threaded, no atomics). The ordering is safe for the existing GIL model: the GIL is acquired before any inc_ref/dec_ref that touches shared objects. The acquire fence on zero ensures the object's final mutation happens-before its destruction.

**threading module.** `/Users/adpena/Projects/molt/src/molt/stdlib/threading.py:1-100+` — real threading backed by ~55 Rust intrinsics (`molt_thread_spawn_shared`, `molt_lock_new/acquire/release`, `molt_rlock_*`, `molt_condition_*`, `molt_event_*`, `molt_semaphore_*`, `molt_barrier_*`, `molt_local_*`). Thread spawning uses `/Users/adpena/Projects/molt/runtime/molt-runtime/src/concurrency/isolates.rs` which calls `std::thread::spawn`, acquires the GIL in the worker via `GilGuard::new()`, and dispatches the Python callable through `call_callable1`. The GIL is released during the call if the callable releases it explicitly. The thread registry (`THREAD_REGISTRY: Lazy<Mutex<ThreadRegistry>>`) tracks threads by token.

**concurrent.futures.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/concurrent.rs` — `ThreadPoolExecutor` is real: crossbeam-channel unbounded work queue, `std::thread::spawn` workers that acquire the GIL to run the Python callable, `SharedFuture` (`Arc<Mutex<FutureState>>`). Real OS threads, real Python callables dispatched.

**multiprocessing.** `/Users/adpena/Projects/molt/src/molt/stdlib/multiprocessing/context.py` delegates to `_api_surface.py` behind a capability check. The gap audit (doc 16) confirms: fork/forkserver map to spawn; `Queue.put/get` raise `NotImplementedError` for parent/child semantics. For an AOT binary, `multiprocessing.spawn` must re-exec the binary with a specific entry point argument — this is architecturally undesigned.

**PEP 703 / free-threading.** Zero committed design or implementation. The `object/gil.rs` Phase 1 scaffold exists but is not connected to any real execution path. `GIL_RELEASED` is never set by production code. No committed plan for eliminating the global mutex on the hot RC path.

**PEP 734 / subinterpreters.** `src/molt/stdlib/_interpreters.py` and `_interpchannels.py` are pure RuntimeError stubs (`TODO(stdlib-parity, milestone:SL3, priority:P1, status:planned)`). No design document.

**Memory model.** The current model is: GIL-held = all object mutations are serialized. The AtomicU32 refcount is correctly ordered for the GIL + cpython-abi signal-handler case. Free-threading would require every mutable object to carry an `ObjectLock` (the stub in `object/gil.rs`) and every mutation site to acquire it — this is a large structural change touching live dict, list, set, and every builder function.

### Frontier Definition

CPython 3.14 (PEP 703 second-generation + PEP 779 promoted to "supported optional"): per-object locks, biased reference counting, lock-free freelist, specializing adaptive interpreter re-enabled under no-GIL. The single-thread performance penalty is ~5-10%. The `InterpreterPoolExecutor` (PEP 734) gives shared-nothing parallelism with channels for data exchange. For a compiled AOT system the correct frontier is not GIL removal in the CPython sense — it is: (a) a clear decision on molt's concurrency model (GIL-per-interpreter-slot vs. lock-free objects vs. message-passing-only), (b) a `multiprocessing.spawn` protocol that re-execs the binary via a registered entry-point manifest, and (c) a subinterpreter isolation model for `concurrent.interpreters`.

### Scores

Data science: importance 2, gap 3. Engineering: importance 3, gap 3. Web: importance 3, gap 3. ML: importance 2, gap 2.

**Weighted gap-importance product (sum over domains): 34 / 48 possible.**

### Dependency Edges

The RC substrate (#20 RC-ownership-drop-insertion arc) must stabilize before free-threading can be designed (per-object locking multiplies RC complexity). The `@par` dispatch path in `object/gil.rs` is a stub dependency on the StateDispatch arc (#24). The inliner activation (E1) and call-graph (#S4) do not block this, but a working call-graph is needed to identify thread-escape analysis for safe RC elision.

### Verdict: NEEDS-FRONTIER-DOC-NOW

This is the highest undesigned gap for web and engineering domains. The decision about molt's concurrency model — GIL-per-interpreter, per-object lock, lock-free — must be made before any threading-adjacent work (multiprocessing, subinterpreters, free-threading pillar, WASM threading) can proceed coherently. The `spawn`-for-AOT problem is novel and needs explicit design. Commission **Doc 28: Threading Concurrency Model & Multiprocessing Spawn Protocol**.

---

## SUBSYSTEM 2: Buffer Protocol / memoryview / Zero-Copy Spine

### Current State

**memoryview.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops_memoryview.rs:1-701` — substantive implementation. Supports 1-D and N-D shaped views (`alloc_memoryview` / `alloc_memoryview_shaped`), `cast()` with C-contiguous guard, `tobytes()`, `tolist()` (recursive N-D), `count()`, `index()`, `hex()`, `toreadonly()`, `release()` (currently a no-op — released-view state is not modeled). Feature-gated via `builtin_memoryview` in `stdlib_server` and above.

`molt_buffer_export` at line 639 exposes a `BufferExport` C struct (ptr, len, readonly, stride, itemsize) for bytes/bytearray/memoryview objects — this is the C-ABI zero-copy bridge.

**buffer2d.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/buffer2d.rs:1-167` — a molt-specific 2D integer buffer with get/set/matmul. This is not PEP 3118 compliant; it is a private ML scaffold.

**cpython-abi buffer.** `/Users/adpena/Projects/molt/runtime/molt-cpython-abi/src/api/buffer.rs:1-82` — `PyObject_GetBuffer` **always returns -1** (does not support the buffer protocol from C extensions). `PyBuffer_Release` drops the refcount. This means C extension modules that call `PyObject_GetBuffer` on molt objects always fail — numpy zero-copy is structurally blocked at the C-ABI layer.

**DLPack.** No `__dlpack__` / `__dlpack_device__` methods on `Tensor` in `/Users/adpena/Projects/molt/src/molt/gpu/tensor.py` (grepped; none found). The GPU buffer is accessible through `molt_buffer_export` but there is no DLPack capsule, no `DLTensor` struct, no `__dlpack_compat__` method.

**PEP 3118 completeness.** The N-D memoryview supports shapes and strides. `suboffsets` (Fortran-order indirect arrays) are not modeled. Format strings beyond scalar formats are not documented as complete. The gap to full PEP 3118 is: released-view modeling, suboffsets, writable memoryview mutation (the `bytes` path at line 65 returns a readonly view; bytearray writable path is present but mutation through the view is unverified).

### Frontier Definition

The data science zero-copy chain requires three layers: (1) full PEP 3118 from Python side (molt has ~80% of this), (2) `PyObject_GetBuffer` surfacing through `molt-cpython-abi` so that C extensions (numpy's C layer) can get a raw pointer into molt objects, (3) `__dlpack__`/`__dlpack_device__` on `Tensor` so that JAX/PyTorch/Arrow can exchange GPU buffers without copy. The DLPack spec (v0.8+, Python spec via `dmlc.github.io/dlpack`) uses a capsule-based protocol that doesn't require the CPython buffer protocol — it is the modern cross-framework zero-copy standard. All three layers are independently tractable but form a dependency chain: C extension numpy interop requires (2); ML framework interop requires (3).

### Scores

Data science: importance 3, gap 3. Engineering: importance 2, gap 2. Web: importance 1, gap 1. ML: importance 3, gap 3.

**Weighted product: 28 / 48.**

### Dependency Edges

`PyObject_GetBuffer` fix in `molt-cpython-abi` depends on `#49 C-ABI decision` — specifically whether molt commits to the CPython object layout (`PyObject` header with `ob_type` pointer) or keeps its NaN-boxing layout and translates at the ABI boundary. The DLPack `Tensor` extension is independent of the C-ABI decision.

### Verdict: NEEDS-FRONTIER-DOC-NOW

The `PyObject_GetBuffer` always-returning-minus-one is a hard block on numpy zero-copy. The missing `__dlpack__` on Tensor blocks all modern ML framework interop. This is the single highest-impact gap for ML and data science. Commission **Doc 29: Zero-Copy Spine (Buffer Protocol Completion + DLPack + C-ABI Buffer Bridge)**. This is the doc the mandate calls for.

---

## SUBSYSTEM 3: Serialization (pickle, json, struct, marshal)

### Current State

**json.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/json.rs` + `src/molt/stdlib/json/__init__.py:1-60`. The module has `molt_json_loads_ex`, `molt_json_dumps_ex`, `molt_json_raw_decode_ex`, `molt_json_encode_basestring_obj`, `molt_json_detect_encoding`, etc. The Python shim has a fast-path via `_try_intrinsic_dumps`. The implementation uses `serde_json` (Cargo.toml:178). Performance: serde_json is faster than CPython's pure-Python json but far from orjson class (which uses SIMD string escaping with AVX-512/SSE2, direct-to-bytes output, zero-copy datetime/uuid serialization). No SIMD-accelerated string scanning. No streaming interface. The `cls` and `object_hook` fallbacks exist at the Python level.

**pickle.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/functions_pickle.rs` + `src/molt/stdlib/pickle.py`. Protocols 0-5 core, memo, reducer, state_setter, `PickleBuffer` (`NEXT_BUFFER`/`READONLY_BUFFER`). Differential tests green 10/10 per doc 16. Gap: pickle protocol 5 out-of-band buffer support (critical for ML checkpoint zero-copy) is present at the opcode level but the actual buffer handoff to `PickleBuffer` objects that reference external memory (e.g., GPU tensors) is unverified against real ML payloads.

**struct.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/structs.rs:1-80+`. Full parse_format with all endian/alignment modes. All standard-size format codes. This appears complete for the common case.

**marshal.** `src/molt/stdlib/marshal.py` — 0.6KB, partial per doc 16.

### Frontier Definition

orjson architecture: Rust pyo3-ffi, type dispatch via ob_type pointer examination, SIMD string escaping (AVX-512 with SSE2/scalar fallback), direct bytes output without intermediate Python objects, native datetime/uuid/numpy serialization, LTO+panic=abort. For ML checkpoint-sized payloads: pickle protocol 5 with `PickleBuffer` zero-copy for GPU tensors, safetensors (Hugging Face's zero-copy tensor serialization format using mmap), and potentially msgpack (`rmpv` is already in Cargo.toml as optional). The frontier for molt is: (1) SIMD string escaping in the json fast-path, (2) direct-to-bytes allocation bypass (avoid allocating a Python bytes object when the caller will immediately write to a socket or file), (3) pickle protocol 5 buffer handoff verified against GPU Tensor objects.

### Scores

Data science: importance 3, gap 2. Engineering: importance 2, gap 1. Web: importance 3, gap 2. ML: importance 3, gap 2.

**Weighted product: 24 / 48.**

### Dependency Edges

json SIMD path depends on the `simdutf` dependency already present in Cargo.toml (line 233) for UTF-8 processing — the infrastructure is there. pickle protocol 5 buffer handoff depends on the zero-copy spine (Subsystem 2). marshal completeness is low-value and standalone.

### Verdict: NEEDS-FIX-ONLY for json (SIMD string escaping, direct-bytes path — no new design doc needed, this is an optimization task). NEEDS-FRONTIER-DOC for the ML checkpoint / safetensors / pickle-p5-buffer-handoff arc, which is part of Doc 29 (zero-copy spine).

---

## SUBSYSTEM 4: Web Stack

### Current State

**TLS/SSL.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/ssl.rs:1-60+` — backed by rustls (Cargo.toml line 237: `rustls = { version = "0.23", optional = true }`). `SSLContext` creation, `SSLSocket` wrapping, handshake/read/write with GIL release during I/O. Missing: `recv`/`send` with `MSG_*` flags (3 `NotImplementedError` sites per doc 16), session resumption, ALPN negotiation completeness, client certificate auth.

**Sockets.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/sockets.rs:1-80+` — backed by `mio` (v1.2, Cargo.toml line 235) + `socket2`. Full `MoltSocketKind` enum: `TcpStream`, `TcpListener`, `UdpSocket`, `UnixStream`/`Listener`/`Datagram`. `socket2` for raw socket creation. `TcpStream` and `TcpListener` are mio-wrapped for non-blocking I/O via the event loop.

**WebSockets.** `tungstenite = { version = "0.29", features = ["handshake", "rustls-tls-webpki-roots"], optional = true }` (Cargo.toml line 240). The feature is present; the Python-facing API wiring needs to be audited in the asyncio module.

**HTTP.** `/Users/adpena/Projects/molt/runtime/molt-runtime-http/src/functions_http.rs:1-100+` — HTTP client using `TcpStream` + manual HTTP/1.1 parser. No HTTP/2 (h2 crate absent from Cargo.toml). No HTTP/1.1 pipelining. No keep-alive pool. The `urllib.request` module uses this path.

**ASGI/server runway.** The event loop at `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs:1-100` implements `asyncio.BaseEventLoop` semantics with a `ReadyQueue` (VecDeque), `TimerHeap` (BinaryHeap), and mio-backed I/O (native) or host-delegated (WASM). This is the correct foundation. However there is no ASGI `receive`/`send` abstraction, no HTTP/2 upgrade path in the event loop, and no server-side connection lifecycle manager (the building blocks for an ASGI server like uvicorn).

**Streams integration.** The doc 27 arc (asyncio) is in-flight. The event loop's `add_reader`/`add_writer`/`notify_reader_ready`/`notify_writer_ready` (fd-based, native only per lines 19-22 of event_loop.rs) provide the substrate for streams. The `asyncio.streams.StreamReader`/`StreamWriter` is present in the stdlib tree under `asyncio/`.

### Frontier Definition

The frontier for "web" is: HTTP/2 + TLS ALPN (h2 crate + rustls ALPN), an ASGI interface layer (`Scope`, `Receive`, `Send` callable protocol over the mio event loop), WebSocket upgrade handling integrated with the event loop's fd callbacks, and HTTP keep-alive pooling in the client. Granian (Rust ASGI server) demonstrates the correct architecture: Rust handles connection accept/HTTP parse/send; Python application code sees only the ASGI interface. For molt this maps to: mio loop accepts connections, a Rust HTTP parser (httparse crate or similar) produces ASGI scope dictionaries, Python `async def app(scope, receive, send)` is dispatched through the existing `call_callable3` path.

### Scores

Data science: importance 1, gap 2. Engineering: importance 3, gap 3. Web: importance 3, gap 3. ML: importance 1, gap 2.

**Weighted product: 28 / 48.**

### Dependency Edges

Blocked by doc 27 (asyncio event loop / real async generators). The ASGI server layer cannot be designed until the async generator protocol is working (doc 26 real-async-generators is in-flight). HTTP/2 is independent and can proceed in parallel. WebSocket upgrade depends on tungstenite wiring (already in Cargo.toml).

### Verdict: NEEDS-FRONTIER-DOC-NOW. Commission **Doc 30: Web Stack (HTTP/2 + ASGI Interface + WebSocket Integration)** — with explicit dependency on doc 26/27 for the async receive/send callable dispatch.

---

## SUBSYSTEM 5: mmap + File I/O Throughput

### Current State

**mmap.** `src/molt/stdlib/mmap.py` is a pure import-smoke stub (301B, `molt_import_smoke_runtime_ready`). No implementation exists.

**File I/O architecture.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/io.rs:1-80+` — uses `std::fs::OpenOptions` and `std::io::{Read, Seek, Write}`. Default buffer size is `DEFAULT_BUFFER_SIZE: i64 = 8192` (line 23). VFS layer (`vfs/file.rs`) handles virtual filesystem with `Vec<u8>` content + cursor model (not zero-copy). Real file access uses `std::fs` directly.

**sendfile / copy_file_range.** No implementation found. No `os.sendfile` intrinsic in the intrinsic list (doc 16 lists `molt_os_listdir`, `molt_os_walk`, `molt_os_stat` etc.; no `molt_os_sendfile`). `shutil.py` is partial (9.4KB, doc 16).

**io buffering.** The 8192-byte buffer is the only I/O buffering layer. No readahead, no page-aligned buffer, no `io_uring` or `kqueue`-based async read, no direct-I/O (`O_DIRECT`).

**stdlib_fs_extra feature.** Provides `glob` and `tempfile` crates (Cargo.toml lines 95, 217). Not related to throughput.

### Frontier Definition

For data-engineering workloads: mmap (`mmap.mmap` backed by `libc::mmap`/`CreateFileMapping` on Windows) with Python buffer protocol export so that `memoryview(mmap_obj)` works and numpy can read it zero-copy. `os.sendfile` for zero-copy network transfer. `copy_file_range` (Linux 4.5+, macOS 12.0+) for in-kernel file copy. Async file reads via `io_uring` (tokio-uring or nix io_uring bindings) — this is the frontier used by databases and high-throughput log processors.

### Scores

Data science: importance 3, gap 3. Engineering: importance 3, gap 3. Web: importance 2, gap 2. ML: importance 2, gap 3.

**Weighted product: 30 / 48.**

### Dependency Edges

mmap implementation requires the buffer protocol completion (Subsystem 2) to be useful — a working `mmap` that doesn't expose a buffer protocol view is half the value. The `os.sendfile` / `copy_file_range` calls are independent. Async file I/O depends on doc 27 (asyncio event loop substrate).

### Verdict: NEEDS-FRONTIER-DOC-NOW for mmap (it is a 301B stub with no design). Commission **Doc 31: mmap + File I/O Throughput (mmap implementation, sendfile, copy_file_range, io_uring async path)**. The mmap doc should be written jointly with or after Doc 29 (zero-copy spine) since the buffer protocol export from mmap is a shared concern.

---

## SUBSYSTEM 6: Regex

### Current State

**Engine architecture.** The regex engine is entirely in Python (`src/molt/stdlib/re/` — wait, this directory does not exist; `re.py` is a 4.6KB wrapper). The Rust satellite `molt-runtime-regex` (`runtime/molt-runtime-regex/src/regex.rs:1-80`) provides lookaround helpers (`molt_re_positive_lookahead`, `molt_re_negative_lookahead`, `molt_re_positive_lookbehind`, `molt_re_negative_lookbehind`), verbose-strip, `fullmatch_check`, named-backref advance. The main `functions_re.rs` provides `molt_re_literal_matches` and character-class fast paths.

The engine itself is pure Python with Rust-backed literal and lookaround fast paths. There is no NFA/DFA compiled engine, no regex crate, no RE2/PCRE2 integration.

**Known gaps.** From doc 16: lookahead/named groups/flag scoping/backreferences all raise `NotImplementedError`. The comment in `regex.rs:57` (`SENTINEL_FALLBACK: i64 = -2`) shows the design intent: intrinsics return -2 when they cannot handle the pattern, and the Python layer is supposed to fall back — but the comment in doc 16 says "host fallback disabled," meaning complex patterns currently just fail.

**Re error arity.** One drift bug was found in past sessions (the `re.error` arity fix from the parity sweep, referenced in MEMORY.md project_session_20260602_correctness_sweep).

### Frontier Definition

The Rust `regex` crate (v1.11+) uses a lazy DFA with the Teddy SIMD algorithm (AVX2/NEON) borrowed from Hyperscan, Unicode support, and RE2-like worst-case `O(n)` guarantee. It achieves ~1-2 GB/s on typical patterns, far above CPython's pure-Python engine (~130 MB/s). For molt the frontier is: replace the pure-Python engine with a Rust `regex` crate backend exposed through the existing `molt-runtime-regex` satellite, providing a `MoltPattern` handle (compiled NFA/DFA), group capture extraction, and flag mapping. The `regex` crate is already in the Rust ecosystem and has no unsafe code. The critical correctness constraint is Unicode semantics: `regex` crate defaults to Unicode-aware matching which must align with Python's `re.UNICODE` default.

**Named groups, backreferences.** The `regex` crate does not support backreferences (they break the `O(n)` guarantee). Python's `re` allows backreferences. This requires a two-tier design: `regex` crate for patterns without backreferences (the vast majority of data-science use), and a fallback DFA+backtracking engine for patterns with `\1`/`(?P=name)`. The `regex` crate's `regex::bytes` variant handles binary data.

### Scores

Data science: importance 3, gap 3. Engineering: importance 2, gap 2. Web: importance 3, gap 3. ML: importance 1, gap 1.

**Weighted product: 26 / 48.**

### Dependency Edges

Independent of all in-flight arcs. Adding `regex` crate to `molt-runtime-regex/Cargo.toml` is the only structural dependency.

### Verdict: NEEDS-FRONTIER-DOC-NOW. The gap is severe (any pattern with a named group or lookahead raises NotImplementedError). Commission **Doc 32: Regex Engine (regex crate integration, two-tier NFA/backtrack, Unicode parity)**.

---

## SUBSYSTEM 7: Math / Decimal / Random

### Current State

**math.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/math.rs:1-60+` — full `stdlib_math` feature, backed by `libm` on WASM and native `f64` on x86/arm64. `math.sumprod` (3.12 new) implemented. Most transcendentals present. `cmath` is partial (doc 16: "complex math pending complex literal support"). `statistics` has `NormalDist` pending.

**decimal.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/decimal.rs:1-13` — conditional dispatch: `decimal_with_mpdec.rs` when `molt_has_mpdec` is set, else `decimal_without_mpdec.rs:1-80+`. The without-mpdec version is a complete pure-Rust reimplementation using `BigInt` arithmetic. Doc 16 marks decimal as "Full — complete 3.12 API." The key gap: no `libmpdec` (C library used by CPython) binding. The pure-Rust path may have performance gaps on very high precision.

**random.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/random_mod.rs:1-80+` — Mersenne Twister (MT19937), parameters match CPython's `_randommodule.c`. Handle model: `Mutex<HashMap<i64, MersenneTwisterRng>>`. Distribution algorithms are described as "follows CPython 3.12 random.py exactly." `getrandom::fill` for OS entropy. Determinism contract: seeded Random instances produce identical output to CPython — critical for reproducible ML training and test fixtures.

**fractions.** Doc 16: "Full — 9.2KB." No issues.

**operator.** Doc 16: "Partial — fast-path intrinsics pending." The 1.9KB file suggests minimal implementation.

### Frontier Definition

For math: deterministic cross-platform float output (platform-independent `libm` for transcendentals). For decimal: `libmpdec` binding for production-grade big-decimal performance (CPython uses it). For random: the MT19937 is already correct; the frontier is adding PCG64 (numpy's default RNG since 1.17) and Philox (JAX's default) behind a `Generator` class matching numpy's `numpy.random.Generator` API — this is the data science blocker (numpy's `rng.standard_normal()` uses PCG64).

### Scores

Data science: importance 2, gap 1. Engineering: importance 2, gap 1. Web: importance 1, gap 1. ML: importance 2, gap 1.

**Weighted product: 10 / 48.**

### Dependency Edges

Random: numpy Generator-compatible API blocks on numpy interop more broadly (Subsystem 2 zero-copy spine). Decimal: independent. Math determinism: independent.

### Verdict: ADEQUATE for the current phase. The MT19937 is complete and correct. Decimal has a working pure-Rust path. The PCG64/Philox Generator class is a **NEEDS-FIX-ONLY** task (implement in `random_mod.rs` without a new design doc) once numpy interop is unblocked.

---

## SUBSYSTEM 8: Datetime / Zoneinfo

### Current State

**datetime.** `src/molt/stdlib/datetime.py` — 33KB, doc 16: "basic classes/methods OK; zoneinfo integration pending." The 33KB size suggests a substantive pure-Python implementation, not a stub.

**zoneinfo.** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/zoneinfo.rs:1-60+` — a real implementation. Reads IANA TZif v1/v2/v3 binary data from `/usr/share/zoneinfo`. Has `Transition` structs, UTC offset, DST flag, abbreviation index. The satellite `molt-runtime-zoneinfo/` has three files (`zoneinfo.rs`, `bridge.rs`, `lib.rs`). Feature-gated via `stdlib_zoneinfo` in `stdlib_full`.

However, doc 16 says `zoneinfo/_zoneinfo.py`, `_tzpath.py`, `_common.py` "all marked TODO(stdlib-parity, milestone:SL3, status:planned)". This is a contradiction: the Rust implementation exists, but the Python-facing API surface in the stdlib wrapper is incomplete or not wired through.

**Integration gap.** The `zoneinfo.rs` reads raw TZif files but the Python `zoneinfo.ZoneInfo(key)` → `datetime.astimezone()` flow requires the datetime class to accept `tzinfo` objects with `utcoffset()`, `tzname()`, `dst()` methods. This wiring is the likely missing piece: the Rust `molt_zoneinfo_*` intrinsics exist, but the Python `ZoneInfo` class that wraps them and implements the `tzinfo` protocol is the stub.

### Frontier Definition

The `tzdata` package (Python 3.9+) ships IANA time zone data as a Python package fallback when `/usr/share/zoneinfo` is absent (WASM, Windows, containerized deploys). The frontier for WASM and edge deploys is bundling tzdata in the VFS or providing a build-time snapshot. For performance: in-memory TZif parsing with hash-indexed lookup (already partially done in `zoneinfo.rs` via `OnceLock` + `HashMap`). The gap-closing task is: complete the Python `ZoneInfo` class surface, wire `utcoffset()`/`tzname()`/`dst()` through the Rust intrinsics, and add tzdata fallback for WASM.

### Scores

Data science: importance 2, gap 2. Engineering: importance 2, gap 2. Web: importance 3, gap 2. ML: importance 1, gap 1.

**Weighted product: 18 / 48.**

### Dependency Edges

WASM tzdata bundling depends on VFS bundle-tar feature (already present in Cargo.toml line 141). `datetime` class is largely independent of compiler-foundation arcs.

### Verdict: NEEDS-FIX-ONLY. The Rust backend exists; this is a Python-wiring and integration task, not a fundamental redesign. No new frontier doc needed. Track as a fix task against `stdlib_zoneinfo` + `datetime.py` integration.

---

## SUBSYSTEM 9: Logging Throughput (Discovered)

### Current State

`/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/functions_logging.rs:1-60+` — `molt_logging_percent_style_format` is a Rust intrinsic doing percent-style format string expansion (`%s`, `%d`, `%r`, `%f`, etc.) including mapping-key lookup (`%(key)s`). The logging module is in `stdlib_server` tier (line 73 of Cargo.toml: `stdlib_logging_ext` in `stdlib_server`). `logging.config` and `handlers` are partial per doc 16.

The intrinsic-backed percent formatter is a meaningful optimization for high-throughput logging (avoids Python-level string parsing on every log call). The gap: structured logging (JSON log records for log aggregators), `logging.config.dictConfig` completeness, `RotatingFileHandler`/`TimedRotatingFileHandler` in `handlers`.

### Scores

Data science: importance 1, gap 1. Engineering: importance 3, gap 2. Web: importance 3, gap 2. ML: importance 1, gap 1.

**Weighted product: 16 / 48.**

### Verdict: ADEQUATE for basic logging. NEEDS-FIX-ONLY for `handlers` and `dictConfig`. No frontier doc needed.

---

## SUBSYSTEM 10: CSV (Discovered)

### Current State

`stdlib_csv = ["stdlib_serial"]` in Cargo.toml (line 111). `runtime/molt-runtime-serial/src/csv.rs` owns `molt_csv_runtime_ready()`, and the generated serial CSV resolver routes `molt_csv_*` symbols to that crate. A drift bug was found and fixed in past sessions (csv.rs:2008 gating, per MEMORY.md). The `_csv.py` (listed in stdlib) at `src/molt/stdlib/_csv.py` is likely the low-level C-module replacement.

### Scores

Data science: importance 3, gap 1. Engineering: importance 2, gap 1. Web: importance 1, gap 1. ML: importance 1, gap 1.

**Weighted product: 10 / 48. ADEQUATE.**

---

## SUBSYSTEM 11: sqlite3 (Discovered)

### Current State

`sqlite = ["dep:molt-db", "molt-db/sqlite"]` (Cargo.toml line 124) — bundled SQLite amalgamation via `rusqlite`. Doc 16: "partial — most ops OK; some edge cases pending." In `stdlib_full` tier only.

### Scores

Data science: importance 2, gap 1. Engineering: importance 3, gap 2. Web: importance 2, gap 2. ML: importance 1, gap 1.

**Weighted product: 16 / 48.**

### Verdict: NEEDS-FIX-ONLY. No frontier doc. The structural question is whether sqlite should be available in `stdlib_standard` (currently only in `stdlib_full`). Track as a fix task.

---

## SUBSYSTEM 12: Free-Threading Pillar / PEP 703 + PEP 734

This was broken out as a top-level topic in the mandate and has its own depth beyond the threading module. The structural question is: what does molt's memory model commit to for concurrent execution?

**Current state (precise).** The GIL (global `Mutex<()>`) serializes all Python bytecode execution across all threads. The refcounts are `AtomicU32` (correct for signal-handler safety, not needed for multi-thread safety under the GIL). The `ObjectLock` stub in `object/gil.rs` is Phase 1 infrastructure for per-object locking but is completely disconnected from any execution path. `GIL_RELEASED: AtomicBool` is never set by production code.

The implication: for PEP 703 free-threading, molt would need to (1) audit every mutable operation on heap objects (list append, dict set, object setattr) and replace GIL-protected batch ops with fine-grained `ObjectLock` acquisitions, (2) make RC inc/dec safe without the GIL (biased RC — thread-local fast path, global CAS on cross-thread share), (3) re-enable the specializing adaptive interpreter for the no-GIL path. This is a multi-month structural arc.

The intermediate value is PEP 734 (subinterpreters): each interpreter gets its own GIL and heap. This is closer to molt's current model (the isolate system in `concurrency/isolates.rs` already spawns threads with independent GilGuards). The design gap is the channel/queue communication protocol between isolates.

**Verdict: NEEDS-FRONTIER-DOC-NOW.** Scope this as the same document as Subsystem 1 (Threading Concurrency Model). Doc 28 should cover: (a) the multiprocessing spawn-for-AOT protocol, (b) the subinterpreter channel model (PEP 734), (c) a decision on the free-threading roadmap with a realistic timeline given the RC substrate work.

---

## SEQUENCED PROGRAM

### Active in-flight docs (context for sequencing)

- Doc 26: Real async generators (in-flight, blocks ASGI/streaming)
- Doc 27: asyncio audit + event-loop improvements (in-flight)
- Foundation arcs: E1-activation, S5-MemSSA, S6-SCEV, RC-ownership (#20) — all landing or planned

### Commission-now list (with dependencies noted)

**Doc 28 — Threading Concurrency Model & Multiprocessing Spawn Protocol.** Commission immediately. Inputs: current concurrency/isolates.rs + concurrency/gil.rs + threading.py + object/gil.rs Phase 1 stub. Deliverable: (a) decision on GIL-per-interpreter vs. per-object locking vs. message-passing-only, (b) multiprocessing spawn protocol for AOT binaries (re-exec entry-point manifest), (c) subinterpreter channel design (PEP 734 fidelity), (d) free-threading roadmap decision (deferred to post-RC-ownership arc). Benchmark lane: `threading.Thread` creation latency vs CPython, `ThreadPoolExecutor` throughput on CPU-bound workloads (embarrassingly parallel), comparison vs `multiprocessing.Pool` on spawn. Dependency: independent of compiler arcs but must wait for RC-ownership (#20) final design before committing to biased-RC.

**Doc 29 — Zero-Copy Spine (Buffer Protocol + DLPack + C-ABI Buffer Bridge).** Commission immediately, after `#49 C-ABI decision` is resolved or simultaneously with it. Inputs: `ops_memoryview.rs` (current state), `cpython-abi/src/api/buffer.rs` (always-returns-minus-one), `molt_buffer_export` C struct, `tensor.py` (no `__dlpack__`). Deliverable: (a) `PyObject_GetBuffer` returning real memory pointers for bytes/bytearray/memoryview/Tensor, (b) `Py_buffer` shape/stride/format population, (c) `__dlpack__`/`__dlpack_device__` on `molt.gpu.Tensor`, (d) `memoryview` released-view state modeling. Benchmark lane: numpy `np.frombuffer(molt_bytes)` zero-copy roundtrip, PyTorch `torch.from_dlpack(tensor.__dlpack__())` roundtrip latency and memory copy count. Dependency: C-ABI decision (#49) for the `PyObject_GetBuffer` surface.

**Doc 30 — Web Stack (HTTP/2 + ASGI Interface + WebSocket).** Commission after doc 26/27 are landed. Inputs: `event_loop.rs` (mio-backed, fd callbacks), `sockets.rs` (MoltSocket), `ssl.rs` (rustls), `runtime-http` (HTTP/1.1 only). Deliverable: (a) HTTP/2 multiplexed stream handling via h2 crate, (b) ASGI `(scope, receive, send)` dispatch layer over the mio event loop, (c) WebSocket upgrade integrated with fd callbacks via tungstenite, (d) `ssl.recv`/`ssl.send` with MSG_* flag support. Benchmark lane: HTTP/1.1 and HTTP/2 request throughput (reqs/sec at 1KB and 64KB body), WebSocket message throughput, comparison vs uvicorn/hypercorn on the same ASGI app. Dependency: doc 26 (async generators for streaming receive), doc 27 (event loop correctness).

**Doc 31 — mmap + File I/O Throughput.** Commission after Doc 29 (mmap must export a buffer protocol view; the design is shared). Inputs: `mmap.py` stub (301B), `io.rs` (8192 buffer, `std::fs`). Deliverable: (a) `mmap.mmap` backed by `libc::mmap`/platform equivalent, exposing buffer protocol via `PyObject_GetBuffer`, (b) `os.sendfile` intrinsic, (c) `copy_file_range` intrinsic on Linux/macOS, (d) async file read path using the mio event loop's readiness notification or a dedicated io_uring backend. Benchmark lane: mmap sequential scan throughput vs `f.read()`, `sendfile` throughput vs `read`+`write` for HTTP file serving, `copy_file_range` vs `shutil.copy`. Dependency: Doc 29 for buffer protocol.

**Doc 32 — Regex Engine (regex crate integration, two-tier NFA/backtrack).** Commission immediately; fully independent. Inputs: `molt-runtime-regex/Cargo.toml` (no regex crate today), `regex.rs` (lookaround helpers), Python re module (4.6KB wrapper with NotImplementedError on advanced features). Deliverable: (a) `regex` crate added to `molt-runtime-regex`, (b) `MoltPattern` handle type (compiled NFA/DFA, thread-safe via `Regex::new`), (c) `group capture` extraction mapping to Python `Match` object, (d) two-tier routing: patterns without backreferences go to `regex` crate, patterns with backreferences go to a backtracking engine (can use a smaller crate like `fancy-regex` which uses `regex` internally plus backtracking), (e) flag mapping (`IGNORECASE`, `MULTILINE`, `DOTALL`, `ASCII`, `VERBOSE`, `UNICODE`). Benchmark lane: throughput on typical web log parsing patterns (1MB/s → target >500MB/s), comparison vs CPython `re`, comparison vs `re2` bindings. Dependency: independent.

### Fix-only tasks (no new design doc; tracked as implementation tickets)

- **Zoneinfo / datetime wiring**: Complete the Python `ZoneInfo` class surface in `stdlib_zoneinfo`, wire `utcoffset()`/`tzname()`/`dst()` through the existing Rust TZif reader, add tzdata bundle for WASM. Estimated: medium effort.
- **json SIMD string escaping**: Replace the string escaping loop in the json fast-path with `simdutf`-backed scanning (the crate is already in Cargo.toml). Estimated: small effort.
- **ssl MSG_* flags**: Implement `MSG_PEEK` / `MSG_OOB` send/recv flags in `ssl.rs` via rustls API or raw socket operations. Estimated: small effort.
- **PCG64/Philox Generator class**: Add numpy-compatible `Generator` class to `random_mod.rs` implementing `standard_normal()`, `integers()`, `random()` with PCG64 and Philox64x4 bit generators. Estimated: medium effort.
- **logging handlers + dictConfig**: Implement `RotatingFileHandler`, `TimedRotatingFileHandler`, `SocketHandler` and complete `dictConfig` parsing. Estimated: medium effort.
- **sqlite in stdlib_standard tier**: Evaluate binary-size impact of moving `sqlite = ...` up from `stdlib_full` to `stdlib_standard`. Small if the amalgamation can be feature-gated by profile.
- **pickle protocol 5 buffer handoff**: Verify that `PickleBuffer(molt_tensor)` correctly propagates to `pickle.dumps(protocol=5)` with out-of-band buffers, and that `loads` with `buffers=` kwarg works for GPU tensor reconstruction. Medium effort, no new design needed.
- **marshal completeness**: Implement full marshal protocol (codes `l`, `y`, `c`, `z`, `<`, `>` etc.) for CPython cross-process object transfer. Small-medium effort.

### Dependency-ordered timeline sketch

Phase 0 (now, parallel to E1-activation + S5-MemSSA): Commission docs 28 and 32. Both are independent of compiler arcs. Doc 32 (regex) can be implemented immediately after commission. Doc 28 (threading) is a design decision document first.

Phase 1 (after #49 C-ABI decision + doc 26/27 landing): Commission doc 29 (zero-copy spine). This is the gate for numpy interop and ML framework exchange.

Phase 2 (after doc 29 delivered): Commission doc 31 (mmap + file I/O). mmap needs the buffer protocol work from doc 29.

Phase 3 (after doc 26/27 fully landed): Commission doc 30 (web stack). The ASGI layer needs real async generators.

Phase 4 (after RC-ownership arc #20 is complete): Return to doc 28 to add the free-threading roadmap section, now that the RC substrate design is stable enough to reason about biased-RC.

### The top-5 commission-immediately list

1. **Doc 28: Threading Concurrency Model & Multiprocessing Spawn Protocol** — The concurrency decision is load-bearing for every web and engineering workload. The multiprocessing spawn gap makes molt unusable for data-science patterns that use `ProcessPoolExecutor`. This decision must be made before any threading-adjacent work compounds the design space.

2. **Doc 32: Regex Engine** — A `re.compile(r'(?P<name>...)').match(s).group('name')` raising `NotImplementedError` is a hard blocker for data science (log parsing, data extraction), web (URL routing, input validation), and engineering (config parsing). The fix is structurally contained (one satellite crate, no compiler-arc dependencies) and the `regex` crate is production-grade. This is the fastest frontier win available.

3. **Doc 29: Zero-Copy Spine (Buffer Protocol + DLPack + C-ABI Buffer Bridge)** — `PyObject_GetBuffer` returning -1 makes numpy, pandas, PyArrow, and every C extension that touches array data structurally unusable. `__dlpack__` absence blocks JAX/PyTorch tensor exchange. This is the single highest-leverage ML and data science fix in the codebase.

4. **Doc 30: Web Stack (HTTP/2 + ASGI)** — HTTP/2 and ASGI are the baseline for any modern Python web deployment. uvicorn-class performance requires both. This is blocked on doc 26/27 (async generators) but should be commissioned now so the design is ready when the dependency lands.

5. **Doc 31: mmap + File I/O Throughput** — mmap is a 301B stub. Data-science patterns like `np.memmap`, Arrow file readers, and any ML framework that uses memory-mapped model weights are completely broken. The implementation is straightforward (libc binding + buffer protocol) once Doc 29's buffer protocol work is in hand.

---

## Cross-cutting observations

**RC substrate and threading.** The `AtomicU32` refcount (using `Relaxed` for inc, `Release` for dec, `Acquire` fence at zero) is correct for the current GIL model but is not safe for free-threading without biased-RC or a different memory model. This means Doc 28 must not commit to free-threading until the RC ownership arc (#20) completes.

**C-ABI decision gate (#49).** The `PyObject_GetBuffer` fix in doc 29 is the most visible consequence of the C-ABI decision. If molt commits to a translation layer (NaN-boxed bits → PyObject header on entry to cpython-abi calls), then `GetBuffer` can return a real pointer into molt's object memory. If molt commits to a shared layout, the fix is simpler. Either way, the buffer subsystem needs explicit design.

**StateDispatch (#24) and async generator intersection.** The ASGI receive/send design in doc 30 depends on StateDispatch providing the `async for item in receive_channel()` pattern, which is exactly what doc 26 (real async generators) delivers. The commissioning order is correct: doc 26 → doc 27 → doc 30.

**WASM threading gap.** `threading.Event.wait` calls `crate::molt_event_wait` which in `threading_helpers.rs` always returns immediately (no-op for WASM). The event loop is the correct WASM concurrency model, not OS threads. Doc 28 must specify the WASM threading surface explicitly (either stub-all-blocking-primitives as no-ops, or implement single-threaded simulation via the event loop's `call_later` timer).

---

Sources:
- [PEP 703 – Making the Global Interpreter Lock Optional in CPython](https://peps.python.org/pep-0703/)
- [PEP 779 – Criteria for supported status for free-threaded Python](https://peps.python.org/pep-0779/)
- [Python 3.14 Free-Threading True Parallelism Without the GIL](https://dev.to/edgar_montano/python-314-free-threading-true-parallelism-without-the-gil-a12)
- [What's new in Python 3.14](https://docs.python.org/3/whatsnew/3.14.html)
- [Free-Threaded Python Unleashed – Real Python July 2025](https://realpython.com/python-news-july-2025/)
- [Rust regex crate performance discussion](https://github.com/rust-lang/regex/discussions/960)
- [RE#: High Performance Derivative-Based Regex Matching (ArXiv)](https://arxiv.org/pdf/2407.20479)
- [orjson GitHub – Fast Rust-backed Python JSON](https://github.com/ijl/orjson)
- [orjson vs json 2025 comparison](https://morethanmonkeys.medium.com/comparing-json-and-orjson-in-python-which-json-library-should-you-use-in-2025-850cd39ecb7d)
- [Data interchange mechanisms – Python array API standard 2025](https://data-apis.org/array-api/latest/design_topics/data_interchange.html)
- [DLPack Python specification](https://dmlc.github.io/dlpack/latest/python_spec.html)
- [The DLPack Protocol – Apache Arrow](https://arrow.apache.org/docs/python/dlpack.html)
- [Python ASGI server landscape 2026](https://www.deployhq.com/blog/python-application-servers-in-2025-from-wsgi-to-modern-asgi-solutions)
- [FastAPI Engine: Inside Uvicorn ASGI server](https://leapcell.io/blog/uvicorn-performance-python-asgi)
- [Zero-Copy Data Sharing with DLPack](https://apxml.com/courses/advanced-jax/chapter-5-jax-interoperability-custom-operations/zero-copy-dlpack)
- [zoneinfo – IANA time zone support (Python docs)](https://docs.python.org/3/library/zoneinfo.html)
