/-
  MoltTIR.Backend.LuauEmit -- Translation from MoltTIR to Luau target AST.

  Models the core translation logic in runtime/molt-backend/src/luau.rs:
  - Expression emission (IR Expr -> LuauExpr)
  - Instruction emission (IR Instr -> List LuauStmt)
  - Block emission (IR Block -> List LuauStmt)
  - Function emission (IR Func -> LuauFunc)
  - Builtin function mapping (IR builtin names -> Luau wrapper closures)
  - 0-based to 1-based index adjustment for table access

  The translation uses a naming context (VarNames) to map SSA variable
  numbers to string names in the Luau output.
-/
import MoltTIR.Syntax
import MoltTIR.Semantics.EvalExpr
import MoltTIR.Backend.LuauSyntax

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Variable naming context
-- ======================================================================

/-- Maps IR SSA variable IDs to Luau variable name strings.
    In the real backend this is derived from Python source names
    when available, falling back to `_v{n}` synthetic names. -/
abbrev VarNames := MoltTIR.Var → String

/-- Default naming scheme: _v0, _v1, ... -/
def defaultVarName (x : MoltTIR.Var) : String := s!"_v{x}"

-- ======================================================================
-- Section 2: Operator mapping
-- ======================================================================

/-- Map IR binary operator to Luau binary operator. -/
def emitBinOp : MoltTIR.BinOp → LuauBinOp
  | .add => .add
  | .sub => .sub
  | .mul => .mul
  | .div => .div
  | .floordiv => .idiv
  | .mod => .mod
  | .pow => .pow
  | .eq => .eq
  | .ne => .ne
  | .lt => .lt
  | .le => .le
  | .gt => .gt
  | .ge => .ge
  -- Bitwise ops map to Luau's bit32 library calls in practice.
  -- At the AST level we model them as distinct bitwise operator nodes.
  | .bit_and => .band   -- real backend uses bit32.band
  | .bit_or  => .bor    -- real backend uses bit32.bor
  | .bit_xor => .bxor   -- real backend uses bit32.bxor
  | .lshift  => .lshl   -- real backend uses bit32.lshift
  | .rshift  => .lshr   -- real backend uses bit32.rshift
  | .and_    => .land   -- short-circuit modeled at terminator level
  | .or_     => .lor    -- short-circuit modeled at terminator level
  | .is      => .eq     -- identity mapped to equality in Luau
  | .is_not  => .ne     -- identity mapped to inequality in Luau
  | .in_     => .eq     -- membership approximated (placeholder)
  | .not_in  => .ne     -- membership approximated (placeholder)

/-- Map IR unary operator to Luau unary operator. -/
def emitUnOp : MoltTIR.UnOp → LuauUnOp
  | .neg => .neg
  | .not => .lnot
  | .abs => .abs
  | .invert => .lnot  -- approximation; real backend uses bit32.bnot
  | .pos => .abs     -- unary plus approximated as abs (identity for numeric)

-- ======================================================================
-- Section 3: Expression emission
-- ======================================================================

/-- Translate an IR expression to a Luau expression.
    This is the core of emit_expr in luau.rs. -/
def emitExpr (names : VarNames) : MoltTIR.Expr → LuauExpr
  | .val (.int n) => .intLit n
  | .val (.float f) => .floatLit f
  | .val (.str s) => .strLit s
  | .val (.bool b) => .boolLit b
  | .val .none => .nil
  | .var x => .varRef (names x)
  | .bin op a b => .binOp (emitBinOp op) (emitExpr names a) (emitExpr names b)
  | .un op a => .unOp (emitUnOp op) (emitExpr names a)

-- ======================================================================
-- Section 4: Index adjustment (0-based IR -> 1-based Luau)
-- ======================================================================

/-- Adjust a 0-based IR index expression to a 1-based Luau index.
    In the real backend: `x[i]` becomes `x[i + 1]`.
    This is applied in list_get, list_set, and string indexing. -/
def adjustIndex (idx : LuauExpr) : LuauExpr :=
  .binOp .add idx (.intLit 1)

/-- Emit a 1-based table index access from a 0-based IR index. -/
def emitTableAccess (tbl : LuauExpr) (idx : LuauExpr) : LuauExpr :=
  .index tbl (adjustIndex idx)

-- ======================================================================
-- Section 5: Builtin function mapping
-- ======================================================================

