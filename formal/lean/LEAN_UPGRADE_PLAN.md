# Lean Upgrade Plan

**Current pin:** `leanprover/lean4:v4.34.1`
**Status:** COMPLETE (4.28.0 -> 4.34.1, 2026-10-06)
**History:** 4.16.0 -> 4.28.0 (MOL-295, 2026-03-16); 4.28.0 -> 4.34.1 (2026-10-06)

`formal/lean/lean-toolchain` is the one authority for the Lean version. This
file records how each upgrade went. It does not set the version.

## 1. Current State

- **Lean version:** `leanprover/lean4:v4.34.1` (Lake 5.0.0)
- **lakefile.lean:** Standard Lake DSL config, no Mathlib, no Lake packages
- **Default target:** `lake build` builds the `MoltTIR` library: 88 jobs, 0 errors
- **Other libraries:** `lake build MoltPython MoltLowering` builds 19 jobs, 0 errors
- **Sorry count:** 13, measured by `tools/check_lean_sorry_count.py`
  (the ratchet in `SORRY_BASELINE` is 28)
- **NanBoxBV.lean:** `bv_decide` drafts for the NaN-boxing proofs type-check on 4.34.1

## 2. Upgrade 4.28.0 -> 4.34.1

I read the release notes for every stable release from v4.29.0 to v4.34.1.
Two changes broke the default target. Three more changes added warnings.
No `sorry`, axiom, option, or heartbeat limit was added, and no theorem
statement changed.

### Breaks fixed

| Lean change | File | Fix |
|-------------|------|-----|
| v4.31.0 #13243: structure-instance patterns no longer fill fields from default values | `MoltTIR/Passes/GuardHoist.lean` | `instrGuardExpr` now matches `{ rhs := .un .not (.var x), .. }`. The old pattern filled `fast_int_hint := false` and `fast_float_hint := false` from the defaults, so an instruction with a type hint was never a guard. Guard identity now depends only on the RHS, as the docstring says. |
| On 4.34.1, `simp [...] at hp` turns membership in a `List.filter` of a closed list into a disjunction plus a residual `match`; 4.28.0 computed the filtered list | `MoltTIR/Meta/Completeness.lean` | `exprPasses_all_verified` first proves `exprTransformingPasses = [.constFold, .sccp, .cse]` by `rfl`, rewrites, then splits the membership with `List.mem_cons`. |

### Warnings fixed

| Lean change | Files | Fix |
|-------------|-------|-----|
| v4.34.0 #14501: `if_pos`/`if_neg` renamed to `ite_eq_left`/`ite_eq_right` | `Runtime/NanBoxBV.lean`, `Runtime/IntrinsicContracts.lean` | Use the new names (same signatures). |
| `List.Sublist.cons₂` deprecated | `SSA/Properties.lean` | Use `List.Sublist.cons_cons` (same signature). |
| New `linter.defProp`: a `def` whose type is a `Prop` | `Semantics/Determinism.lean`, `Simulation/Diagram.lean`, `Simulation/PassSimulation.lean`, `Compilation/ForwardSimulation.lean` | 10 declarations changed from `def` to `theorem`. The two re-exports `eval_det` and `exec_det` take their type from `type_of%`, so the statement is not written twice. |

### Changes checked and not hit

- v4.29.0 transparency change for implicit arguments, and v4.33.0
  `backward.isDefEq.respectTransparency.types`: no proof broke.
- v4.29.0 `noncomputable` semantics: no new annotation needed.
- v4.31.0 app-elaborator beta reduction and goal-tag changes: no proof broke.
- v4.32.0 `do` elaborator default: no module failed.
- v4.33.0 kernel `maxRecDepth` bound and v4.34.0 `bv_normalize` port to
  `Sym.simp`: no proof broke, and `NanBoxBV.lean` still type-checks.

### Whole-tree comparison

36 `.lean` files are not reachable from any Lake library root, so `lake build`
does not check them. I built every module under `MoltTIR/`, `MoltPython/` and
`MoltLowering/` on both 4.28.0 and 4.34.1 with a scratch Lake config. The same
14 modules fail on both versions, at the same 196 error sites. The upgrade
adds no failure. After the fixes above, the warning count is 221 on both
versions.

## 3. Upgrade 4.16.0 -> 4.28.0 (MOL-295)

- `lean-toolchain` updated to `leanprover/lean4:v4.28.0`.
- `lake update && lake build` succeeded without source changes.
- `bv_decide` became usable on `UInt64` goals (support added in 4.17.0).
- The two `NanBoxCorrect.lean` sorrys that motivated the upgrade
  (`fused_xor_implies_isInt`, `fused_xor_unbox`) were closed by manual
  BitVec reasoning. The `bv_decide` drafts in `NanBoxBV.lean` remain as an
  alternative proof strategy.

## 4. `bv_decide` Status

The `bv_decide` proofs in `NanBoxBV.lean` are drafts, not the primary proof
path. The manual proofs in `NanBoxCorrect.lean` are complete and sorry-free.

- `fused_xor_implies_isInt`: forward direction of the XOR tag check. The
  `bv_decide` version bit-blasts the 64-bit constraint and solves it with SAT.
- `fused_xor_unbox`: 47-bit sign-extension roundtrip. `bv_decide` handles the
  bitvector part; the `Int` lifting is manual.

## 5. Next Steps

| Item | Priority | Status |
|------|----------|--------|
| Lower `SORRY_BASELINE` from 28 to the measured 13 | P2 | Open |
| Repair or delete the 14 modules outside the Lake roots that fail on both 4.28.0 and 4.34.1 | P2 | Open |
| Switch NanBox proofs to `bv_decide` (optional) | P4 | Drafted in NanBoxBV.lean |
| Evaluate `bv_omega` for mixed BitVec/Int goals | P4 | Not started |
| Track Lean releases after 4.34.1 | P4 | Monitoring |
