# Documentation Index

Molt targets **Python 3.12+** semantics only. Documentation is navigation and
status memory; the live codebase, executable tests, and generated evidence are
the source of truth.

## Start Here

- Project overview: [README.md](../README.md)
- First install and run: [getting-started.md](getting-started.md)
- Current supported state: [spec/STATUS.md](spec/STATUS.md)
- Forward plan: [../ROADMAP.md](../ROADMAP.md)

## Core Navigation

- Canonical reading list: [CANONICALS.md](CANONICALS.md)
- Developer guide: [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
- Operations: [OPERATIONS.md](OPERATIONS.md)
- Multi-agent verification coordination: [ops/MULTI_AGENT_COORDINATION.md](ops/MULTI_AGENT_COORDINATION.md)
- Spec index: [spec/README.md](spec/README.md)
- Compatibility architecture: [spec/areas/compat/README.md](spec/areas/compat/README.md)
- Import/bootstrap, external package admission, native extension sidecar custody, backend-IR direct-call closure, shared-stdlib cache, and public importlib transaction authority: [spec/areas/compat/contracts/import_system_contract.md](spec/areas/compat/contracts/import_system_contract.md)
- WASM optimization and import-retention authority: [spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md](spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md)

## Debugging

- Canonical debug operations and artifact roots: [OPERATIONS.md](OPERATIONS.md)
- Memory-guard custody for tests, current-test incident diagnostics, and cleanup diagnostics: [OPERATIONS.md](OPERATIONS.md)
- Backend daemon identity custody and verified signal authority (`src/molt/backend_daemon_custody.py`): [OPERATIONS.md](OPERATIONS.md)
- Architecture and ownership map for the debug surface: [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
- Public debug command family: `molt debug repro|ir|verify|trace|reduce|bisect|diff|perf`

## Product And Proof

- Compatibility corpus manifest: [COMPATIBILITY_CORPUS_MANIFEST.md](COMPATIBILITY_CORPUS_MANIFEST.md)
- Ordinary class constructor and `__new__` parity status: [spec/STATUS.md](spec/STATUS.md)
- Standalone binary proof workflow: [proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
- Benchmarking guide: [BENCHMARKING.md](BENCHMARKING.md)
- Detailed benchmark report: [benchmarks/bench_summary.md](benchmarks/bench_summary.md)
- Ecosystem compatibility matrix and NumPy source-recompiled extension package/native artifact publication custody: [spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md](spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md)
- GPU primitive stack architecture, public tinygrad shim custody, and runtime-handle surface: [architecture/gpu-primitive-stack.md](architecture/gpu-primitive-stack.md)
- Active GPU parallelism, MLIR, runtime-handle integration, and tinygrad off-the-shelf friend-suite driver: [spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md](spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md)
- Backend LIR representation evidence: [spec/areas/compiler/backend_lir_representation.generated.md](spec/areas/compiler/backend_lir_representation.generated.md)
- Luau backend optimization and generated support: [spec/areas/compiler/LUAU_BACKEND_OPTIMIZATION.md](spec/areas/compiler/LUAU_BACKEND_OPTIMIZATION.md), [spec/areas/compiler/luau_support_matrix.generated.md](spec/areas/compiler/luau_support_matrix.generated.md)
- Stdlib intrinsic audit and platform availability: [spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md](spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md), [spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md](spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md)

## Compiler Foundation Routing

- Integrated foundation program: [design/foundation/00_integrated_parallel_program.md](design/foundation/00_integrated_parallel_program.md)
- RC ownership and drop insertion: [design/foundation/20_rc-ownership-drop-insertion.md](design/foundation/20_rc-ownership-drop-insertion.md)
- Perceus-style borrow inference: [design/foundation/27_perceus_borrow_inference.md](design/foundation/27_perceus_borrow_inference.md)
- ExceptionRegion ownership, shared TIR facts, shared drop artifacts, backend parity evidence, and HandlerState frontier: [design/foundation/45_exception_region_ownership.md](design/foundation/45_exception_region_ownership.md)
- Current module-scope control-flow binding status: [spec/STATUS.md](spec/STATUS.md)
- Codebase decomposition program: [design/foundation/21_decomposition_program.md](design/foundation/21_decomposition_program.md)
- Parallel build, crate extraction, incremental cache, and compiler-throughput architecture: [design/parallel_build_architecture.md](design/parallel_build_architecture.md), [architecture/compilation-model.md](architecture/compilation-model.md)
- Op-kind registry and generated dispatch direction: [design/foundation/25_op_kind_registry.md](design/foundation/25_op_kind_registry.md)

## Workspace Guides

- Examples: [../examples/README.md](../examples/README.md)
- Demo app: [../demo/README.md](../demo/README.md)
- Bench harnesses: [../bench/README.md](../bench/README.md), [../bench/friends/README.md](../bench/friends/README.md)
- Packaging and release: [../packaging/README.md](../packaging/README.md), [../packaging/templates/linux/README.md](../packaging/templates/linux/README.md)

## Planning

- Active roadmap: [../ROADMAP.md](../ROADMAP.md)
- Planning authority manifest: [design/foundation/authority_manifest.toml](design/foundation/authority_manifest.toml)
- Long-horizon north star and compression ladder: [design/foundation/51_ten_year_roadmap.md](design/foundation/51_ten_year_roadmap.md)
- Autonomous operating doctrine and 5/10/50-year gap map: [design/foundation/52_autonomous_operating_charter.md](design/foundation/52_autonomous_operating_charter.md)
- 90-day execution slice: [ROADMAP_90_DAYS.md](ROADMAP_90_DAYS.md)
- Roadmap archive: [ROADMAP.md](ROADMAP.md)
