# Molt Roadmap (Active)

For current supported state, use [docs/spec/STATUS.md](docs/spec/STATUS.md).
This file is forward-looking only. It is refreshed from live code, executable
tests, and generated evidence; the implementation remains the authority when a
roadmap claim drifts.

## Strategic Target

- Reach full CPython `>=3.12` parity for the supported Molt subset.
- Ship standalone binaries with no hidden host Python installation fallback.
- Outperform CPython on the benchmark suites Molt claims as core product lanes.
- Treat tiny-program cold start and output binary size as product-critical
  performance axes, not secondary packaging polish: native, browser/WASM, Luau,
  and MLIR output surfaces must have the same canonical evidence matrix, with
  release artifacts ratcheting toward <50 ms cold start and <2 MB
  gzipped/runtime payloads on the five-year arc.
- Preserve Molt's design exclusions around runtime monkeypatching,
  unrestricted dynamic execution, and unrestricted reflection.

## Current Priorities

1. Close the ownership-correctness front before claiming broader compatibility:
   native DropInsertion activation, finalizer ordering, standalone `__del__`
   exception isolation, and ExceptionRegion Phase 1.
2. Delete legacy RC and compatibility lanes as each structural owner becomes
   authoritative; no second source of truth remains after a touched arc lands.
3. Replace hint-driven backend recovery with a shared representation-aware
   backend contract so native, WASM, and future LLVM lowering optimize from the
   same typed facts.
4. Drive native, WASM, and Luau toward the same supported contract.
5. Expand the first-class CLI/profile/target/backend validation matrix until
   every supported backend release claim is backed by end-to-end proof instead
   of backend-internal proof alone.
6. Finish consolidating setup, doctor, validate, and thin-wrapper behavior into
   one coherent CLI-first DX surface.
7. Keep multi-agent build throughput deterministic by default: canonical
   dev/CI/DX and CLI Cargo paths run with `CARGO_INCREMENTAL=0` unless an
   operator explicitly opts into incremental-debug work, and guarded Cargo
   interruptions quarantine only Cargo incremental state with receipts instead
   of deleting shared target artifacts. Tempfile-backed binary subprocess
   capture is an adapter over the same guard authority, so inherited-pipe-safe
   build/probe lanes do not fork memory, repro, profile, or quarantine custody.
   The memory-guard wiring audit now consumes the real subprocess-coverage
   scanner, and the subprocess audit is clean: future unexpected raw launchers,
   stale allowlist entries, or expanded allowlist counts fail the wiring audit.
   `tools/dx_build_timer.py`
   has moved its Cargo timing/version probes to `MOLT_DX_BUILD` custody, and
   `tools/cold_start_decompose.py` now routes safe-run, no-op C compile, dyld
   timing, and Molt-probe build subprocesses through `MOLT_COLD_START`.
   `tools/gen_intrinsics.py` now formats generated Rust through
   `MOLT_GENERATOR` custody, and `tools/perf_inner_repeat.py` plus its
   perf-scoreboard proof test now route inner-repeat proof children through
   `MOLT_BENCH` / `MOLT_TEST`. `tools/perf_scoreboard.py` also routes
   `safe_run.py --json` workload timing children and Codon build children
   through `MOLT_BENCH`, and has a single bounded `_metadata_probe` authority
   for read-only host metadata plus one `_profiling_popen` authority for
   interactive `/usr/bin/sample` profiling children.
   `tools/molt_dev.py` now routes git/interpreter probes, manifest gates,
   toolchain marker probes, worktree cleanup, and difftest byte captures
   through shared guard helpers, while detached daemon execution uses
   fork/exec/wait custody instead of raw subprocess calls.
   The Kani CI proof workflow now wraps install/setup/proof commands with
   `tools/guarded_exec.py`, and nightly/security workflows guard direct
   cargo-deny/cargo-audit plus Quint proof invocations the same way.
8. Keep Python `3.12`/`3.13`/`3.14` target-version gates explicit across CLI,
   pyproject/UV-oriented workflows, frontend caches, backend caches, and the
   unconditional runtime bootstrap/state contract used by importlib and stdlib
   gates across native, WASM, standalone Rust source emission, and isolate
   entry paths.
9. Make performance reporting and compatibility reporting generator-owned
   instead of manually synchronized across multiple docs.
9. Drive the Luau target from checked source emission to full current/future
   Luau parity coverage, with generated OpIR support evidence and no silent
   semantic stubs.
10. Add first-class output startup and binary-size evidence to the performance
    loop so regressions in minimum executable footprint, loader cost, dyld/code
    signature fixed costs, runtime feature bloat, native/WASM parity, and WASM
    payload size are caught before benchmark throughput hides them.
11. Keep intrinsic names equal to runtime symbols unless a future design proves
    a first-class alias abstraction is required; the retired async-sleep
    name/symbol bridge is not a compatibility pattern to revive.
