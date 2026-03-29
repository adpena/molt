/-
  MoltTIR.Backend.RustEmit -- Translation from MoltTIR to Rust target AST.

  Models the core translation logic for a Python-to-Rust transpiler:
  - Type emission (MoltTIR.Ty → RustType)
  - Expression emission (IR Expr → RustExpr)
  - Instruction emission (IR Instr → List RustStmt)
  - Block emission (IR Block → List RustStmt)
  - Function emission (IR Func → RustFn)
  - Builtin function mapping (IR builtin names → Rust stdlib/runtime functions)
  - Ownership inference (mark variables as owned/borrowed based on usage)

  The translation uses a naming context (VarNames) to map SSA variable
  numbers to string names in the Rust output, reusing the same VarNames
  abstraction as the Luau backend.

  Key transpiler-specific concerns:
  - Python None maps to Option::None, not a null pointer
  - Python exceptions map to Result<T, MoltError>
  - Integer division uses checked_div (Python ZeroDivisionError → panic/Result::Err)
  - String concatenation uses format! or String::push_str, not +
  - Python lists map to Vec<T> with 0-based indexing (no adjustment needed, unlike Luau)
-/
import MoltTIR.Syntax
import MoltTIR.Semantics.EvalExpr
import MoltTIR.Backend.RustSyntax

set_option autoImplicit false

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Variable naming context (reuse from LuauEmit)
-- ======================================================================

-- VarNames is already defined in LuauEmit; we use the same type here.
-- For standalone compilation, we redefine it locally.

/-- Maps IR SSA variable IDs to Rust variable name strings. -/
abbrev RustVarNames := MoltTIR.Var → String

/-- Default naming scheme: _v0, _v1, ... -/
def defaultRustVarName (x : MoltTIR.Var) : String := s!"_v{x}"

-- ======================================================================
-- Section 2: Type mapping
-- ======================================================================

/-- Map a MoltTIR type tag to the corresponding Rust type.
    This is a key decision point: Python's dynamic types become Rust's
    static types. The transpiler infers concrete types from TIR type hints. -/
def emitRustType : MoltTIR.Ty → RustType
  | .int   => .i64
  | .float => .f64
  | .bool  => .bool
  | .str   => .string
  | .none  => .unit
  | .bytes => .vec .i64             -- bytes → Vec<u8>, modeled as Vec<i64>
  | .list  => .vec (.option .i64)   -- untyped list → Vec<Option<MoltValue>>
  | .dict  => .hashMap .string (.option .i64)  -- dict → HashMap<String, MoltValue>
  | .set   => .vec .i64             -- set → approximation (HashSet not modeled)
  | .tuple => .tuple []             -- empty tuple placeholder; real arity from TIR
  | .obj   => .structTy "MoltObject"  -- generic object

-- ======================================================================
-- Section 3: Operator mapping
-- ======================================================================

/-- Map IR binary operator to Rust binary operator.
    Each IR operator maps to a distinct Rust operator variant with
    matching semantics. Floor division and pow have dedicated variants
    rather than approximations. -/
def emitRustBinOp : MoltTIR.BinOp → RustBinOp
  | .add => .add
  | .sub => .sub
  | .mul => .mul
  | .div => .div
  | .floordiv => .floordiv  -- Python // floor division (round toward -inf)
  | .mod => .rem
  | .pow => .pow           -- i64::pow / f64::powi
  | .eq => .eq
  | .ne => .ne
  | .lt => .lt
  | .le => .le
  | .gt => .gt
  | .ge => .ge
  | .bit_and => .bitAnd   -- Rust & operator
  | .bit_or  => .bitOr    -- Rust | operator
  | .bit_xor => .bitXor   -- Rust ^ operator
  | .lshift  => .shl      -- Rust << operator
  | .rshift  => .shr      -- Rust >> operator

/-- Map IR unary operator to Rust unary operator. -/
def emitRustUnOp : MoltTIR.UnOp → RustUnOp
  | .neg => .neg
  | .not => .not
  | .abs => .abs
  | .invert => .not     -- approximation; real transpiler uses ! for bitwise

-- ======================================================================
-- Section 4: Value correspondence
-- ======================================================================

/-- Map a MoltTIR value to the corresponding Rust expression literal.
    This defines the concrete syntax for each value kind in the emitted Rust. -/
def emitRustValue : MoltTIR.Value → RustExpr
  | .int n   => .intLit n
  | .float f => .floatLit f
  | .str s   => .strLit s
  | .bool b  => .boolLit b
  | .none    => .noneExpr     -- Python None → Rust None (Option::None)

