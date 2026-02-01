# Spec Index

This index groups specs by area so changes can anchor to the right contract.
Spec numbers are listed first; unnumbered specs are labeled UNNUMBERED.
Long-form topic docs live under `docs/spec/areas/`.

## Status
- STATUS: `docs/spec/STATUS.md`

## Core Architecture And Scope
- 0000 Vision: `docs/spec/areas/core/0000-vision.md`
- 0002 Architecture: `docs/spec/areas/core/0002-architecture.md`
- 0004 Tiers: `docs/spec/areas/core/0004-tiers.md`
- 0025 Reproducible And Deterministic Mode: `docs/spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md`
- 0800 What Molt Is Willing To Break: `docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md`

## Runtime And Execution
- 0003 Runtime: `docs/spec/areas/runtime/0003-runtime.md`
- 0009 GC Design: `docs/spec/areas/runtime/0009_GC_DESIGN.md`
- 0024 Runtime State Lifecycle: `docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md`
- 0026 Concurrency And GIL: `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`
- 0027 Runtime Architecture Map: `docs/spec/areas/runtime/0027_RUNTIME_ARCHITECTURE_MAP.md`
- 0300 Tasks And Channels: `docs/spec/areas/runtime/0300_TASKS_AND_CHANNELS.md`
- 0501 Data Model And Dtypes: `docs/spec/areas/runtime/0501_DATA_MODEL_AND_DTYPES.md`
- 0502 Execution Engine: `docs/spec/areas/runtime/0502_EXECUTION_ENGINE.md`
- 0505 IO Async And Connectors: `docs/spec/areas/runtime/0505_IO_ASYNC_AND_CONNECTORS.md`
- 0911 Molt Worker V0 Spec: `docs/spec/areas/runtime/0911_MOLT_WORKER_V0_SPEC.md`
- 0922 Strict Tier Rules For Trusted Types: `docs/spec/areas/runtime/0922_STRICT_TIER_RULES_FOR_TRUSTED_TYPES.md`

## Compiler And Lowering
- 0017 Type System And Specialization: `docs/spec/areas/compiler/0017_TYPE_SYSTEM_AND_SPECIALIZATION.md`
- 0019 Bytecode Lowering Matrix: `docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md`
- 0100 Molt IR: `docs/spec/areas/compiler/0100_MOLT_IR.md`
- 0190 Lowering Rules: `docs/spec/areas/compiler/0190_LOWERING_RULES.md`
- 0191 Deopt And Guard Model: `docs/spec/areas/compiler/0191_DEOPT_AND_GUARD_MODEL.md`
- 0192 Idioms And Semantic Patterns: `docs/spec/areas/compiler/0192_IDIOMS_AND_SEMANTIC_PATTERNS.md`
- 0193 Idiom Rewrites For Agents: `docs/spec/areas/compiler/0193_IDIOM_REWRITES_FOR_AGENTS.md`
- 0201 Guard And Deopt Lang: `docs/spec/areas/compiler/0201_GUARD_AND_DEOPT_LANG.md`
- 0202 Foreign Function Boundary: `docs/spec/areas/compiler/0202_FOREIGN_FUNCTION_BOUNDARY.md`
- 0920 Type Facts Artifact Ty Integration: `docs/spec/areas/compiler/0920_TYPE_FACTS_ARTIFACT_TY_INTEGRATION.md`

## Compatibility And Contracts
- 0014 Type Coverage Matrix: `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md`
- 0015 Stdlib Compatibility Matrix: `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md`
- 0016 Args Kwargs: `docs/spec/areas/compat/0016_ARGS_KWARGS.md`
- 0016 Verified Subset Contract: `docs/spec/areas/compat/0016_VERIFIED_SUBSET_CONTRACT.md`
- 0018 Molt Package ABI: `docs/spec/areas/compat/0018_MOLT_PACKAGE_ABI.md`
- 0021 Syntactic Features Matrix: `docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md`
- 0023 Semantic Behavior Matrix: `docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md`
- 0024 Asyncio Coverage Matrix: `docs/spec/areas/compat/0024_ASYNCIO_COVERAGE_MATRIX.md`
- 0025 Core Language PEP Coverage: `docs/spec/areas/compat/0025_CORE_LANGUAGE_PEP_COVERAGE.md`
- 0210 CPython Bridge PyO3: `docs/spec/areas/compat/0210_CPYTHON_BRIDGE_PYO3.md`
- 0211 Compatibility And Fallback Contract: `docs/spec/areas/compat/0211_COMPATIBILITY_AND_FALLBACK_CONTRACT.md`
- 0212 C API Symbol Matrix: `docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md`

