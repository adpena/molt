# Session Changes Report

**Date**: 2026-03-16
**Branch**: `main`
**Baseline commit**: `82e5326d` (chore: gitignore agent worktrees to prevent false drift)

---

## Summary

| Metric | Count |
|--------|-------|
| Modified files (tracked) | 51 |
| New files (untracked) | 44 (excluding `.hypothesis/` cache) |
| Total insertions | ~5,226 |
| Total deletions | ~857 |
| Net new lines in new source files | ~12,757 |
| Lean `sorry` removals | 45 |
| Lean `sorry` additions | 37 |
| **Net sorry reduction** | **8** |

---

## Categorized Changes

### 1. Formal Verification — Modified Lean Files (36 files)

Major sorry-closure and proof-strengthening work across the Lean formalization.

**Core / Syntax / Semantics**
- `formal/lean/MoltTIR/Syntax.lean` (+3/-1)
- `formal/lean/MoltTIR/Semantics/EvalExpr.lean` (+1)
- `formal/lean/MoltLowering/Correct.lean` (+887 net — large proof expansion)

**SSA**
- `formal/lean/MoltTIR/SSA/PassPreservesSSA.lean` (+1,142 net — major proof)
- `formal/lean/MoltTIR/SSA/WellFormedSSA.lean` (+158)

**Passes**
- `formal/lean/MoltTIR/Passes/SCCPCorrect.lean` (+204)
- `formal/lean/MoltTIR/Passes/SCCPMultiCorrect.lean` (+170)
- `formal/lean/MoltTIR/Passes/GuardHoistCorrect.lean` (+190)
- `formal/lean/MoltTIR/Passes/GuardHoist.lean` (+10)
- `formal/lean/MoltTIR/Passes/EdgeThreadCorrect.lean` (+11)
- `formal/lean/MoltTIR/Passes/FullPipeline.lean` (+8)
- `formal/lean/MoltTIR/Passes/Pipeline.lean` (+4)

**Runtime**
- `formal/lean/MoltTIR/Runtime/NanBoxCorrect.lean` (+230)
- `formal/lean/MoltTIR/Runtime/OwnershipModel.lean` (+56)
- `formal/lean/MoltTIR/Runtime/WasmABI.lean` (+78)
- `formal/lean/MoltTIR/Runtime/WasmNativeCorrect.lean` (+151)

**Backend**
- `formal/lean/MoltTIR/Backend/LuauCorrect.lean` (+19)
- `formal/lean/MoltTIR/Backend/LuauEmit.lean` (+1)
- `formal/lean/MoltTIR/Backend/LuauSemantics.lean` (+1)
- `formal/lean/MoltTIR/Backend/LuauSyntax.lean` (+1)
- `formal/lean/MoltTIR/Backend/RustCorrect.lean` (+18)
- `formal/lean/MoltTIR/Backend/RustEmit.lean` (+1)
- `formal/lean/MoltTIR/Backend/RustSemantics.lean` (+1)
- `formal/lean/MoltTIR/Backend/RustSyntax.lean` (+1)
- `formal/lean/MoltTIR/Backend/TargetIndependence.lean` (+2)
- `formal/lean/MoltTIR/Backend/WasmEmit.lean` (+1)

**Simulation / End-to-End**
- `formal/lean/MoltTIR/Simulation/Adequacy.lean` (+22)
- `formal/lean/MoltTIR/Simulation/Compose.lean` (+21)
- `formal/lean/MoltTIR/Simulation/PassSimulation.lean` (+102)
- `formal/lean/MoltTIR/EndToEnd.lean` (+6)
- `formal/lean/MoltTIR/EndToEndProperties.lean` (+6)

**Validation / Compilation / Determinism / Meta**
- `formal/lean/MoltTIR/Validation/SCCPValid.lean` (+360)
- `formal/lean/MoltTIR/Validation/TranslationValidation.lean` (+8)
- `formal/lean/MoltTIR/Compilation/CompilationCorrectness.lean` (+49)
- `formal/lean/MoltTIR/Determinism/BuildReproducibility.lean` (+13)
- `formal/lean/MoltTIR/Meta/SorryAudit.lean` (+5)
- `formal/lean/MoltTIR/TypeSystem/TypeInference.lean` (+9)

**Build config**
- `formal/lean/lakefile.lean` (+7)
- `formal/lean/lean-toolchain` (+2/-1)

### 2. Formal Verification — New Lean Files (6 files)

- `formal/lean/MoltTIR/Backend/LuauTargetSemantics.lean`
- `formal/lean/MoltTIR/Runtime/CapabilityGate.lean`
- `formal/lean/MoltTIR/Runtime/IntrinsicContracts.lean`
- `formal/lean/MoltTIR/Runtime/NanBoxBV.lean`
- `formal/lean/MoltPython/Properties/VersionCompat.lean`
- `formal/lean/MoltPython/VersionGated.lean`
- `formal/lean/SORRY_BASELINE` (tracking file)

