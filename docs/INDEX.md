# Documentation Index

Molt targets **Python 3.12+** semantics only. Do not spend effort on <=3.11.

## Start Here
- [AGENTS.md](AGENTS.md)
- [docs/CANONICALS.md](docs/CANONICALS.md)
- [CONTRIBUTING.md](CONTRIBUTING.md)
- [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md)
- [docs/OPERATIONS.md](docs/OPERATIONS.md)
- Platform pitfalls: [README.md](README.md) (quickstart), [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md), and [docs/OPERATIONS.md](docs/OPERATIONS.md).

## Navigation Hubs
- Project quickstart + top-level status: [README.md](../README.md)
- Canonical documentation index: [docs/INDEX.md](INDEX.md)
- Full spec index (all spec areas): [docs/spec/README.md](spec/README.md)
- Differential test organization and run ledger: [tests/differential/INDEX.md](../tests/differential/INDEX.md)

## Workspace Guides
- Examples: [examples/README.md](../examples/README.md)
- Demo app and offload workflow: [demo/README.md](../demo/README.md)
- Symphony orchestration setup: [docs/SYMPHONY.md](docs/SYMPHONY.md)
- Symphony canonical alignment ledger: [docs/SYMPHONY_CANONICAL_ALIGNMENT.md](docs/SYMPHONY_CANONICAL_ALIGNMENT.md)
- Harness engineering alignment: [docs/HARNESS_ENGINEERING.md](docs/HARNESS_ENGINEERING.md)
- Symphony claw-loop research map: [docs/SYMPHONY_CLAW_LOOP_RESEARCH.md](docs/SYMPHONY_CLAW_LOOP_RESEARCH.md)
- Symphony quality score rubric: [docs/QUALITY_SCORE.md](docs/QUALITY_SCORE.md)
- Execution plan template: [docs/exec-plans/TEMPLATE.md](docs/exec-plans/TEMPLATE.md)
- Symphony human role: [docs/SYMPHONY_HUMAN_ROLE.md](docs/SYMPHONY_HUMAN_ROLE.md)
- Symphony operator playbook: [docs/SYMPHONY_OPERATOR_PLAYBOOK.md](docs/SYMPHONY_OPERATOR_PLAYBOOK.md)
- Linear workspace bootstrap: [docs/LINEAR_WORKSPACE_BOOTSTRAP.md](docs/LINEAR_WORKSPACE_BOOTSTRAP.md)
- Benchmark harnesses: [bench/README.md](../bench/README.md), [bench/friends/README.md](../bench/friends/README.md)
- Packaging/release guides: [packaging/README.md](../packaging/README.md), [packaging/templates/linux/README.md](../packaging/templates/linux/README.md)

## Vision, Scope, Status
- 0000 Vision: [docs/spec/areas/core/0000-vision.md](docs/spec/areas/core/0000-vision.md)
- 0025 Reproducible And Deterministic Mode: [docs/spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md](docs/spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md)
- 0800 What Molt Is Willing To Break: [docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md)
- STATUS: [docs/spec/STATUS.md](docs/spec/STATUS.md)
- [ROADMAP.md](ROADMAP.md) (active roadmap)
- [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md)
- [docs/ROADMAP.md](docs/ROADMAP.md) (detailed archive/reference)

## Architecture and Runtime
- 0002 Architecture: [docs/spec/areas/core/0002-architecture.md](docs/spec/areas/core/0002-architecture.md)
- 0003 Runtime: [docs/spec/areas/runtime/0003-runtime.md](docs/spec/areas/runtime/0003-runtime.md)
- 0024 Runtime State Lifecycle: [docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md](docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md)
- 0026 Concurrency And GIL: [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)
- 0027 Runtime Architecture Map: [docs/spec/areas/runtime/0027_RUNTIME_ARCHITECTURE_MAP.md](docs/spec/areas/runtime/0027_RUNTIME_ARCHITECTURE_MAP.md)
- 0004 Tiers: [docs/spec/areas/core/0004-tiers.md](docs/spec/areas/core/0004-tiers.md)
- 0191 Deopt And Guard Model: [docs/spec/areas/compiler/0191_DEOPT_AND_GUARD_MODEL.md](docs/spec/areas/compiler/0191_DEOPT_AND_GUARD_MODEL.md)

