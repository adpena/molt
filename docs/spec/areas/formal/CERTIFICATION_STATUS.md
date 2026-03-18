# Molt Compiler & Transpiler Correctness Certification Status

**Generated:** 2026-03-16
**Epic:** MOL-273 — Molt Compiler & Transpiler Correctness Certification
**Sub-task status:** 25 of 25 Done

---

## Overall Certification Percentage

| Metric | Value |
|--------|-------|
| Lean 4 proof files | 109 |
| Total theorem/lemma declarations | ~1,331 |
| Actual `sorry` tactics remaining | **9** |
| Files with any sorry | 5 of 109 (104 of 109 files are sorry-free) |
| Trust axioms (intentional) | **68** across 6 files |

The codebase currently contains 9 tactic `sorry`s across 5 proof files. The checker
ignores comment and documentation mentions of "sorry"; only tactic invocations count.
The open holes are concentrated in lowering, SSA preservation, and SCCP validation.

---

## Trust Axiom Summary

The 68 axioms are intentional trust-boundary declarations. They model properties of
external systems (hardware, runtime, toolchain) that cannot be proven within Lean.

| Category | Count | Files | Justification |
|----------|-------|-------|---------------|
| Intrinsic contracts (Python builtins) | 61 | `IntrinsicContracts.lean` | Python runtime behavior; validated by differential tests |
| IEEE 754 / hardware | 1 | `CrossPlatform.lean` | Hardware property; validated by cross-platform testing |
| SSA well-formedness | 1 | `PassSimulation.lean` | Compiler construction guarantee; validated by verifier |
| SCCP worklist soundness | 1 | `SCCPValid.lean` | Global induction over worklist; local steps proven |
| Build infrastructure | 3 | `BuildReproducibility.lean` | External toolchain (cache, Cranelift, linker) |
| Compile determinism | 1 | `CompileDeterminism.lean` | No-timestamp property; validated by differential tests |

See `formal/lean/AXIOM_INVENTORY.md` for the complete enumeration.

---

## Current Proof State

The sections below separate currently closed proofs from the remaining open holes.
Read the file-specific notes literally; the repository is not globally sorry-free.

### Closed Files

104 of 109 Lean proof files are currently sorry-free.

The remaining 5 files contain the 9 open tactic `sorry`s listed above.

### Backend Layer

The backend proof set is largely closed:

- **LuauCorrect.lean** -- Full semantic correctness (`emitExpr_correct`): structural
  induction proving that for every IR expression, if IR evaluation succeeds, Luau
  evaluation of the emitted expression succeeds with the corresponding value. Environment
  preservation (`emitInstr_preserves_env`). Index adjustment, builtin mapping, operator
  totality.

- **LuauTargetSemantics.lean** -- Deep formalization of Luau target semantics: extended
  value model (closures, userdata, tables), Luau-specific operations (# length,
  table.insert/remove, nil propagation), string semantics, type coercion rules,
  Python-Luau correspondence theorems.

- **RustCorrect.lean** -- Full semantic correctness (`emitRustExpr_correct`), parallel to
  Luau. Environment correspondence with injectivity. Type mapping totality and
  faithfulness. SSA ownership safety.

- **RustSyntax.lean / RustSemantics.lean / RustEmit.lean** -- Complete Rust AST subset,
  evaluation functions, and emission functions.

- **CrossBackend.lean** -- `all_backends_equiv`: all 4 backends (Native, WASM, Luau, Rust)
  produce identical observable behavior. All 6 pairwise equivalences.
  `pipeline_backend_equiv` and `optimized_equiv_unoptimized_any_backend`.

- **BackendDeterminism.lean** -- Per-backend emission determinism, observable behavior
  determinism, cross-compilation determinism, full pipeline determinism, artifact-level
  determinism.

- **TargetIndependence.lean** -- Lift-once-use-everywhere meta-theorem. Type safety,
  determinism, termination, and memory safety are all target-independent.

