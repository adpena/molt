# Molt Formalization Suite — Comprehensive Audit

**Date:** 2026-03-29
**Scope:** Full-stack review of Lean 4 proofs, Quint models, Rust TIR implementation, Python frontend, all backends, test infrastructure
**Method:** 10-agent parallel audit covering every layer

---

## Executive Summary

The Molt formalization suite is **architecturally ambitious** but has **significant gaps between what the documentation claims and what the proofs actually cover**. The Lean proofs are mathematically sound within their scope, but that scope is ~35% of the actual TIR surface. Several proofs model idealized versions of algorithms rather than the actual implementation. Critical infrastructure gaps (no PR-level CI for proofs, vacuous cross-backend theorems, operator approximations) undermine confidence.

### Key Metrics (Honest Assessment)

| Metric | Documented | Actual |
|--------|-----------|--------|
| Sorry count | 9 across 5 files | ~104 across 29 files |
| Axiom count | 68 across 6 files | 8 in Lean proper (61 intrinsic axioms may be reduced) |
| Opcode coverage (Lean) | Not stated | 31 of 92 (34%) |
| Type system coverage | Not stated | Flat tags only; no Union/Box/DynBox/Func/Never/Ptr |
| Pipeline pass coverage | Not stated | 3 of 8 running passes proven (37%) |
| Backend proof fidelity | "All sorry-free" | Vacuous (all defined as same function) |
| Cross-backend equivalence | "All 6 pairs proven" | Proven by `rfl` — definitions identical |
| Native backend proofs | Implied complete | ZERO proofs |
| Frontend formal coverage | Not stated | ZERO (50K+ LOC unformalized) |
| Test opcode coverage | Not stated | 56% tested, 44% zero coverage |
| Quint models in CI | 16 models | 3 of 16 gated |

---

## Layer 1: Lean Proof Quality

### 1.1 Sorry Inventory (Actual)

The documented 9 sorrys are concentrated in the core chain. However, full audit reveals ~104 sorry occurrences across 29 files when counting:
- Forward references in Meta/ files
- Function-level simulation gaps in Compilation/
- Blocked proofs downstream of SCCPCorrect.lean type errors
- NanBoxBV.lean sorrys awaiting Lean 4.17+ bv_decide

**Blocking chain:** SCCPCorrect.lean type errors → Diagram.lean → PassSimulation.lean → Compose.lean → FullChain.lean → CompilationCorrectness.lean → EndToEnd.lean

### 1.2 Proof Substance

Most proofs are substantive (structural induction, lattice arguments). However:
- `BackendDeterminism.lean`: All theorems proven by `rfl` (trivially true)
- `CrossBackend.lean`: All equivalence theorems proven by `rfl` (vacuous)
- `TargetIndependence.lean`: Meta-theorem that lifts nothing (backends identical by construction)

### 1.3 Axiom Assessment

8 axioms in Lean files (legitimate trust boundaries):
- 3 in BuildReproducibility.lean (cache, Cranelift, linker determinism)
- 1 in CompileDeterminism.lean (no timestamps)
- 1 in CrossPlatform.lean (IEEE 754)
- 3 in PassSimulation.lean (SSA well-formedness, instruction totality, guard hoisting)

61 intrinsic contract axioms in IntrinsicContracts.lean — status unclear whether reduced.

---

## Layer 2: Lean ↔ Rust Correspondence

### 2.1 Type System Divergence (CRITICAL)

| Feature | Lean `Ty` | Rust `TirType` |
|---------|-----------|-----------------|
| Parametric containers | Flat (list, dict, set, tuple) | `List(T)`, `Dict(K,V)`, `Set(T)`, `Tuple(Vec<T>)` |
| Union types | — | `Union(Vec<TirType>)` up to 3 members |
| Box/Unbox model | — | `Box(T)`, `DynBox` (NaN-boxed) |
| Callable types | — | `Func(FuncSignature)` |
| Bottom type | — | `Never` |
| Pointer types | — | `Ptr(T)` |
| BigInt | — | `BigInt` |
| Lattice meet | — | Full `meet()` with union collapsing |

**Impact:** The lattice meet operation — backbone of type refinement at SSA join points — has no formal counterpart. All type-driven passes depend on it.

### 2.2 Opcode Divergence (CRITICAL)

