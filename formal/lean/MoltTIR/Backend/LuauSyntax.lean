/-
  MoltTIR.Backend.LuauSyntax -- Luau target AST for the Molt transpiler backend.

  Models the subset of Luau syntax that the Molt backend emits
  (runtime/molt-backend/src/luau.rs, ~4300 lines). This is a structured
  representation of emitted Luau source, used as the target language
  in the translation correctness proofs.

  Key Luau-specific modeling decisions:
  - 1-based indexing (unlike IR which is 0-based)
  - Table constructors model both list-style {a, b, c} and record-style {x=1}
  - Method calls model the colon syntax (obj:method(args))
  - No explicit statement separators (Luau uses newlines/semicolons optionally)

  Note: LuauExpr and LuauStmt are mutually recursive (function expressions
  contain statement bodies, statements contain expressions). We avoid Lean4's
  mutual inductive limitations by dropping inline function expressions from
  LuauExpr — the backend emits these as localFunc statements instead.
-/
import MoltTIR.Types

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Luau binary and unary operators
-- ======================================================================

/-- Luau binary operators. Subset that the Molt backend actually emits. -/
inductive LuauBinOp where
  -- arithmetic
  | add | sub | mul | div | idiv | mod | pow
  -- comparison
  | eq | ne | lt | le | gt | ge
  -- logical
  | land | lor
  -- bitwise (models bit32.band/bor/bxor/lshift/rshift library calls)
  | band | bor | bxor | lshl | lshr
  -- string
  | concat
  deriving DecidableEq, Repr

/-- Luau unary operators. -/
inductive LuauUnOp where
  | neg     -- arithmetic negation (-)
  | lnot    -- logical negation (not)
  | len     -- length operator (#)
  | abs     -- absolute value (math.abs)
  deriving DecidableEq, Repr

-- ======================================================================
-- Section 2: Luau expressions (non-recursive with statements)
-- ======================================================================

/-- Luau expression AST. Models the fragment of Luau that the backend emits.
    Note: inline function expressions (funcExpr) are omitted to avoid
    mutual recursion with LuauStmt. The backend emits these as localFunc
    statements, which is the more common pattern in emitted Luau code. -/
inductive LuauExpr where
  | intLit (n : Int)
  | floatLit (f : Int)           -- modeled as fixed-point, matching IR convention
  | strLit (s : String)
  | boolLit (b : Bool)
  | nil
  | varRef (name : String)
  | binOp (op : LuauBinOp) (lhs rhs : LuauExpr)
  | unOp (op : LuauUnOp) (arg : LuauExpr)
  | call (func : LuauExpr) (args : List LuauExpr)
  | methodCall (obj : LuauExpr) (method : String) (args : List LuauExpr)
  | index (tbl : LuauExpr) (key : LuauExpr)
  | dotIndex (tbl : LuauExpr) (field : String)
  | tableCtor (fields : List (Option String × LuauExpr))
  deriving Repr

-- ======================================================================
-- Section 3: Luau statements
-- ======================================================================

/-- Luau statement AST. References LuauExpr but not vice versa,
    avoiding mutual inductive recursion. -/
inductive LuauStmt where
  | localDecl (name : String) (init : Option LuauExpr)
  | assign (target : LuauExpr) (val : LuauExpr)
  | ifStmt (cond : LuauExpr)
           (thenBody : List LuauStmt)
           (elseBody : Option (List LuauStmt))
  | forNumeric (var_ : String) (start stop step : LuauExpr) (body : List LuauStmt)
  | forIn (vars : List String) (iter : LuauExpr) (body : List LuauStmt)
  | whileLoop (cond : LuauExpr) (body : List LuauStmt)
  | returnStmt (val : Option LuauExpr)
  | exprStmt (e : LuauExpr)
  | localFunc (name : String) (params : List String) (body : List LuauStmt)

-- ======================================================================
-- Section 4: Luau top-level structures
-- ======================================================================

/-- A named Luau function (top-level or local). -/
structure LuauFunc where
  name   : String
  params : List String
  body   : List LuauStmt

/-- A complete Luau module as emitted by the Molt backend.
    Corresponds to the full output of `emit_luau_module` in luau.rs. -/
structure LuauModule where
  prelude   : List LuauStmt    -- conditional prelude helpers (molt_list_get, etc.)
  functions : List LuauFunc    -- translated IR functions
  mainBody  : List LuauStmt    -- module-level statements (entry point)

-- ======================================================================
-- Section 5: Utility definitions
-- ======================================================================

/-- Check if a Luau expression is a simple literal (no sub-expressions). -/
def LuauExpr.isLiteral : LuauExpr → Bool
  | .intLit _ | .floatLit _ | .strLit _ | .boolLit _ | .nil => true
  | _ => false


end MoltTIR.Backend
