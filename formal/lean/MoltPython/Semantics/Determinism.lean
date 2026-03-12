/-
  MoltPython.Semantics.Determinism -- Determinism of Python expression evaluation.

  Since evalPyExpr is a total function (Nat -> PyEnv -> PyExpr -> Option PyValue),
  determinism is structural: the same inputs always produce the same output.
  This mirrors the approach in MoltTIR.Semantics.Determinism.
-/
import MoltPython.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltPython

/-- evalPyExpr is deterministic: given the same fuel, environment, and expression,
    if it produces two values, they must be equal.
    This is trivially true because evalPyExpr is a function (not a relation). -/
theorem evalPyExpr_deterministic (fuel : Nat) (env : PyEnv) (e : PyExpr) :
    ∀ v1 v2, evalPyExpr fuel env e = some v1 →
             evalPyExpr fuel env e = some v2 → v1 = v2 := by
  intro v1 v2 h1 h2
  simp [h1] at h2
  exact h2

/-- Truthiness is deterministic (it's a function). -/
theorem truthy_deterministic (v : PyValue) :
    ∀ b1 b2, v.truthy = b1 → v.truthy = b2 → b1 = b2 := by
  intro b1 b2 h1 h2
  rw [← h1, ← h2]

/-- Binary operator evaluation is deterministic. -/
theorem evalBinOp_deterministic (op : BinOp) (a b : PyValue) :
    ∀ v1 v2, evalBinOp op a b = some v1 →
             evalBinOp op a b = some v2 → v1 = v2 := by
  intro v1 v2 h1 h2
  simp [h1] at h2
  exact h2

/-- Unary operator evaluation is deterministic. -/
theorem evalUnaryOp_deterministic (op : UnaryOp) (a : PyValue) :
    ∀ v1 v2, evalUnaryOp op a = some v1 →
             evalUnaryOp op a = some v2 → v1 = v2 := by
  intro v1 v2 h1 h2
  simp [h1] at h2
  exact h2

/-- Comparison operator evaluation is deterministic. -/
theorem evalCompareOp_deterministic (op : CompareOp) (a b : PyValue) :
    ∀ r1 r2, evalCompareOp op a b = some r1 →
             evalCompareOp op a b = some r2 → r1 = r2 := by
  intro r1 r2 h1 h2
  simp [h1] at h2
  exact h2

/-- evalExprList is deterministic. -/
theorem evalExprList_deterministic (fuel : Nat) (env : PyEnv) (es : List PyExpr) :
    ∀ vs1 vs2, evalExprList fuel env es = some vs1 →
               evalExprList fuel env es = some vs2 → vs1 = vs2 := by
  intro vs1 vs2 h1 h2
  simp [h1] at h2
  exact h2

/-- evalCompareChain is deterministic. -/
theorem evalCompareChain_deterministic (fuel : Nat) (env : PyEnv) (left : PyExpr)
    (ops : List CompareOp) (comps : List PyExpr) :
    ∀ v1 v2, evalCompareChain fuel env left ops comps = some v1 →
             evalCompareChain fuel env left ops comps = some v2 → v1 = v2 := by
  intro v1 v2 h1 h2
  simp [h1] at h2
  exact h2

/-- evalBoolOp is deterministic. -/
theorem evalBoolOp_deterministic (fuel : Nat) (env : PyEnv) (op : BoolOp) (es : List PyExpr) :
    ∀ v1 v2, evalBoolOp fuel env op es = some v1 →
             evalBoolOp fuel env op es = some v2 → v1 = v2 := by
  intro v1 v2 h1 h2
  simp [h1] at h2
  exact h2

end MoltPython