- **WasmNativeCorrect.lean** -- Integer arithmetic, NaN-boxing, string operations,
  memory layout, and function call convention all target-independent.

- **WasmABI.lean** -- WASM value types, NaN-boxed value representation, object header
  layout, pointer boxing, ABI consistency summary theorem.

### Cross-Platform Determinism (1 file, 0 sorrys, 1 axiom)

- **CrossPlatform.lean** -- NaN-boxing, integer operations, object layout, call
  convention, IR, expression evaluation, and optimization pipeline are all
  platform-independent. Uses `ieee754_basic_ops_deterministic` axiom (hardware property).

### Mid-Level Optimization Passes

- **ConstFoldCorrect.lean** -- Constant folding expression correctness, fully proven.
- **DCECorrect.lean** -- Dead code elimination instruction correctness.
- **CSECorrect.lean** -- Common subexpression elimination.
- **SCCPCorrect.lean** -- SCCP abstract evaluation soundness is available in the strong-invariant theorem; the weak `absEvalExpr_sound` var case still has 1 open `sorry`.
- **SCCPMultiCorrect.lean** -- Multi-block SCCP correctness.
- **LICMCorrect.lean** -- Loop-invariant code motion correctness.
- **GuardHoistCorrect.lean** -- Guard hoisting correctness (fully proven).
- **EdgeThreadCorrect.lean** -- Edge threading correctness.
- **JoinCanonCorrect.lean** -- Join canonicalization correctness.
- **Simulation/Adequacy.lean** -- `fullPipeline_contextual_equiv` is sorry-free.
- **Simulation/FullChain.lean** -- Three-phase composition (Phase 1 + 2 + 3).
- **Simulation/Compose.lean** -- constFold, SCCP, DCE, CSE pipeline composition.
- **Simulation/PassSimulation.lean** -- Pass simulation framework (1 trust axiom: `ssa_of_wellformed_tir`).

### SSA and Validation

- **SSA/Dominance.lean** -- SSA dominance properties.
- **SSA/Properties.lean** -- SSA structural properties.
- **SSA/PassPreservesSSA.lean** -- `cse_preserves_ssa` and `licm_preserves_ssa` still have 3 open `sorry`s in this file.
- **Validation/SCCPValid.lean** -- SCCP validation still has 1 open `sorry` in the worklist soundness chain.
- **Validation/ConstFoldValid.lean** -- Constant folding validation.
- **Validation/DCEValid.lean** -- DCE validation.
- **Validation/TranslationValidation.lean** -- Translation validation framework.

### Lowering

- **MoltLowering/Correct.lean** -- Lowering soundness is partially proven; `lowerEnv_corr` and `lowering_reflects_eval` still have 2 open `sorry`s.
- **MoltLowering/ASTtoTIR.lean** -- AST-to-TIR translation.
- **MoltLowering/Properties.lean** -- Lowering structural properties.

### Forward Simulation

- **Compilation/ForwardSimulation.lean** -- General simulation composition
  (`compose_simulations`) proven via Molt-specific receptiveness argument.
- **Compilation/CompilationCorrectness.lean** -- Full compilation correctness chain depends on the current lowering and SSA gaps.

### Meta-Theory (sorry-free)

- **Meta/Completeness.lean** -- Metatheory soundness, all proofs sorry-free.
- **Meta/SorryAudit.lean** -- Audit infrastructure and gap tracking (historical).

### End-to-End

- **EndToEnd.lean** -- Expression-level `endToEnd_correct` remains a closed theorem, but the overall pipeline is still gated by the open holes above.
- **EndToEndProperties.lean** -- End-to-end property preservation.

### Build Determinism (sorry-free, 4 trust axioms)

- **BuildReproducibility.lean** -- Multi-module build reproducibility. 3 trust axioms
  for external toolchain (cache correctness, Cranelift determinism, linker determinism).
- **CompileDeterminism.lean** -- Compile-time determinism. 1 trust axiom for
  no-timestamp-in-artifact.