12. Keep test execution under mandatory memory custody: direct pytest must
    enter startup custody through `sitecustomize.py`, the packaged pytest entry
    point, or the repo-configured plugin-autoload fallback before collection,
    then re-exec through the process-tree guard when unguarded; interpreter
    option forms and programmatic pytest launches must use pytest hook args as
    the authority, and attempts to disable the guard plugins with `-p no:...`
    or `PYTEST_ADDOPTS`, to replace the repo pytest config with unsafe `-c`,
    or to set `PYTEST_DISABLE_PLUGIN_AUTOLOAD` without the explicit repo guard
    config plugin must fail closed. Direct `tests/**.py` and `python -m tests.*`
    launches must also re-exec through the same process-tree guard via
    path-local/project `sitecustomize.py` startup hooks, without adding harness
    files inside differential corpus directories. Pytest and test-wrapper
    incidents must use canonical parent-side
    `MOLT_PYTEST_CURRENT_TEST_FILE` custody under `tmp/pytest-memory-guard/`,
    with xdist workers writing per-worker sidecars so the parent guard can
    include all bounded worker records and mark the record whose pid lineage
    matches the violating process. Legacy `*_MEMORY_GUARD=0` knobs are not
    custody bypasses.
    Tempfile-backed capture helpers, suite calibration, wasm diff, DX build
    timing, perf-scoreboard launchers, and CLI binary smoke probes must all use
    the same parent-side guard/profile/repro custody.
    Adaptive guard budgets must use live OS pressure where available, including
    macOS `vm_stat` available pages, and incident repro must include bounded
    Claude/Codex/control-plane PGID samples so host crashes are attributable.
    Repo cleanup must prove host/Claude/Codex control-plane process groups and
    external Claude/Codex-descendant process groups are protected before any
    drain path can terminate Molt-owned workers, while still allowing the
    current guard to terminate its own child tree. Every
    sentinel trip/drain/stale-preflight path must preserve sampled processes,
    external parents, resolved guard limits, and bounded repro context inline in
    JSON diagnostics. Cleanup JSON is the carrier for parsed sentinel events;
    raw sentinel streams stay verbose-only.
    Backend-daemon identity custody now lives in
    `src/molt/backend_daemon_custody.py`: CLI restart/stale-cleanup paths and
    `tests/molt_diff.py`, `tools/bench.py`, `tools/bench_wasm.py`, and
    `tools/bench_individual.py` use the shared verifier before signaling, and
    legacy `.pid` files are unlink-only debris. `tools/compile_progress.py`
	    excludes backend daemons from marker-scoped child cleanup, and
	    `tools/verify_native_binary_valid.sh` builds daemon-off without blanket
	    `pkill`. `tools/bench.py` and `tools/bench_wasm.py` now use cache-enabled
	    Molt builds by default, with `--no-molt-build-cache` reserved for deliberate
	    cold/no-cache studies. `tests/molt_diff.py` now keeps fallback `MOLT_CACHE`
	    on the persistent diff cache root instead of deleting it with each
	    per-test output tree, and exposes `--stdlib-profile` for one-file full
	    stdlib/import loops without env-only setup. `MOLT_STDLIB_MODULE_SYMBOLS`
	    now has one backend parser authority and malformed values fail closed
	    instead of falling back to heuristic shared-stdlib partitioning. TIR
	    `ExceptionRegions` diagnostics now fail closed at pass-manager
	    verification, blocking missing/ambiguous/too-early handler-match-ref
	    release facts before backend lowering. Shared TIR DropInsertion now
	    consumes CreationRefs at the `raise` boundary and MatchRefs after the
	    owning `exception_pop` by materializing ordinary TIR `DecRef` ops, and
	    native Cranelift is activated on that same shared drop path. The old
	    native-only CreationRef lifetime carve-out and exception-pop side path are
	    deleted. Validator fail-closed coverage now includes
	    ambiguous-depth, path-alternative pop, loop re-entry close-boundary,
	    shared `exception_pop` splitting with block-arg payloads,
	    malformed Luau block structure, and
	    terminal drop-pipeline entry diagnostics; checked LLVM/WASM/Luau
	    consumption evidence covers lowering order, WASM import/LIR retention,
	    Luau target-info terminal-drop execution plus shared-drop no-ops, and
	    executed Luau, LLVM, plus WASM differential runtime proof for the
	    raise/catch leak loop, while the wider `HandlerState` boundary and
	    authoritative speed evidence remain the parity closeout. CLI rebuild/cache policy now treats shared exact-key
	    cache artifacts as non-destructive during
	    build/link/probe hot paths,
	    publishes JSON/cache/text/byte/file/archive sidecars and final file
	    artifacts through atomic temp siblings, shares Cargo rebuild locks by
	    resolved build-state root, derives WASM runtime rebuild outputs from
	    Cargo `compiler-artifact` JSON instead of deleting candidate artifacts
	    before rebuild, requires byte-digest sidecars before hydrating WASM
	    runtime `.wasm`/`.a` candidates, replaces vendored directory
	    delete/copy with prepared temp-tree plus restore-on-failure
	    publication, and keys module-analysis artifacts by import scan
	    semantics. Respected `PYTHONPATH` roots that duplicate repo-owned roots
	    now stay internal instead of becoming transitive-external admission
	    barriers, so `PYTHONPATH=src` no longer prunes stdlib closure modules
	    such as `abc`/`copy` from full-profile importlib builds. Backend IR
	    preparation now also fails before codegen when a direct call targets a
	    module-owned symbol whose module is absent from the graph, including the
	    originating function/op index for repro while preserving lazy
	    `MODULE_IMPORT` runtime boundaries. Next, extend
	    the invalidation/retention evidence matrix and
	    wire OS-level directory exchange where available so
	    high-concurrency build lanes prove they rebuild only invalidated final
	    outputs or tool artifacts across native, WASM, LLVM, and Luau profiles.
