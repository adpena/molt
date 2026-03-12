/-
  MoltTIR.Backend.LuauCorrect -- Correctness theorems for Luau emission.

  Proves that the translation from MoltTIR to Luau preserves key semantic
  properties. The main results:

  1. Expression emission is structurally faithful (literal/var/bin/un cases).
  2. Instruction emission produces exactly one local declaration.
  3. All known IR builtins map to valid Luau functions.
  4. 0-based IR index + 1 = 1-based Luau index.
  5. Unpacked builtin calls correctly access tuple elements.

  Complex semantic equivalence proofs (requiring a full Luau evaluation model)
  are now partially filled in using LuauSemantics.lean and LuauEnvCorr.lean.
-/
import MoltTIR.Backend.LuauEmit
import MoltTIR.Backend.LuauEnvCorr

set_option autoImplicit false

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Index adjustment correctness
-- ======================================================================

/-- 0-based IR index + 1 = 1-based Luau index.
    This is the fundamental invariant of the index adjustment transform.
    For any natural number n representing a 0-based index,
    adjustIndex produces n + 1 (the 1-based equivalent). -/
theorem index_adjust_correct (n : Nat) :
    adjustIndex (.intLit (Int.ofNat n)) = .binOp .add (.intLit (Int.ofNat n)) (.intLit 1) := by
  rfl

/-- Index adjustment preserves non-negativity: if the IR index is >= 0,
    the Luau index is >= 1. This is an arithmetic fact about the adjustment. -/
theorem index_adjust_nonneg (n : Nat) :
    (0 : Int) ≤ Int.ofNat n → (1 : Int) ≤ Int.ofNat n + 1 := by
  omega

/-- The table access helper correctly composes index adjustment with indexing. -/
theorem emitTableAccess_structure (tbl idx : LuauExpr) :
    emitTableAccess tbl idx = .index tbl (adjustIndex idx) := by
  rfl

-- ======================================================================
-- Section 2: Expression emission structural preservation
-- ======================================================================

/-- Value literals emit to the corresponding Luau literal. -/
theorem emitExpr_val_int (names : VarNames) (n : Int) :
    emitExpr names (.val (.int n)) = .intLit n := by
  rfl

theorem emitExpr_val_bool (names : VarNames) (b : Bool) :
    emitExpr names (.val (.bool b)) = .boolLit b := by
  rfl

theorem emitExpr_val_str (names : VarNames) (s : String) :
    emitExpr names (.val (.str s)) = .strLit s := by
  rfl

theorem emitExpr_val_none (names : VarNames) :
    emitExpr names (.val .none) = .nil := by
  rfl

/-- Variable references emit to varRef with the mapped name. -/
theorem emitExpr_var (names : VarNames) (x : MoltTIR.Var) :
    emitExpr names (.var x) = .varRef (names x) := by
  rfl

/-- Binary expressions emit structurally: emitExpr preserves the binary
    operator structure, mapping both the operator and sub-expressions. -/
theorem emitExpr_bin (names : VarNames) (op : MoltTIR.BinOp) (a b : MoltTIR.Expr) :
    emitExpr names (.bin op a b) =
      .binOp (emitBinOp op) (emitExpr names a) (emitExpr names b) := by
  rfl

/-- Unary expressions emit structurally. -/
theorem emitExpr_un (names : VarNames) (op : MoltTIR.UnOp) (a : MoltTIR.Expr) :
    emitExpr names (.un op a) =
      .unOp (emitUnOp op) (emitExpr names a) := by
  rfl

-- ======================================================================
-- Section 3: Instruction emission preserves environment
-- ======================================================================

/-- Instruction emission produces exactly one local declaration. -/
theorem emitInstr_single (names : VarNames) (i : MoltTIR.Instr) :
    (emitInstr names i).length = 1 := by
  rfl

