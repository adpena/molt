# STATUS (Canonical)

Last updated: 2026-02-13

This document is the source of truth for Molt's current capabilities and
limitations. Update this file whenever behavior or scope changes, and keep
README and [ROADMAP.md](../../ROADMAP.md) in sync.

## Strategic Target
- Performance: reach parity with or exceed Codon on representative native and
  wasm-relevant workloads.
- Coverage/interoperability: approach Nuitka-level CPython surface coverage and
  ecosystem interoperability, while preserving Molt vision constraints
  (determinism, explicit capabilities, and no implicit host-Python fallback).

## Optimization Program Status (2026-02-12)
- Program state: Week 1 observability is complete and Week 0 baseline-lock artifacts are captured.
- Execution assumption: optimization execution is active; Week 2 specialization and wasm-stabilization clusters are unblocked.
- Canonical optimization scope: [OPTIMIZATIONS_PLAN.md](../../OPTIMIZATIONS_PLAN.md).
- Canonical optimization execution log: [docs/benchmarks/optimization_progress.md](docs/benchmarks/optimization_progress.md).
- Current progress: runtime instrumentation + benchmark diff tooling are landed, and baseline lock summary is published at [bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md](bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md).
- Active risk signal (2026-02-12): frontend/mid-end compile throughput regressed on stdlib-heavy module graphs; deterministic wasm benchmark builds can timeout before runtime execution. The dedicated compile-time recovery tranche (profile gating + tiering + budgets + per-pass telemetry + deterministic parallel rollout) is now partially implemented in frontend/CLI, including diagnostics sink integration and opt-in process-level parallel lowering, and is tracked in [OPTIMIZATIONS_PLAN.md](../../OPTIMIZATIONS_PLAN.md).

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
- Implemented: Rust 2024 `unsafe_op_in_unsafe_fn` hardening in `runtime/molt-runtime/src/async_rt/channels.rs` (explicit unsafe blocks + safety rationale comments).
- Implemented: Rust 2024 hardening follow-up in `runtime/molt-runtime/src/async_rt/generators.rs`; remaining `unsafe_op_in_unsafe_fn` hits are cleared for that module.
- Implemented: pickle parity tranche advanced in runtime core (`runtime/molt-runtime/src/builtins/functions.rs`) with reducer 6-tuple `state_setter` lowering plus VM `POP`/`POP_MARK` support; targeted native differential tranche is green (`10/10`) including new regressions `tests/differential/stdlib/pickle_reduce_state_setter.py` and `tests/differential/stdlib/pickle_main_function_global_resolution.py`.
- Active blocker: capability-enabled runtime-heavy wasm tranche (`/Volumes/APDataStore/Molt/wasm_runtime_heavy_tranche_20260213c/summary.json`) is `1/5` green (`zipfile_roundtrip_basic.py` pass) with blockers in asyncio running-loop policy parity, linked wasm table-ref trap (`asyncio_task_basic.py`), zipimport module lookup parity, and wasm-thread limitation for `smtplib`.
- Implemented: postgres-boundary isolation for `fallible-iterator 0.2` via explicit alias dependency `fallible-iterator-02` in `runtime/molt-worker`; `0.3` remains on rusqlite paths.
- Temporary upstream exception: `fallible-iterator` remains dual-version in the graph because `tokio-postgres`/`postgres-protocol` currently pin `0.2` while `rusqlite` pins `0.3`.
- TODO(toolchain, owner:runtime, milestone:TL2, priority:P1, status:partial): collapse dual `fallible-iterator` versions once postgres stack releases support `fallible-iterator 0.3+`; keep the boundary isolated/documented until upstream unblocks.

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
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): ship profile-gated mid-end policy matrix (`dev` correctness-first cheap opts; `release` full fixed-point) with deterministic pass ordering and explicit diagnostics (CLI->frontend profile plumbing is landed; diagnostics sink expansion remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add tiered optimization policy (Tier A entry/hot functions, Tier B normal user functions, Tier C heavy stdlib/dependency functions) with deterministic classification and override knobs (baseline deterministic classifier + env overrides are landed; telemetry-driven hotness promotion remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): enforce per-function mid-end wall-time budgets with an automatic degrade ladder that disables expensive transforms before correctness gates and records degrade reasons (budget/degrade ladder is landed in fixed-point loop; heuristic tuning + diagnostics surfacing remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add per-pass wall-time telemetry (`attempted`/`accepted`/`rejected`/`degraded`, `ms_total`, `ms_p95`) plus top-offender diagnostics by module/function/pass (frontend per-pass timing/counters + hotspot rendering are landed; CLI/JSON sink wiring remains).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): surface active optimization profile/tier policy and degrade events in CLI build diagnostics and JSON outputs for deterministic triage (diagnostics sink now includes profile/tier/degrade summaries + pass hotspots; remaining work is richer UX controls/verbosity partitioning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add process-level parallel frontend module-lowering and deterministic merge ordering, then extend to large-function optimization workers where dependency-safe (dependency-layer process-pool lowering is landed behind `MOLT_FRONTEND_PARALLEL_MODULES`; remaining work is broader eligibility and worker-level tuning telemetry).
- TODO(compiler, owner:compiler, milestone:LF3, priority:P1, status:planned): migrate hot mid-end kernels (CFG build, SCCP lattice transfer, dominator/liveness) to Rust with Python orchestration preserved for policy control.
- Implemented: CI hardening for `tools/check_molt_ir_ops.py` now includes mandatory `--require-probe-execution` after `diff-basic`, so required-probe execution status and failure-queue linkage regressions fail CI.

## Capabilities (Current)
- Active stdlib lowering execution plan:
  [docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md](docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md).
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
- memoryview exposes `format`/`shape`/`strides`/`nbytes`, `cast`, tuple scalar indexing, and 1D slicing/assignment for bytes/bytearray-backed views.
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
- `str.lower`/`str.upper`/`str.capitalize`, list methods (`append`/`extend`/`insert`/`remove`/`pop`/`count`/`index` with start/stop + parity errors, `clear`/`copy`/`reverse`/`sort`),
  and `dict.clear`/`dict.copy`/`dict.popitem`/`dict.setdefault`/`dict.update`/`dict.fromkeys`.
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): migrate all Python stdlib modules to Rust intrinsics-only implementations (Python files may only be thin intrinsic-forwarding wrappers); compiled binaries must reject Python-only stdlib modules. See [docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md](docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md).
- Intrinsics audit is enforced by `tools/check_stdlib_intrinsics.py` (generated doc + lint), including `intrinsic-backed` / `intrinsic-partial` / `probe-only` / `python-only` status tracking and a transitive dependency gate preventing non-`python-only` modules from importing `python-only` stdlib modules.
- Fallback-pattern enforcement now runs across all stdlib modules by default in `tools/check_stdlib_intrinsics.py`; narrowing to intrinsic-backed-only scope is explicit (`--fallback-intrinsic-backed-only`).
- Implemented: bootstrap strict roots (`builtins`, `sys`, `types`, `importlib`, `importlib.machinery`, `importlib.util`) now require an intrinsic-backed transitive stdlib closure in `tools/check_stdlib_intrinsics.py`.
- Implemented: CPython top-level + submodule stdlib union coverage gates now run in `tools/check_stdlib_intrinsics.py` (missing entries, duplicate module/package mappings, and required package-kind mismatches are hard failures).
- Implemented: canonical CPython baseline union is versioned in `tools/stdlib_module_union.py` (generated by `tools/gen_stdlib_module_union.py`) with update workflow documented in [docs/spec/areas/compat/0027_STDLIB_TOP_LEVEL_UNION_BASELINE.md](docs/spec/areas/compat/0027_STDLIB_TOP_LEVEL_UNION_BASELINE.md).
- Implemented: stdlib coverage is complete by name for the CPython 3.12/3.13/3.14 union (`320` top-level required names, `743` required `.py` submodule names), with current checker snapshot `intrinsic-backed=0`, `intrinsic-partial=873`, `probe-only=0`, `python-only=0` under full-coverage attestation mode (any non-attested module/submodule is classified as `intrinsic-partial`).
- Implemented: non-CPython top-level stdlib extras are now limited to `_intrinsics` (runtime loader helper) and `test` (CPython regrtest compatibility facade); Molt-specific DB shim moved out of stdlib.
- Implemented: Molt-specific DB shim moved out of stdlib namespace (`moltlib.molt_db`), with `molt.molt_db` compatibility shim retained for existing imports.
- Implemented: intrinsic pass-only fallback detection is enforced for `json` (try/except + `pass` around intrinsic calls now fails `tools/check_stdlib_intrinsics.py`).
- Implemented: `test.support` now prefers CPython `Lib/test/support` when available (env `MOLT_REGRTEST_CPYTHON_DIR` first, then host stdlib discovery), with a local Molt fallback module for environments without CPython test sources.
- Core compiled-surface gate is enforced by `tools/check_core_lane_lowering.py`: modules imported (transitively) by `tests/differential/basic/CORE_TESTS.txt` must be `intrinsic-backed` only.
- Execution program for complete Rust lowering is tracked in [docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md](docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md) (core blockers first, then socket -> threading -> asyncio, then full stdlib sweep).
- Implemented: `__future__` and `keyword` module data/queries are now sourced from Rust intrinsics (`molt_future_features`, `molt_keyword_lists`, `molt_keyword_iskeyword`, `molt_keyword_issoftkeyword`), removing probe-only status.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): remove `typing` fallback ABC scaffolding and lower protocol/ABC bootstrap helpers into Rust intrinsics-only paths.
- Implemented: `builtins` bootstrap no longer probes host `builtins`; descriptor constructors are intrinsic-backed (`molt_classmethod_new`, `molt_staticmethod_new`, `molt_property_new`) and fail fast when intrinsics are missing.
- Implemented: `pathlib` now routes core path algebra and filesystem operations through Rust intrinsics (`molt_path_join`, `molt_path_isabs`, `molt_path_dirname`, `molt_path_splitext`, `molt_path_abspath`, `molt_path_resolve`, `molt_path_parts`, `molt_path_parents`, `molt_path_relative_to`, `molt_path_with_name`, `molt_path_with_suffix`, `molt_path_expanduser`, `molt_path_match`, `molt_path_glob`, `molt_path_exists`, `molt_path_listdir`, `molt_path_mkdir`, `molt_path_unlink`, `molt_path_rmdir`, `molt_file_open_ex`); targeted differential lane (`os`/`time`/`traceback`/`pathlib`/`threading`) ran `24/24` green with RSS caps enabled.
- Implemented: `molt_path_isabs`/`molt_path_parts`/`molt_path_parents` now use runtime-owned splitroot-aware shaping so Windows drive/UNC absolute semantics are intrinsic-backed (no Python fallback logic), and `pathlib.Path` now supports reverse-division (`\"prefix\" / Path(\"leaf\")`) via intrinsic path joins.
- Implemented: `glob` now lowers through Rust intrinsics (`molt_glob_has_magic`, `molt_glob`) with the Python shim reduced to intrinsic forwarding + output validation; runtime-owned matching now covers `root_dir`, recursive `**` gating (`recursive=True`), `include_hidden`, trailing-separator directory semantics, pathlike `root_dir`, bytes-pattern outputs (`list[bytes]`), mixed bytes/str parity errors, and intrinsic `dir_fd` relative traversal on native hosts (Linux `/proc`/`/dev/fd`, Apple `fcntl(F_GETPATH)`, and Windows handle-path resolution). Wasm hosts use capability-aware behavior: server/WASI targets support `dir_fd` when fd-path resolution is exposed, while browser-like hosts raise explicit `NotImplementedError` for relative `dir_fd` globbing.
- Implemented: `fnmatch` and `shlex` now route matching/tokenization through Rust intrinsics (`molt_fnmatch`, `molt_fnmatchcase`, `molt_fnmatch_filter`, `molt_fnmatch_translate`, `molt_shlex_split_ex`, `molt_shlex_join`) with Python modules reduced to argument normalization + iterator glue.
- Implemented: `stat`, `textwrap`, and `urllib.parse` core surfaces are now runtime-owned through dedicated intrinsics (`molt_stat_*`, `molt_textwrap_*`, `molt_urllib_*`); `stat` now includes intrinsic-backed file-type constants, permission/set-id bits, `ST_*` indexes, and helper functions (`S_IFMT`/`S_IMODE` + `S_IS*`) with a thin Python shim and no host fallback path.
- Implemented: `urllib.parse.urlencode` now lowers through runtime intrinsic `molt_urllib_urlencode`; the shim keeps only query-item normalization and output validation.
- Implemented: `urllib.error` now lowers exception construction/formatting through dedicated runtime intrinsics (`molt_urllib_error_urlerror_init`, `molt_urllib_error_urlerror_str`, `molt_urllib_error_httperror_init`, `molt_urllib_error_httperror_str`, `molt_urllib_error_content_too_short_init`) for `URLError`, `HTTPError`, and `ContentTooShortError`; the module shim is reduced to class shell wiring and raises immediately when intrinsics are unavailable.
- Implemented: `urllib.request` opener core now lowers through dedicated runtime intrinsics (`molt_urllib_request_request_init`, `molt_urllib_request_opener_init`, `molt_urllib_request_add_handler`, `molt_urllib_request_open`) covering request/bootstrap wiring, handler ordering/dispatch, and `data:` URL fallback behind default-opener wiring; Python shim is limited to class shells and response adaptation, with `data:` metadata parity (`getcode()`/`status` -> `None`).
- Implemented: `http.client` now lowers request/response execution through dedicated runtime intrinsics (`molt_http_client_execute`, `molt_http_client_response_*`) and `http.server`/`socketserver` serve-loop lifecycle paths are intrinsic-backed (`molt_socketserver_serve_forever`, `molt_socketserver_shutdown`, queue dispatch intrinsics), with Python shims reduced to thin state wiring and handler shaping.
- Implemented: `enum` and `pickle` are now intrinsic-backed on core construction/serialization paths (`molt_enum_init_member`, `molt_pickle_dumps_core`, `molt_pickle_loads_core`) with `pickle.py` reduced to thin intrinsic-forwarding wrappers (`dump`/`dumps`/`load`/`loads`, `Pickler`, `Unpickler`, `PickleBuffer`) for protocols `0..5`; protocol-5 out-of-band `PickleBuffer` lanes now decode/encode through intrinsic `NEXT_BUFFER`/`READONLY_BUFFER` handling with `loads(..., buffers=...)`; broader CPython 3.12+ reducer/error-text/API-surface parity remains queued.
- Implemented: `queue` now has intrinsic-backed `LifoQueue` and `PriorityQueue` constructors/ordering (`molt_queue_lifo_new`, `molt_queue_priority_new`) on top of existing intrinsic-backed FIFO queue operations.
- Implemented: `statistics` function surface now lowers through Rust intrinsics (`molt_statistics_mean`, `molt_statistics_fmean`, `molt_statistics_stdev`, `molt_statistics_variance`, `molt_statistics_pvariance`, `molt_statistics_pstdev`, `molt_statistics_median`, `molt_statistics_median_low`, `molt_statistics_median_high`, `molt_statistics_median_grouped`, `molt_statistics_mode`, `molt_statistics_multimode`, `molt_statistics_quantiles`, `molt_statistics_harmonic_mean`, `molt_statistics_geometric_mean`, `molt_statistics_covariance`, `molt_statistics_correlation`, `molt_statistics_linear_regression`) with shim-level `StatisticsError` mapping.
- Implemented: runtime-backed slice statistics intrinsics (`molt_statistics_mean_slice`, `molt_statistics_stdev_slice`) are wired for native+wasm lowering paths and preserve generic fallback behavior via runtime-owned slicing/iteration.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete full `statistics` 3.12+ API/PEP parity beyond intrinsic-lowered function surface (for example `NormalDist` and remaining edge-case semantics).
- `enumerate` builtin returns an iterator over `(index, value)` with optional `start`.
- `iter(callable, sentinel)`, `map`, `filter`, `zip(strict=...)`, and `reversed` return lazy iterator objects with CPython-style stop conditions.
- `iter(obj)` enforces that `__iter__` returns an iterator, raising `TypeError` with CPython-style messages for non-iterators.
- Builtin function objects for allowlisted builtins (`any`, `all`, `abs`, `ascii`, `bin`, `oct`, `hex`, `chr`, `ord`, `divmod`, `hash`, `callable`, `repr`, `format`, `getattr`, `hasattr`, `round`, `iter`, `next`, `anext`, `print`, `super`, `sum`, `min`, `max`, `sorted`, `map`, `filter`, `zip`, `reversed`).
- `sorted()` enforces keyword-only `key`/`reverse` arguments (CPython parity).
- Builtin reductions: `sum`, `min`, `max` with key/default support across core ordering types.
- TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:partial): dynamic execution builtins: `compile` now performs Rust parser-backed syntax/scope validation for `exec`/`eval`/`single` modes and returns a runtime code object, but `eval`/`exec` and full compile codegen/sandboxing remain missing; regrtest `test_future_stmt` still depends on full `compile`.
- Differential parity probes for dynamic execution (`eval`/`exec`) are tracked in `tests/differential/basic/exec_*` and `tests/differential/basic/eval_*` and are **expected to fail** until sandboxed dynamic execution lands.
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
- Async generators (`async def` with `yield`) are not supported.
  (TODO(async-runtime, owner:frontend, milestone:TC2, priority:P1, status:missing): implement async generator lowering and runtime parity.)
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
- Differential tests: core-language basic includes pattern matching, async generator finalization, and `while`-`else` probes; failures are expected for pattern matching/async gen until the features are implemented.
- Augmented assignment: slice targets (`seq[a:b] += ...`) are supported, including extended-slice length checks.
- Exceptions: `try/except/else/finally` + `raise`/reraise + `except*` (ExceptionGroup matching/splitting/combining); `__traceback__` now returns
  traceback objects (`tb_frame`/`tb_lineno`/`tb_next`) with frame objects carrying `f_code`/`f_lineno` (see
  [docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md](docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md)). Builtin exception hierarchy now matches CPython (BaseExceptionGroup,
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
- `runpy.run_path` path coercion/abspath/is-file probing is runtime-lowered via `molt_runpy_resolve_path` (bootstrap-PWD aware), and
  execution is intrinsic-backed via `molt_runpy_run_path` (restricted assignment/docstring evaluator; no host fallback).
- `globals()` can be referenced as a first-class callable (module-bound) and returns the defining module globals; `locals()`/`vars()`/`dir()` remain lowered as direct calls,
  and no-arg callable parity for these builtins is still limited.
  (TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.)
- Runtime safety: NaN-boxed pointer conversions resolve through a pointer registry to avoid int->ptr casts in Rust; host pointer args now use raw pointer ABI in native + wasm; strict-provenance Miri is green.
- Hashing: SipHash13 + `PYTHONHASHSEED` parity (randomized by default; deterministic when seed=0); see [docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md](docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md).
- GC: reference counting only; cycle collector pending (see [docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md](docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md)).
  (TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): implement cycle collector.)