### 3. Test Files — New (24 files)

**Property-based tests**
- `tests/property/test_math_intrinsics.py`
- `tests/property/test_string_intrinsics.py`
- `tests/property/test_hash_intrinsics.py`
- `tests/property/test_collection_intrinsics.py`

**Model-based tests**
- `tests/model_based/test_model_refcount.py`
- `tests/model_based/test_model_optimization_pipeline.py`
- `tests/model_based/test_model_determinism.py`

**Fuzz tests**
- `tests/fuzz/test_fuzz_differential.py`
- `tests/fuzz/test_fuzz_extended.py`

**Mutation tests**
- `tests/mutation/test_mutation_extended.py`

**Determinism tests**
- `tests/determinism/test_ir_determinism.py`
- `tests/determinism/test_entropy_audit.py`
- `tests/determinism/__init__.py`

**Correspondence / Validation / Backend / WASM tests**
- `tests/test_lean_python_correspondence.py`
- `tests/test_lean_rust_correspondence.py`
- `tests/test_backend_parity.py`
- `tests/test_translation_validation_e2e.py`
- `tests/test_translation_validator_unit.py`
- `tests/test_luau_transpiler_correctness.py`
- `tests/test_version_compat.py`
- `tests/test_wasm_determinism.py`
- `tests/test_wasm_performance.py`

**Test fixtures**
- `tests/fixtures/tv_dumps/` (6 JSON fixture files: before/after for DCE, CSE, SCCP)

### 4. Test Files — Modified (2 files)

- `tests/fuzz/test_fuzz_smoke.py` (+116)
- `tests/mutation/test_mutation_smoke.py` (+283)

### 5. Tool Files — New (6 files)

- `tools/correctness_dashboard.py`
- `tools/translation_validator.py`
- `tools/check_lean_sorry_count.py`
- `tools/check_correspondence_extended.py`
- `tools/check_reproducible_build_extended.py`
- `tools/update_linear_status.sh`

### 6. Tool Files — Modified (5 files)

- `tools/ci_gate.py` (+24)
- `tools/check_determinism.py` (+478)
- `tools/check_translation_validation.py` (+77)
- `tools/mutation_test.py` (+236)
- `tools/quint_trace_to_tests.py` (+385)

### 7. Frontend / Source

- `src/molt/frontend/tv_hooks.py` (NEW — translation validation hooks)

### 8. Rust / Kani (5 new + 2 modified)

**New Kani harness files**
- `runtime/molt-obj-model/tests/kani_nanbox.rs`
- `runtime/molt-obj-model/tests/kani_refcount.rs`
- `runtime/molt-obj-model/tests/kani_intrinsic_contracts.rs`
- `runtime/molt-runtime/tests/kani_string_ops.rs`
- `runtime/molt-runtime/tests/kani_object.rs`

**Modified Cargo.toml (Kani deps)**
- `runtime/molt-obj-model/Cargo.toml` (+6)
- `runtime/molt-runtime/Cargo.toml` (+3)

### 9. CI / Workflows

- `.github/workflows/ci.yml` (+26 — new CI steps)
- `.github/workflows/kani.yml` (NEW — Kani verification workflow)

### 10. Benchmarks

- `bench/wasm_baseline.json` (NEW)
- `bench/wasm_bench.py` (NEW)

### 11. Documentation

- `docs/spec/areas/formal/CERTIFICATION_STATUS.md` (NEW)

### 12. Dependency Updates

- `Cargo.lock` (+28/-28 — Rust dependency updates)
- `uv.lock` (+448/-448 — Python dependency updates)

### 13. Generated / Cache (should NOT be committed)

- `.hypothesis/` directory (Hypothesis test framework cache)
- `tests/determinism/__pycache__/` (Python bytecode cache)

---

## Commit Plan (7 logical commits)

### Commit 1: `formal: Lean 4.28 upgrade + sorry closure + axiom elimination`

All `formal/lean/` changes including new and modified Lean source files, build config, and sorry tracking.