## Security
- 0010 Security: `docs/spec/areas/security/0010-security.md`
- 0020 Runtime Safety Invariants: `docs/spec/areas/security/0020_RUNTIME_SAFETY_INVARIANTS.md`

## Tooling
- 0001 Toolchains: `docs/spec/areas/tooling/0001-toolchains.md`
- 0009 Packaging: `docs/spec/areas/tooling/0009-packaging.md`
- 0011 CI: `docs/spec/areas/tooling/0011-ci.md`
- 0012 Molt Commands: `docs/spec/areas/tooling/0012_MOLT_COMMANDS.md`
- 0013 Python Dependencies: `docs/spec/areas/tooling/0013_PYTHON_DEPENDENCIES.md`
- 0200 Profile Artifact: `docs/spec/areas/tooling/0200_PROFILE_ARTIFACT.md`
- 0602 When To Write Extensions Or Binaries: `docs/spec/areas/tooling/0602_WHEN_TO_WRITE_EXTENSIONS_OR_BINARIES.md`

## Testing
- 0007 Testing: `docs/spec/areas/testing/0007-testing.md`
- 0504 Differential Testing Oracle: `docs/spec/areas/testing/0504_DIFFERENTIAL_TESTING_ORACLE.md`

## Performance And Benchmarks
- 0008 Benchmarking: `docs/spec/areas/perf/0008-benchmarking.md`
- 0510 Loop Optimization And Vectorization: `docs/spec/areas/perf/0510_LOOP_OPTIMIZATION_AND_VECTORIZATION.md`
- 0511 String Optimization And Text Kernels: `docs/spec/areas/perf/0511_STRING_OPTIMIZATION_AND_TEXT_KERNELS.md`
- 0512 Arch Optimization And SIMD: `docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md`
- 0601 Benchmark Harness And CI Gates: `docs/spec/areas/perf/0601_BENCHMARK_HARNESS_AND_CI_GATES.md`
- 0603 Benchmarks: `docs/spec/areas/perf/0603_BENCHMARKS.md`
- 0604 Binary Size And Cold Start: `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md`
- 0914 Bench Runner And Results Format: `docs/spec/areas/perf/0914_BENCH_RUNNER_AND_RESULTS_FORMAT.md`
- 0961 Benchmarks Contract: `docs/spec/areas/perf/0961_BENCHMARKS_CONTRACT.md`

## WASM
- 0005 WASM Interop: `docs/spec/areas/wasm/0005-wasm-interop.md`
- 0400 WASM Portable ABI: `docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md`
- 0401 WASM Targets And Constraints: `docs/spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md`
- 0963 Pyodide Lessons For Molt WASM: `docs/spec/areas/wasm/0963_PYODIDE_LESSONS_FOR_MOLT_WASM.md`
- 0964 Molt WASM ABI Browser Demo And Constraints: `docs/spec/areas/wasm/0964_MOLT_WASM_ABI_BROWSER_DEMO_AND_CONSTRAINTS.md`
- 0965 Cloudflare Workers Lessons For Molt: `docs/spec/areas/wasm/0965_CLOUDFLARE_WORKERS_LESSONS_FOR_MOLT.md`
- 0966 External Inspirations Codon Py2WASM Trio Go OpenMP: `docs/spec/areas/wasm/0966_EXTERNAL_INSPIRATIONS_CODON_PY2WASM_TRIO_GO_OPENMP.md`
- 0967 Portable Plugin Manifest + Schema Resolution: `docs/spec/areas/wasm/0967_PORTABLE_PLUGIN_MANIFEST.md`

## Web
- 0600 Streaming And WebSockets: `docs/spec/areas/web/0600_STREAMING_AND_WEBSOCKETS.md`
- 0900 HTTP Server Runtime: `docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md`
- 0901 Web Framework And Routing: `docs/spec/areas/web/0901_WEB_FRAMEWORK_AND_ROUTING.md`
- 0921 Schema Compiled Boundaries Pydantic: `docs/spec/areas/web/0921_SCHEMA_COMPILED_BOUNDARIES_PYDANTIC.md`
- 0930 FastAPI Pydantic Patterns To Molt: `docs/spec/areas/web/0930_FASTAPI_PYDANTIC_PATTERNS_TO_MOLT.md`
- 0931 Eliminating Pydantic Runtime Calls: `docs/spec/areas/web/0931_ELIMINATING_PYDANTIC_RUNTIME_CALLS.md`
- 0932 Molt Schema DSL And Pydantic Compat: `docs/spec/areas/web/0932_MOLT_SCHEMA_DSL_AND_PYDANTIC_COMPAT.md`
- 0933 FastAPI How It Works And Comparison: `docs/spec/areas/web/0933_FASTAPI_HOW_IT_WORKS_AND_COMPARISON.md`