### Runtime (sorry-free)

- **NanBoxCorrect.lean** -- NaN-boxing encode/decode correctness.
- **NanBoxBV.lean** -- BitVec-based NaN-boxing proofs (drafted for bv_decide).
- **IntrinsicContracts.lean** -- 61 trust axioms for Python builtin behavior.
- **MemorySafety.lean / MemorySafetyCorrect.lean** -- Memory safety model and proofs.
- **Refcount.lean / RCElisionCorrect.lean** -- Reference counting elision proofs.
- **OwnershipModel.lean** -- Ownership and lifetime model.
- **CapabilityGate.lean** -- Capability-based security gate.

---

## What is NOT Proven (trust axioms)

All 68 axioms are intentional trust-boundary declarations. They fall into two classes:

### Legitimate Trust Boundary (not closable within Lean)

These axioms model properties of external systems:

1. **`ieee754_basic_ops_deterministic`** -- IEEE 754 conformance for basic float
   operations. This is a hardware property validated by cross-platform differential tests.

2. **`cache_hit_correct`**, **`cranelift_deterministic`**, **`linker_deterministic`**,
   **`no_timestamp_in_artifact`** -- External toolchain properties. Validated by
   differential testing (same source -> same binary across runs).

3. **61 intrinsic contract axioms** -- Python builtin behavior (len, abs, bool, str,
   sorted, reversed, min, max, hash, type, isinstance, etc.). These model the runtime's
   behavior. Validated by the Python differential test suite (~3,500 test cases).

### Closable with More Infrastructure

These axioms encode compiler invariants that could be proven with additional formalization:

4. **`ssa_of_wellformed_tir`** -- Well-formed TIR is in SSA form. Closable by
   formalizing the SSA construction pass. Medium effort.

5. **`sccpWorklist_env_strongSound`** -- Multi-block SCCP worklist produces sound
   abstract environments. Closable by global induction over the worklist iteration
   coupled with execution-trace reachability. Hard effort.

---

## MOL-273 Epic Assessment

### Status: RECONCILIATION NEEDED

The epic "Molt Compiler & Transpiler Correctness Certification" is not yet complete.
All 25 sub-tasks are Done in Linear, but the formal verification codebase currently contains:

- **9 sorry tactics** (current)
- **68 trust axioms** (intentional, documented, validated by testing)
- **~1,331 theorems/lemmas** with complete proofs
- **104 Lean proof files**, sorry-free
- **5 Lean proof files**, with open tactic `sorry`s

### Certification Posture

| Property | Status |
|----------|--------|
| End-to-end expression correctness | In progress |
| All backend proofs (Luau, Rust, WASM, Native) | Mostly proven |
| Cross-backend equivalence (all 6 pairs) | In progress |
| All determinism proofs | Mostly proven |
| All optimization pass correctness | In progress |
| SSA preservation for all passes | In progress |
| Lowering soundness and reflection | In progress |
| Forward simulation composition | In progress |
| Build reproducibility | Proven (4 trust axioms for external toolchain) |
| Python runtime semantics | Axiomatized (61 trust axioms, validated by tests) |

### Recommendations for Future Work

1. **Axiom closure (P3):** The 2 closable axioms (`ssa_of_wellformed_tir`,
   `sccpWorklist_env_strongSound`) could be proven with additional formalization
   effort (~2-3 weeks). This would reduce the trust boundary to only the 66
   legitimate external-system axioms.

2. **Lean upgrade (P3):** Complete the upgrade to Lean 4.28 to use `bv_decide` for
   the NaN-boxing BitVec proofs in `NanBoxBV.lean`. See `LEAN_UPGRADE_PLAN.md`.

3. **Intrinsic axiom reduction (P4):** Some of the 61 intrinsic axioms are provable
   if the runtime builtins are given concrete definitions in the model (e.g.,
   `reversed_involution` follows from `List.reverse_reverse`). This would reduce the
   trust surface but requires modeling heap-allocated values.
