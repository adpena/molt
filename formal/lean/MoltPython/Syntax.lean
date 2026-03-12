/-
  MoltPython.Syntax -- Python 3.12+ subset AST for the Molt source formalization.

  Models the verified subset of Python that Molt compiles. This is NOT a full
  CPython AST -- it omits dynamic features (exec, eval, monkeypatching) that
  Molt explicitly breaks (see docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md).

  The AST closely follows CPython's ast module structure (expressions, statements,
  operators) but restricted to the statically-analyzable subset.
-/
set_option autoImplicit false

namespace MoltPython

/-- Variable names in the Python source. -/
abbrev Name := String

/-- Binary arithmetic and bitwise operators. -/
inductive BinOp where
  | add | sub | mul | div | floorDiv | mod | pow
  | bitAnd | bitOr | bitXor | lShift | rShift
  deriving DecidableEq, Repr

/-- Unary operators. -/
inductive UnaryOp where
  | not | neg | invert
  deriving DecidableEq, Repr

/-- Comparison operators. Python supports chained comparisons (a < b < c),
    modeled as a list of (op, expr) pairs after the first operand. -/
inductive CompareOp where
  | eq | notEq | lt | ltE | gt | gtE
  | is | isNot | «in» | notIn
  deriving DecidableEq, Repr

/-- Boolean operators (short-circuit). -/
inductive BoolOp where
  | and | or
  deriving DecidableEq, Repr

/-- Python expressions. Uses fuel-friendly mutual recursion via List. -/
inductive PyExpr where
  | intLit (n : Int)
  | floatLit (f : Int)           -- model floats as Int for determinism (same as MoltTIR)
  | boolLit (b : Bool)
  | strLit (s : String)
  | noneLit
  | name (x : Name)
  | binOp (op : BinOp) (left right : PyExpr)
  | unaryOp (op : UnaryOp) (operand : PyExpr)
  | compare (left : PyExpr) (ops : List CompareOp) (comparators : List PyExpr)
  | boolOp (op : BoolOp) (values : List PyExpr)
  | ifExpr (test body orElse : PyExpr)
  | call (func : PyExpr) (args : List PyExpr)
  | subscript (value : PyExpr) (slice : PyExpr)
  | listExpr (elts : List PyExpr)
  | tupleExpr (elts : List PyExpr)
  | dictExpr (keys : List PyExpr) (values : List PyExpr)
  deriving Repr

/-- Function parameter (name with optional default). -/
structure Param where
  name : Name
  default? : Option PyExpr
  deriving Repr

/-- Python statements. -/
inductive PyStmt where
  | assign (target : Name) (value : PyExpr)
  | augAssign (target : Name) (op : BinOp) (value : PyExpr)
  | ifStmt (test : PyExpr) (body : List PyStmt) (elifs : List (PyExpr × List PyStmt)) (orElse : List PyStmt)
  | whileStmt (test : PyExpr) (body : List PyStmt) (orElse : List PyStmt)
  | forStmt (target : Name) (iter : PyExpr) (body : List PyStmt) (orElse : List PyStmt)
  | funcDef (name : Name) (params : List Param) (body : List PyStmt)
  | returnStmt (value : Option PyExpr)
  | classDef (name : Name) (bases : List PyExpr) (body : List PyStmt)
  | importStmt (module : Name) (names : List Name)
  | tryStmt (body : List PyStmt) (handlers : List (Option Name × Name × List PyStmt)) (orElse : List PyStmt) (finally_ : List PyStmt)
  | withStmt (items : List (PyExpr × Option Name)) (body : List PyStmt)
  | assertStmt (test : PyExpr) (msg : Option PyExpr)
  | pass
  | break_
  | continue_
  | exprStmt (value : PyExpr)
  deriving Repr

/-- A Python module is a list of top-level statements. -/
abbrev PyModule := List PyStmt

end MoltPython
