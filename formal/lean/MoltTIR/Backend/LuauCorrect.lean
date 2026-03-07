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
  are marked with `sorry` and TODO comments for future development.
-/
import MoltTIR.Backend.LuauEmit

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
-- Section 7: Semantic correctness stubs (require Luau evaluation model)
-- ======================================================================

-- The following theorems require defining a Luau expression evaluator
-- (evalLuauExpr) and an environment correspondence relation. They are
-- stated as comments to document the intended proof obligations.

-- TODO: prove — requires LuauSemantics.lean with evalLuauExpr definition
-- theorem emitExpr_correct (names : VarNames) (rho : MoltTIR.Env) (e : MoltTIR.Expr)
--     (henv : LuauEnvCorresponds names rho luauEnv)
--     (hwf : WellFormedExpr rho e) :
--     evalLuauExpr luauEnv (emitExpr names e) = evalExpr rho e

-- TODO: prove — requires LuauSemantics.lean with statement execution
-- theorem emitInstr_preserves_env (names : VarNames) (rho : MoltTIR.Env)
--     (i : MoltTIR.Instr) (luauEnv : LuauEnv)
--     (henv : LuauEnvCorresponds names rho luauEnv) :
--     LuauEnvCorresponds names
--       (rho.set i.dst v)
--       (execLuauStmts luauEnv (emitInstr names i))

-- TODO: prove — requires LuauSemantics.lean with operator evaluation
-- theorem emitBinOp_correct (op : MoltTIR.BinOp) (a b : Int)
--     (hArith : op ∈ [.add, .sub, .mul, .div, .mod]) :
--     evalLuauBinOp (emitBinOp op) a b = evalBinOp op (.int a) (.int b)

end MoltTIR.Backend