13. Finish the large external-package compile-memory front with tinygrad as the
    canonical friend-suite driver. External roots are now resolvable without
    implying unlimited static closure; full package closure is explicit through
    `MOLT_EXTERNAL_STATIC_PACKAGES` and cache-keyed. Explicitly admitted
    packages now also fail closed on package-local `.so`/`.pyd` artifacts
    without valid `extension_manifest.json` sidecars, and the graph/wrapper
    plus backend object-cache inputs include native artifact and manifest
    hashes. Native builds now publish those validated artifacts, manifests,
    package `__init__.py` files, and runtime extension shim candidates into a
    deterministic `external_static_packages/<plan-digest>/` artifact root,
    inject that staged root into generated native binaries before runtime
    startup, and hash the staged bytes into the final link fingerprint without
    adding runtime-loaded extensions to the linker command. Unsupported
    non-native, MLIR, WASM, and object-only outputs now fail closed when
    external native artifacts are admitted. Frontend source leases and
    byte-backed backend IR custody now avoid retaining the whole upstream graph
    through lowering/backend handoff, and daemon full-request custody removed
    the hidden one-shot overlap. Current durable evidence remains fail-closed
    before Molt adapter workload execution: `bench/results/friends/20260612T203111Z/`
    fails with a lost-daemon outcome and no outer memory-guard violation,
    `20260612T205850Z/` fails with an empty daemon response, and the 21:12
    sidecar `tmp/memory_guard/friends_tinygrad_molt_daemon_harness_custody.json`
    records a Molt-owned backend daemon RSS guard trip near the 12 GB cap.
    Native application-object batching now consumes the same
    `MOLT_BACKEND_BATCH_OP_BUDGET` authority as stdlib batching. Daemon-off
    proof now builds the full-stdlib adapter and reaches runtime execution under
    guard; after the importlib bootstrap export fix and list-clear detach proof,
    the current Molt runtime blocker remains `molt fatal: invalid object header
    before dec_ref` at 1.985 GB peak RSS
    (`tmp/memory_guard/tinygrad_adapter_run_daemon_off_after_list_detach_retry.json`).
    The current remaining work is runtime object-header/RC correctness plus
    daemon outcome/log custody until the adapter workload executes and leaves a
    benchmark artifact.
14. Make ecosystem compatibility a first-class generated scoreboard, starting
    with NumPy. The current matrix has a dedicated NumPy row deriving `partial`
    through `D28` source-recompiled `libmolt` extension packages, not through
    host-Python or CPython wheel fallback. The next structural objective is a
    Molt-green `numpy_off_the_shelf` compile/import probe that drives NumPy
    C-API symbol closure, source-recompiled native extension package
    build/staging/runtime proof, explicit extension execution capability,
    all-loaded-module origin custody, and no-host runtime loading.

## Milestone Sequence

### Near Term

- Finish the documentation-architecture cleanup and turn doc ownership into CI
  policy.
- Tighten compatibility rollups around generated evidence.
- Finish DropInsertion convergence from the live code state: native Cranelift
  is activated on TIR DropInsertion after the loop-phi raw/heap representation
  invariant proof. The remaining convergence work is the wider WASM/native
  finalizer and RC-balance matrix plus deletion of the broader competing native
  value-tracking RC lane where shared facts now cover the ownership surface.
- Finish backend-neutral ExceptionRegion Phase 1. TIR now has a shared
  read-only `ExceptionRegions` analysis that records MatchRef producers,
  reachable path-depth release pops, and diagnostics; shared DropInsertion
  consumes CreationRefs at the `raise` boundary and MatchRefs after the owning
  `exception_pop` by materializing ordinary TIR `DecRef` ops; native Cranelift is
  active on that shared drop path; and the old native-only CreationRef lifetime
  carve-out plus exception-pop side path are deleted. Validator fail-closed
  coverage now includes missing-pop, ambiguous-depth, path-alternative pop, loop
  re-entry close-boundary, shared `exception_pop` splitting with block-arg
  payloads, malformed Luau block structure, and terminal
  drop-pipeline diagnostics. Checked
  backend consumption evidence now covers LLVM lowering order, WASM import/LIR
  retention, Luau target-info terminal-drop execution plus shared-drop no-ops,
  and executed Luau, LLVM, plus WASM differential runtime proof for the
  raise/catch leak loop. The durable milestone that remains is to finish the
  wider `HandlerState` boundary and obtain authoritative
  `bench_exception_heavy` speed evidence.
  The prior WASM runtime-surface blocker that pulled `molt-db`/sqlite into the
  linked runtime is closed at the feature-plane level: wasm micro and full
  runtime profiles now exclude sqlite from the compile-time availability surface
  and Cargo command/fingerprint facts. The corrected `run --target wasm`
  end-to-end proof now builds a structurally valid linked artifact, advances
  past the former `func 1233` stack-validation failure, and the
  `tools/wasm_diff.py` leak-loop differential now passes for
  `tests/differential/memory/exception_raise_catch_loop_leak.py`. The JS harness
  host map includes the process host ABI imports required by the linked runtime,
  including `env::molt_process_terminate_host`.
- Finish finalizer ownership boundaries: the standalone raising-finalizer lane,
  native scope-exit ordering gate, plain-object false-positive guard,
  object-attribute release smoke, focused container clear/pop boundaries,
  inline object-field child release, exit-semantics lane, and shared `DeleteVar`
  old-slot release boundary are green, including the explicit local `del` /
  `gc.collect()` resurrection-once differential. Remaining work is the broader
  resurrection/leak matrix and backend-wide finalizer ordering parity.
- Resume CallFacts only after the exception-region lane is no longer competing
  for the same no-throw/handler facts; its first landing must be a real
  generated/typed analysis surface, not comments or inert markers.
- Make typed SSA / explicit representation facts survive lowering without
  degrading into transport-only hints.
- Continue backend-native function decomposition by complete op-family handlers
  that are independent codegen units. The current live code has moved indexing
  and scalar builtin runtime-call dispatch (`id`, `ord`, fused `ord_at`, `chr`)
  under `native_backend/function_compiler/fc/`; keep extracting whole families
  without splitting semantic ownership or moving `len` until its
  representation-plan specialization can move as a complete structural unit.
