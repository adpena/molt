# Compatibility Architecture (CPython 3.12+)

**Status:** Active
**Owner:** frontend + runtime + stdlib + tooling
**Purpose:** Canonical architecture for tracking Molt compatibility against CPython 3.12+ across language, stdlib, C-API, native targets, and wasm targets.

## Non-Negotiable Governance
- This directory is the canonical compatibility truth for Molt.
- Compatibility claims must be organized by CPython architecture surface, not ad-hoc feature buckets.
- All major compatibility claims must include reproducible evidence (differential tests and/or generator output).
- No dual truth: if a file is generated, humans do not hand-edit semantic status in that file.
- Native and wasm compatibility must be tracked as first-class dimensions, not hidden in prose notes.
- Version-gated behavior for 3.12/3.13/3.14 must be explicit and test-backed.
- The architectural target is full CPython `>=3.12` parity for compiled Molt outputs, except for the explicit carve-outs in the dynamic execution policy contract.
- Compiled binaries must remain self-contained and must not rely on a host CPython fallback lane.

## CPython Reference Inputs
Canonical local CPython documentation mirror:
- `molt/docs/python_documentation`

Primary source categories:
- Language reference: `docs/python_documentation/python-3.12-docs-text/reference/index.txt`
- Stdlib/library reference: `docs/python_documentation/python-3.12-docs-text/library/index.txt`
- C-API reference: `docs/python_documentation/python-3.12-docs-text/c-api/index.txt`

When updating compatibility status, use the 3.12/3.13/3.14 docs together and record version-specific deltas explicitly.

## Directory Taxonomy
- `contracts/`: normative contracts and policy boundaries.
- `surfaces/language/`: language-level compatibility (syntax, semantics, PEP coverage).
- `surfaces/stdlib/`: stdlib API/behavior/intrinsics coverage and platform availability.
- `surfaces/c_api/`: `libmolt` C-API contract and symbol coverage.
- `plans/`: execution programs and lowering plans.

## Canonical Status Model
Use this status vocabulary in all newly authored coverage matrices:
- `missing`: no intentional implementation.
- `api_shape_only`: import/name shape exists but behavior is not implemented.
- `behavior_partial`: behavior implemented for a meaningful subset; gaps documented.
- `behavior_full`: behavior matches targeted CPython version policy for supported semantics.
- `intentional_divergence`: explicit project-policy divergence.

Track these dimensions when relevant:
- `py312`, `py313`, `py314`
- `native`
- `wasm_wasi`
- `wasm_browser`
- `linux`, `macos`, `windows`

## Generated vs Hand-Edited Files
Generated files (do not hand-edit semantic data):
- `surfaces/stdlib/stdlib_intrinsics_audit.generated.md`
- `surfaces/stdlib/asyncio_surface.generated.md`
- `surfaces/language/core_language_pep_coverage.generated.md`
- `surfaces/language/generator_api_coverage.generated.md`
- `surfaces/stdlib/stdlib_platform_availability.generated.md`

Hand-edited control files:
- `surfaces/stdlib/stdlib_surface_matrix.md`
- `surfaces/language/type_coverage_matrix.md`
- `surfaces/language/syntactic_features_matrix.md`
- `surfaces/language/semantic_behavior_matrix.md`
- `surfaces/c_api/libmolt_c_api_surface.md`
- `surfaces/c_api/c_api_symbol_matrix.md`
- `plans/stdlib_lowering_plan.md`
- `plans/tkinter_lowering_plan.md`
- all files under `contracts/`

## Required Update Workflow
1. Refresh stdlib union baseline and stubs:
   - `python3 tools/gen_stdlib_module_union.py`
   - `python3 tools/sync_stdlib_top_level_stubs.py --write`
   - `python3 tools/sync_stdlib_submodule_stubs.py --write`
2. Refresh stdlib intrinsic audit doc:
   - `python3 tools/check_stdlib_intrinsics.py --update-doc`
3. Refresh CPython availability matrix:
   - `python3 tools/gen_compat_platform_availability.py --write`
4. Re-run compatibility gates:
   - `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
   - `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
   - `python3 tools/check_dynamic_policy.py`
   - `python3 tools/check_differential_suite_layout.py`
5. Sync rollup docs in the same change:
   - `docs/spec/STATUS.md`
   - `ROADMAP.md`
   - `docs/spec/README.md`
   - `docs/INDEX.md`

## Canonical Indexes
- Language surface index: `docs/spec/areas/compat/surfaces/language/language_surface_matrix.md`
- Stdlib surface index: `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md`
- C-API surface index: `docs/spec/areas/compat/surfaces/c_api/c_api_surface_index.md`
- libmolt extension ABI contract: `docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md`
- Dynamic execution/reflection policy contract: `docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md`
- Execution plan: `docs/spec/areas/compat/plans/stdlib_lowering_plan.md`
- Tkinter execution plan: `docs/spec/areas/compat/plans/tkinter_lowering_plan.md`
