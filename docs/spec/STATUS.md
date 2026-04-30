# STATUS (Canonical)

This document is the source of truth for Molt's current supported surface.
It is current-state only. For forward-looking priorities, use
[ROADMAP.md](../../ROADMAP.md).

## Project Scope And Target

- Strategic target: full CPython `>=3.12` parity for supported Molt semantics.
- Product target: full CPython `>=3.12` parity without hidden host fallback for
  the semantics Molt claims today.
- Compiled binaries must not rely on a host Python installation.
- runtime monkeypatching, unrestricted `exec`/`eval`/`compile`, and unrestricted
  reflection remain intentional design exclusions for compiled binaries.

## Supported Today

- Native AOT compilation is real and active.
- Standalone binary workflows are a first-class product requirement.
- Differential testing against CPython is a core validation path.
- Build target semantics are explicit for Python `3.12`, `3.13`, and `3.14`:
  `molt build --python-version`, `[tool.molt.build] python-version`, and
  `project.requires-python` resolve the target version before parsing, module
  graph discovery, frontend cache lookup, backend cache lookup, and runtime
  `sys.version_info` bootstrap. The compiled bootstrap is unconditional: native,
  WASM, standalone Rust source emission, and isolate entry paths stamp the
  selected target version before user code/importlib gates run. Runtime
  version-gated stdlib decisions read that runtime state instead of ambient
  process env, and Rust source outputs materialize `sys.version_info`,
  `sys.version`, and `sys.hexversion` from the same stamped state. Rust source
  outputs also own executable module-cache get/set/delete semantics for emitted
  import bootstrap IR, with cache misses represented as `None` rather than a
  truthy sentinel. Malformed or non-string target-version config fails closed
  instead of falling back to another target. The default target remains Python
  `3.12`.
- Rust-first stdlib lowering is the canonical direction, with generated audit
  surfaces under `docs/spec/areas/compat/surfaces/stdlib/`.
- WASM remains a supported target area, but same-contract parity with native is
  still incomplete.
- Luau is a checked source-emission target for the current/future Luau surface;
  current OpIR support is generated in
  `docs/spec/areas/compiler/luau_support_matrix.generated.md`.

## Intentionally Unsupported

- Unrestricted dynamic execution (`exec`, `eval`, `compile`) in compiled binaries.
- Runtime monkeypatching as a compatibility mechanism.
- Unrestricted reflection that breaks AOT determinism and layout guarantees.
- Silent fallback to a host CPython runtime.

## Known Major Gaps / Blockers

- CPython coverage is incomplete across language, stdlib, and target-specific
  behavior.
- Native and WASM parity is still incomplete for several claimed surfaces.
- Luau parity is incomplete and must be extended through checked-build,
  static-analysis, and CPython-vs-Luau evidence rather than silent stub emission.
- The runpy dynamic-lane expected failures list is currently empty because
  supported lanes moved to intrinsic support; governance for unsupported
  runpy dynamic execution remains documented rather than tracked through an
  active expected-failure entry.
- The current backend entry path still carries a stringly `SimpleIR` transport
  and compatibility hint fields (`fast_int` / `fast_float` / `raw_int` /
  `type_hint`) for legacy consumers, but the canonical backend contract is now
  the shared representation-aware TIR/LIR path across native and WASM.
- Benchmark reporting and compatibility rollups are being simplified so they are
  generated from canonical evidence instead of maintained by hand in multiple docs.

## Validation Summary

- Canonical local DX now routes through:
  - `molt setup`
  - `molt doctor`
  - `molt validate`
- Backend completion now requires an explicit end-to-end CLI/profile/target
  matrix, not only backend-internal unit and lowering proof:
  - native `build` / `run` / `compare` on `dev` and `release`
  - LLVM release parity on the covered slice
  - linked-WASM CLI build plus Node execution
  - conformance and benchmark entrypoints on the same CLI validation surface
  - honest failure surfaces for intentionally unsupported dynamic execution
- Compatibility evidence is tracked in the differential suites, generated
  compatibility docs, and proof workflows linked below.

## Compatibility Summary

<!-- GENERATED:compat-summary:start -->
- Stdlib lowering audit: `916` modules audited; `41` intrinsic-backed; `875` intrinsic-partial; `0` python-only.
- Platform availability metadata: `66` modules with explicit availability notes; `41` WASI-blocked; `37` Emscripten-blocked in CPython docs.
- Deep evidence: see the stdlib intrinsics audit and platform availability matrices under `docs/spec/areas/compat/surfaces/stdlib/`.
<!-- GENERATED:compat-summary:end -->

## Performance Summary

<!-- GENERATED:bench-summary:start -->
Latest run: 2026-03-21 (macOS arm64, CPython 3.12.13).
Top speedups: `bench_bytearray_replace.py` 2.15x, `bench_bytes_replace.py` 1.64x, `bench_startup.py` 1.56x, `bench_memoryview_tobytes.py` 1.34x, `bench_set_ops.py` 1.13x.
Regressions: `bench_class_hierarchy.py` 0.01x, `bench_struct.py` 0.05x, `bench_bytes_find_only.py` 0.06x, `bench_bytes_find.py` 0.07x, `bench_exception_heavy.py` 0.10x, `bench_attr_access.py` 0.11x, `bench_json_roundtrip.py` 0.13x, `bench_descriptor_property.py` 0.13x, `bench_str_endswith.py` 0.19x, `bench_str_startswith.py` 0.21x, `bench_str_find.py` 0.21x, `bench_str_count.py` 0.23x, `bench_str_replace.py` 0.27x, `bench_str_count_unicode.py` 0.28x, `bench_str_find_unicode_warm.py` 0.32x, `bench_str_find_unicode.py` 0.34x, `bench_str_count_unicode_warm.py` 0.34x, `bench_counter_words.py` 0.38x, `bench_dict_views.py` 0.57x, `bench_dict_ops.py` 0.65x, `bench_bytearray_find.py` 0.76x, `bench_gc_pressure.py` 0.79x, `bench_str_split.py` 0.94x.
Slowest: `bench_class_hierarchy.py` 0.01x, `bench_struct.py` 0.05x, `bench_bytes_find_only.py` 0.06x.
Build/run failures: PyPy skipped for `bench_channel_throughput.py`, `bench_parse_msgpack.py`, `bench_ptr_registry.py`; Codon baseline unavailable; Nuitka baseline unavailable; Pyodide baseline unavailable.
WASM run: 2026-03-28 (macOS arm64, CPython 3.12.13). Slowest: `bench_sum.py` 0.00s; largest sizes: `bench_sum.py` 7182.5 KB; WASM vs CPython slowest ratios: `bench_sum.py` 0.00x.
<!-- GENERATED:bench-summary:end -->

## Deep Links

- Compatibility architecture: [areas/compat/README.md](areas/compat/README.md)
- Language surface index: [areas/compat/surfaces/language/language_surface_matrix.md](areas/compat/surfaces/language/language_surface_matrix.md)
- Stdlib surface index: [areas/compat/surfaces/stdlib/stdlib_surface_index.md](areas/compat/surfaces/stdlib/stdlib_surface_index.md)
- Detailed benchmark report: [../benchmarks/bench_summary.md](../benchmarks/bench_summary.md)
- Standalone proof workflow: [../proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](../proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
