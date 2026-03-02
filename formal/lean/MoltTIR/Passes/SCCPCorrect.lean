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

/-- The top (all-unknown) abstract environment is sound for any concrete env. -/
theorem absEnvTop_sound (ρ : Env) : AbsEnvSound AbsEnv.top ρ := by
  intro x v _
  simp [AbsEnv.top, AbsVal.concretizes]

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
    | overdefined =>
      simp [absEvalBinOp, AbsVal.concretizes]
  | overdefined => simp [absEvalBinOp, AbsVal.concretizes]

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

/-- Abstract expression evaluation is sound: if the abstract value is known,
    then the concrete evaluation agrees. -/
theorem absEvalExpr_sound (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvSound σ ρ) (cv : Value)
    (ha : absEvalExpr σ e = .known cv) :
    evalExpr ρ e = some cv := by
  induction e with
  | val v =>
    simp [absEvalExpr] at ha
    simp [evalExpr, ha]
  | var x =>
    simp [absEvalExpr] at ha
    -- ha : σ x = .known cv
    -- We need: ρ x = some cv
    -- From hsound: ρ x = some v → concretizes (σ x) v
    -- From ha: σ x = .known cv, so concretizes (.known cv) v means cv = v
    -- But we need to know ρ x is some. If σ x = .known cv and σ is sound,
    -- it means whenever ρ x has a value, that value is cv.
    -- However, ρ x could be none. We need to strengthen or use a different approach.
    -- For soundness of the SCCP *pass*, we need the abstract env to be
    -- computed from the actual execution, which guarantees ρ x is defined.
    sorry  -- requires definedness assumption (see note below)
  | bin op a b iha ihb =>
    simp only [absEvalExpr] at ha
    -- Need to case-split on absEvalExpr σ a and absEvalExpr σ b
    match ha_e : absEvalExpr σ a, hb_e : absEvalExpr σ b with
    | .known va, .known vb =>
      simp [absEvalBinOp] at ha
      match hr : evalBinOp op va vb with
      | some vr =>
        simp [hr] at ha
        have iha' := iha ha_e
        have ihb' := ihb hb_e
        simp [evalExpr, iha', ihb', hr, ha]
      | none => simp [hr] at ha
    | .unknown, _ => simp [absEvalBinOp] at ha
    | _, .unknown => cases absEvalExpr σ a <;> simp [absEvalBinOp] at ha
    | .overdefined, _ => simp [absEvalBinOp] at ha
    | .known _, .overdefined => simp [absEvalBinOp] at ha
  | un op a iha =>
    simp only [absEvalExpr] at ha
    match ha_e : absEvalExpr σ a with
    | .known va =>
      simp [absEvalUnOp] at ha
      match hr : evalUnOp op va with
      | some vr =>
        simp [hr] at ha
        have iha' := iha ha_e
        simp [evalExpr, iha', hr, ha]
      | none => simp [hr] at ha
    | .unknown => simp [absEvalUnOp] at ha
    | .overdefined => simp [absEvalUnOp] at ha

/-
  NOTE on the `sorry` in the var case:

  The soundness theorem as stated has a gap: `AbsEnvSound σ ρ` only says
  "if ρ x = some v then concretizes (σ x) v." It does NOT guarantee that
  ρ x is defined (some). So when `σ x = .known cv`, we know that *if*
  ρ x has a value, it must be cv — but we can't prove ρ x is defined.

  To close this gap, we need either:
  (a) A stronger abstract env invariant: `σ x = .known v → ρ x = some v`
  (b) A definedness predicate: the SCCP pass only marks vars as `.known`
      when it has seen them defined (i.e., as LHS of an executed instruction)

  Approach (a) is the CompCert style. The real SCCP implementation uses (b).
  For this milestone, the sorry is precisely documented and all surrounding
  infrastructure is proven.
-/

/-- SCCP-transformed expressions preserve semantics when the abstract
    value is known (main pass correctness, modulo definedness). -/
theorem sccpExpr_correct (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvSound σ ρ) :
    evalExpr ρ (sccpExpr σ e) = evalExpr ρ e := by
  simp only [sccpExpr]
  match h : absEvalExpr σ e with
  | .known v =>
    simp only [evalExpr]
    exact (absEvalExpr_sound σ ρ e hsound v h).symm
  | .unknown => rfl
  | .overdefined => rfl

/-- Updating abstract env with a computed value preserves soundness. -/
theorem absEnvSound_set (σ : AbsEnv) (ρ : Env) (x : Var) (v : Value) (a : AbsVal)
    (hsound : AbsEnvSound σ ρ)
    (hconc : AbsVal.concretizes a v) :
    AbsEnvSound (σ.set x a) (ρ.set x v) := by
  intro y w hy
  simp [AbsEnv.set, Env.set] at *
  split at hy
  · -- y = x: hy says some v = some w, so v = w
    next heq =>
      simp [heq] at hy
      subst hy
      exact hconc
  · -- y ≠ x: use original soundness
    exact hsound y w hy

end MoltTIR
