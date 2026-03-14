/-
  MoltLowering.Correct — Semantic preservation for AST→TIR lowering.

  The "big theorem" (CompCert-style `transf_program_correct`):
  evaluating a Python expression and then lowering the result value
  equals lowering the expression and then evaluating in TIR.

  Diagram:

      PyExpr  ──evalPyExpr──→  PyValue
        │                        │
    lowerExpr              lowerValue
        │                        │
        ▼                        ▼
      TIR.Expr ──evalExpr──→  TIR.Value

  The theorem states this diagram commutes for the expression subset
  where lowerExpr succeeds (scalars, variables, binops, unaryops).

  Approach:
  - Prove by structural induction on the Python expression.
  - The theorem requires an "environment correspondence" hypothesis:
    the Python env and TIR env agree on all mapped variables.
  - Literal cases are direct.
  - Variable case follows from environment correspondence.
  - BinOp case requires showing operator correspondence preserves semantics.
  - UnaryOp case is similar.
  - Complex cases (compare, boolop, if, call, etc.) are out of scope for
    expression-level lowering — they return none from lowerExpr.
-/
import MoltLowering.ASTtoTIR
import MoltLowering.Properties

set_option autoImplicit false

namespace MoltLowering

-- ═══════════════════════════════════════════════════════════════════════════
-- Environment correspondence predicate
-- ═══════════════════════════════════════════════════════════════════════════

/-- Two environments correspond under a name map: for every mapped variable,
    the Python value (lowered) equals the TIR value. -/
def envCorr (nm : NameMap) (pyEnv : MoltPython.PyEnv) (tirEnv : MoltTIR.Env) : Prop :=
  ∀ (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue),
    nm.lookup x = some n →
    pyEnv.lookup x = some v →
    ∃ tv, lowerValue v = some tv ∧ tirEnv n = some tv

/-- lowerEnv produces an environment that corresponds to the source. -/
theorem lowerEnv_corr (nm : NameMap) (pyEnv : MoltPython.PyEnv)
    (hscalar : ∀ (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue),
      nm.lookup x = some n →
      pyEnv.lookup x = some v →
      ∃ tv, lowerValue v = some tv) :
    envCorr nm pyEnv (lowerEnv nm pyEnv) := by
  sorry

-- ═══════════════════════════════════════════════════════════════════════════
-- Operator semantics correspondence
-- ═══════════════════════════════════════════════════════════════════════════

/-- Binary operator semantics correspondence for int*int arithmetic.
    Covers: add, sub, mul, mod, floorDiv, pow. -/
theorem binOp_int_comm (op : MoltPython.BinOp) (x y : Int)
    (hresult : ∃ pv, MoltPython.evalBinOp op (.intVal x) (.intVal y) = some pv)
    (htir : ∃ tv, MoltTIR.evalBinOp (lowerBinOp op) (.int x) (.int y) = some tv) :
    (do let pv ← MoltPython.evalBinOp op (.intVal x) (.intVal y)
        lowerValue pv) =
    MoltTIR.evalBinOp (lowerBinOp op) (.int x) (.int y) := by
  obtain ⟨pv, hpv⟩ := hresult
  obtain ⟨tv, htv⟩ := htir
  cases op <;> simp_all [MoltPython.evalBinOp, MoltTIR.evalBinOp, lowerBinOp,
    lowerValue, Option.bind]
  all_goals split <;> simp_all

theorem unaryOp_neg_int_comm (x : Int) :
    (do let pv ← MoltPython.evalUnaryOp .neg (.intVal x)
        lowerValue pv) =
    MoltTIR.evalUnOp (lowerUnaryOp .neg) (.int x) := by
  simp [MoltPython.evalUnaryOp, MoltTIR.evalUnOp, lowerUnaryOp, lowerValue]

theorem unaryOp_not_bool_comm (b : Bool) :
    (do let pv ← MoltPython.evalUnaryOp .not (.boolVal b)
        lowerValue pv) =
    MoltTIR.evalUnOp (lowerUnaryOp .not) (.bool b) := by
  simp [MoltPython.evalUnaryOp, MoltTIR.evalUnOp, lowerUnaryOp, lowerValue,
        MoltPython.PyValue.truthy]

-- ═══════════════════════════════════════════════════════════════════════════
-- The Main Theorem: Semantic Preservation
-- ═══════════════════════════════════════════════════════════════════════════

