# Molt Compiler & Transpiler Correctness Certification Status

**Generated:** 2026-03-16
**Updated:** 2026-03-29 (comprehensive audit corrections)
**Epic:** MOL-273 — Molt Compiler & Transpiler Correctness Certification
**Sub-task status:** 25 of 25 Done

---

## Scope Disclaimer

The Lean formalization suite is architecturally ambitious and covers real ground, but
readers must understand its boundaries before drawing conclusions:

1. **Opcode coverage is partial.** Lean proofs cover 31 of 92 TIR opcodes (~34%).
   Missing categories include memory operations, calls, containers, iteration,
   exceptions, generators, imports, and the SCF dialect. See "Known Formalization Gaps"
   below for the full list.

2. **The Lean type system is simplified.** The Lean `Ty` inductive uses flat tags
   (int, float, str, bool, list, dict, set, tuple, none, object). The Rust `TirType`
   has a parametric lattice with `Union(Vec<TirType>)`, `Box(T)`, `DynBox`, `Func`,
   `Never`, `Ptr(T)`, `BigInt`, and a full `meet()` operation with union collapsing.
   The lattice meet — backbone of type refinement at SSA join points — has no formal
   counterpart.

3. **Cross-backend equivalence proofs are vacuous.** All 4 backends (Native, WASM,
   Luau, Rust) are defined identically in the Lean model. The 6 pairwise equivalence
   theorems hold by `rfl`. This proves consistency of the model, not actual behavioral
   equivalence of the real backends.

4. **5 of 8 running TIR pipeline passes have no formal proof.** Only constant
   folding, SCCP (partial), and DCE have Lean proofs. Unboxing, escape analysis,
   refcount elision, strength reduction, bounds-check elimination, and type guard
   hoisting are unproven.

5. **The native (Cranelift) backend has zero formal proofs.** This is the production
   backend for non-WASM targets.

6. **The Python frontend (~50K LOC) has zero formal coverage.** This includes AST
   visitors, midend passes (SCCP, CSE, DCE, LICM run at Python level), type hint
   propagation, and loop bound analysis. No proof connects the Python-level passes
   to the Rust-level passes or shows they compose correctly.

7. **Lean proofs build weekly, not per-PR.** Sorry count is not CI-gated. Regressions
   can go undetected for up to a week.

---

## Overall Certification Percentage

| Metric | Value |
|--------|-------|
| Lean 4 proof files | 111 |
| Total theorem/lemma declarations | ~1,331 |
| Actual `sorry` tactics remaining | **~104** (9 in core chain; ~95 in simulation, compilation, end-to-end, NanBoxBV, and meta files) |
| Files with any sorry | **29 of 111** (82 of 111 files are sorry-free) |
| Trust axioms (intentional) | **69** across 7 files (8 Lean-proper + 61 intrinsic contracts) |
| TIR opcode coverage (Lean) | **31 of 92** (34%) |
| TIR pipeline pass coverage | **3 of 8** running passes (37%) |

The 9 "core chain" sorrys (in lowering, SSA preservation, SCCP validation) are the ones
blocking the end-to-end correctness theorem. The remaining ~95 sorrys are spread across
simulation framework files, compilation correctness chain files, NanBoxBV (awaiting
`bv_decide` in Lean 4.17+), and meta/forward-reference files. Both categories need closure,
but the core chain sorrys are the critical path.

---

## Trust Axiom Summary

The axioms are intentional trust-boundary declarations. They model properties of
external systems (hardware, runtime, toolchain) that cannot be proven within Lean.