## Compiler and Lowering
- 0019 Bytecode Lowering Matrix: [docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md)
- 0190 Lowering Rules: [docs/spec/areas/compiler/0190_LOWERING_RULES.md](docs/spec/areas/compiler/0190_LOWERING_RULES.md)
- 0192 Idioms And Semantic Patterns: [docs/spec/areas/compiler/0192_IDIOMS_AND_SEMANTIC_PATTERNS.md](docs/spec/areas/compiler/0192_IDIOMS_AND_SEMANTIC_PATTERNS.md)

## Compatibility and Stdlib
- Compatibility architecture index: [docs/spec/areas/compat/README.md](docs/spec/areas/compat/README.md)
- Language surface index: [docs/spec/areas/compat/surfaces/language/language_surface_matrix.md](docs/spec/areas/compat/surfaces/language/language_surface_matrix.md)
- Stdlib surface index: [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md)
- Stdlib surface matrix: [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)
- C-API surface index: [docs/spec/areas/compat/surfaces/c_api/c_api_surface_index.md](docs/spec/areas/compat/surfaces/c_api/c_api_surface_index.md)
- C-API v0 bootstrap symbol list (including scalar/object-bytes/array constructors, type/module parity wrappers, runtime-owned module-state registries, expanded scan-driven CPython-compat shim coverage via `include/Python.h`, and the widened NumPy header lane for dtype/data-memory/ufunc scaffolding): [docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md](docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md)
- Stdlib lowering execution plan: [docs/spec/areas/compat/plans/stdlib_lowering_plan.md](docs/spec/areas/compat/plans/stdlib_lowering_plan.md)
- Tkinter lowering execution plan: [docs/spec/areas/compat/plans/tkinter_lowering_plan.md](docs/spec/areas/compat/plans/tkinter_lowering_plan.md)
- Tkinter runtime semantics differential probe (`tkinter.ttk:runtime_semantics`): [tests/differential/stdlib/tkinter_phase0_core_semantics.py](../tests/differential/stdlib/tkinter_phase0_core_semantics.py)
- Compatibility fallback contract: [docs/spec/areas/compat/contracts/compatibility_fallback_contract.md](docs/spec/areas/compat/contracts/compatibility_fallback_contract.md)
- Dynamic execution + reflection policy contract: [docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md](docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
- Verified subset contract: [docs/spec/areas/compat/contracts/verified_subset_contract.md](docs/spec/areas/compat/contracts/verified_subset_contract.md)
- **Stdlib intrinsics sprint (2026-02-25)**: ~85 new Rust intrinsics across os, sys, signal, _thread, _asyncio, subprocess. See [STATUS.md](spec/STATUS.md) and [ROADMAP.md](../ROADMAP.md) for details.

## Testing and Benchmarks
- 0007 Testing: [docs/spec/areas/testing/0007-testing.md](docs/spec/areas/testing/0007-testing.md)
- 0008 Minimum Must-Pass Matrix: [docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
- 0008 Benchmarking: [docs/spec/areas/perf/0008-benchmarking.md](docs/spec/areas/perf/0008-benchmarking.md)
- 0604 Binary Size And Cold Start: [docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md](docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md)
- [docs/BENCHMARKING.md](docs/BENCHMARKING.md)
- [docs/OPERATIONS.md](docs/OPERATIONS.md)

## Capabilities and Security
- [docs/CAPABILITIES.md](docs/CAPABILITIES.md)
- [docs/SECURITY.md](docs/SECURITY.md)
- 0014 Determinism And Security Enforcement Checklist: [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md)
- 0211 Compatibility And Fallback Contract: [docs/spec/areas/compat/contracts/compatibility_fallback_contract.md](docs/spec/areas/compat/contracts/compatibility_fallback_contract.md)
- 0216 Dynamic Execution And Reflection Policy Contract: [docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md](docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
- 0215 Verified Subset Contract: [docs/spec/areas/compat/contracts/verified_subset_contract.md](docs/spec/areas/compat/contracts/verified_subset_contract.md)

## Web, DB, and WASM
- 0900 HTTP Server Runtime: [docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md](docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md)
- 0700 Molt DB Layer Vision: [docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md](docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md)
- 0400 WASM Portable ABI: [docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md](docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md)
- 0401 WASM Targets And Constraints: [docs/spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md](docs/spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md)

## Full spec index
- [docs/spec/README.md](docs/spec/README.md)

## Internal And Archival
- Agent memory log (archival context, not canonical status): [docs/AGENT_MEMORY.md](docs/AGENT_MEMORY.md)
