# Molt Roadmap (Active)

Canonical current status: [docs/spec/STATUS.md](docs/spec/STATUS.md). This roadmap is forward-looking.

## Planning Doc Hierarchy
- Current state and capabilities: [docs/spec/STATUS.md](docs/spec/STATUS.md) (canonical source of truth).
- Active project plan and backlog: [ROADMAP.md](ROADMAP.md) (this file).
- Near-term sequencing and execution windows: [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md).
- Optimization strategy and track scope: [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md).
- Optimization execution history and artifacts: [docs/benchmarks/optimization_progress.md](docs/benchmarks/optimization_progress.md).
- Month 1 enforcement artifacts:
  - [docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
  - [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md)
- Historical/detail roadmap archive: [docs/ROADMAP.md](docs/ROADMAP.md) (retained for context; do not treat as canonical state).

## Legend
- **Status:** Implemented (done), Partial (some semantics missing), Planned (scoped but not started), Missing (no implementation), Divergent (intentional difference from CPython).
- **Priority:** P0 (blocker), P1 (high), P2 (medium), P3 (lower).
- **Tier/Milestone:** `TC*` (type coverage), `SL*` (stdlib), `DB*` (database), `DF*` (dataframe/pandas), `LF*` (language features), `RT*` (runtime), `TL*` (tooling), `M*` (syntax milestones), `M-GPU-*` (GPU acceleration).

## Strategic North-Star
- Performance target: parity with or superiority to Codon on tracked benches.
- Compatibility target: near-Nuitka CPython coverage + interoperability for
  Molt-supported semantics, without violating Molt break-policy constraints.

## Optimization Program Kickoff (2026-02-11)
- Week 1 observability is complete and Week 0 baseline-lock artifacts are captured.
- Canonical optimization scope: [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md).
- Canonical execution log and milestone history: [docs/benchmarks/optimization_progress.md](docs/benchmarks/optimization_progress.md).
- Current Week 1 evidence artifact: [bench/results/optimization_progress/2026-02-11_week1_observability/summary.md](bench/results/optimization_progress/2026-02-11_week1_observability/summary.md).
- Week 0 baseline lock summary: [bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md](bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md).
- Guard/deopt feedback artifact wiring is available: runtime emits
  `molt_runtime_feedback.json` when `MOLT_RUNTIME_FEEDBACK=1`
  (override path with `MOLT_RUNTIME_FEEDBACK_FILE`), and schema validation is
  enforced via `tools/check_runtime_feedback.py` including required
  `deopt_reasons.call_indirect_noncallable` and
  `deopt_reasons.invoke_ffi_bridge_capability_denied`, plus
  `deopt_reasons.guard_tag_type_mismatch` and
  `deopt_reasons.guard_dict_shape_layout_mismatch` with guard-layout
  mismatch breakdown counters (`*_null_obj`, `*_non_object`,
  `*_class_mismatch`, `*_non_type_class`,
  `*_expected_version_invalid`, `*_version_mismatch`).
- Week 2 readiness note: baseline gate is satisfied; prioritize specialization + wasm-stabilization clusters from the lock summary failure lists.
- Metric-mode alignment note: optimization/lowering scoreboard semantics are canonicalized to `docs/spec/STATUS.md` checker snapshots; avoid stale historical module-count snapshots when triaging active gates.

## Optimization Swarm Execution Protocol (2026-02-24)
- Optimization work is orchestrated via a control-plane-first swarm:
  - conductor (sequencing + stop/go),
  - integrator (single merge authority),
  - perf sheriff (benchmark/throughput gates),
  - correctness sheriff (diff/parity/memory gates),
  - docs sheriff (same-change status sync),
  - lane workers (non-overlapping implementation ownership).
- Execution waves:
  1. Wave 0: refresh baselines and align docs/metric modes.
  2. Wave 1: hypothesis + measurement packets only.
  3. Wave 2: controlled implementation slices with kill switches.
- Merge policy:
  - no optimization landing without artifacted perf diff + correctness + lowering gates.
  - keep `STATUS.md`, `ROADMAP.md`, `OPTIMIZATIONS_PLAN.md`, and `docs/benchmarks/optimization_progress.md` synchronized when optimization status changes.
- 2026-02-25 Wave 0 release-lane triage:
  - fixed release compile regression in `runtime/molt-runtime/src/async_rt/scheduler.rs` where `_asyncio` task-entry/exit paths used invalid `raise_exception` call shapes.
  - targeted correctness gates are green (`cargo check -p molt-runtime`; `tests/test_tkinter_phase0_wrappers.py` -> `2 passed`).
  - Wave 0 benchmark harness interpreter mismatch (`packaging.markers` missing in uv-only lane) is resolved: `tools/bench.py` and `tools/bench_wasm.py` now invoke Molt builds via `uv run --python 3.12 python3` instead of bare `sys.executable`, matching the pattern already used in `tools/compile_progress.py`. Remaining Wave 0 blocker is release-runtime compile churn; canonical tracking is in `docs/benchmarks/optimization_progress.md`.

## Stdlib Intrinsics Program (2026-03-01)
- Canonical plan: [docs/spec/areas/compat/plans/stdlib_lowering_plan.md](docs/spec/areas/compat/plans/stdlib_lowering_plan.md).

### Rust-First Stdlib Lowering Sprint (2026-03-01)
- Completed: **Cranelift Codegen Optimization** — full Cranelift 0.128 feature exploitation: cold block marking
  (36+ slow paths + exception handlers), MemFlags::trusted() (34 load/store sites), alias analysis, CFG metadata,
  colocated libcalls, CPU feature auto-detection (AVX2/NEON/CRC), inline stack probing, Spectre mitigations off,
  frame pointer omission in release.
- Completed: **SIMD Phase 5–7** — title case (NEON/SSE2), bytes.hex() SIMD, base64_mod SIMD parity
  (b16_encode, b64_encode unrolled, b64_decode filter), qp_encode/decode SIMD, hardware CRC32
  (aarch64 __crc32d, x86 SSE4.2), optimized Adler-32 (chunked NMAX + 16× unrolled), JSON
  scanstring_decode SIMD, JSON ensure_ascii SIMD, memchr2-based splitlines, SIMD whitespace
  split helpers, array byte search (memchr), HTML tokenizer memchr2 scanning.
- Completed: **SIMD Phase 3+4** — string/bytes predicate SIMD (isdigit, isalpha, isalnum, islower, isupper,
  isprintable), str.swapcase/capitalize ASCII fast paths, ascii() SIMD scan, JSON SIMD safe-char scan.
- Completed: **SIMD Phase 2** — hex encode/decode (NEON/SSE2 lookup-table), base64 whitespace stripping,
  single-byte replace (AVX2/NEON/SSE2), ASCII isspace SIMD, NEON whitespace-split, strip fast-skip.
- Completed: **SIMD Expansion** — 20+ runtime operations with SSE2/AVX2/NEON fast paths.
  String/bytes equality, lexicographic comparison, sequence comparison, float vector sum,
  ASCII case conversion, ASCII predicates, hash computation, str.lower/upper, math.dist,
  math.hypot. +1,133 lines. `.cargo/config.toml` target-cpu=native for Apple Silicon/x86.
- Completed: `re` Phase 1 Rust parser — 2,586-line recursive-descent parser in regex.rs.
  CompiledPattern + ReNode enum + global handle registry. 4 new intrinsics.
- Completed: `re` Phase 1b backtracking NFA match engine — 1,100-line continuation-passing
  engine. Supports all ReNode variants. `molt_re_execute` and `molt_re_finditer_collect`
  now fully implemented. 60/60 tests pass. Fixed strip_verbose VERBOSE flag check.
- Completed: libmolt C-API Phase 1+2 — 117 CPython C-API functions, 80 tests passing.
  Phase 1: PyList/Dict/Tuple/Iter/type check. Phase 2: Object Protocol (Repr/Str/Hash/
  IsTrue/Not/Type/Length/GetAttr/SetAttr/DelAttr/HasAttr/RichCompare/IsInstance/IsSubclass/
  CallableCheck), Number/Mapping/Sequence/Set/Bytes/String/Unicode/Exception/RefCount/
  Conversion/Memory protocols. c_api.rs: 6,561 lines.
- Completed: `stringprep` module (RFC 3454) — 719-line Rust module, 17 table membership
  intrinsics, map_table_b3 case folding. 13 unit tests. Intrinsic-backed Python wrapper.
- Fixed: async generator StopAsyncIteration → RuntimeError (PEP 479 analog).
  Async generators are fully implemented at all layers (frontend/backend/runtime).
- Completed: asyncio Barrier rewrite + Semaphore CM + Server methods + Queue enhancements
  + BrokenBarrierError export + __repr__ on all sync primitives. 24 parity gaps closed.
- Completed: tkinter Toplevel+Wm mixin (P0) + _splitdict fix + Entry.bbox/validate +
  _root() + grid_children/place_children + Font.__del__ + ttk additions. 16 parity gaps
  closed.
- Completed: asyncio staggered_race, tkinter WASM import gate, 26 dead intrinsics deleted,
  5 singletons wired, tkinter/asyncio bug fixes.
- Completed: CPU kernelization loop classifier design (5-phase plan, metadata-only initially).
- Completed: GPU/MLIR groundwork research (melior 0.26 + cudarc 0.17 + 5-stage plan).
- Intrinsics audit: 2,193 total, 1,838 Python-wired, 355 Rust-internal, zero unwired.

### Rust-First Stdlib Lowering Sprint (2026-02-28)
- Completed: `base64` — all 18 functions rewired to existing Rust intrinsics.
  Removed ~400 lines of pure-Python encode/decode loops.
- Completed: `random` — new `random_mod.rs` (1457 lines) with full MT engine +
  21 intrinsics. Python `Random` class is now a thin handle wrapper.
- Completed: `heapq` — 5 new Rust intrinsics (`heapify_max`, `heappop_max`,
  `nsmallest`, `nlargest`, `merge`) with proper heap algorithms.
- Completed: `copy` — wired to `molt_copy_copy`/`molt_copy_deepcopy` intrinsics.
  Removed ~350 lines of Python dispatch/traversal.
- Completed: `pprint` — wired to `molt_pprint_pformat`/`molt_pprint_safe_repr`/etc.
- Completed: `uuid` byte-loop cleanup, `json` dead code removal.
- Completed: `zlib` — all 27 intrinsics wired (compress/decompress/crc32/adler32 +
  Compress/Decompress handle classes + 14 constants).
- Completed: `ipaddress` — 30 intrinsics wired (IPv4/IPv6 address + network handle
  classes; eliminated pure-Python parse/compress implementations).
- Completed: `shutil` — 9 additional intrinsics wired (copy/copy2/copytree/move/etc.).
- Completed: `subprocess` — 3 convenience intrinsics wired (run/check_call/check_output).
- Completed: `enum` — all 10 Flag/auto/StrEnum/unique/verify intrinsics wired.
  Added StrEnum class, @unique/@verify decorators, Flag iteration via flag_decompose,
  FlagBoundary sentinels.
- Completed: `warnings` — 8 intrinsics wired. Dead inline regex engine eliminated.
  Fast-path to Rust for common warn/warn_explicit calls.
- Completed: `logging` — wired 31 Rust intrinsics (handle-based LogRecord/Formatter/
  Handler/StreamHandler/Logger). basicConfig, shutdown, level name mapping all delegate
  to Rust. Python class interfaces preserved for subclassing.
- Completed: `string` Template/Formatter — 5 new Rust intrinsics (template_scan,
  template_is_valid, template_get_identifiers, formatter_parse, field_name_split).
  Eliminated ~290 lines Python parsing. Formatter._vformat stays as acceptable Python.
- Blocker: `encodings` punycode/idna/uu_codec need new Rust implementations (~545
  lines Python total).

### Compiler + WASM + Stdlib Hardening Sprint (2026-02-28)
- Completed: guard_tag_for_hint extended (set/frozenset/intarray type tags).
- Completed: 6 WASM silent-divergence fixes (os.getppid ENOSYS, HTTP Date UTC,
  datetime.now OSError, select.select break-not-spin, thread.ident=1, utcoffset OSError).
- Completed: orphaned complex_core.rs deleted (26 dead intrinsics).
- Completed: `re` anchors (\b, \B, \A, \Z) implemented in NFA engine.
- Completed: `collections.ChainMap` (11 intrinsics), `io.SEEK_*` constants,
  `os.DirEntry.stat()/inode()`, `datetime` timedelta arithmetic + combine/fromisocalendar,
  `typing` 7 new APIs, `pathlib.Path.walk()`, `functools.cached_property`.

### Asyncio & Tkinter Parity Sprint (2026-02-28)
- Completed: asyncio pipe transports (`connect_read_pipe`/`connect_write_pipe`)
  with 11 new pipe transport Rust intrinsics.
- Completed: 42 new Rust intrinsics for asyncio Future/Event/Lock/Semaphore/Queue
  state machines; all 97 bare `except` blocks eliminated from asyncio shim.
- Completed: WASM capability gating for 6 asyncio I/O operations.
- Completed: Transport/Protocol base classes added to asyncio surface.
- Completed: asyncio 3.13 version-gated APIs (`as_completed` async iter,
  `Queue.shutdown`) and 3.14 version-gated APIs (`get_event_loop` `RuntimeError`,
  child watcher removal, policy deprecation).
- Completed: tkinter 10 Rust intrinsics wired (event parsing, Tcl list/dict
  conversion, hex color validation, option normalization).
- Completed: all tkinter strict mode violations resolved.
- Completed: tkinter 3.13 (`tk_busy_*`, `PhotoImage.copy_replace`) and 3.14
  (`trace_variable` deprecation) version-specific APIs added.
- Completed: tkinter 100% submodule coverage achieved.

### Stdlib Intrinsics Sprint (2026-02-25)
- Completed: major 5-track sprint adding ~85 new Rust intrinsics, ~1,250 LOC
  Rust runtime, ~1,600 LOC Python shim rewrites.
- Track A (os): wired ~25 existing intrinsics (`access`, `chdir`, `cpu_count`,
  `link`, `truncate`, `umask`, `uname`, `getppid`, `getuid`/`getgid`/`geteuid`/`getegid`,
  `getlogin`, `getloadavg`, `removedirs`, `devnull`, `get_terminal_size`,
  `walk`, `scandir`, `path.commonpath`/`commonprefix`,
  `path.getatime`/`getctime`/`getmtime`/`getsize`, `path.samefile`,
  `F_OK`/`R_OK`/`W_OK`/`X_OK`) and added ~15 new Rust intrinsics (`dup2`,
  `lseek`, `ftruncate`, `isatty`, `fdopen`, `sendfile`, `kill`, `waitpid`,
  `getpgrp`/`setpgrp`/`setsid`, `sysconf`/`sysconf_names`, `path.realpath`,
  `utime`). Total `os` module now has ~40 intrinsic-backed APIs.
- Track B (sys): added ~20 new intrinsics (`maxsize`, `maxunicode`,
  `byteorder`, `prefix`, `exec_prefix`, `base_prefix`, `base_exec_prefix`,
  `platlibdir`, `float_info`, `int_info`, `hash_info`, `thread_info`,
  `intern`, `getsizeof`, `stdlib_module_names`, `builtin_module_names`,
  `orig_argv`, `copyright`, `displayhook`, `excepthook`).
- Track C (_thread + signal): rewrote `_thread.py` with full intrinsic-backed
  surface (`allocate_lock`, `LockType`, `start_new_thread`, `exit`,
  `get_ident`, `get_native_id`, `_count`, `stack_size`, `interrupt_main`,
  `TIMEOUT_MAX`, `error`); extended `signal` with 12 new constant intrinsics
  (`SIGBUS` through `SIGSYS`) and 5 POSIX function intrinsics (`strsignal`,
  `pthread_sigmask`, `pthread_kill`, `sigpending`, `sigwait`).
- Track D (asyncio): expanded `_asyncio.py` with C-accelerated surface
  functions (`current_task`, `_enter_task`, `_leave_task`, `_register_task`,
  `_unregister_task`) backed by 4 new Rust intrinsics with runtime task-state
  management.
- Track E (subprocess): added `start_new_session`, `process_group` params,
  `pid` property, `send_signal` method, `check_call`, `getstatusoutput`,
  `getoutput` with new `molt_process_spawn_ex` Rust intrinsic;
  `concurrent.futures` verified intrinsic-complete with no changes needed.
- Tkinter cross-platform execution plan:
  [docs/spec/areas/compat/plans/tkinter_lowering_plan.md](docs/spec/areas/compat/plans/tkinter_lowering_plan.md).
- Implemented: Tkinter runtime now ships dual-path `molt_tk_*` lowering:
  deterministic intrinsic-backed headless behavior by default (core Tk command
  semantics plus broad `tkinter.ttk` command-family lowering) and an opt-in
  native Tcl/Tk backend (`cargo` feature `molt_tk_native`) that performs
  interpreter app creation (`useTk` aware), Tcl command dispatch, callback
  wiring, `after`/event-loop pumping, and Tk lifecycle operations.
- Native-unavailable behavior remains explicitly capability-gated and
  deterministic (`RuntimeError` on native hosts, `NotImplementedError` on wasm),
  with no host-Python fallback.
- Implemented: `_tkinter` stdlib shim now routes an expanded intrinsic-backed API through
  `molt_tk_*` intrinsics (Tkapp shell, call/event helpers, var helpers,
  conversion helpers, and config helpers) and retains deterministic unsupported
  behavior.
- Implemented (2026-02-28): 10 Rust intrinsics wired for tkinter (event
  parsing, Tcl list/dict conversion, hex color validation, option
  normalization); all strict mode violations resolved; 3.13 (`tk_busy_*`,
  `PhotoImage.copy_replace`) and 3.14 (`trace_variable` deprecation)
  version-gated APIs added; 100% submodule coverage achieved.
- Implemented: headless Rust Tk command lowering now covers major
  `tkinter.ttk` execution families (Treeview, `ttk::style`,
  notebook/panedwindow/container/widget subcommands, and
  `ttk::notebook::enableTraversal`) without Python-side behavior fallback.
- Differential regression coverage now includes
  `tests/differential/stdlib/tkinter_phase0_core_semantics.py` to validate
  `_tkinter`/`tkinter` import + missing-attribute error-shape contracts,
  `_tkinter` intrinsic-backed core API presence (`create`, Tkapp helpers, conversion and
  var helpers, and exported constants/types), and tkinter wrapper submodule
  import/error-shape/capability-gate contracts (`tkinter.__main__`,
  dialog/helper wrappers, and `tkinter.ttk`) without requiring a real GUI backend,
  including runtime-lowered core + `ttk` semantics checks
  (`tkinter:runtime_core_semantics`, `tkinter.ttk:runtime_semantics`).
- Hard gate contract in `tools/check_stdlib_intrinsics.py` now includes:
  - zero `probe-only`,
  - zero `python-only`,
  - intrinsic-partial ratchet budget (`tools/stdlib_intrinsics_ratchet.json`),
  - fallback anti-pattern blocking for `_py_*` direct/dynamic imports (including alias and keyword-argument dynamic forms).
- Blocker-first tranche update:
  - landed importlib blocker/resolver hardening (`importlib.machinery` + `importlib.util`)
    with regression tests and targeted differential evidence.
  - `concurrent.futures` currently intrinsic-backed; `pickle` remains intrinsic-partial.
  - wasm-linked build blocker fixed in `tools/wasm_link.py`: malformed UTF-8
    function-name entries in optional `name` sections no longer hard-fail table-ref append.
  - wasm runner hardening landed: deterministic Node resolver (`MOLT_NODE_BIN`
    + auto-select Node >= 18) and explicit `run_wasm.js` WASI fallback
    (`node:wasi` -> `wasi`) with actionable error text.
  - wasm socket constants payload now exports required CPython-facing names
    (`AF_INET`, `SOCK_STREAM`, `SOL_SOCKET`, etc.) from runtime intrinsic
    `molt_socket_constants`.
  - linked-wasm asyncio table-ref trap is closed: poll dispatch now uses
    runtime table-base addressing + legacy-slot normalization, linked artifacts
    export `molt_set_wasm_table_base`, and scheduler execution no longer
    recursively acquires `task_queue_lock`.
  - linked-wasm metadata import regression is closed: wasm backend now
    registers `sys_hexversion`, `sys_api_version`, `sys_abiflags`, and
    `sys_implementation_payload` imports used by builtin wrapper/table wiring,
    removing the `missing builtin import for sys_hexversion` panic class in
    targeted linked bench runs.
  - runtime-heavy wasm regression lane is green for this blocker tranche:
    `tests/test_wasm_runtime_heavy_regressions.py` now passes on
    asyncio/zipimport/smtplib targeted cases.
  - native runtime-heavy cluster sweep is green (`119/119` pass) for
    `_asyncio`/`smtplib`/`zipfile`/`zipimport` with `MOLT_DIFF_MEASURE_RSS=1`
    and memory caps enabled.
  - strict closure sweep for `re`/`pathlib`/`socket` is green (`102/102` pass)
    with `MOLT_DIFF_MEASURE_RSS=1` and memory caps enabled.
  - targeted compression differential smoke is green (`4/4` pass for
    `bz2_basic`/`gzip_basic`/`lzma_basic`/`zlib_basic`) with
    `MOLT_DIFF_MEASURE_RSS=1`, external-volume artifact roots, and per-process
    memory caps.
  - pickle closure tranche advanced for default class/dataclass graph semantics:
    runtime now serializes class layout field state (`__molt_field_offsets__`)
    and restores CPython-style `BUILD` ordering (`__dict__` + slot-state),
    while preserving reducer/copyreg precedence before fallback instance
    lowering.
  - new pickle parity regressions are green in native + wasm lanes:
    `tests/differential/stdlib/pickle_class_dataclass_roundtrip.py` and
    `tests/test_wasm_pickle_class_dataclass_roundtrip.py`.
  - checker strict-root coverage now includes `re` in
    `CRITICAL_STRICT_IMPORT_ROOTS` with regression test coverage
    (`tests/test_check_stdlib_intrinsics.py`).
  - runtime-heavy/data/metadata-email/tooling clusters remain intrinsic-partial and are the active burn-down queue.
- Current snapshot: `intrinsic-backed=0`, `intrinsic-partial=873`,
  `probe-only=0`, `python-only=0`; strict gate keeps modules/submodules
  intrinsic-partial until full CPython 3.12+ parity/TODO burn-down is complete.
- Current wasm blockers before runtime-heavy promotion:
  - thread-dependent stdlib server paths remain capability/host blocked on wasm
    by design (`NotImplementedError: threads are unavailable in wasm`), so
    full server parity for these lanes still requires an explicit wasm threading
    strategy.
  - Node/V8 Zone OOM remains reproducible on some linked runtime-heavy modules
    (`zipfile`/`zipimport` family) even with single-task wasm compilation.
- Weekly scoreboard (required): track
  `intrinsic-backed`, `intrinsic-partial`, `probe-only`, `python-only`,
  missing required top-level/submodule entries, native pass %, wasm pass %, and
  memory regressions.

## 90-Day Priority Queue: Molt IR Spec Closure (2026-02-11)
- Source audit: [docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md](docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md).
- Historical baseline snapshot (pre-closure audit): 109 implemented,
  13 partial, 12 missing.
- Current inventory gate (`tools/check_molt_ir_ops.py`) reports
  `missing=0` for spec-op presence in frontend emit/lowering coverage, and
  required dedicated-lane presence in native + wasm backends, plus
  behavior-level semantic assertions for dedicated call/guard/ownership/
  conversion lanes.
- 2026-02-11 implementation update: dedicated frontend/lowering/backend lanes are
  now present for `CALL_INDIRECT`, `INVOKE_FFI`, `GUARD_TAG`,
  `GUARD_DICT_SHAPE`, `INC_REF`/`DEC_REF`/`BORROW`/`RELEASE`, and conversion
  ops (`BOX`/`UNBOX`/`CAST`/`WIDEN`); semantic hardening and differential
  evidence remain in progress.
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
- `INVOKE_FFI` hardening update (2026-02-11): frontend now tags bridge-policy
  invocations with a dedicated lane marker (`s_value="bridge"`), native/wasm
  backends route through `molt_invoke_ffi_ic`, and runtime enforces
  `python.bridge` capability in non-trusted mode for bridge-tagged calls.
- `CALL_INDIRECT` hardening update (2026-02-11): native/wasm backends route
  `call_indirect` through dedicated `molt_call_indirect_ic` /
  `call_indirect_ic` lanes with explicit callable precheck before IC dispatch.
- Frontend mid-end update (2026-02-11): `SimpleTIRGenerator.map_ops_to_json`
  now runs a lightweight optimization pipeline before lowering
  (`_coalesce_check_exception_ops` + CFG/dataflow mid-end). The mid-end now
  builds explicit basic blocks, computes CFG successors/predecessors,
  dominators, and backward liveness, then applies deterministic fixed-point
  passes (`simplify -> SCCP -> canonicalize -> DCE`) with SCCP sparse lattice
  propagation (`unknown`/`constant`/`overdefined`) over SSA names and now
  tracks executable CFG edges explicitly (edge-filtered predecessor merges).
  SCCP coverage now includes arithmetic, boolean, comparison, `TYPE_OF`,
  `CONTAINS`/`INDEX` constant-folding, selected `ISINSTANCE` folds, and
  selected guard/type fact propagation (including guard-failure edge
  termination). It now tracks both try exceptional and try normal completion
  facts and uses them for explicit try-edge threading. Control simplification
  now threads executable edges across `IF`, `LOOP_BREAK_IF_*`, `LOOP_END`,
  and `TRY_*`, applies deeper loop/try rewrites (including conservative
  dead-backedge loop marker flattening and dead try-body suffix pruning after
  proven guard/raise exits), and performs region-aware CFG simplification across
  `IF`/`ELSE`, `LOOP_*`, `TRY_*`, and `LABEL`/`JUMP` regions (including
  dead-label pruning and no-op jump elimination). A structural pre-SCCP
  canonicalization round now strips degenerate empty branch/loop/try regions
  before each SCCP round. The pass also adds conservative branch-tail merging +
  loop-invariant pure-op hoisting and runs effect-aware CSE/DCE under CFG
  safety checks. Read-heap CSE now uses conservative
  alias/effect classes (`dict`/`list`/`indexable`/`attr`) so unrelated writes
  no longer invalidate all read value numbers, including global reuse for
  `GETATTR`/`LOAD_ATTR`/`INDEX` reads under no-interfering-write guards.
  Read-heap invalidation now treats call/invoke operations as conservative
  write barriers, and class-level alias epochs are augmented with lightweight
  object-sensitive epochs for higher hit-rate without unsafe reuse.
  Exceptional try-edge pruning now preserves balanced `TRY_START`/`TRY_END`
  structure unless dominance/post-dominance plus pre-trap
  `CHECK_EXCEPTION`-free proofs permit marker elision.
  The mid-end now also models explicit `CHECK_EXCEPTION` CFG branch targets and
  threads proven exceptional checks into direct `JUMP` edges to handler labels
  with dominance-safe guards, and normalizes nested try/except multi-handler
  join trampolines (label->jump chains) before CSE rounds.
  Expanded cross-block value reuse remains explicitly gated by a CFG
  definite-assignment verifier with automatic fallback to safe mode when proof
  fails. Loop analysis now tracks `(start, step, bound, compare-op)` tuples for
  affine induction facts and monotonic loop-bound proofs used by SCCP. CFG
  construction now lives in a dedicated
  `src/molt/frontend/cfg_analysis.py` module with explicit `BasicBlock` and
  `CFGGraph` structures; mid-end telemetry now reports expanded-mode acceptance
  plus per-transform diagnostics (`sccp_branch_prunes`,
  `loop_edge_thread_prunes`, `try_edge_thread_prunes`,
  `unreachable_blocks_removed`, `cfg_region_prunes`, `label_prunes`,
  `jump_noop_elisions`, `licm_hoists`, `guard_hoist_*`, `gvn_hits`,
  `dce_removed_total`) through `MOLT_MIDEND_STATS=1`. Function-scoped
  acceptance/attempt telemetry is now tracked in `midend_stats_by_function`
  (`sccp`, `edge_thread`, `loop_rewrite`, `guard_hoist`, `cse`,
  `cse_readheap`, `gvn`, `licm`, `dce`, `dce_pure_op`) with
  attempted/accepted/rejected breakdown for transform families. It currently
  elides trivial `PHI`
  nodes, proven no-op `GUARD_TAG` checks, and redundant branch-symmetric guards,
  with join preservation across structured
  `IF`/`ELSE`, `LOOP_*`, `TRY_*`, and `LABEL`/`JUMP` regions; regression coverage
  lives in
  `tests/test_frontend_midend_passes.py`.
- P0 closure items (dedicated lanes landed; semantic/deopt and differential
  coverage hardening remain):
  - `CallIndirect`, `InvokeFFI`, `GuardTag`, `GuardDictShape`.
- P1 ownership/LIR gaps:
  - `IncRef`, `DecRef`, `Borrow`, `Release`.
- P2 conversion and canonicalization gaps:
  - `Box`, `Unbox`, `Cast`, `Widen` and alias-name normalization for partial ops.
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
- Implemented: CI hardening for `tools/check_molt_ir_ops.py` now includes mandatory `--require-probe-execution` after `diff-basic`, so required-probe execution status and failure-queue linkage regressions fail CI.

## Compiler Optimization Stabilization Tranche (2026-02-12)
- Priority override: recover frontend/mid-end compile throughput while preserving correctness and deterministic outputs.
- Current regression signal from active runs: stdlib-heavy module lowering tails dominate compile time and can timeout before wasm/native execution in no-cache bench paths.
- Current tranche status: profile plumbing, tier classification, per-function budget/degrade ladder, per-pass timing/hotspot telemetry, CLI diagnostics sink integration, and deterministic process-level parallel lowering (opt-in) are landed in frontend/CLI. Latest tightening pass now defaults stdlib functions to Tier C unless explicitly promoted, adds finer stage-level/pre-pass budget degrade checkpoints, and applies stdlib-aware effective min-cost thresholds in layer-parallel policy diagnostics; remaining work is broader parallel eligibility and diagnostics UX refinement.
- Execution order (implementation slices):
  1. Profile-gated policy matrix (`dev` cheap/correctness-first, `release` full fixed-point).
  2. Tiered optimization policy (Tier A hot, Tier B normal, Tier C heavy dependency/stdlib).
  3. Per-function budgets with degrade ladder (disable expensive transforms first, never correctness gates).
  4. Per-pass wall-time telemetry and top-offender diagnostics.
  5. Process-level parallel module lowering with deterministic merge order.
  6. Optional large-function optimization workers and staged Rust kernel migration.
- Exit criteria:
  - deterministic second-run IR stability,
  - reduced p95 frontend lowering latency on stdlib-heavy modules,
  - verifier fallback/correctness regressions do not increase.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): ship profile-gated mid-end policy matrix (`dev` correctness-first cheap opts; `release` full fixed-point) with deterministic pass ordering and explicit diagnostics (CLI->frontend profile plumbing is landed; diagnostics sink now also surfaces active midend policy config and heuristic knobs; remaining work is broader tuning closure and any additional triage UX).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add tiered optimization policy (Tier A entry/hot functions, Tier B normal user functions, Tier C heavy stdlib/dependency functions) with deterministic classification and override knobs (baseline deterministic classifier + env overrides are landed; runtime-feedback and PGO hot-function promotion are now wired through the existing tier promotion path).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): enforce per-function mid-end wall-time budgets with an automatic degrade ladder that disables expensive transforms before correctness gates and records degrade reasons (budget/degrade ladder is landed in fixed-point loop; heuristic tuning + diagnostics surfacing remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add per-pass wall-time telemetry (`attempted`/`accepted`/`rejected`/`degraded`, `ms_total`, `ms_p95`) plus top-offender diagnostics by module/function/pass (frontend per-pass timing/counters, CLI/JSON sink wiring, and hotspot rendering are landed).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P0, status:implemented): root-cause/fix mid-end miscompiles feeding missing values into runtime lookup/call sites (SCCP treats MISSING as non-propagatable via _SCCP_MISSING sentinel, DCE protects MISSING ops from elimination, definite-assignment verifier tracks MISSING definitions explicitly; dev-profile gate removed — mid-end runs for both dev and release profiles; stdlib gate remains until canonicalized stdlib lowering is proven stable).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): surface active optimization profile/tier policy and degrade events in CLI build diagnostics and JSON outputs for deterministic triage (diagnostics sink now includes profile/tier/degrade summaries + pass hotspots, and stderr verbosity partitioning is landed; remaining work is richer CLI UX controls beyond verbosity).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add process-level parallel frontend module-lowering and deterministic merge ordering, then extend to large-function optimization workers where dependency-safe (dependency-layer process-pool lowering is landed behind `MOLT_FRONTEND_PARALLEL_MODULES`; remaining work is broader eligibility and worker-level tuning telemetry).
