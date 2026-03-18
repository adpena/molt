# STATUS (Canonical)

Last updated: 2026-03-01

This document is the source of truth for Molt's current capabilities and
limitations. Update this file whenever behavior or scope changes, and keep
README and [ROADMAP.md](../../ROADMAP.md) in sync.

## Strategic Target
- Performance: reach parity with or exceed Codon on representative native and
  wasm-relevant workloads.
- Coverage/interoperability: approach Nuitka-level CPython surface coverage and
  ecosystem interoperability, while preserving Molt vision constraints
  (determinism, explicit capabilities, and no implicit host-Python fallback).

## Rust-First Stdlib Lowering Sprint (2026-03-01)
- Completed: **SIMD Expansion** — 20+ runtime operations now have explicit SSE2/AVX2/NEON
  fast paths (+1,133 lines across ops.rs and math.rs):
  - String/bytes equality: `simd_bytes_eq` (16B SSE2, 32B AVX2, 16B NEON) wired into
    `string_bits_eq`, `molt_string_eq`, `obj_eq` (string/bytes/cross-type paths)
  - Byte-level lexicographic comparison: `simd_find_first_byte_diff` prefix-skip for
    `compare_string_bytes` and `compare_bytes_like`
  - Sequence element comparison: `simd_find_first_mismatch` (2×u64 SSE2, 4×u64 AVX2,
    2×u64 NEON) wired into `compare_sequence`, `obj_eq` (tuple/list equality)
  - Float vector sum: `sum_f64_simd_*` (2×f64 SSE2, 4×f64 AVX2, 2×f64 NEON) replaces
    `sum_floats_scalar` in `molt_vec_sum_float`/`molt_vec_sum_float_trusted`
  - ASCII case conversion: `bytes_ascii_upper`/`lower`/`swapcase`/`capitalize` all SIMD-
    accelerated with 16-byte NEON/SSE2 chunks (bit-5 set/clear/toggle)
  - ASCII predicates: `bytes_ascii_islower`/`isupper` SIMD bulk range checking
  - Hash computation: `simd_max_byte_value` (SSE2/AVX2/NEON) for ASCII fast path in
    `hash_string_bytes` — skips char-by-char iteration for pure ASCII
  - `str.lower()`/`str.upper()`: ASCII fast path delegates to SIMD `bytes_ascii_lower`/
    `bytes_ascii_upper` instead of `to_lowercase()`/`to_uppercase()` for pure ASCII
  - `math.dist`: SIMD squared-difference sum (NEON fmaq, AVX2 mul+add, SSE2 mul+add)
    with sqrt, fallback to iterative hypot for inf/nan
  - `math.hypot` (multi-arg): SIMD sum-of-squares (NEON fmaq, AVX2/SSE2 mul+add)
  - `.cargo/config.toml`: `target-cpu=native` for non-WASM targets enables full Apple
    Silicon NEON and x86-64 AVX2/AVX-512 auto-vectorization
- Completed: **Cranelift Codegen Optimization** — full Cranelift 0.128 feature exploitation:
  - Cold block marking: 36 slow-path blocks + exception handlers marked cold for better
    i-cache/branch-prediction layout
  - MemFlags::trusted(): 34 load/store sites upgraded from MemFlags::new() to trusted()
    (notrap + aligned), allowing Cranelift to elide redundant trap checks
  - Alias analysis enabled: redundant-load elimination across basic blocks
  - CFG metadata emission: enables downstream profilers/tools
  - Colocated libcalls: skip GOT/PLT indirection for direct PC-relative calls
  - CPU feature auto-detection: `builder_with_options(true)` enables AVX2/SSE4.2/BMI2/
    POPCNT on x86, NEON/AES/CRC on aarch64 for Cranelift-generated code
  - Inline stack probing: avoids function call overhead for stack probes
  - Spectre mitigations disabled: trusted-code compilation, no sandbox overhead
  - Frame pointer omission in release builds: frees rbp/x29 register
- Completed: **SIMD Phase 2 Expansion** — additional SIMD-accelerated operations:
  - Hex encode/decode: NEON vqtbl1q_u8 / SSE2 _mm_shuffle_epi8 lookup-table hex conversion
    (16 bytes → 32 hex chars per iteration) in binascii
  - Hex decode: NEON/SSE2 parallel nibble validation + conversion (32 hex chars → 16 bytes)
  - Base64 whitespace stripping: SIMD bulk whitespace classification (5-way compare) for
    base64 decode preprocessing
  - Single-byte replace: NEON vbslq_u8 / AVX2 _mm256_blendv_epi8 / SSE2 _mm_blendv_epi8
    conditional byte replacement (16-32 bytes per iteration)
  - ASCII whitespace detection: SIMD `simd_is_all_ascii_whitespace` for bytes.isspace(),
    bytearray.isspace(), str.isspace() (ASCII fast path)
  - Whitespace split: NEON variant added to `find_ascii_split_whitespace` (was SSE2-only)
  - Strip fast-skip: SIMD left-strip 16-byte chunk skipping for bytes.strip()
- Completed: **SIMD Phase 5–7 Expansion** — comprehensive SIMD coverage push:
  - Phase 5: SIMD `bytes_ascii_title` (NEON/SSE2 alpha classification + word-boundary tracking),
    ASCII fast-path for `str.title()`, SIMD hex encoding for `bytes.hex()`/`bytearray.hex()`
    (NEON vqtbl1q_u8 / SSE2 shuffle), SIMD `b16_encode` in base64 module, 4× unrolled
    `b64_encode`, SIMD `b64_decode` filter (NEON/SSE2 valid-char classification), SIMD
    `qp_encode` passthrough scan
  - Phase 6: Hardware CRC32 for `zlib.crc32` (aarch64 __crc32d 8B/instruction, x86_64
    _mm_crc32_u64 SSE4.2), optimized Adler-32 with chunked NMAX=5552 + 16× unrolled inner
    loop, memchr-based `qp_decode` with bulk extend_from_slice, memchr2-based `splitlines`,
    SIMD JSON `scanstring_decode` safe-ASCII scan, SIMD JSON `ensure_ascii` bulk copy,
    SIMD whitespace split helpers (`find_next_ascii_whitespace`/`skip_ascii_whitespace`
    with NEON 6-way vceqq/SSE2 cmpeq+movemask)
  - Phase 7: memchr-based `array.count`/`array.index` for byte typecodes (B/UB),
    memchr2-based HTML tokenizer data scanning for '<'/'&'
- Completed: **SIMD Phase 3+4 Expansion** — string/bytes predicate acceleration:
  - String predicates (ASCII fast path): `isdigit`/`isdecimal` (SIMD '0'-'9' range),
    `isalpha` (SIMD [A-Za-z] via OR-0x20), `isalnum` (combined alpha+digit),
    `islower`/`isupper` (SIMD has-any-upper/lower scan), `isprintable` (SIMD [0x20..0x7E])
  - Bytes predicates: `isalpha`/`isalnum`/`isdigit` all use SIMD bulk classification
  - Case conversion: `str.swapcase()` → SIMD `bytes_ascii_swapcase`,
    `str.capitalize()` → SIMD `bytes_ascii_capitalize` for pure-ASCII strings
  - `ascii()`: SIMD non-ASCII byte scan + bulk prefix copy
  - JSON encoding: SIMD safe-character scan (16B NEON/SSE2) skips bulk ASCII runs
- Completed: `stringprep` module — new 719-line Rust module (`stringprep.rs`) with all 17
  RFC 3454 table membership intrinsics (a1, b1, c11-c9, d1, d2) as code point range checks,
  `map_table_b3` with 47 exception entries for case folding. 13 unit tests. Intrinsic-backed
  Python wrapper (122 lines). 2 new manifest intrinsics.
- Fixed: Async generator StopAsyncIteration → RuntimeError conversion (PEP 479 analog) —
  StopAsyncIteration raised inside async gen body was propagating as-is (wrong); now
  correctly converts to RuntimeError with informative message.
- Completed: `re` Phase 1 Rust parser — 2,586-line recursive-descent regex parser in
  `regex.rs` with CompiledPattern, ReNode enum (13 variants), global handle registry.
  4 new intrinsics (`molt_re_compile`, `molt_re_execute`, `molt_re_finditer_collect`,
  `molt_re_pattern_info`). Supports full Python regex syntax: groups, backreferences,
  lookahead/lookbehind, character classes, quantifiers, alternation, scoped flags.
- Completed: `re` Phase 1b backtracking NFA match engine — 1,100-line continuation-passing
  engine with `MatchState` + `try_match` recursion. Supports all ReNode variants: literals,
  character classes, quantifiers (greedy/lazy), groups with capture, backreferences,
  lookahead/lookbehind, anchors, word boundaries. `molt_re_execute` and
  `molt_re_finditer_collect` now fully implemented (no longer stubs). 60/60 tests pass.
  Fixed `strip_verbose` to respect VERBOSE flag.
- Completed: libmolt C-API Phase 1+2 — 117 CPython C-API functions, 80 tests, all passing.
  Phase 1: PyList, PyDict, PyTuple, iterator protocol, type checks.
  Phase 2: Object Protocol (Repr/Str/Hash/IsTrue/Not/Type/Length/GetAttr/SetAttr/DelAttr/
  HasAttr/RichCompare/RichCompareBool/IsInstance/IsSubclass/CallableCheck), Number Protocol
  (Add/Sub/Mul/TrueDivide/FloorDivide/Remainder/Power/Neg/Pos/Abs/Invert/Lshift/Rshift/
  And/Or/Xor/Check/Long/Float), Mapping Protocol (Length/Keys/Values/Items/GetItemString/
  HasKey), Set Protocol (New/FrozenSetNew/Size/Contains/Add/Discard/Pop/Clear/Check/
  FrozenSetCheck), Sequence Protocol (GetItem/Length/Contains), Bytes/String
  (FromStringAndSize/AsString/Size/FromString/AsUTF8/AsUTF8AndSize), Unicode additions
  (GetLength/Concat/Contains/CompareWithASCIIString), Dict additions
  (GetItemString/DelItem/DelItemString/Keys/Values/Items/Update/Copy), List additions
  (Insert/Sort/Reverse/AsTuple), Exception Protocol (SetString/SetNone/Occurred/Clear/
  NoMemory), RefCount (IncRef/DecRef/XINCREF/XDECREF), Conversions
  (Long_AsLong/FromLong/Float_AsDouble/FromDouble/Bool_FromLong/BuildNone), Memory
  (PyMem_Malloc/Realloc/Free, PyObject_Malloc/Realloc/Free). Total c_api.rs: 6,561 lines.
- Completed: asyncio Barrier rewrite (wait returns index 0..parties-1, abort, reset wakes
  with BrokenBarrierError, parties/n_waiting properties, async context manager),
  Semaphore __aenter__/__aexit__, Server methods (is_serving, start_serving, serve_forever,
  get_loop, close_clients, abort_clients), Queue maxsize/__repr__/__class_getitem__,
  BrokenBarrierError export in locks module, __repr__ on all synchronization primitives.
- Completed: tkinter Toplevel inherits Wm mixin (P0 blocker), _splitdict return type fix,
  Entry.bbox/validate, Spinbox.validate, Misc._root/Widget._root, grid_children/
  place_children aliases, Tk._windowingsystem stored, PhotoImage.data, Menu.entryindex,
  Font.__del__, ttk Combobox.get, Spinbox.get, Scale.set, Panedwindow.add, Treeview.xview/
  yview.
- Intrinsics audit (full subdirectory scan): 2,193 total, 1,838 Python-wired, 355
  Rust-internal, zero genuinely unwired.
- Completed: asyncio `staggered_race` — full CPython 3.12-faithful implementation
  replacing stub. Uses mutable list cells instead of nonlocal (Molt compiler constraint),
  try/except instead of contextlib.suppress.
- Completed: tkinter WASM import gate — `emscripten`/`wasi` platforms get `ImportError`
  at import time, matching CPython behavior.
- Cleanup: deleted 26 dead intrinsics (6 sys.bootstrap, 15 importlib granular, 5 dead
  singletons). Wired 5 singleton intrinsics (copy_error, datetime isoformat date/time,
  encodings aliases_map, http parse_header_pairs). Fixed tkinter Image.__getitem__
  (configure→cget), ScrolledText rewrite (proper Text subclass), Font.__eq__/__hash__,
  tix deprecation warning, asyncio Semaphore dual-state fix.
- Completed: `concurrent.futures` — wired all 17 `molt_concurrent_*` intrinsics.
  ThreadPoolExecutor and Future now use Rust handle-based pattern. `submit()`,
  `result()`, `exception()`, `done()`, `cancel()`, `add_done_callback()`,
  `wait()`, `as_completed()` all delegate to Rust. Replaced 661-line Python
  reimplementation. Dual-mode Future supports both Rust-backed (from submit) and
  Python-managed (standalone construction) paths.
- Completed: `asyncio` event loop RT2 — wired 28 `molt_event_loop_*` intrinsics
  into _EventLoop class. call_soon/call_later/call_at, add_reader/remove_reader,
  add_writer/remove_writer, run_once (hot path), time, start/stop, is_running/
  is_closed/close, debug mode, exception handler, task factory all delegate to
  Rust-owned event loop handle. _run_once() now executes entirely in Rust.
- Completed: `asyncio` Queue waiters — wired 6 intrinsics (add_getter/putter,
  notify_getters/putters, getter_count/putter_count). Eliminated Python-side
  `_getters`/`_putters` deques that duplicated Rust VecDeque state — fixes
  potential cross-thread correctness risk (same class of bug as Feb deque fix).
- Completed: `codecs` StreamReader/StreamWriter — wired 7 `molt_codecs_stream_*`
  intrinsics. Both classes now use Rust handle-based pattern with __del__ cleanup.
- Completed: `re` quick wins — wired `molt_re_strip_verbose` (VERBOSE pre-processing
  eliminates per-character Python overhead) and `molt_re_fullmatch_check` (boundary
  check). No new Rust code needed.
- Cleanup: deleted 6 dead `molt_sys_bootstrap_*` intrinsics from manifest (superseded
  by aggregate `molt_sys_bootstrap_payload`). Regenerated generated.rs + _intrinsics.pyi.
- Audit: identified 15 unwired importlib intrinsics as dead — superseded by aggregate
  orchestration (`molt_importlib_find_spec_orchestrate`) or covered by more complete
  aggregate intrinsics already wired in Python. Dead intrinsics pending manifest deletion:
  `molt_importlib_source_loader_payload`, `molt_importlib_coerce_search_paths`,
  `molt_importlib_finder_signature`, `molt_importlib_path_importer_cache_signature`,
  `molt_importlib_existing_spec`, `molt_importlib_export_attrs`,
  `molt_importlib_find_spec_payload`, `molt_importlib_find_spec_from_path_hooks`,
  `molt_importlib_namespace_paths`, `molt_importlib_search_paths`,
  `molt_importlib_parent_search_paths`, `molt_importlib_runtime_state_payload`,
  `molt_importlib_runtime_state_view`, `molt_importlib_spec_from_file_location_payload`,
  `molt_importlib_metadata_entry_points_payload`.

- Completed: `base64` module rewired — all 18 public functions now delegate to
  existing `molt_base64_*` Rust intrinsics in `base64_mod.rs`. Removed ~400 lines
  of pure-Python encode/decode loops.
- Completed: `random` module — new `random_mod.rs` (1457 lines) with full Mersenne
  Twister engine, handle registry, and 21 intrinsics. Python `Random` class is now
  a thin handle wrapper. Only `binomialvariate` retains Python control flow (uses
  Rust-backed `random()` and math intrinsics).
- Completed: `heapq` module — 5 new Rust intrinsics (`heapify_max`, `heappop_max`,
  `nsmallest`, `nlargest`, `merge`) with proper heap algorithms. `nsmallest`/
  `nlargest` use genuine heap tournament; `merge` uses k-way heap merge (replaces
  naive Python sort fallbacks).
- Completed: `copy` module rewired — `copy()` and `deepcopy()` now delegate to
  `molt_copy_copy`/`molt_copy_deepcopy` Rust intrinsics. Removed ~350 lines of
  Python dispatch tables and traversal loops.
- Completed: `pprint` module rewired — `pprint`/`pformat`/`saferepr`/`isreadable`/
  `isrecursive` and `PrettyPrinter.pformat` now delegate to Rust intrinsics
  (`molt_pprint_pformat`, `molt_pprint_safe_repr`, etc.).
- Completed: `uuid` — replaced `_int_to_bytes` byte-by-byte loop with
  `int.to_bytes(length, "big")`, `_bytes_to_hex` with `data.hex()`.
- Completed: `json` — deleted dead `_walk_circular_markers` and
  `_validate_no_circular_references` pure-Python functions.
- Completed: `zlib` — wired all 27 `molt_zlib_*` intrinsics. Compress/Decompress
  classes are thin handle wrappers with `__del__` cleanup. 14 constants bootstrapped
  from Rust at import time.
- Completed: `ipaddress` — wired 30 `molt_ipaddress_*` intrinsics. IPv4Address/
  IPv6Address/IPv4Network use `__slots__ = ("_handle",)` handle pattern. Eliminated
  `_parse_ipv4`, `_parse_ipv6`, `_compress_ipv6` pure-Python implementations.
- Completed: `shutil` — wired 9 additional `molt_shutil_*` intrinsics (copy, copy2,
  copytree, move, disk_usage, get_terminal_size, chown, make_archive, unpack_archive).
- Completed: `subprocess` — wired 3 convenience intrinsics (`molt_subprocess_run`,
  `molt_subprocess_check_call`, `molt_subprocess_check_output`).
- Completed: `enum` — wired all 10 `molt_enum_*` Flag/auto/StrEnum/unique/verify
  intrinsics. Flag.__or__/__and__/__xor__/__invert__/__contains__ delegate to Rust.
  Added StrEnum class, @unique decorator, @verify decorator, flag_decompose for
  Flag iteration, FlagBoundary sentinels (CONFORM/EJECT/KEEP/STRICT/NAMED_FLAGS/UNIQUE).
- Completed: `warnings` — wired 8 `molt_warnings_*` intrinsics. `formatwarning`/
  `showwarning` delegate to Rust. `warn`/`warn_explicit` fast-path to Rust when no
  record/capture hooks. Eliminated dead inline regex engine (`_SimpleRegex`,
  `_simple_regex_match`, `_tokenize_pattern` — ~70 lines of dead code).
- Completed: `logging` — wired 31 Rust intrinsics (up from 1). LogRecord, Formatter,
  Handler, StreamHandler, Logger now use Rust handle-based pattern. Record creation,
  message formatting, handler emit/flush/close, logger level checks, effective level,
  basicConfig, shutdown, level name mapping all delegate to Rust. Python class interfaces
  preserved for subclassing compatibility. Remaining: FileHandler I/O path integration,
  QueueHandler/QueueListener (depend on queue intrinsics).
- Completed: `string` Template/Formatter — 5 new Rust intrinsics in `string_ext.rs`:
  `molt_string_template_scan`, `molt_string_template_is_valid`,
  `molt_string_template_get_identifiers`, `molt_string_formatter_parse`,
  `molt_string_formatter_field_name_split`. Eliminated ~290 lines of pure-Python
  parsing (identifier helpers, template scanner, formatter parser, field name splitter).
  Formatter._vformat stays as Python (user-subclassable API).
- Blocker: `encodings/punycode.py` (~210 lines RFC 3492), `encodings/idna.py` (~280
  lines RFC 3490), `encodings/uu_codec.py` (~55 lines) — zero Rust intrinsics exist.

## Compiler + WASM + Stdlib Hardening Sprint (2026-02-28)
- Completed: frontend `_guard_tag_for_hint` extended with `set` (17), `frozenset` (18),
  `intarray` (16) type tag mappings for guard emission.
- Completed: WASM `os.getppid()` now raises `OSError(ENOSYS)` instead of silently
  returning 0.
- Completed: WASM HTTP `Date:` header uses Howard Hinnant UTC algorithm instead of
  hardcoded epoch string.
- Completed: WASM `datetime.now()`/`fromtimestamp()`/`utcoffset()` now raise `OSError`
  when host timezone unavailable instead of silent UTC fallback.
- Completed: WASM `select.select()` breaks immediately instead of spin-looping
  (prevents host event loop freeze).
- Completed: WASM `threading.current_thread().ident` returns 1 (main thread) instead
  of 0.
- Completed: orphaned `complex_core.rs` deleted (26 handle-based intrinsics
  incompatible with canonical GC object model; complex type works via
  ops.rs/numbers.rs/attributes.rs).
- Completed: `re` engine now supports `\b` (word boundary), `\B` (non-boundary),
  `\A` (absolute start), `\Z` (absolute end) anchors. `(?:...)` non-capturing groups
  confirmed already working.
- Completed: `collections.ChainMap` added with 11 intrinsic bindings — unblocks
  `string.Template.substitute()`.
- Completed: `io.SEEK_SET`/`SEEK_CUR`/`SEEK_END` constants added.
- Completed: `os.DirEntry.stat()` and `inode()` methods added.
- Completed: `datetime` timedelta arithmetic (abs/truediv/floordiv/mod),
  `date.fromisocalendar()`, `datetime.combine()` wired.
- Completed: `typing` additions (`assert_type`, `assert_never`, `is_typeddict`,
  `LiteralString`, `get_overloads`, `clear_overloads`, `dataclass_transform`).
- Completed: `pathlib.Path.walk()` via `os.walk` delegation.
- Completed: `functools.cached_property` descriptor class added.

## Asyncio & Tkinter Parity Sprint (2026-02-28)
- Completed: asyncio pipe transports (`connect_read_pipe`/`connect_write_pipe`)
  implemented with 11 new pipe transport Rust intrinsics.
- Completed: 42 new Rust intrinsics for asyncio Future/Event/Lock/Semaphore/Queue
  state machines, eliminating all 97 bare `except` blocks from asyncio shim.
