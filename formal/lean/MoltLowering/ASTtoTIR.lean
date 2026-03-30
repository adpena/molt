/-
  MoltLowering.ASTtoTIR — Lowering from Python AST (MoltPython) to TIR (MoltTIR).

  Defines the translation functions that constitute the first phase of Molt's
  compilation pipeline: Python AST → TIR IR. This is the formal analog of
  `compiler/molt/frontend/` in the real codebase.

  The lowering covers the *expression subset* that both formalizations share:
  literals (int, float, bool, str, none), variables, binary ops, unary ops.
  Compound expressions (comparisons, boolops, if-exprs, calls, subscripts,
  list/tuple/dict literals) do not have direct TIR Expr analogs — they are
  lowered to sequences of TIR instructions in a block. We mark those cases
  with sorry and precise TODO annotations for future work.

  Design note: TIR uses SSA variables (Nat) while Python uses named variables
  (String). The lowering requires a name-to-SSA mapping, modeled here as a
  NameMap. The real compiler maintains this during HIR→TIR lowering.
-/
import MoltPython.Semantics.EvalExpr
import MoltTIR.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltLowering

/-- Mapping from Python variable names to TIR SSA variable indices.
    Built during lowering; each new Python name gets a fresh Nat. -/
abbrev NameMap := List (MoltPython.Name × MoltTIR.Var)

namespace NameMap

def empty : NameMap := []

def lookup (nm : NameMap) (x : MoltPython.Name) : Option MoltTIR.Var :=
  match nm with
  | [] => none
  | (k, v) :: rest => if k == x then some v else lookup rest x

def insert (nm : NameMap) (x : MoltPython.Name) (v : MoltTIR.Var) : NameMap :=
  (x, v) :: nm

theorem lookup_insert_eq (nm : NameMap) (x : MoltPython.Name) (v : MoltTIR.Var) :
    lookup (insert nm x v) x = some v := by
  simp [insert, lookup]

theorem lookup_insert_ne (nm : NameMap) (x y : MoltPython.Name) (v : MoltTIR.Var)
    (h : y ≠ x) :
    lookup (insert nm x v) y = lookup nm y := by
  simp [insert, lookup]
  intro heq
  exact absurd heq (Ne.symm h)

end NameMap

-- ═══════════════════════════════════════════════════════════════════════════
-- Value correspondence
-- ═══════════════════════════════════════════════════════════════════════════

/-- Lower a Python value to a TIR value.
    Only the scalar types shared by both formalizations are mapped.
    Compound values (list, tuple, dict, func, class) have no direct TIR Value
    analog — they are represented as heap objects in the runtime, not as
    TIR expression-level values. -/
def lowerValue : MoltPython.PyValue → Option MoltTIR.Value
  | .intVal n   => some (.int n)
  | .floatVal f => some (.float f)
  | .boolVal b  => some (.bool b)
  | .strVal s   => some (.str s)
  | .noneVal    => some .none
  -- Compound values do not lower to TIR expression values.
  -- They are represented via heap handles in the runtime.
  | .listVal _  => Option.none
  | .tupleVal _ => Option.none
  | .dictVal _  => Option.none
  | .funcVal _ _ _ => Option.none
  | .classVal _ => Option.none

