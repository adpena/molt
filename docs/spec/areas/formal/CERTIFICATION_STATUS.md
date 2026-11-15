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

3. **Cross-backend equivalence proofs are vacuous.** Both backends (Native, WASM)
   are defined identically in the Lean model. The pairwise equivalence theorem
   holds by `rfl`. This proves consistency of the model, not actual behavioral
   equivalence of the real backends.

4. **Most running TIR pipeline passes have no formal proof.** Only constant
   folding, SCCP (partial), and DCE have Lean proofs. The remainder — including
   unboxing, escape analysis, refcount elision, dead-store elimination,
   strength reduction, bounds-check elimination, type guard hoisting,
   vectorization, polyhedral, and the lowering/canonicalization phases — are
   unproven. The pipeline body in `runtime/molt-passes/src/tir/passes/mod.rs`
   `run_pipeline` is the canonical source of pass identity and ordering.

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
| TIR pipeline pass coverage | **3** passes proven (constant folding, SCCP partial, DCE); see `tir::passes::run_pipeline` for the full set |

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

- **WasmCorrect.lean** -- Emission correctness for the `val` and `var` expression cases,
  instruction structure and operator totality. The Luau and Rust-transpile models were
  deleted with their targets on 2026-11-12; git history holds them.

### Certification Posture

| Property | Status | Notes |
|----------|--------|-------|
| End-to-end expression correctness | **Blocked** | 9 core-chain sorrys; ~95 additional sorrys in dependent files |
| Backend model proofs (WASM) | Partial | `WasmCorrect` proves the `val` and `var` emission cases only; no full `emitExpr_correct` (HF-63) |
| Native (Cranelift) backend proofs | **None** | Zero formal proofs for the production native backend |
| Cross-backend equivalence (WASM-Native) | **Vacuous** | Proven by `rfl` — all backends defined identically |
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

7. **`bv_decide` proofs (P3):** Lean 4.35.0 is pinned and provides `bv_decide`; use it
   for the NaN-boxing BitVec proofs in `NanBoxBV.lean`. See `LEAN_UPGRADE_PLAN.md`.

8. **Intrinsic axiom reduction (P4):** Some of the 61 intrinsic axioms are provable
   if the runtime builtins are given concrete definitions in the model (e.g.,
   `reversed_involution` follows from `List.reverse_reverse`). This would reduce the
   trust surface but requires modeling heap-allocated values.
