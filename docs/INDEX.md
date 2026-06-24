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
- Stdlib intrinsic loading/default metadata contract: [spec/areas/runtime/0031_STDLIB_INTRINSICS_LOADING.md](spec/areas/runtime/0031_STDLIB_INTRINSICS_LOADING.md)

## Compiler Foundation Routing

- Integrated foundation program: [design/foundation/00_integrated_parallel_program.md](design/foundation/00_integrated_parallel_program.md)
- RC ownership and drop insertion: [design/foundation/20_rc-ownership-drop-insertion.md](design/foundation/20_rc-ownership-drop-insertion.md)
- Perceus-style borrow inference: [design/foundation/27_perceus_borrow_inference.md](design/foundation/27_perceus_borrow_inference.md)
- ExceptionRegion ownership, shared TIR facts, shared drop artifacts, backend parity evidence, and HandlerState frontier: [design/foundation/45_exception_region_ownership.md](design/foundation/45_exception_region_ownership.md)
- Current module-scope control-flow binding status: [spec/STATUS.md](spec/STATUS.md)
- Codebase decomposition program: [design/foundation/21_decomposition_program.md](design/foundation/21_decomposition_program.md), including the crate-graph per-move execution spec [21f](design/foundation/21f_crate_graph_smove_execution_specs.md)
- Parallel build, crate extraction, incremental cache, and compiler-throughput architecture: [design/parallel_build_architecture.md](design/parallel_build_architecture.md), [architecture/compilation-model.md](architecture/compilation-model.md)
- Op-kind registry and generated dispatch/classifier direction: [design/foundation/25_op_kind_registry.md](design/foundation/25_op_kind_registry.md)
- Generator fusion and deforestation routing: [design/generator_fusion.md](design/generator_fusion.md)

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
- 100-year portfolio route cluster: throughput and async [54](design/foundation/54_throughput_concurrency_async.md), ownership lattice [55](design/foundation/55_memory_safety_ownership_lattice.md), DX/build speed [56](design/foundation/56_dx_buildspeed_tooling.md), UX/diagnostics [57](design/foundation/57_ux_cli_errors_onboarding.md), killer demos [58](design/foundation/58_killer_demos.md), semantic fact plane [59](design/foundation/59_semantic_fact_plane.md), whole-program DCE [60](design/foundation/60_tree_shaking_whole_program_dce.md), size plane [61](design/foundation/61_binary_size_and_output_optimization.md), cold start [62](design/foundation/62_startup_cold_start.md), deforestation/fusion [63](design/foundation/63_deforestation_fusion.md), scoreboards [64](design/foundation/64_perf_scoreboards_and_harness.md), perf compression [65](design/foundation/65_perf_compression_ladder.md), CPython parity [66](design/foundation/66_compat_cpython_parity.md), tinygrad/DFlash fidelity [67](design/foundation/67_compat_tinygrad_dflash.md), and ShapeFacts class layout [68](design/foundation/68_shapefacts_rung4_class_layout.md).
- 90-day execution slice: [ROADMAP_90_DAYS.md](ROADMAP_90_DAYS.md)
- Roadmap archive: [ROADMAP.md](ROADMAP.md)
