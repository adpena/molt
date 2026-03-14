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
-- Lowerable-input lemmas
-- ═══════════════════════════════════════════════════════════════════════════

/-- If a Python BinOp produces a lowerable result, then both inputs are lowerable.
    This is a key structural fact: the Python evalBinOp cases that produce
    scalar results always operate on scalar inputs. -/
theorem binOp_lowerable_inputs (op : MoltPython.BinOp) (va vb : MoltPython.PyValue)
    (pv : MoltPython.PyValue)
    (heval : MoltPython.evalBinOp op va vb = some pv)
    (hlv : ∃ tv, lowerValue pv = some tv) :
    (∃ tva, lowerValue va = some tva) ∧ (∃ tvb, lowerValue vb = some tvb) := by
  cases op <;> cases va <;> cases vb <;>
    simp [MoltPython.evalBinOp] at heval <;>
    (first
     | (subst heval; exact ⟨⟨_, rfl⟩, ⟨_, rfl⟩⟩)
     | (obtain ⟨_, rfl⟩ := heval; exact ⟨⟨_, rfl⟩, ⟨_, rfl⟩⟩)
     | (split at heval <;> (first | subst heval | obtain ⟨_, rfl⟩ := heval) <;>
        exact ⟨⟨_, rfl⟩, ⟨_, rfl⟩⟩)
     | (obtain ⟨tv, htv⟩ := hlv;
        cases pv <;> simp [lowerValue] at htv))

/-- If a Python UnaryOp produces a lowerable result, then the input is lowerable. -/
theorem unaryOp_lowerable_input (op : MoltPython.UnaryOp) (va : MoltPython.PyValue)
    (pv : MoltPython.PyValue)
    (heval : MoltPython.evalUnaryOp op va = some pv)
    (hlv : ∃ tv, lowerValue pv = some tv) :
    ∃ tva, lowerValue va = some tva := by
  cases op <;> cases va <;> simp [MoltPython.evalUnaryOp] at heval <;>
    (first
     | exact ⟨_, rfl⟩
     | (subst heval; exact ⟨_, rfl⟩))

-- ═══════════════════════════════════════════════════════════════════════════
-- Operator semantics correspondence
-- ═══════════════════════════════════════════════════════════════════════════

/-- Binary operator semantics correspondence for int*int arithmetic.

    Shows that evaluating a Python BinOp on integer values and then lowering
    the result equals lowering the values first and evaluating the TIR BinOp.

    This covers: add, sub, mul, mod, floorDiv, pow. -/
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

/-- Full binary operator correspondence: for ANY operator and ANY pair of lowerable
    values, if Python evalBinOp succeeds with a lowerable result, then TIR evalBinOp
    on the lowered values produces the lowered result. -/
theorem binOp_comm (op : MoltPython.BinOp) (va vb : MoltPython.PyValue)
    (tva tvb : MoltTIR.Value)
    (hla : lowerValue va = some tva) (hlb : lowerValue vb = some tvb)
    (pv : MoltPython.PyValue) (tv : MoltTIR.Value)
    (heval : MoltPython.evalBinOp op va vb = some pv)
    (hlv : lowerValue pv = some tv) :
    MoltTIR.evalBinOp (lowerBinOp op) tva tvb = some tv := by
  cases va <;> cases vb <;> simp [lowerValue] at hla hlb
  all_goals (subst hla; subst hlb;
    cases op <;> simp_all [MoltPython.evalBinOp, MoltTIR.evalBinOp, lowerBinOp, lowerValue];
    first
    | done
    | (split at heval <;> simp_all))

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