- Keep the TIR pipeline unconditional for backend-facing lowering; debugging
  uses dumps and verifier evidence rather than an environment-variable bypass,
  and frontend midend fixed-point/idempotence verification must fail closed
  instead of accepting non-converged IR under policy knobs. Midend rounds must
  remain algebraically closed across CSE and DCE: any pure definitions made dead
  by CSE are verified and eliminated before the fixed-point comparison.
- Close the highest-value native and WASM parity blockers.
- Burn down the remaining Molt side of the enabled tinygrad off-the-shelf lane
  (`a83710396c991272241e40da94489747c2393851`): upstream tinygrad and the
  CPython public-API adapter now run with clean pinned custody, while the Molt
  runner is executable by default. Keep upstream tinygrad unmodified, keep
  `MOLT_EXTERNAL_STATIC_PACKAGES=tinygrad` as the explicit full-closure contract,
  keep the runner on `{python} -m molt.cli run` with the full stdlib build
  profile forwarded, and reduce backend-daemon compile RSS until the runner
  reaches adapter workload execution under the benchmark memory guard. Current
  evidence gets past manifest skip and full-profile validation, then kills
  `molt-backend --daemon` at 12.005 GB after 435.5s
  (`tmp/memory_guard/friends_tinygrad_molt_sqlite_profile.json`). Path-backed
  `ModuleSourceLease`/analysis metadata, byte-backed backend IR custody, and
  bounded native TIR optimization result consumption are now in-tree; guarded
  evidence reached the bounded path and reduced the single-backend peak before
  exposing aggregate process-tree RSS from overlapping daemon/fallback
  lifetimes. Daemon compile custody now treats
  full-request admission as terminal ownership, and verified live daemons are
  not restarted after short readiness misses while they may be compiling. The
  follow-up runner proof (`bench/results/friends/20260612T203111Z/`,
  `tmp/memory_guard/friends_tinygrad_molt_daemon_custody.json`) confirms
  aggregate process-tree RSS no longer comes from duplicate daemon/one-shot
  lifetimes (`violation=null`, no orphaned groups, 4.92 GB peak process-tree
  RSS); the current failure is a fail-closed lost daemon outcome. A 2026-06-12 guarded rerun
  after the explicit-`DeleteVar` finalizer boundary work
  (`bench/results/friends/20260612T205850Z/`) stayed fail-closed without a memory
  violation, protected host/control-plane process groups in the sentinel log,
  and returned after 208.19s with `Backend daemon compile failed: backend daemon
  returned empty response`. The 21:12 guard sidecar
  (`tmp/memory_guard/friends_tinygrad_molt_daemon_harness_custody.json`) records
  a separate Molt-owned backend daemon RSS guard trip at the 12 GB process cap;
  it did not leave a full `results.json`/`summary.md` benchmark artifact.
  Native application-object batching now consumes the same
  `MOLT_BACKEND_BATCH_OP_BUDGET` authority as stdlib batching. Daemon-off proof
  now builds the full-stdlib adapter and reaches runtime execution under guard;
  after the importlib bootstrap export fix and list-clear detach proof, the
  current Molt runtime blocker remains `molt fatal: invalid object header before
  dec_ref` at 1.985 GB peak RSS
  (`tmp/memory_guard/tinygrad_adapter_run_daemon_off_after_list_detach_retry.json`).
  The native benchmark/friend-suite Molt result contract now records
  phase-aware `molt_failure` payloads, so the next rerun can distinguish
  build-phase `daemon_crash` details such as `backend_daemon_empty_response`
  from run-phase `runtime_crash` details such as
  `molt_runtime_invalid_object_header_before_dec_ref` instead of collapsing
  them into generic failed rows. The next structural landing point is runtime
  object-header/RC correctness, while daemon-enabled runs still need the
  compile-memory split to leave a complete benchmark artifact.
