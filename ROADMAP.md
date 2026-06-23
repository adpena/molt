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
   `tools/gen_intrinsics.py` now emits rustfmt-stable generated Rust, skips
   exact-content no-op writes before invoking rustfmt, lazy-loads memory-guard
   formatting custody only when a changed Rust file needs formatting, and
   formats changed generated Rust through `MOLT_GENERATOR`. Resolver bodies
   live in generated per-category modules under
   `runtime/molt-runtime/src/intrinsics/generated_resolvers/`, leaving
   `generated.rs` as the single parser-facing intrinsic manifest table.
   The `html` and `unicodedata` text domains plus `zoneinfo` now use extracted
   runtime leaves as their only implementation authorities, with the in-facade
   fallback modules deleted and resolver arms gated by `stdlib_text` /
   `stdlib_zoneinfo`.
   `stringprep` now owns the first generated per-crate intrinsic sub-registry;
   the next registry throughput step is moving the remaining generated
   categories behind the same thin `molt-runtime` facade composition.
   `tools/perf_inner_repeat.py` plus its
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
    current guard to terminate its own child tree. The low-level guard now
    captures a pre-launch PGID baseline and, after direct PID-lineage teardown,
    drains only new orphaned Molt-owned process groups whose parents are init or
    already selected for cleanup; keep this as the required shape for future
    timeout/interruption cleanup so reparented `molt.cli build`/`molt-backend`
    workers cannot survive while concurrent Claude/Codex/control-plane groups
    remain protected. Guard summary paths must be evidence-producing before the
    child launch: `status: "running"` summaries carry repro command, resolved
    limits, guard identity, and bounded host/control-plane samples, and parent
    SIGTERM/SIGINT/SIGHUP rewrites the same path with `guard_interrupted` before
    exit. Every sentinel trip/drain/stale-preflight path must preserve sampled
    processes,
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
	    authoritative speed evidence remain the parity closeout. The 2026-06-15
	    local parity rerun passed the all-backend `exception_region`,
	    `shared_drop`, and `exception_pop` Rust filters plus the focused WASM LIR
	    `DelBoundary` release regression and a root `dev-fast` backend build; the
	    same continuation proved marker-only `drop_inserted` facts survive
	    pass-manager snapshot/restore as first-class fact changes and that
	    frontend JSON preserves `bound_local` for list/tuple/dict/set/frozenset
	    absorbing constructors; the
	    same-day targeted hot-only `bench_exception_heavy` attempt refused before
	    sampling and moved no speed claim. CLI rebuild/cache policy now treats shared exact-key
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
    `MOLT_BACKEND_BATCH_OP_BUDGET` authority as stdlib batching, and the
    production self-spawn worker path is covered by
    `cargo test -p molt-backend --test native_batch_worker_spawn`
    (`tmp/memory_guard/cargo_test_native_batch_worker_spawn_cleanup_diag_20260615.json`):
    the real `molt-backend` binary compiles two live functions as two
    materialized batches through `--native-batch-job-file`. Daemon-off
    proof now builds the full-stdlib adapter and reaches upstream tinygrad
    runtime execution under guard. The importlib bootstrap export, list-clear
    detach, namedtuple return-boundary ownership, defaultdict factory-handle
    ownership, deque retained-handle ownership, descriptor-cache retained
    snapshots, and descriptor-bind reentrant class-dict mutation custody
    supersede both the older 1.985 GB invalid-header receipt and the fresh
    `graph_rewrite` invalid-header receipt as current blockers. Fresh
    2026-06-20 guarded
    evidence builds the full-stdlib adapter, gets past the
    `tinygrad/uop/ops.py:1586` teardown invalid-header abort, fixes the
    post-JSON `argparse.Namespace` return-cleanup double drop, and makes direct
    execution of the rebuilt adapter exit cleanly for all four default
    public-API workloads. The official `tinygrad_off_the_shelf` Molt friend
    runner with clean pinned source custody now fails closed at
    `tinygrad/uop/upat.py:167`, where upstream `upat_compile` calls
    `exec(code_str, globs, namespace)`. The current remaining work is a static
    AOT-compatible path for tinygrad's lazy pattern compiler. The producer side
    now exists as `tools/tinygrad_upat_static_exec_registry.py`, which captures
    deterministic UPat matcher sources from the pinned checkout and emits a
    fail-closed generated factory registry without runtime `exec`; runtime/build
    graph consumption is still open. Prior failure artifact:
    `bench/results/friends/2026-06-20-tinygrad-origin-fix-rerun/`.
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
  The 2026-06-15 local rerun passed the all-backend Rust consumption filters
  (`exception_region`, `shared_drop`, `exception_pop`), the focused WASM LIR
  `DelBoundary` release regression, and a root `dev-fast` backend build. The
  same continuation proved marker-only `drop_inserted` facts survive
  pass-manager snapshot/restore as first-class fact changes and that frontend
  JSON preserves `bound_local` for list/tuple/dict/set/frozenset absorbing
  constructors. The
  same-day hot-only `bench_exception_heavy` rerun was non-authoritative and
  refused during the size phase, so the performance status remains unchanged.
  The 2026-06-20 direct release-fast WASM backend replay of the saved full
  stdlib handoff IR now proves the active-frame ExceptionRegions rule: implicit
  `TryStart`/`CheckException` handler ownership is created only when the target
  label is active in the lexical exception frame stack. The previously fatal
  `_collections_abc__Sequence_index` analysis completes with zero MatchRef
  release facts at 259.1 MiB, and the guarded backend replay exits 0 under the
  same 12 GiB process cap with a 0.324 GiB rusage peak.
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
- Finish finalizer ownership boundaries: the native scope-exit ordering
  differential, unit-level direct-field/runtime finalizer guards, shared
  `DeleteVar` old-slot release boundary, and frontend `bound_local` carrier for
  list/tuple/dict/set/frozenset absorbing constructors are green. The
  2026-06-15 focused native differential shard now passes
  `finalizer_scope_exit_ordering.py`, `finalizer_object_attr_release.py`,
  `finalizer_matrix.py`, `finalizer_container_clear.py`, and
  `finalizer_standalone_raise_swallow.py`
  (`tmp/diff/finalizer_reaudit_after_borrowed_self.json`,
  `logs/finalizer_reaudit_after_borrowed_self.log`) after deleting the legacy
  compiled-`__init__` extra `self` retain in type-call dispatch. Remaining work
  is widening the same finalizer-ordering proof across backend/profile parity
  and deleting any stale value-tracking lanes once shared ownership facts cover
  them.
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
  keep the runner on `{project_python} -m molt.cli run` with the full stdlib
  build profile forwarded, and retire the current upstream lazy-pattern compiler
  blocker without widening Molt's AOT contract. Earlier evidence got past
  manifest skip and full-profile validation, then killed
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
  `MOLT_BACKEND_BATCH_OP_BUDGET` authority as stdlib batching, and the
  production self-spawn worker path is covered by
  `cargo test -p molt-backend --test native_batch_worker_spawn`
  (`tmp/memory_guard/cargo_test_native_batch_worker_spawn_cleanup_diag_20260615.json`):
  the real `molt-backend` binary compiles two live functions as two materialized
  batches through `--native-batch-job-file`. Daemon-off proof
  now builds the full-stdlib adapter and reaches runtime execution under guard;
  a 2026-06-15 list-workloads smoke
  (`tmp/memory_guard/tinygrad_importlib_module_from_spec_smoke.json`) timed out
  after 900s with `violation=null`, no orphaned process groups, 3.75 GB peak
  process-tree RSS, and Cargo incremental quarantine while compiling the
  full-stdlib tinygrad adapter. The active IR for that lane was 49 MB with
  5,845 functions and 866,671 ops, so classify this result as cold
  build/compiler-throughput evidence before adapter workload enumeration, not a
  tinygrad semantic failure. Direct guarded backend replays of that IR
  (`tmp/memory_guard/tinygrad_backend_replay_indexed_20260615.json` and
  `tmp/memory_guard/tinygrad_backend_replay_indexed_scratch_20260615.json`)
  both detected 1,469 leaf functions and failed closed before object emission
  because `MOLT_RUNTIME_INTRINSIC_SYMBOLS` was absent; their 0.891 GB and
  0.887 GB peak RSS receipts are backend compile-memory evidence only. A later
  lazy-index guarded list-workloads retry
  (`tmp/memory_guard/tinygrad_adapter_list_workloads_lazy_index_20260615.json`)
  still timed out in the full-stdlib adapter build after 1200s with
  `violation=null`, no orphaned process groups, 1.34 GB peak process RSS, and
  2.28 GB peak process-tree RSS; the post-run sentinel receipt
  (`tmp/memory_guard/process_sentinel_after_lazy_index_20260615.json`) returned
  0 with no incident or orphaned process groups. The older 1.985 GB
  invalid-header receipt is now historical: after the importlib bootstrap
  export, list-clear detach, namedtuple return-boundary ownership, defaultdict
  factory-handle ownership, and deque retained-handle ownership fixes, fresh
  2026-06-20 guarded evidence builds the full-stdlib adapter, gets past the
  `tinygrad/uop/ops.py:1586` teardown invalid-header abort, and fixes the
  post-JSON `argparse.Namespace` return-cleanup double drop. Direct execution
  of the rebuilt adapter now exits cleanly for all four default public-API
  workloads. The official `tinygrad_off_the_shelf` Molt friend runner with
  clean pinned source custody reached upstream tinygrad's lazy pattern compiler
  at `tinygrad/uop/upat.py:167`, where `upat_compile` calls
  `exec(code_str, globs, namespace)`. Unrestricted `exec()` is outside Molt's
  verified AOT subset. The static materialization producer
  `tools/tinygrad_upat_static_exec_registry.py` is now wired into the friend
  manifest as a prepare step; its generated
  `_molt_tinygrad_upat_static_exec_registry` module is admitted beside
  `tinygrad` in the Molt static-package lane, and the adapter installs
  `exec_static` as the package-scoped `tinygrad.uop.upat.exec` global. The next
  blocker is fresh guarded runner evidence for that wired registry path. Prior
  failure artifact:
  `bench/results/friends/2026-06-20-tinygrad-origin-fix-rerun/`.
  The native benchmark/friend-suite Molt result contract records phase-aware
  `molt_failure` payloads, so reruns distinguish build-phase daemon details
  such as `backend_daemon_empty_response` from run-phase runtime details such as
  `molt_runtime_invalid_object_header_before_dec_ref` instead of collapsing
  them into generic failed rows.
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
			  The Molt runner executes the correct `{project_python} -m molt.cli run`
			  full-stdlib command by default. Earlier daemon-custody evidence reached
			  the backend daemon and no longer tripped the outer memory guard after
			  daemon full-request custody closed the hidden one-shot overlap, but still
			  failed before adapter workload execution through lost-daemon /
			  empty-response outcomes; the separate 21:12 sidecar recorded a
			  Molt-owned backend daemon RSS guard trip near the 12 GB process cap.
			  Current 2026-06-20 evidence builds the full-stdlib adapter and now fails
			  closed at upstream `tinygrad/uop/upat.py:167`, where `upat_compile`
			  requires unrestricted `exec(code_str, globs, namespace)`, outside Molt's
			  verified AOT subset. `tools/tinygrad_upat_static_exec_registry.py` now
			  emits the deterministic static-registry producer for those matcher
			  sources; consuming that registry in static package lowering/runtime
			  dispatch remains the active blocker. `tools/bench_friends.py`
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
  Shared stdlib and native object cache identity now also includes resolved
  capability config, capability-manifest runtime env, and ambient
  `MOLT_CAPABILITIES`/resource/audit/IO env that can be present in per-file
  differential runs. The Rust-owned import transaction now exists for the
  active importlib and `builtins.__import__` runtime paths. The public
  `molt_importlib_import_module(name, package)` intrinsic remains the
  CPython-public `importlib.import_module` API wrapper: it owns public argument
  validation and relative-name resolution, then delegates into the same
  resolved transaction core as `molt_importlib_import_transaction`. The empty
  `_MODULE_ALIASES` branch is gone, and frontend literal/direct-call folding of
  `importlib.import_module("literal")` now emits the public
  `molt_importlib_import_module(name, None)` wrapper whenever callable identity
  and an absolute literal name are statically proven; runtime import owns target
  availability, version-gated absence, module cache custody, provenance,
  fromlist behavior, and error shape. Frontend module-attribute
  mutation tracking now refuses both the transaction fold and cross-module static
  direct-call lowering whenever `importlib.import_module` is rebound through
  `importlib` or a module alias. Build-time module graph discovery now mirrors
  that callable-identity model for `importlib`, `importlib as alias`, and
  `from importlib import import_module as alias`, while refusing static target
  collection after `import_module` rebinding. Module graph resolution now requires exact
  filesystem casing, known project modules authorize only exact graph members,
  and ordinary source syntax imports now call the same transaction intrinsic
  with explicit `name`/`fromlist`/`level` payloads while bootstrap/importlib
  modules keep a private cycle-breaking `MODULE_IMPORT` boundary, and the
  focused native shard now preserves `_bootstrap` / `_bootstrap_external`
  submodule identity. Graph-proven `fromlist` child auto-import/binding now
  lives inside the Rust transaction for the focused native path, preserving
  package exports and binding successful child modules on the parent before the
  final `IMPORT_FROM`; a focused rerun also proves that child body dependency
  failures stay pending through active handler frames instead of being cleared as
  absent child modules. Static package `__all__` child modules named by source
  `from package import *` now flow through the import scan, persisted
  module-analysis cache, dependency graph, and Rust transaction `fromlist=["*"]`
  path before star binding; unresolved `__all__` names stay runtime-visible and
  raise the normal star-binding `AttributeError`. Covered evidence is the runtime
  `from_import_child_missing_clear_preserves_unrelated_pending_failure_in_handler`
  unit, the seven-case native import transaction/fromlist slice, the current
  10-file full-profile importlib transaction differential shard
  (`logs/importlib_10_diff_full.log`,
  `logs/importlib_10_diff_full_results.jsonl`), and the four-file full-profile
  differential active transaction/fromlist slice. The static package `__all__`
  star-child evidence is
  `tests/cli/test_cli_import_collection.py::test_from_import_star_graph_admits_static_all_child_module`,
  `tests/test_native_import_star_all_regressions.py`,
  `logs/import_star_package_all_child_pair_diff.log`,
  `logs/import_star_package_all_child_pair_diff_results.jsonl`,
  `logs/importlib_transaction_fromlist_star_regression_diff.log`, and
  `logs/importlib_transaction_fromlist_star_regression_diff_results.jsonl`. The
  relative `builtins.__import__` package-context path now matches the covered
  CPython 3.12 order for dict-required `globals`, `__package__`,
  `__spec__.parent`, missing-parent `AttributeError`, `__name__` fallback,
  package `__path__`, and no-known-parent errors; evidence is
  `tests/test_native_import_package_context_regressions.py`,
  `tests/differential/basic/import_dunder_package_context.py`,
  `logs/import_dunder_package_context_diff.log`, and
  `logs/import_dunder_package_context_diff_results.jsonl`. Public resolver
  validation for `importlib.import_module` and `importlib.util.resolve_name`
  now shares private Rust relative-name math while preserving CPython 3.12
  API-specific errors for non-string names/packages, missing packages, empty
  names, and beyond-top-level relative imports; evidence is
  `tests/test_native_importlib_public_api_regressions.py`,
  `tests/differential/stdlib/importlib_public_api_validation.py`,
  `logs/importlib_public_api_validation_diff.log`, and
  `logs/importlib_public_api_validation_diff_results.jsonl`. `FileLoader` and
  `SourceFileLoader.load_module` now use the shared Rust spec-execution
  transaction for materialization, `sys.modules` preinsert, failed-new-load
  rollback, existing-module no-rollback behavior, and successful substitution
  return selection; evidence is
  `tests/test_native_importlib_load_module_transaction.py`,
  `tests/differential/stdlib/importlib_load_module_transaction.py`,
  `logs/importlib_load_module_transaction_diff.log`,
  `logs/importlib_load_module_transaction_diff_results.jsonl`,
  `logs/importlib_spec_execution_transaction_regression_diff.log`, and
  `logs/importlib_spec_execution_transaction_regression_diff_results.jsonl`. The four-file
  transaction/fromlist differential slice intentionally fails closed under the
  micro profile when full-profile stdlib features are required. The remaining
  structural import work is to continue
  public API validation beyond the covered import-module/resolve-name resolver
  and load-module/bootstrap-submodule cases while sharing the private resolver,
  continue collapsing manual `exec_module`/namespace spec execution into the
  same Rust transaction, and separate semantic failures from full-profile native
  compile-throughput failures. The latest guarded `importlib.util.module_from_spec`
  external-module proof reached the harness 600s build timeout with
  `violation=null` and no orphaned process groups, so its current blocker is
  build/DX throughput rather than runtime importlib semantics. The separate
  full-profile native `threading` import split-transport blocker is closed: the
  SimpleIR megafunction splitter now treats cloned suffix cleanup handlers as
  dataflow-custody edges, cloning only when external reads are available from
  the extracted chunk or split frame and failing closed instead of stripping
  external `check_exception` targets. Guarded proof passed
  `test_native_full_profile_import_threading_survives_split_frame_transport`
  for default, `1000`, and `500` split limits
  (`logs/memory_guard_native_threading_all_summary.json`; `violation=null`, no
  orphaned groups, 2.41 GB peak process-tree RSS). Continue to close
  dynamic/broader `fromlist` star/`__all__` plus namespace-package edge
  semantics under the same transaction authority.
