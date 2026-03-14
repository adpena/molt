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
-/
import MoltLowering.ASTtoTIR
import MoltLowering.Properties

set_option autoImplicit false

namespace MoltLowering

-- ═══════════════════════════════════════════════════════════════════════════
-- Environment correspondence predicate
-- ═══════════════════════════════════════════════════════════════════════════

def envCorr (nm : NameMap) (pyEnv : MoltPython.PyEnv) (tirEnv : MoltTIR.Env) : Prop :=
  ∀ (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue),
    nm.lookup x = some n →
    pyEnv.lookup x = some v →
    ∃ tv, lowerValue v = some tv ∧ tirEnv n = some tv

theorem lowerEnv_corr (nm : NameMap) (pyEnv : MoltPython.PyEnv)
    (hscalar : ∀ (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue),
      nm.lookup x = some n →
      pyEnv.lookup x = some v →
      ∃ tv, lowerValue v = some tv) :
    envCorr nm pyEnv (lowerEnv nm pyEnv) := by
  sorry

-- ═══════════════════════════════════════════════════════════════════════════
-- Helper lemmas for operator correspondence
-- ═══════════════════════════════════════════════════════════════════════════

/-- If a Python BinOp produces a lowerable result, both inputs are lowerable. -/
theorem binOp_lowerable_inputs (op : MoltPython.BinOp) (va vb : MoltPython.PyValue)
    (pv : MoltPython.PyValue)
    (heval : MoltPython.evalBinOp op va vb = some pv)
    (hlv : ∃ tv, lowerValue pv = some tv) :
    (∃ tva, lowerValue va = some tva) ∧ (∃ tvb, lowerValue vb = some tvb) := by
  sorry

/-- If a Python UnaryOp produces a lowerable result, the input is lowerable. -/
theorem unaryOp_lowerable_input (op : MoltPython.UnaryOp) (va : MoltPython.PyValue)
    (pv : MoltPython.PyValue)
    (heval : MoltPython.evalUnaryOp op va = some pv)
    (hlv : ∃ tv, lowerValue pv = some tv) :
    ∃ tva, lowerValue va = some tva := by
  sorry

/-- Full binary operator correspondence. -/
theorem binOp_comm (op : MoltPython.BinOp) (va vb : MoltPython.PyValue)
    (tva tvb : MoltTIR.Value)
    (hla : lowerValue va = some tva) (hlb : lowerValue vb = some tvb)
    (pv : MoltPython.PyValue) (tv : MoltTIR.Value)
    (heval : MoltPython.evalBinOp op va vb = some pv)
    (hlv : lowerValue pv = some tv) :
    MoltTIR.evalBinOp (lowerBinOp op) tva tvb = some tv := by
  sorry

/-- Unary neg correspondence for any lowerable value. -/
theorem unaryOp_comm_neg (va : MoltPython.PyValue)
    (tva : MoltTIR.Value)
    (hla : lowerValue va = some tva)
    (pv : MoltPython.PyValue) (tv : MoltTIR.Value)
    (heval : MoltPython.evalUnaryOp .neg va = some pv)
    (hlv : lowerValue pv = some tv) :
    MoltTIR.evalUnOp (lowerUnaryOp .neg) tva = some tv := by
  sorry

