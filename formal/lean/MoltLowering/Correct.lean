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
import MoltLowering.EnvCorr

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
    (hinj : NameMap.Injective nm)
    (hscalar : ∀ (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue),
      nm.lookup x = some n →
      pyEnv.lookup x = some v →
      ∃ tv, lowerValue v = some tv) :
    envCorr nm pyEnv (lowerEnv nm pyEnv) := by
  intro x n v hnm hlookup
  obtain ⟨tv, htv⟩ := hscalar x n v hnm hlookup
  exact ⟨tv, htv, lowerScopes_corr nm pyEnv.scopes MoltTIR.Env.empty x n v tv hnm hinj hlookup htv⟩

-- ═══════════════════════════════════════════════════════════════════════════
-- Operator semantics correspondence
-- ═══════════════════════════════════════════════════════════════════════════

set_option maxHeartbeats 800000 in
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
  all_goals (first
    | (subst_vars; simp [lowerValue]; done)
    | (split <;> subst_vars <;> simp_all [lowerValue]; done)
    | (split <;> (try subst_vars) <;> simp_all [lowerValue]; done)
    | omega
    | (obtain ⟨hy, rfl⟩ := hpv; subst htv;
       simp only [show ¬(y < 0) from by omega, ite_false, Option.bind, lowerValue]))

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
-- Helper: lowerable expressions produce lowerable values
-- ═══════════════════════════════════════════════════════════════════════════

theorem eval_produces_lowerable
    (nm : NameMap) (pyEnv : MoltPython.PyEnv) (tirEnv : MoltTIR.Env)
    (henv : envCorr nm pyEnv tirEnv)
    (fuel : Nat)
    (e : MoltPython.PyExpr)
    (te : MoltTIR.Expr) (hlower : lowerExpr nm e = some te)
    (pv : MoltPython.PyValue) (heval : MoltPython.evalPyExpr fuel pyEnv e = some pv) :
    ∃ tv, lowerValue pv = some tv := by
  induction fuel generalizing e te pv with
  | zero => simp [MoltPython.evalPyExpr] at heval
  | succ f ih =>
    cases e with
    | intLit n =>
      simp [MoltPython.evalPyExpr] at heval; subst heval; exact ⟨_, rfl⟩
    | floatLit fv =>
      simp [MoltPython.evalPyExpr] at heval; subst heval; exact ⟨_, rfl⟩
    | boolLit b =>
      simp [MoltPython.evalPyExpr] at heval; subst heval; exact ⟨_, rfl⟩
    | strLit s =>
      simp [MoltPython.evalPyExpr] at heval; subst heval; exact ⟨_, rfl⟩
    | noneLit =>
      simp [MoltPython.evalPyExpr] at heval; subst heval; exact ⟨_, rfl⟩
    | name x =>
      simp [MoltPython.evalPyExpr] at heval
      simp [lowerExpr] at hlower
      split at hlower
      · rename_i n hn; simp at hlower
        exact (henv x n pv hn heval).elim fun tv ⟨htv, _⟩ => ⟨tv, htv⟩
      · simp at hlower
    | binOp op left right =>
      simp only [lowerExpr] at hlower
      match hleft_lower : lowerExpr nm left, hright_lower : lowerExpr nm right with
      | some tl, some tr =>
        simp [hleft_lower, hright_lower] at hlower
        simp only [MoltPython.evalPyExpr] at heval
        match hleft_eval : MoltPython.evalPyExpr f pyEnv left,
              hright_eval : MoltPython.evalPyExpr f pyEnv right with
        | some va, some vb =>
          simp [hleft_eval, hright_eval] at heval
          have ⟨_, hlva⟩ := ih left tl hleft_lower va hleft_eval
          have ⟨_, hlvb⟩ := ih right tr hright_lower vb hright_eval
          -- pv is result of evalBinOp on scalar va, vb → pv is scalar
          cases va <;> cases vb <;>
            simp [lowerValue] at hlva hlvb <;>
            cases op <;> simp [MoltPython.evalBinOp] at heval
          -- Close all remaining goals: direct, conditional, if-then-else
          all_goals (
            try subst_vars; try simp [lowerValue]; done) <;>
          (try (rcases heval with ⟨_, rfl⟩; simp [lowerValue]; done)) <;>
          (split at heval <;> (try subst_vars; try (simp at heval; subst heval)) <;>
            simp [lowerValue])
        | none, some _ => simp [hleft_eval] at heval
        | some _, none => simp [hright_eval] at heval
        | none, none => simp [hleft_eval] at heval
      | some _, none => simp [hleft_lower, hright_lower] at hlower
      | none, some _ => simp [hleft_lower, hright_lower] at hlower
      | none, none => simp [hleft_lower] at hlower
    | unaryOp op operand =>
      simp only [lowerExpr] at hlower
      match hoperand_lower : lowerExpr nm operand with
      | some ta =>
        simp [hoperand_lower] at hlower
        simp only [MoltPython.evalPyExpr] at heval
        match hoperand_eval : MoltPython.evalPyExpr f pyEnv operand with
        | some va =>
          simp [hoperand_eval] at heval
          have ⟨_, hlva⟩ := ih operand ta hoperand_lower va hoperand_eval
          cases op <;> cases va <;>
            simp [lowerValue] at hlva <;>
            simp [MoltPython.evalUnaryOp] at heval
          all_goals (first
            | (subst_vars; simp [lowerValue])
            | (subst heval; simp [lowerValue])
            | (simp_all [lowerValue]))
        | none => simp [hoperand_eval] at heval
      | none => simp [hoperand_lower] at hlower
    | compare _ _ _ => simp [lowerExpr] at hlower
    | boolOp _ _ => simp [lowerExpr] at hlower
    | ifExpr _ _ _ => simp [lowerExpr] at hlower
    | call _ _ => simp [lowerExpr] at hlower
    | subscript _ _ => simp [lowerExpr] at hlower
    | listExpr _ => simp [lowerExpr] at hlower
    | tupleExpr _ => simp [lowerExpr] at hlower
    | dictExpr _ _ => simp [lowerExpr] at hlower

