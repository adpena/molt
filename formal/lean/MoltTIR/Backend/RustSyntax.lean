/-
  MoltTIR.Backend.RustSyntax -- Rust target AST for the Molt transpiler backend.

  Models the subset of Rust syntax that a Python-to-Rust transpiler emits.
  This is a source-to-source translation: MoltTIR → Rust source code. The
  Rust compiler (rustc) then handles type checking, borrow checking, and
  native code generation.

  Key Rust-specific modeling decisions:
  - Ownership annotations (Owned, Borrowed, MutBorrowed) on bindings
  - Explicit mutability on let bindings
  - Option<T> models Python None / exception paths
  - Result<T, E> models fallible operations
  - No implicit coercions — type conversions are explicit
  - Vec<T> for Python lists, HashMap<K,V> for dicts
  - Tuple types for Python tuples
  - Struct for Python class instances

  Note: RustExpr and RustStmt avoid mutual recursion. Match expressions
  with complex arms are modeled as matchStmt in RustStmt (parallel to
  Luau's approach of emitting inline function expressions as localFunc
  statements instead of funcExpr in LuauExpr).
-/
import MoltTIR.Types

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Ownership annotations
-- ======================================================================

/-- Rust ownership / borrowing mode for values and bindings.
    This is the central difference from Luau/GC'd targets: the transpiler
    must decide how each value is held. -/
inductive Ownership where
  | owned        -- T — value is moved/owned
  | borrowed     -- &T — shared reference
  | mutBorrowed  -- &mut T — exclusive mutable reference
  deriving DecidableEq, Repr

-- ======================================================================
-- Section 2: Rust types
-- ======================================================================

/-- Rust type AST. Models the subset of Rust types that the transpiler emits.
    Corresponds to the Rust types used in Molt's runtime crate. -/
inductive RustType where
  | i64                                       -- Python int (within safe range)
  | f64                                       -- Python float
  | bool                                      -- Python bool
  | string                                    -- Python str (String in Rust)
  | unit                                      -- Python None type / ()
  | option (inner : RustType)                 -- Option<T> for nullable values
  | vec (elem : RustType)                     -- Vec<T> for Python list
  | hashMap (key value : RustType)            -- HashMap<K,V> for Python dict
  | tuple (elems : List RustType)             -- (T1, T2, ...) for Python tuple
  | structTy (name : String)                  -- named struct for class instances
  | ref (ownership : Ownership) (inner : RustType)  -- &T, &mut T wrappers
  deriving Repr

-- ======================================================================
-- Section 3: Rust binary and unary operators
-- ======================================================================