- Completed: WASM capability gating for 6 asyncio I/O operations
  (`connect_read_pipe`, `connect_write_pipe`, `create_unix_connection`,
  `create_unix_server`, `open_unix_connection`, `start_unix_server`).
- Completed: Transport/Protocol base classes added to asyncio surface.
- Completed: 3.13 version-specific APIs (`as_completed` async iter,
  `Queue.shutdown`) and 3.14 version-specific APIs (`get_event_loop`
  `RuntimeError`, child watcher removal, policy deprecation) added with
  explicit version gating.
- Completed: tkinter 10 Rust intrinsics wired (event parsing, Tcl list/dict
  conversion, hex color validation, option normalization).
- Completed: all tkinter strict mode violations resolved.
- Completed: tkinter 3.13 (`tk_busy_*`, `PhotoImage.copy_replace`) and 3.14
  (`trace_variable` deprecation) version-specific APIs added.
- Completed: tkinter 100% submodule coverage achieved.

## Stdlib Intrinsics Sprint (2026-02-25)
- Completed: major stdlib intrinsics sprint adding ~85 new Rust intrinsics across
  6 modules (~1,250 LOC Rust, ~1,600 LOC Python shim rewrites).
- Track A (os): wired ~25 existing intrinsics (`access`, `chdir`, `cpu_count`,
  `link`, `truncate`, `umask`, `uname`, `getppid`, `getuid`, `getgid`, `geteuid`,
  `getegid`, `getlogin`, `getloadavg`, `removedirs`, `devnull`,
  `get_terminal_size`, `walk`, `scandir`, `path.commonpath`,
  `path.commonprefix`, `path.getatime`/`getctime`/`getmtime`/`getsize`,
  `path.samefile`, `F_OK`/`R_OK`/`W_OK`/`X_OK`) and added ~15 new Rust
  intrinsics (`dup2`, `lseek`, `ftruncate`, `isatty`, `fdopen`, `sendfile`,
  `kill`, `waitpid`, `getpgrp`, `setpgrp`, `setsid`, `sysconf`,
  `sysconf_names`, `path.realpath`, `utime`).
- Track B (sys): added ~20 new intrinsics (`maxsize`, `maxunicode`,
  `byteorder`, `prefix`, `exec_prefix`, `base_prefix`, `base_exec_prefix`,
  `platlibdir`, `float_info`, `int_info`, `hash_info`, `thread_info`, `intern`,
  `getsizeof`, `stdlib_module_names`, `builtin_module_names`, `orig_argv`,
  `copyright`, `displayhook`, `excepthook`).
- Track C (_thread + signal): rewrote `_thread.py` with existing thread
  intrinsics (`allocate_lock`, `LockType`, `start_new_thread`, `exit`,
  `get_ident`, `get_native_id`, `_count`, `stack_size`, `interrupt_main`,
  `TIMEOUT_MAX`, `error`); extended signal with 12 new constant intrinsics
  (`SIGBUS` through `SIGSYS`) and 5 POSIX function intrinsics (`strsignal`,
  `pthread_sigmask`, `pthread_kill`, `sigpending`, `sigwait`).
- Track D (asyncio): expanded `_asyncio.py` with C-accelerated surface
  functions (`current_task`, `_enter_task`, `_leave_task`, `_register_task`,
  `_unregister_task`) backed by 4 new Rust intrinsics with runtime task-state
  management.
- Track E (subprocess): added `start_new_session`, `process_group` params,
  `pid` property, `send_signal` method, `check_call`, `getstatusoutput`,
  `getoutput` with a new `molt_process_spawn_ex` Rust intrinsic;
  `concurrent.futures` verified complete with no additional changes needed.

## Optimization Program Status (2026-02-24)
- Program state: Week 1 observability is complete, compile-throughput recovery is partial, and optimization execution is now managed by a control-plane-first swarm protocol.
- Execution assumption: optimization execution is active; Wave 0 baseline refresh + doc alignment is mandatory before broad Wave 2 rollout slices.
- Canonical optimization scope: [OPTIMIZATIONS_PLAN.md](../../OPTIMIZATIONS_PLAN.md).
- Canonical optimization execution log: [docs/benchmarks/optimization_progress.md](docs/benchmarks/optimization_progress.md).
- Current progress: runtime instrumentation + benchmark diff tooling are landed, baseline lock summary remains published at [bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md](bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md), and compile-time recovery policy/tiering/budgets/telemetry slices are partially implemented in frontend/CLI.
- 2026-02-25 Wave 0 release-lane triage: fixed release-runtime compile regression in `_asyncio` scheduler task-entry/task-exit helpers (`runtime/molt-runtime/src/async_rt/scheduler.rs`) by restoring correct `raise_exception` API usage; targeted validation is green (`cargo check -p molt-runtime`, `tests/test_tkinter_phase0_wrappers.py`).
- Active risk signal: frontend/mid-end compile throughput regressed on stdlib-heavy module graphs; deterministic wasm benchmark builds can timeout before runtime execution.
- Active Wave 0 blocker signal: native benchmark harness runs remain sensitive to interpreter environment and long-tail release-runtime rebuild churn (`uv` lane missing `packaging.markers`, repeated release rebuild stalls under contention); treat this as a tooling/perf-gate blocker until the refresh workflow is stabilized.
- Active governance policy for optimization merges:
  - perf + correctness + lowering gates are all required in the same change.
  - optimize-or-die behavior is prohibited: any red-line regression is a stop/rollback signal.
  - optimization docs (`STATUS`, `ROADMAP`, `OPTIMIZATIONS_PLAN`, `optimization_progress`) must stay synchronized in the same change when status semantics shift.
  - checker snapshot metric mode in this file is canonical for lowering scoreboards; historical plan snapshots must not override active gate semantics.

## Toolchain Port Tranche (2026-02-13)
- Implemented: backend toolchain port to latest requested major lines (`cranelift 0.128.x`, `wasm-encoder 0.245.1`, `wasmparser 0.245.1`) with compile/test parity green in `runtime/molt-backend`.
- Implemented: Cranelift 0.128 tuning adoption in backend defaults:
  - release builds now request `log2_min_function_alignment=4` (16-byte minimum alignment),
  - debug/dev builds now default to `regalloc_algorithm=single_pass` for compile-throughput,
  - explicit override knobs are available via `MOLT_BACKEND_REGALLOC_ALGORITHM`, `MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2`, and `MOLT_BACKEND_LIBCALL_CALL_CONV`.
- Implemented: worker SQL parser port to `sqlparser 0.61.x` with compatibility-preserving query-limit wrapping semantics in `runtime/molt-worker`.
- Implemented: linked wasm runner wiring fix in `run_wasm.js`; linked artifacts no longer unconditionally require `MOLT_RUNTIME_WASM` sidecar reads.
- Implemented: regression coverage for the linked-runner sidecar path in `tests/test_wasm_linked_runner_node_flags.py::test_run_wasm_linked_does_not_require_runtime_sidecar_when_linked`.
- Implemented: linked bench compile/run wiring fix in `tools/bench_wasm.py`; linked-mode builds now set `MOLT_WASM_TABLE_BASE` from reloc-runtime table imports, preventing linked `output_linked.wasm` call-indirect signature traps.
- Implemented: linked wasm runtime bootstrap now calls optional `molt_table_init` export before `molt_main` in `run_wasm.js`, matching passive-element initialization requirements.
- Implemented: regression coverage for linked table/signature path in `tests/test_wasm_linked_runner_node_flags.py::test_run_wasm_linked_bench_sum_has_no_table_signature_trap` and `tests/test_bench_wasm_node_resolver.py::test_prepare_wasm_binary_sets_linked_table_base`.
- Implemented: linked-wasm builtin metadata import wiring fix in
  `runtime/molt-backend/src/wasm.rs`; missing import ids for
  `sys_hexversion`/`sys_api_version`/`sys_abiflags`/`sys_implementation_payload`
  are now registered before wrapper/table emission, eliminating
  `missing builtin import for sys_hexversion` panic failures in targeted linked
  wasm bench runs.
- Implemented: wasm runtime artifact hardening in `src/molt/cli.py` now validates runtime wasm magic, retries with an isolated target dir on corrupt/zero-filled artifacts, and (when `release` wasm artifacts are invalid) falls back to `release-fast` via `MOLT_WASM_RUNTIME_FALLBACK_PROFILE` for deterministic linked-build recovery.
- Implemented: regression coverage for wasm artifact hardening in `tests/cli/test_cli_wasm_artifact_validation.py`, including corrupt-primary recovery and `release -> release-fast` fallback-profile behavior.
- Implemented: Rust 2024 `unsafe_op_in_unsafe_fn` hardening in `runtime/molt-runtime/src/async_rt/channels.rs` (explicit unsafe blocks + safety rationale comments).
- Implemented: Rust 2024 hardening follow-up in `runtime/molt-runtime/src/async_rt/generators.rs`; remaining `unsafe_op_in_unsafe_fn` hits are cleared for that module.
- Implemented: pickle parity tranche advanced in runtime core (`runtime/molt-runtime/src/builtins/functions.rs`) with reducer 6-tuple `state_setter` lowering plus VM `POP`/`POP_MARK` support; targeted native differential tranche is green (`10/10`) including new regressions `tests/differential/stdlib/pickle_reduce_state_setter.py` and `tests/differential/stdlib/pickle_main_function_global_resolution.py`.
- Implemented: pickle core now preserves default-instance graph semantics for class layout fields (`__molt_field_offsets__`) and CPython-like `BUILD` state ordering (`__dict__` merge + slot-state setattr), with reducer/copyreg precedence fixed before default-instance fallback. New regressions are green in native + wasm lanes: `tests/differential/stdlib/pickle_class_dataclass_roundtrip.py` and `tests/test_wasm_pickle_class_dataclass_roundtrip.py`.
- Implemented: capability-enabled runtime-heavy wasm blocker tranche is green in targeted regression lane (`tests/test_wasm_runtime_heavy_regressions.py`: `3/3` pass for asyncio task table-ref path, zipimport failure-shape parity, and deterministic smtplib wasm thread fail-fast).
- Implemented: native runtime-heavy cluster differential sweep is green (`119/119` pass across `_asyncio`, `smtplib`, `zipfile`, and `zipimport`) with RSS profiling + memory caps enforced.
- Implemented: native strict-closure differential slices for `re`/`pathlib`/`socket` are green (`102/102` pass) with RSS profiling + memory caps enforced.
- Implemented: targeted compression differential smoke (`bz2_basic`,
  `gzip_basic`, `lzma_basic`, `zlib_basic`) is green (`4/4`) with
  `MOLT_DIFF_MEASURE_RSS=1`, external-volume artifact roots, and per-process
  memory caps.
- Implemented: critical strict-import gate now includes `re` (checker + generated audit docs + regression test coverage).
- Implemented: postgres-boundary isolation for `fallible-iterator 0.2` via explicit alias dependency `fallible-iterator-02` in `runtime/molt-worker`; `0.3` remains on rusqlite paths.
- Temporary upstream exception: `fallible-iterator` remains dual-version in the graph because `tokio-postgres`/`postgres-protocol` currently pin `0.2` while `rusqlite` pins `0.3`.
- TODO(tooling, owner:runtime, milestone:TL2, priority:P1, status:partial): collapse dual `fallible-iterator` versions once postgres stack releases support `fallible-iterator 0.3+`; keep the boundary isolated/documented until upstream unblocks.

## Roadmap 90-Day Execution Artifacts (2026-02-12)
- Delivered Month 1 determinism/security enforcement checklist:
  [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md).
- Delivered Month 1 minimum must-pass Tier 0/1 + diff parity matrix:
  [docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md).
- Partial Month 1 core-spec finalization:
  sign-off readiness and implementation-alignment updates landed in
  [docs/spec/areas/core/0000-vision.md](docs/spec/areas/core/0000-vision.md) and
  [docs/spec/areas/compiler/0100_MOLT_IR.md](docs/spec/areas/compiler/0100_MOLT_IR.md); explicit owner sign-off pending.
- Partial Month 2 guard/deopt instrumentation wiring:
  runtime emits `molt_runtime_feedback.json` artifacts when
  `MOLT_RUNTIME_FEEDBACK=1` (path via `MOLT_RUNTIME_FEEDBACK_FILE`, default
  `molt_runtime_feedback.json`) and schema checks are gated via
  `tools/check_runtime_feedback.py`, including required
  `deopt_reasons.call_indirect_noncallable` and
  `deopt_reasons.invoke_ffi_bridge_capability_denied`, plus
  `deopt_reasons.guard_tag_type_mismatch` and
  `deopt_reasons.guard_dict_shape_layout_mismatch` with guard-layout
  mismatch breakdown counters (`*_null_obj`, `*_non_object`,
  `*_class_mismatch`, `*_non_type_class`,
  `*_expected_version_invalid`, `*_version_mismatch`).
- IR implementation coverage audit was added and linked:
  [docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md](docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md)
  (historical baseline snapshot: 109 implemented, 13 partial, 12 missing).
- Current inventory gate (`tools/check_molt_ir_ops.py`) reports
  `missing=0` for spec-op presence in frontend emit/lowering coverage, and
  required dedicated-lane presence in native + wasm backends, plus
  behavior-level semantic assertions for dedicated call/guard/ownership/
  conversion lanes.
- 2026-02-11 implementation update: frontend/lowering/backend now include
  dedicated lanes for `CALL_INDIRECT`, `INVOKE_FFI`, `GUARD_TAG`,
  `GUARD_DICT_SHAPE`, `INC_REF`/`DEC_REF`/`BORROW`/`RELEASE`, and
  conversions (`BOX`/`UNBOX`/`CAST`/`WIDEN`); semantic hardening and
  differential evidence remain in progress.
- Behavior-level lane regression tests are in
  `tests/test_frontend_ir_alias_ops.py` for raw emit + lowered lane presence
  (`call_indirect`, `guard_tag`, `guard_dict_shape`, ownership lanes, and
  conversion lanes).
- Differential parity evidence now includes dedicated-lane probes:
  `tests/differential/basic/call_indirect_dynamic_callable.py`,
  `tests/differential/basic/call_indirect_noncallable_deopt.py`,
  `tests/differential/basic/invoke_ffi_os_getcwd.py`,
  `tests/differential/basic/invoke_ffi_bridge_capability_enabled.py`,
  `tests/differential/basic/invoke_ffi_bridge_capability_denied.py`,
  `tests/differential/basic/guard_tag_type_hint_fail.py`, and
  `tests/differential/basic/guard_dict_shape_mutation.py`.
- CI enforcement update (2026-02-11): after `diff-basic`, CI now runs
  `tools/check_molt_ir_ops.py --require-probe-execution` against
  `rss_metrics.jsonl` + `ir_probe_failures.txt`, making required probe
  execution/failure-queue linkage a hard gate.
- `INVOKE_FFI` hardening update (2026-02-11): bridge-policy invocations are
  tagged in lowered IR (`s_value="bridge"`), backends call
  `molt_invoke_ffi_ic`, and runtime enforces `python.bridge` capability for
  bridge-tagged calls when not trusted.
- `CALL_INDIRECT` hardening update (2026-02-11): `call_indirect` now routes
  through dedicated native/wasm runtime lanes (`molt_call_indirect_ic` /
  `call_indirect_ic`) with explicit callable precheck before IC dispatch.
- Frontend mid-end update (2026-02-11): `SimpleTIRGenerator.map_ops_to_json`
  now applies a CFG/dataflow optimization pipeline prior to JSON lowering
  (check-exception coalescing + explicit basic-block CFG + dominator/liveness
  passes). This now includes deterministic fixed-point ordering
  (`simplify -> SCCP -> canonicalize -> DCE`) with sparse SCCP lattice
  propagation (`unknown`/`constant`/`overdefined`) over SSA names, explicit
  executable-edge tracking (edge-filtered predecessor merges), and SCCP folding
  for arithmetic/boolean/comparison/`TYPE_OF` plus constant-safe
  `CONTAINS`/`INDEX`, selected `ISINSTANCE` folds, and selected guard facts
  (including guard-failure edge termination). It now threads executable edges
  for `IF`/`LOOP_BREAK_IF_*`/`LOOP_END`/`TRY_*`, tracks try exceptional vs
  normal completion facts, applies deeper loop/try rewrites (including
  conservative dead-backedge loop marker flattening and dead try-body suffix
  pruning after proven guard/raise exits), and performs region-aware CFG
  simplification across
  structured `IF`/`ELSE`, `LOOP_*`, `TRY_*`, and `LABEL`/`JUMP` regions
  (including dead-label pruning and no-op jump elimination). A structural
  canonicalization step now runs before SCCP each round to strip degenerate
  empty branch/loop/try regions. The pass also includes conservative
  branch-tail merging, loop-invariant pure-op hoisting, effect-aware global CSE
  over pure/read-heap ops, and side-effect-aware DCE with strict protection of
  guard/call/exception/control ops. Expanded cross-block value reuse remains
  guarded by a CFG definite-assignment verifier and automatically falls back to
  the safe mode when proof fails. Read-heap CSE now uses conservative
  alias/effect classes (`dict`/`list`/`indexable`/`attr`) so unrelated writes
  do not globally invalidate read value numbers, including global reuse for
  `GETATTR`/`LOAD_ATTR`/`INDEX` reads under no-interfering-write checks.
  Read-heap invalidation now treats call/invoke operations as conservative
  write barriers, and class-level alias epochs are augmented with lightweight
  object-sensitive epochs for higher hit-rate without unsafe reuse.
  Exceptional try-edge pruning now preserves balanced `TRY_START`/`TRY_END`
  structure unless dominance/post-dominance plus pre-trap
  `CHECK_EXCEPTION`-free proofs permit marker elision.
  The CFG now models explicit `CHECK_EXCEPTION` branch targets and threads
  proven exceptional checks into direct handler `jump` edges with
  dominance-safe guards before unreachable-region pruning, and normalizes
  nested try/except multi-handler join trampolines (label->jump chains)
  before CSE rounds.
  analysis now tracks `(start, step, bound, compare-op)` tuples for affine
  induction facts and monotonic loop-bound proofs used by SCCP. It performs
  trivial `PHI`
  elision, proven no-op `GUARD_TAG` elision, and dominance-safe hoisting of
  duplicate branch guards, with preservation across structured joins, with
  regression coverage in
  `tests/test_frontend_midend_passes.py`.
  CFG construction is now centralized in
  `src/molt/frontend/cfg_analysis.py` (`BasicBlock`/`CFGGraph`) and mid-end
  acceptance counters are reportable with `MOLT_MIDEND_STATS=1`, including
  per-transform diagnostics (`sccp_branch_prunes`,
  `loop_edge_thread_prunes`, `try_edge_thread_prunes`,
  `unreachable_blocks_removed`, `cfg_region_prunes`, `label_prunes`,
  `jump_noop_elisions`, `licm_hoists`, `guard_hoist_*`, `gvn_hits`,
  `dce_removed_total`) plus function-scoped acceptance/attempt telemetry in
  `midend_stats_by_function` (`sccp`, `edge_thread`, `loop_rewrite`,
  `guard_hoist`, `cse`, `cse_readheap`, `gvn`, `licm`, `dce`, `dce_pure_op`)
  with attempted/accepted/rejected breakdown for transform families.
- Compile-time stabilization tranche (2026-02-12): core implementation is now
  partially landed for profile-gated optimization policy (`dev`/`release`),
  tiered optimization classes (A/B/C), per-function budgeted degrade ladders,
  and per-pass wall-time offender telemetry. Latest tightening pass now
  defaults stdlib functions to Tier C unless explicitly promoted, adds
  finer stage-level/pre-pass budget degrade checkpoints (including preemptive
  degrade evaluation), and surfaces stdlib-aware effective min-cost thresholds
  in frontend parallel layer diagnostics.
- Prioritized IR closure queue for the active 90-day window:
  - P0: `CallIndirect`, `InvokeFFI`, `GuardTag`, `GuardDictShape`.
  - P1: `IncRef`, `DecRef`, `Borrow`, `Release`.
  - P2: `Box`, `Unbox`, `Cast`, `Widen` + partial alias-name normalization.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): complete `CALL_INDIRECT` hardening with broader deopt reason telemetry (dedicated runtime lane, noncallable differential probe, CI-enforced probe execution/failure-queue linkage, and runtime-feedback counter `deopt_reasons.call_indirect_noncallable` are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): complete `INVOKE_FFI` hardening with broader deopt reason telemetry (bridge-lane marker, runtime capability gate, negative capability differential probe, CI-enforced probe execution/failure-queue linkage, and runtime-feedback counter `deopt_reasons.invoke_ffi_bridge_capability_denied` are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): harden `GUARD_TAG` specialization/deopt semantics + coverage (runtime-feedback counter `deopt_reasons.guard_tag_type_mismatch` is landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): harden `GUARD_DICT_SHAPE` invalidation/deopt semantics + coverage (runtime-feedback aggregate counter `deopt_reasons.guard_dict_shape_layout_mismatch` and per-reason breakdown counters are landed).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): enforce explicit LIR ownership invariants for `INC_REF`/`DEC_REF` across frontend/backend with differential parity evidence.
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): enforce borrow/release lifetime invariants for `BORROW`/`RELEASE` with safety checks and parity coverage.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P2, status:partial): add generic conversion ops (`BOX`, `UNBOX`, `CAST`, `WIDEN`) with deterministic semantics and native/wasm parity coverage.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P2, status:partial): normalize alias op naming (`BRANCH`/`RETURN`/`THROW`/`LOAD_ATTR`/`STORE_ATTR`/`CLOSURE_LOAD`/`CLOSURE_STORE`) or codify canonical aliases in `0100_MOLT_IR`.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:partial): extend sparse SCCP beyond current arithmetic/boolean/comparison/type-of coverage into broader heap/call-specialization families and a stronger loop-bound solver for cross-iteration constant reasoning.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:partial): extend loop/try edge threading beyond current executable-edge + conservative loop-marker rewrites into full loop-end and exceptional-handler CFG rewrites with dominance/post-dominance preservation.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): ship profile-gated mid-end policy matrix (`dev` correctness-first cheap opts; `release` full fixed-point) with deterministic pass ordering and explicit diagnostics (CLI->frontend profile plumbing is landed; diagnostics sink now also surfaces active midend policy config and heuristic knobs; remaining work is broader tuning closure and any additional triage UX).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add tiered optimization policy (Tier A entry/hot functions, Tier B normal user functions, Tier C heavy stdlib/dependency functions) with deterministic classification and override knobs (baseline deterministic classifier + env overrides are landed; runtime-feedback and PGO hot-function promotion are now wired through the existing tier promotion path).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): enforce per-function mid-end wall-time budgets with an automatic degrade ladder that disables expensive transforms before correctness gates and records degrade reasons (budget/degrade ladder is landed in fixed-point loop; heuristic tuning + diagnostics surfacing remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add per-pass wall-time telemetry (`attempted`/`accepted`/`rejected`/`degraded`, `ms_total`, `ms_p95`) plus top-offender diagnostics by module/function/pass (frontend per-pass timing/counters, CLI/JSON sink wiring, and hotspot rendering are landed).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): surface active optimization profile/tier policy and degrade events in CLI build diagnostics and JSON outputs for deterministic triage (diagnostics sink now includes profile/tier/degrade summaries + pass hotspots, and stderr verbosity partitioning is landed; remaining work is richer UX controls beyond verbosity).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add process-level parallel frontend module-lowering and deterministic merge ordering, then extend to large-function optimization workers where dependency-safe (dependency-layer process-pool lowering is landed behind `MOLT_FRONTEND_PARALLEL_MODULES`; remaining work is broader eligibility and worker-level tuning telemetry).
- TODO(compiler, owner:compiler, milestone:LF3, priority:P1, status:planned): migrate hot mid-end kernels (CFG build, SCCP lattice transfer, dominator/liveness) to Rust with Python orchestration preserved for policy control.
- Implemented: CI hardening for `tools/check_molt_ir_ops.py` now includes mandatory `--require-probe-execution` after `diff-basic`, so required-probe execution status and failure-queue linkage regressions fail CI.

