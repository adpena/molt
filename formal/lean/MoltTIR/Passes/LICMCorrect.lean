/-
  MoltTIR.Passes.LICMCorrect — correctness proof for LICM.

  Key theorem: hoisting a pure loop-invariant instruction preserves
  expression semantics. If the instruction's RHS only references variables
  defined outside the loop, then evaluating it at the preheader produces
  the same result as evaluating it inside any loop iteration.

  Proof strategy:
  - Use evalExpr_agreeOn: if two environments agree on exprVars, they
    produce the same evaluation result.
  - Show that loop iterations only modify loop-defined variables.
  - Loop-invariant expressions reference no loop-defined variables.
  - Therefore the preheader environment and any iteration environment
    agree on the expression's free variables.
-/
import MoltTIR.Passes.LICM
import MoltTIR.Passes.DCECorrect

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Environments that agree outside the loop
-- ══════════════════════════════════════════════════════════════════

/-- Two environments agree on all variables NOT defined in the loop. -/
def EnvAgreeOutsideLoop (f : Func) (loop : NaturalLoop) (ρ₁ ρ₂ : Env) : Prop :=
  ∀ x, x ∉ loopDefs f loop → ρ₁ x = ρ₂ x

/-- Agreement outside loop is symmetric. -/
theorem envAgreeOutsideLoop_symm (f : Func) (loop : NaturalLoop) (ρ₁ ρ₂ : Env)
    (h : EnvAgreeOutsideLoop f loop ρ₁ ρ₂) :
    EnvAgreeOutsideLoop f loop ρ₂ ρ₁ :=
  fun x hx => (h x hx).symm

/-- Agreement outside loop is reflexive. -/
theorem envAgreeOutsideLoop_refl (f : Func) (loop : NaturalLoop) (ρ : Env) :
    EnvAgreeOutsideLoop f loop ρ ρ :=
  fun _ _ => rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Loop-invariant expressions evaluate the same
-- ══════════════════════════════════════════════════════════════════

/-- If two environments agree outside the loop, and an expression is
    loop-invariant (all vars defined outside), then the expression
    evaluates to the same result in both environments. -/
theorem loopInvariantExpr_eval_eq (f : Func) (loop : NaturalLoop)
    (ρ₁ ρ₂ : Env) (e : Expr)
    (hagree : EnvAgreeOutsideLoop f loop ρ₁ ρ₂)
    (hinv : isLoopInvariantExpr f loop e) :
    evalExpr ρ₁ e = evalExpr ρ₂ e := by
  apply evalExpr_agreeOn
  intro x hx
  exact hagree x (hinv x hx)

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Setting a loop-defined variable preserves agreement
-- ══════════════════════════════════════════════════════════════════

/-- Extending the environment with a loop-defined variable preserves
    agreement outside the loop. -/
theorem envAgreeOutsideLoop_set_loopDef
    (f : Func) (loop : NaturalLoop) (ρ₁ ρ₂ : Env) (x : Var) (v : Value)
    (hagree : EnvAgreeOutsideLoop f loop ρ₁ ρ₂)
    (hdef : x ∈ loopDefs f loop) :
    EnvAgreeOutsideLoop f loop (ρ₁.set x v) ρ₂ := by
  intro y hy
  simp [Env.set]
  have hne : y ≠ x := fun heq => hy (heq ▸ hdef)
  simp [hne]
  exact hagree y hy

/-- Setting the same variable to the same value in both environments
    preserves agreement outside the loop. -/
theorem envAgreeOutsideLoop_set_both
    (f : Func) (loop : NaturalLoop) (ρ₁ ρ₂ : Env) (x : Var) (v : Value)
    (hagree : EnvAgreeOutsideLoop f loop ρ₁ ρ₂) :
    EnvAgreeOutsideLoop f loop (ρ₁.set x v) (ρ₂.set x v) := by
  intro y hy
  simp [Env.set]
  split
  · rfl
  · exact hagree y hy

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Main LICM correctness theorem
-- ══════════════════════════════════════════════════════════════════