- Continue the clean `molt-gpu` scheduler lane from the current code state:
  Movement/ShapeTracker binding is now a real per-kernel input-view contract
  with storage identity separated from binding identity across scheduler,
  fusion, CPU interpreter, runtime bridge, and executable renderers.
  `Contiguous` materialization now lowers to explicit copy kernels with fresh
  storage identity. CPU materialization is raw-byte exact; executable shader
  copy bodies stay fail-closed when backend dtype narrowing would break the
  copy contract. Metal e2e proof now covers flipped and padded materialization,
  raw `UInt16` copy preservation, and repeated-storage binding slots that share
  one device buffer; CPU copy proof covers every current dtype element width and
  padded raw zero-fill; runtime bridge proof covers one storage id routed
  through distinct CPU and Metal view slots; cross-renderer text proof covers
  non-float `UInt32` copy bodies. `bench_primitives` now proves contiguous CPU
  materialization at raw-copy class speed (`64.68 us` vs `53.70 us` raw copy
  for four source megabytes), flipped f32 materialization at `66.83 us`, flipped
  1/2/4/8-byte raw element rows at `66.52/66.43/66.60/66.46 us`, plus shrunk
  (`55.86 us`) and padded (`55.26 us`) single-view copies through raw-span
  plans. For non-MXFP storage, MLIR `MaterializeCopy` now emits flat memref
  arguments by binding slot plus generated ShapeTracker
  affine/mask arithmetic for contiguous, flipped, shrunk, padded/masked,
  permuted, composed, and expanded zero-stride views. MLIR compute now lowers
  pure elementwise kernels to real flat memref signatures, `scf.for` loops,
  ShapeTracker-indexed loads, masked-safe `scf.if` zero-fill, typed op SSA, and
  final stores; coverage includes flipped, padded/masked, same-storage distinct
  slots, composed views, integer-vs-float comparison typing, constants,
  prior-op chains, and explicit non-MXFP cast conversion selection. Cast target
  dtype now has first-class lazy/scheduler/runtime custody through
  `LazyOp::Cast`, `FusedOp::dst_dtype()`, and `molt_gpu_prim_cast`; CPU execution
  uses typed scalar Cast/Bitcast values for terminal, fused intermediate, and
  pre-reduce cases; old untyped unary Cast/Bitcast construction rejects.
	  Runtime tensor lifecycle now has typed raw upload and typed zero-fill through
	  `molt_gpu_prim_create_tensor_raw` and `molt_gpu_prim_zeros_dtype`, with MXFP
	  upload fail-closed until the block/exponent layout is explicit. Runtime
	  readback exposes dtype, logical storage byte count, and exact raw realized
	  storage-byte copy through `molt_gpu_prim_dtype`, `molt_gpu_prim_nbytes`, and
	  `molt_gpu_prim_read_data_raw`, while the legacy f32 readback API remains
	  fail-closed for realized non-Float32 tensors. Metal
	  e2e proof now byte-compares non-f32 Cast/Bitcast storage for
	  Float32->Int32/UInt16/UInt8 and equal-width Float32<->UInt32 against the CPU
	  interpreter.
	  Reductions now carry first-class `ReductionDomain` metadata through lazy
	  shape inference, scheduling, fusion, kernel hashing, CPU interpretation,
	  MIL rank restoration, and shader renderer codegen. CPU and shader
	  renderers lower the explicit row-major domain index instead of inferring
	  flat `input_numel / output_numel` segments, and fusion treats post-reduce
	  output-shape expansion as a hard boundary until broadcast-after-reduce is a
	  real IR primitive. MLIR now lowers `ReduceSum`/`ReduceMax` from that same
	  domain metadata with nested `scf.for` loops, dtype-correct accumulator
	  identities, pre-reduce prefixes, same-output-shape suffixes, and explicit
	  invalid-reference checks. MIL now carries ranked values through compute
	  lowering, reshapes flat gathered ShapeTracker views back to the logical
	  reduction input shape, emits domain axes against ranked tensors, and
	  returns the ranked reduction output shape. Shader renderers now reduce the
	  declared `reduce_op.srcs()[0]` rather than the last pre-reduce temporary.
		  `FusedOp`/`FusedOpDomain` construction is constructor-only with private
		  fields and accessor-based readers, so op/domain/dtype invariants cannot be
		  invalidated by post-construction mutation. MLIR MXFP buffer storage,
		  `MaterializeCopy`, constants, zero constants, element types, and quantized
		  casts stay fail-closed until explicit block/exponent storage plus conversion
		  lowering exist.
  MIL now lowers verified Bool, Int8/16/32, UInt8/16/32, Float16, and Float32
  `MaterializeCopy` views through explicit `range_1d`/`gather`/`select` tensor
  ops with safe masked gather indices, dtype-correct zero literals, physical
  offset span checks, and int32-domain guards; contiguous views return the input
	  binding directly. MIL compute read views remain Float32-only.
			  Next, add MIL BF16/64-bit/MXFP materialization only with real Core ML package
			  compile/run/raw byte-roundtrip proof, keep unsupported dialects fail-closed,
				  add MLIR MXFP block/exponent storage and `MaterializeCopy` lowering before
				  quantized cast lowering, then add a first-class window/im2col primitive
				  for tinygrad convolution wrapper migration and typed nonzero-pad
				  semantics. Constructor upload, typed zeros, dtype codes,
				  exact raw readback, public bytes/uint/int readback, elementwise
				  unary/binary operations, ternary `where`, typed casts, explicit-axis reductions,
				  Rust-owned all-axis reductions through `molt_gpu_prim_reduce_all`,
				  movement-family views, and matmul composition are now covered by
				  focused wrapper/runtime tests.
	  Off-the-shelf upstream tinygrad benchmarking is now a first-class enabled
	  driver: `bench/friends/manifest.toml` registers pinned suite
	  `tinygrad_off_the_shelf` at commit
	  `a83710396c991272241e40da94489747c2393851`, with a non-synthetic
	  `tinygrad` runner for upstream `test/test_tiny.py` through
	  `uv run --isolated --with typeguard` plus runner-local
	  `PYTHONPATH={suite_root}` so the checked-out upstream package stays clean;
	  the CPython runner uses `tools/tinygrad_off_shelf_adapter.py`
	  as a public-API benchmark driver against the checked-out upstream package.
		  The Molt runner executes the correct `{python} -m molt.cli run`
		  full-stdlib command by default; current evidence reaches the backend daemon
		  and no longer trips the outer memory guard after daemon full-request
		  custody closed the hidden one-shot overlap, but still fails before
		  adapter workload execution through lost-daemon / empty-response outcomes.
		  A separate 21:12 sidecar records a Molt-owned backend daemon RSS guard
		  trip near the 12 GB process cap. App-object batching now shares the
		  stdlib op-budget authority; backend compile-memory reduction remains the
		  next owner to retire. `tools/bench_friends.py`
	  now fail-closes
	  git-source custody by verifying requested refs against checked-out `HEAD`,
	  requiring clean upstream checkouts, supporting per-suite `--suite-root` and
	  `--repo-ref` overrides for already-pinned clones, and ingesting adapter JSON
	  workload timings into `results.json` instead of relying only on subprocess
	  wall time. The public tinygrad shim now routes both `import tinygrad` and
	  `from tinygrad import Tensor` through `molt.gpu.Tensor` without binding a
	  case-mismatched child module, and the adapter includes `where_promotion`
	  plus `movement_views` as public dtype/ternary-select and view-movement
	  compatibility workloads.