**Files:**
- `formal/lean/lean-toolchain`
- `formal/lean/lakefile.lean`
- `formal/lean/SORRY_BASELINE` (new)
- `formal/lean/.lake/config/` (new, if needed)
- `formal/lean/MoltLowering/Correct.lean`
- `formal/lean/MoltPython/Env.lean`
- `formal/lean/MoltPython/Semantics/EvalExpr.lean`
- `formal/lean/MoltPython/Properties/VersionCompat.lean` (new)
- `formal/lean/MoltPython/VersionGated.lean` (new)
- `formal/lean/MoltTIR/Syntax.lean`
- `formal/lean/MoltTIR/WellFormed.lean`
- `formal/lean/MoltTIR/EndToEnd.lean`
- `formal/lean/MoltTIR/EndToEndProperties.lean`
- `formal/lean/MoltTIR/Meta/SorryAudit.lean`
- `formal/lean/MoltTIR/SSA/Dominance.lean`
- `formal/lean/MoltTIR/SSA/PassPreservesSSA.lean`
- `formal/lean/MoltTIR/SSA/Properties.lean`
- `formal/lean/MoltTIR/SSA/WellFormedSSA.lean`
- `formal/lean/MoltTIR/Semantics/BlockCorrect.lean`
- `formal/lean/MoltTIR/Semantics/EvalExpr.lean`
- `formal/lean/MoltTIR/Passes/CSE.lean`
- `formal/lean/MoltTIR/Passes/CSECorrect.lean`
- `formal/lean/MoltTIR/Passes/DCE.lean`
- `formal/lean/MoltTIR/Passes/DCECorrect.lean`
- `formal/lean/MoltTIR/Passes/EdgeThreadCorrect.lean`
- `formal/lean/MoltTIR/Passes/FullPipeline.lean`
- `formal/lean/MoltTIR/Passes/GuardHoist.lean`
- `formal/lean/MoltTIR/Passes/GuardHoistCorrect.lean`
- `formal/lean/MoltTIR/Passes/JoinCanonCorrect.lean`
- `formal/lean/MoltTIR/Passes/LICMCorrect.lean`
- `formal/lean/MoltTIR/Passes/Pipeline.lean`
- `formal/lean/MoltTIR/Passes/SCCPCorrect.lean`
- `formal/lean/MoltTIR/Passes/SCCPMultiCorrect.lean`
- `formal/lean/MoltTIR/Runtime/NanBox.lean`
- `formal/lean/MoltTIR/Runtime/NanBoxCorrect.lean`
- `formal/lean/MoltTIR/Runtime/NanBoxBV.lean` (new)
- `formal/lean/MoltTIR/Runtime/OwnershipModel.lean`
- `formal/lean/MoltTIR/Runtime/RCElisionCorrect.lean`
- `formal/lean/MoltTIR/Runtime/Refcount.lean`
- `formal/lean/MoltTIR/Runtime/WasmABI.lean`
- `formal/lean/MoltTIR/Runtime/WasmNative.lean`
- `formal/lean/MoltTIR/Runtime/WasmNativeCorrect.lean`
- `formal/lean/MoltTIR/Runtime/CapabilityGate.lean` (new)
- `formal/lean/MoltTIR/Runtime/IntrinsicContracts.lean` (new)
- `formal/lean/MoltTIR/Optimization/RefcountElision.lean`
- `formal/lean/MoltTIR/Backend/LuauCorrect.lean`
- `formal/lean/MoltTIR/Backend/LuauEmit.lean`
- `formal/lean/MoltTIR/Backend/LuauEnvCorr.lean`
- `formal/lean/MoltTIR/Backend/LuauSemantics.lean`
- `formal/lean/MoltTIR/Backend/LuauSyntax.lean`
- `formal/lean/MoltTIR/Backend/LuauTargetSemantics.lean` (new)
- `formal/lean/MoltTIR/Backend/RustCorrect.lean`
- `formal/lean/MoltTIR/Backend/RustEmit.lean`
- `formal/lean/MoltTIR/Backend/RustSemantics.lean`
- `formal/lean/MoltTIR/Backend/RustSyntax.lean`
- `formal/lean/MoltTIR/Backend/TargetIndependence.lean`
- `formal/lean/MoltTIR/Backend/WasmEmit.lean`
- `formal/lean/MoltTIR/Compilation/CompilationCorrectness.lean`
- `formal/lean/MoltTIR/Compilation/ForwardSimulation.lean`
- `formal/lean/MoltTIR/Determinism/BuildReproducibility.lean`
- `formal/lean/MoltTIR/Determinism/CompileDeterminism.lean`
- `formal/lean/MoltTIR/Determinism/CrossPlatform.lean`
- `formal/lean/MoltTIR/Simulation/Adequacy.lean`
- `formal/lean/MoltTIR/Simulation/Compose.lean`
- `formal/lean/MoltTIR/Simulation/FullChain.lean`
- `formal/lean/MoltTIR/Simulation/PassSimulation.lean`
- `formal/lean/MoltTIR/TypeSystem/TypeInference.lean`
- `formal/lean/MoltTIR/Validation/SCCPValid.lean`
- `formal/lean/MoltTIR/Validation/TranslationValidation.lean`