- TODO(compiler, owner:compiler, milestone:LF3, priority:P1, status:planned): migrate hot mid-end kernels (CFG build, SCCP lattice transfer, dominator/liveness) to Rust with Python orchestration preserved for policy control.

## Parity-First Execution Plan
Guiding principle: lock CPython parity and robust test coverage before large optimizations or new higher-level surface area.

Parity gates (required before major optimizations that touch runtime, call paths, lowering, or object layout):
- Relevant matrix entries in [docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md](docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md), [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md),
  [docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md), [docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md](docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md), and
  [docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md](docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md) are updated to match the implementation status.
- Differential tests cover normal + edge-case behavior (exception type/messages, ordering, and protocol fallbacks).
- Native + WASM parity checks added or updated for affected behaviors.
- Runtime lifecycle plan tracked and up to date ([docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md](docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md)).

Plan (parity-first, comprehensive):
1) Matrix audit and coverage map: enumerate missing/partial cells in the matrices above, link each to at least one
   differential test, and ensure TODOs exist in code for remaining gaps.
2) Core object protocols: attribute access/descriptor binding, dunder fallbacks, container protocols
   (`__iter__`/`__len__`/`__contains__`/`__reversed__`), equality/ordering/hash/format parity, and strict exception behavior.