| Category | Count | Files | Justification | Status |
|----------|-------|-------|---------------|--------|
| Intrinsic contracts (Python builtins) | 61 | `IntrinsicContracts.lean` | Python runtime behavior; validated by differential tests | Some may be reducible (e.g., `reversed_involution` from `List.reverse_reverse`) |
| IEEE 754 / hardware | 1 | `CrossPlatform.lean` | Hardware property; validated by cross-platform testing | Legitimate trust boundary |
| SSA well-formedness | 1 | `PassSimulation.lean` | Compiler construction guarantee; validated by verifier | Closable with formalized SSA construction |
| SCCP worklist soundness | 1 | `PassSimulation.lean` | Global induction over worklist; local steps proven | Closable but hard |
| Instruction totality | 1 | `PassSimulation.lean` | All instructions have defined semantics | Closable by extending opcode model |
| Guard hoisting assumptions | 1 | `PassSimulation.lean` | Guard movement safety | Closable with dominator-tree formalization |
| Build infrastructure | 3 | `BuildReproducibility.lean` | External toolchain (cache, Cranelift, linker) | Legitimate trust boundary |
| Compile determinism | 1 | `CompileDeterminism.lean` | No-timestamp property; validated by differential tests | Legitimate trust boundary |

Note: The previous count of 68 axioms came from counting 61 intrinsic + 1 IEEE 754 +
1 SSA + 1 SCCP + 3 build + 1 compile = 68. The actual Lean axiom count is 69 (61 + 8)
due to the instruction totality and guard hoisting axioms in PassSimulation.lean that
were previously uncounted.

See `formal/lean/AXIOM_INVENTORY.md` for the complete enumeration.

---

## Known Formalization Gaps

### Missing Opcodes (61 of 92 unmodeled)

| Category | Missing Opcodes |
|----------|----------------|
| Memory | Alloc, StackAlloc, Free, LoadAttr, StoreAttr, DelAttr, Index, StoreIndex, DelIndex |
| Calls | Call, CallMethod, CallBuiltin |
| Box/Unbox | BoxVal, UnboxVal, TypeGuard |
| Refcount | IncRef, DecRef |
| Containers | BuildList, BuildDict, BuildTuple, BuildSet, BuildSlice |
| Iteration | GetIter, IterNext, ForIter |
| Generators | Yield, YieldFrom, StateBlockStart, StateBlockEnd |
| Exceptions | Raise, CheckException, TryStart, TryEnd |
| Imports | Import, ImportFrom |
| SCF dialect | ScfIf, ScfFor, ScfWhile, ScfYield |
| Deopt | Deopt |
| BinOps | And, Or, Is, IsNot, In, NotIn |
| UnOps | Pos |
| Terminators | Switch, Unreachable |

### Missing Type System Features

- `Union(Vec<TirType>)` — up to 3-member union types
- `Box(T)`, `DynBox` — NaN-boxed value model
- `Func(FuncSignature)` — callable types
- `Never` — bottom type
- `Ptr(T)` — pointer types
- `BigInt` — arbitrary-precision integers
- Lattice `meet()` with union collapsing — backbone of SSA join refinement

### Backend Proof Limitations

- **Vacuous equivalence:** All backends defined as the same function; `rfl` proves nothing about real behavior
- **Operator approximations in Luau proofs:** `bit_xor` → `land`, `lshift` → `land`, `rshift` → `land`
- **Operator approximations in Rust proofs:** `floordiv` → `div` (wrong for negatives), `pow` → `mul`
- **WASM proofs:** Bitwise ops use placeholder semantics
- **No unboxed fast-path modeling:** Rust backends emit type-specialized code; proofs assume all NaN-boxed

### Pipeline Pass Coverage

| Pass | In Pipeline? | Lean Proof? | Drift from Rust? |
|------|-------------|-------------|-------------------|
| Unboxing | Yes | No | — |
| Escape analysis | Yes | No | — |
| SCCP | Yes | Partial (1 sorry) | **Major** — Lean is single-pass forward; Rust is iterative fixpoint, exception-aware |
| DCE | Yes | Yes | **Major** — Lean is simple filter; Rust is two-phase reachability + cascading (10 rounds) |
| Constant folding | Lean only | Yes | N/A — not in Rust pipeline |
| CSE | Lean only | Yes | N/A — not in Rust pipeline |
| Refcount elision | Yes | No | — |
| Strength reduction | Yes | No | — |
| Bounds-check elimination | Yes | No | — |
| Type guard hoisting | Yes | No | — |

### Frontend/Lowering Gaps

