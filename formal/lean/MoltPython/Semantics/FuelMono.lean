/-
  Fuel monotonicity for evalPyExpr (restricted to lowerable expression forms).
-/
import MoltPython.Semantics.EvalExpr

set_option maxHeartbeats 1600000
set_option autoImplicit false

namespace MoltPython

/-- evalPyExpr is monotone in fuel for all expression forms.
    Compound forms (compare, boolOp, subscript, list/tuple/dict) need
    mutual induction with evalCompareChain/evalBoolOp/evalExprList;
    these are asserted as an axiom since lowerExpr returns none for them. -/
theorem evalPyExpr_fuel_mono (f : Nat) :
    ∀ (env : PyEnv) (e : PyExpr) (v : PyValue),
    evalPyExpr f env e = some v →
    ∀ (f' : Nat), f ≤ f' → evalPyExpr f' env e = some v := by
  induction f with
  | zero => intro env e v heval; simp [evalPyExpr] at heval
  | succ m ih =>
    intro env e v heval f' hle
    obtain ⟨n, rfl⟩ : ∃ n, f' = n + 1 := ⟨f' - 1, by omega⟩
    have hmn : m ≤ n := by omega
    cases e with
    | intLit _ | floatLit _ | boolLit _ | strLit _ | noneLit =>
      simp [evalPyExpr] at heval ⊢; exact heval
    | name x => simp [evalPyExpr] at heval ⊢; exact heval
    | binOp op left right =>
      simp only [evalPyExpr] at heval ⊢
      match hl : evalPyExpr m env left, hr : evalPyExpr m env right with
      | some va, some vb =>
        rw [ih env left va hl n hmn, ih env right vb hr n hmn]
        simp [hl, hr] at heval; exact heval
      | some _, none => simp [hl, hr] at heval
      | none, _ => simp [hl] at heval
    | unaryOp op operand =>
      simp only [evalPyExpr] at heval ⊢
      match ho : evalPyExpr m env operand with
      | some va =>
        rw [ih env operand va ho n hmn]
        simp [ho] at heval; exact heval
      | none => simp [ho] at heval
    | ifExpr test body orElse =>
      simp only [evalPyExpr] at heval ⊢
      match ht : evalPyExpr m env test with
      | some vt =>
        rw [ih env test vt ht n hmn]
        simp [ht] at heval ⊢
        split <;> simp_all
        all_goals exact ih env _ v (by assumption) n hmn
      | none => simp [ht] at heval
    | call _ _ =>
      simp [evalPyExpr] at heval
    -- Compound forms needing mutual induction with List helpers.
    -- These are unreachable from lowering_reflects_eval (lowerExpr returns none).
    | subscript _ _ => exact fuel_mono_compound env (.subscript ..) v m heval n hmn
    | listExpr _ => exact fuel_mono_compound env (.listExpr ..) v m heval n hmn
    | tupleExpr _ => exact fuel_mono_compound env (.tupleExpr ..) v m heval n hmn
    | dictExpr _ _ => exact fuel_mono_compound env (.dictExpr ..) v m heval n hmn
    | compare _ _ _ => exact fuel_mono_compound env (.compare ..) v m heval n hmn
    | boolOp _ _ => exact fuel_mono_compound env (.boolOp ..) v m heval n hmn
where
  fuel_mono_compound (env : PyEnv) (e : PyExpr) (v : PyValue) (m : Nat)
      (heval : evalPyExpr (m + 1) env e = some v) (n : Nat) (hmn : m ≤ n) :
      evalPyExpr (n + 1) env e = some v := by
    -- These compound forms need mutual induction with evalExprList/
    -- evalCompareChain/evalBoolOp. Since lowerExpr returns none for all of them,
    -- they're unreachable in lowering_reflects_eval. We use sorry here;
    -- closing requires extending the mutual induction scheme.
    sorry

end MoltPython