/-- **Semantic preservation for expression lowering.** -/
theorem lowering_preserves_eval
    (nm : NameMap) (pyEnv : MoltPython.PyEnv) (tirEnv : MoltTIR.Env)
    (henv : envCorr nm pyEnv tirEnv)
    (fuel : Nat) (hfuel : fuel > 0)
    (e : MoltPython.PyExpr)
    (te : MoltTIR.Expr) (hlower : lowerExpr nm e = some te)
    (pv : MoltPython.PyValue) (heval : MoltPython.evalPyExpr fuel pyEnv e = some pv)
    (tv : MoltTIR.Value) (hlv : lowerValue pv = some tv) :
    MoltTIR.evalExpr tirEnv te = some tv := by
  cases e with
  | intLit n =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
  | floatLit f =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f' =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
  | boolLit b =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
  | strLit s =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
  | noneLit =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
  | name x =>
    simp [lowerExpr] at hlower
    split at hlower
    · rename_i n hn
      simp at hlower; subst hlower
      cases fuel with
      | zero => omega
      | succ f =>
        simp [MoltPython.evalPyExpr] at heval
        have hcorr := henv x n pv hn heval
        obtain ⟨tv', htv', htir⟩ := hcorr
        simp [MoltTIR.evalExpr]
        have : tv = tv' := by rw [hlv] at htv'; cases htv'; rfl
        subst this; exact htir
    · simp at hlower
  | binOp _op _left _right =>
    -- TODO(compiler, owner:compiler, milestone:M3, priority:P1, status:partial):
    --   TIR evalBinOp now covers the full operator set (add, sub, mul, mod,
    --   floordiv, pow, float ops, string ops, comparisons). The operator
    --   correspondence holds for all scalar type combinations.
    --
    --   Remaining blocker: PyExpr is a nested inductive type (contains List PyExpr
    --   in some constructors), so Lean's `induction` tactic cannot generate the
    --   eliminator. The `cases` tactic does not provide IH for sub-expressions.
    --   To close: define a custom well-founded recursion principle for PyExpr,
    --   or use `termination_by` with a size measure.
    sorry
  | unaryOp _op _operand =>
    -- TODO(compiler, owner:compiler, milestone:M3, priority:P1, status:partial):
    --   TIR evalUnOp now covers not on all scalar types (truthy coercion),
    --   neg on int and float, and abs on int. The semantic gap for not is closed.
    --
    --   Remaining blocker: same nested inductive issue as binOp — need IH for
    --   the operand sub-expression, which `cases` does not provide.
    sorry
  | compare _ _ _ => simp [lowerExpr] at hlower
  | boolOp _ _ => simp [lowerExpr] at hlower
  | ifExpr _ _ _ => simp [lowerExpr] at hlower
  | call _ _ => simp [lowerExpr] at hlower
  | subscript _ _ => simp [lowerExpr] at hlower
  | listExpr _ => simp [lowerExpr] at hlower
  | tupleExpr _ => simp [lowerExpr] at hlower
  | dictExpr _ _ => simp [lowerExpr] at hlower

-- ═══════════════════════════════════════════════════════════════════════════
-- Corollary: Determinism of lowered evaluation
-- ═══════════════════════════════════════════════════════════════════════════

theorem lowered_eval_deterministic
    (tirEnv : MoltTIR.Env) (te : MoltTIR.Expr) :
    ∀ v1 v2, MoltTIR.evalExpr tirEnv te = some v1 →
             MoltTIR.evalExpr tirEnv te = some v2 → v1 = v2 :=
  MoltTIR.evalExpr_deterministic tirEnv te

-- ═══════════════════════════════════════════════════════════════════════════
-- Backward direction (for completeness characterization)
-- ═══════════════════════════════════════════════════════════════════════════

theorem lowering_reflects_eval
    (nm : NameMap) (pyEnv : MoltPython.PyEnv) (tirEnv : MoltTIR.Env)
    (henv : envCorr nm pyEnv tirEnv)
    (e : MoltPython.PyExpr)
    (te : MoltTIR.Expr) (hlower : lowerExpr nm e = some te)
    (tv : MoltTIR.Value) (htir : MoltTIR.evalExpr tirEnv te = some tv) :
    ∃ (fuel : Nat) (pv : MoltPython.PyValue),
      MoltPython.evalPyExpr fuel pyEnv e = some pv ∧
      lowerValue pv = some tv := by
  sorry

end MoltLowering
