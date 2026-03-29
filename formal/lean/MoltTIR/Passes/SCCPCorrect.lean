/-
  MoltTIR.Passes.SCCPCorrect — soundness proof for SCCP.

  Main theorem: if the abstract environment soundly approximates the
  concrete environment (every variable's abstract value concretizes
  the concrete value), then SCCP-transformed expressions evaluate
  to the same result.

  The proof strategy follows standard abstract interpretation soundness:
  show that abstract evaluation is sound w.r.t. concrete evaluation,
  then show that replacing expressions with their abstract constants
  preserves semantics.
-/
import MoltTIR.Passes.SCCP

namespace MoltTIR

/-- An abstract environment soundly approximates a concrete environment. -/
def AbsEnvSound (σ : AbsEnv) (ρ : Env) : Prop :=
  ∀ x v, ρ x = some v → AbsVal.concretizes (σ x) v

/-- Strong abstract environment soundness (CompCert style).
    Adds the converse: if σ x = known v, then ρ x is defined with value v. -/
def AbsEnvStrongSound (σ : AbsEnv) (ρ : Env) : Prop :=
  (∀ x v, ρ x = some v → AbsVal.concretizes (σ x) v) ∧
  (∀ x v, σ x = .known v → ρ x = some v)

/-- Strong soundness implies weak soundness. -/
theorem absEnvStrongSound_implies_sound (σ : AbsEnv) (ρ : Env)
    (h : AbsEnvStrongSound σ ρ) : AbsEnvSound σ ρ := h.1

/-- The top (all-unknown) abstract environment is sound for any concrete env. -/
theorem absEnvTop_sound (ρ : Env) : AbsEnvSound AbsEnv.top ρ := by
  intro x v _
  simp [AbsEnv.top, AbsVal.concretizes]

/-- The top (all-unknown) abstract environment is strongly sound. -/
theorem absEnvTop_strongSound (ρ : Env) : AbsEnvStrongSound AbsEnv.top ρ := by
  constructor
  · intro x v _; simp [AbsEnv.top, AbsVal.concretizes]
  · intro x v h; simp [AbsEnv.top] at h

/-- Abstract binary op evaluation is sound. -/
theorem absEvalBinOp_sound (op : BinOp) (a b : AbsVal) (va vb : Value)
    (ha : AbsVal.concretizes a va) (hb : AbsVal.concretizes b vb) :
    ∀ vr, evalBinOp op va vb = some vr →
    AbsVal.concretizes (absEvalBinOp op a b) vr := by
  intro vr hr
  cases a with
  | unknown => simp [absEvalBinOp, AbsVal.concretizes]
  | known va' =>
    cases b with
    | unknown => simp [absEvalBinOp, AbsVal.concretizes]
    | known vb' =>
      simp [AbsVal.concretizes] at ha hb
      subst ha; subst hb
      simp [absEvalBinOp, hr, AbsVal.concretizes]
    | overdefined => simp [absEvalBinOp, AbsVal.concretizes]
  | overdefined =>
    cases b <;> simp [absEvalBinOp, AbsVal.concretizes]

/-- Abstract unary op evaluation is sound. -/
theorem absEvalUnOp_sound (op : UnOp) (a : AbsVal) (va : Value)
    (ha : AbsVal.concretizes a va) :
    ∀ vr, evalUnOp op va = some vr →
    AbsVal.concretizes (absEvalUnOp op a) vr := by
  intro vr hr
  cases a with
  | unknown => simp [absEvalUnOp, AbsVal.concretizes]
  | known va' =>
    simp [AbsVal.concretizes] at ha
    subst ha
    simp [absEvalUnOp, hr, AbsVal.concretizes]
  | overdefined => simp [absEvalUnOp, AbsVal.concretizes]

/-
  NOTE on absEvalExpr_sound:

  The var case requires knowing that `ρ x` is defined when `σ x = .known cv`.
  `AbsEnvSound` alone only provides the forward direction (ρ x = some v →
  concretizes (σ x) v), which is insufficient. We use `AbsEnvStrongSound`
  which adds the converse: `σ x = .known v → ρ x = some v`.

  This is the standard CompCert-style approach (a). Downstream callers
  using `AbsEnvSound` should migrate to `AbsEnvStrongSound` — the lemma
  `absEnvStrongSound_implies_sound` bridges the gap where needed.
-/

/-- Abstract expression evaluation is sound: if the abstract value is known,
    then the concrete evaluation agrees.
    Uses `AbsEnvStrongSound` to establish variable definedness in the var case. -/