## Capabilities (Current)
- Active stdlib lowering execution plan:
  [docs/spec/areas/compat/plans/stdlib_lowering_plan.md](docs/spec/areas/compat/plans/stdlib_lowering_plan.md).
- Active Tkinter cross-platform lowering plan:
  [docs/spec/areas/compat/plans/tkinter_lowering_plan.md](docs/spec/areas/compat/plans/tkinter_lowering_plan.md).
- Implemented: Tkinter runtime now has a dual-path intrinsic backend for
  `molt_tk_*`: deterministic intrinsic-backed headless behavior by default
  (core Tk command semantics plus broad `tkinter.ttk` command-family lowering),
  plus an opt-in native Tcl/Tk backend (`cargo` feature `molt_tk_native`) that
  wires app creation (`useTk` aware), Tcl command dispatch, callback
  registration, `after` scheduling, event pumping (`dooneevent`/`mainloop`),
  and Tk destroy/quit lifecycle through an embedded interpreter.
- Capability-gated fallback behavior remains deterministic when native Tk is
  unavailable (native: `RuntimeError`, wasm: `NotImplementedError`), with no
  host-Python fallback lane.
- Implemented: `_tkinter` Python shim now forwards an expanded intrinsic-backed
  surface (`TkappType`, call/event helpers, variable helpers, conversion
  helpers, and config helpers) directly to `molt_tk_*` intrinsics with no
  host-Python fallback path.
- Implemented: 10 Rust intrinsics wired for tkinter (event parsing, Tcl
  list/dict conversion, hex color validation, option normalization); all strict
  mode violations resolved.
- Implemented: tkinter 3.13 version-gated APIs (`tk_busy_*`,
  `PhotoImage.copy_replace`) and 3.14 version-gated APIs (`trace_variable`
  deprecation) added with explicit version gating.
- Implemented: tkinter 100% submodule coverage achieved across all
  `tkinter.*` submodules.
- Implemented: headless Rust Tk command lowering now covers major `tkinter.ttk`
  execution lanes (Treeview semantics, `ttk::style` configure/map/lookup/layout/
  element/theme paths, notebook + panedwindow container operations,
  `ttk::notebook::enableTraversal`, and common widget subcommands such as
  `state`/`instate`/`invoke`/`current`/`set`/`get`/`validate`/progress controls).
- Differential regression coverage now includes
  `tests/differential/stdlib/tkinter_phase0_core_semantics.py` to validate
  `_tkinter`/`tkinter` import + missing-attribute error-shape contracts,
  `_tkinter` intrinsic-backed core API presence (`create`, Tkapp helpers,
  conversion and var helpers, and exported constants/types), and tkinter wrapper
  submodule import/error-shape/capability-gate contracts (`tkinter.__main__`,
  dialog/helper wrappers, and `tkinter.ttk`) without requiring a real GUI
  backend, including runtime-lowered core + `ttk` semantics probes
  (`tkinter:runtime_core_semantics`, `tkinter.ttk:runtime_semantics`).
- Implemented: checker-level intrinsic-partial ratchet enforcement
  (`tools/check_stdlib_intrinsics.py`) with budget file
  `tools/stdlib_intrinsics_ratchet.json`.
- Implemented: host fallback `_py_*` import anti-pattern blocking in
  `tools/check_stdlib_intrinsics.py`.
- Implemented: importlib resolver hardening for module-name coercion and live
  resolver precedence in `importlib.machinery`/`importlib.util`, including
  one-shot default `PathFinder` bootstrap.
- Differential regression coverage includes
  `importlib_find_spec_path_importer_cache_intrinsic.py` and
  `importlib_find_spec_path_hooks_intrinsic.py`.
- Unit regression coverage includes
  `tests/test_stdlib_importlib_machinery.py`.
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
- memoryview exposes `format`/`shape`/`strides`/`nbytes`, `cast`, tuple scalar indexing, and 1D slicing/assignment/count/index for bytes/bytearray-backed views; multi-dimensional `count`/`index` retain CPython `NotImplementedError` behavior.
- `str.find`/`str.count`/`str.startswith`/`str.endswith` support start/end slices with Unicode-aware offsets; `str.split`/`str.rsplit` support `None` separators and `maxsplit` for str/bytes/bytearray; `str.replace` supports `count`; `str.strip`/`str.lstrip`/`str.rstrip` support default whitespace and `chars` argument; `str.join` accepts arbitrary iterables.
- Range materialization lowering now emits a dedicated runtime fast path (`list_from_range`) for `list(range(...))` and simple `[i for i in range(...)]` comprehensions, avoiding generator/list-append call overhead on hot loops.
- Dict increment idioms of the form `d[k] = d.get(k, 0) + delta` now lower to a dedicated runtime op (`dict_inc`) with int fast path + generic add fallback.
- Fused split+count lanes (`string_split_ws_dict_inc`, `string_split_sep_dict_inc`) now include a string-key dict probe fast path (hash+byte compare) with explicit fallback to generic dict semantics when mixed/non-string keys are encountered.
- Adaptive vector lane selection is enabled for `vec_sum_int*` and `vec_sum_float*` via runtime counters (`MOLT_ADAPTIVE_VEC_LANES`, default on), preserving generic fallback semantics while reducing wasted probe overhead in mixed workloads.
- For-loop element hint propagation now carries iterable element types (including `file_text`/`file_bytes`) into loop targets, enabling broader lowering of string/bytes method calls (for example split-heavy ETL loops) without host fallback paths.
- `statistics.mean`/`statistics.stdev` calls over slice expressions now lower to dedicated runtime ops (`statistics_mean_slice`, `statistics_stdev_slice`) with list/tuple fast paths and runtime-owned generic fallback semantics; hot loops avoid intermediate slice list allocations where possible.
- `statistics_mean_slice`/`statistics_stdev_slice` now use int/float element fast coercion lanes inside the slice loops (fallback preserved for generic numeric objects).
- `abs(...)` builtin now lowers directly to a dedicated runtime op (`abs`) instead of dynamic call dispatch in hot loops.
- `dict.setdefault(key, [])` now has a dedicated lowering/runtime lane (`dict_setdefault_empty_list`) that avoids eager empty-list allocation while preserving `dict.setdefault` behavior.
- `str.lower`/`str.upper`/`str.capitalize`/`str.title`/`str.swapcase`, `str.format_map`, `str.removeprefix`/`str.removesuffix`, `str.zfill`, `str.center`/`str.ljust`/`str.rjust`, `str.expandtabs`, and `%` string formatting (`%s`/`%r`/`%a`/`%c`/`%d`/`%i`/`%u`/`%o`/`%x`/`%X`/`%e`/`%E`/`%f`/`%F`/`%g`/`%G` + `%%`, including `*` width/precision),
  list methods (`append`/`extend`/`insert`/`remove`/`pop`/`count`/`index` with start/stop + parity errors, `clear`/`copy`/`reverse`/`sort`),
  and `dict.clear`/`dict.copy`/`dict.popitem`/`dict.setdefault`/`dict.update`/`dict.fromkeys`.
- Differential coverage for this `str` tranche: `tests/differential/basic/str_case_methods_extended.py` (case transform lane),
  `tests/differential/basic/str_predicates_surface.py` (predicate surface across empty, cased, and Unicode-heavy inputs),
  and `tests/differential/basic/str_lowered_methods.py` + `tests/differential/basic/str_lowered_methods_edges.py` (newly lowered string method/error/% lanes).
- List dunder arithmetic methods (`__add__`/`__mul__`/`__rmul__`/`__iadd__`/`__imul__`) are available for dynamic access and follow CPython error behavior.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): advance native `re` engine to full syntax/flags/groups; native engine supports literals, `.`, char classes/ranges (`\\d`/`\\w`/`\\s`), groups/alternation, greedy + non-greedy quantifiers, and `IGNORECASE`/`MULTILINE`/`DOTALL` flags. Matcher hot paths for literal/any/char-class advancement, char/range/category checks, anchors, backreference/group-presence resolution, scoped-flag math, group capture/value materialization, and replacement expansion are intrinsic-backed; remaining advanced features/flags still raise `NotImplementedError` (no host fallback).
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): migrate all Python stdlib modules to Rust intrinsics-only implementations (Python files may only be thin intrinsic-forwarding wrappers); compiled binaries must reject Python-only stdlib modules. See [docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md).
- Intrinsics audit is enforced by `tools/check_stdlib_intrinsics.py` (generated doc + lint), including `intrinsic-backed` / `intrinsic-partial` / `probe-only` / `python-only` status tracking and a transitive dependency gate preventing non-`python-only` modules from importing `python-only` stdlib modules.
- Fallback-pattern enforcement now runs across all stdlib modules by default in `tools/check_stdlib_intrinsics.py`; narrowing to intrinsic-backed-only scope is explicit (`--fallback-intrinsic-backed-only`).
- Implemented: host-fallback dynamic import blocking now covers `_py_*` alias and keyword-argument forms (`from importlib import import_module; import_module(...)` and `__import__(name=...)`) in addition to direct imports.
- Implemented: bootstrap strict roots (`builtins`, `sys`, `types`, `importlib`, `importlib.machinery`, `importlib.util`) now require an intrinsic-backed transitive stdlib closure in `tools/check_stdlib_intrinsics.py`.
- Implemented: CPython top-level + submodule stdlib union coverage gates now run in `tools/check_stdlib_intrinsics.py` (missing entries, duplicate module/package mappings, and required package-kind mismatches are hard failures).
- Implemented: canonical CPython baseline union is versioned in `tools/stdlib_module_union.py` (generated by `tools/gen_stdlib_module_union.py`) with update workflow documented in [docs/spec/areas/compat/surfaces/stdlib/stdlib_union_baseline.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_union_baseline.md).
- Implemented: stdlib coverage is complete by name for the CPython 3.12/3.13/3.14 union (`320` top-level required names, `540` required `.py` submodule names), with current checker snapshot `intrinsic-backed=0`, `intrinsic-partial=873`, `probe-only=0`, `python-only=0`, `missing_top_level=0`, `missing_submodules=0`, and zero fallback violations under full-coverage attestation mode (any non-attested module/submodule is classified as `intrinsic-partial`).
- Implemented: non-CPython top-level stdlib extras are now limited to `_intrinsics` (runtime loader helper) and `test` (CPython regrtest compatibility facade); Molt-specific DB shim moved out of stdlib.
- Implemented: Molt-specific DB shim moved out of stdlib namespace (`moltlib.molt_db`), with `molt.molt_db` compatibility shim retained for existing imports.
- Implemented: intrinsic pass-only fallback detection is enforced for `json` (try/except + `pass` around intrinsic calls now fails `tools/check_stdlib_intrinsics.py`).
- Implemented: `test.support` now prefers CPython `Lib/test/support` when available (env `MOLT_REGRTEST_CPYTHON_DIR` first, then host stdlib discovery), with a local Molt fallback module for environments without CPython test sources.
- Core compiled-surface gate is enforced by `tools/check_core_lane_lowering.py`: modules imported (transitively) by `tests/differential/basic/CORE_TESTS.txt` must be `intrinsic-backed` only.
- Execution program for complete Rust lowering is tracked in [docs/spec/areas/compat/plans/stdlib_lowering_plan.md](docs/spec/areas/compat/plans/stdlib_lowering_plan.md) (core blockers first, then socket -> threading -> asyncio, then full stdlib sweep).
- Implemented: `__future__` and `keyword` module data/queries are now sourced from Rust intrinsics (`molt_future_features`, `molt_keyword_lists`, `molt_keyword_iskeyword`, `molt_keyword_issoftkeyword`), removing probe-only status.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): remove `typing` fallback ABC scaffolding and lower protocol/ABC bootstrap helpers into Rust intrinsics-only paths.
- Implemented: `builtins` bootstrap no longer probes host `builtins`; descriptor constructors are intrinsic-backed (`molt_classmethod_new`, `molt_staticmethod_new`, `molt_property_new`) and fail fast when intrinsics are missing.
- Implemented: `pathlib` now routes core path algebra and filesystem operations through Rust intrinsics (`molt_path_join`, `molt_path_isabs`, `molt_path_dirname`, `molt_path_splitext`, `molt_path_abspath`, `molt_path_resolve`, `molt_path_parts`, `molt_path_parents`, `molt_path_relative_to`, `molt_path_with_name`, `molt_path_with_suffix`, `molt_path_expanduser`, `molt_path_match`, `molt_path_glob`, `molt_path_exists`, `molt_path_listdir`, `molt_path_mkdir`, `molt_path_unlink`, `molt_path_rmdir`, `molt_file_open_ex`); targeted differential lane (`os`/`time`/`traceback`/`pathlib`/`threading`) ran `24/24` green with RSS caps enabled.
- Implemented: `molt_path_isabs`/`molt_path_parts`/`molt_path_parents` now use runtime-owned splitroot-aware shaping so Windows drive/UNC absolute semantics are intrinsic-backed (no Python fallback logic), and `pathlib.Path` now supports reverse-division (`\"prefix\" / Path(\"leaf\")`) via intrinsic path joins.
- Implemented: `glob` now lowers through Rust intrinsics (`molt_glob_has_magic`, `molt_glob`) with the Python shim reduced to intrinsic forwarding + output validation; runtime-owned matching now covers `root_dir`, recursive `**` gating (`recursive=True`), `include_hidden`, trailing-separator directory semantics, pathlike `root_dir`, bytes-pattern outputs (`list[bytes]`), mixed bytes/str parity errors, and intrinsic `dir_fd` relative traversal on native hosts (Linux `/proc`/`/dev/fd`, Apple `fcntl(F_GETPATH)`, and Windows handle-path resolution). Wasm hosts use capability-aware behavior: server/WASI targets support `dir_fd` when fd-path resolution is exposed, while browser-like hosts raise explicit `NotImplementedError` for relative `dir_fd` globbing.
- Implemented: `fnmatch` and `shlex` now route matching/tokenization through Rust intrinsics (`molt_fnmatch`, `molt_fnmatchcase`, `molt_fnmatch_filter`, `molt_fnmatch_translate`, `molt_shlex_split_ex`, `molt_shlex_join`) with Python modules reduced to argument normalization + iterator glue.
- Implemented: `stat`, `textwrap`, and `urllib.parse` core surfaces are now runtime-owned through dedicated intrinsics (`molt_stat_*`, `molt_textwrap_*`, `molt_urllib_*`); `stat` now includes intrinsic-backed file-type constants, permission/set-id bits, `ST_*` indexes, and helper functions (`S_IFMT`/`S_IMODE` + `S_IS*`) with a thin Python shim and no host fallback path, and `textwrap` now routes `TextWrapper` option-rich wrap/fill + predicate-aware `indent` through `molt_textwrap_wrap_ex`/`molt_textwrap_fill_ex`/`molt_textwrap_indent_ex` with targeted differential coverage for option-sensitive behavior (`drop_whitespace`, `expand_tabs`, `break_on_hyphens`, `max_lines`).
- Implemented: `urllib.parse.urlencode` now lowers through runtime intrinsic `molt_urllib_urlencode`; the shim keeps only query-item normalization and output validation.
- Implemented: `urllib.error` now lowers exception construction/formatting through dedicated runtime intrinsics (`molt_urllib_error_urlerror_init`, `molt_urllib_error_urlerror_str`, `molt_urllib_error_httperror_init`, `molt_urllib_error_httperror_str`, `molt_urllib_error_content_too_short_init`) for `URLError`, `HTTPError`, and `ContentTooShortError`; the module shim is reduced to class shell wiring and raises immediately when intrinsics are unavailable.
- Implemented: `urllib.request` opener core now lowers through dedicated runtime intrinsics (`molt_urllib_request_request_init`, `molt_urllib_request_opener_init`, `molt_urllib_request_add_handler`, `molt_urllib_request_open`) covering request/bootstrap wiring, handler ordering/dispatch, and `data:` URL fallback behind default-opener wiring; Python shim is limited to class shells and response adaptation, with `data:` metadata parity (`getcode()`/`status` -> `None`).
- Implemented: `urllib.response` now provides CPython-style `addbase`/`addclosehook`/`addinfo`/`addinfourl` classes, and intrinsic-backed response handles lower through `molt_urllib_request_response_*` accessors (including `read`/`read1`/`readinto`/`readinto1`/`readline`/`readlines`, `readable`/`writable`/`seekable`/`tell`/`seek`, plus `molt_urllib_request_response_message` for `HTTPMessage` header materialization) instead of Python-side response shims; closed-read behavior now matches CPython split semantics (`data:` raises `ValueError`, HTTP returns EOF/zero where applicable).
- Implemented: `http.client` now lowers request/response execution through dedicated runtime intrinsics (`molt_http_client_execute`, `molt_http_client_response_*`) and `http.server`/`socketserver` serve-loop lifecycle paths are intrinsic-backed (`molt_socketserver_serve_forever`, `molt_socketserver_shutdown`, queue dispatch intrinsics), with Python shims reduced to thin state wiring and handler shaping.
- Implemented: `enum` and `pickle` are now intrinsic-backed on core construction/serialization paths (`molt_enum_init_member`, `molt_pickle_dumps_core`, `molt_pickle_loads_core`) with `pickle.py` reduced to thin intrinsic-forwarding wrappers (`dump`/`dumps`/`load`/`loads`, `Pickler`, `Unpickler`, `PickleBuffer`) for protocols `0..5`; protocol-5 out-of-band `PickleBuffer` lanes now decode/encode through intrinsic `NEXT_BUFFER`/`READONLY_BUFFER` handling with `loads(..., buffers=...)`; broader CPython 3.12+ reducer/error-text/API-surface parity remains queued.
- Implemented: `queue` now has intrinsic-backed `LifoQueue` and `PriorityQueue` constructors/ordering (`molt_queue_lifo_new`, `molt_queue_priority_new`) on top of existing intrinsic-backed FIFO queue operations.
- Differential coverage now includes queue-family targeted probes for `Queue.shutdown`/`ShutDown` surface/behavior across interpreter versions, `Queue.put/get(block=False, timeout=...)` timeout-ignore semantics with state-driven `Full`/`Empty`, invalid-timeout typing parity (`Queue.put(timeout='bad')` succeeds for unbounded queues while `Queue.get(timeout='bad')` and full `Queue.put(timeout='bad')` raise `TypeError`), `SimpleQueue.get(block=False, timeout=...)` empty-path behavior, `SimpleQueue.get(timeout='bad')` `TypeError` behavior, and `_queue` import/surface sanity (`Empty`, `SimpleQueue`).
- Implemented: `statistics` function surface now lowers through Rust intrinsics (`molt_statistics_mean`, `molt_statistics_fmean`, `molt_statistics_stdev`, `molt_statistics_variance`, `molt_statistics_pvariance`, `molt_statistics_pstdev`, `molt_statistics_median`, `molt_statistics_median_low`, `molt_statistics_median_high`, `molt_statistics_median_grouped`, `molt_statistics_mode`, `molt_statistics_multimode`, `molt_statistics_quantiles`, `molt_statistics_harmonic_mean`, `molt_statistics_geometric_mean`, `molt_statistics_covariance`, `molt_statistics_correlation`, `molt_statistics_linear_regression`) with shim-level `StatisticsError` mapping.
- Implemented: runtime-backed slice statistics intrinsics (`molt_statistics_mean_slice`, `molt_statistics_stdev_slice`) are wired for native+wasm lowering paths and preserve generic fallback behavior via runtime-owned slicing/iteration.
- Implemented: `statistics.NormalDist.samples` now follows CPython version-gated semantics for Python 3.12+ through intrinsic-backed `molt_statistics_normal_dist_samples` in both lanes (`gauss` route for 3.12/3.13 and runtime-owned inverse-CDF route for 3.14+ with seeded/unseeded uniform draws), with no host-Python fallback path.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete full `statistics` 3.12+ API/PEP parity beyond intrinsic-lowered function surface (for example `NormalDist` and remaining edge-case semantics).
- `enumerate` builtin returns an iterator over `(index, value)` with optional `start`.
- `iter(callable, sentinel)`, `map`, `filter`, `zip(strict=...)`, and `reversed` return lazy iterator objects with CPython-style stop conditions.
- `iter(obj)` enforces that `__iter__` returns an iterator, raising `TypeError` with CPython-style messages for non-iterators.
- Builtin function objects for allowlisted builtins (`any`, `all`, `abs`, `ascii`, `bin`, `oct`, `hex`, `chr`, `ord`, `divmod`, `hash`, `callable`, `repr`, `format`, `getattr`, `hasattr`, `round`, `iter`, `next`, `anext`, `print`, `super`, `sum`, `min`, `max`, `sorted`, `map`, `filter`, `zip`, `reversed`).
- `sorted()` enforces keyword-only `key`/`reverse` arguments (CPython parity).
- Builtin reductions: `sum`, `min`, `max` with key/default support across core ordering types.
- Policy-deferred: dynamic execution (`eval`/`exec`/`compile`) remains out of active burndown for compiled binaries; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Differential parity probes for dynamic execution (`eval`/`exec`) are tracked in `tests/differential/basic/exec_*` and `tests/differential/basic/eval_*` and are intentionally expected-failure policy cases until that policy changes.
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
- Current acceleration execution lane is worker IPC (`molt_worker`) for reliability/isolation; an opt-in in-process fast path is planned for precompiled deploy-time handlers.
- `molt_worker` supports sync/async runtimes (`MOLT_WORKER_RUNTIME` / `--runtime`), enforces cancellation/timeout checks in the fake DB path, compiled dispatch loops, pool waits, Postgres queries, and SQLite via interrupt handles; validates export manifests; reports queue/pool metrics per request (queue_us/handler_us/exec_us/decode_us plus ms rollups); fake DB decode cost can be simulated via `MOLT_FAKE_DB_DECODE_US_PER_ROW` and CPU work via `MOLT_FAKE_DB_CPU_ITERS`. Thread and queue tuning are available via `MOLT_WORKER_THREADS` and `MOLT_WORKER_MAX_QUEUE` (CLI overrides).
- `molt-db` provides a bounded pool, a feature-gated async pool primitive, a native-only SQLite connector (feature-gated in `molt-worker`), and an async Postgres connector (tokio-postgres + rustls) with per-connection statement caching.
- `molt_db_adapter` exposes a framework-agnostic DB IPC payload builder aligned with [docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md](docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md); worker-side `db_query`/`db_exec` support SQLite (sync) and Postgres (async) with json/msgpack results (Arrow IPC for `db_query`), db-specific metrics, and structured decoding for Postgres arrays/ranges/intervals/multiranges in json/msgpack plus Arrow IPC struct/list encodings (including lower-bound metadata). WASM DB host intrinsics (`db_query`/`db_exec`) are defined with stream handles and `db.read`/`db.write` capability gating, and the Node/WASI host adapter is wired in `run_wasm.js`.
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
- Stdlib `linecache` supports `getline`/`getlines`/`checkcache`/`lazycache` with `fs.read` gating and loader-backed cache entries; lazy loader `get_source` lookup now lowers through `molt_linecache_loader_get_source` so `ImportError`/`OSError` mapping is runtime-owned instead of Python-side fallback handling.
- Stdlib `pkgutil` supports filesystem `iter_modules`/`walk_packages` with `fs.read` gating.
- Stdlib `compileall` supports filesystem `compile_file`/`compile_dir`/`compile_path` with `fs.read` gating (no pyc emission).
- Stdlib `py_compile` supports `compile` with `fs.read`/`fs.write` gating (writes empty placeholder .pyc only).
- Stdlib `enum` provides minimal `Enum`/`IntEnum`/`Flag`/`IntFlag` support with `auto`, name/value accessors, and member maps.
- Stdlib `traceback` supports `format_exc`/`format_tb`/`format_list`/`format_stack`/`print_exception`/`print_list`/`print_stack`, `extract_tb`/`extract_stack`, `StackSummary` extraction, and runtime-lowered exception-chain formatting via `molt_traceback_format_exception`; traceback extraction now routes through `molt_traceback_extract_tb`/`molt_traceback_payload`, suppress-context probing lowers through `molt_traceback_exception_suppress_context`, stack frame entry retrieval routes through `molt_getframe`, and `TracebackException.from_exception` consumes a single runtime-owned chain payload (`molt_traceback_exception_chain_payload`) for frame/cause/context shaping. Full parity pending.
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
- TODO(offload, owner:runtime, milestone:SL2, priority:P1, status:planned): add a Phase 1 in-process fast path for precompiled endpoint exports (startup-loaded ABI, no runtime compilation) while preserving worker IPC semantics for capability gating, cancellation, and error mapping.
- `asyncio.CancelledError` follows CPython inheritance (BaseException subclass), so cancellation bypasses `except Exception`.