- Promote NumPy from implicit tier-C aspiration to an explicit fail-closed
  ecosystem lane. `tools/ecosystem/dynamism_features.json` now separates `D23`
  CPython binary-wheel bridging from `D28` source-recompiled `libmolt`
  extension packages, the generated ecosystem matrix includes NumPy as
  `partial` through `D28`, and `bench/friends/manifest.toml` registers enabled
  pinned suite `numpy_off_the_shelf` at upstream commit
  `c81c49f77451340651a751e76bca607d85e4fd55`. The lane now has custody-only
  source-tree audit, an isolated CPython `numpy==2.4.2` public-API baseline, a
  canonical `molt extension scan --source {suite_root}/numpy --fail-on-missing`
  C-API closure gauge with per-symbol `runtime_backed`,
  `source_compile_only`, `fail_fast`, and `missing` statuses, and a real Molt
  runner using
  `MOLT_EXTERNAL_STATIC_PACKAGES=numpy`, explicit `module.extension.exec`
  capability, and all-loaded-`numpy.*` module-origin custody. External static
  package admission now scans package-local native artifacts, requires valid
  sidecar manifests with module/path/checksum/ABI/target/platform/capability
  facts, and fingerprints those facts in graph, wrapper, and backend
  object-cache inputs. Native builds now deterministically stage/publish the
  validated artifact, sidecar, package `__init__.py`, and runtime extension
  shim candidates under a plan-digested `external_static_packages` runtime root,
  inject that root into generated native binaries before runtime startup, and
  include the staged bytes in final link reuse decisions. Target modes without a
  runtime-custody consumer now fail closed. Friend-suite
  metrics exclude custody/scan runners from speedup math and
  reject ignored checkout artifacts; the next complete structural landing point
  is making the C-API scan and Molt runner green through source-recompiled
  package build orchestration, NumPy C-API symbol closure, and no-host NumPy
  import/runtime-load proof.
- Import/bootstrap hardening: preserve the new module-init import-closure
  boundary as the graph-builder invariant. Plain package imports must admit only
  semantically required module-init dependencies; lazy backend/device families
  become explicit runtime/device edges. The CLI now materializes a single
  immutable `ImportPlan` after entry graph discovery and before frontend
  analysis; runtime-import support closure is owned by entry planning, while
  materialization owns namespace stubs, generated importer modules, known-module
  sets, allowlist snapshots, and graph metadata. Core stdlib closure now reuses
  the same nested-scan exception set as normal stdlib discovery, so modules such
  as `collections` admit required function-body support imports like `copy`
  without reopening third-party lazy backend families. Shared stdlib cache keys
  now seed every explicit stdlib module init exactly like backend DFE, cache
  reuse requires backend-written partition manifests (sorted function names plus
  body hash), and the native backend rejects any shared-stdlib partition whose
  SimpleIR function references are not closed inside the partition before
  reuse or publish, so stale objects cannot externalize a different stdlib
  partition or leave `collections__UserDict_copy -> copy__copy` unresolved.
  The Rust-owned import transaction now exists for the active importlib and
  `builtins.__import__` runtime paths, and the retired
  `molt_importlib_import_module` intrinsic/export/WASM row has been deleted so
  importlib cannot drift through a resolved-name-only side door. The empty
  `_MODULE_ALIASES` branch is gone, and frontend literal/direct-call folding of
  `importlib.import_module("literal")` now emits a direct
  `molt_importlib_import_transaction(name, None, None, ("*",), 0)` call
  whenever callable identity and an absolute literal name are statically proven;
  runtime import owns target availability, version-gated absence, module cache
  custody, provenance, fromlist behavior, and error shape. Frontend module-attribute
  mutation tracking now refuses both the transaction fold and cross-module static
  direct-call lowering whenever `importlib.import_module` is rebound through
  `importlib` or a module alias. Module graph resolution now requires exact
  filesystem casing, known project modules authorize only exact graph members,
  and ordinary source syntax imports now call the same transaction intrinsic
  with explicit `name`/`fromlist`/`level` payloads while bootstrap/importlib
  modules keep a private cycle-breaking `MODULE_IMPORT` boundary, and the
  focused native shard now preserves `_bootstrap` / `_bootstrap_external`
  submodule identity. Graph-proven `fromlist` child auto-import/binding now
  lives inside the Rust transaction for the focused native path, preserving
  package exports and binding successful child modules on the parent before the
  final `IMPORT_FROM`. The remaining structural import work is to continue
  public API validation beyond the covered `import_module` relative-package and
  bootstrap-submodule cases while sharing the private resolver,
  implement CPython 3.12 package-context calculation
  (`__package__` / `__spec__.parent` / `__name__` fallback), and close
  `fromlist` star/`__all__` plus namespace-package edge semantics under the
  same transaction authority.
- Establish baseline and ratchet artifacts for tiny hello-world output rows
  across native, linked-WASM, Luau, and MLIR: release/dev size, backend/profile
  dimensions, same-path startup where runnable, fresh-path startup where
  runnable, CPython process baseline, and tiny C baseline. WASM already proves
  that tiny payloads are possible under the right linked/profile discipline;
  native must converge by making runtime reachability as precise as the WASM
  import/export path rather than relying on coarse domain features alone.
- Keep the generated Luau support matrix current and use it to prioritize
  checked CPython-vs-Luau feature-gap closure.