- Imports: file-based sys.path resolution and `spec_from_file_location` are supported;
  `importlib.util.find_spec` now routes `meta_path`, `path_hooks`, namespace package search,
  extension-module spec discovery, sourceless bytecode spec discovery, zip-source spec discovery,
  and `path_importer_cache`
  finder reuse through runtime intrinsics.
  (TODO(import-system, owner:stdlib, milestone:TC3, priority:P2, status:partial): full extension/sourceless execution parity beyond capability-gated restricted-source shim hooks.)
- Entry modules execute under `__main__` while remaining importable under their real module name (distinct module objects).
- Module metadata: compiled modules set `__file__`/`__package__`/`__spec__` (ModuleSpec + filesystem loader) and package `__path__`; `importlib.machinery.SourceFileLoader`
  package/module shaping and source decode payload now lower through runtime intrinsics (`molt_importlib_source_loader_payload`,
  `molt_importlib_source_exec_payload`), file reads lower via `molt_importlib_read_file`, and source execution remains intrinsic-lowered
  via `molt_importlib_exec_restricted_source` (restricted evaluator, no host fallback). `importlib.import_module` dispatch lowers through
  `molt_module_import` (no Python `__import__` fallback). `importlib.util` filesystem discovery/cache-path +
  `spec_from_file_location` package shaping now lower through `molt_importlib_find_spec_payload`,
  `molt_importlib_bootstrap_payload`, `molt_importlib_runtime_state_payload`, `molt_importlib_cache_from_source`, and
  `molt_importlib_spec_from_file_location_payload`.
  `importlib.machinery.ZipSourceLoader` source payload/execution now lowers through
  `molt_importlib_zip_source_exec_payload`, and module spec package detection now lowers through
  `molt_importlib_module_spec_is_package`; extension/sourceless loader execution is intrinsic-owned via
  `molt_importlib_exec_extension`/`molt_importlib_exec_sourceless` with capability-gated intrinsic execution lanes
  (`*.molt.py` + `*.py` candidates) before explicit `ImportError`, and unsupported restricted-shim candidates now
  continue probing later candidates deterministically before final failure. Restricted shim execution now
  also handles `from x import *` semantics (including `__all__` validation/fallback underscore filtering)
  in runtime-owned paths.
  `importlib.resources` package root/namespace resolution and traversable stat/listdir payloads are runtime-lowered via
  `molt_importlib_resources_package_payload` and `molt_importlib_resources_path_payload` (including zip/whl/egg namespace/resource roots); loader reader bootstrap
  lowers through `molt_importlib_resources_module_name`/`molt_importlib_resources_loader_reader` (including
  explicit fallback from `module.__spec__.loader` to `module.__loader__`), and custom
  reader contract surfaces lower through `molt_importlib_resources_reader_roots`/`molt_importlib_resources_reader_contents`/
  `molt_importlib_resources_reader_resource_path`/`molt_importlib_resources_reader_is_resource`/
  `molt_importlib_resources_reader_open_resource_bytes`/`molt_importlib_resources_reader_child_names`; direct resources text/binary reads lower through
  `molt_importlib_read_file`. `importlib.metadata` dist-info scan + metadata parsing lower through
  `molt_importlib_bootstrap_payload`, `molt_importlib_metadata_dist_paths`,
  `molt_importlib_metadata_entry_points_payload`/`molt_importlib_metadata_entry_points_select_payload`,
  `molt_importlib_metadata_normalize_name`, and `molt_importlib_metadata_payload`
  (including `Requires-Dist`/`Provides-Extra`/`Requires-Python` payload fields).
  TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.machinery full extension/sourceless execution parity beyond capability-gated restricted-source shim lanes (zip source loader path is intrinsic-lowered).