- Python AST → SimpleIR: 50K+ LOC, zero formal coverage
- SimpleIR → TIR JSON: undocumented format (defined by Rust deserialization)
- CFG construction: not proven
- SSA conversion: tested but not verified
- Type refinement: 720 lines, zero coverage
- Python-level and Rust-level passes run independently with no equivalence proof

---

## Current Proof State

The sections below separate currently closed proofs from the remaining open holes.
Read the file-specific notes literally; the repository is not globally sorry-free.

### Closed Files

82 of 111 Lean proof files are currently sorry-free.

The remaining 29 files contain ~104 open tactic `sorry`s. Of these, 9 are in the core
proof chain (lowering, SSA preservation, SCCP validation) and block the end-to-end
theorem. The rest are in simulation framework, compilation correctness, NanBoxBV, and
meta files.

### Backend Layer

The backend proof set models backends structurally. The proofs are internally consistent
but vacuous with respect to actual backend behavior (see Scope Disclaimer above):

- **LuauCorrect.lean** -- Full semantic correctness (`emitExpr_correct`): structural
  induction proving that for every IR expression, if IR evaluation succeeds, Luau
  evaluation of the emitted expression succeeds with the corresponding value. Environment
  preservation (`emitInstr_preserves_env`). Index adjustment, builtin mapping, operator
  totality. Note: operator approximations (bit_xor, shifts map to land).

- **LuauTargetSemantics.lean** -- Deep formalization of Luau target semantics: extended
  value model (closures, userdata, tables), Luau-specific operations (# length,
  table.insert/remove, nil propagation), string semantics, type coercion rules,
  Python-Luau correspondence theorems.

- **RustCorrect.lean** -- Full semantic correctness (`emitRustExpr_correct`), parallel to
  Luau. Environment correspondence with injectivity. Type mapping totality and
  faithfulness. SSA ownership safety. Note: operator approximations (floordiv → div,
  pow → mul).

- **RustSyntax.lean / RustSemantics.lean / RustEmit.lean** -- Complete Rust AST subset,
  evaluation functions, and emission functions.

- **CrossBackend.lean** -- `all_backends_equiv`: all 4 backends produce identical
  observable behavior. All 6 pairwise equivalences hold by `rfl` because all backends
  are defined as the same function. **This proves model consistency, not real behavioral
  equivalence.**

- **BackendDeterminism.lean** -- Per-backend emission determinism, observable behavior
  determinism, cross-compilation determinism, full pipeline determinism, artifact-level
  determinism. **All theorems proven by `rfl` (trivially true by construction).**

- **TargetIndependence.lean** -- Lift-once-use-everywhere meta-theorem. Type safety,
  determinism, termination, and memory safety are all target-independent. Vacuous:
  backends are identical in the model.

- **WasmNativeCorrect.lean** -- Integer arithmetic, NaN-boxing, string operations,
  memory layout, and function call convention. Bitwise ops use placeholder semantics.

- **WasmABI.lean** -- WASM value types, NaN-boxed value representation, object header
  layout, pointer boxing, ABI consistency summary theorem.

### Cross-Platform Determinism (1 file, 0 sorrys, 1 axiom)

- **CrossPlatform.lean** -- NaN-boxing, integer operations, object layout, call
  convention, IR, expression evaluation, and optimization pipeline are all
  platform-independent. Uses `ieee754_basic_ops_deterministic` axiom (hardware property).

### Mid-Level Optimization Passes

- **ConstFoldCorrect.lean** -- Constant folding expression correctness, fully proven.
  Note: constant folding is not in the Rust pipeline; this proves a pass that only
  exists in the Lean model.
- **DCECorrect.lean** -- Dead code elimination instruction correctness. Note: the Lean
  model is a simple single-pass filter; the Rust implementation uses two-phase
  reachability with cascading (10 rounds). Significant drift.
- **CSECorrect.lean** -- Common subexpression elimination. Not in the Rust pipeline.
- **SCCPCorrect.lean** -- SCCP abstract evaluation soundness is available in the
  strong-invariant theorem; the weak `absEvalExpr_sound` var case still has 1 open
  `sorry`. Note: major drift from Rust (single-pass vs. iterative fixpoint).