- Keep the canonical local validation matrix green across:
  - `molt validate --suite smoke`
  - `molt validate`
  - native `build` / `run` / `compare` on `dev` and `release`
  - linked-WASM build plus Node execution
  - honest unsupported-semantics failures (`exec` / `eval`)

### Medium Term

- Expand language and stdlib coverage under the Rust-first lowering model.
- Keep retiring `fast_int` / `fast_float` / `type_hint` transport hints and raw
  scalar shadow lanes as the architectural center of backend optimization.
  TIR-to-SimpleIR lowering no longer accepts an external type map for scalar
  hint reseeding; remaining performance work must flow through shared
  representation-aware TIR/LIR contracts. Native int codegen has retired the
  raw-int shadow transport in favor of `int_primary_vars`, and native
  float-primary codegen now treats
  `float_primary_vars` as the single static authority for F64-primary variables.
  TIR functions now own a persistent `value_types` map, and type refinement
  writes op-result facts back into that map instead of leaving them as a
  transient extraction side table; range/list devirtualization also records the
  I64/Bool facts it synthesizes for loop-carried values. Backend scalar
  lowering now builds a final-codegen-time `ScalarRepresentationPlan` from
  refined TIR/LIR facts, `SimpleValueNames`, and explicit single-output
  provenance, then derives semantic scalar/None classifications from that plan
  instead of recomputing them from SimpleIR op strings. Native uses the plan for
  raw-primary carrier eligibility, scalar slot escape safety, scalar
  store-target discovery, and operation lane preference, while preserving the
  stricter exact-carrier safety predicates needed by codegen. Legacy WASM and
  Luau scalar fast paths now consume the same plan for integer-family
  arithmetic, comparison, truthiness, and index-key scalar decisions instead of
  trusting `fast_int`, `fast_float`, or scalar `type_hint` transport metadata.
  Generic container annotations now parse through the same TIR type authority:
  `list[T]`, `dict[K, V]`, `set[T]`, and fixed-arity `tuple[...]` produce
  structured `TirType` facts for SSA/value maps while malformed, dynamic, or
  unsupported compound hints remain `DynBox`.
  Backend semantic container dispatch now reads those facts through the shared
  plan for Luau, WASM import selection/emission, native `len`/`contains`, and
  LLVM `len`; `container_type` / `type_hint` strings alone no longer select
  those specialized paths. Semantic `list[int]` is not treated as flat
  `list_int` storage proof; native direct storage optimizations now require a
  shared `ContainerStorageKind::FlatListInt` fact seeded by structural
  `list_int_new` producers and queried through the representation plan.
  Native bool codegen has the same raw-closed `bool_primary_vars` contract for
  constants, alias/store propagation, comparisons, identity checks, and
  truthiness casts. Bool-primary escape points now box raw `0/1` carriers
  through a dedicated raw-bool boxing helper instead of feeding I64 carriers to
  the b1-condition bool boxer. Raw-closed bool join carriers also stay on the
  main raw `0/1` Variable contract across store/load/copy and structured phi
  binding, while unsafe join slots remain boxed. Proven-bool list indexing now
  enters the same bool-primary contract when the index operand is raw-primary,
  keeping the existing index-fast-path selection separate from output
  representation. Unknown-list getitem truthiness uses an explicit conditional
  list-bool carrier for the runtime list/list_bool split. Float primary
  eligibility is now definition-scoped: unsupported producers such as `pow`
  keep only their own outputs boxed and cannot disable unrelated proven float
  locals in the same function. The native raw-f64 shadow lane is retired:
  `float_primary_vars` is the only raw-F64 authority, and non-primary floats
  are boxed immediately in their main I64 variable. Native scalar store-target
  discovery is now shared across int, float, bool, and str lanes, preserving
  the all-sources rule. The native raw-bool shadow lane is retired as well:
  `bool_primary_vars` is the only raw-bool authority, and non-primary bools
  stay boxed in their main I64 variables. Native int-primary now means exact
  i64 representation, not semantic Python `int`; bounded add/sub and
  raw-closed counted store/load loop carriers may enter raw-primary only after
  shared interval proof, while unbounded arithmetic and shifts stay
  boxed/runtime-backed until range and shift-count proofs make raw lowering
  sound.
- Harden daemon, build, and harness workflows for multi-agent development.
- Move more hot semantics into runtime primitives and intrinsics.
- Split runtime/stdout/bootstrap feature surfaces through a shared
  `RuntimeSurfacePlan` so a tiny supported program does not link async,
  logging, filesystem/tempfile, GPU, networking, UI, or compatibility
  subsystems it cannot reach. Link-time dead stripping remains necessary but is
  not sufficient for five-year binary-size and cold-start targets; native and
  WASM must share one per-intrinsic/per-primitive reachability authority. The
  WASM non-reloc Auto path now uses emitted import lookups as the final
  retention authority and strips unreferenced imports after validation; the
  conservative dependency scan is reloc-only linker input, not a second
  non-reloc truth source.

### Long Term

- Broaden extension support through `libmolt`.
- Push native and WASM performance toward the project target.
- Make cold-start and binary-size gates as central as throughput gates across
  native, WASM browser/Node/Cloudflare, LLVM/MLIR, and Luau output surfaces.
- Continue converging on a larger practical CPython 3.12+ surface without
  regressing determinism or packaging guarantees.

## Active Blockers

- Incomplete same-contract parity between native and WASM for important surfaces.
- Windows native `stdlib_net` is target-gated through explicit no-net intrinsics
  until the WinSock target ABI lands as one coherent implementation: constants,
  sockaddr storage, resolver calls, socket ownership, SSL handle custody, and
  async poller readiness must share one authority before the target can claim
  native socket support.
