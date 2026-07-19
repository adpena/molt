/-
  MoltTIR.Backend.LuauSemantics -- Luau evaluation model for correctness proofs.

  Defines a small-step evaluator for the Luau AST subset emitted by the Molt
  backend. This is the semantic counterpart to LuauSyntax.lean: while the syntax
  file defines the target AST structure, this file defines what those AST nodes
  *mean* in terms of values and environment updates.

  Key modeling decisions:
  - Luau numbers are IEEE 754 doubles, but for Molt's integer subset they behave
    identically to Python ints. We model them as Int, matching the MoltTIR.Value
    representation. This is sound because Molt only compiles programs that use
    integers within the safe-integer range (|n| < 2^53).
  - Table indexing is 1-based (Luau convention).
  - The evaluator is total (returns Option), with None representing stuck states
    (type errors, undefined variables, etc.).
-/
import MoltTIR.Backend.LuauSyntax
import MoltTIR.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Luau values
-- ======================================================================

/-- Luau runtime values. Mirrors MoltTIR.Value but represents the Luau
    type system: all numbers are doubles (modeled as Int for the integer subset),
    strings, booleans, nil, and tables. -/
inductive LuauValue where
  | number (n : Int)       -- Python int carried by a Luau number
  | float (f : Int)        -- Python float carried by a Luau number
  | boolean (b : Bool)     -- Luau boolean
  | str (s : String)       -- Luau string
  | nil                    -- Luau nil
  | table (entries : List (Int × LuauValue))  -- array-like table (1-based keys)
  deriving Repr

/-- Identity for the scalar carrier subset where provenance is sufficient.
    Equal number/float/string payloads do not establish Python object identity;
    reference identity is outside this pure expression model. -/
def luauScalarEq : LuauValue → LuauValue → Option Bool
  | .boolean x, .boolean y => some (x == y)
  | .nil, .nil => some true
  | .number _, .number _ | .float _, .float _ | .str _, .str _ => none
  | .table _, .table _ => none
  | _, _ => some false

-- ======================================================================
-- Section 2: Luau environment
-- ======================================================================