- Establish baseline and ratchet artifacts for tiny hello-world output rows
  across native, linked-WASM, Luau, and MLIR: release/dev size, backend/profile
  dimensions, same-path startup where runnable, fresh-path startup where
  runnable, CPython process baseline, and tiny C baseline. WASM already proves
  that tiny payloads are possible under the right linked/profile discipline;
  native must converge by making runtime reachability as precise as the WASM
  import/export path rather than relying on coarse domain features alone.
- Luau source emission now runs through the shared TIR module phase before text
  codegen: per-function TIR pipeline, `run_module_pipeline`, and fail-closed
  SimpleIR back-conversion. Keep the generated Luau support matrix current and
  use it to prioritize post-E1 runtime, surface, and CPython-vs-Luau feature-gap
  closure. `checked_add` is now listed as `implemented-exact` and guarded by
  checked helper emission.
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
  transient extraction side table. The type-refine solver treats produced values
  as `Never` until solved, recomputes op results from opcode, operands, and
  structural attrs each round, widens known-dynamic results to `DynBox`, and
  fails closed on nonconvergence instead of freezing oscillating values through a
  stderr fallback. Range/list devirtualization also records the
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
- Finalizer dispatch has unit-level runtime and direct-field guards, the native
  scope-exit ordering differential passes, and the shared `DeleteVar` old-slot
  release boundary is covered. The 2026-06-15 focused native differential shard
  now passes standalone raising-finalizer isolation, object-attribute release,
  container clear/pop/delete finalizers, and the finalizer matrix after removing
  the obsolete compiled-constructor `self` retain; remaining finalizer work is
  backend/profile parity and stale value-tracking deletion, not benchmark-only
  proof.
- ExceptionRegion / HandlerState ownership is not fully landed. Native
  Cranelift now consumes shared TIR DropInsertion releases from
  `ExceptionRegions`/CreationRef facts as ordinary `DecRef` ops, and the
  exception-specific native release side paths are deleted.
  Validator coverage and checked LLVM/WASM/Luau backend consumption evidence
  are present, and Luau plus LLVM runtime execute the generated raise/catch
  leak-loop artifact while `tools/wasm_diff.py` passes the same leak-loop
  differential for WASM; full closure still requires the wider `HandlerState`
  boundary and authoritative `bench_exception_heavy` speed evidence. The
  2026-06-20 active-frame regression closes the false-owner/RSS cliff where
  inactive universal `CheckException` handler targets manufactured MatchRef
  obligations after the protected region had closed; this is coverage for the
  proven shared slice, not completion of the wider HandlerState boundary. The
  targeted native/LLVM `exception_raise_catch_loop_leak` gates are green under
  `MOLT_ASSERT_NO_LEAK=1`; the 2026-06-12 after-Luau-parity
  hot-only run was valid for cycle attribution but not authoritative for
  performance movement because the host was non-quiescent. The 2026-06-15
  hot-only rerun also moved no performance claim because it refused before
  sampling.
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