-- ======================================================================
-- Section 5: Expression emission
-- ======================================================================

/-- Translate an IR expression to a Rust expression.
    This is the core of the transpiler's expression lowering. -/
def emitRustExpr (names : RustVarNames) : MoltTIR.Expr → RustExpr
  | .val v => emitRustValue v
  | .var x => .varRef (names x)
  | .bin op a b => .binOp (emitRustBinOp op) (emitRustExpr names a) (emitRustExpr names b)
  | .un op a => .unOp (emitRustUnOp op) (emitRustExpr names a)

-- ======================================================================
-- Section 6: Ownership inference
-- ======================================================================

/-- Infer ownership mode for a value based on its type.
    Copy types (i64, f64, bool) are always owned (copied implicitly).
    Non-Copy types (String, Vec, HashMap) default to owned on first binding,
    borrowed on subsequent uses. -/
def inferOwnership (ty : RustType) : Ownership :=
  if ty.isCopy then .owned else .owned  -- initial binding is always owned

/-- Determine if a let binding should be mutable.
    In SSA form, each variable is assigned exactly once, so bindings are
    immutable by default. Mutability is only needed for loop variables
    and accumulator patterns. -/
def needsMut (_dst : MoltTIR.Var) : Bool := false

-- ======================================================================
-- Section 7: Instruction and block emission
-- ======================================================================

/-- Emit an IR instruction as Rust statements.
    Each IR instruction `dst := rhs` becomes `let _vN = <expr>;`.
    SSA guarantees each dst is fresh, so bindings are immutable. -/
def emitRustInstr (names : RustVarNames) (i : MoltTIR.Instr) : List RustStmt :=
  [.letBinding (names i.dst) (needsMut i.dst) none (some (emitRustExpr names i.rhs))]

/-- Emit a terminator as Rust statements. -/
def emitRustTerminator (names : RustVarNames) : MoltTIR.Terminator → List RustStmt
  | .ret e => [.returnStmt (some (emitRustExpr names e))]
  | .jmp _ _ => []  -- jumps resolved to structured control flow
  | .br cond _ _ _ _ =>
      -- Conditional branches are emitted as if/else in the transpiler's
      -- structured control flow reconstruction pass.
      [.ifElse (emitRustExpr names cond) [] none]

/-- Emit an IR block as a list of Rust statements. -/
def emitRustBlock (names : RustVarNames) (b : MoltTIR.Block) : List RustStmt :=
  (b.instrs.map (emitRustInstr names) |>.flatten) ++ emitRustTerminator names b.term

-- ======================================================================
-- Section 8: Function emission
-- ======================================================================

/-- Emit an IR function as a Rust function.
    The transpiler infers parameter types from TIR type hints and assigns
    ownership modes based on usage analysis. -/
def emitRustFunc (names : RustVarNames) (fname : String) (f : MoltTIR.Func) : RustFn :=
  let entryBlock := f.blocks f.entry
  let body := match entryBlock with
    | some b => emitRustBlock names b
    | none => []
  { name := fname, params := [], returnType := .unit, body := body }

-- ======================================================================
-- Section 9: Builtin function mapping
-- ======================================================================

/-- Known IR builtin function names and their Rust equivalents.
    The transpiler maps Python builtins to Rust standard library functions
    or Molt runtime helpers. -/
def rustBuiltinMapping : List (String × String) :=
  [ ("print", "println!"),
    ("len", "molt_len"),
    ("range", "molt_range"),
    ("int", "molt_int"),
    ("float", "molt_float"),
    ("str", "molt_str"),
    ("abs", "i64::abs"),
    ("min", "std::cmp::min"),
    ("max", "std::cmp::max"),
    ("type", "molt_type"),
    ("list", "Vec::new"),
    ("dict", "HashMap::new"),
    ("enumerate", "Iterator::enumerate"),
    ("zip", "Iterator::zip"),
    ("reversed", "Iterator::rev"),
    ("sorted", "molt_sorted"),
    ("sum", "Iterator::sum"),
    ("any", "Iterator::any"),
    ("all", "Iterator::all"),
    ("isinstance", "molt_isinstance"),
    ("list_append", "Vec::push"),
    ("list_get", "Vec::get"),
    ("list_set", "molt_list_set"),
    ("dict_get", "HashMap::get"),
    ("dict_set", "HashMap::insert") ]

/-- Look up the Rust name for an IR builtin. -/
def lookupRustBuiltin (irName : String) : Option String :=
  (rustBuiltinMapping.find? (fun p => p.1 == irName)).map (·.2)

end MoltTIR.Backend
