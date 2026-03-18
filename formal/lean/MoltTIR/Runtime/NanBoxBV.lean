/-
  MoltTIR.Runtime.NanBoxBV — DRAFT: NanBox sorry closures via bv_decide

  Lean toolchain upgraded to 4.28.0 — bv_decide now available with UInt64 support.

  This file shows how the 2 remaining sorry obligations
  in NanBoxCorrect.lean can be closed using the `bv_decide` tactic, which
  reduces bitvector goals to SAT problems solved by the built-in CaDiCaL solver.

  Background:
  - `bv_decide` was introduced in Lean 4.12.0 for BitVec/Bool goals
  - Lean 4.17.0 added a UInt64 preprocessor (PR #6711) so bv_decide works
    directly on UInt64 without manual toBitVec conversion
  - The `bv_omega` tactic handles mixed bitvector/integer arithmetic

  References:
  - formal/lean/MoltTIR/Runtime/NanBoxCorrect.lean (original proofs with sorrys)
  - formal/lean/LEAN_UPGRADE_PLAN.md (upgrade plan)
  - https://lean-lang.org/doc/reference/latest/releases/v4.17.0/ (UInt64 in bv_decide)

  Ticket: MOL-295
-/
import MoltTIR.Runtime.NanBox
import Std.Tactic.BVDecide

set_option autoImplicit false

namespace MoltTIR.Runtime.NanBoxBV

open MoltTIR.Runtime

-- ══════════════════════════════════════════════════════════════════
-- Definitions re-exported from NanBoxCorrect (needed for the theorems)
-- ══════════════════════════════════════════════════════════════════

def INT_WIDTH : Nat := 47
def INT_SHIFT : Nat := 17
def EXPECTED_INT_TAG : UInt64 := QNAN ||| TAG_INT

/-- XOR a NaN-boxed value against the expected int tag pattern. -/
def xorTagCheck (bits : UInt64) : UInt64 := bits ^^^ EXPECTED_INT_TAG

/-- The fused tag check: (xored >>> 47) == 0 iff the value was an int. -/
def fusedIsInt (bits : UInt64) : Bool :=
  ((xorTagCheck bits) >>> (47 : UInt64)) == (0 : UInt64)

/-- An integer fits in the 47-bit inline representation. -/
def intFitsInline (n : Int) : Prop := -2^46 ≤ n ∧ n < 2^46

/-- Sign-extend a 47-bit value to a full 64-bit signed integer. -/
def signExtend47 (v : UInt64) : Int :=
  let payload := v &&& INT_MASK
  if payload &&& INT_SIGN ≠ 0 then
    (payload.toNat : Int) - (1 <<< 47 : Nat)
  else
    (payload.toNat : Int)

-- ══════════════════════════════════════════════════════════════════
-- Sorry #1: fused_xor_implies_isInt
--
-- Original location: NanBoxCorrect.lean line 676
-- Original sorry reason: requires bit-level reasoning that TAG_CHECK
--   masks a subset of the bits checked by the XOR-shift test.
-- ══════════════════════════════════════════════════════════════════

/-- Forward direction: fusedIsInt implies IsInt.
    If the XOR-shift check passes (bits 47..63 match QNAN|TAG_INT after XOR),
    then the TAG_CHECK mask also matches (since TAG_CHECK tests a subset of
    those bits).

    TACTIC: bv_decide
    This is a pure UInt64 bitwise implication. After unfolding the definitions
    to expose the UInt64 AND/OR/XOR/shift operations, bv_decide bitblasts
    the 64-bit constraint and solves via SAT. The CaDiCaL solver handles
    this in seconds even for 64-bit width. -/
theorem fused_xor_implies_isInt (bits : UInt64) :
    fusedIsInt bits = true → IsInt bits := by
  unfold fusedIsInt xorTagCheck IsInt TAG_CHECK EXPECTED_INT_TAG
  unfold QNAN TAG_INT TAG_MASK
  -- After unfolding, the goal is a pure UInt64 bitwise proposition.
  -- bv_decide's UInt64 preprocessor (Lean >= 4.17) converts to BitVec 64
  -- and bitblasts to SAT.
  --
  -- Expected: bv_decide closes this in one step.
  -- Fallback: if bv_decide times out, try:
  --   simp only [UInt64.toBitVec_and, UInt64.toBitVec_or, UInt64.toBitVec_xor,
  --              UInt64.toBitVec_shiftRight]
  --   bv_decide
  bv_decide

-- ══════════════════════════════════════════════════════════════════
-- Sorry #2: fused_xor_unbox
--
-- Original location: NanBoxCorrect.lean line 739
-- Original sorry reason: 47-bit sign-extension roundtrip via
--   BitVec.ofInt / toNat that Lean's automation cannot handle.
-- ══════════════════════════════════════════════════════════════════

/-- The fused XOR unbox produces the correct integer value.
    After XORing with (QNAN | TAG_INT), the upper 17 bits are zero (for valid ints),
    so the 47-bit payload is in the correct position for sign extension.

    Self-contained manual proof (Approach B):
      Step 1 (pure BV): XOR with QNAN|TAG_INT strips the tag, leaving raw &&& INT_MASK.
        Proven via bv_decide after unfolding to UInt64 bitwise ops.
      Step 2 (mixed BV/Int): signExtend47 of the masked payload recovers n.
        Case split on n >= 0 vs n < 0, using BitVec.toNat_ofInt, omega,
        and Nat.testBit for sign-bit reasoning. -/
theorem fused_xor_unbox (n : Int) (h : intFitsInline n) :
    let bits := fromInt n
    let xored := xorTagCheck bits
    signExtend47 xored = n := by
  simp only []
  -- ── Step 1: XOR strips the tag, leaving raw &&& INT_MASK ──
  have step1 : xorTagCheck (fromInt n) =
      UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK := by
    unfold xorTagCheck fromInt EXPECTED_INT_TAG
    simp only [QNAN, TAG_INT, INT_MASK]
    bv_decide
  rw [step1]
  -- ── Step 2: signExtend47 of masked payload recovers n ──
  -- Factor out the proof into a helper about the masked payload
  suffices hsuff : signExtend47 (UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK) = n from hsuff
  unfold signExtend47 intFitsInline at *
  obtain ⟨hlo, hhi⟩ := h
  -- INT_MASK is idempotent under AND
  have h_idem : (UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK) &&& INT_MASK =
      UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK := by
    apply UInt64.eq_of_toBitVec_eq
    simp only [UInt64.toBitVec_and, BitVec.and_assoc, BitVec.and_self]
  rw [h_idem]
  by_cases hn : n ≥ 0
  · -- ── Case n ≥ 0 ──
    -- raw.toNat = n.toNat (since n < 2^46 < 2^47 and INT_MASK = 2^47 - 1)
    have hraw_nat : (UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK).toNat = n.toNat := by
      rw [UInt64.toNat_and, UInt64.toNat_ofBitVec, BitVec.toNat_ofInt]
      have hm : INT_MASK.toNat = 0x00007fffffffffff := by native_decide
      rw [hm, show (0x00007fffffffffff : Nat) = 2^47 - 1 from by omega,
          Nat.and_two_pow_sub_one_eq_mod]
      omega
    -- Sign bit is clear: raw &&& INT_SIGN = 0
    have h_sign_clear : (UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK) &&& INT_SIGN = 0 := by
      have h_toNat_eq : ((UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK) &&& INT_SIGN).toNat =
          (0 : UInt64).toNat := by
        rw [UInt64.toNat_and, hraw_nat]
        have hsv : INT_SIGN.toNat = 2^46 := by native_decide
        have h0 : (0 : UInt64).toNat = 0 := by native_decide
        rw [hsv, h0]
        apply Nat.eq_of_testBit_eq; intro i
        rw [Nat.testBit_and, Nat.zero_testBit, Nat.testBit_two_pow]
        by_cases hi : 46 = i
        · subst hi; simp [Nat.testBit_lt_two_pow (by omega : n.toNat < 2^46)]
        · simp [hi]
      exact UInt64.eq_of_toBitVec_eq (BitVec.eq_of_toNat_eq h_toNat_eq)
    -- The if-branch goes to the else (sign bit clear)
    rw [if_neg (by rw [h_sign_clear]; simp)]
    rw [hraw_nat]; omega
  · -- ── Case n < 0 ──
    have hn_neg : n < 0 := by omega
    -- raw.toNat = (n % 2^64).toNat % 2^47
    have hraw_nat : (UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK).toNat =
        (n % (2^64 : Int)).toNat % 2^47 := by
      rw [UInt64.toNat_and, UInt64.toNat_ofBitVec, BitVec.toNat_ofInt]
      have hm : INT_MASK.toNat = 0x00007fffffffffff := by native_decide
      rw [hm, show (0x00007fffffffffff : Nat) = 2^47 - 1 from by omega]
      exact Nat.and_two_pow_sub_one_eq_mod _ 47
    -- For n in [-2^46, 0): (n % 2^64).toNat % 2^47 = (2^47 + n).toNat
    have hraw_val : (n % (2^64 : Int)).toNat % 2^47 = (2^47 + n).toNat := by omega
    -- Sign bit is set: raw &&& INT_SIGN ≠ 0
    have h_sign_set : (UInt64.ofBitVec (BitVec.ofInt 64 n) &&& INT_MASK) &&& INT_SIGN ≠ 0 := by
      intro h_eq
      have h_nat := congrArg UInt64.toNat h_eq
      rw [UInt64.toNat_and, hraw_nat, hraw_val] at h_nat
      have hsv : INT_SIGN.toNat = 2^46 := by native_decide
      have h0 : (0 : UInt64).toNat = 0 := by native_decide
      rw [hsv, h0] at h_nat
      -- (2^47 + n).toNat ≥ 2^46, so bit 46 is set, &&& 2^46 ≠ 0
      have hdiv : (2^47 + n).toNat / 2^46 = 1 := by omega
      have hbit : (2^47 + n).toNat.testBit 46 = true := by
        rw [Nat.testBit, Nat.shiftRight_eq_div_pow, hdiv]; rfl
      have hcontra : ((2^47 + n).toNat &&& 2^46).testBit 46 = true := by
        rw [Nat.testBit_and, hbit, Nat.testBit_two_pow]; simp
      rw [h_nat] at hcontra
      exact absurd hcontra (by simp [Nat.zero_testBit])
    -- The if-branch goes to the then (sign bit set)
    rw [if_pos h_sign_set, hraw_nat, hraw_val]
    omega

-- ══════════════════════════════════════════════════════════════════
-- Validation: concrete tests that already pass on Lean 4.16
-- (included here to confirm the definitions match NanBoxCorrect)
-- ══════════════════════════════════════════════════════════════════

-- These use native_decide and work on any Lean version.
-- They serve as a sanity check that our re-exported definitions are correct.

theorem fused_xor_check_42 : fusedIsInt (fromInt 42) = true := by native_decide
theorem fused_xor_check_neg1 : fusedIsInt (fromInt (-1)) = true := by native_decide
theorem fused_xor_check_0 : fusedIsInt (fromInt 0) = true := by native_decide

theorem fused_xor_unbox_42 :
    signExtend47 (xorTagCheck (fromInt 42)) = 42 := by native_decide
theorem fused_xor_unbox_neg1 :
    signExtend47 (xorTagCheck (fromInt (-1))) = -1 := by native_decide
theorem fused_xor_unbox_0 :
    signExtend47 (xorTagCheck (fromInt 0)) = 0 := by native_decide

-- Boundary cases
theorem fused_xor_unbox_max :
    signExtend47 (xorTagCheck (fromInt 70368744177663)) = 70368744177663 := by native_decide
theorem fused_xor_unbox_min :
    signExtend47 (xorTagCheck (fromInt (-70368744177664))) = -70368744177664 := by native_decide

-- ══════════════════════════════════════════════════════════════════
-- Bonus: examples of what bv_decide can prove (on Lean >= 4.17)
-- ══════════════════════════════════════════════════════════════════

-- These demonstrate bv_decide's power on UInt64 goals.

-- Example 1: TAG_CHECK masks are disjoint from payload masks
theorem int_mask_and_tag_check_bv : INT_MASK &&& TAG_CHECK = 0 := by
  unfold INT_MASK TAG_CHECK QNAN TAG_MASK
  bv_decide

-- Example 2: XOR self-inverse property for NaN-box tags
theorem xor_self_inverse (v : UInt64) : (v ^^^ EXPECTED_INT_TAG) ^^^ EXPECTED_INT_TAG = v := by
  unfold EXPECTED_INT_TAG QNAN TAG_INT
  bv_decide

-- Example 3: The tag region is preserved through mask-then-OR
theorem tag_preserved (payload : UInt64) :
    (QNAN ||| TAG_INT ||| (payload &&& INT_MASK)) &&& TAG_CHECK = QNAN ||| TAG_INT := by
  simp only [QNAN, TAG_INT, TAG_CHECK, TAG_MASK, INT_MASK]
  bv_decide

end MoltTIR.Runtime.NanBoxBV