## Limitations (Current)
- Core-lane strict lowering gate is green and enforced (`tools/check_core_lane_lowering.py`), and core-lane differential currently passes.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P0, status:partial): complete concurrency substrate lowering in strict order (`socket`/`select`/`selectors` -> `threading` -> `asyncio`) with intrinsic-only compiled semantics in native + wasm.
- Classes/object model: no metaclasses or dynamic `type()` construction.
- Implemented: `types.GenericAlias.__parameters__` derives `TypeVar`/`ParamSpec`/`TypeVarTuple` from `__args__`.
- Implemented: PEP 695 core-lane lowering uses Rust intrinsics for type parameter creation and GenericAlias construction/call dispatch (`molt_typing_type_param`, `molt_generic_alias_new`) for `typing`/frontend paths.
- Implemented: Type parameter defaults now lower through `typing._molt_type_param` for `ast.TypeVar.default_value`, and `typing.TypeVar(default=...)` is version-gated to Python >= 3.13 semantics (`typing.NoDefault`/`has_default` available only on 3.13+ targets).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): finish remaining PEP 695 metadata work (alias metadata/TypeAliasType) and broaden type-parameter default coverage beyond current TypeVar path where required.
- Attributes: fixed struct fields with dynamic instance-dict fallback; no
  user-defined `__slots__` beyond dataclass lowering; object-level
  class `__dict__` returns a mappingproxy view.
- Class instantiation bypasses user-defined `__new__` for non-exception classes (allocates instances directly before `__init__`).
  (TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.)
- Strings: `str.isdigit` now follows Unicode digit properties (ASCII + superscripts + non-ASCII digit sets).
- Strings: predicate surface now includes intrinsic-backed `str.isalpha`, `str.isalnum`, `str.isdecimal`, `str.isdigit`, `str.isnumeric`, `str.islower`, `str.isupper`, `str.isspace`, `str.istitle`, `str.isprintable`, and `str.isascii`; `str.title`/`str.capitalize` now use runtime Unicode titlecase tables (generated in `build.rs`) for CPython-aligned digraph/ligature behavior.
- Dataclasses: compile-time lowering covers init/repr/eq/order/unsafe_hash/frozen/slots/match_args/kw_only,
  field flags, InitVar/ClassVar/KW_ONLY, __match_args__, stdlib helpers, and `make_dataclass`.
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
- Async generators (`async def` with `yield`) are fully supported at all layers (frontend
  visit_AsyncFunctionDef, backend ASYNCGEN_NEW/TrampolineKind::AsyncGen, runtime
  molt_asyncgen_poll/molt_asyncgen_new/molt_asyncgen_close). PEP 479 analog: StopAsyncIteration
  raised inside async gen body is now correctly converted to RuntimeError.
- `contextlib` is intrinsic-backed for `contextmanager`/`ContextDecorator` + `ExitStack`/`AsyncExitStack`,
  `asynccontextmanager`/`aclosing`, `suppress`, `redirect_stdout`/`redirect_stderr`, `nullcontext`,
  `closing`, `AbstractContextManager`, `AbstractAsyncContextManager`, and `chdir`
  (including runtime-owned abstract subclasshook checks and cwd enter/exit paths).
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
- Implemented: structural pattern matching (`match`/`case`) via cell-based AST-to-IR desugaring — all PEP 634 pattern types (literal, variable, sequence, mapping, class, or, as, star, singleton, guard) with 24 differential test files.
- Differential tests: core-language basic includes pattern matching, async generator finalization, and `while`-`else` probes; pattern matching is implemented; async gen finalization gaps remain.
- Augmented assignment: slice targets (`seq[a:b] += ...`) are supported, including extended-slice length checks.
- Exceptions: `try/except/else/finally` + `raise`/reraise + `except*` (ExceptionGroup matching/splitting/combining); `__traceback__` now returns
  traceback objects (`tb_frame`/`tb_lineno`/`tb_next`) with frame objects carrying `f_code`/`f_lineno` (see
  [docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md](docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md)). Builtin exception hierarchy now matches CPython (BaseExceptionGroup,
  OSError/Warning trees, ExceptionGroup MRO).
  (TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame objects to full CPython parity fields.)
  (TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): exception `__init__` + subclass attribute parity (ExceptionGroup tree).)
- Code objects: `__code__` exposes `co_filename`/`co_name`/`co_firstlineno`, `co_varnames`, arg counts
  (`co_argcount`/`co_posonlyargcount`/`co_kwonlyargcount`), `co_linetable`, `co_freevars`, `co_cellvars`,
  and baseline `co_flags` (`CO_OPTIMIZED|CO_NEWLOCALS`) for intrinsic-created code objects.
  (TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): complete closure/generator/coroutine-specific `co_flags` and free/cellvar parity.)