- Imports: module-level `from x import *` honors `__all__` (with strict name checks) and otherwise skips underscore-prefixed names.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root builds (namespace packages + PYTHONPATH roots supported; remaining: package discovery hardening, `__init__` edge cases, deterministic dependency graph caching).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:planned): method-binding safety pass (guard/deopt on method lookup + cache invalidation rules for call binding).
- Asyncio: shim exposes `run`/`sleep`, `EventLoop`, `Task`/`Future`, `Event`, `wait`, `wait_for`, `shield`, basic `gather`,
  stream helpers (`open_connection`/`start_server`), and `add_reader`/`add_writer`; advanced loop APIs, task groups, and full
  transport/protocol adapters remain pending. Asyncio subprocess stdio now supports `stderr=STDOUT` and fd-based redirection,
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
- C API: no `libmolt` C-extension surface yet; [docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md](docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md) is target-only.
- Policy: Molt binaries never fall back to CPython; C-extension compatibility is planned via `libmolt` (primary) with an explicit, capability-gated bridge as a non-default escape hatch.
  (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define and implement the initial C API shim).
- Intrinsics registry is runtime-owned and strict; CPython shims have been removed from tooling/tests. `molt_json` and `molt_msgpack` now require runtime intrinsics (no Python-library fallback).
- Matmul (`@`): supported only for `molt_buffer`/`buffer2d`; other types raise
  `TypeError` (TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): consider
  `__matmul__`/`__rmatmul__` fallback for custom types).