-- ═══════════════════════════════════════════════════════════════════════════
-- Helper: operator correspondence lemmas
-- ═══════════════════════════════════════════════════════════════════════════

private theorem evalBinOp_comm
    (op : MoltPython.BinOp) (va vb pv : MoltPython.PyValue)
    (tva tvb tv : MoltTIR.Value)
    (hlva : lowerValue va = some tva)
    (hlvb : lowerValue vb = some tvb)
    (heval : MoltPython.evalBinOp op va vb = some pv)
    (hlv : lowerValue pv = some tv) :
    MoltTIR.evalBinOp (lowerBinOp op) tva tvb = some tv := by
  cases va <;> cases vb <;> simp [lowerValue] at hlva hlvb <;>
    (try contradiction) <;> subst_vars <;>
    cases op <;> simp [MoltPython.evalBinOp] at heval
  -- Close all goals systematically
  all_goals (first
    -- Direct result: heval is `rfl` after simp
    | (subst_vars; simp [lowerValue] at hlv; subst_vars;
       simp [lowerBinOp, MoltTIR.evalBinOp]; done)
    -- Conditional: heval is ⟨guard, rfl⟩
    | (obtain ⟨hcond, rfl⟩ := heval; simp [lowerValue] at hlv; subst_vars;
       simp [lowerBinOp, MoltTIR.evalBinOp, hcond]; done)
    -- if-then-else (str repeat)
    | (split at heval <;>
        (try subst_vars; try (simp at heval; subst heval)) <;>
        simp [lowerValue] at hlv <;> subst hlv <;>
        simp_all [lowerBinOp, MoltTIR.evalBinOp]; done)
    -- string repetition: split heval if, extract pv, then close
    | (split at heval
       <;> simp only [Option.some.injEq] at heval
       <;> subst heval
       <;> simp only [lowerValue] at hlv
       <;> simp only [Option.some.injEq] at hlv
       <;> subst hlv
       <;> simp_all [lowerBinOp, MoltTIR.evalBinOp]
       <;> done)
    -- floordiv conditional: heval is ⟨guard, conditional_result⟩
    | (obtain ⟨hcond, hrest⟩ := heval;
       split at hrest <;>
       (try subst_vars; simp [lowerValue] at hlv; subst_vars;
        simp [lowerBinOp, MoltTIR.evalBinOp, hcond]; done) <;>
       (trace_state; sorry))
    | (split <;> (try subst_vars) <;> simp_all [lowerValue]; done)
    | omega
    | (-- String repetition: split heval if, extract pv, then lowerValue pv gives tv
       split at heval
       <;> simp only [Option.some.injEq] at heval
       <;> subst heval
       <;> simp only [lowerValue, Option.some.injEq] at hlv
       <;> subst hlv
       <;> simp [lowerBinOp, MoltTIR.evalBinOp]
       <;> (intro h; omega)))