- Runtime lifecycle: `molt_runtime_init()`/`molt_runtime_shutdown()` manage a `RuntimeState` that owns caches, pools, and async registries; TLS guard drains per-thread caches on thread exit, scheduler/sleep workers join on shutdown, and freed TYPE_ID_OBJECT headers return to the object pool with fallback deallocation for non-pooled types.
- Tooling: `molt clean --cargo-target` removes Cargo `target/` build artifacts when requested.
- Process-based concurrency is partial: spawn-based `multiprocessing` (Process/Pool/Queue/Pipe/SharedValue/SharedArray) is capability-gated and supports `maxtasksperchild`; `fork`/`forkserver` map to spawn semantics (no true fork yet). `subprocess` and `concurrent.futures` remain pending.
  (TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.)
  (TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures`.)
- `sys.argv` is initialized from compiled argv (native + wasm harness); decoding currently uses lossy UTF-8/UTF-16 until surrogateescape/fs-encoding parity lands.
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): decode argv via filesystem encoding + surrogateescape once Molt strings can represent surrogate escapes.)
- `sys.executable` now honors `MOLT_SYS_EXECUTABLE` when set (the diff harness pins it to the host Python to avoid recursive `-c` subprocess spawns); otherwise it falls back to the compiled argv[0].
- `sys.modules` mirrors the runtime module cache for compiled code; `sys._getframe` is available in compiled runtimes with partial frame objects (see introspection TODOs).
- `sys.path` bootstrap/environment policy is runtime-owned via intrinsic payload (`molt_sys_bootstrap_payload`) with deterministic fields for
  `PYTHONPATH`/`MOLT_MODULE_ROOTS`/`VIRTUAL_ENV` site-packages/`PWD`/stdlib-root/include-cwd policy (including pre-split path lists); stdlib wrappers consume that payload
  directly and do not read host env in Python shims.
- `runpy.run_path` path coercion/abspath/is-file probing is runtime-lowered via `molt_runpy_resolve_path` (bootstrap-PWD aware).
- `runpy.run_module` now preserves dotted-module/package `__main__` resolution semantics on the intrinsic path (builtins `__import__`
  fromlist behavior is Rust-owned for parity, without widening dynamic execution scope).
- `runpy.run_path` module-source execution now supports restricted reference-assignment RHS (for example `sys.argv[0]`) on the intrinsic lane
  without widening dynamic execution policy.
- `runpy` execution remains on a restricted intrinsic lane and does not widen dynamic execution policy for compiled binaries.
- runpy dynamic-lane expected failures are currently empty because supported lanes moved to intrinsic support.
- `runpy` negative-path parity (`runpy_path_resolution_errors`) remains an active supported lane; full `runpy` package/code-object
  execution parity remains pending behind explicit policy gating.
- Implemented: `signal` parity fixes for diff-critical constants now lower through Rust intrinsics (`molt_signal_nsig`,
  `molt_signal_sig_block`, `molt_signal_sig_unblock`, `molt_signal_sig_setmask`), and default `SIGINT` handler parity now
  routes through intrinsic-backed `default_int_handler` wiring in `signal`/`_signal` without host fallback.
- Implemented: `subprocess` public sentinel constants (`PIPE`/`STDOUT`/`DEVNULL`) now expose CPython values via dedicated
  intrinsics while preserving internal spawn-mode mapping in Python shim glue; `_asyncio.current_task()` now raises
  `RuntimeError("no running event loop")` outside a running loop, and `_thread` lock type naming aligns with CPython-visible
  `lock` type name.
- Implemented: `shutil.rmtree` is now intrinsic-backed (`molt_shutil_rmtree`) for cleanup parity in os/scandir/walk lanes.
- `globals()` can be referenced as a first-class callable (module-bound) and returns the defining module globals; `locals()`/`vars()`/`dir()` remain lowered as direct calls,
  and no-arg callable parity for these builtins is still limited.
  (TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.)
- Runtime safety: NaN-boxed pointer conversions resolve through a pointer registry to avoid int->ptr casts in Rust; host pointer args now use raw pointer ABI in native + wasm; strict-provenance Miri is green.
- Hashing: SipHash13 + `PYTHONHASHSEED` parity (randomized by default; deterministic when seed=0); see [docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md](docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md).
- GC: reference counting only; cycle collector pending (see [docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md](docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md)).
  (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): implement cycle collector.)
- Imports: file-based sys.path resolution and `spec_from_file_location` are supported;
  `importlib.util.find_spec` now routes `meta_path`, `path_hooks`, namespace package search,
  extension-module spec discovery, sourceless bytecode spec discovery, zip-source spec discovery,
  and `path_importer_cache`
  finder reuse through runtime intrinsics.
  (TODO(import-system, owner:stdlib, milestone:TC3, priority:P2, status:partial): full extension/sourceless execution parity beyond capability-gated restricted-source shim hooks.)
- Entry modules execute under `__main__` while remaining importable under their real module name (distinct module objects).
- Module metadata: compiled modules set `__file__`/`__package__`/`__spec__` (ModuleSpec + filesystem loader) and package `__path__`; `importlib.machinery.SourceFileLoader`
  package/module shaping and source decode payload now lower through runtime intrinsics (`molt_importlib_source_exec_payload`),
  file reads lower via `molt_importlib_read_file`, and source execution remains intrinsic-lowered
  via `molt_importlib_exec_restricted_source` (restricted evaluator, no host fallback). `importlib.import_module` dispatch lowers through
  `molt_module_import` (no Python `__import__` fallback). `importlib.util` filesystem discovery/cache-path +
  `spec_from_file_location` package shaping now lower through `molt_importlib_find_spec_orchestrate`,
  `molt_importlib_bootstrap_payload`, `molt_importlib_cache_from_source`, and
  `molt_importlib_spec_from_file_location`.
  `importlib.machinery.ZipSourceLoader` source payload/execution now lowers through
  `molt_importlib_zip_source_exec_payload`, and module spec package detection now lowers through
  `molt_importlib_module_spec_is_package`; extension/sourceless loader execution is intrinsic-owned via
  `molt_importlib_exec_extension`/`molt_importlib_exec_sourceless` with capability-gated intrinsic execution lanes
  (`*.molt.py` + `*.py` candidates) before explicit `ImportError`, and unsupported restricted-shim candidates now
  continue probing later candidates deterministically before final failure. Restricted shim execution now
  also handles `from x import *` semantics (including `__all__` validation/fallback underscore filtering)
  in runtime-owned paths.
  `importlib.resources` package root/namespace resolution and traversable stat/listdir payloads are runtime-lowered via
  `molt_importlib_resources_package_payload` and `molt_importlib_resources_path_payload` (including zip/whl/egg namespace/resource roots); files()-path arbitration
  now lowers through `molt_importlib_resources_files_payload`, and loader reader bootstrap
  lowers through `molt_importlib_resources_module_name`/`molt_importlib_resources_loader_reader` (including
  explicit fallback from `module.__spec__.loader` to `module.__loader__`), and custom
  reader contract surfaces lower through `molt_importlib_resources_reader_roots`/`molt_importlib_resources_reader_contents`/
  `molt_importlib_resources_reader_resource_path`/`molt_importlib_resources_reader_is_resource`/
  `molt_importlib_resources_reader_open_resource_bytes`/`molt_importlib_resources_reader_child_names`; direct resources text/binary reads lower through
  `molt_importlib_read_file`. `importlib.resources._functional` string-anchor path-name APIs now lower through dedicated
  runtime intrinsics (`molt_importlib_resources_open_resource_bytes_from_package_parts`,
  `molt_importlib_resources_read_text_from_package_parts`,
  `molt_importlib_resources_contents_from_package_parts`,
  `molt_importlib_resources_is_resource_from_package_parts`,
  `molt_importlib_resources_resource_path_from_package_parts`) with a thin wrapper for CPython-compatible warnings/encoding
  argument behavior. `importlib.metadata` dist-info scan + metadata parsing lower through
  `molt_importlib_bootstrap_payload`, `molt_importlib_metadata_dist_paths`,
  `molt_importlib_metadata_entry_points_payload`/`molt_importlib_metadata_entry_points_select_payload`,
  `molt_importlib_metadata_normalize_name`, and `molt_importlib_metadata_payload`
  (including `Requires-Dist`/`Provides-Extra`/`Requires-Python` payload fields); bulk distribution cache payloads,
  `importlib.metadata.files()`, and `importlib.metadata.packages_distributions()` now lower through runtime payload
  intrinsics `molt_importlib_metadata_distributions_payload`, `molt_importlib_metadata_record_payload`, and
  `molt_importlib_metadata_packages_distributions_payload`.
  Importlib helper-module lowering also now routes through runtime intrinsics: `importlib._abc`/`importlib.readers`/`importlib.simple`
  use intrinsic-backed import loading (`molt_importlib_import_optional`/`molt_importlib_import_required`), `importlib.resources` helper
  modules use intrinsic-backed joinpath/import/mode/leaf-name helpers (`molt_importlib_resources_joinpath`,
  `molt_importlib_resources_open_mode_is_text`, `molt_importlib_resources_package_leaf_name`), and `importlib.metadata`
  helper modules use runtime-owned functools/itertools/operator helpers (`molt_functools_lru_cache`,
  `molt_functools_update_wrapper`, `molt_itertools_filterfalse`, `molt_operator_truth`, `molt_operator_eq`, `molt_operator_lt`)
  without host-Python fallback lanes.
  Build-time module-graph import collection now resolves constant module-name flows through helper wrappers (for example
  `importlib.import_module(module_name)` inside a helper invoked as `_probe(MODULE_NAME)`), so generated `_molt_importer`
  dispatchers include the required stdlib modules/submodules and avoid brittle runtime fallback paths for those imports.
  TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery full extension/sourceless execution parity beyond capability-gated restricted-source shim lanes (zip source loader path is intrinsic-lowered).
- Imports: module-level `from x import *` honors `__all__` (with strict name checks) and otherwise skips underscore-prefixed names.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root builds (namespace packages + PYTHONPATH roots supported; remaining: package discovery hardening, `__init__` edge cases, deterministic dependency graph caching).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:planned): method-binding safety pass (guard/deopt on method lookup + cache invalidation rules for call binding).
- Asyncio: shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `Event`, `wait`, `wait_for`, `shield`, basic `gather`,
  stream helpers (`open_connection`/`start_server`), `add_reader`/`add_writer`, pipe transports
  (`connect_read_pipe`/`connect_write_pipe` via 11 new pipe transport intrinsics), and
  Transport/Protocol base classes; 42 new Rust intrinsics cover Future/Event/Lock/Semaphore/Queue
  state machines; all 97 bare `except` blocks eliminated from asyncio shim; WASM capability gating
  complete for 6 I/O operations; 3.13 version-gated APIs (`as_completed` async iter,
  `Queue.shutdown`) and 3.14 version-gated APIs (`get_event_loop` `RuntimeError`, child watcher
  removal, policy deprecation) added. Asyncio subprocess stdio now supports `stderr=STDOUT` and fd-based redirection,
  with mode normalization/runtime validation lowered into Rust intrinsic `molt_asyncio_subprocess_stdio_normalize`.
  Timer and fd-watcher teardown now lower through `molt_asyncio_timer_handle_cancel` and `molt_asyncio_fd_watcher_unregister`.
  Runtime capability gates for SSL transport, Unix sockets, and child-watchers are intrinsic-owned
  (`molt_asyncio_require_ssl_transport_support`, `molt_asyncio_require_unix_socket_support`, `molt_asyncio_require_child_watcher_support`)
  so unsupported paths raise deterministic runtime/capability errors rather than Python `NotImplementedError`.
  SSL orchestration is runtime-owned via `molt_asyncio_ssl_transport_orchestrate`; `ssl=False` now returns an explicit
  non-SSL payload path, client TLS execution for `open_connection`/`create_connection` + `open_unix_connection`/`create_unix_connection`
  plus client/server-side `start_tls` upgrades now lower into runtime-owned rustls stream intrinsics
  (`molt_asyncio_tls_client_connect_new`, `molt_asyncio_tls_client_from_fd_new`,
  `molt_asyncio_tls_server_payload`, `molt_asyncio_tls_server_from_fd_new`), and server TLS execution for
  `start_server`/`start_unix_server` lowers through the same runtime cert/key payload + fd-upgrade intrinsics
  instead of Python fail-fast stubs.
  Event-loop semantics target a single-threaded, deterministic scheduler; true parallelism is explicit via executors or isolated
  runtimes.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task APIs + task groups + I/O adapters + executor semantics.)
  Logging core is implemented (Logger/Handler/Formatter/LogRecord + basicConfig) with deterministic formatting and
  capability-gated sinks; `logging.config` and `logging.handlers` remain pending.
  (TODO(async-runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors and explicit message passing; shared-memory parallelism only via opt-in safe types.)
- C API: bootstrap `libmolt` C-extension surface is landed (`runtime/molt-runtime/src/c_api.rs` + `include/molt/molt.h`) for runtime/GIL, errors, scalar constructors/accessors (`none`/bool/int/float), object protocol (including bytes-name attribute helpers + compare/contains bool helpers), numerics, sequence/mapping, array-to-container constructors, buffer/bytes wrappers, plus type/module parity wrappers (`molt_type_ready`, module create/add/get APIs, runtime-owned module metadata/state registries, and `PyState_*` backing APIs). A CPython-style source-compat shim header is now available at `include/Python.h` / `include/molt/Python.h` (including partial `PyErr_*`, `PySequence_*`/`PyMapping_*`, `PyArg_ParseTuple`, O(n + k) `PyArg_ParseTupleAndKeywords`, module-creation/access helpers such as `PyModule_New(Object)`, `PyModule_GetDef`, `PyModule_GetState`, `PyModule_SetDocString`, `PyModule_GetFilename(Object)`, `PyModule_FromDefAndSpec(2)`, `PyModule_ExecDef`, `PyState_*`, callback-backed `PyModule_AddFunctions`, expanded heap-type creation via `PyType_FromSpec(WithBases)` selected slot lowering + `METH_CLASS`/`METH_STATIC`, and module-associated type APIs `PyType_FromModuleAndSpec`/`PyType_GetModule*`).
- Tooling/runtime: `molt publish` hard-verifies extension wheels (`.whl`) as extension metadata (`extension_manifest.json`) with required checksums/capabilities before publish, and runtime now enforces extension metadata ABI/capability/checksum checks across import spec-discovery boundaries and load/exec boundaries (finder + loader/exec lanes) with fingerprint-aware validation caching for replaced artifacts; CI includes an extension build/scan/audit/verify/publish dry-run matrix (`linux native`, `linux cross-aarch64-gnu`, `linux cross-musl`, `macos native`, `windows native`) with cross-lane wheel-tag assertions, native-lane runtime import smoke execution checks, and a wasm-target rejection contract check on the `linux native` lane.
- Policy: Molt binaries never fall back to CPython; C-extension compatibility is planned via `libmolt` (primary) with an explicit, capability-gated bridge as a non-default escape hatch.
  (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): extend the initial C API shim from bootstrap wrappers to broader source-compat and ABI coverage).
- Intrinsics registry is runtime-owned and strict; CPython shims have been removed from tooling/tests. `molt_json` and `molt_msgpack` now require runtime intrinsics (no Python-library fallback).
- Matmul (`@`): supported only for `molt_buffer`/`buffer2d`; other types raise
  `TypeError`; TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): consider `__matmul__`/`__rmatmul__` fallback for custom types.
- Roadmap focus: async runtime core (Task/Future scheduler, contextvars, cancellation injection), capability-gated async I/O,
  DB semantics expansion, WASM DB parity, framework adapters, and production hardening (see ROADMAP).
- Numeric tower: complex supported via canonical NaN-boxed object model (TYPE_ID_COMPLEX with ComplexParts in
  ops.rs/numbers.rs/attributes.rs; orphaned handle-based complex_core.rs deleted 2026-02-28);
  decimal is Rust intrinsic-backed with full arithmetic operators (add/sub/mul/truediv/floordiv/mod/pow/neg/pos/abs),
  comparisons (eq/lt/le/gt/ge/hash/bool), conversions (int/round/trunc/floor/ceil/from_float/to_eng_string/adjusted/
  as_integer_ratio), math methods (sqrt/ln/log10/exp/fma/max/min/remainder_near/scaleb/next_minus/next_plus/
  number_class), predicates (is_finite/is_infinite/is_nan/is_normal/is_signed/is_subnormal/is_zero), copy operations
  (copy_abs/copy_negate/copy_sign/same_quantum), and context management (prec/rounding/traps/flags); `int` still
  missing full method surface (e.g., `bit_length`, `to_bytes`, `from_bytes`).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P3, status:partial): remaining Decimal edge cases
  (NaN payload propagation, context-aware signal routing, __format__ spec).)
  (TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): `int` method parity.)
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
- Implemented: TaskGroup + Runner cancellation fanout now routes through
  intrinsic batch cancellation (`molt_asyncio_cancel_pending`) and intrinsic
  gather orchestration, reducing Python-side cancellation loops in shutdown and
  error paths.
- Implemented: asyncio synchronization hot paths now lower waiter fanout/removal
  through Rust intrinsics (`molt_asyncio_waiters_notify`,
  `molt_asyncio_waiters_notify_exception`,
  `molt_asyncio_waiters_remove`, `molt_asyncio_barrier_release`), covering
  lock/condition/semaphore/barrier/queue wake paths and cancellation cleanup
  loops.
- Implemented: asyncio task/future transfer and event-waiter teardown now route
  through Rust intrinsics (`molt_asyncio_future_transfer`,
  `molt_asyncio_event_waiters_cleanup`), removing Python callback-orchestration
  loops from `Task.__await__`, `wrap_future`, and token cleanup paths.
- Implemented: asyncio task registry + event-waiter token maps are now runtime-owned
  (`molt_asyncio_task_registry_set`/`get`/`current`/`pop`/`move`/`values`,
  `molt_asyncio_event_waiters_register`/`unregister`/`cleanup_token`), removing
  Python-owned `_TASKS` / `_EVENT_WAITERS` bookkeeping and making loop/task hot
  paths intrinsic-only.
- Implemented: TaskGroup done-callback error fanout and ready-queue drain now
  lower through Rust intrinsics (`molt_asyncio_taskgroup_on_task_done`,
  `molt_asyncio_ready_queue_drain`), removing Python-side task scan loops and
  event-loop ready-batch copy/clear churn in hot paths.
- Implemented: asyncio coroutine predicates now route through inspect intrinsics
  (`molt_inspect_iscoroutine`, `molt_inspect_iscoroutinefunction`) instead of
  Python inspect dispatch.
- Implemented: asyncio running/event-loop state now routes through runtime
  intrinsics (`molt_asyncio_running_loop_get`/`set`,
  `molt_asyncio_event_loop_get`/`set`,
  `molt_asyncio_event_loop_policy_get`/`set`) rather than Python globals.
- TODO(compiler, owner:compiler, milestone:TC2, priority:P0, status:implemented): fix async lowering/back-end verifier for `asyncio.gather` poll paths — native backend now inserts pending/ready blocks in dominance-compatible order with all target blocks registered before branch emission; WASM backend pre-stores the pending return value before the If block so the conditional body has a clean stack profile.
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
- Implemented: websocket wasm-host edge failures now raise explicit intrinsic-owned errors (capability-denied connect, missing host transport, and wait-registration failures) instead of generic fallback failures.
- TODO(perf, owner:runtime, milestone:RT3, priority:P2, status:planned): cache mio websocket poll streams/registrations to avoid per-wait `TcpStream` clones.

## Thread Safety + GIL Notes
- Runtime mutation is serialized by a GIL-like lock; only one host thread may
  execute Python/runtime code at a time within the process.
- PEP 684 (per-interpreter GIL) and PEP 703 (free-threading) are currently out of
  scope for shipped runtime semantics; current contract is single-GIL with explicit
  roadmap tasks for per-runtime isolation and safe cross-thread ownership.
- PEP 744 (JIT) is N/A for Molt's current architecture: Molt remains an AOT
  compiler/runtime pipeline.
- Runtime state and object headers are not thread-safe; `Value` and heap objects
  are not `Send`/`Sync` unless explicitly documented otherwise.
- Cross-thread sharing of live Python objects is unsupported by default; serialize or
  freeze data before crossing threads.
- `threading.Thread` uses the shared-runtime intrinsic spawn path by default
  (`molt_thread_spawn_shared`) and lifecycle/identity is tracked in the runtime
  thread registry intrinsics.
- `threading` bootstrap hook semantics (`settrace`, `setprofile`, `excepthook`)
  remain thin Python wrappers around intrinsic-backed thread lifecycle (no
  CPython fallback lane in compiled execution).
- `threading` timeout shaping now matches CPython negative-timeout behavior for
  `Thread.join`, `Condition.wait`, `Event.wait`, and `Semaphore.acquire` (with
  non-blocking semaphore timeout argument errors preserved).
- `threading.stack_size` is now runtime-owned via Rust intrinsics
  (`molt_thread_stack_size_get`/`molt_thread_stack_size_set`), and thread spawn
  paths consume the configured runtime stack size.
- `threading.RLock` ownership/recursion save+restore state is now runtime-owned
  via Rust intrinsics (`molt_rlock_is_owned`,
  `molt_rlock_release_save`, `molt_rlock_acquire_restore`), removing Python-side
  owner/count bookkeeping from compiled execution paths.
- Handle table and pointer registry may use internal locks; lock ordering rules
  are defined in [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define
  the per-runtime GIL strategy, runtime instance ownership model, and allowed
  cross-thread object sharing rules.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): implement
  sharded/lock-free handle resolution and measure lock-sensitive benchmark deltas
  (attr access, container ops).
- Runtime mutation entrypoints require a `PyToken`; only `molt_handle_resolve` is
  GIL-exempt by contract (see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)).

## Performance Notes
- `print` builds a single intermediate string before writing.
  (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): stream print writes to avoid large intermediate allocations.)
- `dict.fromkeys` does not pre-size using iterable length hints.
  (TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): pre-size `dict.fromkeys` to reduce rehashing.)

## Stdlib Coverage
- Asyncio & tkinter parity sprint (2026-02-28): asyncio pipe transports
  implemented (11 new pipe transport intrinsics), 42 new Rust intrinsics for
  Future/Event/Lock/Semaphore/Queue state machines, all 97 bare `except`
  blocks eliminated, WASM capability gating for 6 I/O ops, Transport/Protocol
  base classes added, 3.13/3.14 version-specific APIs gated; tkinter 10 Rust
  intrinsics wired, all strict mode violations resolved, 3.13/3.14
  version-gated APIs added, 100% submodule coverage achieved.
- Stdlib intrinsics sprint (2026-02-25): ~85 new Rust intrinsics landed across
  `os` (~40 APIs total), `sys` (~20 new), `signal` (12 constant + 5 function
  intrinsics), `_thread` (full rewrite on existing thread intrinsics),
  `_asyncio` (4 new runtime-state intrinsics for C-accelerated task surface),
  and `subprocess` (`molt_process_spawn_ex` + expanded Popen surface).
  `concurrent.futures` verified intrinsic-complete.
- Partial shims: `warnings`, `traceback`, `types`, `inspect`, `ast`, `atexit`, `ctypes`, `urllib.parse`, `urllib.error`, `urllib.request`, `fnmatch` (`*`/`?`
  + bracket class/range matching; literal `[]`/`[[]`/`[]]` escapes (no backslash
  quoting)), `copy`, `string`, `struct`, `typing`, `sys`, `os`, `pathlib`,
  `tempfile`, `gc`, `weakref`, `random` (Random API + MT parity: `seed`/`getstate`/`setstate`, `randrange`/`randint`/`shuffle`, `choice`/`choices`/`sample`, `randbytes`, `SystemRandom` via `os.urandom`, plus distributions: `uniform`, `triangular`, `normalvariate`, `gauss`, `lognormvariate`, `expovariate`, `vonmisesvariate`, `gammavariate`, `betavariate`, `paretovariate`, `weibullvariate`, `binomialvariate`), `time` (`monotonic`, `perf_counter`, `process_time`, `sleep`, `get_clock_info`, `time`/`time_ns` gated by `time.wall`, plus `localtime`/`gmtime`/`strftime` + `struct_time` + `asctime`/`ctime` + `timezone`/`daylight`/`altzone`/`tzname` + `mktime` + `timegm`), `json` (loads/dumps with parse hooks, indent, separators, allow_nan, `JSONEncoder`/`JSONDecoder`, `JSONDecodeError` details), `base64` (b16/b32/b32hex/b64/b85/a85/z85 encode/decode + urlsafe + legacy helpers), `hashlib`/`hmac` (Rust intrinsics for guaranteed algorithms + `pbkdf2_hmac`/`scrypt`; unsupported algorithms raise), `pickle` (protocols `0..5` on intrinsic core path, including protocol-`2+` memo/reducer/extension and persistent-hook lanes; still intrinsic-partial for full CPython 3.12+ edge semantics),
  `socket` (runtime-backed, capability-gated; fd duplication/fromfd/inheritable plus socket-file reader read/readline paths route via Rust intrinsics, `dup` now clones via runtime socket-handle intrinsic, default-timeout validation is CPython-shaped, and `gethostbyaddr`/`getfqdn` now lower through dedicated Rust intrinsics; advanced options + wasm parity pending), `select` (`select.select` + `poll`/`epoll`/`kqueue`/`devpoll` objects now intrinsic-backed via runtime selector registries),
  `selectors` (CPython-shaped backend classes now route through intrinsic-backed `select` objects rather than Python async fan-out; focused differential coverage now locks register/modify/unregister semantics, timeout normalization (`timeout <= 0` uses non-blocking polls), event-mask filtering, and `DefaultSelector` backend readiness behavior), `asyncio`, `contextvars`, `contextlib`, `threading`, `zipfile`, `zipimport`,
  `functools`, `itertools`, `operator`, `heapq`, `collections`.
  Supported shims: `keyword` (`kwlist`/`softkwlist`, `iskeyword`, `issoftkeyword`), `pprint` (PrettyPrinter/pformat/pprint parity).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand zipfile/zipimport with bytecode caching + broader archive support.
- Implemented: `zipfile` CRC32 hot path is now intrinsic-backed (`molt_zipfile_crc32`), removing Python-side table construction and fixing backend compile instability in `zipimport_basic` differential lanes.
- Implemented: `zipfile` central-directory parsing and ZIP64-extra payload construction now lower through dedicated Rust intrinsics (`molt_zipfile_parse_central_directory`, `molt_zipfile_build_zip64_extra`), `zipfile._path` directory/implied-dir matching and glob translation route through dedicated Rust intrinsics (`molt_zipfile_path_implied_dirs`, `molt_zipfile_path_resolve_dir`, `molt_zipfile_path_is_child`, `molt_zipfile_path_translate_glob`), and `zipfile.main` extract-path sanitization lowers through `molt_zipfile_normalize_member_path`.
- Implemented: `pathlib.PureWindowsPath` now matches CPython drive/anchor/parts/parent semantics in the intrinsic-first shim, including UNC and drive-root edge cases.
- Implemented: `smtplib.SMTP.sendmail` parity slice is wired (`MAIL`/`RCPT`/`DATA` flow with refused-recipient payload) and covered by differential test `tests/differential/stdlib/smtplib_sendmail_basic.py`.
- Implemented: `zipimport` API parity expanded with `zipimporter.get_filename` + `zipimporter.is_package`; `get_source` now raises `ZipImportError` for missing modules (CPython behavior).
- Implemented: `bisect`/`_bisect` are now fully intrinsic-backed on compiled paths (`molt_bisect_left`, `molt_bisect_right`, `molt_bisect_insort_left`, `molt_bisect_insort_right`), with CPython-style top-level aliases (`bisect`, `insort`) and targeted differential/API-gap probes green.
- Implemented: `zipfile` read-path object-state hardening now reconstructs central-directory index on demand when compiled object state is incomplete (`ZipFile.namelist`/`ZipFile.read`).
- Implemented: differential RSS top summaries now resolve status from final diff outcome (`pass`/`fail`/`skip`/`oom`) instead of attempt-level run status; regression coverage is in `tests/test_molt_diff_expected_failures.py`.
- Implemented: wasm linker post-processing now tolerates malformed UTF-8 function names in optional wasm `name` sections while appending table-ref elements (invalid entries are skipped instead of failing linked build).
- Implemented: wasm runner Node selection is now deterministic and version-gated (`MOLT_NODE_BIN` override + auto-select Node >= 18), and `run_wasm.js` now resolves WASI via `node:wasi` first, then `wasi`, with an explicit actionable error when unavailable.
- Implemented: wasm socket constants payload now exports core CPython-facing names (`AF_INET`, `AF_INET6`, `AF_UNIX`, `SOCK_STREAM`, `SOL_SOCKET`, etc.) via runtime intrinsic `molt_socket_constants`, eliminating missing-constant failures in socket bootstrap consumers.
- Implemented: linked-wasm async poll dispatch is now table-base-aware (with legacy slot normalization), linked artifacts export `molt_set_wasm_table_base`, and scheduler task execution no longer recursively acquires `task_queue_lock`; this closes the `molt_call_indirect1` signature-mismatch + recursive no-threads mutex panic path seen in wasm runtime-heavy lanes.
- Implemented: targeted wasm runtime-heavy regression lane is green for this tranche (`tests/test_wasm_runtime_heavy_regressions.py`): asyncio task basic no longer traps, zipimport behavior matches CPython failure shape for `zipimporter(zip_path).load_module(\"pkg.mod\")`, and smtplib thread-dependent path fails fast deterministically with `NotImplementedError`.
- Implemented: logging percent-style format fallback is now intrinsic-backed (`molt_logging_percent_style_format`) with differential regression `tests/differential/stdlib/logging_percent_style_intrinsic.py`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close parity gaps for `ast`, `ctypes`, and `urllib.parse`/`urllib.error`/`urllib.request` per matrix coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): complete socket/select/selectors parity (OS-specific flags, fd inheritance, error mapping, cancellation) and align with asyncio adapters.
- Implemented: wasm/non-Unix socket host ABI now carries ancillary payload buffers and recvmsg `msg_flags` for `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into`; runtime no longer hardcodes `msg_flags=0` in wasm paths.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` cross-platform ancillary parity (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`), while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty ancillary control messages.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete asyncio transport/runtime parity after intrinsic capability gates (full SSL transport semantics, Unix-socket behavior parity across native/wasm, and child-watcher behavior depth on supported hosts).
- Implemented: intrinsic-backed `pathlib.Path.glob`/`rglob` segment matching now covers `*`/`?`/`[]` classes plus recursive `**` traversal in the runtime matcher (no Python fallback path).
- Implemented: `os.read`/`os.write` are now Rust-intrinsic-backed (`molt_os_read`/`molt_os_write`) and validated with differential coverage (`os_read_write_basic.py`, `os_read_write_errors.py`) in intrinsic-only compiled runs.
- Implemented: threading basic differential lane (`tests/differential/basic/threading_*.py`) is green (`24/24`) under intrinsic-only compiled runs with RSS profiling enabled.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs exist for regrtest (support: captured_output/captured_stdout/captured_stderr, check_syntax_error, findfile, run_with_tz, warnings_helper utilities: check_warnings/check_no_warnings/check_no_resource_warning/check_syntax_warning/ignore_warnings/import_deprecated/save_restore_warnings_filters/WarningsRecorder, cpython_only, requires, swap_attr/swap_item, import_helper basics: import_module/import_fresh_module/make_legacy_pyc/ready_to_import/frozen_modules/multi_interp_extensions_check/DirsOnSysPath/isolated_modules/modules_setup/modules_cleanup, os_helper basics: temp_dir/temp_cwd/unlink/rmtree/rmdir/make_bad_fd/can_symlink/skip_unless_symlink + TESTFN constants); doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Implemented: `os.environ` mapping methods are runtime-intrinsic-backed (`molt_env_snapshot`/`molt_env_set`/`molt_env_unset`) with str-only key/value checks; `os.putenv`/`os.unsetenv` are lowered to dedicated runtime intrinsics (`molt_env_putenv`/`molt_env_unsetenv`) and keep CPython-style separation from `os.environ`/`os.getenv`.
- Implemented: `os.mkdir`/`os.makedirs` now pass `mode` through Rust intrinsics (`molt_path_mkdir(path, mode)` / `molt_path_makedirs(path, mode, exist_ok)`), including `__index__` exception-propagation parity for non-int mode adapters; focused diff coverage is green in `os_mkdir_makedirs_mode_intrinsic.py`.
- Implemented: `sys` metadata attrs `hexversion`, `api_version`, `abiflags`, and `implementation` are now intrinsic-backed (`molt_sys_hexversion`, `molt_sys_api_version`, `molt_sys_abiflags`, `molt_sys_implementation_payload`) with validated payload shaping in the shim; focused diff coverage is green in `sys_metadata_intrinsics.py`.
- Implemented: `weakref.ReferenceType.__callback__` now lowers through Rust intrinsic `molt_weakref_callback` with CPython-style alive/dead behavior (`None` after referent collection); focused diff coverage is green in `weakref_callback_property_intrinsic.py`.
- Implemented: `os.stat`/`os.lstat`/`os.fstat`/`os.rename`/`os.replace` are now Rust-intrinsic-backed (`molt_os_stat`, `molt_os_lstat`, `molt_os_fstat`, `molt_os_rename`, `molt_os_replace`) with thin shim-side `os.stat_result` shaping and focused coverage in `os_stat_rename_replace_intrinsic.py`.
- Implemented: `sys.flags` now lowers through Rust payload intrinsic `molt_sys_flags_payload` with shim-side payload validation and CPython-ordered sequence-field shaping; focused coverage is in `sys_flags_intrinsic.py`.
- Implemented: `token` now boots through intrinsic readiness (`molt_import_smoke_runtime_ready`) and loads CPython 3.12 token payload objects from Rust via `molt_token_payload_312` (backed by runtime payload data), covering constants, `tok_name`, `EXACT_TOKEN_TYPES`, helper predicates (`ISTERMINAL`/`ISNONTERMINAL`/`ISEOF`), and `__all__`; focused differential coverage is in `tests/differential/stdlib/token_core_api.py`.
- Implemented: weakref finalize edge transitions (`detach`/`peek`/`alive` and idempotent invocation semantics) now have focused differential coverage in `weakref_finalize_detach_peek_edges.py` alongside existing atexit-ordering parity coverage.
- Implemented: uuid module parity (UUID accessors, `uuid1`/`uuid3`/`uuid4`/`uuid5`, namespaces, SafeUUID).
- Implemented: collections.abc parity (ABC registration, structural checks, mixins).
- Implemented: `atexit` core callback registry/shutdown-drain execution is intrinsic-backed (no import-only stub lane on compiled runs), including callback continuation after failures, version-gated stderr callback+traceback reporting parity (3.12 vs 3.13+ punctuation shape), custom `sys.unraisablehook` payload parity (`UnraisableHookArgs` shape/version semantics), and weakref-finalizer runner ordering parity with user callbacks.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (runtime fast-path parser + performance tuning).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module exposes only minimal toggles/collect; wire to runtime cycle collector and implement full API.
- Implemented: `abc.update_abstractmethods` now lowers through Rust intrinsic `molt_abc_update_abstractmethods` (no Python-side abstract-method scanning loop in `abc.py`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close `_abc` edge-case cache/version parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): replace placeholder iterator/view types (`object`/`type`) so ABC registration doesn't need guards.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `_asyncio` shim now uses intrinsic-backed running-loop hooks; broader C-accelerated parity remains pending.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): asyncio submodule parity (events/tasks/streams/etc) beyond import-only allowlisting.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `_bz2` compression backend parity for `bz2`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `random` distribution test vectors and edge-case coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): `struct` intrinsics cover the CPython 3.12 format table (including half-float) with endianness + alignment and C-contiguous memoryview chain handling for pack/unpack/pack_into/unpack_from; remaining gaps are exact CPython diagnostic-text parity on selected edge cases.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): deterministic `time` clock policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics (`setlocale` catalog coverage, category handling, and host-locale compatibility).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement gettext translation catalog/domain parity (filesystem-backed `.mo` loading and locale/domain selection).
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale data for `time.localtime`/`time.strftime` on wasm hosts.
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): Node/V8 Zone OOM can still reproduce on some linked runtime-heavy modules in unrestricted/manual Node runs; parity and benchmark runners now enforce `--no-warnings --no-wasm-tier-up --no-wasm-dynamic-tiering --wasm-num-compilation-tasks=1` while root-causing host/runtime interaction.
- Implemented: `_asyncio` wasm running-loop panic root cause fixed in runtime zip layout strict-bits access (`runtime/molt-runtime/src/object/layout.rs`: unaligned `read`/`write` for wasm-safe metadata loads/stores).
- TODO(wasm-parity, owner:stdlib, milestone:SL2, priority:P0, status:partial): runtime-heavy wasm server lanes that depend on `threading` remain blocked (threads are unavailable in wasm); keep these as promotion blockers for `smtplib`/socketserver-style workloads until a supported wasm threading strategy is finalized.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/surrogatepass/namereplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (full encodings import hooks + charmap codec intrinsics); incremental encoder/decoder now backed by Rust handle-based intrinsics, BOM constants from Rust, register_error/lookup_error wired, base encode/decode + registry/lookup present.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete `pickle` CPython 3.12+ parity (remaining reducer/object-hook edges, full Pickler/Unpickler class-surface/error-text parity, and exhaustive protocol-5 buffer/graph corner semantics beyond current class/dataclass/reducer lanes).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` shim covers constants, predicates, `trunc`/`floor`/`ceil`, `fabs`/`copysign`/`fmod`/`modf`, `frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`, and `sqrt`; Rust intrinsics cover predicates (`isfinite`/`isinf`/`isnan`), `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc`; remaining: determinism policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish remaining `types` shims (CapsuleType + any missing helper/descriptor types).
- Import-only stubs: `collections.abc`, `_collections_abc`, `_asyncio`, `_bz2`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement core collections.abc surfaces.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib extension/sourceless execution parity beyond capability-gated restricted-source shim lanes.
- Implemented: relative import resolution now honors `__package__`/`__spec__` metadata (including `__main__`) and namespace packages, with CPython-matching errors for missing or over-deep parents.
- Implemented: `importlib.resources` custom loader reader contract parity is now wired through reader-backed traversables (`contents`/`is_resource`/`open_resource`/`resource_path`) on top of intrinsic namespace + archive resource payloads, with archive-member path tagging in runtime payloads so `resource_path()` stays filesystem-only across direct + traversable + roots fallback lanes, while archive reads remain intrinsic-backed via `open_resource()`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata dependency/advanced metadata semantics beyond intrinsic payload parsing.
- Implemented: `http.cookies` now has an intrinsic-backed SimpleCookie/Morsel subset (`__setitem__`/`__getitem__`, `load("a=1; b=2")`, `output`, items iteration, and focused morsel attrs: `path`/`secure`/`httponly`/`max-age`/`expires`), with remaining parity tracked in the compatibility matrix.
- Planned import-only stubs: `html`, `html.parser`,
  `ipaddress`, `mimetypes`, `wsgiref`, `xml`, `email.policy`, `email.message`, `email.parser`,
  `email.utils`, `email.header`, `urllib.robotparser`,
  `logging.config`, `logging.handlers`, `cgi`, `zlib`.
  Additional 3.12+ planned/import-only modules (e.g., `annotationlib`, `codecs`, `configparser`,
  `difflib`, `dis`, `encodings`, `tokenize`, `trace`, `xmlrpc`, `zipapp`) are tracked in
  [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md) Section 3.0b.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + coverage smoke tests.)
- See [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md) for the full matrix.

## Django Demo Blockers (Current)
- Remaining stdlib gaps for Django internals: `operator` intrinsics, richer `collections` perf (runtime deque), and `re`/`datetime`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): operator intrinsics + runtime deque + `re`/`datetime` parity.)
- Async loop/task APIs + `contextvars` cover Task/Future/gather/Event/`wait_for`;
  task groups/wait/shield plus async I/O cancellation propagation and long-running
  workload hardening are pending.
  (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): task groups/wait/shield + I/O cancellation + hardening.)
- Top priority: finish wasm parity for DB connectors before full DB adapter expansion (see [docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md](docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md)).
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
- Dev build throughput controls are available and enabled by default: `--profile dev` routes to Cargo `dev-fast`; native backend compiles use a persistent backend daemon with lock-coordinated restart/retry; shared build state (locks/fingerprints) lives under `<CARGO_TARGET_DIR>/.molt_state/` (override with `MOLT_BUILD_STATE_DIR`) while daemon sockets default to `MOLT_BACKEND_DAEMON_SOCKET_DIR` (local temp path).
- Throughput tooling is available for repeatable setup + measurement: `tools/throughput_env.sh`, `tools/throughput_matrix.py`, and `tools/molt_cache_prune.py`.
- Release compile iteration lane is available via Cargo profile override `MOLT_RELEASE_CARGO_PROFILE=release-fast`; `tools/compile_progress.py` includes dedicated `release_fast_cold`, `release_fast_warm`, and `release_fast_nocache_warm` cases for measurement and regression tracking.
- Friend-suite benchmarking harness is available via `tools/bench_friends.py` with pinned manifest configuration in `bench/friends/manifest.toml`; runs emit reproducible JSON/markdown artifacts and can publish [docs/benchmarks/friend_summary.md](docs/benchmarks/friend_summary.md).
- On macOS arm64, uv runs that target Python 3.14 force `--no-managed-python` and
  require a system `python3.14` to avoid uv-managed hangs.
- WIT interface contract lives at `wit/molt-runtime.wit` (WASM runtime intrinsics).
- Single-module wasm linking via `tools/wasm_link.py` (requires `wasm-ld`) is required for Node/wasmtime runs of runtime outputs; enable with `--linked`/`--require-linked` (or `MOLT_WASM_LINK=1`).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): harden backend daemon lane (multi-job compile API, unbounded daemon request/job/cache service mode for large-machine deployments, richer health telemetry, deterministic readiness/restart semantics, and config-digest lane separation with cache reset-on-change are landed; remaining work is sustained high-contention soak evidence + restart/backoff tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add batch compile server mode for diff runs to amortize backend startup and reduce per-test compile overhead (in-process JSON-line batch server, hard request deadlines, force-close shutdown, cooldown-based retry hardening, and fail-open/strict modes are landed behind env gates; remaining work is default-on rollout criteria + perf guard thresholds).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add function-level object caching so unchanged functions can be relinked without recompiling whole scripts (function cache-key lane now includes backend codegen-env digest + IR top-level extras digest, module/function cache-tier fallback + daemon function-cache plumbing are landed, import-closure reuse now shares module-resolution caches across graph discovery/package-parent/namespace-parent passes, and invalid cached-artifact guard + daemon cache-tier telemetry are wired; remaining work is import-graph-aware scheduling across diff batches + fleet-level perf tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add import-graph-aware diff scheduling and distributed cache playbooks for multi-host agent fleets.
- TODO(perf, owner:tooling, milestone:TL2, priority:P1, status:partial): finish friend-owned suite adapters (Codon/PyPy/Nuitka/Pyodide), pin immutable suite refs/commands, and enable nightly friend scorecard publication.

## Known Gaps
- Documented: `Molt Edge` is now proposed as an explicit Edge/Workers tier with a
  minimal VFS, snapshot-oriented deployment, and Cloudflare-first host profile;
  canonical guidance now lives in 0294, 0295, and 0968.
- Browser host harness is available under `wasm/browser_host.html` with
  DB host support, WebSocket-backed stream sockets, and websocket host intrinsics; production browser host I/O is still pending for storage + broader parity coverage.
  (TODO(wasm-host, owner:runtime, milestone:RT3, priority:P2, status:partial): add browser host I/O bindings + capability plumbing for storage and parity tests.)
- TODO(wasm-host, owner:runtime, milestone:RT3, priority:P1, status:planned): implement the Edge/Workers minimal VFS contract (`/bundle`, `/tmp`, stdio devices, optional `/state`) and parity tests across browser, WASI, and Cloudflare Worker hosts.
- TODO(wasm-host, owner:runtime, milestone:RT3, priority:P1, status:planned): define and implement `molt.snapshot` generation/restore for edge deployments, including deterministic init rules, capability manifest capture, and cold-start benchmark reporting.
- Cross-target native builds (non-host triples/architectures) are not yet wired into
  the CLI/build pipeline.
  (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): wire cross-target builds into CLI.)
- SQLite/Postgres connectors remain native-only; wasm DB host adapters exist (Node/WASI + browser), parity tests now cover browser host cancellation + Arrow IPC payload delivery, but real backend coverage is still pending.
  (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity with real backend coverage.)
- Single-module WASM link now rejects `molt_call_indirect*` imports, `reloc.*`/`linking`/`dylink.0` sections, and table/memory imports; element segments are validated to target table 0 with `ref.null`/`ref.func` init exprs. Linked runs no longer rely on JS call_indirect stubs (direct-link path still uses env wrappers by design).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): re-enable safe direct-linking by relocating the runtime heap base or enforcing non-overlapping memory layouts to avoid wasm-ld in hot loops.
- Implemented: linked-wasm dynamic intrinsic dispatch no longer requires Python static-dispatch shims for channel intrinsics; runtime uses a canonical 64-bit channel handle ABI so dynamic intrinsic calls and direct calls share the same call_indirect signature.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): OPT-0003 phase 1 landed (sharded pointer registry); benchmark and evaluate lock-free alternatives next (see [OPTIMIZATIONS_PLAN.md](../../OPTIMIZATIONS_PLAN.md)).
- Single-module wasm linking remains experimental; wasm-ld links relocatable output when `MOLT_WASM_LINK=1`, but broader module coverage is still pending (direct-link runs are disabled for now).

## TODO Mirror Ledger (Auto-Generated)
<!-- BEGIN TODO MIRROR LEDGER -->
- DONE(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:done): async generator lowering and runtime parity (`async def` with `yield`) — fully implemented at all layers. PEP 479 analog StopAsyncIteration→RuntimeError conversion fixed 2026-03-01.
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P0, status:planned): Rust event loop + I/O poller with cancellation propagation and deterministic scheduling guarantees; expose as asyncio core.
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): cancellation injection on await).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): task-based concurrency).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): wasm async iteration/scheduler parity.
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): wasm scheduler semantics).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): executor integration).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): native-only tokio host adapter for compiled async tasks with determinism guard + capability gating (no WASM impact).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): Implement the remaining `libmolt` C-API v0 surface per `0214` and keep this matrix aligned with real coverage.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:done): minimal `libmolt` C-API bootstrap subset (buffer, numerics, sequence/mapping, errors, GIL mapping) as the primary C-extension compatibility foundation.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): expanded `libmolt` wrappers with type/module parity entry points (`molt_type_ready`, module create/add/get APIs with bytes-name + constant helpers), runtime-owned module definition/state registries, callback-backed method wrappers (`molt_cfunction_create_bytes`, `molt_module_add_cfunction_bytes`), CPython-compat module helpers (`PyModule_New(Object)`, `PyModule_GetDef`, `PyModule_GetState`, `PyModule_SetDocString`, `PyModule_GetFilename(Object)`, `PyModule_FromDefAndSpec(2)`, `PyModule_ExecDef`, `PyState_*`), and partial `PyType` parity (`PyType_FromSpec(WithBases)` selected slot lowering + `METH_CLASS`/`METH_STATIC`, plus `PyType_FromModuleAndSpec` and `PyType_GetModule`/`PyType_GetModuleState`/`PyType_GetModuleByDef`).
- Implemented(tooling, owner:tooling, milestone:SL3, priority:P1, status:partial): extension runtime load/import boundaries now enforce manifest ABI/capability/checksum validation (including module-mismatch rejection at loader/exec boundaries), and CI covers extension wheel `build + scan + audit + verify + publish --dry-run` in `linux native`, `linux cross-aarch64-gnu`, `linux cross-musl`, `macos native`, and `windows native` lanes plus wasm-target rejection checks on the `linux native` lane.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): landed `PyArg_ParseTuple` + `PyArg_ParseTupleAndKeywords` shims for the current fast-path format subset (`O,O!,b,B,h,H,i,I,l,k,L,K,n,c,d,f,p,s,s#,z,z#,y#`, `|`, `$`) with kwlist lookup and duplicate positional/keyword detection.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): scan-driven CPython header parity sprint expanded `include/molt/Python.h` with high-impact wrappers/macros for type/ref helpers, tuple/list/dict fast helpers, memory allocators, thread/GIL shims, compare/call builders (`PyObject_CallFunction*`, `Py_BuildValue` subset), capsule/buffer helpers, module/capsule import helpers (`PyImport_ImportModule`, `PyCapsule_Import`), `PyArg_UnpackTuple`, iter/number/float helpers, and set/complex checks; initial NumPy compatibility headers (`include/numpy/*`) are now wired into extension scans and CI extension matrix lanes with added DType/scalar helper stubs, and a minimal `datetime.h` shim lane (`PyDateTimeAPI`/`PyDateTime_IMPORT`/basic checks) is now surfaced to extension scans; NumPy/Pandas source scans (NumPy `2.4.2`, pandas `3.0.1`) now report `Py*` missing-symbol counts reduced from `1484/189` to `1193/28`.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define `libmolt` C-extension ABI surface + bridge policy).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define and implement `libmolt` C API shim + `Py_LIMITED_API` target (see [docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md](docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md)).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): Define the `Py_LIMITED_API` version Molt targets (3.10?).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): define hollow-symbol policy + error surface).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add per-pass wall-time telemetry (`attempted`/`accepted`/`rejected`/`degraded`, `ms_total`, `ms_p95`) plus top-offender diagnostics by module/function/pass (frontend pass telemetry, CLI/JSON sink wiring, and hotspot rendering are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add tiered optimization policy (Tier A entry/hot functions, Tier B normal user functions, Tier C heavy stdlib/dependency functions) with deterministic classification and override knobs (baseline classifier + env override knobs are landed; runtime-feedback and PGO hot-function promotion are now wired through the existing tier promotion path).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): enforce per-function mid-end wall-time budgets with an automatic degrade ladder that disables expensive transforms before correctness gates and records degrade reasons (budget/degrade ladder is landed in fixed-point loop; tuning heuristics and function-level diagnostics surfacing remain).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): ship profile-gated mid-end policy matrix (`dev` correctness-first cheap opts; `release` full fixed-point) with deterministic pass ordering and explicit diagnostics (profile plumbing into frontend policy is landed; diagnostics sink now also surfaces active midend policy config and heuristic knobs; remaining work is broader tuning closure and any additional triage UX).
- TODO(compiler, owner:compiler, milestone:RT2, priority:P2, status:planned): canonical loop lowering).
- TODO(compiler, owner:compiler, milestone:RT2, priority:P2, status:planned): dict version tag guards).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P0, status:implemented): root-cause/fix mid-end miscompiles feeding missing values into runtime lookup/call sites (SCCP treats MISSING as non-propagatable via _SCCP_MISSING sentinel, DCE protects MISSING ops from elimination, definite-assignment verifier tracks MISSING definitions explicitly; dev-profile gate removed — mid-end runs for both dev and release profiles; stdlib gate remains until canonicalized stdlib lowering is proven stable).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P0, status:partial): root-cause and fix stdlib mid-end miscompiles that can route missing values into runtime lookups/call sites; keep this hard safety gate until canonicalized stdlib lowering is proven stable (user-code MISSING-value fixes landed; stdlib gate remains active).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P0, status:implemented): root-cause/fix dev-profile mid-end miscompiles before re-enabling by default (SCCP/DCE/verifier hardening landed; dev-profile gate removed — mid-end enabled by default for all profiles).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P1, status:partial): restore PHI-based bool-op lowering once PHI merge semantics preserve operand objects exactly for short-circuit expressions.
- TODO(dataframe, owner:runtime, milestone:DF1, priority:P1, status:planned): missing-data promotion rules).
- TODO(dataframe, owner:runtime, milestone:DF1, priority:P1, status:planned): nullable dtype missing-data semantics)
- TODO(dataframe, owner:runtime, milestone:DF1, priority:P2, status:planned): dictionary encoding for strings).
- TODO(dataframe, owner:runtime, milestone:DF2, priority:P2, status:planned): Molt-native kernel data model).
- TODO(dataframe, owner:runtime, milestone:DF2, priority:P2, status:planned): Molt-native kernel library)
- TODO(dataframe, owner:runtime, milestone:DF2, priority:P2, status:planned): decimal dtype semantics)
- TODO(dataframe, owner:runtime, milestone:DF2, priority:P2, status:planned): pandas-style index semantics + oracle tests).
- TODO(dataframe, owner:runtime, milestone:DF2, priority:P2, status:planned): timezone-aware datetime support)
- TODO(db, owner:runtime, milestone:DB1, priority:P1, status:planned): SQLite demo path before Postgres).
- TODO(db, owner:runtime, milestone:DB1, priority:P2, status:planned): json/jsonb decode policy).
- TODO(db, owner:runtime, milestone:DB1, priority:P2, status:planned): option vs sentinel policy).
- TODO(db, owner:runtime, milestone:DB1, priority:P2, status:planned): unsupported type fallback policy).
- TODO(db, owner:runtime, milestone:DB2, priority:P1, status:partial): native database drivers).
- TODO(db, owner:runtime, milestone:DB2, priority:P2, status:planned): expression expansion).
- TODO(db, owner:runtime, milestone:DB2, priority:P2, status:planned): real Postgres swap).
- TODO(db, owner:runtime, milestone:DB2, priority:P2, status:planned): window function support).
- TODO(db, owner:runtime, milestone:DB3, priority:P3, status:planned): ORM-like facade).
- TODO(docs, owner:docs, milestone:SL1, priority:P3, status:planned): add `TODO(stdlib-compat, ...)` markers for interim gaps.
- TODO(docs, owner:docs, milestone:SL2, priority:P3, status:planned): document unsupported re features).
- TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:missing): native HTTP package).
- TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:missing): native WebSocket + streaming I/O).
- TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:planned): WebSocket host connect hook + capability registry).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root build discovery (namespace packages + PYTHONPATH roots done; remaining: deterministic graph caching + `__init__` edge cases).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): project-root builds (package discovery hardening, `__init__` edge handling, deterministic dependency graph caching).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): project-root builds (package discovery, `__init__` handling, namespace packages, deterministic dependency graph caching).
- TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame fields (f_back, f_globals, f_locals) and keep f_lasti/f_lineno updated.
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame objects to CPython parity (`f_globals`, `f_locals`, `f_lasti`, `f_lineno` updates).
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame/traceback objects to CPython parity (`f_back`, `f_globals`, `f_locals`, live `f_lasti`/`f_lineno`).
- TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): complete code object parity for closure/generator/coroutine metadata (`co_freevars`/`co_cellvars` values and full `co_flags` bitmask semantics).
- TODO(introspection, owner:runtime, milestone:TC3, priority:P2, status:missing): full frame objects + `gi_code` parity.
- TODO(observability, owner:tooling, milestone:TL2, priority:P3, status:planned): Prometheus integration).
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): Django test-client coverage + retry policy). `molt_accel` ships as an optional dependency group (`pip install .[accel]`) with a packaged default exports manifest so the decorator can fall back to `molt-worker` in PATH when `MOLT_WORKER_CMD` is unset. A demo app scaffold lives in `demo/`.
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): compile entrypoints into molt_worker.
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): finalize accel retry/backoff + non-demo handler coverage.)
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): propagate cancellation into real DB tasks; extend compiled handlers beyond demo coverage.
- TODO(offload, owner:runtime, milestone:SL1, priority:P2, status:planned): adapter/DB contract path).
- TODO(opcode-matrix, owner:frontend, milestone:M2, priority:P3, status:planned): Optimize `SETUP_WITH` to inline `__enter__` (Milestone 2).
- Implemented: `MATCH_*` semantics via AST desugaring (PEP 634 full coverage, 24 differential test files).
- Implemented: pattern matching (`match`/`case`) lowering with all PEP 634 pattern types.
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:missing): awaitable `__aiter__` support). |
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): Add async generator op coverage (e.g., `ASYNC_GEN_WRAP`) and confirm lowering gaps.
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator coverage). |
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator op coverage). |
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator opcode coverage and lowering gaps (see [docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md)).
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:planned): expand KW_NAMES error-path coverage (duplicate keywords, positional-only violations) in differential tests.
- TODO(packaging, owner:tooling, milestone:SL2, priority:P2, status:partial): default wire codecs to MsgPack/CBOR).
- TODO(perf, owner:compiler, milestone:RT2, priority:P1, status:planned): reduce startup/import-path dispatch overhead for stdlib-heavy scripts (bind intrinsic-backed imports at lower cost and trim module-init call traffic) so wins translate to short-lived CLI/data scripts as well as long-running services.
- TODO(perf, owner:compiler, milestone:RT2, priority:P1, status:planned): wasm `simd128` kernels for string scans.
- TODO(perf, owner:compiler, milestone:RT2, priority:P2, status:planned): simd128 short-needle kernels).
- TODO(perf, owner:compiler, milestone:RT2, priority:P2, status:planned): vectorizable region detection).
- TODO(perf, owner:compiler, milestone:TC2, priority:P1, status:planned): implement PEP 709-style comprehension inlining for list/set/dict comprehensions (beyond the simple range fast path), and gate rollout with pyperformance `comprehensions` + targeted differential comprehension tranche benchmarks.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:partial): SIMD kernels for reductions + scans).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): Unicode index caches + wider SIMD).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): float + int mix kernels).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): implement sharded/lock-free handle resolution and track lock-sensitive benchmark deltas (attr access, container ops).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): reduce handle-resolution overhead beyond the sharded registry and measure lock-sensitive benchmark deltas (attr access, container ops).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): reduce handle/registry lock scope and measure lock-sensitive benchmarks).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:partial): bytes/bytearray fast paths).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): 32-bit partials + overflow guards for `prod`).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): biased RC).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): cache type comparison dispatch on type objects).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): cached UTF-8 index tables for repeated non-ASCII `find`/`count`.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): implement a native Windows socketpair using WSAPROTOCOL_INFO or AF_UNIX to avoid loopback TCP overhead.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): pre-size `dict.fromkeys` using iterable length hints to reduce rehashing.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): profiling-driven vectorization).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): safe NEON multiply strategy).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): stream print writes to avoid building intermediate output strings for large payloads.
- TODO(perf, owner:runtime, milestone:RT3, priority:P3, status:planned): AVX-512 or 32-bit specialization for vectorized `prod` reductions.
- TODO(perf, owner:runtime, milestone:RT3, priority:P3, status:planned): AVX-512 reductions).
- TODO(perf, owner:tooling, milestone:TL2, priority:P1, status:partial): finish friend suite adapters/pinned command lanes and run nightly scorecards in CI.)
- TODO(perf, owner:tooling, milestone:TL2, priority:P2, status:planned): benchmarking regression gates).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): thread PyToken through runtime mutation entrypoints).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define per-runtime GIL strategy and runtime instance ownership model).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define the per-runtime GIL strategy, runtime instance ownership model, and allowed cross-thread object sharing rules (see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define the per-runtime GIL strategy, runtime instance ownership model, and the allowed cross-thread object sharing rules.
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): fork/forkserver currently map to spawn semantics; implement true fork support.
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): implement true fork support). |
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors, explicit message passing, and capability-gated shared-memory primitives.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): benchmark sharded registry
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:planned): replace pointer-registry locks with sharded or lock-free lookups once registry load is characterized.
- TODO(runtime-provenance, owner:runtime, milestone:RT2, priority:P2, status:planned): audit remaining pointer
- TODO(runtime-provenance, owner:runtime, milestone:RT2, priority:P2, status:planned): bound or evict transient const-pointer registrations in the pointer registry.
- TODO(security, owner:runtime, milestone:RT2, priority:P1, status:missing): memory/CPU quota enforcement for native binaries).
- TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.
- TODO(semantics, owner:runtime, milestone:LF1, priority:P1, status:partial): exception objects + last-exception plumbing. |
- TODO(semantics, owner:runtime, milestone:LF1, priority:P1, status:partial): exception propagation + suppression semantics for context manager exit paths.
- TODO(semantics, owner:runtime, milestone:TC1, priority:P0, status:planned): audit negative-indexing parity across indexable types + add differential coverage for error messages.
- TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): exception `__init__` + subclass attribute parity (UnicodeError fields, ExceptionGroup tree).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): tighten exception `__init__` + subclass attribute parity (ExceptionGroup tree).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): Formalize "Lazy Task" divergence policy.
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence policy (see [docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md](docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md)).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence). |
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): Implement cycle collector (currently pure RC).
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector implementation (see [docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md](docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md)).
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector). |
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): incremental mark-and-sweep GC).
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:partial): finalizer guarantees). |
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:partial): signal handling parity). |
- TODO(stdlib-compat, owner:frontend, milestone:SL1, priority:P2, status:planned): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:partial): finish `open`/file object parity (broader codecs + full error handlers, text-mode seek/tell cookies, Windows fileno/isatty) with differential + wasm coverage.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:missing): expose file handle `flush()` and wire wasm parity for file flushing.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): `array` + `struct` deterministic layouts and packing (struct intrinsics cover the CPython 3.12 format table with alignment + half-float support, and C-contiguous nested-memoryview buffer windows; remaining struct gap is exact CPython diagnostic-text parity on selected edge cases).
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): filesystem-encoding + surrogateescape decoding parity.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): `array` deterministic layout + buffer protocol.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): array runtime layout + buffer protocol).
- TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P2, status:planned): `hashlib` deterministic hashing policy.
- Policy lock (dynamic execution): compiled binaries intentionally stay on restricted-source import/runpy execution lanes; unrestricted code-object execution is deferred by policy, not an active burndown target (see `docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md`).
- Future reconsideration requires explicit capability gating, documented utility analysis, reproducible perf/memory evidence, and explicit user approval before implementation.
- Import statement parity update: `import os.path` now lowers through runtime `MODULE_IMPORT` when the dotted-name parent is allowlisted/known, so statement imports match intrinsic import paths and no longer raise `ImportError` on alias-backed `os.path` lanes.
- Focused non-stdlib TODO burndown refresh (2026-02-25): 17 real items are tracked for next-wave execution.
- Compiler mid-end gates: 10
- Runtime/module exec parity: 4
- Doctor/perf stragglers: 3
- Canonical focused set (17):
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): complete `CALL_INDIRECT` hardening with broader deopt reason telemetry (dedicated runtime lane, noncallable differential probe, CI-enforced probe execution/failure-queue linkage, and runtime-feedback counter `deopt_reasons.call_indirect_noncallable` are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): complete `INVOKE_FFI` hardening with broader deopt reason telemetry (bridge-lane marker, runtime capability gate, negative capability differential probe, CI-enforced probe execution/failure-queue linkage, and runtime-feedback counter `deopt_reasons.invoke_ffi_bridge_capability_denied` are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): harden `GUARD_TAG` specialization/deopt semantics + coverage (runtime-feedback counter `deopt_reasons.guard_tag_type_mismatch` is landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): harden `GUARD_DICT_SHAPE` invalidation/deopt semantics + coverage (runtime-feedback aggregate counter `deopt_reasons.guard_dict_shape_layout_mismatch` and per-reason breakdown counters are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): ship profile-gated mid-end policy matrix (`dev` correctness-first cheap opts; `release` full fixed-point) with deterministic pass ordering and explicit diagnostics (CLI->frontend profile plumbing is landed; diagnostics sink now also surfaces active midend policy config and heuristic knobs; remaining work is broader tuning closure and any additional triage UX).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add tiered optimization policy (Tier A entry/hot functions, Tier B normal user functions, Tier C heavy stdlib/dependency functions) with deterministic classification and override knobs (baseline deterministic classifier + env overrides are landed; runtime-feedback and PGO hot-function promotion are now wired through the existing tier promotion path).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): enforce per-function mid-end wall-time budgets with an automatic degrade ladder that disables expensive transforms before correctness gates and records degrade reasons (budget/degrade ladder is landed in fixed-point loop; heuristic tuning + diagnostics surfacing remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add per-pass wall-time telemetry (`attempted`/`accepted`/`rejected`/`degraded`, `ms_total`, `ms_p95`) plus top-offender diagnostics by module/function/pass (frontend per-pass timing/counters, CLI/JSON sink wiring, and hotspot rendering are landed).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P0, status:implemented): root-cause/fix mid-end miscompiles feeding missing values into runtime lookup/call sites (SCCP treats MISSING as non-propagatable via _SCCP_MISSING sentinel, DCE protects MISSING ops from elimination, definite-assignment verifier tracks MISSING definitions explicitly; dev-profile gate removed — mid-end runs for both dev and release profiles; stdlib gate remains until canonicalized stdlib lowering is proven stable).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P1, status:partial): restore PHI-based bool-op lowering once PHI merge semantics preserve operand objects exactly for short-circuit expressions.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root builds (namespace packages + PYTHONPATH roots supported; remaining: package discovery hardening, `__init__` edge cases, deterministic dependency graph caching).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P2, status:partial): full extension/sourceless execution parity beyond capability-gated restricted-source shim hooks.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): importlib.machinery pending parity (package/module shaping + file reads + restricted-source execution lanes are intrinsic-lowered; remaining loader/finder parity is namespace/extension/zip behavior).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures` (spawn-based partial; IPC + lifecycle parity pending).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): harden backend daemon lane (multi-job compile API, unbounded daemon request/job/cache service mode for large-machine deployments, richer health telemetry, deterministic readiness/restart semantics, and config-digest lane separation with cache reset-on-change are landed; remaining work is sustained high-contention soak evidence + restart/backoff tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add batch compile server mode for diff runs to amortize backend startup and reduce per-test compile overhead (in-process JSON-line batch server, hard request deadlines, force-close shutdown, cooldown-based retry hardening, and fail-open/strict modes are landed behind env gates; remaining work is default-on rollout criteria + perf guard thresholds).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add function-level object caching so unchanged functions can be relinked without recompiling whole scripts (function cache-key lane now includes backend codegen-env digest + IR top-level extras digest, module/function cache-tier fallback + daemon function-cache plumbing are landed, import-closure reuse now shares module-resolution caches across graph discovery/package-parent/namespace-parent passes, and invalid cached-artifact guard + daemon cache-tier telemetry are wired; remaining work is import-graph-aware scheduling across diff batches + fleet-level perf tuning).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:partial): finish `io` parity (codec coverage, Windows isatty).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:partial): io pending parity) |
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): Bridge phase 1 (worker-process bridge default when enabled; Arrow IPC/MsgPack/CBOR batching; profiling warnings).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): Bridge phase 2 (embedded CPython feature flag + deterministic denylist + effect contracts; never default).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract (IPC/ABI, capability gating, deterministic denylist for C extensions) as an explicit, opt-in compatibility layer only.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract (IPC/ABI, capability gating, deterministic fallback for C extensions).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge contract and enforcement hooks.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge phase 1 (dev-only embedded CPython; no production).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge phase 2 (capability-gated embedded bridge + effect contracts).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge phase 3 (worker-process default + Arrow/MsgPack/CBOR batching).
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures` (spawn-based partial; IPC + lifecycle parity pending).
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): bootstrap `sys.stdout` so print(file=None) always honors the sys stream.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/backslashreplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:runtime, milestone:TC2, priority:P2, status:missing): `str(bytes, encoding, errors)` decoding parity for bytes-like inputs.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): extend XML-RPC coverage to support full marshalling/fault handling and introspection APIs with Rust-backed parsing/serialization.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): extend `zipapp` coverage to full CPython semantics (interpreter shebangs, custom entry-points, and in-memory target handling) via Rust intrinsics.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): extend queue-backed logging handler parity for advanced listener lifecycle and queue edge cases after baseline stdlib queue support stabilizes.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): replace the minimal built-in timezone table with a full IANA tzdb-backed ZoneInfo implementation in Rust intrinsics.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): runtime backlog.\n",
- TODO(stdlib-compat, owner:stdlib, milestone:LF1, priority:P1, status:missing): `contextlib.contextmanager` lowering and generator-based manager support.
- TODO(stdlib-compat, owner:stdlib, milestone:LF3, priority:P2, status:planned): expand `io`/`pathlib` to buffered + streaming wrappers with capability gates.
- TODO(stdlib-compat, owner:stdlib, milestone:LF3, priority:P2, status:planned): io/pathlib stubs + capability enforcement. |
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python stdlib modules with Rust intrinsics-only implementations (thin wrappers only); compiled binaries must reject Python-only stdlib modules. See `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.",
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:partial): test fixture partial marker.\n"
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:missing): full `open`/file object parity (modes/buffering/text/encoding/newline/fileno/seek/tell/iter/context manager) with differential + wasm coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` intrinsics + float determinism policy (non-transcendentals covered; trig/log/exp parity pending).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `struct` alignment + full format table parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill `builtins` module attribute coverage.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill out remaining `math` intrinsics (determinism policy).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): finish remaining `math` intrinsics (determinism policy); predicates, `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc` are now wired in Rust.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): implement full struct format/alignment parity.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` (`deque`, `Counter`, `defaultdict`) parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` runtime `deque` type + O(1) ops + view parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `functools` fast paths (`lru_cache`, `partial`, `reduce`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `itertools` + `operator` core-adjacent intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): `struct` intrinsics cover `pack`/`unpack`/`calcsize` + `pack_into`/`unpack_from`/`iter_unpack` across the CPython 3.12 format table (including half-float) with C-contiguous nested-memoryview windows; remaining gaps are exact CPython diagnostic-text parity on selected edge cases.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): align remaining struct edge-case error text with CPython.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `bisect` helpers + fast paths.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `heapq` randomized stress + perf tracking.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module API + runtime cycle collector hook.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (Encoder/Decoder classes, JSONDecodeError details, runtime fast-path parser).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): advance native `re` engine to full syntax/flags/groups; native engine covers core syntax (literals, `.`, classes/ranges, groups/alternation, greedy + non-greedy quantifiers) and `IGNORECASE`/`MULTILINE`/`DOTALL`; advanced features/flags raise `NotImplementedError` (no host fallback).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): close
- Implemented(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:implemented): Decimal arithmetic + formatting parity (all operators, math methods, predicates, copy ops, conversions wired to Rust intrinsics).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Python 3.12+ statistics API/PEP parity beyond function surface lowering (for example NormalDist and remaining edge-case text parity).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` ancillary-data parity (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`) while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty control messages.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete cross-platform ancillary parity for `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`), while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty ancillary control messages.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): continue `re` parser/matcher lowering into Rust intrinsics; literal/any/char-class advancement, char/range/category matching, anchor/backref/scoped-flag matcher nodes, group capture/value materialization, and replacement expansion are intrinsic-backed, while remaining lookaround variants, verbose parser edge cases, and full Unicode class/casefold parity are pending.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): continue full json parity work (JSONDecodeError formatting nuances, cls hooks, and additional runtime fast paths).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): finish `json` parity plan (performance tuning + full cls/callback parity) and add a runtime fast-path parser for dynamic strings.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): finish asyncio transport feature coverage after intrinsic capability gates (remaining native/wasm TLS edge parity and complete child-watcher behavior on supported hosts).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): fixture partial marker.\n",
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full gc module API + runtime cycle collector hook.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): lower SMTP client transport and protocol handling into Rust intrinsics and add STARTTLS/auth/LMTP parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): lower shelve persistence + dbm backends into Rust intrinsics and match CPython backend selection semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): contextmanager lowering). |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): implement `make_dataclass` once dynamic class construction is allowed.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `enum` parity (aliases, functional API, Flag/IntFlag edge cases).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `pickle` protocol 1+ and broader type coverage (bytes/bytearray, memo cycles).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `random` distributions + extended test vectors.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining `pathlib` parity gaps (walk(), owner()/group(), hardlink_to(), is_mount()/is_block_device()/is_char_device()/is_fifo()/is_socket(), lchmod(), glob edge cases); stat()/lstat()/touch()/rename()/replace()/chmod() now wired.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining pathlib glob edge parity (`root_dir`/hidden semantics, full Windows flavor/symlink nuances) and full Path parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining pickle CPython 3.12+ parity gaps before intrinsic-backed promotion.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P3, status:partial): remaining Decimal edge cases (NaN payload propagation, context-aware signal routing, __format__ spec).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete full statistics 3.12+ API/PEP parity beyond function surface lowering.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): deterministic clock policy) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `time` module surface (`timegm`) + deterministic clock policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand random distribution test vectors) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): finish Enum/Flag/IntFlag parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `json` parity plan (runtime fast-path + performance tuning + full cls/callback parity).
- Implemented(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `re` engine supports `\b`/`\B`/`\A`/`\Z` anchors, `(?:...)` non-capturing groups, lookahead/lookbehind; remaining gaps: backreferences, `(?P<name>...)` named groups, `re.VERBOSE` flag.
  TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `re` backreference + named group support.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P0, status:planned): full asyncio parity (tasks, task groups, streams, subprocess, executors) built on the runtime loop.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task API parity).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): extend intrinsic-backed `queue` support beyond `Queue`/`SimpleQueue` core semantics to full parity (`LifoQueue`, `PriorityQueue`, richer API/edge-case parity) and align dependent `logging.handlers` coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): implement full PEP 695 type params (bounds/constraints/defaults, ParamSpec/TypeVarTuple, alias metadata).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): move csv parser/writer hot paths to dedicated Rust intrinsics while preserving CPython parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (full encodings import hooks + charmap codec intrinsics); incremental encoder/decoder, BOM constants, register_error/lookup_error now wired to Rust.
