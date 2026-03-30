/-
  Fuel monotonicity for evalPyExpr — mutual induction with evalExprList,
  evalCompareChain, and evalBoolOp.
-/
import MoltPython.Semantics.EvalExpr

set_option maxHeartbeats 6400000
set_option autoImplicit false

namespace MoltPython

/-- Mutual fuel monotonicity: all four evaluation functions are monotone in fuel. -/
private theorem fuel_mono_quad (f : Nat) :
    (∀ (env : PyEnv) (e : PyExpr) (v : PyValue),
      evalPyExpr f env e = some v → ∀ f', f ≤ f' → evalPyExpr f' env e = some v) ∧
    (∀ (env : PyEnv) (es : List PyExpr) (vs : List PyValue),
      evalExprList f env es = some vs → ∀ f', f ≤ f' → evalExprList f' env es = some vs) ∧
    (∀ (env : PyEnv) (left : PyExpr) (ops : List CompareOp) (comps : List PyExpr) (v : PyValue),
      evalCompareChain f env left ops comps = some v → ∀ f', f ≤ f' →
      evalCompareChain f' env left ops comps = some v) ∧
    (∀ (env : PyEnv) (op : BoolOp) (vals : List PyExpr) (v : PyValue),
      evalBoolOp f env op vals = some v → ∀ f', f ≤ f' →
      evalBoolOp f' env op vals = some v) := by
  induction f with
  | zero =>
    refine ⟨?_, ?_, ?_, ?_⟩
    · intro env e v h; simp [evalPyExpr] at h
    · intro env es vs h f' hle
      cases es with
      | nil => cases f' <;> simp_all [evalExprList]
      | cons => simp [evalExprList] at h
    · intro env left ops comps v h f' hle
      cases ops with
      | nil => cases f' <;> simp_all [evalCompareChain]
      | cons op ops' =>
        cases comps with
        | nil => cases f' <;> simp_all [evalCompareChain]
        | cons => simp [evalCompareChain] at h
    · intro env op vals v h f' hle
      cases vals with
      | nil => cases op <;> cases f' <;> simp_all [evalBoolOp]
      | cons => simp [evalBoolOp] at h
  | succ m ih =>
    obtain ⟨ih_e, ih_l, ih_c, ih_b⟩ := ih
    refine ⟨?_, ?_, ?_, ?_⟩
    · -- evalPyExpr
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
          rw [ih_e env left va hl n hmn, ih_e env right vb hr n hmn]
          simp [hl, hr] at heval; exact heval
        | some _, none => simp [hl, hr] at heval
        | none, _ => simp [hl] at heval
      | unaryOp op operand =>
        simp only [evalPyExpr] at heval ⊢
        match ho : evalPyExpr m env operand with
        | some va =>
          rw [ih_e env operand va ho n hmn]; simp [ho] at heval; exact heval
        | none => simp [ho] at heval
      | ifExpr test body orElse =>
        simp only [evalPyExpr] at heval ⊢
        match ht : evalPyExpr m env test with
        | some vt =>
          rw [ih_e env test vt ht n hmn]; simp [ht] at heval ⊢
          split <;> simp_all
          all_goals exact ih_e env _ v (by assumption) n hmn
        | none => simp [ht] at heval
      | call _ _ => simp [evalPyExpr] at heval
      | subscript value slice =>
        simp only [evalPyExpr] at heval ⊢
        match hv : evalPyExpr m env value, hs : evalPyExpr m env slice with
        | some vv, some sv =>
          rw [ih_e env value vv hv n hmn, ih_e env slice sv hs n hmn]
          simp [hv, hs] at heval; exact heval
        | some _, none => simp [hv, hs] at heval
        | none, _ => simp [hv] at heval
      | listExpr elts =>
        simp only [evalPyExpr] at heval ⊢
        match he : evalExprList m env elts with
        | some vs =>
          rw [ih_l env elts vs he n hmn]; simp [he] at heval; subst heval; rfl
        | none => simp [he] at heval
      | tupleExpr elts =>
        simp only [evalPyExpr] at heval ⊢
        match he : evalExprList m env elts with
        | some vs =>
          rw [ih_l env elts vs he n hmn]; simp [he] at heval; subst heval; rfl
        | none => simp [he] at heval
      | dictExpr keys values =>
        simp only [evalPyExpr] at heval ⊢
        match hk : evalExprList m env keys, hv : evalExprList m env values with
        | some ks, some vs =>
          rw [ih_l env keys ks hk n hmn, ih_l env values vs hv n hmn]
          simp [hk, hv] at heval; subst heval; rfl
        | some _, none => simp [hk, hv] at heval
        | none, _ => simp [hk] at heval
      | compare left ops comps =>
        simp only [evalPyExpr] at heval ⊢
        exact ih_c env left ops comps v heval n hmn
      | boolOp op vals =>
        simp only [evalPyExpr] at heval ⊢
        exact ih_b env op vals v heval n hmn
    · -- evalExprList
      intro env es vs heval f' hle
      obtain ⟨n, rfl⟩ : ∃ n, f' = n + 1 := ⟨f' - 1, by omega⟩
      have hmn : m ≤ n := by omega
      cases es with
      | nil => cases m <;> cases n <;> simp_all [evalExprList]
      | cons e rest =>
        simp only [evalExprList] at heval ⊢
        match he : evalPyExpr m env e, hr : evalExprList m env rest with
        | some v, some vrest =>
          rw [ih_e env e v he n hmn, ih_l env rest vrest hr n hmn]
          simp [he, hr] at heval; subst heval; rfl
        | some _, none => simp [he, hr] at heval
        | none, _ => simp [he] at heval
    · -- evalCompareChain
      intro env left ops comps v heval f' hle
      obtain ⟨n, rfl⟩ : ∃ n, f' = n + 1 := ⟨f' - 1, by omega⟩
      have hmn : m ≤ n := by omega
      cases ops with
      | nil => cases comps <;> simp_all [evalCompareChain]
      | cons op ops' =>
        cases comps with
        | nil => simp_all [evalCompareChain]
        | cons comp comps' =>
          simp only [evalCompareChain] at heval ⊢
          match hl : evalPyExpr m env left, hc : evalPyExpr m env comp with
          | some lv, some rv =>
            rw [ih_e env left lv hl n hmn, ih_e env comp rv hc n hmn]
            simp [hl, hc] at heval
            match hcmp : evalCompareOp op lv rv with
            | some result =>
              simp [hcmp] at heval ⊢
              split at heval
              · split; exact heval; contradiction
              · split; contradiction; exact ih_c env comp ops' comps' v heval n hmn
            | none => simp [hcmp] at heval
          | some _, none => simp [hl, hc] at heval
          | none, _ => simp [hl] at heval
    · -- evalBoolOp
      intro env op vals v heval f' hle
      obtain ⟨n, rfl⟩ : ∃ n, f' = n + 1 := ⟨f' - 1, by omega⟩
      have hmn : m ≤ n := by omega
      cases vals with
      | nil => cases op <;> simp_all [evalBoolOp]
      | cons e rest =>
        match hrest : rest with
        | [] =>
          subst hrest
          simp only [evalBoolOp] at heval ⊢
          exact ih_e env e v heval n hmn
        | _ :: _ =>
          cases op <;> simp only [evalBoolOp] at heval ⊢
          all_goals (
            match he : evalPyExpr m env e with
            | some ve =>
              rw [ih_e env e ve he n hmn]
              simp [he] at heval ⊢
              split at heval
              · split; exact heval; contradiction
              · split; contradiction; exact ih_b env _ _ v heval n hmn
            | none => simp [he] at heval)

/-- evalPyExpr is monotone in fuel. -/
theorem evalPyExpr_fuel_mono (f : Nat) (env : PyEnv) (e : PyExpr) (v : PyValue)
    (heval : evalPyExpr f env e = some v) (f' : Nat) (hle : f ≤ f') :
    evalPyExpr f' env e = some v :=
  (fuel_mono_quad f).1 env e v heval f' hle

end MoltPython