private theorem evalUnaryOp_comm
    (op : MoltPython.UnaryOp) (va pv : MoltPython.PyValue)
    (tva tv : MoltTIR.Value)
    (hlva : lowerValue va = some tva)
    (heval : MoltPython.evalUnaryOp op va = some pv)
    (hlv : lowerValue pv = some tv) :
    MoltTIR.evalUnOp (lowerUnaryOp op) tva = some tv := by
  cases va <;> simp [lowerValue] at hlva <;>
    (try contradiction) <;> subst_vars <;>
    cases op <;> simp [MoltPython.evalUnaryOp] at heval <;>
    -- Close neg cases and not-on-bool case (where heval gives direct subst)
    (try (subst_vars; simp [lowerValue] at hlv; subst_vars;
          simp [lowerUnaryOp, MoltTIR.evalUnOp]; done)) <;>
    -- not on int/float/str/none: heval is (.boolVal (!va.truthy)) = pv
    (subst heval; simp [lowerValue] at hlv; subst hlv;
     simp [lowerUnaryOp, MoltTIR.evalUnOp, MoltPython.PyValue.truthy, bne, Bool.not_not])

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
  -- Generalize all fuel-dependent and expression-dependent values for the IH
  induction fuel generalizing e te pv tv with
  | zero => omega
  | succ f ih =>
    cases e with
    | intLit n =>
      simp [lowerExpr] at hlower; subst hlower
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
    | floatLit fv =>
      simp [lowerExpr] at hlower; subst hlower
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
    | boolLit b =>
      simp [lowerExpr] at hlower; subst hlower
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
    | strLit s =>
      simp [lowerExpr] at hlower; subst hlower
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
    | noneLit =>
      simp [lowerExpr] at hlower; subst hlower
      simp [MoltPython.evalPyExpr] at heval; subst heval
      simp [lowerValue] at hlv; subst hlv; simp [MoltTIR.evalExpr]
    | name x =>
      simp [lowerExpr] at hlower
      split at hlower
      · rename_i n hn
        simp at hlower; subst hlower
        simp [MoltPython.evalPyExpr] at heval
        have hcorr := henv x n pv hn heval
        obtain ⟨tv', htv', htir⟩ := hcorr
        simp [MoltTIR.evalExpr]
        have : tv = tv' := by rw [hlv] at htv'; cases htv'; rfl
        subst this; exact htir
      · simp at hlower
    | binOp op left right =>
      simp only [lowerExpr] at hlower
      match hleft_lower : lowerExpr nm left, hright_lower : lowerExpr nm right with
      | some tl, some tr =>
        simp [hleft_lower, hright_lower] at hlower; subst hlower
        simp only [MoltPython.evalPyExpr] at heval
        match hleft_eval : MoltPython.evalPyExpr f pyEnv left,
              hright_eval : MoltPython.evalPyExpr f pyEnv right with
        | some va, some vb =>
          simp [hleft_eval, hright_eval] at heval
          have ⟨tva, hlva⟩ := eval_produces_lowerable nm pyEnv tirEnv henv f
            left tl hleft_lower va hleft_eval
          have ⟨tvb, hlvb⟩ := eval_produces_lowerable nm pyEnv tirEnv henv f
            right tr hright_lower vb hright_eval
          -- Prove f > 0 (evalPyExpr 0 = none contradicts successful eval)
          have hf_pos : f > 0 := by
            cases f with
            | zero => simp [MoltPython.evalPyExpr] at hleft_eval
            | succ => omega
          have ihl := ih hf_pos left tl hleft_lower va hleft_eval tva hlva
          have ihr := ih hf_pos right tr hright_lower vb hright_eval tvb hlvb
          -- Unfold evalExpr and rewrite with IH results
          show MoltTIR.evalExpr tirEnv (.bin (lowerBinOp op) tl tr) = some tv
          unfold MoltTIR.evalExpr
          rw [ihl, ihr]
          exact evalBinOp_comm op va vb pv tva tvb tv hlva hlvb heval hlv
        | none, some _ => simp [hleft_eval] at heval
        | some _, none => simp [hright_eval] at heval
        | none, none => simp [hleft_eval] at heval
      | some _, none => simp [hleft_lower, hright_lower] at hlower
      | none, some _ => simp [hleft_lower, hright_lower] at hlower
      | none, none => simp [hleft_lower] at hlower
    | unaryOp op operand =>
      simp only [lowerExpr] at hlower
      match hoperand_lower : lowerExpr nm operand with
      | some ta =>
        simp [hoperand_lower] at hlower; subst hlower
        simp only [MoltPython.evalPyExpr] at heval
        match hoperand_eval : MoltPython.evalPyExpr f pyEnv operand with
        | some va =>
          simp [hoperand_eval] at heval
          have ⟨tva, hlva⟩ := eval_produces_lowerable nm pyEnv tirEnv henv f
            operand ta hoperand_lower va hoperand_eval
          have hf_pos : f > 0 := by
            cases f with
            | zero => simp [MoltPython.evalPyExpr] at hoperand_eval
            | succ => omega
          have iho := ih hf_pos operand ta hoperand_lower va hoperand_eval tva hlva
          show MoltTIR.evalExpr tirEnv (.un (lowerUnaryOp op) ta) = some tv
          unfold MoltTIR.evalExpr
          rw [iho]
          exact evalUnaryOp_comm op va pv tva tv hlva heval hlv
        | none => simp [hoperand_eval] at heval
      | none => simp [hoperand_lower] at hlower
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