- Implemented: `tempfile` now uses CPython-style candidate temp-dir ordering, including Windows defaults (`~\\AppData\\Local\\Temp`, `%SYSTEMROOT%\\Temp`, `c:\\temp`, `c:\\tmp`, `\\temp`, `\\tmp`) and cwd fallback.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): asyncio pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close parity gaps for `ast`, `ctypes`, `urllib.parse`, and `uuid` (see stdlib matrix).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close remaining socketserver class/lifecycle parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.client connection/chunked/proxy parity on top of intrinsic execute/response core.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.cookies quoting/attribute/parser parity beyond intrinsic-backed subset.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.server parser/handler lifecycle parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete queue edge-case/API parity (task accounting corners, comparator/error-path fidelity, and broader CPython coverage).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete queue edge-case/API parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete socket/select/selectors parity after intrinsic-backed object lowering (`poll`/`epoll`/`kqueue`/`devpoll` + backend selector classes); remaining work is OS-flag/error fidelity, fd inheritance corners, and wasm/browser host parity.
- Implemented: when `env.read` is denied, `tempfile` temp-dir selection no longer hard-fails and deterministically falls back to OS/cwd candidates.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand ctypes intrinsic coverage beyond the core scalar/structure/array/pointer subset.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand `asyncio` shim to full loop/task APIs (task groups, wait, shields) and I/O adapters.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): extend Rust ast lowering to additional stmt/expr variants and full argument shape parity; unsupported nodes currently raise RuntimeError immediately.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): fill out `types` shims (TracebackType, FrameType, FunctionType, coroutine/asyncgen types, etc).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish urllib.request handler/response/network parity on top of intrinsic opener core.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): full importlib native extension and pyc execution parity beyond capability-gated restricted-source shim lanes.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full _asyncio C-accelerated surface on top of runtime intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement `_asyncio` parity or runtime hooks.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement `_bz2` compression/decompression parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full metadata version semantics and remaining entry point selection edge cases.
- Implemented: `tempfile` temp-dir selection now probes candidate usability with secure create/write/unlink checks and raises `FileNotFoundError` when no candidate is writable.
- Implemented: `codecs` incremental encoder/decoder backed by Rust handle-based intrinsics; BOM constants from Rust; register_error/lookup_error wired. Remaining: full encodings import hooks + charmap codec intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib extension/sourceless execution parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery full native extension/pyc execution parity beyond restricted source shim lanes) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata full parsing + dependency/entry point semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.util non-source loader execution parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): tempfile parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + tests).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): asyncio submodule parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): import-only allowlisted stdlib modules (`argparse`, `ast`, `collections.abc`, `_collections_abc`, `_abc`, `_asyncio`, `_bz2`, `_weakref`, `_weakrefset`, `platform`, `time`, `tomllib`, `warnings`, `traceback`, `types`, `inspect`, `copy`, `copyreg`, `string`, `numbers`, `unicodedata`, `tempfile`, `ctypes`) to minimal parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): ast parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): cgi 3.12-path parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close `_abc` edge-case cache/version parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining abc edge-case parity around subclasshook/cache invalidation.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining non-UTF8 bytes/traversal-order edge parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining shlex parser/state parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining string parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining textwrap edge-case/module-surface parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.error/request integration parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.parse parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.response file-wrapper and integration edge parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): complete `fnmatch` bytes/normcase/cache parity on top of intrinsic-backed `molt_fnmatch*` runtime lane.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): complete `glob` parity (`root_dir`, `recursive`/`**` edge semantics, `include_hidden`) on top of intrinsic-backed `molt_glob`/`molt_glob_has_magic`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): complete `shlex` parser/state parity (`sourcehook`, `wordchars`, incremental stream semantics) on top of intrinsic-backed lexer/join lane.
- Note (doctest dynamic execution policy): doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand ctypes surface + data model parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement full gettext translation catalog/domain parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement gettext translation catalog/domain parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement tarfile parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement zipimporter bytecode/cache parity + broader archive support.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): importlib.machinery pending parity (package/module shaping + file reads + restricted-source execution lanes are intrinsic-lowered; remaining loader/finder parity is namespace/extension/zip behavior).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): inspect pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): pkgutil loader/zipimport/iter_importers parity (filesystem-only iter_modules/walk_packages today).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): test package pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): tighten `weakref.finalize` shutdown-order parity (including `atexit` edge cases) against CPython.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): traceback pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): types pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs for regrtest (support: captured_output/captured_stdout/captured_stderr, check_syntax_error, findfile, run_with_tz, warnings_helper utilities: check_warnings/check_no_warnings/check_no_resource_warning/check_syntax_warning/ignore_warnings/import_deprecated/save_restore_warnings_filters/WarningsRecorder, cpython_only, requires, swap_attr/swap_item, import_helper basics: import_module/import_fresh_module/make_legacy_pyc/ready_to_import/frozen_modules/multi_interp_extensions_check/DirsOnSysPath/isolated_modules/modules_setup/modules_cleanup, os_helper basics: temp_dir/temp_cwd/unlink/rmtree/rmdir/make_bad_fd/can_symlink/skip_unless_symlink + TESTFN constants); doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): warnings pending parity) |
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): argparse — 11 handle-based intrinsics wired (parser_new/add_argument/parse_args/format_help/format_usage/error/add_subparsers/add_parser/add_mutually_exclusive/group_add_argument/parser_drop). JSON monolith eliminated. |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): binascii pending parity) |
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): email — 26 intrinsics wired across 9 submodules (message handle API, utils parsedate/getaddresses/make_msgid/format_datetime, policy_new, headerregistry address format, header encode_word). |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): email remaining — _parseaddr Python RFC parser, charset mapping tables, feedparser stub, _header_value_parser stubs. |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): getopt pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): html.parser pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): ipaddress pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.config pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): logging.handlers pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): numbers pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): tomllib pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): unicodedata pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): xml pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): zlib pending parity) |
- TODO(stdlib-parity, owner:stdlib, milestone:SL1, priority:P1, status:planned): continue tightening math determinism policy coverage and platform notes.
- TODO(stdlib-parity, owner:stdlib, milestone:SL2, priority:P1, status:planned): complete native re parity and continue migrating parser/matcher execution into Rust (remaining lookaround variants, named-group edge cases, verbose-mode parser details, and full Unicode class/casefold semantics).
- TODO(stdlib-parity, owner:stdlib, milestone:SL2, priority:P1, status:planned): continue expanding socket parity (remaining option/error nuance, ancillary edge semantics, and broader platform-specific constant coverage).
- TODO(stdlib-parity, owner:stdlib, milestone:SL2, priority:P1, status:planned): parity backlog.\n",
- TODO(stdlib-parity, owner:stdlib, milestone:SL2, priority:P2, status:planned): continue broadening pathlib parity (glob recursion corner cases, Windows drive/anchor flavor nuances, and symlink edge semantics) while keeping path shaping in runtime intrinsics.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): "
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_aix_support` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_android_support` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_apple_support` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_ast_unparse` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_ast` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_blake2` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_colorize` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_compat_pickle` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_compression` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_contextvars` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_crypt` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_csv` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_ctypes` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_curses_panel` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_curses` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_datetime` now re-exports the intrinsic-backed `datetime` surface with the expected CPython public compatibility names (`UTC`, `datetime_CAPI`, core temporal types). |
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_dbm` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_decimal` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_elementtree` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_functools` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_gdbm` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_hashlib` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_heapq` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_hmac` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_imp` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_interpchannels` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_interpqueues` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_interpreters` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_io` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_ios_support` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_locale` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_lsprof` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_lzma` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_markupbase` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_md5` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_msi` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_osx_support` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_overlapped` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_posixshmem` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_py_warnings` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_pydatetime` now re-exports the intrinsic-backed `datetime` surface with CPython-compatible public names, including `sys`. |
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pydecimal` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyio` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pylong` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl._minimal_curses` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl._module_completer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl._threading_handler` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.base_eventqueue` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.commands` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.completing_reader` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.console` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.curses` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.fancy_termios` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.historical_reader` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.input` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.keymap` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.main` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.pager` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.reader` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.readline` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.simple_interact` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.terminfo` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.trace` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.types` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.unix_console` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.unix_eventqueue` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.utils` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.windows_console` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.windows_eventqueue` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_random` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_remote_debugging` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_scproxy` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sha1` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sha2` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sha3` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_signal` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sqlite3` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sre` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_ssl` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_stat` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_string` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_strptime` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_struct` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_suggestions` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_symtable` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sysconfig` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_thread` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_tokenize` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_tracemalloc` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_types` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_uuid` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_warnings` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_winapi` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_wmi` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_zoneinfo` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_zstd` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `aifc` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `annotationlib` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `antigravity` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio.tools re-exports graph introspection functions from asyncio; full parity pending deeper runtime integration.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio.windows_events provides ProactorEventLoop, IocpProactor, and policy re-exports; platform-gated for win32.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio.windows_utils provides PipeHandle/pipe/Popen wrappers; overlapped I/O semantics are simplified.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `audioop` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `cgi` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `cgitb` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `chunk` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `compression.zstd._zstdfile` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `compression.zstd` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `concurrent.futures.interpreter` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `concurrent.interpreters._crossinterp` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `concurrent.interpreters._queues` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `concurrent.interpreters` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `crypt` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `ctypes._layout` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `dbm.gnu` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `dbm.sqlite3` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `encodings._win_cp_codecs` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `ensurepip.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `ensurepip._uninstall` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `getopt` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.autocomplete_w` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.autocomplete` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.autoexpand` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.browser` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.calltip_w` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.calltip` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.codecontext` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.colorizer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.config_key` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.config` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.configdialog` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.debugger_r` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.debugger` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.debugobj_r` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.debugobj` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.delegator` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.dynoption` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.editor` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.filelist` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.format` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.grep` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.help_about` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.help` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.history` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.hyperparser` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.idle` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.iomenu` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.macosx` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.mainmenu` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.multicall` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.outwin` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.parenmatch` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.pathbrowser` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.percolator` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.pyparse` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.pyshell` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.query` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.redirector` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.replace` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.rpc` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.run` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.runscript` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.scrolledlist` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.search` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.searchbase` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.searchengine` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.sidebar` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.squeezer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.stackviewer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.statusbar` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.textview` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.tooltip` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.tree` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.undo` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.util` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.window` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.zoomheight` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib.zzdummy` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `idlelib` top-level stub with full intrinsic-backed lowering.
- Implemented: replaced `importlib.metadata.diagnose` stub with CPython-shaped diagnostic helpers (`inspect(path)` + `run()`) under intrinsic-first module policy.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.btm_matcher` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.btm_utils` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixer_base` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixer_util` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_apply` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_asserts` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_basestring` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_buffer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_dict` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_except` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_exec` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_execfile` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_exitfunc` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_filter` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_funcattrs` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_future` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_getcwdu` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_has_key` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_idioms` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_import` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_imports2` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_imports` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_input` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_intern` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_isinstance` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_itertools_imports` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_itertools` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_long` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_map` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_metaclass` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_methodattrs` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_ne` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_next` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_nonzero` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_numliterals` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_operator` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_paren` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_print` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_raise` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_raw_input` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_reduce` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_reload` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_renames` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_repr` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_set_literal` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_standarderror` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_sys_exc` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_throw` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_tuple_params` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_types` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_unicode` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_urllib` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_ws_comma` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_xrange` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_xreadlines` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes.fix_zip` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.fixes` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.main` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.patcomp` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.conv` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.driver` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.grammar` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.literals` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.parse` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.pgen` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.token` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2.tokenize` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pgen2` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pygram` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.pytree` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3.refactor` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `lib2to3` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `mailcap` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `mimetypes` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `msilib` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `msvcrt` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `nis` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `nntplib` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `nt` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `ntpath` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `nturl2path` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `numbers` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `ossaudiodev` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `pipes` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `pydoc_data.topics` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `site` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sndhdr` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `spwd` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sqlite3.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sqlite3.dbapi2` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sqlite3.dump` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sqlite3` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `string.templatelib` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sunau` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sysconfig.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `sysconfig` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `telnetlib` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tkinter.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tkinter.colorchooser` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): advance `tkinter.commondialog` from intrinsic-backed command wiring to full CPython parity.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tkinter.constants` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): advance `tkinter.dialog` from intrinsic-backed command wiring to full CPython parity.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tkinter.dnd` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): advance `tkinter.filedialog` from intrinsic-backed command wiring to full CPython parity.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tkinter.font` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): advance `tkinter.messagebox` from intrinsic-backed command wiring to full CPython parity.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tkinter.scrolledtext` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): advance `tkinter.simpledialog` from intrinsic-backed command wiring to full CPython parity.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tkinter.tix` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tomllib._parser` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tomllib._re` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tomllib._types` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtle` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.bytedesign` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.chaos` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.clock` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.colormixer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.forest` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.fractalcurves` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.lindenmayer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.minimal_hanoi` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.nim` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.paint` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.peace` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.penrose` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.planet_and_moon` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.rosette` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.round_dance` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.sorting_animate` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.tree` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.two_canvases` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo.yinyang` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `turtledemo` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest._log` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.async_case` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.case` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.loader` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.main` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.mock` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.result` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.runner` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.signals` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.suite` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `unittest.util` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `urllib.robotparser` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `uu` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `venv.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `winreg` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `winsound` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `wsgiref.handlers` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `wsgiref.types` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `wsgiref.validate` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xdrlib` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom.NodeFilter` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom.domreg` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom.expatbuilder` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom.minicompat` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom.minidom` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom.pulldom` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom.xmlbuilder` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.etree.ElementInclude` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.etree.ElementPath` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.etree.ElementTree` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.etree.cElementTree` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.etree` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.parsers.expat` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.parsers` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.sax._exceptions` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.sax.expatreader` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.sax.handler` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.sax.saxutils` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.sax.xmlreader` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.sax` package stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml` top-level stub with full intrinsic-backed lowering.
- Implemented: replaced `zipfile.__main__` stub with `python -m zipfile` entrypoint wiring to `zipfile.main()` (create/list/test/extract paths now execute through Molt’s intrinsic-first zipfile implementation).
- Implemented: replaced `zipfile._path.glob` stub with version-gated CPython-style glob translation helpers (`translate` lane on 3.12; `Translator` lane on 3.13+).
- Implemented: replaced `zipfile._path` package stub with CPython-shaped `Path`/directory lookup behavior for Molt zip archives (no host-Python fallback lane).
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `zoneinfo._common` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `zoneinfo._tzpath` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `zoneinfo._zoneinfo` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P2, status:planned): implement bz2 compression/decompression parity or runtime-backed hooks.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P3, status:planned): continue signature/introspection parity expansion.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P3, status:planned): continue unittest runner/result/decorator parity expansion.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P3, status:planned): extend import_helper coverage (extension loader helpers, importlib.machinery parity, and script helper utilities beyond ready_to_import).
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P3, status:planned): expand os_helper coverage for file, path, and process helpers used by CPython tests.
- Note (doctest dynamic execution policy): doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(syntax, owner:frontend, milestone:LF1, priority:P1, status:partial): `with` lowering for async/multi-context managers + try/finally lowering in IR.
- TODO(syntax, owner:frontend, milestone:LF2, priority:P2, status:planned): class lowering for `__init__` and factory classmethods (dataclass defaults now wired in stdlib).
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:missing): full `with`/contextlib lowering with exception flow.
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): f-string format specifiers and debug spec (`f"{x:.2f}"`, `f"{x=}"`) parity (see [docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md](docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md)).
- Implemented: `match`/`case` lowering via cell-based PEP 634 desugaring (24 differential test files).
- Implemented: structural pattern matching with all PEP 634 pattern types (literal, variable, sequence, mapping, class, or, as, star, singleton, guard).
- TODO(syntax, owner:frontend, milestone:M3, priority:P3, status:missing): type alias statement (`type X = ...`) and generic class syntax (`class C[T]: ...`) coverage (see [docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md](docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md)).
- TODO(tests, owner:frontend, milestone:TC2, priority:P2, status:planned): KW_NAMES error-path coverage (duplicate keywords, positional-only violations) in differential tests.
- TODO(tests, owner:runtime, milestone:SL1, priority:P1, status:partial): expand native+wasm codec parity coverage for binary/floats/large ints/tagged values + deeper container shapes.
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): add security-focused differential tests for attribute access edge cases (descriptor exceptions, `__getattr__` recursion traps).
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): expand exception differential coverage.
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): security-focused attribute access tests (descriptor exceptions, `__getattr__` recursion traps).
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): add wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
- TODO(tooling, owner:release, milestone:TL2, priority:P2, status:partial): enforce signature verification/trust policy during load.)
- TODO(tooling, owner:release, milestone:TL2, priority:P2, status:planned): formalize release tagging (start at `v0.0.001`, increment thousandth) and require super-bench stats for README performance summaries.
- TODO(tooling, owner:runtime, milestone:TL2, priority:P1, status:partial): remove the temporary dual `fallible-iterator` graph when postgres ecosystem crates support `0.3+`; until then, keep 0.2 usage isolated to postgres-boundary code paths and document the constraint in status/review notes.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P1, status:partial): implement `molt extension build` with `libmolt` headers + ABI tagging (cross-target target-triple wiring + CI native/cross dry-run lanes are landed; broader linker/sysroot hardening pending).
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:partial): implement `molt extension audit` and wire into `molt verify` (audit CLI + verify integration landed; richer policy diagnostics pending).
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): define canonical wheel tags for `libmolt` extensions.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): extension rebuild pipeline (headers, build helpers, audit tooling) for `libmolt`-compiled wheels.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add process-level parallel frontend module-lowering and deterministic merge ordering, then extend to large-function optimization workers where dependency-safe (dependency-layer process-pool lowering is landed behind `MOLT_FRONTEND_PARALLEL_MODULES`; remaining work is broader eligibility + worker telemetry/perf tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): harden backend daemon lane (multi-job compile API, bounded request/job guardrails, richer health telemetry, deterministic readiness/restart semantics, and config-digest lane separation with cache reset-on-change are landed; remaining work is sustained high-contention soak evidence + restart/backoff tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): surface active optimization profile/tier policy and degrade events in CLI build diagnostics and JSON outputs for deterministic triage (diagnostics sink is landed for policy/tier/degrade + pass hotspots, and stderr verbosity partitioning is landed; remaining work is richer CLI UX controls beyond verbosity).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add batch compile server mode for diff runs to amortize backend startup and reduce per-test compile overhead (in-process JSON-line batch server, hard request deadlines, force-close shutdown, cooldown-based retry hardening, and fail-open/strict modes are landed behind env gates; remaining work is default-on rollout criteria + perf guard thresholds).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add function-level object caching so unchanged functions can be relinked without recompiling whole scripts (function cache-key lane now includes backend codegen-env digest + IR top-level extras digest, module/function cache-tier fallback + daemon function-cache plumbing are landed, and invalid cached-artifact guard + daemon cache-tier telemetry are wired; remaining work is import-graph-aware scheduling + fleet-level perf tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:partial): cross-target ergonomics).
- Implemented: `molt doctor` now surfaces optimization-path diagnostics beyond basic toolchain presence (`sccache`, backend daemon enablement, cargo/cache path routing, and external-volume routing hints).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): CI perf artifacts + release uploads)
- Implemented: `molt parity-run` now runs file/module entrypoints with CPython only (no Molt compilation), supports `--python`/`--python-version`, forwards script args, and exposes optional `--timing`/`--json` reporting for parity workflows.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add distributed cache guidance/tooling for multi-host agent fleets (remote `sccache` backend and validation playbooks).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add import-graph-aware diff scheduling to maximize cache locality and reduce redundant rebuild pressure.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): broaden deopt taxonomy + profile-consumption loop).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): lockfile-missing policy decision).
- Implemented: when `MOLT_HOME` is unset, CLI defaults now place `MOLT_HOME` under `MOLT_CACHE/home`, removing default reliance on the legacy `~/.molt` artifact/cleanup path.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): runtime profiling hints in TFA).
- TODO(type-coverage, owner:compiler, milestone:TC2, priority:P2, status:planned): generator/iterator state in wasm ABI.
- TODO(type-coverage, owner:compiler, milestone:TC2, priority:P2, status:planned): wasm ABI for generator state. |
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): `try/except/finally` lowering + raise paths.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): builtin constructors for `tuple`, `dict`, `bytes`, `bytearray`.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): builtin reductions (`sum/min/max`) and `len` parity.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): type-hint specialization policy (`--type-hints=check` with runtime guards).
- Implemented(type-coverage, owner:runtime, milestone:TC2, priority:P1, status:implemented): complex type via canonical GC object model (ops.rs/numbers.rs/attributes.rs); orphaned handle-based complex_core.rs deleted.
  TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): specialized complex IR ops (COMPLEX_ADD, COMPLEX_REAL) for performance; slow-path dispatch works correctly.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:partial): `int()` keyword arguments (`x`, `base`) parity.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:missing): async comprehensions (async for/await in comprehensions).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:missing): lower classes defining `__next__` without `__iter__` without backend panics.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:partial): builtin conversions (`str`, `bool`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:partial): comprehension lowering currently routes through iterator/generator paths with a narrow `LIST_FROM_RANGE` fast path; broaden lowering coverage while preserving CPython semantics.
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): async iteration builtins (`aiter`, `anext`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): builtin conversions (`int`, `float`, `complex`, `str`, `bool`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): builtin iterators (`iter`, `next`, `reversed`, `enumerate`, `zip`, `map`, `filter`).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:planned): builtin numeric ops (`abs`, `round`, `pow`, `divmod`, `min`, `max`, `sum`).
- TODO(type-coverage, owner:frontend, milestone:TC3, priority:P2, status:missing): full import/module fallback classification.
- TODO(type-coverage, owner:runtime, milestone:LF2, priority:P2, status:planned): `type`/`object` layout, `isinstance`/`issubclass`.
- TODO(type-coverage, owner:runtime, milestone:LF2, priority:P2, status:planned): descriptor builtins (`property`, `classmethod`, `staticmethod`, `super`).
- TODO(type-coverage, owner:runtime, milestone:LF2, priority:P2, status:planned): type/object + MRO + descriptor protocol. |
- TODO(type-coverage, owner:runtime, milestone:TC1, priority:P1, status:partial): exception object model + raise/try. |
- TODO(type-coverage, owner:runtime, milestone:TC1, priority:P1, status:partial): exception objects + stack trace capture.
- TODO(type-coverage, owner:runtime, milestone:TC1, priority:P1, status:partial): recursion limits + `RecursionError` guard semantics.
- TODO(type-coverage, owner:runtime, milestone:TC1, priority:P2, status:partial): expand `bytes`/`bytearray` encoding coverage (additional codecs + full error handlers).
- TODO(type-coverage, owner:runtime, milestone:TC1, priority:P2, status:partial): typed exception matching beyond kind-name classes.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): bytes semantics beyond literals).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): matmul dunder hooks (`__matmul__`/`__rmatmul__`) with buffer2d fast path.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): rounding intrinsics (`floor`, `ceil`) + full deterministic semantics for edge cases.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): formatting builtins (`repr`, `ascii`, `bin`, `hex`, `oct`, `chr`, `ord`) + full `format` protocol (named fields, format specs, conversion flags).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): generator state objects + StopIteration.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): identity builtins (`hash`, `id`, `callable`).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): rounding intrinsics (`round`, `floor`, `ceil`, `trunc`) with deterministic semantics.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): set/frozenset hashing + deterministic ordering.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): memoryview multi-dimensional slicing + sub-views (C-order parity).
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): memoryview multi-dimensional slicing + sub-views (retain C-order semantics + parity errors).
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): metaclass execution). |
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): derive `types.GenericAlias.__parameters__` from `TypeVar`/`ParamSpec`/`TypeVarTuple` once typing metadata lands.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): buffer protocol + memoryview layout.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): descriptor builtins (`property`, `classmethod`, `staticmethod`, `super`).
- TODO(type-coverage, owner:stdlib, milestone:TC2, priority:P2, status:planned): `builtins` module parity notes.
- TODO(type-coverage, owner:stdlib, milestone:TC2, priority:P3, status:planned): `builtins` module parity notes.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:partial): dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Note (dynamic execution policy): dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Note (reflection policy): unrestricted reflection (`dir`/`vars`/`globals`/`locals`) is policy-deferred for compiled binaries; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Note (runtime monkeypatch policy): runtime monkeypatching of modules, types, or functions is policy-deferred for compiled binaries; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): module object + import rules. |
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): reflection builtins (`type`, `isinstance`, `issubclass`, `getattr`, `setattr`, `hasattr`, `dir`, `vars`, `globals`, `locals`).
- TODO(type-coverage, owner:tests, milestone:TC1, priority:P1, status:planned): add exception + set coverage to molt_diff.
- TODO(type-coverage, owner:tests, milestone:TC2, priority:P2, status:partial): execute matrix end-to-end).
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity with real backends + coverage).
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity).
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P2, status:planned): ship additional production host adapters (CF Workers) and wasm parity tests that exercise real DB backends with cancellation.
- TODO(wasm-host, owner:runtime, milestone:RT3, priority:P3, status:planned): component model target support).
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): expand browser socket coverage (UDP/listen/server sockets) + parity tests.)
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): capability-enabled runtime-heavy wasm tranche (`/Volumes/APDataStore/Molt/wasm_runtime_heavy_tranche_20260213c/summary.json`) is still blocked (`1/5` pass): `asyncio__asyncio_running_loop_intrinsic.py` event-loop-policy parity mismatch, `asyncio_task_basic.py` table-ref trap in linked wasm runtime, `zipimport_basic.py` zipimport module-lookup parity gap, and `smtplib_basic.py` thread-unavailable wasm limitation. Keep this as a blocker before promoting runtime-heavy cluster completion.
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale on wasm hosts). (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): deterministic clock policy) |
- TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P1, status:planned): wasm host parity for the asyncio runtime loop, poller, sockets, and subprocess I/O.
- TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P2, status:planned): zero-copy string passing for WASM).
<!-- END TODO MIRROR LEDGER -->