- **SCCPMultiCorrect.lean** -- Multi-block SCCP correctness.
- **LICMCorrect.lean** -- Loop-invariant code motion correctness.
- **GuardHoistCorrect.lean** -- Guard hoisting correctness (fully proven).
- **EdgeThreadCorrect.lean** -- Edge threading correctness.
- **JoinCanonCorrect.lean** -- Join canonicalization correctness.
- **Simulation/Adequacy.lean** -- `fullPipeline_contextual_equiv` is sorry-free.
- **Simulation/FullChain.lean** -- Three-phase composition (Phase 1 + 2 + 3).
- **Simulation/Compose.lean** -- constFold, SCCP, DCE, CSE pipeline composition.
- **Simulation/PassSimulation.lean** -- Pass simulation framework (3 trust axioms:
  `ssa_of_wellformed_tir`, instruction totality, guard hoisting).

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

### Meta-Theory

- **Meta/Completeness.lean** -- Metatheory soundness.
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
- **NanBoxBV.lean** -- BitVec-based NaN-boxing proofs (drafted for bv_decide). Contains
  sorrys awaiting Lean 4.17+ `bv_decide` tactic.
- **IntrinsicContracts.lean** -- 61 trust axioms for Python builtin behavior.
- **MemorySafety.lean / MemorySafetyCorrect.lean** -- Memory safety model and proofs.
- **Refcount.lean / RCElisionCorrect.lean** -- Reference counting elision proofs.
- **OwnershipModel.lean** -- Ownership and lifetime model.
- **CapabilityGate.lean** -- Capability-based security gate.

---

## What is NOT Proven (trust axioms)

All 69 axioms are intentional trust-boundary declarations. They fall into two classes:

### Legitimate Trust Boundary (not closable within Lean)

These axioms model properties of external systems:

1. **`ieee754_basic_ops_deterministic`** -- IEEE 754 conformance for basic float
   operations. This is a hardware property validated by cross-platform differential tests.

2. **`cache_hit_correct`**, **`cranelift_deterministic`**, **`linker_deterministic`**,
   **`no_timestamp_in_artifact`** -- External toolchain properties. Validated by
   differential testing (same source -> same binary across runs).

3. **61 intrinsic contract axioms** -- Python builtin behavior (len, abs, bool, str,
   sorted, reversed, min, max, hash, type, isinstance, etc.). These model the runtime's
   behavior. Validated by the Python differential test suite (~3,500 test cases). Some
   are theoretically closable if builtins are given concrete definitions.

### Closable with More Infrastructure

These axioms encode compiler invariants that could be proven with additional formalization:

4. **`ssa_of_wellformed_tir`** -- Well-formed TIR is in SSA form. Closable by
   formalizing the SSA construction pass. Medium effort.

5. **`sccpWorklist_env_strongSound`** -- Multi-block SCCP worklist produces sound
   abstract environments. Closable by global induction over the worklist iteration
   coupled with execution-trace reachability. Hard effort.

6. **Instruction totality** -- All instructions have defined semantics. Closable by
   extending the opcode model to cover all 92 opcodes (currently 31).

7. **Guard hoisting assumptions** -- Guard movement safety. Closable with
   dominator-tree formalization.

---

## MOL-273 Epic Assessment

### Status: RECONCILIATION NEEDED

The epic "Molt Compiler & Transpiler Correctness Certification" is not yet complete.
All 25 sub-tasks are Done in Linear, but the formal verification codebase currently contains:

- **~104 sorry tactics** across 29 files (9 in the core chain blocking end-to-end; ~95 elsewhere)
- **69 trust axioms** (intentional, documented, validated by testing)
- **~1,331 theorems/lemmas** with complete proofs
- **82 Lean proof files**, sorry-free
- **29 Lean proof files**, with open tactic `sorry`s
- **31 of 92 opcodes** formalized (34%)
- **3 of 8 pipeline passes** formally proven (37%)
- **0 of ~50K LOC** Python frontend formalized