3) Call + iteration semantics: CALL_BIND/CALL_METHOD, `*args`/`**kwargs`, iterator error propagation, generators,
   coroutines, and async iteration; keep native + WASM parity in lockstep.
4) Stdlib core: builtins + `collections`/`functools`/`itertools`/`operator`/`heapq`/`bisect` to parity per
   [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md), with targeted differential coverage.
5) Security + robustness tests: capability gating, invalid input handling, descriptor edge cases, and recursion/stack
   behavior to catch safety regressions early.

## Concurrency & Parallelism (Vision -> Plan)
- Default: CPython-correct asyncio semantics on a single-threaded event loop (deterministic ordering, structured cancellation).
- True parallelism is explicit: executors + isolated runtimes/actors with message passing.
- Shared-memory parallelism is opt-in, capability-gated, and limited to explicitly safe types.
- Current: runtime mutation is serialized by a GIL-like lock in the global runtime state; see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md).

Planned milestones:
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P0, status:partial): Rust event loop + I/O poller with cancellation propagation and deterministic scheduling guarantees; expose as asyncio core. Pipe transports (`connect_read_pipe`/`connect_write_pipe`) now implemented with 11 new intrinsics; 42 new Rust intrinsics for Future/Event/Lock/Semaphore/Queue state machines landed; WASM capability gating for 6 I/O operations complete; Transport/Protocol base classes added; 3.13/3.14 version-specific APIs gated.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P0, status:partial): full asyncio parity (tasks, task groups, streams, subprocess, executors) built on the runtime loop. Pipe transports, synchronization primitives, and version-gated APIs now implemented; remaining: full executor semantics and advanced loop APIs.
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define the per-runtime GIL strategy, runtime instance ownership model, and allowed cross-thread object sharing rules (see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)).
- Implemented: explicit `PyToken` GIL token API and `with_gil`/`with_gil_entry` enforcement on runtime mutation entrypoints (see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)).
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors, explicit message passing, and capability-gated shared-memory primitives.
- TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P1, status:planned): wasm host parity for the asyncio runtime loop, poller, sockets, and subprocess I/O.

## Performance
- Vector reduction kernels now cover `sum`/`prod`/`min`/`max` plus float `sum` lanes (list/tuple/range variants), with adaptive lane gating counters (`MOLT_ADAPTIVE_VEC_LANES`) to reduce failed-probe overhead while preserving generic fallbacks.
- Range materialization now has a dedicated runtime lane (`list_from_range`) used by `list(range(...))` and simple `[i for i in range(...)]` comprehensions to remove generator/list-append call overhead from hot loops.
- Dict increment idioms (`d[k] = d.get(k, 0) + delta`) now lower to a dedicated runtime lane (`dict_inc`) with int fast path + generic add fallback.
- Fused split+count lanes (`string_split_ws_dict_inc`, `string_split_sep_dict_inc`) now include a string-key dict probe fast path (hash+byte compare) with explicit fallback to generic dict semantics for mixed/non-string-key maps.
- Iterable element hints now propagate through for-loop lowering (including `file_text`/`file_bytes` iterables), unlocking broader split/find/count primitive lowering in ETL-style loops without manual type hints.
- `statistics.mean/stdev` on slice expressions now lower to dedicated runtime lanes (`statistics_mean_slice`, `statistics_stdev_slice`) with list/tuple fast paths and runtime-owned generic fallback for non-list/tuple inputs.
- Slice statistics lanes now include int/float element fast-coercion in hot loops (generic numeric fallback preserved).
- `abs(...)` now lowers to a dedicated runtime lane (`abs`) to remove dynamic-call overhead from numeric hot loops.
- `dict.setdefault(key, [])` now lowers to a dedicated lane (`dict_setdefault_empty_list`) that avoids eager empty-list allocation and reduces grouping overhead in ETL-style loops.
- String kernel SIMD paths cover find/split/replace with Unicode-safe index translation; next: Unicode index caches and wider SIMD (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): Unicode index caches + wider SIMD).
- TODO(perf, owner:compiler, milestone:RT2, priority:P1, status:planned): reduce startup/import-path dispatch overhead for stdlib-heavy scripts (bind intrinsic-backed imports at lower cost and trim module-init call traffic) so wins translate to short-lived CLI/data scripts as well as long-running services.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): implement sharded/lock-free handle resolution and track lock-sensitive benchmark deltas (attr access, container ops).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): stream print writes to avoid building intermediate output strings for large payloads.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): pre-size `dict.fromkeys` using iterable length hints to reduce rehashing.
- Implemented: websocket readiness integration via io_poller for native + wasm (`molt_ws_wait_new`) to avoid busy-polling and enable batch wakeups.
- Implemented: release iteration compile profile lane via Cargo `release-fast`, including dedicated compile-progress measurement cases (`release_fast_cold`, `release_fast_warm`, `release_fast_nocache_warm`) for before/after release-lane comparison.
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
- Implemented: Unicode-backed `str` predicate tranche (`str.isdigit`/`str.isdecimal`/`str.isnumeric`/`str.isalpha`/`str.isalnum`/`str.islower`/`str.isupper`/`str.isspace`/`str.istitle`/`str.isprintable`/`str.isascii`) with coverage in `tests/differential/basic/str_predicates_surface.py`.
- Implemented: lambda lowering with closures, defaults, and kw-only/varargs support.
- Implemented: `sorted()` builtin with stable ordering + key/reverse (core ordering types).
- Implemented: `sorted()` enforces keyword-only `key`/`reverse` arguments (CPython parity).
- Implemented: `list.sort` with key/reverse and rich-compare fallback for user-defined types.
- Implemented: `str.lower`/`str.upper`/`str.capitalize`/`str.swapcase`, `list.clear`/`list.copy`/`list.reverse`, and `dict.setdefault`/`dict.update`.
- Implemented: container dunder/membership fallbacks (`__contains__`/`__iter__`/`__getitem__`) and builtin class method access for list/dict/str/bytes/bytearray.
- Implemented: dynamic call binding for bound methods/descriptors with builtin defaults + expanded class decorator parity coverage.
- Implemented: print keyword-argument parity tests (`sep`, `end`, `file`, `flush`) for native + wasm.
- Implemented: compiled `sys.argv` initialization for native + wasm harness; TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): filesystem-encoding + surrogateescape decoding parity.
- Implemented: `sys.executable` override via `MOLT_SYS_EXECUTABLE` (diff harness pins it to the host Python to avoid recursive `-c` subprocess spawns).
- TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): complete code object parity for closure/generator/coroutine metadata (`co_freevars`/`co_cellvars` values and full `co_flags` bitmask semantics).
- TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): importlib.machinery pending parity (package/module shaping + file reads + restricted-source execution lanes are intrinsic-lowered; remaining loader/finder parity is namespace/extension/zip behavior).
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
- Implemented: structural pattern matching (`match`/`case`) via cell-based AST-to-IR desugaring — all PEP 634 pattern types (literal, variable, sequence, mapping, class, or, as, star, singleton, guard), with 24 differential test files (~565 lines) in `tests/differential/basic/pattern_matching_*.py` and `tests/differential/basic/match_*.py`.
- Implemented: `MATCH_*` opcode semantics covered via AST desugaring to existing IR ops (ISINSTANCE, GETATTR, INDEX, DICT_GET, LEN, SLICE, EQ, IS).
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): f-string format specifiers and debug spec (`f"{x:.2f}"`, `f"{x=}"`) parity (see [docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md](docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md)).
- TODO(syntax, owner:frontend, milestone:M3, priority:P3, status:missing): type alias statement (`type X = ...`) and generic class syntax (`class C[T]: ...`) coverage (see [docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md](docs/spec/areas/compat/surfaces/language/syntactic_features_matrix.md)).
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator opcode coverage and lowering gaps (see [docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md)).
- TODO(compiler, owner:compiler, milestone:TC2, priority:P0, status:implemented): fix async lowering/back-end verifier for `asyncio.gather` poll paths — native backend now inserts pending/ready blocks in dominance-compatible order with all target blocks registered before branch emission; WASM backend pre-stores the pending return value before the If block so the conditional body has a clean stack profile.
- Implemented: generator/async poll trampolines are task-aware (generator/coroutine/asyncgen) so wasm no longer relies on arity overrides.
- TODO(perf, owner:compiler, milestone:TC2, priority:P2, status:planned): optimize wasm trampolines with bulk payload initialization and shared helpers to cut code size and call overhead.
- Implemented: cached task-trampoline eligibility on function headers to avoid per-call attribute lookups.
- Implemented: coroutine trampolines reuse the current cancellation token to avoid per-call token allocations.
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector implementation (see [docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md](docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md)).
- Implemented: runtime lifecycle refactor moved caches/pools/async registries into `RuntimeState`, removed lazy_static globals, and added TLS guard cleanup for user threads (see [docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md](docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md)).
- Implemented: host pointer args use raw pointer ABI; strict-provenance Miri stays green (pointer registry remains for NaN-boxed handles).
- TODO(runtime-provenance, owner:runtime, milestone:RT2, priority:P2, status:planned): bound or evict transient const-pointer registrations in the pointer registry.
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence policy (see [docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md](docs/spec/areas/compat/surfaces/language/semantic_behavior_matrix.md)).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): extend the landed `libmolt` C API bootstrap shim to broader source-compat, `Py_LIMITED_API` targeting, and ABI guarantees (see [docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md](docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md)).

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
- Differential tests: `tests/differential/basic/file_open_modes.py`, `file_buffering_text.py`,
  `file_text_encoding_newline.py`, `file_iteration_context.py`, `file_seek_tell_fileno.py` (move to verified subset on parity).
