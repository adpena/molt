# Canonical Reading List

These are the must-read docs for anyone changing Molt. Treat them as routing
surfaces; live code, executable tests, and generated evidence remain the source
of truth when a claim drifts.

## Start Here

- Project overview: [README.md](../README.md)
- First install and run: [getting-started.md](getting-started.md)
- Current supported state: [spec/STATUS.md](spec/STATUS.md)
- Forward plan: [../ROADMAP.md](../ROADMAP.md)

## Core Engineering

- [../AGENTS.md](../AGENTS.md)
- [../CONTRIBUTING.md](../CONTRIBUTING.md)
- [ROOT_LAYOUT.md](ROOT_LAYOUT.md)
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
- [OPERATIONS.md](OPERATIONS.md)
- [ops/MULTI_AGENT_COORDINATION.md](ops/MULTI_AGENT_COORDINATION.md)
- [INDEX.md](INDEX.md)

## Scope And Contracts

- [spec/areas/core/0000-vision.md](spec/areas/core/0000-vision.md)
- [spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md)
- [spec/areas/compat/README.md](spec/areas/compat/README.md)
- [spec/areas/compat/surfaces/language/language_surface_matrix.md](spec/areas/compat/surfaces/language/language_surface_matrix.md)
- [spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md](spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md)
- [spec/areas/compat/surfaces/c_api/c_api_surface_index.md](spec/areas/compat/surfaces/c_api/c_api_surface_index.md)

## Compiler Foundation

- [design/foundation/00_integrated_parallel_program.md](design/foundation/00_integrated_parallel_program.md)
- [design/foundation/20_rc-ownership-drop-insertion.md](design/foundation/20_rc-ownership-drop-insertion.md)
- [design/foundation/21_decomposition_program.md](design/foundation/21_decomposition_program.md)
- [design/parallel_build_architecture.md](design/parallel_build_architecture.md)
- [architecture/compilation-model.md](architecture/compilation-model.md)
- [architecture/gpu-primitive-stack.md](architecture/gpu-primitive-stack.md)
- [spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md](spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md)
- [design/foundation/25_op_kind_registry.md](design/foundation/25_op_kind_registry.md)
- [design/foundation/27_perceus_borrow_inference.md](design/foundation/27_perceus_borrow_inference.md)
- [design/foundation/45_exception_region_ownership.md](design/foundation/45_exception_region_ownership.md)

## Long-Horizon Operating Doctrine

- [design/foundation/authority_manifest.toml](design/foundation/authority_manifest.toml)
- [design/foundation/51_ten_year_roadmap.md](design/foundation/51_ten_year_roadmap.md)
- [design/foundation/52_autonomous_operating_charter.md](design/foundation/52_autonomous_operating_charter.md)
- Portfolio route cluster: [54 throughput/concurrency](design/foundation/54_throughput_concurrency_async.md), [55 ownership lattice](design/foundation/55_memory_safety_ownership_lattice.md), [56 DX/build speed](design/foundation/56_dx_buildspeed_tooling.md), [57 UX/diagnostics](design/foundation/57_ux_cli_errors_onboarding.md), [58 killer demos](design/foundation/58_killer_demos.md), [59 semantic fact plane](design/foundation/59_semantic_fact_plane.md), [60 whole-program DCE](design/foundation/60_tree_shaking_whole_program_dce.md), [61 size plane](design/foundation/61_binary_size_and_output_optimization.md), [62 cold start](design/foundation/62_startup_cold_start.md), [63 deforestation/fusion](design/foundation/63_deforestation_fusion.md), [64 scoreboards](design/foundation/64_perf_scoreboards_and_harness.md), [65 perf compression](design/foundation/65_perf_compression_ladder.md), [66 CPython parity](design/foundation/66_compat_cpython_parity.md), and [67 tinygrad/DFlash fidelity](design/foundation/67_compat_tinygrad_dflash.md).

## Proof And Validation

- [BENCHMARKING.md](BENCHMARKING.md)
- [../bench/friends/README.md](../bench/friends/README.md)
- [COMPATIBILITY_CORPUS_MANIFEST.md](COMPATIBILITY_CORPUS_MANIFEST.md)
- [proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
- [spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
- [spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md)