/-- Luau environment: maps string variable names to optional values.
    Luau uses lexical scoping with string identifiers (unlike IR's Nat SSA vars). -/
abbrev LuauEnv := String → Option LuauValue

namespace LuauEnv

def empty : LuauEnv := fun _ => none

def set (env : LuauEnv) (name : String) (v : LuauValue) : LuauEnv :=
  fun n => if n = name then some v else env n

theorem set_eq (env : LuauEnv) (name : String) (v : LuauValue) :
    (env.set name v) name = some v := by
  simp [set]

theorem set_ne (env : LuauEnv) (name other : String) (v : LuauValue) (h : other ≠ name) :
    (env.set name v) other = env other := by
  simp [set, h]

end LuauEnv

-- ======================================================================
-- Section 3: Value correspondence
-- ======================================================================

/-- Convert a MoltTIR value to the corresponding Luau value.
    This defines the semantic correspondence between the source and target
    value domains. -/
def valueToLuau : MoltTIR.Value → LuauValue
  | .int n   => .number n
  | .float f => .float f
  | .bool b  => .boolean b
  | .str s   => .str s
  | .none    => .nil

/-- The target scalar equality authority exactly preserves TIR structural
    identity, including the distinction between int and float carriers. -/
@[simp] theorem luauScalarEq_valueToLuau (x y : MoltTIR.Value) :
    luauScalarEq (valueToLuau x) (valueToLuau y) =
      MoltTIR.scalarIdentityEq x y := by
  cases x <;> cases y <;> rfl

/-- Convert a Luau value back to MoltTIR value (partial inverse).
    Tables have no MoltTIR.Value counterpart at the expression level,
    so they map to none. -/
def luauToValue : LuauValue → Option MoltTIR.Value
  | .number n  => some (.int n)
  | .float f   => some (.float f)
  | .boolean b => some (.bool b)
  | .str s     => some (.str s)
  | .nil       => some .none
  | .table _   => none

/-- `valueToLuau` and `luauToValue` preserve the source scalar kind. The
    semantic model retains int/float provenance even though both lower through
    Luau's numeric carrier, because Python identity distinguishes them. -/
theorem luauToValue_valueToLuau_int (n : Int) :
    luauToValue (valueToLuau (.int n)) = some (.int n) := by rfl

theorem luauToValue_valueToLuau_bool (b : Bool) :
    luauToValue (valueToLuau (.bool b)) = some (.bool b) := by rfl

theorem luauToValue_valueToLuau_str (s : String) :
    luauToValue (valueToLuau (.str s)) = some (.str s) := by rfl

theorem luauToValue_valueToLuau_none :
    luauToValue (valueToLuau .none) = some .none := by rfl

theorem luauToValue_valueToLuau_float (f : Int) :
    luauToValue (valueToLuau (.float f)) = some (.float f) := by rfl

-- ======================================================================
-- Section 4: Luau binary and unary operator evaluation
-- ======================================================================

/-- Evaluate a Luau binary operator on two Luau values.
    For the integer subset, Luau arithmetic matches Python exactly. -/
def evalLuauBinOp (op : LuauBinOp) (a b : LuauValue) : Option LuauValue :=
  match op, a, b with
  -- arithmetic (number × number → number)
  | .add,  .number x, .number y => some (.number (x + y))
  | .sub,  .number x, .number y => some (.number (x - y))
  | .mul,  .number x, .number y => some (.number (x * y))
  | .add,  .number x, .float y => some (.float (x + y))
  | .sub,  .number x, .float y => some (.float (x - y))
  | .mul,  .number x, .float y => some (.float (x * y))
  | .add,  .float x, .number y => some (.float (x + y))
  | .sub,  .float x, .number y => some (.float (x - y))
  | .mul,  .float x, .number y => some (.float (x * y))
  | .add,  .float x, .float y => some (.float (x + y))
  | .sub,  .float x, .float y => some (.float (x - y))
  | .mul,  .float x, .float y => some (.float (x * y))
  | .mod,  .number x, .number y => if y == 0 then none else some (.number (x % y))
  | .idiv, .number x, .number y => if y == 0 then none else some (.number (x / y))
  | .pow,  .number x, .number y =>
      if y < 0 then none
      else some (.number (x ^ y.toNat))
  | .add,  .str x, .str y => some (.str (x ++ y))
  | .mul,  .str s, .number n =>
      if n ≤ 0 then some (.str "")
      else some (.str (String.join (List.replicate n.toNat s)))
  | .mul,  .number n, .str s =>
      if n ≤ 0 then some (.str "")
      else some (.str (String.join (List.replicate n.toNat s)))
  -- value equality and ordered comparison (verified scalar subset)
  | .eq,   .number x, .number y => some (.boolean (x == y))
  | .ne,   .number x, .number y => some (.boolean (x != y))
  | .eq,   .boolean x, .boolean y => some (.boolean (x == y))
  | .ne,   .boolean x, .boolean y => some (.boolean (x != y))
  | .lt,   .number x, .number y => some (.boolean (x < y))
  | .le,   .number x, .number y => some (.boolean (x ≤ y))
  | .gt,   .number x, .number y => some (.boolean (x > y))
  | .ge,   .number x, .number y => some (.boolean (x ≥ y))
  | .eq,   .number x, .float y => some (.boolean (x == y))
  | .ne,   .number x, .float y => some (.boolean (x != y))
  | .lt,   .number x, .float y => some (.boolean (x < y))
  | .le,   .number x, .float y => some (.boolean (x ≤ y))
  | .gt,   .number x, .float y => some (.boolean (x > y))
  | .ge,   .number x, .float y => some (.boolean (x ≥ y))
  | .eq,   .float x, .number y => some (.boolean (x == y))
  | .ne,   .float x, .number y => some (.boolean (x != y))
  | .lt,   .float x, .number y => some (.boolean (x < y))
  | .le,   .float x, .number y => some (.boolean (x ≤ y))
  | .gt,   .float x, .number y => some (.boolean (x > y))
  | .ge,   .float x, .number y => some (.boolean (x ≥ y))
  | .eq,   .float x, .float y => some (.boolean (x == y))
  | .ne,   .float x, .float y => some (.boolean (x != y))
  | .lt,   .float x, .float y => some (.boolean (x < y))
  | .le,   .float x, .float y => some (.boolean (x ≤ y))
  | .gt,   .float x, .float y => some (.boolean (x > y))
  | .ge,   .float x, .float y => some (.boolean (x ≥ y))
  -- Identity is structural over source scalar kinds. In particular an int and
  -- float with the same numeric payload are not identical.
  | .rawequal, x, y => (luauScalarEq x y).map .boolean
  | .not_rawequal, x, y => (luauScalarEq x y).map (fun same => .boolean (!same))
  -- operand-returning Boolean operators
  | .land, .boolean false, _ => some (.boolean false)
  | .land, .boolean true, y => some y
  | .lor, .boolean true, _ => some (.boolean true)
  | .lor, .boolean false, y => some y
  -- string concatenation
  | .concat, .str x, .str y => some (.str (x ++ y))
  -- bitwise (models bit32.band/bor/bxor/lshift/rshift on integers)
  -- Note: Int lacks bitwise instances in Lean 4; we model these via Nat conversion.
  | .band, .number x, .number y => some (.number (Int.ofNat (x.toNat &&& y.toNat)))
  | .bor,  .number x, .number y => some (.number (Int.ofNat (x.toNat ||| y.toNat)))
  | .bxor, .number x, .number y => some (.number (Int.ofNat (x.toNat ^^^ y.toNat)))
  | .lshl, .number x, .number y =>
      if y < 0 then none else some (.number (Int.ofNat (x.toNat <<< y.toNat)))
  | .lshr, .number x, .number y =>
      if y < 0 then none else some (.number (Int.ofNat (x.toNat >>> y.toNat)))
  -- catch-all for type mismatches and unmodeled ops
  | _, _, _ => none

/-- Evaluate a Luau unary operator. -/
def evalLuauUnOp (op : LuauUnOp) (a : LuauValue) : Option LuauValue :=
  match op, a with
  | .neg,  .number x  => some (.number (-x))
  | .neg,  .float x   => some (.float (-x))
  | .pos,  .number x  => some (.number x)
  | .pos,  .float x   => some (.float x)
  | .lnot, .boolean x => some (.boolean (!x))
  | .lnot, .number x  => some (.boolean (x == 0))     -- truthy: 0 is falsy
  | .lnot, .float x   => some (.boolean (x == 0))
  | .lnot, .str s     => some (.boolean (s == ""))     -- truthy: "" is falsy
  | .lnot, .nil       => some (.boolean true)          -- truthy: nil is falsy
  | .abs,  .number x  => some (.number (if x < 0 then -x else x))
  | _, _ => none

-- ======================================================================
-- Section 5: Luau expression evaluator
-- ======================================================================

/-- Evaluate a Luau expression in a Luau environment.
    Total, deterministic. Returns none on undefined variables, type errors, etc. -/
def evalLuauExpr (env : LuauEnv) : LuauExpr → Option LuauValue
  | .intLit n     => some (.number n)
  | .floatLit f   => some (.float f)
  | .strLit s     => some (.str s)
  | .boolLit b    => some (.boolean b)
  | .nil          => some .nil
  | .varRef name  => env name
  | .binOp op l r =>
      match evalLuauExpr env l, evalLuauExpr env r with
      | some vl, some vr => evalLuauBinOp op vl vr
      | _, _ => none
  | .unOp op arg =>
      match evalLuauExpr env arg with
      | some va => evalLuauUnOp op va
      | none => none
  | .index tbl key =>
      match evalLuauExpr env tbl, evalLuauExpr env key with
      | some (.table entries), some (.number k) =>
          (entries.find? (fun p => p.1 == k)).map (·.2)
      | _, _ => none
  | .dotIndex _tbl _field => none  -- not modeled (requires record semantics)
  | .call _func _args => none      -- not modeled (requires call semantics)
  | .methodCall _ _ _ => none       -- not modeled
  | .tableCtor _fields => none      -- not modeled (requires table construction)

-- ======================================================================
-- Section 6: Luau statement executor
-- ======================================================================

/-- Execute a single Luau statement, producing an updated environment.
    Only models the subset needed for instruction emission proofs:
    localDecl and assign. -/
def execLuauStmt (env : LuauEnv) : LuauStmt → Option LuauEnv
  | .localDecl name (some init) =>
      match evalLuauExpr env init with
      | some v => some (env.set name v)
      | none => none
  | .localDecl name none =>
      some (env.set name .nil)
  | .assign (.varRef name) val =>
      match evalLuauExpr env val with
      | some v => some (env.set name v)
      | none => none
  | _ => none  -- other statement forms not modeled for basic proofs

/-- Execute a list of Luau statements sequentially. -/
def execLuauStmts (env : LuauEnv) : List LuauStmt → Option LuauEnv
  | [] => some env
  | s :: ss =>
      match execLuauStmt env s with
      | some env' => execLuauStmts env' ss
      | none => none

-- ======================================================================
-- Section 7: Evaluator properties
-- ======================================================================

/-- Evaluating a Luau literal always succeeds. -/
theorem evalLuauExpr_intLit (env : LuauEnv) (n : Int) :
    evalLuauExpr env (.intLit n) = some (.number n) := by rfl

theorem evalLuauExpr_boolLit (env : LuauEnv) (b : Bool) :
    evalLuauExpr env (.boolLit b) = some (.boolean b) := by rfl

theorem evalLuauExpr_strLit (env : LuauEnv) (s : String) :
    evalLuauExpr env (.strLit s) = some (.str s) := by rfl

theorem evalLuauExpr_nil (env : LuauEnv) :
    evalLuauExpr env .nil = some .nil := by rfl

/-- Executing a localDecl with a literal initializer always succeeds
    and extends the environment. -/
theorem execLuauStmt_localDecl_intLit (env : LuauEnv) (name : String) (n : Int) :
    execLuauStmt env (.localDecl name (some (.intLit n))) =
      some (env.set name (.number n)) := by
  rfl

/-- Executing an empty statement list is the identity. -/
theorem execLuauStmts_nil (env : LuauEnv) :
    execLuauStmts env [] = some env := by rfl

/-- Executing a singleton list delegates to execLuauStmt. -/
theorem execLuauStmts_singleton (env : LuauEnv) (s : LuauStmt) :
    execLuauStmts env [s] = execLuauStmt env s := by
  simp [execLuauStmts]
  cases execLuauStmt env s with
  | none => rfl
  | some env' => rfl

end MoltTIR.Backend
