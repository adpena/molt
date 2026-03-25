# Lean Upgrade Plan: 4.16.0 -> 4.28.0

**Ticket:** MOL-295
**Date:** 2026-03-16
**Status:** COMPLETE (4.16 -> 4.28)

## 1. Current State

- **Lean version:** `leanprover/lean4:v4.28.0` (upgraded from 4.16.0)
- **lakefile.lean:** Standard Lake DSL config, no Mathlib dependency
- **Sorry count:** 0 (all sorrys closed)
- **NanBoxBV.lean:** `bv_decide` proofs drafted for BitVec-based NaN-boxing verification

## 2. Upgrade Summary

The upgrade from Lean 4.16.0 to 4.28.0 is **complete**. Key outcomes:

- All 109 Lean proof files build successfully on 4.28.0
- `native_decide` proofs remain stable across the version jump
- `bv_decide` is available for UInt64 goals (UInt64 support added in 4.17.0)
- No breaking changes encountered (the codebase does not use metaprogramming,
  FFI linking, or the iterator API)

## 3. `bv_decide` Status

The `bv_decide` proofs in `NanBoxBV.lean` are **drafted** but not yet the primary
proof path. The existing proofs in `NanBoxCorrect.lean` use manual BitVec reasoning
and are sorry-free.

### Drafted `bv_decide` Proofs

- `fused_xor_implies_isInt` -- Forward direction of XOR tag check. The `bv_decide`
  version bitblasts the 64-bit constraint and solves via SAT.
- `fused_xor_unbox` -- 47-bit sign-extension roundtrip. Uses `bv_decide` for the
  bitvector portion with manual `Int` lifting.

These drafts serve as reference for future simplification of the NaN-boxing proofs.
The current manual proofs are complete and correct; switching to `bv_decide` would
reduce proof size but is not required for correctness.

## 4. Breaking Changes Encountered

None. The upgrade was clean:

- `lean-toolchain` updated to `leanprover/lean4:v4.28.0`
- `lake update && lake build` succeeded without modifications
- No `Std.Iterators` namespace issues (we do not use iterators)
- No `letLambdaTelescope` / `mkLetFVars` issues (we do not use these combinators)
- No `TryThis` API issues (we do not use `TryThis`)

## 5. Original Motivation (Resolved)

The upgrade was originally motivated by 2 remaining sorry tactics in
`NanBoxCorrect.lean`:

- `fused_xor_implies_isInt` (line 676): forward direction of XOR tag check
- `fused_xor_unbox` (line 739): 47-bit sign-extension roundtrip

Both sorrys have been closed (via manual BitVec reasoning, not `bv_decide`).
The `bv_decide` drafts in `NanBoxBV.lean` remain as an alternative proof strategy.

## 6. Next Steps

| Item | Priority | Status |
|------|----------|--------|
| Switch NanBox proofs to `bv_decide` (optional) | P4 | Drafted in NanBoxBV.lean |
| Evaluate `bv_omega` for mixed BitVec/Int goals | P4 | Not started |
| Track Lean 4.29+ releases for further improvements | P4 | Monitoring |

No urgent action required. The upgrade is complete and all proofs are sorry-free.
