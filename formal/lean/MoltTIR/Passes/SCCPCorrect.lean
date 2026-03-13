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

/-- Abstract expression evaluation is sound: if the abstract value is known,
    then the concrete evaluation agrees. -/
theorem absEvalExpr_sound (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvSound σ ρ) (cv : Value)
    (ha : absEvalExpr σ e = .known cv) :
    evalExpr ρ e = some cv := by
  induction e generalizing cv with
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

  Below we provide `AbsEnvStrongSound` (approach (a)) and a sorry-free
  `absEvalExpr_strong_sound` for callers that can establish the stronger invariant.
-/

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

/-- Strong abstract environment soundness (CompCert style).
    Adds the converse: if σ x = known v, then ρ x is defined with value v. -/
def AbsEnvStrongSound (σ : AbsEnv) (ρ : Env) : Prop :=
  (∀ x v, ρ x = some v → AbsVal.concretizes (σ x) v) ∧
  (∀ x v, σ x = .known v → ρ x = some v)

/-- Strong soundness implies weak soundness. -/
theorem absEnvStrongSound_implies_sound (σ : AbsEnv) (ρ : Env)
    (h : AbsEnvStrongSound σ ρ) : AbsEnvSound σ ρ := h.1

/-- The top (all-unknown) abstract environment is strongly sound. -/
theorem absEnvTop_strongSound (ρ : Env) : AbsEnvStrongSound AbsEnv.top ρ := by
  constructor
  · intro x v _; simp [AbsEnv.top, AbsVal.concretizes]
  · intro x v h; simp [AbsEnv.top] at h

/-- Updating abstract env preserves strong soundness.
    TODO(formal, owner:compiler, milestone:M5, priority:P1, status:partial):
    The second constructor requires case analysis on a = .known w that
    needs Mathlib's tauto tactic or manual decidability reasoning. -/
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
      -- From hdef: a = .known w → w = v, so w = v
      -- hdef : a = .known v ∨ a ≠ .known v → ∀ w, a = .known w → w = v
      -- We can provide either disjunct.
      -- We know a = .known w from hy. Need a = .known v or a ≠ .known v.
      -- By cases on whether v = w:
      have hw_eq : w = v := by
        by_cases hvw : a = .known v
        · exact hdef (Or.inl hvw) w hy
        · exact hdef (Or.inr hvw) w hy
      rw [hw_eq]
    · -- y ≠ x: use original strong soundness
      simp [heq] at hy ⊢
      exact hsound.2 y w hy

/-- Abstract expression evaluation is sound under strong soundness.
    This version has NO sorry for the var case — the var case uses the
    strong invariant's converse direction to establish definedness.
    TODO(formal, owner:compiler, milestone:M5, priority:P1, status:partial):
    bin/un cases need reworking. -/
theorem absEvalExpr_strong_sound (σ : AbsEnv) (ρ : Env) (e : Expr)
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
    cases ha_abs : absEvalExpr σ a <;> cases hb_abs : absEvalExpr σ b <;>
      simp [ha_abs, hb_abs, absEvalBinOp] at ha
    · rename_i va vb
      split at ha
      · rename_i v heval
        cases ha
        have ha_eq := iha va ha_abs
        have hb_eq := ihb vb hb_abs
        simp [evalExpr, ha_eq, hb_eq, heval]
      · exact absurd ha (by simp [AbsVal.noConfusion])
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

/-- SCCP-transformed expressions preserve semantics when the abstract
    value is known (main pass correctness, modulo definedness).
    Uses weak soundness + absEvalExpr_sound (inherits its sorry). -/
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

/-- SCCP-transformed expressions preserve semantics (sorry-free version).
    Uses strong soundness. -/
theorem sccpExpr_correct_strong (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvStrongSound σ ρ) :
    evalExpr ρ (sccpExpr σ e) = evalExpr ρ e := by
  simp only [sccpExpr]
  match h : absEvalExpr σ e with
  | .known v =>
    simp only [evalExpr]
    exact (absEvalExpr_strong_sound σ ρ e hsound v h).symm
  | .unknown => rfl
  | .overdefined => rfl

end MoltTIR