- Roadmap focus: async runtime core (Task/Future scheduler, contextvars, cancellation injection), capability-gated async I/O,
  DB semantics expansion, WASM DB parity, framework adapters, and production hardening (see ROADMAP).
- Numeric tower: complex supported; decimal is Rust intrinsic-backed with context (prec/rounding/traps/flags),
  quantize/compare/compare_total/normalize/exp/div/as_tuple + `str`/`repr`/float conversions (no Python fallback path;
  when vendored libmpdec sources are absent, runtime uses the native Rust decimal backend); `int` still missing
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
- Implemented: websocket wasm-host edge failures now raise explicit intrinsic-owned errors (capability-denied connect, missing host transport, and wait-registration failures) instead of generic fallback failures.
- TODO(perf, owner:runtime, milestone:RT3, priority:P2, status:planned): cache mio websocket poll streams/registrations to avoid per-wait `TcpStream` clones.

## Thread Safety + GIL Notes
- Runtime mutation is serialized by a GIL-like lock; only one host thread may
  execute Python/runtime code at a time within the process.
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
- Partial shims: `warnings`, `traceback`, `types`, `inspect`, `ast`, `ctypes`, `urllib.parse`, `urllib.error`, `urllib.request`, `fnmatch` (`*`/`?`
  + bracket class/range matching; literal `[]`/`[[]`/`[]]` escapes (no backslash
  quoting)), `copy`, `string`, `struct`, `typing`, `sys`, `os`, `pathlib`,
  `tempfile`, `gc`, `weakref`, `random` (Random API + MT parity: `seed`/`getstate`/`setstate`, `randrange`/`randint`/`shuffle`, `choice`/`choices`/`sample`, `randbytes`, `SystemRandom` via `os.urandom`, plus distributions: `uniform`, `triangular`, `normalvariate`, `gauss`, `lognormvariate`, `expovariate`, `vonmisesvariate`, `gammavariate`, `betavariate`, `paretovariate`, `weibullvariate`, `binomialvariate`), `time` (`monotonic`, `perf_counter`, `process_time`, `sleep`, `get_clock_info`, `time`/`time_ns` gated by `time.wall`, plus `localtime`/`gmtime`/`strftime` + `struct_time` + `asctime`/`ctime` + `timezone`/`daylight`/`altzone`/`tzname` + `mktime` + `timegm`), `json` (loads/dumps with parse hooks, indent, separators, allow_nan, `JSONEncoder`/`JSONDecoder`, `JSONDecodeError` details), `base64` (b16/b32/b32hex/b64/b85/a85/z85 encode/decode + urlsafe + legacy helpers), `hashlib`/`hmac` (Rust intrinsics for guaranteed algorithms + `pbkdf2_hmac`/`scrypt`; unsupported algorithms raise), `pickle` (protocols `0..5` on intrinsic core path, including protocol-`2+` memo/reducer/extension and persistent-hook lanes; still intrinsic-partial for full CPython 3.12+ edge semantics),
  `socket` (runtime-backed, capability-gated; fd duplication/fromfd/inheritable plus socket-file reader read/readline paths route via Rust intrinsics, `dup` now clones via runtime socket-handle intrinsic, default-timeout validation is CPython-shaped, and `gethostbyaddr`/`getfqdn` now lower through dedicated Rust intrinsics; advanced options + wasm parity pending), `select` (`select.select` + `poll`/`epoll`/`kqueue`/`devpoll` objects now intrinsic-backed via runtime selector registries),
  `selectors` (CPython-shaped backend classes now route through intrinsic-backed `select` objects rather than Python async fan-out), `asyncio`, `contextvars`, `contextlib`, `threading`, `zipfile`, `zipimport`,
  `functools`, `itertools`, `operator`, `bisect`, `heapq`, `collections`.
  Supported shims: `keyword` (`kwlist`/`softkwlist`, `iskeyword`, `issoftkeyword`), `pprint` (PrettyPrinter/pformat/pprint parity).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand zipfile/zipimport with bytecode caching + broader archive support.