- Pytest unit tests: invalid mode/buffering/encoding/newline combos, fd-based `open`, `closefd`/`opener` errors, path-like objects.
- WASM parity: harness tests for read/write/line iteration using temp files via Node/WASI host I/O.
- Security/robustness: fuzz mode strings + newline values, and validate close/idempotency + leak-free handles.
- Windows parity: newline translation + path handling coverage in CI.
- Differential suite is now split by ownership lane: core/builtin semantics in `tests/differential/basic/`, stdlib module/submodule coverage in `tests/differential/stdlib/`, and wasm-focused scaffolds in `tests/wasm_planned/` until wasm parity lands.

Sign-off criteria:
- All above tests pass on 3.12/3.13/3.14 + wasm parity runs; matrices + STATUS updated; no capability bypass.

## Stdlib
- Partial: importable `builtins` module binding supported builtins (attribute gaps tracked in the matrix).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill `builtins` module attribute coverage.)
- Partial: asyncio shim (`run`/`sleep` lowered to runtime with delay/result semantics; `wait`/`wait_for`/`shield` + basic `gather` supported; `set_event_loop`/`new_event_loop` stubs); pipe transports (`connect_read_pipe`/`connect_write_pipe`) implemented with 11 new pipe transport intrinsics; 42 new Rust intrinsics for Future/Event/Lock/Semaphore/Queue state machines; all 97 bare `except` blocks eliminated; WASM capability gating for 6 I/O ops; Transport/Protocol base classes added; 3.13 (`as_completed` async iter, `Queue.shutdown`) and 3.14 (`get_event_loop` `RuntimeError`, child watcher removal, policy deprecation) version-gated APIs added (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task API parity).
- Implemented: asyncio TaskGroup/Runner cancellation fanout lowered through
  intrinsic batch cancellation (`molt_asyncio_cancel_pending`) plus intrinsic
  gather-based drain paths.
- Implemented: asyncio lock/condition/semaphore/barrier/queue waiter fanout +
  cancellation-removal loops now route through Rust intrinsics
  (`molt_asyncio_waiters_notify`, `molt_asyncio_waiters_notify_exception`,
  `molt_asyncio_waiters_remove`,
  `molt_asyncio_barrier_release`) to keep hot synchronization paths off
  Python-side list/deque loops.
- Implemented: asyncio future transfer and event waiter-teardown callbacks now
  lower through Rust intrinsics (`molt_asyncio_future_transfer`,
  `molt_asyncio_event_waiters_cleanup`), shrinking Python callback logic in
  `Task.__await__`/`wrap_future` and token cleanup paths.
- Implemented: asyncio TaskGroup done-callback error fanout and event-loop
  ready-queue draining now lower through Rust intrinsics
  (`molt_asyncio_taskgroup_on_task_done`, `molt_asyncio_ready_queue_drain`),
  reducing Python task-scan/callback loops in cancellation/error and
  ready-dispatch hot paths.
- Partial: shims for `warnings`, `traceback`, `types`, `inspect`, `ast`, `ctypes`, `uuid`, `urllib.parse`, `fnmatch`, `copy`, `pickle` (protocol 0 only), `pprint`, `string`, `struct`, `typing`, `sys` (significantly expanded 2026-02-25), `os` (significantly expanded 2026-02-25), `json`, `asyncio`, `_asyncio` (expanded 2026-02-25), `shlex` (`quote`), `threading`, `_thread` (full rewrite 2026-02-25), `signal` (expanded 2026-02-25), `subprocess` (expanded 2026-02-25), `concurrent.futures` (verified complete 2026-02-25), `weakref`, `heapq`, `functools`, `itertools`, `zipfile`, `zipimport`, and `collections` (capability-gated env access).
- Implemented: top-level `_collections` now exposes an intrinsic-backed `OrderedDict` wrapper (with `deque`/`defaultdict` compatibility re-exports); broader collections-family parity remains partial.
- Partial: `decimal` shim backed by Rust intrinsics (contexts/traps/flags, quantize/compare/normalize/exp/div, `as_tuple`, `str`/`repr`/float conversions) with native Rust backend when vendored `libmpdec` sources are unavailable.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Decimal arithmetic + formatting parity (add/sub/mul/pow/sqrt/log/ln, quantize edge cases, NaN payloads).)
- Implemented: strict intrinsics registry + removal of CPython shim fallbacks in tooling/tests; JSON/MsgPack helpers now use runtime intrinsics only.
- Implemented: `tools/check_stdlib_intrinsics.py` now enforces fallback-pattern bans across all stdlib modules by default (strict all-stdlib mode); opt-down to intrinsic-backed-only scope is explicit via `--fallback-intrinsic-backed-only`.
- Implemented: `tools/check_stdlib_intrinsics.py` now enforces CPython 3.12/3.13/3.14 union coverage for both top-level stdlib names and `.py` submodule names (missing-name failures, required-package shape checks, and duplicate module/package mappings).
- Implemented: stdlib coverage stubs are synchronized by `tools/sync_stdlib_top_level_stubs.py` and `tools/sync_stdlib_submodule_stubs.py` against the generated baseline in `tools/stdlib_module_union.py` (`tools/gen_stdlib_module_union.py`).
- Implemented: probe-only and python-only buckets are currently zero; union coverage is complete by name (`320` top-level names, `540` submodule names), with remaining work concentrated in intrinsic-partial burn-down.
- Implemented: non-CPython stdlib top-level extras are now constrained to `_intrinsics` and `test` only.
- Implemented: Molt-specific DB client shim moved from stdlib (`molt_db`) to `moltlib.molt_db`, with `molt.molt_db` compatibility shim retained.
- Implemented: `ast.parse` / `ast.walk` / `ast.get_docstring` now route through Rust intrinsics (`molt_ast_parse`, `molt_ast_walk`, `molt_ast_get_docstring`) with Python wrappers reduced to constructor wiring and argument forwarding.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): extend Rust ast lowering to additional stmt/expr variants and full argument shape parity; unsupported nodes currently raise RuntimeError immediately.
- Implemented: `os` fd I/O lowering for compiled binaries (`molt_os_pipe`, `molt_os_read`, `molt_os_write`) with differential coverage (`os_pipe_basic.py`, `os_read_write_basic.py`, `os_read_write_errors.py`) in intrinsic-only runs.
- Implemented (2026-02-25): `os` module coverage significantly expanded to ~40
  intrinsic-backed APIs (see Stdlib Intrinsics Sprint above for full inventory).
- Implemented (2026-02-25): `sys` module expanded with ~20 new intrinsics for
  platform metadata, info structs, and interpreter hooks (see sprint Track B).
- Implemented (2026-02-25): `_thread` module fully rewritten on existing thread
  intrinsics; `signal` module expanded with 17 new intrinsics (see sprint Track C).
- Implemented (2026-02-25): `_asyncio` expanded with 4 new C-accelerated
  task-state intrinsics (see sprint Track D).
- Implemented (2026-02-28): asyncio pipe transports, 42 new Future/Event/Lock/
  Semaphore/Queue intrinsics, Transport/Protocol base classes, WASM I/O gating,
  and 3.13/3.14 version-specific APIs (see Asyncio & Tkinter Parity Sprint).
- Implemented (2026-02-28): tkinter 10 Rust intrinsics wired, all strict mode
  violations resolved, 3.13/3.14 version-gated APIs, 100% submodule coverage
  (see Asyncio & Tkinter Parity Sprint).
- Implemented (2026-02-25): `subprocess` expanded with `molt_process_spawn_ex`
  intrinsic and broader Popen surface (see sprint Track E);
  `concurrent.futures` verified intrinsic-complete.