**Lean covers 31 opcodes.** Rust implements 92. Missing categories:
- Memory: Alloc, StackAlloc, Free, LoadAttr, StoreAttr, DelAttr, Index, StoreIndex, DelIndex
- Calls: Call, CallMethod, CallBuiltin
- Box/Unbox: BoxVal, UnboxVal, TypeGuard
- Refcount: IncRef, DecRef
- Containers: BuildList, BuildDict, BuildTuple, BuildSet, BuildSlice
- Iteration: GetIter, IterNext, ForIter
- Generators: Yield, YieldFrom, StateBlockStart, StateBlockEnd
- Exceptions: Raise, CheckException, TryStart, TryEnd
- Imports: Import, ImportFrom
- SCF dialect: ScfIf, ScfFor, ScfWhile, ScfYield
- Deopt: Deopt
- Missing BinOps: And, Or, Is, IsNot, In, NotIn
- Missing UnOps: Pos
- Missing Terminators: Switch, Unreachable

### 2.3 Pass Algorithm Drift

| Pass | Lean Model | Rust Implementation | Drift |
|------|-----------|-------------------|-------|
| SCCP | Single-pass forward scan | Iterative fixpoint, exception-aware | **MAJOR** |
| DCE | Simple single-pass filter | Two-phase with reachability + cascading (10 rounds) | **MAJOR** |
| GuardHoist | Toy abstract guard model | Real loop detection via back-edges, dominator preheaders | **MAJOR** |
| ConstFold | Matches well | Not in Rust pipeline | N/A (not used) |
| CSE | Matches well | Not in Rust pipeline | N/A (not used) |

### 2.4 NaN-Boxing Bit Patterns

Runtime constants (Rust molt-obj-model): Match Lean NanBox.lean ✓
WASM backend constants (Rust lower_to_wasm.rs): Use same QNAN `0x7ff8_0000_0000_0000` ✓

Note: One agent reported a potential QNAN mismatch (`0x7ff0` vs `0x7ff8`). This needs manual verification — may be CANONICAL_NAN_BITS vs QNAN distinction.

---

## Layer 3: Backend Proofs

### 3.1 Cross-Backend Equivalence (VACUOUS)

`CrossBackend.lean` defines all 4 backends identically:
```lean
def extractObservableNative (f : Func) (fuel : Nat) : ObservableBehavior :=
  observableFromOutcome (runFunc f fuel)
-- Same definition for WASM, Luau, Rust
```

The equivalence theorem is `rfl`. This proves nothing about actual backend behavior.

### 3.2 Backend-Specific Issues

| Backend | Proofs | Critical Issue |
|---------|--------|----------------|
| **Native (Cranelift)** | ZERO | Production backend, completely unformalized |
| **WASM** | Structural only | Bitwise ops use placeholder semantics |
| **Luau** | Structural only | bit_xor → land, lshift → land, rshift → land |
| **Rust transpiler** | Structural only | floordiv → div (wrong for negatives), pow → mul |

### 3.3 Backend-Specific Type Specialization

Rust backends emit unboxed fast paths based on type inference. Lean proofs assume all values are NaN-boxed. This entire optimization surface is outside formal scope.

---

## Layer 4: Frontend & Lowering

### 4.1 Python Frontend (ZERO formal coverage)

`SimpleTIRGenerator` (~50K LOC) is entirely unformalized:
- AST visitors for all Python constructs
- Midend passes (SCCP, CSE, DCE, LICM) run at Python level
- Type hint propagation (fast_int, fast_float)
- Loop bound analysis (affine range)

### 4.2 SimpleIR → TIR Lowering

- **JSON IR format:** Undocumented (implicitly defined by Rust deserialization)
- **CFG construction:** NOT proven (implicit nesting invariants)
- **SSA conversion:** Tested but not formally verified
- **Type refinement:** 720 lines, ZERO formal coverage

### 4.3 Midend Pass Duplication

Passes run in Python AND Rust:
- Python-level: SCCP, CSE, DCE, LICM, guard hoisting (unproven)
- TIR-level: unboxing, escape analysis, refcount elim, type guard hoist, SCCP, strength reduction, BCE, DCE (3 of 8 proven)
- **No proof that the two levels are equivalent or compose correctly**

---

## Layer 5: Rust TIR Implementation Issues

### 5.1 Verification Fallback Bug (CRITICAL)

`passes/mod.rs` mutates `TirFunction` in-place via `run_pipeline`, then attempts to "fall back" on verification failure. But the original is already mutated — returns corrupted IR.

### 5.2 Deopt Framework Stubbed

`deopt.rs` has `generate_deopt_handler()` but handlers are never inserted into IR. Framework is architectural but non-functional. Only exercised in tests.

### 5.3 Incomplete verify_function()

Does NOT check:
- Op-level attributes (ConstInt without `value`, Call without `callee`)
- Operand count matches opcode expectations
- Return type consistency
- Exception-throwing ops are guarded or in try regions

### 5.4 Loop Detection Heuristic

