/-
  MoltTIR.Backend.RustSemantics -- Rust evaluation model for correctness proofs.

  Defines a small-step evaluator for the Rust AST subset emitted by the Molt
  transpiler. This is the semantic counterpart to RustSyntax.lean: while the syntax
  file defines the target AST structure, this file defines what those AST nodes
  *mean* in terms of values and environment updates.

  Key modeling decisions:
  - Rust integers are i64, matching Python ints within the safe range.
  - 0-based indexing (same as Python, unlike Luau's 1-based).
  - The evaluator is total (returns Option), with None representing stuck states
    (type errors, undefined variables, move-after-use, etc.).
  - Ownership is tracked separately from evaluation: the evaluator operates on
    plain RustValue, and ownership checks are stated as separate properties.
  - Option<T> semantics: Some(v) and None are first-class values.

  The transpiler proof strategy differs from the compiler backend:
  we prove source-to-source equivalence (MoltTIR eval ~ Rust eval),
  then trust rustc for the rest.
-/
import MoltTIR.Backend.RustSyntax
import MoltTIR.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Rust values
-- ======================================================================

/-- Rust runtime values. Mirrors MoltTIR.Value but represents the Rust
    type system.
    - i64 for Python int
    - f64 (modeled as Int for determinism) for Python float
    - bool for Python bool
    - String for Python str
    - unit for Python None
    - option for nullable/optional values
    - vec for Python list
    - tuple for Python tuple -/
inductive RustValue where
  | int (n : Int)            -- i64
  | float (f : Int)          -- f64 (fixed-point model)
  | boolean (b : Bool)       -- bool
  | str (s : String)         -- String
  | unit                     -- ()
  | some (v : RustValue)     -- Some(v)
  | none                     -- None (Option::None)
  | vec (elems : List RustValue)   -- Vec<T>
  | tuple (elems : List RustValue) -- (T1, T2, ...)
  deriving Repr

-- ======================================================================
-- Section 2: Ownership tracking
-- ======================================================================

/-- An owned value that tracks whether it has been moved.
    In Rust, after a move, the original binding is invalid.
    Copy types are never marked as moved (they are implicitly copied). -/
structure OwnedValue where
  value : RustValue
  moved : Bool       -- true if value has been moved out
  deriving Repr

/-- Check if a RustValue is a Copy type (no move semantics needed). -/
def RustValue.isCopy : RustValue → Bool
  | .int _ | .float _ | .boolean _ | .unit => true
  | _ => false

/-- Create a fresh owned value (not yet moved). -/
def OwnedValue.fresh (v : RustValue) : OwnedValue :=
  { value := v, moved := false }

/-- Mark a value as moved. Returns the value and a moved marker. -/
def OwnedValue.moveOut (ov : OwnedValue) : Option (RustValue × OwnedValue) :=
  if ov.moved then
    Option.none  -- use-after-move: stuck
  else if ov.value.isCopy then
    Option.some (ov.value, ov)  -- Copy type: value is copied, original unchanged
  else
    Option.some (ov.value, { ov with moved := true })  -- non-Copy: mark as moved

-- ======================================================================
-- Section 3: Rust environment
-- ======================================================================

/-- Rust environment: maps string variable names to optional values.
    For the evaluation model, we use plain RustValue (not OwnedValue)
    to keep the evaluator simple and compositional. Ownership properties
    are stated separately as theorems over OwnedValue environments. -/
abbrev RustEnv := String → Option RustValue

namespace RustEnv

def empty : RustEnv := fun _ => Option.none

def set (env : RustEnv) (name : String) (v : RustValue) : RustEnv :=
  fun n => if n = name then Option.some v else env n

def get (env : RustEnv) (name : String) : Option RustValue :=
  env name

theorem set_eq (env : RustEnv) (name : String) (v : RustValue) :
    (env.set name v) name = Option.some v := by
  simp [set]

theorem set_ne (env : RustEnv) (name other : String) (v : RustValue) (h : other ≠ name) :
    (env.set name v) other = env other := by
  simp [set, h]

end RustEnv

-- ======================================================================
-- Section 4: Value correspondence
-- ======================================================================

/-- Convert a MoltTIR value to the corresponding Rust value.
    This defines the semantic correspondence between the source and target
    value domains. -/
def valueToRust : MoltTIR.Value → RustValue
  | .int n   => .int n
  | .float f => .float f       -- Rust separates int and float (unlike Luau)
  | .bool b  => .boolean b
  | .str s   => .str s
  | .none    => .none          -- Python None -> Option::None

/-- Convert a Rust value back to MoltTIR value (partial inverse).
    Vec, tuple, Some have no direct MoltTIR.Value counterpart at the
    expression level, so they map to Option.none. -/
def rustToValue : RustValue → Option MoltTIR.Value
  | .int n     => Option.some (.int n)
  | .float f   => Option.some (.float f)
  | .boolean b => Option.some (.bool b)
  | .str s     => Option.some (.str s)
  | .unit      => Option.some .none
  | .none      => Option.some .none      -- Option::None -> Python None
  | .some _    => Option.none            -- no direct correspondence
  | .vec _     => Option.none            -- no direct correspondence
  | .tuple _   => Option.none            -- no direct correspondence

/-- valueToRust and rustToValue form a round-trip for scalar values.
    Unlike Luau (which unifies int/float as number), Rust preserves the
    int/float distinction, so the round-trip is exact for all scalars. -/
theorem rustToValue_valueToRust_int (n : Int) :
    rustToValue (valueToRust (.int n)) = Option.some (.int n) := by rfl

theorem rustToValue_valueToRust_float (f : Int) :
    rustToValue (valueToRust (.float f)) = Option.some (.float f) := by rfl

theorem rustToValue_valueToRust_bool (b : Bool) :
    rustToValue (valueToRust (.bool b)) = Option.some (.bool b) := by rfl

theorem rustToValue_valueToRust_str (s : String) :
    rustToValue (valueToRust (.str s)) = Option.some (.str s) := by rfl

theorem rustToValue_valueToRust_none :
    rustToValue (valueToRust .none) = Option.some .none := by rfl

-- ======================================================================
-- Section 5: Rust binary and unary operator evaluation
-- ======================================================================

/-- Evaluate a Rust binary operator on two Rust values.
    For the integer subset, Rust arithmetic matches Python exactly
    (within the i64 range). -/
def evalRustBinOp (op : RustBinOp) (a b : RustValue) : Option RustValue :=
  match op, a, b with
  -- arithmetic (int x int -> int)
  | .add, .int x, .int y => Option.some (.int (x + y))
  | .sub, .int x, .int y => Option.some (.int (x - y))
  | .mul, .int x, .int y => Option.some (.int (x * y))
  | .div, .int x, .int y => if y == 0 then Option.none else Option.some (.int (x / y))
  | .rem, .int x, .int y => if y == 0 then Option.none else Option.some (.int (x % y))
  -- comparison (int x int -> bool)
  | .eq,  .int x, .int y => Option.some (.boolean (x == y))
  | .ne,  .int x, .int y => Option.some (.boolean (x != y))
  | .lt,  .int x, .int y => Option.some (.boolean (x < y))
  | .le,  .int x, .int y => Option.some (.boolean (x ≤ y))
  | .gt,  .int x, .int y => Option.some (.boolean (x > y))
  | .ge,  .int x, .int y => Option.some (.boolean (x ≥ y))
  -- comparison (bool x bool -> bool)
  | .eq,  .boolean x, .boolean y => Option.some (.boolean (x == y))
  | .ne,  .boolean x, .boolean y => Option.some (.boolean (x != y))
  -- exponentiation (int x int -> int, non-negative exponent only)
  | .pow, .int x, .int y =>
      if y < 0 then Option.none
      else Option.some (.int (x ^ y.toNat))
  -- floor division (Python // semantics: round toward negative infinity)
  -- For positive divisor: same as truncating div when dividend >= 0,
  -- otherwise adjusts by -1 when there is a remainder.
  | .floordiv, .int x, .int y =>
      if y == 0 then Option.none
      else
        let q := x / y
        let r := x % y
        -- Python floor division: if remainder is nonzero and signs differ, subtract 1
        if r != 0 && ((r < 0) != (y < 0)) then Option.some (.int (q - 1))
        else Option.some (.int q)
  -- bitwise (int x int -> int)
  | .bitAnd, .int x, .int y => Option.some (.int (Int.land x y))
  | .bitOr,  .int x, .int y => Option.some (.int (Int.lor x y))
  | .bitXor, .int x, .int y => Option.some (.int (Int.xor x y))
  -- shift (int x int -> int, non-negative shift amount)
  | .shl, .int x, .int y =>
      if y < 0 then Option.none
      else Option.some (.int (Int.shiftLeft x y.toNat))
  | .shr, .int x, .int y =>
      if y < 0 then Option.none
      else Option.some (.int (Int.shiftRight x y.toNat))
  -- logical (bool x bool -> bool)
  | .and, .boolean x, .boolean y => Option.some (.boolean (x && y))
  | .or,  .boolean x, .boolean y => Option.some (.boolean (x || y))
  -- catch-all for type mismatches and unmodeled ops
  | _, _, _ => Option.none

/-- Evaluate a Rust unary operator. -/
def evalRustUnOp (op : RustUnOp) (a : RustValue) : Option RustValue :=
  match op, a with
  | .neg, .int x    => Option.some (.int (-x))
  | .not, .boolean x => Option.some (.boolean (!x))
  | .abs, .int x    => Option.some (.int (if x < 0 then -x else x))
  | _, _ => Option.none

-- ======================================================================
-- Section 6: Rust expression evaluator
-- ======================================================================

/-- Evaluate a Rust expression in a Rust environment.
    Total, deterministic. Returns none on undefined variables, type errors, etc. -/
def evalRustExpr (env : RustEnv) : RustExpr → Option RustValue
  | .intLit n     => Option.some (.int n)
  | .floatLit f   => Option.some (.float f)
  | .strLit s     => Option.some (.str s)
  | .boolLit b    => Option.some (.boolean b)
  | .unitLit      => Option.some .unit
  | .noneExpr     => Option.some .none
  | .varRef name  => env name
  | .binOp op l r =>
      match evalRustExpr env l, evalRustExpr env r with
      | Option.some vl, Option.some vr => evalRustBinOp op vl vr
      | _, _ => Option.none
  | .unOp op arg =>
      match evalRustExpr env arg with
      | Option.some va => evalRustUnOp op va
      | Option.none => Option.none
  | .someExpr inner =>
      match evalRustExpr env inner with
      | Option.some v => Option.some (.some v)
      | Option.none => Option.none
  -- Unmodeled expression forms (require list recursion or call semantics)
  | .tupleExpr _       => Option.none  -- not modeled (requires list recursion)
  | .indexOp _ _       => Option.none  -- not modeled (requires container semantics)
  | .methodCall _ _ _  => Option.none
  | .fieldAccess _ _   => Option.none
  | .closureExpr _ _   => Option.none
  | .refExpr _ _       => Option.none
  | .derefExpr _       => Option.none
  | .callExpr _ _      => Option.none
  | .macroCall _ _     => Option.none

-- ======================================================================
-- Section 7: Rust statement executor
-- ======================================================================

/-- Execute a single Rust statement, producing an updated environment.
    Only models the subset needed for instruction emission proofs:
    letBinding and assign. -/
def execRustStmt (env : RustEnv) : RustStmt → Option RustEnv
  | .letBinding name _mutable _ty (Option.some init) =>
      match evalRustExpr env init with
      | Option.some v => Option.some (env.set name v)
      | Option.none => Option.none
  | .letBinding name _mutable _ty Option.none =>
      Option.some (env.set name .unit)
  | .assign (.varRef name) val =>
      match evalRustExpr env val with
      | Option.some v => Option.some (env.set name v)
      | Option.none => Option.none
  | _ => Option.none  -- other statement forms not modeled for basic proofs

/-- Execute a list of Rust statements sequentially. -/
def execRustStmts (env : RustEnv) : List RustStmt → Option RustEnv
  | [] => Option.some env
  | s :: ss =>
      match execRustStmt env s with
      | Option.some env' => execRustStmts env' ss
      | Option.none => Option.none

-- ======================================================================
-- Section 8: Evaluator properties
-- ======================================================================

/-- Evaluating a Rust integer literal always succeeds. -/
theorem evalRustExpr_intLit (env : RustEnv) (n : Int) :
    evalRustExpr env (.intLit n) = Option.some (.int n) := by rfl

theorem evalRustExpr_boolLit (env : RustEnv) (b : Bool) :
    evalRustExpr env (.boolLit b) = Option.some (.boolean b) := by rfl

theorem evalRustExpr_strLit (env : RustEnv) (s : String) :
    evalRustExpr env (.strLit s) = Option.some (.str s) := by rfl

theorem evalRustExpr_unitLit (env : RustEnv) :
    evalRustExpr env .unitLit = Option.some .unit := by rfl

theorem evalRustExpr_noneLit (env : RustEnv) :
    evalRustExpr env .noneExpr = Option.some .none := by rfl

/-- Executing a letBinding with an integer literal always succeeds
    and extends the environment. -/
theorem execRustStmt_let_intLit (env : RustEnv) (name : String) (n : Int) :
    execRustStmt env (.letBinding name false Option.none (Option.some (.intLit n))) =
      Option.some (env.set name (.int n)) := by
  rfl

/-- Executing an empty statement list is the identity. -/
theorem execRustStmts_nil (env : RustEnv) :
    execRustStmts env [] = Option.some env := by rfl

/-- Executing a singleton list delegates to execRustStmt. -/
theorem execRustStmts_singleton (env : RustEnv) (s : RustStmt) :
    execRustStmts env [s] = execRustStmt env s := by
  simp [execRustStmts]
  cases execRustStmt env s with
  | none => rfl
  | some env' => simp [execRustStmts]

-- ======================================================================
-- Section 9: Ownership safety properties
-- ======================================================================

/-- A fresh value is not moved. -/
theorem fresh_not_moved (v : RustValue) : (OwnedValue.fresh v).moved = false := by
  rfl

/-- Moving a non-Copy value marks it as moved. -/
theorem moveOut_marks_moved (v : RustValue) (hNotCopy : v.isCopy = false) :
    OwnedValue.moveOut (.fresh v) = Option.some (v, { value := v, moved := true }) := by
  simp [OwnedValue.moveOut, OwnedValue.fresh, hNotCopy]

/-- Moving a Copy value does not mark it as moved. -/
theorem moveOut_copy_unchanged (v : RustValue) (hCopy : v.isCopy = true) :
    OwnedValue.moveOut (.fresh v) = Option.some (v, .fresh v) := by
  simp [OwnedValue.moveOut, OwnedValue.fresh, hCopy]

/-- A moved value cannot be moved again (use-after-move). -/
theorem moveOut_moved_stuck (ov : OwnedValue) (hMoved : ov.moved = true) :
    OwnedValue.moveOut ov = Option.none := by
  simp [OwnedValue.moveOut, hMoved]

end MoltTIR.Backend