## Data And DB
- 0700 Molt DB Layer Vision: `docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md`
- 0701 Async PG Pool And Protocol: `docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md`
- 0702 Query Builder And Django Adapter: `docs/spec/areas/db/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md`
- 0703 Row Decoding To Structs And Arrow: `docs/spec/areas/db/0703_ROW_DECODING_TO_STRUCTS_AND_ARROW.md`
- 0704 Transactions And Cancellation: `docs/spec/areas/db/0704_TRANSACTIONS_AND_CANCELLATION.md`
- 0915 Molt DB IPC Contract: `docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md`

## Dataframe
- 0500 Dataframe Vision: `docs/spec/areas/dataframe/0500_DATAFRAME_VISION.md`
- 0503 Pandas Compatibility Matrix: `docs/spec/areas/dataframe/0503_PANDAS_COMPATIBILITY_MATRIX.md`

## Demos
- 0600 Killer Demo Django Offload: `docs/spec/areas/demos/0600_KILLER_DEMO_DJANGO_OFFLOAD.md`
- 0801 Second Killer Demo Background Jobs: `docs/spec/areas/demos/0801_SECOND_KILLER_DEMO_BACKGROUND_JOBS.md`
- 0910 Repro Bench Vertical Slice: `docs/spec/areas/demos/0910_REPRO_BENCH_VERTICAL_SLICE.md`
- 0912 Molt Accel Django Decorator Spec: `docs/spec/areas/demos/0912_MOLT_ACCEL_DJANGO_DECORATOR_SPEC.md`
- 0913 Demo Django Endpoint Contract: `docs/spec/areas/demos/0913_DEMO_DJANGO_ENDPOINT_CONTRACT.md`

## Business
- 0940 Ecosystem Leverage Map: `docs/spec/areas/business/0940_ECOSYSTEM_LEVERAGE_MAP.md`
- 0950 Funding Narrative: `docs/spec/areas/business/0950_FUNDING_NARRATIVE.md`
- 0951 CTO One Pager: `docs/spec/areas/business/0951_CTO_ONE_PAGER.md`
- 0960 Molt Metrics Slide: `docs/spec/areas/business/0960_MOLT_METRICS_SLIDE.md`
- 0962 Mock Investor Slide: `docs/spec/areas/business/0962_MOCK_INVESTOR_SLIDE.md`

## Process And Addenda
- 0006 Roadmap: `docs/spec/areas/process/0006-roadmap.md`
- 0099 Repo Review 2026 01 07: `docs/spec/areas/process/0099_REPO_REVIEW_2026-01-07.md`
- 0290 Architecture Backend Decision: `docs/spec/areas/process/0290_ARCHITECTURE_BACKEND_DECISION.md`
- 0291 Introspective Rapid Iteration: `docs/spec/areas/process/0291_INTROSPECTIVE_RAPID_ITERATION.md`
- 0292 Molt Addendum Runtime First Web DB Pipelines: `docs/spec/areas/process/0292_MOLT_ADDENDUM_RUNTIME_FIRST_WEB_DB_PIPELINES.md`
- 0293 Molt Addendum UV Rust Tooling: `docs/spec/areas/process/0293_MOLT_ADDENDUM_UV_RUST_TOOLING.md`

## Ecosystem
- 0970 Faq Why Not Numba PyPy Cython: `docs/spec/areas/ecosystem/0970_FAQ_WHY_NOT_NUMBA_PYPY_CYTHON.md`
- 0971 Why Coiled Is Relevant: `docs/spec/areas/ecosystem/0971_WHY_COILED_IS_RELEVANT.md`

## Adding Or Updating A Spec
- Keep the scope narrow and list status at the top.
- Link to the relevant matrices when semantics change.
- Update `docs/spec/STATUS.md` and `docs/ROADMAP.md` when scope moves.