- TIR DropInsertion is implemented and active on LLVM/WASM/Luau and native for
  the proven ExceptionRegion slice. The remaining blocker is structural:
  broaden shared drop/codegen facts until stale native value-tracking
  assumptions can be deleted across the full ownership surface. Pre-bail
  exception-only drop coverage is now represented separately from full-function
  `drop_inserted` RC ownership via `exception_region_drops_inserted`; no
  additional native exception-release map remains safe to delete. The next
  native deletion frontier is broader HandlerState/drop coverage that makes the
  remaining legacy value-tracking RC lanes redundant.
- Finalizer dispatch, standalone `__del__` exception isolation, scope-exit
  ordering, plain-object no-false-positive behavior, object-attribute release
  smoke, exit semantics, and explicit local `del` / `gc.collect()`
  resurrection-once are present. Container-owned release boundaries and the
  broader resurrection/leak matrix remain open ownership-boundary defects, not
  benchmark-only defects.
- ExceptionRegion / HandlerState ownership is not fully landed. Native
  Cranelift now consumes shared TIR DropInsertion releases from
  `ExceptionRegions`/CreationRef facts as ordinary `DecRef` ops, and the
  exception-specific native release side paths are deleted.
  Validator coverage and checked LLVM/WASM/Luau backend consumption evidence
  are present, and Luau plus LLVM runtime execute the generated raise/catch
  leak-loop artifact while `tools/wasm_diff.py` passes the same leak-loop
  differential for WASM; full closure still requires the wider `HandlerState`
  boundary and authoritative `bench_exception_heavy` speed evidence. The targeted native/LLVM
  `exception_raise_catch_loop_leak` gates are green under `MOLT_ASSERT_NO_LEAK=1`; the 2026-06-12 after-Luau-parity
  hot-only run was valid for cycle attribution but not authoritative for
  performance movement because the host was non-quiescent.
- Incomplete compatibility coverage across language and stdlib.
- Container/list/dict semantic dispatch is now represented through the shared
  representation plan; remaining work is storage-proof precision,
  backend-parity evidence, and deletion of any residual string-hint dispatch
  lanes that bypass structured `TirType` / `ContainerStorageKind` facts.
- Benchmark suite results are not yet consistently faster than CPython across
  all tracked lanes.
- Tiny native binaries currently have too much fixed linked runtime surface and
  measurable fresh-path startup cost for the five-year <50 ms / <2 MB target.
  The active path is a measured `RuntimeSurfacePlan` that drives native
  link-root selection, WASM import/export manifests, and intrinsic resolver
  generation from the same program reachability facts, not ad-hoc linker flag
  churn.
- The molt-gpu scheduler now binds `Movement` DAG operands as zero-copy
  source-storage views, executable renderers share ShapeTracker index/mask
  codegen for padded/masked reads, the runtime bridge passes full
  source-storage bytes by `buf_id`, and `Contiguous` DAG operands materialize
  through explicit copy kernels with fresh storage identity. The copy path now
  has a `bench_primitives` evidence row; contiguous CPU materialization is
  raw-copy class speed, shrunk/padded single-view materialization is raw-span
  speed, and flipped single-view materialization now uses the preflighted
  fixed-width reverse-copy path across 1/2/4/8-byte raw elements. Non-MXFP MLIR
  materialization now has positive flat-memref ShapeTracker lowering proof. MLIR
  pure elementwise compute now has positive real-memref loop proof with
  ShapeTracker-indexed input loads, masked-safe zero-fill, and explicit non-MXFP
  cast conversion selection plus lazy/scheduler/runtime target dtype custody.
  MIL has positive gather/select materialization proof with safe masked gather
  ordering for Bool, Int8/16/32, UInt8/16/32, Float16, and Float32 storage. CPU
	  typed scalar execution now proves non-f32 Cast/Bitcast values through terminal,
	  intermediate, and pre-reduce paths; runtime upload/readback has typed raw
	  storage APIs while the f32 public readback API fails closed on realized non-Float32
		  tensors; Metal now has raw non-f32 Cast/Bitcast byte proof; and reductions
		  now carry explicit `ReductionDomain` metadata through scheduler, fusion,
		  CPU, MIL ranked reduction lowering, shader codegen, and MLIR reduction
			  loops, including affine non-last-axis renderer, MIL ranked-axis proof, and
			  MLIR loop proof. Remaining GPU blockers are MIL BF16/64-bit/MXFP
				  materialization proof, MLIR MXFP block/exponent storage plus
				  `MaterializeCopy` lowering, MLIR MXFP quantized cast lowering, a
				  window/im2col primitive for tinygrad convolution, and typed
				  nonzero-pad semantics beyond the
				  now-runtime-backed constructor/zeros/readback, unary/binary/cast,
				  ternary `where`, explicit-axis reduce, Rust-owned all-axis reduce,
				  movement-family, and matmul-composition paths.
  Unsupported dialect paths stay fail-closed rather than treating a view/copy
  contract as a raw passthrough.
  TODO(perf, owner:runtime, milestone:RT, priority:P2, status:in-progress).

## Deferred By Policy

- Unrestricted `exec` / `eval` / `compile`.
- Runtime monkeypatching as a default compatibility strategy.
- Hidden host-CPython fallback paths in compiled binaries.
- Unrestricted reflection that violates Molt's AOT constraints.
- The runpy dynamic-lane expected failures list is currently empty because
  supported lanes moved to intrinsic support; unsupported runpy dynamic
  execution remains policy-governed rather than represented by an active
  expected-failure lane.