/-- The emitted local declaration uses the correct variable name. -/
theorem emitInstr_name (names : VarNames) (i : MoltTIR.Instr) :
    emitInstr names i = [.localDecl (names i.dst) (some (emitExpr names i.rhs))] := by
  rfl

-- ======================================================================
-- Section 4: Builtin mapping completeness
-- ======================================================================

/-- The builtin mapping list is non-empty. -/
theorem builtinMapping_nonempty : builtinMapping.length > 0 := by
  native_decide

/-- print maps to print. -/
theorem builtin_print : lookupBuiltin "print" = some "print" := by
  native_decide

/-- len maps to molt_len. -/
theorem builtin_len : lookupBuiltin "len" = some "molt_len" := by
  native_decide

/-- str maps to tostring. -/
theorem builtin_str : lookupBuiltin "str" = some "tostring" := by
  native_decide

/-- abs maps to math.abs. -/
theorem builtin_abs : lookupBuiltin "abs" = some "math.abs" := by
  native_decide

/-- Unknown builtins return none. -/
theorem builtin_unknown : lookupBuiltin "nonexistent_func" = none := by
  native_decide

-- ======================================================================
-- Section 5: Calling convention correctness
-- ======================================================================

/-- The unpack calling convention produces a call with unpack wrapper. -/
theorem emitUnpackCall_structure (func argsTuple : LuauExpr) :
    emitUnpackCall func argsTuple =
      .call func [.call (.varRef "unpack") [argsTuple]] := by
  rfl

/-- Builtin call with known arity N produces exactly N index arguments. -/
theorem emitBuiltinCall_arity (func argsTuple : LuauExpr) (n : Nat) :
    match emitBuiltinCall func argsTuple n with
    | .call _ args => args.length = n
    | _ => False := by
  simp [emitBuiltinCall, List.length_map, List.length_range]

-- ======================================================================
-- Section 6: Operator mapping totality
-- ======================================================================

/-- The binary operator mapping is total: every IR BinOp maps to some LuauBinOp. -/
theorem emitBinOp_total (op : MoltTIR.BinOp) : ∃ (lop : LuauBinOp), emitBinOp op = lop := by
  cases op <;> exact ⟨_, rfl⟩

/-- The unary operator mapping is total: every IR UnOp maps to some LuauUnOp. -/
theorem emitUnOp_total (op : MoltTIR.UnOp) : ∃ (lop : LuauUnOp), emitUnOp op = lop := by
  cases op <;> exact ⟨_, rfl⟩

/-- Arithmetic binary operators map to their Luau counterparts faithfully. -/
theorem emitBinOp_add : emitBinOp .add = .add := by rfl
theorem emitBinOp_sub : emitBinOp .sub = .sub := by rfl
theorem emitBinOp_mul : emitBinOp .mul = .mul := by rfl
theorem emitBinOp_eq  : emitBinOp .eq  = .eq  := by rfl

-- ======================================================================
-- Section 7: Semantic correctness (Luau evaluation model)
-- ======================================================================

/-- Arithmetic binary operator correspondence: for the core arithmetic operators
    (add, sub, mul, mod), evaluating the emitted Luau operator on integer values
    produces the same result as evaluating the IR operator, modulo the value
    correspondence.

    This is the key semantic bridge: Luau number arithmetic on integers is
    identical to Python integer arithmetic (within the safe-integer range).

    Note: div and pow are excluded — Luau `/` is float division (not Python's
    `//` floor division), and pow has different edge cases. The emitBinOp mapping
    handles floordiv → idiv, but the Luau idiv (`//`) semantics are not yet
    modeled in evalLuauBinOp. -/
theorem emitBinOp_correct_add (a b : Int) :
    evalLuauBinOp (emitBinOp .add) (.number a) (.number b) =
      (MoltTIR.evalBinOp .add (.int a) (.int b)).map valueToLuau := by
  rfl

theorem emitBinOp_correct_sub (a b : Int) :
    evalLuauBinOp (emitBinOp .sub) (.number a) (.number b) =
      (MoltTIR.evalBinOp .sub (.int a) (.int b)).map valueToLuau := by
  rfl