-- Equation lemmas for lowerValue (needed because PyValue is a nested inductive
-- and simp/unfold can't reduce lowerValue in downstream proof contexts).
@[simp] theorem lowerValue_intVal (n : Int) : lowerValue (.intVal n) = some (.int n) := rfl
@[simp] theorem lowerValue_floatVal (f : Int) : lowerValue (.floatVal f) = some (.float f) := rfl
@[simp] theorem lowerValue_boolVal (b : Bool) : lowerValue (.boolVal b) = some (.bool b) := rfl
@[simp] theorem lowerValue_strVal (s : String) : lowerValue (.strVal s) = some (.str s) := rfl
@[simp] theorem lowerValue_noneVal : lowerValue .noneVal = some .none := rfl

-- ═══════════════════════════════════════════════════════════════════════════
-- Operator correspondence
-- ═══════════════════════════════════════════════════════════════════════════

/-- Lower a Python binary operator to a TIR binary operator.
    Python separates arithmetic (BinOp) from comparison (CompareOp);
    TIR merges them into a single BinOp enum. This function handles only
    the arithmetic/bitwise BinOp subset. -/
def lowerBinOp : MoltPython.BinOp → MoltTIR.BinOp
  | .add      => .add
  | .sub      => .sub
  | .mul      => .mul
  | .div      => .div
  | .floorDiv => .floordiv
  | .mod      => .mod
  | .pow      => .pow
  | .bitAnd   => .bit_and
  | .bitOr    => .bit_or
  | .bitXor   => .bit_xor
  | .lShift   => .lshift
  | .rShift   => .rshift

/-- Lower a Python comparison operator to a TIR binary operator.
    In TIR, comparisons are just binary operators that return bool. -/
def lowerCompareOp : MoltPython.CompareOp → Option MoltTIR.BinOp
  | .eq    => some .eq
  | .notEq => some .ne
  | .lt    => some .lt
  | .ltE   => some .le
  | .gt    => some .gt
  | .gtE   => some .ge
  -- `is`, `isNot`, `in`, `notIn` have no direct TIR BinOp analog.
  -- They lower to intrinsic calls in the real compiler.
  | .is    => Option.none
  | .isNot => Option.none
  | .«in»  => Option.none
  | .notIn => Option.none

/-- Lower a Python unary operator to a TIR unary operator. -/
def lowerUnaryOp : MoltPython.UnaryOp → MoltTIR.UnOp
  | .not    => .not
  | .neg    => .neg
  | .invert => .invert

-- ═══════════════════════════════════════════════════════════════════════════
-- Expression lowering
-- ═══════════════════════════════════════════════════════════════════════════

/-- Lower a Python expression to a TIR expression.

    This handles the subset of Python expressions that map directly to TIR's
    4-constructor Expr type (val, var, bin, un). Complex Python expressions
    (comparisons, boolops, if-exprs, calls, subscripts, collection literals)
    require lowering to TIR *instruction sequences* (multiple blocks/instrs),
    not a single TIR Expr. Those cases return none.

    The NameMap provides the Python-name → SSA-variable mapping. -/
def lowerExpr (nm : NameMap) : MoltPython.PyExpr → Option MoltTIR.Expr
  -- Literals map to TIR value expressions
  | .intLit n   => some (.val (.int n))
  | .floatLit f => some (.val (.float f))
  | .boolLit b  => some (.val (.bool b))
  | .strLit s   => some (.val (.str s))
  | .noneLit    => some (.val .none)
  -- Variable reference: look up the SSA variable number
  | .name x =>
      match nm.lookup x with
      | some v => some (.var v)
      | Option.none => Option.none
  -- Binary operators: recursively lower both operands
  | .binOp op left right =>
      match lowerExpr nm left, lowerExpr nm right with
      | some la, some ra => some (.bin (lowerBinOp op) la ra)
      | _, _ => Option.none
  -- Unary operators: recursively lower the operand
  | .unaryOp op operand =>
      match lowerExpr nm operand with
      | some a => some (.un (lowerUnaryOp op) a)
      | Option.none => Option.none
  -- The following Python expression forms do NOT map to a single TIR Expr.
  -- They require lowering to instruction sequences (multiple TIR ops).
  -- TODO(compiler, owner:compiler, milestone:M3, priority:P1, status:planned):
  --   Extend lowering to handle these via block-level instruction emission.
  | .compare _ _ _   => Option.none  -- lowers to sequence of compare+branch ops
  | .boolOp _ _      => Option.none  -- lowers to short-circuit branch sequence
  | .ifExpr _ _ _    => Option.none  -- lowers to br terminator
  | .call _ _        => Option.none  -- lowers to call instruction
  | .subscript _ _   => Option.none  -- lowers to intrinsic call
  | .listExpr _      => Option.none  -- lowers to list_new + list_append intrinsics
  | .tupleExpr _     => Option.none  -- lowers to tuple_new intrinsic
  | .dictExpr _ _    => Option.none  -- lowers to dict_new + dict_set intrinsics

-- ═══════════════════════════════════════════════════════════════════════════
-- Environment correspondence
-- ═══════════════════════════════════════════════════════════════════════════

/-- Lower a Python environment to a TIR environment using a name map.

    For each Python variable binding (name → PyValue) in the scope chain,
    if the name has an SSA variable assignment in the NameMap and the value
    can be lowered, we set that SSA variable in the TIR environment.

    This captures the semantic invariant: if the Python environment binds
    name x to value v, and the NameMap assigns x to SSA var n, then the
    TIR environment maps n to lowerValue v. -/
def lowerScope (nm : NameMap) (scope : MoltPython.Scope) (ρ : MoltTIR.Env) : MoltTIR.Env :=
  match scope with
  | [] => ρ
  | (x, v) :: rest =>
      let ρ' := lowerScope nm rest ρ
      match nm.lookup x, lowerValue v with
      | some n, some tv => ρ'.set n tv
      | _, _ => ρ'

def lowerScopes (nm : NameMap) (scopes : List MoltPython.Scope) (ρ : MoltTIR.Env) : MoltTIR.Env :=
  match scopes with
  | [] => ρ
  | s :: rest =>
      -- Inner scopes shadow outer scopes, so we process outer first,
      -- then inner scopes overwrite.
      let ρ' := lowerScopes nm rest ρ
      lowerScope nm s ρ'

/-- Lower a complete Python environment to a TIR environment. -/
def lowerEnv (nm : NameMap) (env : MoltPython.PyEnv) : MoltTIR.Env :=
  lowerScopes nm env.scopes MoltTIR.Env.empty

end MoltLowering