/-- Rust binary operators. Subset that the Molt transpiler emits. -/
inductive RustBinOp where
  -- arithmetic
  | add | sub | mul | div | rem
  -- exponentiation (i64::pow / f64::powi)
  | pow
  -- floor division (Python // semantics: round toward negative infinity)
  | floordiv
  -- bitwise
  | bitAnd | bitOr | bitXor
  -- shift
  | shl | shr
  -- comparison
  | eq | ne | lt | le | gt | ge
  -- logical
  | and | or
  deriving DecidableEq, Repr

/-- Rust unary operators. -/
inductive RustUnOp where
  | neg      -- arithmetic negation (-)
  | not      -- logical/bitwise negation (!)
  | abs      -- absolute value (i64::abs)
  deriving DecidableEq, Repr

-- ======================================================================
-- Section 4: Rust patterns
-- ======================================================================

/-- Rust pattern for match arms and let bindings.
    Defined before RustExpr to avoid forward references. Literal patterns
    use Int/String/Bool directly rather than referencing RustExpr. -/
inductive RustPattern where
  | wildcard                           -- _
  | varBind (name : String)           -- x (binding)
  | intPat (n : Int)                  -- integer literal pattern
  | strPat (s : String)              -- string literal pattern
  | boolPat (b : Bool)              -- boolean literal pattern
  | somePat (inner : RustPattern)    -- Some(p)
  | nonePat                           -- None
  | tuplePat (elems : List RustPattern)
  deriving Repr

-- ======================================================================
-- Section 5: Rust expressions
-- ======================================================================

/-- Rust expression AST. Models the fragment of Rust that the transpiler emits.
    Match expressions with complex pattern arms are modeled as matchStmt in
    RustStmt to avoid mutual recursion (parallel to Luau's approach). -/
inductive RustExpr where
  | intLit (n : Int)
  | floatLit (f : Int)              -- modeled as fixed-point, matching IR convention
  | strLit (s : String)
  | boolLit (b : Bool)
  | unitLit                          -- () — corresponds to Python None
  | varRef (name : String)
  | binOp (op : RustBinOp) (lhs rhs : RustExpr)
  | unOp (op : RustUnOp) (arg : RustExpr)
  | methodCall (obj : RustExpr) (method : String) (args : List RustExpr)
  | fieldAccess (obj : RustExpr) (field : String)
  | indexOp (container : RustExpr) (index : RustExpr)
  | closureExpr (params : List (String × RustType))
                (body : RustExpr)
  | someExpr (inner : RustExpr)      -- Some(x) — wrapping into Option
  | noneExpr                          -- None — Option::None
  | refExpr (ownership : Ownership) (inner : RustExpr)   -- &x, &mut x
  | derefExpr (inner : RustExpr)     -- *x
  | tupleExpr (elems : List RustExpr)
  | callExpr (func : RustExpr) (args : List RustExpr)    -- f(args)
  | macroCall (name : String) (args : List RustExpr)     -- vec![], println![], etc.
  deriving Repr

-- ======================================================================
-- Section 6: Rust statements
-- ======================================================================

/-- Rust statement AST. -/
inductive RustStmt where
  | letBinding (name : String) (mutable : Bool) (ty : Option RustType)
               (init : Option RustExpr)
  | assign (target : RustExpr) (val : RustExpr)
  | exprStmt (e : RustExpr)
  | returnStmt (val : Option RustExpr)
  | ifElse (cond : RustExpr) (thenBody : List RustStmt)
           (elseBody : Option (List RustStmt))
  | forLoop (var_ : String) (iter : RustExpr) (body : List RustStmt)
  | whileLoop (cond : RustExpr) (body : List RustStmt)
  | matchStmt (scrutinee : RustExpr)
              (arms : List (RustPattern × List RustStmt))

-- ======================================================================
-- Section 7: Rust top-level structures
-- ======================================================================

/-- A Rust function parameter with name, type, and ownership mode. -/
structure RustParam where
  name      : String
  ty        : RustType
  ownership : Ownership
  deriving Repr

/-- A named Rust function (top-level or associated). -/
structure RustFn where
  name       : String
  params     : List RustParam
  returnType : RustType
  body       : List RustStmt

/-- A Rust struct definition (models Python class). -/
structure RustStruct where
  name   : String
  fields : List (String × RustType)

/-- A complete Rust module as emitted by the Molt transpiler.
    Corresponds to a single .rs output file. -/
structure RustModule where
  imports    : List String             -- use statements
  structs    : List RustStruct         -- struct definitions (from Python classes)
  functions  : List RustFn             -- translated IR functions
  mainBody   : List RustStmt           -- fn main() body

-- ======================================================================
-- Section 8: Utility definitions
-- ======================================================================

/-- Check if a Rust expression is a simple literal (no sub-expressions). -/
def RustExpr.isLiteral : RustExpr → Bool
  | .intLit _ | .floatLit _ | .strLit _ | .boolLit _ | .unitLit => true
  | _ => false

/-- Check if a Rust type is a scalar Copy type (no ownership transfer needed).
    Only considers leaf types to avoid termination issues with recursive types.
    Option and tuple Copy-ness must be checked separately when needed. -/
def RustType.isCopy : RustType → Bool
  | .i64 | .f64 | .bool | .unit => true
  | _ => false

end MoltTIR.Backend
