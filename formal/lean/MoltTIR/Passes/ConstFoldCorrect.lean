/-
  MoltTIR.Passes.ConstFoldCorrect — correctness proof for constant folding.

  Main theorem: for any environment ρ and expression e,
  evaluating e and evaluating constFoldExpr(e) produce the same result.

  This is the first verified compiler pass in the Molt formalization.
-/
import MoltTIR.Passes.ConstFold

namespace MoltTIR

/-- Constant folding preserves expression semantics.
    For all environments and expressions, evalExpr ρ e = evalExpr ρ (constFoldExpr e). -/
theorem constFoldExpr_correct (ρ : Env) (e : Expr) :
    evalExpr ρ (constFoldExpr e) = evalExpr ρ e := by
  induction e with
  | val _ => rfl
  | var _ => rfl
  | bin op a b iha ihb =>
    simp only [constFoldExpr]
    split
    · -- Both sub-expressions folded to values.
      -- Derive: evalExpr ρ a = some va and evalExpr ρ b = some vb.
      -- Then generalize + subst so the match reduces on concrete discriminants.
      split <;> {
        simp only [*, evalExpr] at iha ihb
        simp only [evalExpr]
        generalize evalExpr ρ a = ea at *
        generalize evalExpr ρ b = eb at *
        subst ea; subst eb; simp_all [evalExpr]
      }
    · -- Catch-all: not both values. Result is .bin op a' b'.
      simp only [evalExpr]; rw [iha, ihb]
  | un op a iha =>
    simp only [constFoldExpr]
    split
    · -- Sub-expression folded to a value.
      split <;> {
        simp only [*, evalExpr] at iha
        simp only [evalExpr]
        generalize evalExpr ρ a = ea at *
        subst ea; simp_all [evalExpr]
      }
    · -- Catch-all: not a value.
      simp only [evalExpr]; rw [iha]

/-- Constant folding preserves instruction semantics. -/
theorem constFoldInstr_correct (ρ : Env) (i : Instr) :
    evalExpr ρ (constFoldInstr i).rhs = evalExpr ρ i.rhs := by
  simp [constFoldInstr, constFoldExpr_correct]

end MoltTIR