- Implemented: `zipfile` CRC32 hot path is now intrinsic-backed (`molt_zipfile_crc32`), removing Python-side table construction and fixing backend compile instability in `zipimport_basic` differential lanes.
- Implemented: `pathlib.PureWindowsPath` now matches CPython drive/anchor/parts/parent semantics in the intrinsic-first shim, including UNC and drive-root edge cases.
- Implemented: `smtplib.SMTP.sendmail` parity slice is wired (`MAIL`/`RCPT`/`DATA` flow with refused-recipient payload) and covered by differential test `tests/differential/stdlib/smtplib_sendmail_basic.py`.
- Implemented: `zipimport` API parity expanded with `zipimporter.get_filename` + `zipimporter.is_package`; `get_source` now raises `ZipImportError` for missing modules (CPython behavior).
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): unittest/test/doctest stubs exist for regrtest (support: captured_output/captured_stdout/captured_stderr, check_syntax_error, findfile, run_with_tz, warnings_helper utilities: check_warnings/check_no_warnings/check_no_resource_warning/check_syntax_warning/ignore_warnings/import_deprecated/save_restore_warnings_filters/WarningsRecorder, cpython_only, requires, swap_attr/swap_item, import_helper basics: import_module/import_fresh_module/make_legacy_pyc/ready_to_import/frozen_modules/multi_interp_extensions_check/DirsOnSysPath/isolated_modules/modules_setup/modules_cleanup, os_helper basics: temp_dir/temp_cwd/unlink/rmtree/rmdir/make_bad_fd/can_symlink/skip_unless_symlink + TESTFN constants); doctest is blocked on eval/exec/compile gating and full unittest parity is pending.
- Implemented: `os.environ` mapping methods are runtime-intrinsic-backed (`molt_env_snapshot`/`molt_env_set`/`molt_env_unset`) with str-only key/value checks; `os.putenv`/`os.unsetenv` are lowered to dedicated runtime intrinsics (`molt_env_putenv`/`molt_env_unsetenv`) and keep CPython-style separation from `os.environ`/`os.getenv`.
- Implemented: uuid module parity (UUID accessors, `uuid1`/`uuid3`/`uuid4`/`uuid5`, namespaces, SafeUUID).
- Implemented: collections.abc parity (ABC registration, structural checks, mixins).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (runtime fast-path parser + performance tuning).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `gc` module exposes only minimal toggles/collect; wire to runtime cycle collector and implement full API.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): tighten `weakref.finalize` shutdown-order parity (including `atexit` edge cases) against CPython.
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
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (incremental/stream codecs + full encodings import hooks + error-handler registration); base encode/decode intrinsics plus registry/lookup and minimal encodings/aliases are present.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): complete `pickle` CPython 3.12+ parity (full reducer tuple semantics, complete Pickler/Unpickler API edge behavior, and exact error-text parity after protocol-5 `PickleBuffer` out-of-band support).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): `math` shim covers constants, predicates, `trunc`/`floor`/`ceil`, `fabs`/`copysign`/`fmod`/`modf`, `frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`, and `sqrt`; Rust intrinsics cover predicates (`isfinite`/`isinf`/`isnan`), `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc`; remaining: determinism policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish remaining `types` shims (CapsuleType + any missing helper/descriptor types).
- Import-only stubs: `collections.abc`, `_collections_abc`, `_asyncio`, `_bz2`.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement core collections.abc surfaces.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib extension/sourceless execution parity beyond capability-gated restricted-source shim lanes.
- Implemented: relative import resolution now honors `__package__`/`__spec__` metadata (including `__main__`) and namespace packages, with CPython-matching errors for missing or over-deep parents.
- Implemented: `importlib.resources` custom loader reader contract parity is now wired through reader-backed traversables (`contents`/`is_resource`/`open_resource`/`resource_path`) on top of intrinsic namespace + archive resource payloads, with archive-member path tagging in runtime payloads so `resource_path()` stays filesystem-only across direct + traversable + roots fallback lanes, while archive reads remain intrinsic-backed via `open_resource()`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): importlib.metadata dependency/advanced metadata semantics beyond intrinsic payload parsing.
- Planned import-only stubs: `html`, `html.parser`, `http.cookies`,
  `ipaddress`, `mimetypes`, `wsgiref`, `xml`, `email.policy`, `email.message`, `email.parser`,
  `email.utils`, `email.header`, `urllib.robotparser`,
  `logging.config`, `logging.handlers`, `cgi`, `zlib`.
  Additional 3.12+ planned/import-only modules (e.g., `annotationlib`, `codecs`, `configparser`,
  `difflib`, `dis`, `encodings`, `tokenize`, `trace`, `xmlrpc`, `zipapp`) are tracked in
  [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md) Section 3.0b.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + coverage smoke tests.)
- See [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md) for the full matrix.

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
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): harden backend daemon lane with multi-job compile API + richer health telemetry under high multi-agent contention.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:planned): add function-level object caching and batch diff compile server mode to reduce repeated backend compiles across unchanged functions/tests.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): add import-graph-aware diff scheduling and distributed cache playbooks for multi-host agent fleets.
- TODO(perf, owner:tooling, milestone:TL2, priority:P1, status:partial): finish friend-owned suite adapters (Codon/PyPy/Nuitka/Pyodide), pin immutable suite refs/commands, and enable nightly friend scorecard publication.

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
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): OPT-0003 phase 1 landed (sharded pointer registry); benchmark and evaluate lock-free alternatives next (see [OPTIMIZATIONS_PLAN.md](../../OPTIMIZATIONS_PLAN.md)).
- Single-module wasm linking remains experimental; wasm-ld links relocatable output when `MOLT_WASM_LINK=1`, but broader module coverage is still pending (direct-link runs are disabled for now).