-- ═══════════════════════════════════════════════════════════════════════════
-- Operator semantics correspondence (int-specialized, fully proved)
-- ═══════════════════════════════════════════════════════════════════════════

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
  induction e generalizing fuel te pv tv with
  | intLit n =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv
      simp [MoltTIR.evalExpr]
  | floatLit f =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f' =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv
      simp [MoltTIR.evalExpr]
  | boolLit b =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv
      simp [MoltTIR.evalExpr]
  | strLit s =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv
      simp [MoltTIR.evalExpr]
  | noneLit =>
    simp [lowerExpr] at hlower; subst hlower
    cases fuel with
    | zero => omega
    | succ f =>
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv
      simp [MoltTIR.evalExpr]
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
  | binOp pyop left right ih_left ih_right =>
    simp [lowerExpr] at hlower
    match hleft_lower : lowerExpr nm left, hright_lower : lowerExpr nm right with
    | some tl, some tr =>
      simp [hleft_lower, hright_lower] at hlower; subst hlower
      cases fuel with
      | zero => omega
      | succ f =>
        simp [MoltPython.evalPyExpr] at heval
        match heval_left : MoltPython.evalPyExpr f pyEnv left,
              heval_right : MoltPython.evalPyExpr f pyEnv right with
        | some val, some vrb =>
          simp [heval_left, heval_right] at heval
          have ⟨⟨tva, htva⟩, ⟨tvb, htvb⟩⟩ :=
            binOp_lowerable_inputs pyop val vrb pv heval ⟨tv, hlv⟩
          have hf_pos : f > 0 := by
            by_contra h; push_neg at h; interval_cases f
            simp [MoltPython.evalPyExpr] at heval_left
          have ih_l := ih_left henv f hf_pos tl hleft_lower val heval_left tva htva
          have ih_r := ih_right henv f hf_pos tr hright_lower vrb heval_right tvb htvb
          simp [MoltTIR.evalExpr, ih_l, ih_r]
          exact binOp_comm pyop val vrb tva tvb htva htvb pv tv heval hlv
        | some _, none => simp [heval_left, heval_right] at heval
        | none, _ => simp [heval_left] at heval
    | some _, none => simp [hleft_lower, hright_lower] at hlower
    | none, _ => simp [hleft_lower] at hlower
  | unaryOp pyop operand ih_operand =>
    simp [lowerExpr] at hlower
    match hop_lower : lowerExpr nm operand with
    | some ta =>
      simp [hop_lower] at hlower; subst hlower
      cases fuel with
      | zero => omega
      | succ f =>
        simp [MoltPython.evalPyExpr] at heval
        match heval_op : MoltPython.evalPyExpr f pyEnv operand with
        | some va =>
          simp [heval_op] at heval
          have ⟨tva, htva⟩ := unaryOp_lowerable_input pyop va pv heval ⟨tv, hlv⟩
          have hf_pos : f > 0 := by
            by_contra h; push_neg at h; interval_cases f
            simp [MoltPython.evalPyExpr] at heval_op
          have ih := ih_operand henv f hf_pos ta hop_lower va heval_op tva htva
          simp [MoltTIR.evalExpr, ih]
          cases pyop with
          | neg => exact unaryOp_comm_neg va tva htva pv tv heval hlv
          | not =>
            cases va <;> simp [lowerValue] at htva <;> subst htva <;>
              simp_all [MoltPython.evalUnaryOp, MoltTIR.evalUnOp, lowerUnaryOp,
                        lowerValue, MoltPython.PyValue.truthy]
          | invert =>
            cases va <;> simp [lowerValue] at htva <;> subst htva <;>
              simp [MoltPython.evalUnaryOp] at heval
        | none => simp [heval_op] at heval
    | none => simp [hop_lower] at hlower
  | compare _ _ _ => simp [lowerExpr] at hlower
  | boolOp _ _ => simp [lowerExpr] at hlower
  | ifExpr _ _ _ => simp [lowerExpr] at hlower
  | call _ _ => simp [lowerExpr] at hlower
  | subscript _ _ => simp [lowerExpr] at hlower
  | listExpr _ => simp [lowerExpr] at hlower
  | tupleExpr _ => simp [lowerExpr] at hlower
  | dictExpr _ _ => simp [lowerExpr] at hlower

-- ═══════════════════════════════════════════════════════════════════════════
-- Corollaries
-- ═══════════════════════════════════════════════════════════════════════════

theorem lowered_eval_deterministic
    (tirEnv : MoltTIR.Env) (te : MoltTIR.Expr) :
    ∀ v1 v2, MoltTIR.evalExpr tirEnv te = some v1 →
             MoltTIR.evalExpr tirEnv te = some v2 → v1 = v2 :=
  MoltTIR.evalExpr_deterministic tirEnv te

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