theorem absEvalExpr_sound (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvStrongSound σ ρ) (cv : Value)
    (ha : absEvalExpr σ e = .known cv) :
    evalExpr ρ e = some cv := by
  induction e generalizing cv with
  | val v =>
    simp [absEvalExpr] at ha
    simp [evalExpr, ha]
  | var x =>
    simp [absEvalExpr] at ha
    exact hsound.2 x cv ha
  | bin op a b iha ihb =>
    simp only [absEvalExpr] at ha
    -- Case split on the abstract results of a and b
    cases ha_abs : absEvalExpr σ a <;> cases hb_abs : absEvalExpr σ b <;>
      simp [ha_abs, hb_abs, absEvalBinOp] at ha
    -- The only non-trivial case: both are .known
    · rename_i va vb
      -- ha : (match evalBinOp op va vb with ...) = .known cv
      split at ha
      · -- evalBinOp op va vb = some v
        rename_i v heval
        cases ha
        -- Now v = cv (from AbsVal.known injection)
        have ha_eq := iha va ha_abs
        have hb_eq := ihb vb hb_abs
        simp [evalExpr, ha_eq, hb_eq, heval]
      · -- evalBinOp op va vb = none → result is .overdefined, not .known cv
        exact absurd ha (by simp [AbsVal.noConfusion])
  | un op a iha =>
    simp only [absEvalExpr] at ha
    cases ha_abs : absEvalExpr σ a <;> simp [ha_abs, absEvalUnOp] at ha
    · rename_i va
      split at ha
      · rename_i v heval
        cases ha
        have ha_eq := iha va ha_abs
        simp [evalExpr, ha_eq, heval]
      · exact absurd ha (by simp [AbsVal.noConfusion])

/-- Updating abstract env with a computed value preserves soundness. -/
theorem absEnvSound_set (σ : AbsEnv) (ρ : Env) (x : Var) (v : Value) (a : AbsVal)
    (hsound : AbsEnvSound σ ρ)
    (hconc : AbsVal.concretizes a v) :
    AbsEnvSound (σ.set x a) (ρ.set x v) := by
  intro y w hy
  unfold AbsEnv.set Env.set at *
  split at hy <;> rename_i heq
  · -- y = x: hy says some v = some w, so v = w
    simp at hy; subst hy
    simp [heq]; exact hconc
  · -- y ≠ x: use original soundness
    simp [heq]; exact hsound y w hy

/-- Updating abstract env preserves strong soundness. -/
theorem absEnvStrongSound_set (σ : AbsEnv) (ρ : Env) (x : Var) (v : Value) (a : AbsVal)
    (hsound : AbsEnvStrongSound σ ρ)
    (hconc : AbsVal.concretizes a v)
    (hdef : a = .known v ∨ a ≠ .known v → ∀ w, a = .known w → w = v) :
    AbsEnvStrongSound (σ.set x a) (ρ.set x v) := by
  constructor
  · exact absEnvSound_set σ ρ x v a hsound.1 hconc
  · intro y w hy
    unfold AbsEnv.set at hy
    unfold Env.set
    by_cases heq : y = x
    · -- y = x: hy says a = .known w
      simp [heq] at hy ⊢
      have hw_eq : w = v := by
        by_cases hvw : a = .known v
        · exact hdef (Or.inl hvw) w hy
        · exact hdef (Or.inr hvw) w hy
      rw [hw_eq]
    · -- y ≠ x: use original strong soundness
      simp [heq] at hy ⊢
      exact hsound.2 y w hy

/-- `absEvalExpr_strong_sound` is now an alias for `absEvalExpr_sound`
    (both use `AbsEnvStrongSound`). Retained for backward compatibility. -/
theorem absEvalExpr_strong_sound (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvStrongSound σ ρ) (cv : Value)
    (ha : absEvalExpr σ e = .known cv) :
    evalExpr ρ e = some cv :=
  absEvalExpr_sound σ ρ e hsound cv ha

/-- SCCP-transformed expressions preserve semantics when the abstract
    value is known (main pass correctness).
    Uses strong soundness for the var-case definedness proof. -/
theorem sccpExpr_correct (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvStrongSound σ ρ) :
    evalExpr ρ (sccpExpr σ e) = evalExpr ρ e := by
  simp only [sccpExpr]
  match h : absEvalExpr σ e with
  | .known v =>
    simp only [evalExpr]
    exact (absEvalExpr_sound σ ρ e hsound v h).symm
  | .unknown => rfl
  | .overdefined => rfl

/-- `sccpExpr_correct_strong` is now an alias for `sccpExpr_correct`
    (both use `AbsEnvStrongSound`). Retained for backward compatibility. -/
theorem sccpExpr_correct_strong (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvStrongSound σ ρ) :
    evalExpr ρ (sccpExpr σ e) = evalExpr ρ e :=
  sccpExpr_correct σ ρ e hsound

end MoltTIR