/-- Unary neg correspondence for any lowerable value. -/
theorem unaryOp_comm_neg (va : MoltPython.PyValue)
    (tva : MoltTIR.Value)
    (hla : lowerValue va = some tva)
    (pv : MoltPython.PyValue) (tv : MoltTIR.Value)
    (heval : MoltPython.evalUnaryOp .neg va = some pv)
    (hlv : lowerValue pv = some tv) :
    MoltTIR.evalUnOp (lowerUnaryOp .neg) tva = some tv := by
  cases va <;> simp [lowerValue] at hla <;> subst hla <;>
    simp_all [MoltPython.evalUnaryOp, MoltTIR.evalUnOp, lowerUnaryOp, lowerValue]

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
  induction e generalizing fuel te pv tv with
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
  | binOp pyop left right ih_left ih_right =>
    -- BinOp case: use induction hypotheses on sub-expressions
    simp [lowerExpr] at hlower
    match hleft_lower : lowerExpr nm left, hright_lower : lowerExpr nm right with
    | some tl, some tr =>
      simp [hleft_lower, hright_lower] at hlower
      subst hlower
      cases fuel with
      | zero => omega
      | succ f =>
        simp [MoltPython.evalPyExpr] at heval
        match heval_left : MoltPython.evalPyExpr f pyEnv left,
              heval_right : MoltPython.evalPyExpr f pyEnv right with
        | some val, some vrb =>
          simp [heval_left, heval_right] at heval
          -- heval : MoltPython.evalBinOp pyop val vrb = some pv
          -- By binOp_lowerable_inputs, both val and vrb are lowerable
          have ⟨⟨tva, htva⟩, ⟨tvb, htvb⟩⟩ :=
            binOp_lowerable_inputs pyop val vrb pv heval ⟨tv, hlv⟩
          -- Derive fuel positivity from successful sub-expression evaluation
          have hf_pos : f > 0 := by
            by_contra h
            push_neg at h
            interval_cases f
            simp [MoltPython.evalPyExpr] at heval_left
          -- By IH, TIR evaluates sub-expressions correctly
          have ih_l := ih_left henv f hf_pos tl hleft_lower val heval_left tva htva
          have ih_r := ih_right henv f hf_pos tr hright_lower vrb heval_right tvb htvb
          -- ih_l : evalExpr tirEnv tl = some tva
          -- ih_r : evalExpr tirEnv tr = some tvb
          simp [MoltTIR.evalExpr, ih_l, ih_r]
          -- Goal: evalBinOp (lowerBinOp pyop) tva tvb = some tv
          exact binOp_comm pyop val vrb tva tvb htva htvb pv tv heval hlv
        | some _, none => simp [heval_left, heval_right] at heval
        | none, _ => simp [heval_left] at heval
    | some _, none => simp [hleft_lower, hright_lower] at hlower
    | none, _ => simp [hleft_lower] at hlower
  | unaryOp pyop operand ih_operand =>
    -- UnaryOp case: use induction hypothesis on the operand
    simp [lowerExpr] at hlower
    match hop_lower : lowerExpr nm operand with
    | some ta =>
      simp [hop_lower] at hlower
      subst hlower
      cases fuel with
      | zero => omega
      | succ f =>
        simp [MoltPython.evalPyExpr] at heval
        match heval_op : MoltPython.evalPyExpr f pyEnv operand with
        | some va =>
          simp [heval_op] at heval
          -- heval : MoltPython.evalUnaryOp pyop va = some pv
          have ⟨tva, htva⟩ := unaryOp_lowerable_input pyop va pv heval ⟨tv, hlv⟩
          have hf_pos : f > 0 := by
            by_contra h
            push_neg at h
            interval_cases f
            simp [MoltPython.evalPyExpr] at heval_op
          have ih := ih_operand henv f hf_pos ta hop_lower va heval_op tva htva
          -- ih : evalExpr tirEnv ta = some tva
          simp [MoltTIR.evalExpr, ih]
          -- Goal: evalUnOp (lowerUnaryOp pyop) tva = some tv
          -- Case split on the operator
          cases pyop with
          | neg => exact unaryOp_comm_neg va tva htva pv tv heval hlv
          | not =>
            -- Python's not uses truthy coercion. TIR's not only handles bool.
            -- Case-split on the value type to show correspondence.
            cases va <;> simp [lowerValue] at htva <;> subst htva <;>
              simp_all [MoltPython.evalUnaryOp, MoltTIR.evalUnOp, lowerUnaryOp,
                        lowerValue, MoltPython.PyValue.truthy]
          | invert =>
            -- Python's invert (~) only handles int; falls to catch-all for others
            cases va <;> simp [lowerValue] at htva <;> subst htva <;>
              simp [MoltPython.evalUnaryOp] at heval
        | none => simp [heval_op] at heval
    | none => simp [hop_lower] at hlower
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
