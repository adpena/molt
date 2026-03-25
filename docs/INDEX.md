# Documentation Index

Molt targets **Python 3.12+** semantics only. Do not spend effort on <=3.11.

## Start Here
- [AGENTS.md](../AGENTS.md)
- [CANONICALS.md](CANONICALS.md)
- [CONTRIBUTING.md](../CONTRIBUTING.md)
- [ROOT_LAYOUT.md](ROOT_LAYOUT.md)
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
- [OPERATIONS.md](OPERATIONS.md)
- Platform pitfalls: [README.md](../README.md) (quickstart), [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md), and [OPERATIONS.md](OPERATIONS.md).

## Navigation Hubs
- Project quickstart + top-level status: [README.md](../README.md)
- Repo root contract: [ROOT_LAYOUT.md](ROOT_LAYOUT.md)
- Operator support snapshot: [SUPPORTED.md](../SUPPORTED.md)
- Canonical documentation index: [INDEX.md](INDEX.md)
- Engineering architecture notes: [architecture/](architecture/)
- Full spec index (all spec areas): [spec/README.md](spec/README.md)
- Compatibility corpus manifest: [COMPATIBILITY_CORPUS_MANIFEST.md](COMPATIBILITY_CORPUS_MANIFEST.md)
- Standalone binary proof workflow: [proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
- Differential test organization and run ledger: [tests/differential/INDEX.md](../tests/differential/INDEX.md)

## Workspace Guides
- Examples: [examples/README.md](../examples/README.md)
- Demo app and offload workflow: [demo/README.md](../demo/README.md)
- Benchmark harnesses: [bench/README.md](../bench/README.md), [bench/friends/README.md](../bench/friends/README.md)
- Packaging/release guides: [packaging/README.md](../packaging/README.md), [packaging/templates/linux/README.md](../packaging/templates/linux/README.md)

## Vision, Scope, Status
- 0000 Vision: [spec/areas/core/0000-vision.md](spec/areas/core/0000-vision.md)
- 0025 Reproducible And Deterministic Mode: [spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md](spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md)
- 0800 What Molt Is Willing To Break: [spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md)
- STATUS: [spec/STATUS.md](spec/STATUS.md)
- [ROADMAP.md](../ROADMAP.md) (active roadmap)
- [ROADMAP_90_DAYS.md](ROADMAP_90_DAYS.md)
- [Detailed roadmap archive](ROADMAP.md)

## Architecture and Runtime
- 0002 Architecture: [spec/areas/core/0002-architecture.md](spec/areas/core/0002-architecture.md)
- 0003 Runtime: [spec/areas/runtime/0003-runtime.md](spec/areas/runtime/0003-runtime.md)
- 0024 Runtime State Lifecycle: [spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md](spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md)
- 0026 Concurrency And GIL: [spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)
- 0027 Runtime Architecture Map: [spec/areas/runtime/0027_RUNTIME_ARCHITECTURE_MAP.md](spec/areas/runtime/0027_RUNTIME_ARCHITECTURE_MAP.md)
- 0004 Tiers: [spec/areas/core/0004-tiers.md](spec/areas/core/0004-tiers.md)
- 0191 Deopt And Guard Model: [spec/areas/compiler/0191_DEOPT_AND_GUARD_MODEL.md](spec/areas/compiler/0191_DEOPT_AND_GUARD_MODEL.md)

## Compiler and Lowering
- 0019 Bytecode Lowering Matrix: [spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md)
- 0190 Lowering Rules: [spec/areas/compiler/0190_LOWERING_RULES.md](spec/areas/compiler/0190_LOWERING_RULES.md)
- 0192 Idioms And Semantic Patterns: [spec/areas/compiler/0192_IDIOMS_AND_SEMANTIC_PATTERNS.md](spec/areas/compiler/0192_IDIOMS_AND_SEMANTIC_PATTERNS.md)

## Compatibility and Stdlib
- Compatibility architecture index: [spec/areas/compat/README.md](spec/areas/compat/README.md)
- Language surface index: [spec/areas/compat/surfaces/language/language_surface_matrix.md](spec/areas/compat/surfaces/language/language_surface_matrix.md)
- Stdlib surface index: [spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md](spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md)
- Stdlib surface matrix: [spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)
- C-API surface index: [spec/areas/compat/surfaces/c_api/c_api_surface_index.md](spec/areas/compat/surfaces/c_api/c_api_surface_index.md)
- C-API v0 bootstrap symbol list (including scalar/object-bytes/array constructors, type/module parity wrappers, runtime-owned module-state registries, and expanded scan-driven CPython-compat shim coverage via `include/Python.h` for parse/call/memory/thread/type helpers, selected `PyType_Spec` slot lowering + `METH_STATIC` support, and `PyType_FromModuleAndSpec`/`PyType_GetModule*`): [spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md](spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md)
- Stdlib lowering execution plan: [spec/areas/compat/plans/stdlib_lowering_plan.md](spec/areas/compat/plans/stdlib_lowering_plan.md)
- Tkinter lowering execution plan: [spec/areas/compat/plans/tkinter_lowering_plan.md](spec/areas/compat/plans/tkinter_lowering_plan.md)
- Tkinter runtime semantics differential probe (`tkinter.ttk:runtime_semantics`): [tests/differential/stdlib/tkinter_phase0_core_semantics.py](../tests/differential/stdlib/tkinter_phase0_core_semantics.py)
- Compatibility fallback contract: [spec/areas/compat/contracts/compatibility_fallback_contract.md](spec/areas/compat/contracts/compatibility_fallback_contract.md)
- Dynamic execution + reflection policy contract: [spec/areas/compat/contracts/dynamic_execution_policy_contract.md](spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
- Verified subset contract: [spec/areas/compat/contracts/verified_subset_contract.md](spec/areas/compat/contracts/verified_subset_contract.md)
- **Stdlib intrinsics sprint (2026-02-25)**: ~85 new Rust intrinsics across os, sys, signal, _thread, _asyncio, subprocess. See [STATUS.md](spec/STATUS.md) and [ROADMAP.md](../ROADMAP.md) for details.

## Testing and Benchmarks
- 0007 Testing: [spec/areas/testing/0007-testing.md](spec/areas/testing/0007-testing.md)
- 0008 Minimum Must-Pass Matrix: [spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
- 0008 Benchmarking: [spec/areas/perf/0008-benchmarking.md](spec/areas/perf/0008-benchmarking.md)
- 0604 Binary Size And Cold Start: [spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md](spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md)
- [BENCHMARKING.md](BENCHMARKING.md)
- [OPERATIONS.md](OPERATIONS.md)

## Capabilities and Security
- [CAPABILITIES.md](CAPABILITIES.md)
- [SECURITY.md](SECURITY.md)
- 0014 Determinism And Security Enforcement Checklist: [spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md)
- 0211 Compatibility And Fallback Contract: [spec/areas/compat/contracts/compatibility_fallback_contract.md](spec/areas/compat/contracts/compatibility_fallback_contract.md)
- 0216 Dynamic Execution And Reflection Policy Contract: [spec/areas/compat/contracts/dynamic_execution_policy_contract.md](spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
- 0215 Verified Subset Contract: [spec/areas/compat/contracts/verified_subset_contract.md](spec/areas/compat/contracts/verified_subset_contract.md)

## Web, DB, and WASM
- 0900 HTTP Server Runtime: [spec/areas/web/0900_HTTP_SERVER_RUNTIME.md](spec/areas/web/0900_HTTP_SERVER_RUNTIME.md)
- 0700 Molt DB Layer Vision: [spec/areas/db/0700_MOLT_DB_LAYER_VISION.md](spec/areas/db/0700_MOLT_DB_LAYER_VISION.md)
- 0400 WASM Portable ABI: [spec/areas/wasm/0400_WASM_PORTABLE_ABI.md](spec/areas/wasm/0400_WASM_PORTABLE_ABI.md)
- 0401 WASM Targets And Constraints: [spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md](spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md)
- 0968 Molt Edge/Workers VFS And Host Capabilities: [spec/areas/wasm/0968_MOLT_EDGE_WORKERS_VFS_AND_HOST_CAPABILITIES.md](spec/areas/wasm/0968_MOLT_EDGE_WORKERS_VFS_AND_HOST_CAPABILITIES.md)
- 0294 Molt Edge And Workers Runtime Proposal: [spec/areas/process/0294_MOLT_EDGE_WORKERS_RUNTIME_PROPOSAL.md](spec/areas/process/0294_MOLT_EDGE_WORKERS_RUNTIME_PROPOSAL.md)
- 0295 MEP-0001 Molt Edge/Workers Tier: [spec/areas/process/0295_MOLT_ENHANCEMENT_PROPOSAL_0001_EDGE_WORKERS_TIER.md](spec/areas/process/0295_MOLT_ENHANCEMENT_PROPOSAL_0001_EDGE_WORKERS_TIER.md)

## Full spec index
- [spec/README.md](spec/README.md)

## Documentation Policy
- `docs/` is reserved for canonical product, developer, architecture, proof, benchmark, and spec material.
- Internal agent/process/planning archives are kept outside the repo.
