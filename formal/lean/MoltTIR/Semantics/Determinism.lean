/-
  MoltTIR.Semantics.Determinism — collected determinism theorems.

  Since our semantics is defined as total functions (not relations),
  determinism is immediate. These theorems serve as documentation and
  regression guards: if the semantics were accidentally changed to a
  relation, these would fail to compile.
-/
import MoltTIR.Semantics.ExecFunc

namespace MoltTIR

-- Re-export the key determinism results for convenient reference.

/-- Expression evaluation is deterministic. -/
def eval_det := @evalExpr_deterministic

/-- Function execution is deterministic (given the same fuel bound). -/
def exec_det := @execFunc_deterministic

/-- Instruction sequence execution is deterministic. -/
theorem execInstrs_deterministic (ρ : Env) (instrs : List Instr) :
    ∀ ρ1 ρ2, execInstrs ρ instrs = some ρ1 → execInstrs ρ instrs = some ρ2 → ρ1 = ρ2 := by
  intro ρ1 ρ2 h1 h2
  simp [h1] at h2
  exact h2

/-- Argument evaluation is deterministic. -/
theorem evalArgs_deterministic (ρ : Env) (es : List Expr) :
    ∀ vs1 vs2, evalArgs ρ es = some vs1 → evalArgs ρ es = some vs2 → vs1 = vs2 := by
  intro vs1 vs2 h1 h2
  simp [h1] at h2
  exact h2

/-- Parameter binding is deterministic. -/
theorem bindParams_deterministic (ps : List Var) (vs : List Value) :
    ∀ ρ1 ρ2, bindParams ps vs = some ρ1 → bindParams ps vs = some ρ2 → ρ1 = ρ2 := by
  intro ρ1 ρ2 h1 h2
  simp [h1] at h2
  exact h2

end MoltTIR
