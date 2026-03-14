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
    the Python value (lowered) equals the TIR value.

    This is the key invariant maintained across the lowering boundary.
    It says: if the NameMap maps Python name x to SSA var n, and the Python
    environment binds x to some value v, then the TIR environment maps n
    to lowerValue v. -/
def envCorr (nm : NameMap) (pyEnv : MoltPython.PyEnv) (tirEnv : MoltTIR.Env) : Prop :=
  ∀ (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue),
    nm.lookup x = some n →
    pyEnv.lookup x = some v →
    ∃ tv, lowerValue v = some tv ∧ tirEnv n = some tv

/-- lowerEnv produces an environment that corresponds to the source.

    TODO(compiler, owner:compiler, milestone:M3, priority:P1, status:planned):
    Full proof requires an injectivity hypothesis on the NameMap (each Python
    name maps to a distinct SSA variable). Without injectivity, lowerScope
    can overwrite an earlier binding for variable n with a later binding from
    a different Python name that also maps to n. The real compiler guarantees
    injectivity by construction (fresh SSA variable for each Python name).
    Deferred: add NameMap.Injective hypothesis and complete the proof. -/
theorem lowerEnv_corr (nm : NameMap) (pyEnv : MoltPython.PyEnv)
    -- We require that all mapped Python values are scalar (lowerable).
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

    Shows that evaluating a Python BinOp on integer values and then lowering
    the result equals lowering the values first and evaluating the TIR BinOp.

    This covers: add, sub, mul, mod (the ops that both formalizations
    implement for int*int). -/
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
  -- mod case: conditional on y == 0
  all_goals split <;> simp_all

/-- Unary operator semantics correspondence.

    For neg on int and not on any value (after lowering), Python and TIR agree. -/
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