`type_guard_hoist.rs` uses BlockId ordering for back-edge detection. This is NOT dominator-based and can be wrong in irregular CFGs.

---

## Layer 6: Test & CI Infrastructure

### 6.1 Opcode Test Coverage

- **Roundtrip tests:** 15 of 92 opcodes (16%)
- **Fuzzer palette:** 19 of 92 opcodes (20%)
- **Differential tests:** 708 tests, 95.9% language feature coverage
- **Combined:** ~56% of opcodes have some test; 44% have ZERO

### 6.2 CI Gaps

| Check | Runs in CI? | Frequency |
|-------|------------|-----------|
| Rust tests | ✅ | Every PR |
| Differential tests | ✅ | Every PR |
| Kani proofs | ✅ | Every PR |
| `lake build` (Lean) | ⚠️ | Weekly nightly only |
| Sorry count gate | ❌ | Never |
| `check_molt_ir_ops.py` | ❌ | Never |
| Quint models (13 of 16) | ❌ | Never |
| Fuzzer regression | ❌ | Never |

---

## Layer 7: Quint Models

16 Quint models exist covering:
- Build determinism, runtime determinism, cross-version semantics
- Midend pipeline, optimization pipeline
- NaN-boxing object model, box/unbox operations, GC safety
- Luau transpiler, calling convention
- Control flow, exception handling, concurrency
- Cache coherence, scheduler fairness, refcount protocol

**Only 3 run in CI** (build_determinism, runtime_determinism, midend_pipeline). The other 13 — including critical ones like exception_handling, gc_safety, and concurrency — are not gated.

---

## Layer 8: Advanced Features

8 advanced optimization passes exist but are NOT in the pipeline:
- CHA, monomorphize, deforestation, vectorize, polyhedral, fast_math, interprocedural, closure_spec
- Deforestation and interprocedural are production-ready
- Polyhedral is a stub (should NOT be enabled)
- GPU infrastructure is partial (CUDA, Metal, WebGPU codegen)
- MLIR bridge is a feature-gated stub

---

## Prioritized Remediation Plan

### P0 — Soundness (Must Fix)

1. **Fix verification fallback bug** — Clone TirFunction before run_pipeline, return clone on failure
2. **Fix backend operator approximations** — At minimum, document as known-unsound; ideally fix semantics
3. **Update CERTIFICATION_STATUS.md** — Honest sorry counts, real coverage percentages, scope disclaimers
4. **Add op-level attribute validation** to verify_function()

### P1 — Formalization Integrity (Should Fix Soon)

5. **Extend Lean Ty to match TirType** — Add Union, Box/DynBox, Func, Never, BigInt, Ptr, lattice meet
6. **Extend Lean BinOp/UnOp/Terminator** — Add missing 8+ operators and Switch/Unreachable
7. **Make cross-backend proofs non-vacuous** — Define backend-specific emission models
8. **Add Lean build to PR CI** — Not just weekly nightly
9. **Gate all 16 Quint models in CI**

### P2 — Coverage (Should Fix)

10. **Formalize unboxing and escape analysis passes** — First 2 passes in pipeline, feed all others
11. **Expand fuzzer to 60+ opcodes** and roundtrip tests to cover all opcode categories
12. **Prove CFG construction correctness** — or add comprehensive validation
13. **Formalize type refinement** — 720 lines on the critical path with zero coverage
14. **Fix loop detection** to use dominator-based back-edges instead of BlockId heuristic

### P3 — Completeness (Aspirational)

15. **Formalize remaining 5 pipeline passes** (refcount_elim, strength_reduction, bce, type_guard_hoist, sccp fixpoint)
16. **Add native backend proofs** (Cranelift — likely translation validation approach)
17. **Formalize Python frontend** (at least control flow marker invariants)
18. **Close 9 core sorrys** blocking the end-to-end chain
19. **Wire deforestation and interprocedural into pipeline** (ready, well-tested)

---

## Appendix: Agent Reports

This audit was conducted by 10 specialized agents:
1. Lean proof quality — sorry/axiom inventory, proof substance
2. Rust TIR vs spec — implementation vs specification documents
3. Frontend lowering — Python→SimpleIR→TIR pipeline
4. Backend codegen — WASM/Cranelift/Luau/Rust vs Lean proofs
5. Runtime formalization — NaN-boxing, RC, capabilities vs Rust
6. Test/fuzz/CI — coverage gaps, tooling
7. Advanced TIR features — GPU/SIMD/parallel, advanced passes
8. Quint/TLA+ models — all formal methods beyond Lean
9. Lean↔codebase drift — opcode correspondence, recent changes
10. End-to-end proof chain — compilation correctness dependencies
