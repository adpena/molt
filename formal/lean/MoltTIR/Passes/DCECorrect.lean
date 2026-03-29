/-
  MoltTIR.Passes.DCECorrect — correctness proof for dead code elimination.

  Key insight: if two environments agree on the free variables of an expression,
  they produce the same evaluation result. This means skipping a dead instruction
  (whose dst is unused) preserves semantics for all subsequent computations.
-/
import MoltTIR.Passes.DCE
import MoltTIR.Semantics.ExecBlock

namespace MoltTIR

/-- Two environments agree on a set of variables. -/
def EnvAgreeOn (vars : List Var) (ρ₁ ρ₂ : Env) : Prop :=
  ∀ x, x ∈ vars → ρ₁ x = ρ₂ x

theorem envAgreeOn_refl (vars : List Var) (ρ : Env) : EnvAgreeOn vars ρ ρ :=
  fun _ _ => rfl

/-- If environments agree on the vars of an expression, evaluation agrees. -/
theorem evalExpr_agreeOn (ρ₁ ρ₂ : Env) (e : Expr)
    (h : EnvAgreeOn (exprVars e) ρ₁ ρ₂) :
    evalExpr ρ₁ e = evalExpr ρ₂ e := by
  induction e with
  | val _ => rfl
  | var y =>
    simp only [evalExpr]
    exact h y (List.mem_cons_self)
  | bin op a b iha ihb =>
    simp only [evalExpr]
    have ha : EnvAgreeOn (exprVars a) ρ₁ ρ₂ :=
      fun x hx => h x (List.mem_append_left _ hx)
    have hb : EnvAgreeOn (exprVars b) ρ₁ ρ₂ :=
      fun x hx => h x (List.mem_append_right _ hx)
    rw [iha ha, ihb hb]
  | un op a iha =>
    simp only [evalExpr]
    rw [iha h]

/-- Setting a variable not in a var list preserves agreement. -/
theorem envAgreeOn_set_left_irrelevant (vars : List Var) (ρ₁ ρ₂ : Env) (x : Var) (v : Value)
    (h : EnvAgreeOn vars ρ₁ ρ₂) (hx : x ∉ vars) :
    EnvAgreeOn vars (ρ₁.set x v) ρ₂ :=
  fun y hy => by
    simp [Env.set]
    have hne : y ≠ x := fun heq => hx (heq ▸ hy)
    simp [hne]
    exact h y hy

/-- Setting the same variable to the same value preserves agreement. -/
theorem envAgreeOn_set_both (vars : List Var) (ρ₁ ρ₂ : Env) (x : Var) (v : Value)
    (h : EnvAgreeOn vars ρ₁ ρ₂) :
    EnvAgreeOn vars (ρ₁.set x v) (ρ₂.set x v) :=
  fun y hy => by
    simp [Env.set]
    split
    · rfl
    · exact h y hy

/-- Setting a variable not in vars on the right side preserves agreement. -/
theorem envAgreeOn_set_right_irrelevant (vars : List Var) (ρ₁ ρ₂ : Env) (x : Var) (v : Value)
    (h : EnvAgreeOn vars ρ₁ ρ₂) (hx : x ∉ vars) :
    EnvAgreeOn vars ρ₁ (ρ₂.set x v) :=
  fun y hy => by
    simp [Env.set]
    have hne : y ≠ x := fun heq => hx (heq ▸ hy)
    simp [hne]
    exact h y hy

/-- Core DCE correctness: executing filtered instructions and full instructions
    from environments that agree on `used` produces environments that still agree.

    Preconditions:
    - Dead instruction destinations don't appear in `used`
    - All RHS vars of all instructions are in `used` -/
theorem dce_instrs_agreeOn
    (used : List Var)
    (instrs : List Instr)
    (hdead : ∀ i ∈ instrs, ¬isLive used i → i.dst ∉ used)
    (hrhs : ∀ i ∈ instrs, ∀ x ∈ exprVars i.rhs, x ∈ used) :
    ∀ (ρ₁ ρ₂ : Env),
      EnvAgreeOn used ρ₁ ρ₂ →
      ∀ ρ₁' ρ₂',
        execInstrs ρ₁ (dceInstrs used instrs) = some ρ₁' →
        execInstrs ρ₂ instrs = some ρ₂' →
        EnvAgreeOn used ρ₁' ρ₂' := by
  induction instrs with
  | nil =>
    intro ρ₁ ρ₂ hagree ρ₁' ρ₂' h1 h2
    simp [dceInstrs, List.filter, execInstrs] at h1 h2
    subst h1; subst h2; exact hagree
  | cons i rest ih =>
    intro ρ₁ ρ₂ hagree ρ₁' ρ₂' h1 h2
    -- Full execution evaluates i.rhs in ρ₂
    simp only [execInstrs] at h2
    -- RHS vars are in used, so agreement holds for the RHS
    have hrhs_i : ∀ x ∈ exprVars i.rhs, x ∈ used :=
      hrhs i (List.mem_cons_self)
    have hagree_rhs : EnvAgreeOn (exprVars i.rhs) ρ₁ ρ₂ :=
      fun x hx => hagree x (hrhs_i x hx)
    have heval_eq : evalExpr ρ₁ i.rhs = evalExpr ρ₂ i.rhs :=
      evalExpr_agreeOn ρ₁ ρ₂ i.rhs hagree_rhs
    -- Match on evalExpr result in the full execution
    match hm : evalExpr ρ₂ i.rhs with
    | none => simp [hm] at h2
    | some val =>
      simp [hm] at h2
      have hm1 : evalExpr ρ₁ i.rhs = some val := by rw [heval_eq, hm]
      -- Set up rest-specific hypotheses
      have hdead_rest : ∀ j ∈ rest, ¬isLive used j → j.dst ∉ used :=
        fun j hj => hdead j (List.Mem.tail _ hj)
      have hrhs_rest : ∀ j ∈ rest, ∀ x ∈ exprVars j.rhs, x ∈ used :=
        fun j hj => hrhs j (List.Mem.tail _ hj)
      -- Case split on whether i is live
      simp only [dceInstrs, List.filter] at h1
      by_cases hlive : isLive used i
      · -- Live instruction: keep it, both execute i
        simp [hlive, execInstrs, hm1] at h1
        have hagree' : EnvAgreeOn used (ρ₁.set i.dst val) (ρ₂.set i.dst val) :=
          envAgreeOn_set_both used ρ₁ ρ₂ i.dst val hagree
        exact ih hdead_rest hrhs_rest (ρ₁.set i.dst val) (ρ₂.set i.dst val) hagree' ρ₁' ρ₂' h1 h2
      · -- Dead instruction: DCE skips, full executes
        simp [hlive] at h1
        have hdst_unused : i.dst ∉ used := hdead i (List.mem_cons_self) hlive
        -- ρ₂ gets extended with i.dst, but i.dst ∉ used, so agreement is preserved
        have hagree' : EnvAgreeOn used ρ₁ (ρ₂.set i.dst val) :=
          envAgreeOn_set_right_irrelevant used ρ₁ ρ₂ i.dst val hagree hdst_unused
        exact ih hdead_rest hrhs_rest ρ₁ (ρ₂.set i.dst val) hagree' ρ₁' ρ₂' h1 h2

end MoltTIR