theorem emitBinOp_correct_mul (a b : Int) :
    evalLuauBinOp (emitBinOp .mul) (.number a) (.number b) =
      (MoltTIR.evalBinOp .mul (.int a) (.int b)).map valueToLuau := by
  rfl

theorem emitBinOp_correct_mod (a b : Int) :
    evalLuauBinOp (emitBinOp .mod) (.number a) (.number b) =
      (MoltTIR.evalBinOp .mod (.int a) (.int b)).map valueToLuau := by
  simp [emitBinOp, evalLuauBinOp, MoltTIR.evalBinOp, valueToLuau]
  split <;> rfl

/-- Comparison operator correspondence for eq. -/
theorem emitBinOp_correct_eq (a b : Int) :
    evalLuauBinOp (emitBinOp .eq) (.number a) (.number b) =
      (MoltTIR.evalBinOp .eq (.int a) (.int b)).map valueToLuau := by
  rfl

/-- Comparison operator correspondence for lt. -/
theorem emitBinOp_correct_lt (a b : Int) :
    evalLuauBinOp (emitBinOp .lt) (.number a) (.number b) =
      (MoltTIR.evalBinOp .lt (.int a) (.int b)).map valueToLuau := by
  rfl

/-- Unary operator correspondence for neg. -/
theorem emitUnOp_correct_neg (a : Int) :
    evalLuauUnOp (emitUnOp .neg) (.number a) =
      (MoltTIR.evalUnOp .neg (.int a)).map valueToLuau := by
  rfl

/-- Unary operator correspondence for not. -/
theorem emitUnOp_correct_not (b : Bool) :
    evalLuauUnOp (emitUnOp .not) (.boolean b) =
      (MoltTIR.evalUnOp .not (.bool b)).map valueToLuau := by
  rfl

/-- Expression emission correctness for value literals:
    emitting a value literal and evaluating it in any Luau environment
    produces the corresponding Luau value. -/
theorem emitExpr_correct_val (names : VarNames) (lenv : LuauEnv) (v : MoltTIR.Value) :
    evalLuauExpr lenv (emitExpr names (.val v)) = some (valueToLuau v) := by
  cases v <;> rfl

/-- Expression emission correctness for variable references:
    if the environments correspond, then evaluating the emitted variable reference
    in the Luau environment yields the corresponding value. -/
theorem emitExpr_correct_var (names : VarNames) (ρ : MoltTIR.Env) (lenv : LuauEnv)
    (x : MoltTIR.Var) (v : MoltTIR.Value)
    (hcorr : LuauEnvCorresponds names ρ lenv)
    (hbound : ρ x = some v) :
    evalLuauExpr lenv (emitExpr names (.var x)) = some (valueToLuau v) := by
  simp [emitExpr, evalLuauExpr]
  exact evalLuauExpr_var_corr names ρ lenv x v hcorr hbound

/-- Full expression emission correctness: structural induction on Expr.
    For each IR expression e, if all variables in e are bound in ρ and the
    environments correspond, then evaluating the emitted Luau expression
    produces the same result (under value correspondence) as the IR evaluator.

    The Option-valued formulation handles the case where IR evaluation itself
    may return none (type errors, undefined vars). We prove: if IR eval succeeds,
    then Luau eval succeeds with the corresponding value.

    The bin/un cases use structural induction with `revert v` to generalize
    the value parameter in the induction hypotheses, then case-split on
    operators and value types. The abs unary op maps to neg in the Luau model
    (an approximation of the real math.abs wrapper), so that case requires
    sorry — see emitUnOp definition note. -/