### Commit 2: `test: property-based + mutation + model-based + determinism tests`

**Files:**
- `tests/property/test_math_intrinsics.py` (new)
- `tests/property/test_string_intrinsics.py` (new)
- `tests/property/test_hash_intrinsics.py` (new)
- `tests/property/test_collection_intrinsics.py` (new)
- `tests/mutation/test_mutation_smoke.py` (modified)
- `tests/mutation/test_mutation_extended.py` (new)
- `tests/model_based/test_model_refcount.py` (new)
- `tests/model_based/test_model_optimization_pipeline.py` (new)
- `tests/model_based/test_model_determinism.py` (new)
- `tests/determinism/test_ir_determinism.py` (new)
- `tests/determinism/test_entropy_audit.py` (new)
- `tests/determinism/__init__.py` (new)

### Commit 3: `test: fuzz + backend + correspondence + TV tests`

**Files:**
- `tests/fuzz/test_fuzz_smoke.py` (modified)
- `tests/fuzz/test_fuzz_differential.py` (new)
- `tests/fuzz/test_fuzz_extended.py` (new)
- `tests/test_backend_parity.py` (new)
- `tests/test_lean_python_correspondence.py` (new)
- `tests/test_lean_rust_correspondence.py` (new)
- `tests/test_translation_validation_e2e.py` (new)
- `tests/test_translation_validator_unit.py` (new)
- `tests/test_luau_transpiler_correctness.py` (new)
- `tests/test_version_compat.py` (new)
- `tests/test_wasm_determinism.py` (new)
- `tests/test_wasm_optimization.py` (new)
- `tests/test_wasm_performance.py` (new)
- `tests/test_wasm_pipeline_e2e.py` (new)
- `tests/fixtures/tv_dumps/` (new, 6 JSON fixture files)

### Commit 4: `tools: correctness dashboard + translation validator + sorry counter`

**Files:**
- `tools/correctness_dashboard.py` (new)
- `tools/translation_validator.py` (new)
- `tools/check_lean_sorry_count.py` (new)
- `tools/check_correspondence_extended.py` (new)
- `tools/check_reproducible_build_extended.py` (new)
- `tools/check_determinism.py` (modified)
- `tools/check_translation_validation.py` (modified)
- `tools/mutation_test.py` (modified)
- `tools/quint_trace_to_tests.py` (modified)
- `tools/wasm_optimize.py` (new)
- `tools/wasm_pipeline.py` (new)
- `tools/wasm_size_audit.py` (new)
- `tools/update_linear_status.sh` (new)
- `src/molt/frontend/__init__.py` (modified)
- `src/molt/frontend/tv_hooks.py` (new)

### Commit 5: `ci: Lean gate + Kani workflow + expanded CI gates`

**Files:**
- `.github/workflows/ci.yml` (modified — includes re-added `formal-lean-check` job)
- `.github/workflows/kani.yml` (new)
- `tools/ci_gate.py` (modified)
- `.gitignore` (modified)

### Commit 6: `runtime: Kani harnesses + dependency updates`

**Files:**
- `runtime/molt-obj-model/Cargo.toml` (modified)
- `runtime/molt-runtime/Cargo.toml` (modified)
- `runtime/molt-obj-model/tests/kani_nanbox.rs` (new)
- `runtime/molt-obj-model/tests/kani_refcount.rs` (new)
- `runtime/molt-obj-model/tests/kani_intrinsic_contracts.rs` (new)
- `runtime/molt-runtime/tests/kani_string_ops.rs` (new)
- `runtime/molt-runtime/tests/kani_object.rs` (new)
- `Cargo.lock` (modified)
- `uv.lock` (modified)
- `bench/wasm_baseline.json` (new)
- `bench/wasm_bench.py` (new)

### Commit 7: `docs: certification status + proof maintenance + axiom inventory`

**Files:**
- `docs/spec/areas/formal/CERTIFICATION_STATUS.md` (new)

### Do NOT commit
- `.hypothesis/` — already in `.gitignore`
- `tests/determinism/__pycache__/` — already covered by `__pycache__/` gitignore pattern
- `formal/lean/.lake/config/` — build cache, exclude from commit
- `type_facts.json` — generated artifact, do not commit

---

## Notes

- `.hypothesis/` is already present in `.gitignore` (no action needed).
- The `formal-lean-check` CI job has been re-added to `.github/workflows/ci.yml`.
- Net sorry delta is -8 (45 removed, 37 added), indicating forward progress on proof closure.
- The largest single-file changes are in `PassPreservesSSA.lean` (+1,142) and `MoltLowering/Correct.lean` (+887), both major proof efforts.