/-- **Semantic preservation for expression lowering.**

    If:
    - The Python expression `e` lowers to TIR expression `te` under name map `nm`
    - The Python and TIR environments correspond under `nm`
    - The Python evaluator (with sufficient fuel) produces value `pv`
    - `pv` is a scalar value (lowerable)

    Then:
    - The TIR evaluator on `te` produces `lowerValue pv`

    This is the CompCert-style forward simulation for the expression fragment.
    It guarantees that the lowering does not change the meaning of expressions.

    The proof proceeds by structural induction on the Python expression.
    Only the expression forms where lowerExpr succeeds are relevant
    (literals, variables, binops, unaryops). -/
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
    simp [lowerExpr] at hlower
    subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval
      subst heval
      simp [lowerValue] at hlv
      subst hlv
      simp [MoltTIR.evalExpr]
  | floatLit f =>
    simp [lowerExpr] at hlower
    subst hlower
    cases fuel with
    | zero => omega
    | succ f' =>
      simp [MoltPython.evalPyExpr] at heval
      subst heval
      simp [lowerValue] at hlv
      subst hlv
      simp [MoltTIR.evalExpr]
  | boolLit b =>
    simp [lowerExpr] at hlower
    subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval
      subst heval
      simp [lowerValue] at hlv
      subst hlv
      simp [MoltTIR.evalExpr]
  | strLit s =>
    simp [lowerExpr] at hlower
    subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval
      subst heval
      simp [lowerValue] at hlv
      subst hlv
      simp [MoltTIR.evalExpr]
  | noneLit =>
    simp [lowerExpr] at hlower
    subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval
      subst heval
      simp [lowerValue] at hlv
      subst hlv
      simp [MoltTIR.evalExpr]
  | name x =>
    -- Variable case: use environment correspondence
    simp [lowerExpr] at hlower
    split at hlower
    · rename_i n hn
      simp at hlower
      subst hlower
      cases fuel with
      | zero => omega
      | succ f =>
        simp [MoltPython.evalPyExpr] at heval
        -- heval : pyEnv.lookup x = some pv
        have hcorr := henv x n pv hn heval
        obtain ⟨tv', htv', htir⟩ := hcorr
        simp [MoltTIR.evalExpr]
        -- We know lowerValue pv = some tv (from hlv) and some tv' (from htv')
        have : tv = tv' := by
          have h := htv'
          rw [hlv] at h
          cases h
          rfl
        subst this
        exact htir
    · simp at hlower
  | binOp _op _left _right =>
    -- TODO(compiler, owner:compiler, milestone:M3, priority:P1, status:partial):
    --   The binOp inductive case requires structural induction (to get IH for
    --   sub-expressions), sub-expression lowerability, and operator correspondence.
    --
    --   Blockers preventing proof completion:
    --   (a) TIR evalBinOp only models int*int arithmetic (add, sub, mul, mod)
    --       and int*int comparisons. Python evalBinOp also supports float, str,
    --       list, and tuple operations. The operator correspondence fails for
    --       these types because TIR returns none where Python succeeds.
    --   (b) TIR evalBinOp does not model pow, div, floorDiv, or bitwise ops
    --       even for int*int. The catch-all returns none.
    --
    --   To close: either extend TIR evalBinOp to cover the full operator set,
    --   or restrict the theorem to the int*int arithmetic subset that TIR
    --   supports (requires a "TIR-compatible types" predicate on expressions).
    sorry
  | unaryOp _op _operand =>
    -- TODO(compiler, owner:compiler, milestone:M3, priority:P1, status:partial):
    --   The unaryOp case has a semantic gap for .not: Python's .not accepts
    --   any value via truthy coercion (e.g., not [] == True), but TIR's .not
    --   only accepts bool. When the operand evaluates to a non-boolean scalar
    --   (str, int, none), the operand IS lowerable but TIR .not would fail.
    --   This requires either:
    --   (a) Extending TIR evalUnOp .not to handle truthy coercion, or
    --   (b) Lowering .not to a truthy-then-negate instruction sequence.
    --   For .neg on int, the correspondence holds (proved via unaryOp_neg_int_comm).
    sorry
  | compare _ _ _ =>
    -- compare does not lower to a single TIR Expr
    simp [lowerExpr] at hlower
  | boolOp _ _ =>
    simp [lowerExpr] at hlower
  | ifExpr _ _ _ =>
    simp [lowerExpr] at hlower
  | call _ _ =>
    simp [lowerExpr] at hlower
  | subscript _ _ =>
    simp [lowerExpr] at hlower
  | listExpr _ =>
    simp [lowerExpr] at hlower
  | tupleExpr _ =>
    simp [lowerExpr] at hlower
  | dictExpr _ _ =>
    simp [lowerExpr] at hlower

-- ═══════════════════════════════════════════════════════════════════════════
-- Corollary: Determinism of lowered evaluation
-- ═══════════════════════════════════════════════════════════════════════════

/-- If lowering preserves evaluation, and both source and target evaluators
    are deterministic, then the lowered program is deterministic.

    This follows directly from MoltTIR.evalExpr_deterministic but we state
    it explicitly as a bridge property. -/
theorem lowered_eval_deterministic
    (tirEnv : MoltTIR.Env) (te : MoltTIR.Expr) :
    ∀ v1 v2, MoltTIR.evalExpr tirEnv te = some v1 →
             MoltTIR.evalExpr tirEnv te = some v2 → v1 = v2 :=
  MoltTIR.evalExpr_deterministic tirEnv te

-- ═══════════════════════════════════════════════════════════════════════════
-- Backward direction (for completeness characterization)
-- ═══════════════════════════════════════════════════════════════════════════

/-- **Backward preservation**: if the TIR evaluator produces a result for
    a lowered expression, then the Python evaluator (with sufficient fuel)
    also produces a result whose lowering matches.

    This is the other half of the simulation — it ensures the lowering does
    not introduce new behaviors that weren't present in the source.

    TODO(compiler, owner:compiler, milestone:M4, priority:P2, status:planned):
    Full backward proof. Requires showing that lowerExpr is "semantics-reflecting":
    if evalExpr tirEnv (lowerExpr nm e) = some tv, then
    ∃ pv fuel, evalPyExpr fuel pyEnv e = some pv ∧ lowerValue pv = some tv.
    The main challenge is constructing the fuel witness. -/
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