/-- Known IR builtin function names and their Luau equivalents.
    In the real backend (emit_builtin_func in luau.rs), each builtin
    is either mapped to a Luau stdlib call or a prelude helper function.
    List/dict indexing is modeled by emitTableAccess and store/index ops,
    not by separate builtin helper functions. -/
def builtinMapping : List (String × String) :=
  [ ("print", "print"),
    ("len", "molt_len"),
    ("range", "molt_range"),
    ("int", "molt_int"),
    ("float", "tonumber"),
    ("str", "tostring"),
    ("abs", "math.abs"),
    ("min", "math.min"),
    ("max", "math.max"),
    ("type", "molt_type"),
    ("list", "molt_list"),
    ("dict", "molt_dict"),
    ("enumerate", "molt_enumerate"),
    ("zip", "molt_zip"),
    ("reversed", "molt_reversed"),
    ("sorted", "molt_sorted"),
    ("sum", "molt_sum"),
    ("any", "molt_any"),
    ("all", "molt_all"),
    ("isinstance", "molt_isinstance"),
    ("list_append", "molt_list_append"),
    ("dict_get", "molt_dict_get"),
    ("dict_set", "molt_dict_set") ]

/-- Look up the Luau name for an IR builtin. -/
def lookupBuiltin (irName : String) : Option String :=
  (builtinMapping.find? (fun p => p.1 == irName)).map (·.2)

-- ======================================================================
-- Section 6: Instruction and block emission
-- ======================================================================

/-- Emit an IR instruction as Luau statements.
    Each IR instruction `dst := rhs` becomes `local _vN = <expr>`. -/
def emitInstr (names : VarNames) (i : MoltTIR.Instr) : List LuauStmt :=
  [.localDecl (names i.dst) (some (emitExpr names i.rhs))]

/-- Emit a terminator as Luau statements. -/
def emitTerminator (names : VarNames) : MoltTIR.Terminator → List LuauStmt
  | .ret e => [.returnStmt (some (emitExpr names e))]
  | .jmp _ _ => []  -- jumps are resolved to structured control flow
  | .br cond _ _ _ _ =>
      -- Conditional branches are emitted as if/else in the real backend's
      -- structured control flow reconstruction pass. Here we emit a
      -- simplified placeholder that captures the condition evaluation.
      [.ifStmt (emitExpr names cond) [] none]
  | .yield val _ _ =>
      -- Generators emit as return in simplified model
      [.returnStmt (some (emitExpr names val))]
  | .switch scrutinee _ _ =>
      -- Switch emitted as placeholder condition check
      [.ifStmt (emitExpr names scrutinee) [] none]
  | .unreachable =>
      -- Unreachable emits nothing
      []

/-- Emit an IR block as a list of Luau statements. -/
def emitBlock (names : VarNames) (b : MoltTIR.Block) : List LuauStmt :=
  (b.instrs.map (emitInstr names) |>.flatten) ++ emitTerminator names b.term

-- ======================================================================
-- Section 7: Function emission
-- ======================================================================

/-- Emit an IR function as a Luau function.
    Corresponds to emit_function in luau.rs. -/
def emitFunc (names : VarNames) (fname : String) (f : MoltTIR.Func) : LuauFunc :=
  let entryBlock := f.blocks f.entry
  let body := match entryBlock with
    | some b => emitBlock names b
    | none => []
  { name := fname, params := [], body := body }

-- ======================================================================
-- Section 8: Calling convention wrapper
-- ======================================================================

/-- The Molt Luau backend wraps builtin function calls to unpack args tuples.
    For a call `f(args_tuple)`, the backend emits:
      `f(args_tuple[1], args_tuple[2], ...)`
    or for variadic builtins:
      `f(unpack(args_tuple))` / `f(table.unpack(args_tuple))`

    This models the unpack variant used for unknown-arity builtins. -/
def emitUnpackCall (func : LuauExpr) (argsTuple : LuauExpr) : LuauExpr :=
  .call func [.call (.varRef "unpack") [argsTuple]]

/-- Emit a builtin call with explicit argument unpacking from an args tuple.
    Given N known arguments, emit: `f(tup[1], tup[2], ..., tup[N])`. -/
def emitBuiltinCall (func : LuauExpr) (argsTuple : LuauExpr) (arity : Nat) : LuauExpr :=
  let args := List.range arity |>.map fun i => LuauExpr.index argsTuple (.intLit (Int.ofNat (i + 1)))
  .call func args

end MoltTIR.Backend