- Implemented: threading stdlib parity lane is green (`tests/differential/stdlib/threading_*.py` -> `24/24` pass) under intrinsic-only compiled runs with RSS profiling enabled.
- Implemented: importlib distribution path discovery now lowers through runtime intrinsics (`molt_importlib_metadata_dist_paths`) and `importlib.metadata` file reads now lower via `molt_importlib_read_file` (no Python-side dist-info scan/open fallback). Note: `molt_importlib_namespace_paths` is an internal Rust helper, not a Python-callable intrinsic — namespace path resolution is performed inside `molt_importlib_find_spec_orchestrate`.
- Implemented: `importlib.resources` traversable stat/listdir shaping now lowers through runtime payload intrinsic (`molt_importlib_resources_path_payload`), and resources open/read helpers now use intrinsic-backed reads (`molt_importlib_read_file`) without Python file-open fallback.
- Implemented: `importlib.resources` loader-reader `resource_path` now enforces filesystem-only results across direct/traversable/roots fallback lanes; archive-member paths are filtered to `None` and continue through intrinsic byte-open flows.
- Implemented: `importlib.metadata` header + entry-point parsing now lowers through runtime payload intrinsic (`molt_importlib_metadata_payload`), leaving wrappers as cache/object shapers.
- Implemented: `importlib.util.find_spec` now routes all spec resolution through `molt_importlib_find_spec_orchestrate` (aggregate orchestration intrinsic covering meta_path, path_hooks, namespace packages, and cache-key computation); Python wrappers no longer run a separate filesystem probe path. Note: granular precursors `molt_importlib_find_spec_payload`, `molt_importlib_find_spec_from_path_hooks`, `molt_importlib_existing_spec`, `molt_importlib_parent_search_paths`, `molt_importlib_finder_signature`, `molt_importlib_path_importer_cache_signature`, `molt_importlib_coerce_search_paths`, `molt_importlib_search_paths`, `molt_importlib_runtime_state_payload`, and `molt_importlib_runtime_state_view` are dead — handled entirely inside the orchestrator.
- Implemented: `importlib.import_module` now falls back to the intrinsic-backed spec/loader flow (`find_spec` + `module_from_spec` + loader `exec_module`) when direct runtime import returns a non-module payload, preserving dynamic `sys.path` package imports without host-Python fallback.
- Implemented: `importlib.resources.files` package root/namespace resolution now lowers through runtime payload intrinsic (`molt_importlib_resources_package_payload`) rather than Python namespace scanning.
- Implemented: `importlib.resources` loader-reader discovery now falls back from `module.__spec__.loader` to `module.__loader__` inside runtime intrinsic `molt_importlib_resources_loader_reader`, keeping custom reader lookup fully runtime-owned.
- Implemented: `importlib.machinery.SourceFileLoader.exec_module` now sources decoded module text through runtime payload intrinsic (`molt_importlib_source_exec_payload`) before intrinsic restricted execution (`molt_importlib_exec_restricted_source`), removing Python-side source decode fallback logic.
- Implemented: `importlib.machinery` extension/sourceless intrinsic execution now continues candidate probing after unsupported restricted-shim parser candidates, then raises deterministic `ImportError` only after all intrinsic candidates are exhausted.
- Implemented: restricted shim execution in runtime now includes `from ... import *` semantics (`__all__` validation + underscore fallback export rules), reducing extension/sourceless shim divergence without host fallback.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close parity gaps for `ast`, `ctypes`, `urllib.parse`, and `uuid` (see stdlib matrix).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand zipfile/zipimport with bytecode caching + broader archive support.
- Implemented: `zipfile` central-directory parsing and ZIP64-extra payload construction now lower through dedicated Rust intrinsics (`molt_zipfile_parse_central_directory`, `molt_zipfile_build_zip64_extra`), `zipfile._path` directory/implied-dir matching and glob translation route through dedicated Rust intrinsics (`molt_zipfile_path_implied_dirs`, `molt_zipfile_path_resolve_dir`, `molt_zipfile_path_is_child`, `molt_zipfile_path_translate_glob`), and `zipfile.main` extract-path sanitization lowers through `molt_zipfile_normalize_member_path`.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures` (spawn-based partial; `subprocess` significantly advanced with `molt_process_spawn_ex` intrinsic in 2026-02-25 sprint; `concurrent.futures` verified intrinsic-complete; IPC + lifecycle parity pending).
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.
- Partial: capability-gated `socket`/`select`/`selectors` backed by runtime sockets + io_poller with intrinsic-backed selector objects (`poll`/`epoll`/`kqueue`/`devpoll`) and backend selector classes; native + wasmtime host implemented. Node/WASI host bindings are wired in `run_wasm.js`; browser host supports WebSocket-backed stream sockets + io_poller readiness while UDP/listen/server sockets remain unsupported.
  (TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): expand browser socket coverage (UDP/listen/server sockets) + parity tests.)
- Implemented: wasm/non-Unix socket host ABI now carries ancillary payload buffers + recvmsg `msg_flags` for `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into`; wasm runtime paths no longer hardcode `msg_flags=0`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete cross-platform ancillary parity for `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`), while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty ancillary control messages.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (Encoder/Decoder classes, JSONDecodeError details, runtime fast-path parser).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): continue `re` parser/matcher lowering into Rust intrinsics; literal/any/char-class advancement, char/range/category matching, anchor/backref/scoped-flag matcher nodes, group capture/value materialization, and replacement expansion are intrinsic-backed, while remaining lookaround variants, verbose parser edge cases, and full Unicode class/casefold parity are pending.
- Implemented: `queue` now lowers `LifoQueue` and `PriorityQueue` construction/ordering through runtime intrinsics (`molt_queue_lifo_new`, `molt_queue_priority_new`) on top of existing intrinsic-backed queue operations.
- Implemented: queue timeout-type differential tranche now covers invalid `timeout` typing parity: unbounded `Queue.put(timeout='bad')` ignores timeout and succeeds, while `Queue.get(timeout='bad')`, bounded-full `Queue.put(timeout='bad')`, and `SimpleQueue.get(timeout='bad')` raise `TypeError`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete queue edge-case/API parity (task accounting corners, comparator/error-path fidelity, and broader CPython coverage).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): `struct` intrinsics cover `pack`/`unpack`/`calcsize` + `pack_into`/`unpack_from`/`iter_unpack` across the CPython 3.12 format table (including half-float) with C-contiguous nested-memoryview windows; remaining gaps are exact CPython diagnostic-text parity on selected edge cases.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `time` module surface (`timegm`) + deterministic clock policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics (`setlocale` catalog coverage, category handling, and host-locale compatibility).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement gettext translation catalog/domain parity (filesystem-backed `.mo` loading and locale/domain selection).
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale data for `time.localtime`/`time.strftime` on wasm hosts.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): tighten `weakref.finalize` shutdown-order parity (including `atexit` edge cases) against CPython.
- Implemented: `abc.update_abstractmethods` now uses runtime intrinsic `molt_abc_update_abstractmethods`; Python-side abstractmethod scanning logic was removed from `abc.py`.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/backslashreplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (full encodings import hooks + charmap codec intrinsics); incremental encoder/decoder now backed by Rust handle-based intrinsics, BOM constants from Rust, register_error/lookup_error wired.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `pickle` protocol 1+ and broader type coverage (bytes/bytearray, memo cycles).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): finish remaining `math` intrinsics (determinism policy); predicates, `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc` are now wired in Rust.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): fill out `types` shims (TracebackType, FrameType, FunctionType, coroutine/asyncgen types, etc).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): replace placeholder iterator/view types (`object`/`type`) so ABC registration doesn't need guards.
- TODO(tests, owner:runtime, milestone:SL1, priority:P1, status:partial): expand native+wasm codec parity coverage for binary/floats/large ints/tagged values + deeper container shapes.
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
- Import-only allowlist expanded for `binascii`, `unittest`, `site`, `sysconfig`, `collections.abc`, `importlib`, and `importlib.util`; planned additions now cover the remaining CPython 3.12+ stdlib surface (see [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md) Section 3.0b), including `annotationlib`, `compileall`, `configparser`, `difflib`, `dis`, `encodings`, `tokenize`, `trace`, `xmlrpc`, and `zipapp` (API parity pending; TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + tests).

## Compatibility Matrix Execution Plan (Next 8 Steps)
1) Done: TC2 iterable unpacking + starred targets in assignment/for targets (tests + spec/status updates).
2) TC2: remaining StopIteration semantics (sync/async) with differential coverage (StopIteration.value propagation done).
3) TC2: builtin conversions (`bool`, `str`) with hook/error parity.
- Implemented: `str(bytes, encoding, errors)` decoding for bytes-like inputs (matches `bytes.decode` codec/handler coverage).
4) Done: TC2 async comprehensions lowering + runtime support with parity tests.
5) TC2/TC3: reflection builtins, CPython `hash` parity (`PYTHONHASHSEED`) + `format`/rounding; update tests + docs.
   Implemented: object-level `__getattribute__`/`__setattr__`/`__delattr__` builtins.
6) SL1: `functools` (`lru_cache`, `partial`, `reduce`) with compile-time lowering and deterministic cache keys; `cmp_to_key`/`total_ordering` landed.
7) SL1: `itertools` + `operator` intrinsics plus `heapq` fast paths; `bisect` is now fully intrinsic-lowered (search + insort + bounds normalization) and `heapq` fast paths remain active.
8) SL1: finish `math` intrinsics beyond `log`/`log2`/`exp`/`sin`/`cos`/`acos`/`lgamma` and trig/hyperbolic (remaining: determinism policy), plus deterministic `array`/`struct` layouts with wasm/native parity tests.

## Offload / IPC
- Partial: `molt_accel` v0 scaffolding (stdio framing + client + decorator) with auto cancel-check detection, payload/response byte metrics, and shared demo payload builders; `molt_worker` stdio shell with demo handlers and compiled dispatch (`list_items`/`compute`/`offload_table`/`health`), plus optional worker pooling via `MOLT_ACCEL_POOL_SIZE`.
  (TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): finalize accel retry/backoff + non-demo handler coverage.)
- Implemented: compiled export loader + manifest validation (schema, reserved-name filtering, error mapping) with queue/timeout metrics.
- Implemented: worker tuning via `MOLT_WORKER_THREADS` and `MOLT_WORKER_MAX_QUEUE` (CLI overrides).
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): propagate cancellation into real DB tasks; extend compiled handlers beyond demo coverage.
- TODO(offload, owner:runtime, milestone:SL2, priority:P1, status:planned): add a Phase 1 in-process fast path for precompiled endpoint exports (startup-loaded ABI, no runtime compilation) while preserving worker IPC semantics for capability gating, cancellation, and error mapping.

## DB
- Partial: `molt-db` pool skeleton (bounded, sync), feature-gated async pool primitive, SQLite connector (native-only; wasm parity pending), and async Postgres connector with statement cache; `molt_worker` exposes `db_query`/`db_exec` for SQLite + Postgres (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity).
- Top priority: wasm parity for DB connectors before expanding DB adapters or query-builder ergonomics.
- Implemented: wasm DB client shims + parity test (`molt_db` async helper) consume response streams and surface bytes/Arrow IPC; Node/WASI host adapter forwards `db_query`/`db_exec` to `molt-worker` via `run_wasm.js`.

## Edge And Workers
- Proposed: `Molt Edge` as a first-class Edge/Workers tier with a minimal VFS, snapshot-oriented deployment, and Cloudflare-first host profile. Canonical docs: `0294_MOLT_EDGE_WORKERS_RUNTIME_PROPOSAL.md`, `0295_MOLT_ENHANCEMENT_PROPOSAL_0001_EDGE_WORKERS_TIER.md`, and `0968_MOLT_EDGE_WORKERS_VFS_AND_HOST_CAPABILITIES.md`.
- Rationale: replace Pyodide-style Worker deployments with a smaller, compiled, capability-first runtime rather than copying Emscripten's whole compatibility model.
- Required runtime work: `/bundle`, `/tmp`, stdio pseudo-devices, explicit storage capability surfaces, and `molt.snapshot` generation/restore.
- Required platform work: Cloudflare Worker host adapter that maps Worker-native VFS and Web APIs onto Molt capability contracts; browser and WASI hosts stay aligned to the same mount/capability model.
- Required validation: cold-start before/after snapshot benchmarks, size tracking, and parity suites for package/resource reads, temp files, capability denials, and Worker-host integration flows.

## Parity Cluster Plan (Next)
- 1) Async runtime core: Task/Future APIs, scheduler, contextvars, and cancellation injection into awaits/I/O. Key files: `runtime/molt-runtime/src/lib.rs`, `src/molt/stdlib/asyncio/__init__.py`, `src/molt/stdlib/contextvars.py`, [docs/spec/STATUS.md](docs/spec/STATUS.md). Outcome: asyncio loop/task parity for core patterns. Validation: new unit + differential tests; `tools/dev.py test`.
- 2) Capability-gated async I/O: sockets/SSL/selectors/time primitives with cancellation propagation. Key files: [docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md](docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md), [docs/spec/areas/runtime/0505_IO_ASYNC_AND_CONNECTORS.md](docs/spec/areas/runtime/0505_IO_ASYNC_AND_CONNECTORS.md), `runtime/molt-runtime/src/lib.rs`. Outcome: async I/O primitives usable by DB/HTTP stacks. Validation: I/O unit tests + fuzzed parser tests + wasm/native parity checks.
- Implemented: native host-level websocket connect hook for `molt_ws_connect` with capability gating for production socket usage.
- 3) DB semantics expansion: implement `db_exec`, transactions, typed param mapping; add multirange + array lower-bound decoding. Key files: `runtime/molt-db/src/postgres.rs`, `runtime/molt-worker/src/main.rs`, [docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md](docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md), [docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md](docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md), [docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md](docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md). Outcome: production-ready DB calls with explicit write gating and full type decoding. Validation: dockerized Postgres integration + cancellation tests.
- 4) WASM DB parity: define WIT/host calls for DB access and implement wasm connectors in molt-db. Key files: `wit/molt-runtime.wit`, `runtime/molt-runtime/src/lib.rs`, `runtime/molt-db/src/lib.rs`, [docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md](docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md). Outcome: wasm builds can execute DB queries behind capability gates. Validation: wasm harness tests + native/wasm result parity.
- 5) Framework-agnostic adapters: finalize `molt_db_adapter` + helper APIs for Django/Flask/FastAPI with shared payload builders. Key files: `src/molt_db_adapter/`, [docs/spec/areas/db/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md](docs/spec/areas/db/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md), `demo/`, `tests/`. Outcome: same IPC contract across frameworks with consistent error mapping. Validation: integration tests in sample Django/Flask/FastAPI apps.
- 6) Production hardening: propagate cancellation into compiled entrypoints/DB tasks, add pool/queue metrics, run bench harness. Key files: `runtime/molt-worker/src/main.rs`, `bench/scripts/`, [docs/spec/areas/demos/0910_REPRO_BENCH_VERTICAL_SLICE.md](docs/spec/areas/demos/0910_REPRO_BENCH_VERTICAL_SLICE.md). Outcome: stable P99/P999 and reliable cancellation/backpressure. Validation: `bench/scripts/run_stack.sh` + stored JSON results.

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
- Implemented: Cranelift 0.128 backend tuning tranche in `runtime/molt-backend` with profile-safe defaults and explicit knobs:
  - release default `log2_min_function_alignment=4` (16-byte minimum function alignment),
  - dev default `regalloc_algorithm=single_pass` for faster local compile loops,
  - opt-in overrides via `MOLT_BACKEND_REGALLOC_ALGORITHM`, `MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2`, and `MOLT_BACKEND_LIBCALL_CALL_CONV`.
- Track complex performance work in [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md) before large refactors.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:planned): replace pointer-registry locks with sharded or lock-free lookups once registry load is characterized.
- Implemented: default `MOLT_HOME` now resolves under `MOLT_CACHE/home` when `MOLT_HOME` is unset, so artifact/clean workflows no longer use the legacy `~/.molt` path by default.
- TODO(tooling, owner:release, milestone:TL2, priority:P2, status:planned): formalize release tagging (start at `v0.0.001`, increment thousandth) and require super-bench stats for README performance summaries.

## Django Demo Path (Draft, 5-Step)
- Step 1 (Core semantics): close TC1/TC2 gaps in [docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md](docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md) for Django-heavy types (dict/list/tuple/set/str, iter/len, mapping protocol, kwargs/varargs ordering per docs/spec/areas/compat/contracts/call_argument_binding_contract.md, descriptor hooks, class `__getattr__`/`__setattr__`).
- Step 2 (Import/module system): package resolution + module objects, `__import__`, and a deterministic `sys.path` policy; unblock `importlib` basics.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root build discovery (namespace packages + PYTHONPATH roots done; remaining: deterministic graph caching + `__init__` edge cases).
- Step 3 (Stdlib essentials): advance [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md) for `functools`, `itertools`, `operator`, `collections`, `contextlib`, `inspect`, `typing`, `dataclasses`, `enum`, `re`, and `datetime` to Partial with tests.
- Step 4 (Async/runtime): production-ready asyncio loop/task APIs, contextvars, cancellation injection, and long-running workload hardening.
- Step 5 (I/O + web/DB): capability-gated `os`, `sys`, `pathlib`, `logging`, `time`, `selectors`, `socket`, `ssl`; ASGI/WSGI surface, HTTP parsing, and DB client + pooling/transactions (start sqlite3 + minimal async driver), plus deterministic template rendering.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining `pathlib` parity gaps (glob edge cases, hidden/root_dir semantics, symlink nuances, and broader PurePath/PurePosixPath API surface) after intrinsic splitroot-aware `isabs`/`parts`/`parents` parity work.
- Cross-framework note: DB IPC payloads and adapters must remain framework-agnostic to support Django/Flask/FastAPI.

## GPU Acceleration Milestones

| Milestone | Description | Prerequisites | Status |
|-----------|-------------|---------------|--------|
| **M-GPU-1** | CPU kernelization: TIR loop classifier + KernelIR → scalar/SIMD/threaded CPU execution | TC2 (type coverage), TL2 (tooling) | Planned |
| **M-GPU-2** | Columnar runtime: MoltTable/MoltColumn backed by Arrow buffers with SIMD kernels | M-GPU-1, DF1 (dataframe tier 1) | Planned |
| **M-GPU-3** | libcudf backend: Route DataFrame ops to GPU via Arrow C Device Interface interop | M-GPU-2, cudarc evaluation | Planned |
| **M-GPU-4** | Custom GPU kernels: Compile kernel-eligible TIR loops to NVPTX/AMDGPU via MLIR | M-GPU-3, mlir-sys evaluation | Planned |
| **M-GPU-5** | Async GPU integration: GpuFuture as first-class Molt future with GIL-release semantics | M-GPU-4, RT3 (async runtime) | Planned |

**Timeline**: No dates assigned. GPU milestones begin after TC2 + SL2 + TL2 are
complete. M-GPU-1 is the earliest actionable item and has no GPU hardware dependency.

## TODO Mirror Ledger (Auto-Generated)
<!-- BEGIN TODO MIRROR LEDGER -->
- TODO(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:missing): async generator lowering and runtime parity (`async def` with `yield`).
- TODO(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:missing): implement async generator lowering and runtime parity.)
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): async I/O cancellation propagation.)
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): task groups/wait/shield + I/O cancellation + hardening.)
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): task-based concurrency).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): wasm async iteration/scheduler parity.
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): executor integration).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): wasm scheduler background workers.)
- TODO(async-runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors and explicit message passing; shared-memory parallelism only via opt-in safe types.)
- TODO(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): Implement the remaining `libmolt` C-API v0 surface per `0214` and keep this matrix aligned with real coverage.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:done): landed minimal `libmolt` C-API bootstrap subset (buffer, numerics, sequence/mapping, errors, GIL mapping) as the primary C-extension compatibility foundation.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): expanded `libmolt` surface with scalar constructors/accessors, bytes-name attribute helpers, object compare/contains bool helpers, and array-to-container constructors (`tuple`/`list`/`dict` builders) plus focused conformance tests in `runtime/molt-runtime/src/c_api.rs`.
- Implemented(tooling, owner:tooling, milestone:SL3, priority:P1, status:partial): `molt publish` now hard-verifies extension wheels as extension metadata with checksum + capability gates before publish.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): added type/module parity wrappers (`molt_type_ready`, module create/add/get APIs including bytes-name and constant helpers), runtime-owned module metadata/state registries, callback-backed method wrappers (`molt_cfunction_create_bytes`, `molt_module_add_cfunction_bytes`), and CPython source-compat headers (`include/Python.h`, `include/molt/Python.h`) for partial `PyType`/`PyModule`/`PyErr`/`PySequence`/`PyMapping` APIs, including module creation/access helpers (`PyModule_New(Object)`, `PyModule_GetDef`, `PyModule_GetState`, `PyModule_SetDocString`, `PyModule_GetFilename(Object)`, `PyModule_FromDefAndSpec(2)`, `PyModule_ExecDef`, `PyState_*`) and expanded type parity (`PyType_FromSpec(WithBases)` selected slot lowering + `METH_CLASS`/`METH_STATIC`, plus `PyType_FromModuleAndSpec` and `PyType_GetModule`/`PyType_GetModuleState`/`PyType_GetModuleByDef`).
- Implemented(runtime/tooling, owner:runtime, milestone:SL3, priority:P1, status:partial): extension import spec/load boundaries now enforce manifest ABI/capability/checksum gates (finder + loader/exec lanes, including explicit module-mismatch rejection) with fingerprint-aware cache invalidation for replaced artifacts and cache-hit/miss telemetry assertions, and CI includes extension wheel `build + scan + audit + verify + publish --dry-run` matrix lanes (`linux native`, `linux cross-aarch64-gnu`, `linux cross-musl`, `macos native`, `windows native`) plus native runtime import smoke checks and a wasm-target rejection contract check on the `linux native` lane.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): `include/molt/Python.h` now includes O(n + k) `PyArg_ParseTupleAndKeywords` support (shared parser core with `PyArg_ParseTuple`) for `O,O!,b,B,h,H,i,I,l,k,L,K,n,c,d,f,p,s,s#,z,z#,y#` + `|`/`$` markers, kwlist-based keyword lookup, and duplicate positional/keyword detection.
- Implemented(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): scan-driven parity push expanded CPython source-compat wrappers/macros in `include/molt/Python.h` (type/ref helpers, tuple/list/dict helpers, memory allocators, GIL/thread shims, compare/call builders, `Py_BuildValue` subset, capsule/buffer helpers, module/capsule import helpers, `PyArg_UnpackTuple`, iter/number/float helpers, set/complex checks) and landed initial NumPy compatibility headers (`include/numpy/*` + `import_array*` capsule wiring + DType/scalar helper stubs) plus a minimal `datetime.h` shim lane (`PyDateTimeAPI`/`PyDateTime_IMPORT`/basic checks), dropping scan-reported missing `Py*` tokens on current NumPy/pandas sources (NumPy `2.4.2`, pandas `3.0.1`) from `1484/189` to `1193/28`.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): Expand `PyArg_ParseTuple`/`PyArg_ParseTupleAndKeywords` beyond the landed minimal format subset.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define `libmolt` C-extension ABI surface + bridge policy).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): extend the initial C API shim from bootstrap wrappers to broader source-compat and ABI coverage).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): Define the `Py_LIMITED_API` version Molt targets (3.10?).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): define hollow-symbol policy + error surface).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add per-pass wall-time telemetry (`attempted`/`accepted`/`rejected`/`degraded`, `ms_total`, `ms_p95`) plus top-offender diagnostics by module/function/pass (frontend pass telemetry, CLI/JSON sink wiring, and hotspot rendering are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add tiered optimization policy (Tier A entry/hot functions, Tier B normal user functions, Tier C heavy stdlib/dependency functions) with deterministic classification and override knobs (baseline classifier + env override knobs are landed; runtime-feedback and PGO hot-function promotion are now wired through the existing tier promotion path).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): enforce per-function mid-end wall-time budgets with an automatic degrade ladder that disables expensive transforms before correctness gates and records degrade reasons (budget/degrade ladder is landed in fixed-point loop; tuning heuristics and function-level diagnostics surfacing remain).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): ship profile-gated mid-end policy matrix (`dev` correctness-first cheap opts; `release` full fixed-point) with deterministic pass ordering and explicit diagnostics (profile plumbing into frontend policy is landed; diagnostics sink now also surfaces active midend policy config and heuristic knobs; remaining work is broader tuning closure and any additional triage UX).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:planned): method-binding safety pass (guard/deopt on method lookup + cache invalidation rules for call binding).
- TODO(compiler, owner:compiler, milestone:RT2, priority:P2, status:planned): canonical loop lowering).
- TODO(compiler, owner:compiler, milestone:RT2, priority:P2, status:planned): dict version tag guards).
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
- TODO(http-runtime, owner:runtime, milestone:SL3, priority:P1, status:missing): HTTP/ASGI runtime + DB driver parity.)
- TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:missing): native HTTP package).
- TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:missing): native WebSocket + streaming I/O).
- TODO(http-runtime, owner:runtime, milestone:SL3, priority:P2, status:planned): WebSocket host connect hook + capability registry).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root builds (namespace packages + PYTHONPATH roots supported; remaining: package discovery hardening, `__init__` edge cases, deterministic dependency graph caching).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): project-root builds (package discovery hardening, `__init__` edge handling, deterministic dependency graph caching).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): project-root builds (package discovery, `__init__` handling, namespace packages, deterministic dependency graph caching).
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P2, status:partial): full extension/sourceless execution parity beyond capability-gated restricted-source shim hooks.)
- TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.)
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame fields (f_back, f_globals, f_locals) and keep f_lasti/f_lineno updated.
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame objects to CPython parity (`f_globals`, `f_locals`, `f_lasti`, `f_lineno` updates).
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame objects to full CPython parity fields.)
- TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): complete closure/generator/coroutine-specific `co_flags` and free/cellvar parity.)
- TODO(introspection, owner:runtime, milestone:TC3, priority:P2, status:missing): full frame objects + `gi_code` parity.
- TODO(introspection, owner:runtime, milestone:TC3, priority:P2, status:missing): implement `gi_code` + full frame objects.)
- TODO(observability, owner:tooling, milestone:TL2, priority:P3, status:planned): Prometheus integration).
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): Django test-client coverage + retry policy). `molt_accel` ships as an optional dependency group (`pip install .[accel]`) with a packaged default exports manifest so the decorator can fall back to `molt-worker` in PATH when `MOLT_WORKER_CMD` is unset. A demo app scaffold lives in `demo/`.
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): compile entrypoints into molt_worker.
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): compiled handler coverage beyond demo exports.)
- TODO(offload, owner:runtime, milestone:SL1, priority:P2, status:planned): adapter/DB contract path).
- TODO(opcode-matrix, owner:frontend, milestone:M2, priority:P3, status:planned): Optimize `SETUP_WITH` to inline `__enter__` (Milestone 2).
- Implemented: `MATCH_*` semantics via AST desugaring (PEP 634 full coverage).
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:missing): awaitable `__aiter__` support). |
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): Add async generator op coverage (e.g., `ASYNC_GEN_WRAP`) and confirm lowering gaps.
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator coverage). |
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator op coverage). |
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:planned): expand KW_NAMES error-path coverage (duplicate keywords, positional-only violations) in differential tests.
- TODO(packaging, owner:tooling, milestone:SL2, priority:P2, status:partial): default wire codecs to MsgPack/CBOR).
- TODO(perf, owner:compiler, milestone:RT2, priority:P1, status:planned): wasm `simd128` kernels for string scans.
- TODO(perf, owner:compiler, milestone:RT2, priority:P2, status:planned): simd128 short-needle kernels).
- TODO(perf, owner:compiler, milestone:RT2, priority:P2, status:planned): vectorizable region detection).
- TODO(perf, owner:compiler, milestone:TC2, priority:P1, status:planned): implement PEP 709-style comprehension inlining for list/set/dict comprehensions (beyond the simple range fast path), and gate rollout with pyperformance `comprehensions` + targeted differential comprehension tranche benchmarks.
- TODO(perf, owner:compiler, milestone:TC2, priority:P1, status:planned): tighten async spill/restore to a CFG-based liveness pass to reduce closure traffic and shrink state_label reload sets.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:partial): SIMD kernels for reductions + scans).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): float + int mix kernels).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): implement
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): reduce handle-resolution overhead beyond the sharded registry and measure lock-sensitive benchmark deltas (attr access, container ops).
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): reduce handle/registry lock scope and measure lock-sensitive benchmarks).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:partial): bytes/bytearray fast paths).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): 32-bit partials + overflow guards for `prod`).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): biased RC).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): cache type comparison dispatch on type objects).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): cached UTF-8 index tables for repeated non-ASCII `find`/`count`.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): implement a native Windows socketpair using WSAPROTOCOL_INFO or AF_UNIX to avoid loopback TCP overhead.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): pre-size `dict.fromkeys` to reduce rehashing.)
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): profiling-driven vectorization).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): safe NEON multiply strategy).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): stream print writes to avoid large intermediate allocations.)
- TODO(perf, owner:runtime, milestone:RT3, priority:P3, status:planned): AVX-512 or 32-bit specialization for vectorized `prod` reductions.
- TODO(perf, owner:runtime, milestone:RT3, priority:P3, status:planned): AVX-512 reductions).
- TODO(perf, owner:runtime, milestone:TC1, priority:P2, status:planned): avoid list_snapshot allocations in membership/count/index by using a list mutation version or iterator guard.)
- TODO(perf, owner:tooling, milestone:TL2, priority:P1, status:partial): finish friend suite adapters/pinned command lanes and run nightly scorecards in CI.)
- TODO(perf, owner:tooling, milestone:TL2, priority:P1, status:partial): finish friend-owned suite adapters (Codon/PyPy/Nuitka/Pyodide), pin immutable suite refs/commands, and enable nightly friend scorecard publication.
- TODO(perf, owner:tooling, milestone:TL2, priority:P2, status:planned): benchmarking regression gates).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): thread PyToken through runtime mutation entrypoints).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define per-runtime GIL strategy and runtime instance ownership model).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define the per-runtime GIL strategy, runtime instance ownership model, and the allowed cross-thread object sharing rules.
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.)
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): fork/forkserver currently map to spawn semantics; implement true fork support.
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): implement true fork support). |
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): OPT-0003 phase 1 landed (sharded pointer registry); benchmark and evaluate lock-free alternatives next (see [OPTIMIZATIONS_PLAN.md](../../OPTIMIZATIONS_PLAN.md)).
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): benchmark sharded registry
- TODO(runtime-provenance, owner:runtime, milestone:RT2, priority:P2, status:planned): audit remaining pointer
- TODO(security, owner:runtime, milestone:RT2, priority:P1, status:missing): memory/CPU quota enforcement for native binaries).
- TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.
- TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor `__new__` overrides for non-exception classes.)
- TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): lower builtin arity checks to runtime `TypeError` instead of compile-time rejection.)
- TODO(semantics, owner:runtime, milestone:LF1, priority:P1, status:partial): exception objects + last-exception plumbing. |
- TODO(semantics, owner:runtime, milestone:LF1, priority:P1, status:partial): exception propagation + suppression semantics for context manager exit paths.
- TODO(semantics, owner:runtime, milestone:TC1, priority:P0, status:planned): audit negative-indexing parity across indexable types + add differential coverage for error messages.
- TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): exception `__init__` + subclass attribute parity (ExceptionGroup tree).)
- TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): exception `__init__` + subclass attribute parity (UnicodeError fields, ExceptionGroup tree).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): Formalize "Lazy Task" divergence policy.
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence). |
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): Implement cycle collector (currently pure RC).
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector). |
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): implement cycle collector.)
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): incremental mark-and-sweep GC).
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:partial): finalizer guarantees). |
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:partial): signal handling parity). |
- TODO(stdlib-compat, owner:frontend, milestone:SL1, priority:P2, status:planned): decorator whitelist + compile-time lowering for `@lru_cache`.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:missing): runtime deque type.)
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:partial): finish `open`/file object parity (broader codecs + full error handlers, text-mode seek/tell cookies, Windows fileno/isatty) with differential + wasm coverage.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P1, status:partial): finish file/open parity per ROADMAP checklist + tests, with native/wasm lockstep.)
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): `array` + `struct` deterministic layouts and packing (struct intrinsics cover the CPython 3.12 format table with alignment + half-float support, and C-contiguous nested-memoryview buffer windows; remaining struct gap is exact CPython diagnostic-text parity on selected edge cases).
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): decode argv via filesystem encoding + surrogateescape once Molt strings can represent surrogate escapes.)
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): `array` deterministic layout + buffer protocol.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:planned): array runtime layout + buffer protocol).
- TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P2, status:planned): `hashlib` deterministic hashing policy.
- Policy lock (dynamic execution): compiled binaries intentionally stay on restricted-source import/runpy execution lanes; unrestricted code-object execution is deferred by policy, not an active burndown target (see `docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md`).
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
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): harden backend daemon lane (multi-job compile API, bounded request/job guardrails, richer health telemetry, deterministic readiness/restart semantics, and config-digest lane separation with cache reset-on-change are landed; remaining work is sustained high-contention soak evidence + restart/backoff tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add batch compile server mode for diff runs to amortize backend startup and reduce per-test compile overhead (in-process JSON-line batch server, hard request deadlines, force-close shutdown, cooldown-based retry hardening, and fail-open/strict modes are landed behind env gates; remaining work is default-on rollout criteria + perf guard thresholds).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add function-level object caching so unchanged functions can be relinked without recompiling whole scripts (function cache-key lane now includes backend codegen-env digest + IR top-level extras digest, module/function cache-tier fallback + daemon function-cache plumbing are landed, and invalid cached-artifact guard + daemon cache-tier telemetry are wired; remaining work is import-graph-aware scheduling + fleet-level perf tuning).
- runpy supported lane update: `runpy_run_module_basic.py` now executes on the intrinsic-backed restricted path with corrected dotted-package `__main__` import resolution.
- runpy supported lane update: `runpy_run_path_basic.py` now executes on the intrinsic-backed restricted path via constrained reference-assignment RHS support (`name`, `.attr`, `[int|str]`).
- runpy supported lane update: `runpy_run_module_alter_sys_intrinsic.py` now executes on the intrinsic-backed restricted source lane with `alter_sys=True` argv0/module-swap parity.
- runpy dynamic-lane expected failures are currently empty because supported lanes moved to intrinsic support.
- stdlib parity update: signal mask constants and NSIG now lower through Rust intrinsics (`molt_signal_sig_block`,
  `molt_signal_sig_unblock`, `molt_signal_sig_setmask`, `molt_signal_nsig`) with CPython-shaped `signal`/`_signal` wrapper wiring.
- stdlib parity update: subprocess public sentinels now come from intrinsic constants (`molt_subprocess_pipe_const`,
  `molt_subprocess_stdout_const`, `molt_subprocess_devnull_const`) while runtime spawn modes remain internal mapping details.
- stdlib parity update: `_asyncio.current_task()` now raises outside a running loop, `shutil.rmtree` is intrinsic-backed, and
  `_thread` lock objects report CPython-compatible `lock` type naming in differential lanes.
- Future reconsideration requires explicit capability gating, documented utility analysis, reproducible perf/memory evidence, and explicit user approval before implementation.
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
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures`.)
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/surrogatepass/namereplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:runtime, milestone:TC2, priority:P2, status:missing): `str(bytes, encoding, errors)` decoding parity for bytes-like inputs.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): extend XML-RPC coverage to support full marshalling/fault handling and introspection APIs with Rust-backed parsing/serialization.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): extend `zipapp` coverage to full CPython semantics (interpreter shebangs, custom entry-points, and in-memory target handling) via Rust intrinsics.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): extend queue-backed logging handler parity for advanced listener lifecycle and queue edge cases after baseline stdlib queue support stabilizes.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): replace the minimal built-in timezone table with a full IANA tzdb-backed ZoneInfo implementation in Rust intrinsics.
- TODO(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): runtime backlog.\n",
- TODO(stdlib-compat, owner:stdlib, milestone:LF1, priority:P1, status:missing): `contextlib.contextmanager` lowering and generator-based manager support.
- TODO(stdlib-compat, owner:stdlib, milestone:LF3, priority:P2, status:planned): expand `io`/`pathlib` to buffered + streaming wrappers with capability gates.
- TODO(stdlib-compat, owner:stdlib, milestone:LF3, priority:P2, status:planned): io/pathlib stubs + capability enforcement. |
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): migrate all Python stdlib modules to Rust intrinsics-only implementations (Python files may only be thin intrinsic-forwarding wrappers); compiled binaries must reject Python-only stdlib modules. See [docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python stdlib modules with Rust intrinsics-only implementations (thin wrappers only); compiled binaries must reject Python-only stdlib modules. See `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.",
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:partial): test fixture partial marker.\n"
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:missing): full `open`/file object parity (modes/buffering/text/encoding/newline/fileno/seek/tell/iter/context manager) with differential + wasm coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` intrinsics + float determinism policy (non-transcendentals covered; trig/log/exp parity pending).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` shim covers constants, predicates, `trunc`/`floor`/`ceil`, `fabs`/`copysign`/`fmod`/`modf`, `frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`, and `sqrt`; Rust intrinsics cover predicates (`isfinite`/`isinf`/`isnan`), `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc`; remaining: determinism policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `struct` alignment + full format table parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill out remaining `math` intrinsics (determinism policy).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): implement full struct format/alignment parity.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): operator intrinsics + runtime deque + `re`/`datetime` parity.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): remove `typing` fallback ABC scaffolding and lower protocol/ABC bootstrap helpers into Rust intrinsics-only paths.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` (`deque`, `Counter`, `defaultdict`) parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `collections` runtime `deque` type + O(1) ops + view parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `functools` fast paths (`lru_cache`, `partial`, `reduce`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:planned): `itertools` + `operator` core-adjacent intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): `struct` intrinsics cover the CPython 3.12 format table (including half-float) with endianness + alignment and C-contiguous memoryview chain handling for pack/unpack/pack_into/unpack_from; remaining gaps are exact CPython diagnostic-text parity on selected edge cases.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): align remaining struct edge-case error text with CPython.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): make these iterators lazy and streaming).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `bisect` helpers + fast paths.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:planned): `heapq` randomized stress + perf tracking.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P0, status:partial): complete concurrency substrate lowering in strict order (`socket`/`select`/`selectors` -> `threading` -> `asyncio`) with intrinsic-only compiled semantics in native + wasm.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module API + runtime cycle collector hook.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module exposes only minimal toggles/collect; wire to runtime cycle collector and implement full API.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (runtime fast-path parser + performance tuning).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): advance native `re` engine to full syntax/flags/groups; native engine covers core syntax (literals, `.`, classes/ranges, groups/alternation, greedy + non-greedy quantifiers) and `IGNORECASE`/`MULTILINE`/`DOTALL`; advanced features/flags raise `NotImplementedError` (no host fallback).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): advance native `re` engine to full syntax/flags/groups; native engine supports literals, `.`, char classes/ranges (`\\d`/`\\w`/`\\s`), groups/alternation, greedy + non-greedy quantifiers, and `IGNORECASE`/`MULTILINE`/`DOTALL` flags. Matcher hot paths for literal/any/char-class advancement, char/range/category checks, anchors, backreference/group-presence resolution, scoped-flag math, group capture/value materialization, and replacement expansion are intrinsic-backed; remaining advanced features/flags still raise `NotImplementedError` (no host fallback).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): close
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Decimal arithmetic + formatting
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Python 3.12+ statistics API/PEP parity beyond function surface lowering (for example NormalDist and remaining edge-case text parity).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` ancillary-data parity (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`) while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty control messages.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` cross-platform ancillary parity (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`), while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty ancillary control messages.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete asyncio transport/runtime parity after intrinsic capability gates (full SSL transport semantics, Unix-socket behavior parity across native/wasm, and child-watcher behavior depth on supported hosts).
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `random` distributions + extended test vectors.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining pathlib glob edge parity (`root_dir`/hidden semantics, full Windows flavor/symlink nuances) and full Path parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining pickle CPython 3.12+ parity gaps before intrinsic-backed promotion.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete Decimal API parity (arithmetic ops, exp/log/pow/sqrt, context quantize/signals edge cases, NaN payloads, and formatting helpers).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete `pickle` CPython 3.12+ parity (remaining reducer/object-hook edges, full Pickler/Unpickler class-surface/error-text parity, and exhaustive protocol-5 buffer/graph corner semantics beyond current class/dataclass/reducer lanes).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete full `statistics` 3.12+ API/PEP parity beyond intrinsic-lowered function surface (for example `NormalDist` and remaining edge-case semantics).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete full statistics 3.12+ API/PEP parity beyond function surface lowering.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): deterministic `time` clock policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): deterministic clock policy) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `random` distribution test vectors and edge-case coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand random distribution test vectors) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): finish Enum/Flag/IntFlag parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): support dataclass inheritance from non-dataclass bases without breaking layout guarantees.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `datetime` + `zoneinfo` time handling policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `json` parity plan (runtime fast-path + performance tuning + full cls/callback parity).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:planned): `re` engine + deterministic regex semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task APIs + task groups + I/O adapters + executor semantics.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): complete socket/select/selectors parity (OS-specific flags, fd inheritance, error mapping, cancellation) and align with asyncio adapters.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): extend intrinsic-backed `queue` support beyond `Queue`/`SimpleQueue` core semantics to full parity (`LifoQueue`, `PriorityQueue`, richer API/edge-case parity) and align dependent `logging.handlers` coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): finish remaining PEP 695 metadata work (alias metadata/TypeAliasType) and broaden type-parameter default coverage beyond current TypeVar path where required.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): move csv parser/writer hot paths to dedicated Rust intrinsics while preserving CPython parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `_asyncio` shim now uses intrinsic-backed running-loop hooks; broader C-accelerated parity remains pending.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `_bz2` compression backend parity for `bz2`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (full encodings import hooks + charmap codec intrinsics); incremental encoder/decoder now backed by Rust handle-based intrinsics, BOM constants from Rust, register_error/lookup_error wired.
- Implemented: `tempfile` now uses CPython-style candidate temp-dir ordering, including Windows defaults (`~\\AppData\\Local\\Temp`, `%SYSTEMROOT%\\Temp`, `c:\\temp`, `c:\\tmp`, `\\temp`, `\\tmp`) and cwd fallback.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): asyncio pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close parity gaps for `ast`, `ctypes`, and `urllib.parse`/`urllib.error`/`urllib.request` per matrix coverage.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close remaining socketserver class/lifecycle parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.client connection/chunked/proxy parity on top of intrinsic execute/response core.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.cookies quoting/attribute/parser parity beyond intrinsic-backed subset.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete http.server parser/handler lifecycle parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete queue edge-case/API parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete socket/select/selectors parity after intrinsic-backed object lowering (`poll`/`epoll`/`kqueue`/`devpoll` + backend selector classes); remaining work is OS-flag/error fidelity, fd inheritance corners, and wasm/browser host parity.
- Implemented: when `env.read` is denied, `tempfile` temp-dir selection no longer hard-fails and deterministically falls back to OS/cwd candidates.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand ctypes intrinsic coverage beyond the core scalar/structure/array/pointer subset.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand `asyncio` shim to full loop/task APIs (task groups, wait, shields) and I/O adapters.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish remaining `types` shims (CapsuleType + any missing helper/descriptor types).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish urllib.request handler/response/network parity on top of intrinsic opener core.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): full importlib native extension and pyc execution parity beyond capability-gated restricted-source shim lanes.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full _asyncio C-accelerated surface on top of runtime intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement `_asyncio` parity or runtime hooks.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement `_bz2` compression/decompression parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement core collections.abc surfaces.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full metadata version semantics and remaining entry point selection edge cases.
- Implemented: `tempfile` temp-dir selection now probes candidate usability with secure create/write/unlink checks and raises `FileNotFoundError` when no candidate is writable.
- Implemented: `codecs` incremental encoder/decoder now backed by Rust handle-based intrinsics (new/encode/decode/reset/drop); BOM constants from Rust; register_error/lookup_error wired to Rust error-handler registry. Remaining: full encodings import hooks + charmap codec intrinsics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib extension/sourceless execution parity beyond capability-gated restricted-source shim lanes.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib extension/sourceless execution parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery full extension/sourceless execution parity beyond capability-gated restricted-source shim lanes (zip source loader path is intrinsic-lowered).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery full native extension/pyc execution parity beyond restricted source shim lanes) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata dependency/advanced metadata semantics beyond intrinsic payload parsing.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata full parsing + dependency/entry point semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.util non-source loader execution parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): tempfile parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + coverage smoke tests.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): asyncio submodule parity (events/tasks/streams/etc) beyond import-only allowlisting.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): asyncio submodule parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O (`io`, `os`, `sys`, `pathlib`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): capability-gated I/O parity.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): import-only allowlisted stdlib modules (`argparse`, `ast`, `collections.abc`, `_collections_abc`, `_abc`, `_asyncio`, `_bz2`, `_weakref`, `_weakrefset`, `platform`, `time`, `tomllib`, `warnings`, `traceback`, `types`, `inspect`, `copy`, `copyreg`, `string`, `numbers`, `unicodedata`, `tempfile`, `ctypes`) to minimal parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): network/process gating (`socket`, `ssl`, `subprocess`, `asyncio`).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): ast parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): cgi 3.12-path parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close `_abc` edge-case cache/version parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close `_abc` edge-case cache/version parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining abc edge-case parity around subclasshook/cache invalidation.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining non-UTF8 bytes/traversal-order edge parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining shlex parser/state parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining string parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining textwrap edge-case/module-surface parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.error/request integration parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.parse parity gaps.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): close remaining urllib.response file-wrapper and integration edge parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): compileall/py_compile parity (pyc output, invalidation modes, optimize levels).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): complete `fnmatch` bytes/normcase/cache parity on top of intrinsic-backed `molt_fnmatch*` runtime lane.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): complete `glob` parity (`root_dir`, `recursive`/`**` edge semantics, `include_hidden`) on top of intrinsic-backed `molt_glob`/`molt_glob_has_magic`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): complete `shlex` parser/state parity (`sourcehook`, `wordchars`, incremental stream semantics) on top of intrinsic-backed lexer/join lane.
- Note (doctest dynamic execution policy): doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand ctypes surface + data model parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): finish abc registry + cache invalidation parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement full gettext translation catalog/domain parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement gettext translation catalog/domain parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement tarfile parity.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement zipimporter bytecode/cache parity + broader archive support.) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): inspect pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): pkgutil loader/zipimport/iter_importers parity (filesystem-only discovery + store/deflate+zip64 zipimport today).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): pkgutil loader/zipimport/iter_importers parity (filesystem-only iter_modules/walk_packages today).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): test package pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): traceback pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): types pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs exist for regrtest (support: captured_output/captured_stdout/captured_stderr, check_syntax_error, findfile, run_with_tz, warnings_helper utilities: check_warnings/check_no_warnings/check_no_resource_warning/check_syntax_warning/ignore_warnings/import_deprecated/save_restore_warnings_filters/WarningsRecorder, cpython_only, requires, swap_attr/swap_item, import_helper basics: import_module/import_fresh_module/make_legacy_pyc/ready_to_import/frozen_modules/multi_interp_extensions_check/DirsOnSysPath/isolated_modules/modules_setup/modules_cleanup, os_helper basics: temp_dir/temp_cwd/unlink/rmtree/rmdir/make_bad_fd/can_symlink/skip_unless_symlink + TESTFN constants); doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs for regrtest (support: captured_output/captured_stdout/captured_stderr, check_syntax_error, findfile, run_with_tz, warnings_helper utilities: check_warnings/check_no_warnings/check_no_resource_warning/check_syntax_warning/ignore_warnings/import_deprecated/save_restore_warnings_filters/WarningsRecorder, cpython_only, requires, swap_attr/swap_item, import_helper basics: import_module/import_fresh_module/make_legacy_pyc/ready_to_import/frozen_modules/multi_interp_extensions_check/DirsOnSysPath/isolated_modules/modules_setup/modules_cleanup, os_helper basics: temp_dir/temp_cwd/unlink/rmtree/rmdir/make_bad_fd/can_symlink/skip_unless_symlink + TESTFN constants); doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): warnings pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): argparse pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): binascii pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.header pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.message pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.parser pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.policy pending parity) |
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:planned): email.utils pending parity) |
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
- DONE(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:done): `_blake2` now exposes intrinsic-backed `blake2b`/`blake2s` constructors and CPython-compatible BLAKE2 constants via Molt `hashlib`.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_colorize` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_compat_pickle` now ships the canonical CPython compatibility mapping tables in-repo for private-module imports without host dependencies.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_compression` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_contextvars` now re-exports Molt's intrinsic-backed `contextvars` surface with CPython-shaped private-module names (`Context`, `ContextVar`, `Token`, `copy_context`).
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_crypt` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_csv` now re-exports Molt's intrinsic-backed csv surface with CPython-shaped private-module names (`Dialect`, `Error`, `reader`, `writer`, dialect registry helpers, and quote constants).
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_ctypes` now re-exports Molt's intrinsic-backed scalar/structure/pointer ctypes subset for CPython-compatible private-module imports.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_curses_panel` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_curses` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_datetime` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_dbm` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_decimal` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_elementtree` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_functools` now exposes an intrinsic-backed compatibility surface for `Placeholder`, `cmp_to_key`, `partial`, and `reduce`.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_gdbm` top-level stub with full intrinsic-backed lowering.
- DONE(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:done): `_hashlib` now exposes intrinsic-backed hash/HMAC compatibility helpers, OpenSSL-style constructor aliases, and key derivation entrypoints through Molt `hashlib`/`hmac`.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_heapq` now re-exports Molt's intrinsic-backed heap operations (`heapify`, `heappush`, `heappop`, `heapreplace`, `heappushpop`) for CPython-compatible private-module imports.
- DONE(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:done): `_hmac` now exposes intrinsic-backed HMAC helpers, one-shot digest entrypoints, and CPython-compatible unknown-hash errors via Molt `hmac`.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_imp` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_interpchannels` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_interpqueues` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_interpreters` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_io` now re-exports Molt's intrinsic-backed core file/stream classes and `open()` surface for CPython-compatible private-module imports.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_ios_support` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_json` now exposes an intrinsic-backed compatibility surface for `encode_basestring`, `encode_basestring_ascii`, `scanstring`, and callable `make_encoder`/`make_scanner` types.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_locale` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_lsprof` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_lzma` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_markupbase` top-level stub with full intrinsic-backed lowering.
- DONE(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:done): `_md5` now exposes an intrinsic-backed `md5` constructor and `MD5Type` via Molt `hashlib`.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_msi` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_osx_support` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_overlapped` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_posixshmem` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_py_warnings` now re-exports Molt's intrinsic-backed `warnings` surface with CPython-shaped public names.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pydatetime` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pydecimal` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyio` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pylong` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.__main__` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl._minimal_curses` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl._module_completer` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl._threading_handler` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.base_eventqueue` module stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_pyrepl.commands` module stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_random` now re-exports Molt's intrinsic-backed `random.Random` type.
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
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_random` now re-exports Molt's intrinsic-backed `random.Random` type.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_remote_debugging` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_scproxy` top-level stub with full intrinsic-backed lowering.
- DONE(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:done): `_sha1` now exposes an intrinsic-backed `sha1` constructor and `SHA1Type` via Molt `hashlib`.
- DONE(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:done): `_sha2` now exposes intrinsic-backed `sha224`/`sha256`/`sha384`/`sha512` constructors and CPython-style type aliases via Molt `hashlib`.
- DONE(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:done): `_sha3` now exposes intrinsic-backed SHA-3 and SHAKE constructors, `implementation`, and CPython-shaped module constants via Molt `hashlib`.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_signal` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sqlite3` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sre` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_ssl` now re-exports Molt's intrinsic-backed TLS context/socket surface for CPython-compatible private-module imports.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_stat` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_string` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_strptime` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_struct` now re-exports Molt's intrinsic-backed `struct` surface (`Struct`, pack/unpack helpers, and `error`).
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_suggestions` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_symtable` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_sysconfig` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_thread` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_tokenize` top-level stub with full intrinsic-backed lowering.
- TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `_tracemalloc` top-level stub with full intrinsic-backed lowering.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_types` now re-exports Molt's intrinsic-backed runtime type objects for CPython-compatible private-module imports.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_uuid` now exposes intrinsic-backed `generate_time_safe()` plus CPython-shaped capability constants over Molt's UUID runtime surface.
- Implemented(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): `_warnings` now re-exports Molt's intrinsic-backed warning filters and top-level warn helpers for CPython-compatible private-module imports.
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
- Implemented: `match`/`case` lowering via cell-based PEP 634 desugaring (24 differential test files).
- TODO(tests, owner:runtime, milestone:SL1, priority:P1, status:partial): expand codec parity coverage for
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): add security-focused differential tests for attribute access edge cases (descriptor exceptions, `__getattr__` recursion traps).
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): expand exception differential coverage.
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): add wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
- TODO(tooling, owner:release, milestone:TL2, priority:P2, status:partial): enforce signature verification/trust policy during load.)
- TODO(tooling, owner:runtime, milestone:TL2, priority:P1, status:partial): collapse dual `fallible-iterator` versions once postgres stack releases support `fallible-iterator 0.3+`; keep the boundary isolated/documented until upstream unblocks.
- TODO(tooling, owner:runtime, milestone:TL2, priority:P1, status:partial): remove the temporary dual `fallible-iterator` graph when postgres ecosystem crates support `0.3+`; until then, keep 0.2 usage isolated to postgres-boundary code paths and document the constraint in status/review notes.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P1, status:partial): implement `molt extension build` with `libmolt` headers + ABI tagging (cross-target target-triple wiring + CI dry-run matrix lanes landed for `linux native`, `linux cross-aarch64-gnu`, `linux cross-musl`, `macos native`, and `windows native`; broader linker/sysroot hardening pending).
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:partial): implement `molt extension audit` and wire into `molt verify` (audit CLI + verify integration + runtime load-boundary metadata enforcement landed; richer policy diagnostics pending).
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): define canonical wheel tags for `libmolt` extensions.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): extension rebuild pipeline (headers, build helpers, audit tooling) for `libmolt`-compiled wheels.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add process-level parallel frontend module-lowering and deterministic merge ordering, then extend to large-function optimization workers where dependency-safe (dependency-layer process-pool lowering is landed behind `MOLT_FRONTEND_PARALLEL_MODULES`; remaining work is broader eligibility + worker telemetry/perf tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): harden backend daemon lane (multi-job compile API, bounded request/job guardrails, richer health telemetry, deterministic readiness/restart semantics, and config-digest lane separation with cache reset-on-change are landed; remaining work is sustained high-contention soak evidence + restart/backoff tuning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): surface active optimization profile/tier policy and degrade events in CLI build diagnostics and JSON outputs for deterministic triage (diagnostics sink is landed for policy/tier/degrade + pass hotspots, and stderr verbosity partitioning is landed; remaining work is richer CLI UX controls beyond verbosity).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add batch compile server mode for diff runs to amortize backend startup and reduce per-test compile overhead (in-process JSON-line batch server, hard request deadlines, force-close shutdown, cooldown-based retry hardening, and fail-open/strict modes are landed behind env gates; remaining work is default-on rollout criteria + perf guard thresholds).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add function-level object caching so unchanged functions can be relinked without recompiling whole scripts (function cache-key lane now includes backend codegen-env digest + IR top-level extras digest, module/function cache-tier fallback + daemon function-cache plumbing are landed, and invalid cached-artifact guard + daemon cache-tier telemetry are wired; remaining work is import-graph-aware scheduling + fleet-level perf tuning).
- Implemented: `molt doctor` now reports optimization-path diagnostics for `sccache`, backend daemon enablement, cargo/cache routing (`CARGO_TARGET_DIR`, `MOLT_CACHE`, `MOLT_DIFF_CARGO_TARGET_DIR`), and external-volume routing recommendations.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): CI perf artifacts + release uploads)
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add distributed cache guidance/tooling for multi-host agent fleets (remote `sccache` backend and validation playbooks).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add import-graph-aware diff scheduling and distributed cache playbooks for multi-host agent fleets.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add import-graph-aware diff scheduling to maximize cache locality and reduce redundant rebuild pressure.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): broaden deopt taxonomy + profile-consumption loop).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): lockfile-missing policy decision).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): runtime profiling hints in TFA).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): wire cross-target builds into CLI.)
- TODO(type-coverage, owner:compiler, milestone:TC2, priority:P2, status:planned): generator/iterator state in wasm ABI.
- TODO(type-coverage, owner:compiler, milestone:TC2, priority:P2, status:planned): wasm ABI for generator state. |
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): `try/except/finally` lowering + raise paths.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): builtin constructors for `tuple`, `dict`, `bytes`, `bytearray`.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): builtin reductions (`sum/min/max`) and `len` parity.
- TODO(type-coverage, owner:frontend, milestone:TC1, priority:P1, status:partial): type-hint specialization policy (`--type-hints=check` with runtime guards).
- TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:missing): complex literal lowering + runtime support.
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
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): consider `__matmul__`/`__rmatmul__` fallback for custom types.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): rounding intrinsics (`floor`, `ceil`) + full deterministic semantics for edge cases.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): formatting builtins (`repr`, `ascii`, `bin`, `hex`, `oct`, `chr`, `ord`) + full `format` protocol (named fields, format specs, conversion flags).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): generator state objects + StopIteration.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): identity builtins (`hash`, `id`, `callable`).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): rounding intrinsics (`round`, `floor`, `ceil`, `trunc`) with deterministic semantics.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:planned): set/frozenset hashing + deterministic ordering.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): memoryview multi-dimensional slicing + sub-views (retain C-order semantics + parity errors).
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): metaclass behavior for descriptor hooks.)
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): metaclass execution). |
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): multi-dimensional slicing/sub-views.)
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): decimal + `int` method parity.)
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): buffer protocol + memoryview layout.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:planned): descriptor builtins (`property`, `classmethod`, `staticmethod`, `super`).
- TODO(type-coverage, owner:stdlib, milestone:TC2, priority:P2, status:planned): `builtins` module parity notes.
- TODO(type-coverage, owner:stdlib, milestone:TC2, priority:P3, status:planned): `builtins` module parity notes.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:partial): dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Note (dynamic execution policy): regrtest `test_future_stmt` still depends on full `compile` support and remains out of active burndown while dynamic execution stays policy-deferred.
- Note (dynamic execution policy): dynamic execution (`eval`/`exec`/`compile`) is policy-deferred; current scope is parser-backed `compile` validation only (`exec`/`eval`/`single` to a runtime code object), while `eval`/`exec` execution and full compile codegen remain intentionally unsupported; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Note (reflection policy): unrestricted reflection (`dir`/`vars`/`globals`/`locals`) is policy-deferred for compiled binaries; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- Note (runtime monkeypatch policy): runtime monkeypatching of modules, types, or functions is policy-deferred for compiled binaries; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): I/O builtins (`open`, `input`, `help`, `breakpoint`) with capability gating.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): import/module rules + module object model (`__import__`, package resolution, `sys.path` policy).
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): module object + import rules. |
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:planned): reflection builtins (`type`, `isinstance`, `issubclass`, `getattr`, `setattr`, `hasattr`, `dir`, `vars`, `globals`, `locals`).
- TODO(type-coverage, owner:tests, milestone:TC1, priority:P1, status:planned): add exception + set coverage to molt_diff.
- TODO(type-coverage, owner:tests, milestone:TC2, priority:P2, status:partial): execute matrix end-to-end).
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB connector parity with real backend coverage (browser host tests cover cancellation + Arrow IPC bytes).)
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity with real backend coverage.)
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity with real backends + coverage).
- TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P2, status:planned): ship additional production host adapters (CF Workers) and wasm parity tests that exercise real DB backends with cancellation.
- TODO(wasm-host, owner:runtime, milestone:RT3, priority:P2, status:partial): add browser host I/O bindings + capability plumbing for storage and parity tests.)
- TODO(wasm-host, owner:runtime, milestone:RT3, priority:P3, status:planned): component model target support).
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): Node/V8 Zone OOM can still reproduce on some linked runtime-heavy modules in unrestricted/manual Node runs; parity and benchmark runners now enforce `--no-warnings --no-wasm-tier-up --no-wasm-dynamic-tiering --wasm-num-compilation-tasks=1` while root-causing host/runtime interaction.
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): expand browser socket coverage (UDP/listen/server sockets) + add more parity tests.)
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): capability-enabled runtime-heavy wasm tranche (`<artifact-root>/wasm_runtime_heavy_tranche_20260213c/summary.json`) is still blocked (`1/5` pass): `asyncio__asyncio_running_loop_intrinsic.py` event-loop-policy parity mismatch, `asyncio_task_basic.py` table-ref trap in linked wasm runtime, `zipimport_basic.py` zipimport module-lookup parity gap, and `smtplib_basic.py` thread-unavailable wasm limitation. Keep this as a blocker before promoting runtime-heavy cluster completion.
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale on wasm hosts). (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): deterministic clock policy) |
- TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P2, status:planned): zero-copy string passing for WASM).
- TODO(wasm-parity, owner:stdlib, milestone:SL2, priority:P0, status:partial): runtime-heavy wasm server lanes that depend on `threading` remain blocked (threads are unavailable in wasm); keep these as promotion blockers for `smtplib`/socketserver-style workloads until a supported wasm threading strategy is finalized.
<!-- END TODO MIRROR LEDGER -->
