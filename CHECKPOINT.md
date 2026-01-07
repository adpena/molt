Checkpoint: 2026-01-07 03:32:30 CST
Git: b4f323a backend: drop unused pointer unbox

Summary
- Added handle_resolve before wasm pointer-based ops (load/store/closure/guarded_load, get_attr_generic_ptr, object_set_class) and normalized raw ptrs in the wasm harness.
- Added differential tests for C3 MRO linearization and super+descriptor precedence.
- Expanded verified subset contract/tooling notes, runtime safety prerequisites, and added a CI step for verified subset manifest checks.
- Added OPT-0004 plan with preliminary handle-table benchmark notes.
- Adjusted super/descriptor test to avoid unsupported ternary expression and to use getattr-based fallback.

Files touched (uncommitted)
- runtime/molt-backend/src/wasm.rs
- tests/wasm_harness.py
- tests/differential/basic/mro_c3_linearization.py
- tests/differential/basic/super_descriptor_precedence.py
- docs/spec/0016_VERIFIED_SUBSET_CONTRACT.md
- docs/spec/0017_TYPE_SYSTEM_AND_SPECIALIZATION.md
- docs/spec/0020_RUNTIME_SAFETY_INVARIANTS.md
- .github/workflows/ci.yml
- OPTIMIZATIONS_PLAN.md

Tests run
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic
- uv run --python 3.12 pytest tests/test_wasm_control_flow.py
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test

Known gaps
- None noted after the latest test pass.

Next 5-step plan
1) Run wasm parity test + differential basic suite to validate handle resolution changes.
2) Run tools/dev.py lint/test once the changes settle.
3) Check for any spec/README/ROADMAP updates needed if behavior changes.
4) Re-run the handle-table bench matrix to update OPT-0004 results.
5) Continue parity work on class semantics + async hang probes.
