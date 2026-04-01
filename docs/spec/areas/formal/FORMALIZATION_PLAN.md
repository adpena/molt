# Molt Full Formalization & Verification Plan

## Status (updated 2026-04-01)

**Original claim:** 9 sorry-using declarations remain (down from 44).
**Actual count (2026-04-01):** ~128 `sorry` occurrences across ~20 Lean files. The `SorryAudit.lean` catalogs ~73 individual sorrys across ~48 theorems. 4 gaps closed with complete proofs. The heaviest concentrations: `Meta/Completeness.lean` (24), `Compilation/ForwardSimulation.lean` (12), `EndToEnd.lean` (8), pass correctness proofs (6 each).

**Codebase**: 66K Python, 150K Rust, 25K Lean across 1,228 files
**Verified**: Compilation correctness, NaN-boxing, WASM ABI, RC elision, 5/6 midend passes, dominator theory
**Also verified**: 152 Kani proof harnesses (CI-gated), 16 Quint models, 9 Hypothesis test suites
**Not present**: Miri (no CI workflow or config found)

NOTE: the historical 14-to-0 breakdown is stale. The 9-sorry figure referred to the
original session's *closing count*; many new sorry-using theorems were added as the
formalization scope expanded to cover TIR passes, backend determinism, and simulation.
The live inventory is `SorryAudit.lean` and the checker output.

---

## Phase 1: Complete Core Formalization (Current — 9 sorrys to 0)

### 1.1 Abstract Interpretation (1 sorry)
- `SCCPCorrect.absEvalExpr_sound` var case: needs a definedness invariant or a
  theorem restatement.

### 1.2 Lowering (2 sorrys)
- `lowerEnv_corr`
- `lowering_reflects_eval`

### 1.3 SSA Invariants (5 sorrys)
- `ssa_implies_wellformed` (2 sorrys)
- `cse_preserves_ssa` (1 sorry)
- `licm_preserves_ssa` (2 sorrys)

### 1.4 Validation (1 sorry)
- `SCCPValid.sccp_valid_transform` / worklist soundness chain

---

## Phase 2: Intrinsic Contract Specifications

### 2.1 Core Arithmetic & Type Contracts (Lean axioms)
30 most-used intrinsics with behavioral specifications:
- Integer ops (10): add/sub/mul/mod/floordiv/pow/neg/abs/eq/lt
- String ops (8): concat/repeat/len/getitem/contains/eq/lt/hash
- List ops (6): append/getitem/setitem/len/contains/concat
- Dict ops (4): getitem/setitem/contains/len
- Type ops (2): isinstance/type_check

### 2.2 Kani Bounded Verification (Rust)
For each Tier 1 axiom, Kani proof harness verifying the Rust
implementation satisfies the Lean spec for all inputs up to bound.

### 2.3 Property-Based Testing
proptest for intrinsics beyond Kani bound: Unicode strings,
large collections, edge cases.

---

## Phase 3: Stdlib Module Verification

### 3.1 Critical modules (88 Rust files, 153K lines, 1988 exported functions)
- builtins (~50 functions): Kani + diff tests
- str methods (~40): Kani + Unicode property tests
- list/dict/set methods (~75): Kani + bounds checking
- os/io (~15): Capability gate verification
- json/codecs (~10): Kani + roundtrip tests
- collections (~20): Kani + thread safety (Miri)

### 3.2 Capability gate verification
Every system-resource-accessing function verified:
- Gate checks present and correct (static analysis)
- Unauthorized access impossible (Kani proof)
- Gate bypass attempts fail deterministically (fuzz testing)

### 3.3 Cross-thread safety
Shared-state modules verified with Miri + Kani + Lean specs.

---

## Phase 4: Native & WASM Backend Verification

### 4.1 Cranelift codegen correctness
Differential testing against interpreter + selective Kani proofs.

### 4.2 WASM target verification
WASM ABI (already sorry-free), memory model, WASI capability mapping.

### 4.3 Binary size optimization verification
DCE completeness, dead import elimination, LTO correctness.

---

## Phase 5: Rust & Luau Transpiler Verification

### 5.1 Rust transpiler
Type mapping, ownership model, trait implementation correctness.

### 5.2 Luau transpiler
Expression emission (1 sorry: abs to neg), env correspondence (sorry-free).

---

## Phase 6: Determinism & Reproducibility

### 6.1 Build determinism (Quint model — verified)
### 6.2 Execution determinism (execFunc_deterministic — proven)
### 6.3 Cross-platform determinism (differential testing)

---

## Verification Stack

```
Lean 4 Formal Proofs (25K lines)
  Compilation correctness PROVEN
  NaN-boxing PROVEN
  SSA framework PROVEN
  Midend passes 5/6 PROVEN
  Intrinsic contracts (Phase 2)

Kani Model Checking (Phase 2-3)
  Intrinsic implementations
  Memory safety proofs
  Bounded arithmetic verification

Miri Runtime Analysis (existing)
  Use-after-free, data race, UB detection

Differential Testing (existing)
  CPython behavioral parity, 1000+ tests

Quint Model Checking (existing)
  Build determinism, pipeline invariants

Property Testing / Fuzzing (Phase 3)
  proptest, AFL/cargo-fuzz, Unicode edge cases
```

---

## Timeline

| Phase | Scope | Priority |
|-------|-------|----------|
| 1 | Close 9 sorrys | P0 |
| 2.1 | Intrinsic Lean specs (30) | P0 |
| 2.2 | Kani proofs (30 intrinsics) | P1 |
| 3.1 | Stdlib Kani (critical modules) | P1 |
| 3.2 | Capability gate verification | P0 |
| 4 | Backend verification | P2 |
| 5 | Transpiler verification | P2 |
| 6 | Determinism extension | P1 |