theorem emitExpr_correct (names : VarNames) (ρ : MoltTIR.Env) (lenv : LuauEnv)
    (e : MoltTIR.Expr) (v : MoltTIR.Value)
    (hcorr : LuauEnvCorresponds names ρ lenv)
    (heval : MoltTIR.evalExpr ρ e = some v) :
    evalLuauExpr lenv (emitExpr names e) = some (valueToLuau v) := by
  revert v
  induction e with
  | val w =>
    intro v heval
    simp [MoltTIR.evalExpr] at heval
    subst heval
    exact emitExpr_correct_val names lenv w
  | var x =>
    intro v heval
    simp [MoltTIR.evalExpr] at heval
    exact emitExpr_correct_var names ρ lenv x v hcorr heval
  | bin op a b iha ihb =>
    intro v heval
    simp only [MoltTIR.evalExpr] at heval
    match ha_eval : MoltTIR.evalExpr ρ a, hb_eval : MoltTIR.evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have iha' := iha va ha_eval
      have ihb' := ihb vb hb_eval
      simp only [emitExpr, evalLuauExpr, iha', ihb']
      -- Case-split on operator and value types, substitute v via heval
      cases op <;> cases va <;> cases vb <;> simp [MoltTIR.evalBinOp] at heval
      -- For each case, heval tells us what v is; substitute and close
      all_goals (first
        | (subst heval; simp [emitBinOp, evalLuauBinOp, valueToLuau]; done)
        | (obtain ⟨hne, rfl⟩ := heval; simp [emitBinOp, evalLuauBinOp, valueToLuau, hne]; done)
        | simp_all [emitBinOp, evalLuauBinOp, valueToLuau])
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | un op a iha =>
    intro v heval
    simp only [MoltTIR.evalExpr] at heval
    match ha_eval : MoltTIR.evalExpr ρ a with
    | some va =>
      simp [ha_eval] at heval
      have iha' := iha va ha_eval
      simp only [emitExpr, evalLuauExpr, iha']
      cases op <;> cases va <;> simp [MoltTIR.evalUnOp] at heval
      all_goals (first
        | (subst heval; simp [emitUnOp, evalLuauUnOp, valueToLuau]; done)
        | sorry)  -- abs maps to neg (approximation); see emitUnOp note
    | none => simp [ha_eval] at heval

/-- Instruction emission preserves environment correspondence.
    After executing the emitted `local name = expr` statement, the Luau
    environment corresponds to the IR environment extended with the new binding.

    Preconditions:
    - The IR expression evaluates successfully (producing value v)
    - The naming context is injective on the extended domain
    - The Luau evaluation of the emitted expression succeeds (follows from
      emitExpr_correct when the expression is well-formed) -/
theorem emitInstr_preserves_env (names : VarNames) (ρ : MoltTIR.Env)
    (i : MoltTIR.Instr) (lenv : LuauEnv) (v : MoltTIR.Value)
    (hcorr : LuauEnvCorresponds names ρ lenv)
    (_heval : MoltTIR.evalExpr ρ i.rhs = some v)
    (hluau_eval : evalLuauExpr lenv (emitExpr names i.rhs) = some (valueToLuau v))
    (hfresh : ρ i.dst = none)
    (hinj : ∀ (y : MoltTIR.Var), ρ y ≠ none → names y ≠ names i.dst) :
    ∃ lenv', execLuauStmts lenv (emitInstr names i) = some lenv' ∧
      LuauEnvCorresponds names (ρ.set i.dst v) lenv' := by
  refine ⟨lenv.set (names i.dst) (valueToLuau v), ?_, envCorr_set names ρ lenv i.dst v hcorr hfresh hinj⟩
  simp [emitInstr, execLuauStmts, execLuauStmt, hluau_eval]

/-- Semantic index adjustment: evaluating `adjustIndex (intLit n)` in any Luau
    environment produces n+1 (the 1-based index). This completes the structural
    index_adjust_correct with a semantic evaluation proof. -/
theorem index_adjust_semantic (env : LuauEnv) (n : Nat) :
    evalLuauExpr env (adjustIndex (.intLit (Int.ofNat n))) =
      some (.number (Int.ofNat n + 1)) := by
  rfl

end MoltTIR.Backend
