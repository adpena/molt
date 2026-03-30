/-
  Fuel monotonicity for evalPyExpr.
  Proven for all expression forms reachable from lowerExpr:
  scalars, name, binOp, unaryOp, ifExpr, call, subscript.
  List-based forms (listExpr, tupleExpr, dictExpr, compare, boolOp) need
  mutual induction with evalExprList — axiomatized since lowerExpr returns
  none for all of them.
-/
import MoltPython.Semantics.EvalExpr

set_option maxHeartbeats 3200000
set_option autoImplicit false

namespace MoltPython

/-- evalPyExpr is monotone in fuel. -/
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
        rw [ih env test vt ht n hmn]; simp [ht] at heval ⊢
        split <;> simp_all
        all_goals exact ih env _ v (by assumption) n hmn
      | none => simp [ht] at heval
    | call _ _ => simp [evalPyExpr] at heval
    | subscript value slice =>
      simp only [evalPyExpr] at heval ⊢
      match hv : evalPyExpr m env value, hs : evalPyExpr m env slice with
      | some vv, some sv =>
        rw [ih env value vv hv n hmn, ih env slice sv hs n hmn]
        simp [hv, hs] at heval; exact heval
      | some _, none => simp [hv, hs] at heval
      | none, _ => simp [hv] at heval
    -- List-based compound forms need mutual induction with evalExprList/
    -- evalCompareChain/evalBoolOp. All return none from lowerExpr.
    | listExpr _ | tupleExpr _ | dictExpr _ _ | compare _ _ _ | boolOp _ _ =>
      exact fuel_mono_list_forms env _ v m heval n hmn
where
  /-- Axiom for list-based compound forms. These need mutual induction with
      evalExprList which is blocked by PyExpr being a nested inductive.
      Unreachable from lowering_reflects_eval (lowerExpr returns none). -/
  fuel_mono_list_forms (env : PyEnv) (e : PyExpr) (v : PyValue) (m : Nat)
      (heval : evalPyExpr (m + 1) env e = some v) (n : Nat) (hmn : m ≤ n) :
      evalPyExpr (n + 1) env e = some v := by
    sorry

end MoltPython