### What IS Proven (substantial)

The formalization achieves genuine results within its scope:

- NaN-boxing encode/decode correctness (machine-checked, sorry-free)
- Expression-level constant folding, DCE, CSE correctness (within simplified model)
- Reference counting elision safety
- Memory safety model with ownership semantics
- Build reproducibility (modulo 4 legitimate external axioms)
- Capability-based security gate properties
- Forward simulation composition framework
- ~1,300+ closed theorems across the proof suite

### What is NOT Proven (gaps to close)

- End-to-end compilation correctness (blocked by 9 core-chain sorrys)
- Any property of the native (Cranelift) backend
- Any property of the Python frontend
- Correctness of 5 of 8 running TIR passes
- Correctness for 61 of 92 opcodes
- Type system fidelity (simplified flat model vs. parametric lattice)
- Cross-backend behavioral equivalence (only model consistency proven)

### Certification Posture

| Property | Status | Notes |
|----------|--------|-------|
| End-to-end expression correctness | **Blocked** | 9 core-chain sorrys; ~95 additional sorrys in dependent files |
| Backend model proofs (Luau, Rust, WASM) | Partial | Structurally proven but with operator approximations |
| Native (Cranelift) backend proofs | **None** | Zero formal proofs for the production native backend |
| Cross-backend equivalence (all 6 pairs) | **Vacuous** | Proven by `rfl` — all backends defined identically |
| All determinism proofs | **Vacuous** | Proven by `rfl` — trivially true by construction |
| Optimization pass correctness (pipeline) | Partial | 3 of 8 running passes proven; 2 proven passes (ConstFold, CSE) not in Rust pipeline |
| SSA preservation for all passes | Partial | 3 sorrys in PassPreservesSSA.lean |
| Lowering soundness and reflection | Partial | 2 sorrys in Correct.lean |
| Forward simulation composition | Partial | Framework proven; blocked by downstream sorrys |
| Build reproducibility | Proven | 4 trust axioms for external toolchain |
| Python runtime semantics | Axiomatized | 61 trust axioms, validated by tests |
| TIR opcode coverage | Partial | 31 of 92 opcodes (34%) |
| Type system fidelity | **Gap** | Flat tags vs. parametric lattice with meet |
| Python frontend | **None** | ~50K LOC with zero formal coverage |

### Recommendations for Future Work

1. **Axiom closure (P2):** The 4 closable axioms (`ssa_of_wellformed_tir`,
   `sccpWorklist_env_strongSound`, instruction totality, guard hoisting) could be
   proven with additional formalization effort (~4-6 weeks). This would reduce the
   trust boundary to only the 65 legitimate external-system axioms.

2. **Core sorry closure (P1):** The 9 core-chain sorrys block the end-to-end theorem.
   Resolving the SCCPCorrect.lean type errors would unblock the downstream chain
   (Diagram → PassSimulation → Compose → FullChain → CompilationCorrectness → EndToEnd).

3. **Lean CI upgrade (P1):** Move `lake build` from weekly nightly to per-PR. Add a
   sorry-count gate to prevent regressions.

4. **Non-vacuous backend proofs (P2):** Define backend-specific emission models so
   cross-backend equivalence is not trivially `rfl`.

5. **Extend opcode model (P2):** Prioritize memory, call, and container opcodes to
   increase coverage from 34% toward 60%+.

6. **Lean type system extension (P2):** Add Union, Box/DynBox, Func, Never to match
   the Rust type system. Without this, type-driven passes cannot be meaningfully proven.

7. **Lean upgrade (P3):** Complete the upgrade to Lean 4.28 to use `bv_decide` for
   the NaN-boxing BitVec proofs in `NanBoxBV.lean`. See `LEAN_UPGRADE_PLAN.md`.

8. **Intrinsic axiom reduction (P4):** Some of the 61 intrinsic axioms are provable
   if the runtime builtins are given concrete definitions in the model (e.g.,
   `reversed_involution` follows from `List.reverse_reverse`). This would reduce the
   trust surface but requires modeling heap-allocated values.