/-- Main LICM correctness: a loop-invariant instruction evaluates to
    the same value at the preheader as at any point inside the loop,
    provided the environments agree outside the loop.

    This justifies hoisting: executing the instruction at the preheader
    produces the same result as executing it inside the loop body. -/
theorem licm_instr_correct (f : Func) (loop : NaturalLoop)
    (ρ_pre ρ_iter : Env) (i : Instr)
    (hagree : EnvAgreeOutsideLoop f loop ρ_pre ρ_iter)
    (hinv : isLoopInvariantInstr f loop i) :
    evalExpr ρ_pre i.rhs = evalExpr ρ_iter i.rhs := by
  exact loopInvariantExpr_eval_eq f loop ρ_pre ρ_iter i.rhs hagree hinv.1

/-- Hoisting preserves the evaluation of the hoisted instruction's RHS. -/
theorem licm_hoisted_eval (f : Func) (loop : NaturalLoop)
    (ρ_pre ρ_iter : Env) (i : Instr) (v : Value)
    (hagree : EnvAgreeOutsideLoop f loop ρ_pre ρ_iter)
    (hinv : isLoopInvariantInstr f loop i)
    (heval : evalExpr ρ_pre i.rhs = some v) :
    evalExpr ρ_iter i.rhs = some v := by
  rw [← licm_instr_correct f loop ρ_pre ρ_iter i hagree hinv]
  exact heval

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Executing loop-body instructions preserves agreement
-- ══════════════════════════════════════════════════════════════════

/-- Executing a single instruction whose dst is loop-defined
    preserves agreement outside the loop. -/
theorem execInstr_preserves_outsideLoop
    (f : Func) (loop : NaturalLoop) (ρ₁ ρ₂ : Env) (i : Instr) (v : Value)
    (hagree : EnvAgreeOutsideLoop f loop ρ₁ ρ₂)
    (hdef : i.dst ∈ loopDefs f loop) :
    EnvAgreeOutsideLoop f loop (ρ₁.set i.dst v) ρ₂ :=
  envAgreeOutsideLoop_set_loopDef f loop ρ₁ ρ₂ i.dst v hagree hdef

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Partition preserves instruction membership
-- ══════════════════════════════════════════════════════════════════

/-- Every instruction in the original list appears in exactly one partition. -/
theorem partitionInstrs_complete (f : Func) (loop : NaturalLoop)
    (instrs : List Instr) (j : Instr) (hj : j ∈ instrs) :
    j ∈ (partitionInstrs f loop instrs).1 ∨ j ∈ (partitionInstrs f loop instrs).2 := by
  induction instrs with
  | nil => exact nomatch hj
  | cons i rest ih =>
    simp only [partitionInstrs]
    cases hb : isLoopInvariantExprBool f loop i.rhs with
    | false =>
      simp only [hb, Bool.false_eq_true, ↓reduceIte]
      cases hj with
      | head => exact Or.inr (List.mem_cons_self)
      | tail _ hrest =>
        match ih hrest with
        | Or.inl h => exact Or.inl h
        | Or.inr h => exact Or.inr (List.Mem.tail _ h)
    | true =>
      simp only [hb, ↓reduceIte]
      cases hj with
      | head => exact Or.inl (List.mem_cons_self)
      | tail _ hrest =>
        match ih hrest with
        | Or.inl h => exact Or.inl (List.Mem.tail _ h)
        | Or.inr h => exact Or.inr h

-- ══════════════════════════════════════════════════════════════════
-- Section 7: All expressions are pure (current model)
-- ══════════════════════════════════════════════════════════════════

/-- In the current pure model, every instruction is loop-invariant
    iff its variables are all defined outside the loop.
    The effect check is always satisfied by instrEffect_pure. -/
theorem isLoopInvariantInstr_of_varsOutside (f : Func) (loop : NaturalLoop)
    (i : Instr)
    (hvars : isLoopInvariantExpr f loop i.rhs) :
    isLoopInvariantInstr f loop i :=
  ⟨hvars, instrEffect_pure i⟩

end MoltTIR
