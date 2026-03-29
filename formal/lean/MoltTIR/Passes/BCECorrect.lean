/-
  MoltTIR.Passes.BCECorrect — correctness proof for bounds check elimination.

  Key theorem: if an index is non-negative and strictly less than the
  container length, the runtime bounds check is redundant — the access
  is guaranteed to be in-bounds.

  Phase 2 only marks constant non-negative indices; the `idx < len`
  part is a *codegen-time* contract (the container must actually have
  sufficient length at runtime).  We formalize the core safety lemma
  and the annotation correctness.
-/
import MoltTIR.Passes.BCE

namespace MoltTIR

-- ---------------------------------------------------------------------------
-- Core safety predicate
-- ---------------------------------------------------------------------------

/-- A bounds check is redundant when the index is within [0, len). -/
def boundsCheckRedundant (idx : Int) (len : Nat) : Prop :=
  0 ≤ idx ∧ idx < ↑len

/-- Main safety theorem: a non-negative index strictly below the container
    length makes the bounds check redundant. -/
theorem bce_safe (idx : Int) (len : Nat)
    (h_nonneg : 0 ≤ idx) (h_bound : idx < ↑len) :
    boundsCheckRedundant idx len :=
  ⟨h_nonneg, h_bound⟩

-- ---------------------------------------------------------------------------
-- Annotation correctness
-- ---------------------------------------------------------------------------

/-- `isConstNonNegIndex` returns true only for `index` ops with a
    non-negative constant index expression. -/
theorem isConstNonNegIndex_spec (op : SideEffectOp) :
    isConstNonNegIndex op = true →
    ∃ obj n dst, op = .index obj (.val (.int n)) dst ∧ 0 ≤ n := by
  intro h
  match op with
  | .index obj (.val (.int n)) dst =>
    simp [isConstNonNegIndex] at h
    exact ⟨obj, n, dst, rfl, h⟩
  | .index _ (.val (.bool _)) _ => simp [isConstNonNegIndex] at h
  | .index _ (.val (.float _)) _ => simp [isConstNonNegIndex] at h
  | .index _ (.val (.str _)) _ => simp [isConstNonNegIndex] at h
  | .index _ (.val .none) _ => simp [isConstNonNegIndex] at h
  | .index _ (.var _) _ => simp [isConstNonNegIndex] at h
  | .index _ (.bin _ _ _) _ => simp [isConstNonNegIndex] at h
  | .index _ (.un _ _) _ => simp [isConstNonNegIndex] at h
  | .call _ _ _ => simp [isConstNonNegIndex] at h
  | .callMethod _ _ _ _ => simp [isConstNonNegIndex] at h
  | .loadAttr _ _ _ => simp [isConstNonNegIndex] at h
  | .storeAttr _ _ _ => simp [isConstNonNegIndex] at h
  | .storeIndex _ _ _ => simp [isConstNonNegIndex] at h
  | .buildList _ _ => simp [isConstNonNegIndex] at h
  | .buildDict _ _ _ => simp [isConstNonNegIndex] at h
  | .buildTuple _ _ => simp [isConstNonNegIndex] at h
  | .getIter _ _ => simp [isConstNonNegIndex] at h
  | .iterNext _ _ => simp [isConstNonNegIndex] at h
  | .raise _ => simp [isConstNonNegIndex] at h
  | .incRef _ => simp [isConstNonNegIndex] at h
  | .decRef _ => simp [isConstNonNegIndex] at h
  | .boxVal _ _ => simp [isConstNonNegIndex] at h
  | .unboxVal _ _ => simp [isConstNonNegIndex] at h
  | .import_ _ _ => simp [isConstNonNegIndex] at h

/-- `bceOp` marks an op as safe iff `isConstNonNegIndex` holds. -/
theorem bceOp_safe_iff (op : SideEffectOp) :
    (bceOp op).bce_safe = isConstNonNegIndex op := by
  simp [bceOp]

/-- Negative constant indices are never marked safe. -/
theorem bce_negative_not_marked (obj : Expr) (n : Int) (dst : Var)
    (h_neg : n < 0) :
    (bceOp (.index obj (.val (.int n)) dst)).bce_safe = false := by
  simp [bceOp, isConstNonNegIndex]
  omega

/-- Non-index operations are never marked safe. -/
theorem bce_non_index_not_marked (callee : String) (args : List Expr) (dst : Var) :
    (bceOp (.call callee args dst)).bce_safe = false := by
  simp [bceOp, isConstNonNegIndex]

-- ---------------------------------------------------------------------------
-- Function-level lifting (sorry for now — awaits extended block model)
-- ---------------------------------------------------------------------------

/-- BCE preserves the semantics of all operations in the function.
    The annotated ops carry the same `op` field; only the `bce_safe`
    annotation is added.  Full semantic preservation at function level
    requires the extended evaluation model for `SideEffectOp`. -/
theorem bceOps_preserve_ops (ops : List SideEffectOp) :
    (bceOps ops).map (·.op) = ops := by
  simp [bceOps, bceOp, List.map_map, Function.comp]

end MoltTIR
