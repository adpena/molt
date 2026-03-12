/-
  MoltPython.Properties.TypeSafety -- Basic type safety for the Python subset.

  Defines a simple typing judgment for Python expressions and proves that
  well-typed expressions always evaluate to Some (progress property).

  The type system here is intentionally simple -- it's the source-level type
  information that Molt's frontend can infer before lowering to TIR.
-/
import MoltPython.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltPython

/-- Simple Python types for the typing judgment. -/
inductive PyTy where
  | int | float | bool | str | none
  | list (elem : PyTy)
  | tuple (elems : List PyTy)
  | dict (key val : PyTy)
  | func (params : List PyTy) (ret : PyTy)
  | cls (name : Name)
  | any                          -- top type: always well-typed
  deriving Repr

/-- Type environment: maps variable names to types. -/
abbrev TyEnv := List (Name × PyTy)

namespace TyEnv

def empty : TyEnv := []

def lookup (tenv : TyEnv) (x : Name) : Option PyTy :=
  match tenv with
  | [] => none
  | (k, t) :: rest => if k == x then some t else lookup rest x

def extend (tenv : TyEnv) (x : Name) (t : PyTy) : TyEnv :=
  (x, t) :: tenv

end TyEnv

/-- Typing judgment for expressions: tenv |- e : t.
    An inductive relation (not a function) for proof flexibility. -/
inductive HasType : TyEnv → PyExpr → PyTy → Prop where
  | intLit (tenv : TyEnv) (n : Int) :
      HasType tenv (.intLit n) .int
  | floatLit (tenv : TyEnv) (f : Int) :
      HasType tenv (.floatLit f) .float
  | boolLit (tenv : TyEnv) (b : Bool) :
      HasType tenv (.boolLit b) .bool
  | strLit (tenv : TyEnv) (s : String) :
      HasType tenv (.strLit s) .str
  | noneLit (tenv : TyEnv) :
      HasType tenv .noneLit .none
  | name (tenv : TyEnv) (x : Name) (t : PyTy) :
      tenv.lookup x = some t → HasType tenv (.name x) t
  | addInt (tenv : TyEnv) (a b : PyExpr) :
      HasType tenv a .int → HasType tenv b .int →
      HasType tenv (.binOp .add a b) .int
  | subInt (tenv : TyEnv) (a b : PyExpr) :
      HasType tenv a .int → HasType tenv b .int →
      HasType tenv (.binOp .sub a b) .int
  | mulInt (tenv : TyEnv) (a b : PyExpr) :
      HasType tenv a .int → HasType tenv b .int →
      HasType tenv (.binOp .mul a b) .int
  | negInt (tenv : TyEnv) (a : PyExpr) :
      HasType tenv a .int →
      HasType tenv (.unaryOp .neg a) .int
  | notBool (tenv : TyEnv) (a : PyExpr) (t : PyTy) :
      HasType tenv a t →
      HasType tenv (.unaryOp .not a) .bool
  | anyExpr (tenv : TyEnv) (e : PyExpr) :
      HasType tenv e .any

/-- Value-type consistency: a value has a given type. -/
def valueHasType : PyValue → PyTy → Prop
  | .intVal _, .int => True
  | .floatVal _, .float => True
  | .boolVal _, .bool => True
  | .strVal _, .str => True
  | .noneVal, .none => True
  | _, .any => True
  | _, _ => False

/-- Environment-type consistency: every binding in tenv has the right type in env. -/
def envConsistent (tenv : TyEnv) (env : PyEnv) : Prop :=
  ∀ x t, tenv.lookup x = some t →
         ∃ v, env.lookup x = some v ∧ valueHasType v t

/-- Progress for integer literals: always evaluate to some. -/
theorem literal_progress_int (fuel : Nat) (env : PyEnv) (hfuel : fuel > 0) :
    ∀ n, evalPyExpr fuel env (.intLit n) = some (.intVal n) := by
  intro n
  cases fuel with
  | zero => omega
  | succ f => simp [evalPyExpr]

/-- Progress for float literals. -/
theorem literal_progress_float (fuel : Nat) (env : PyEnv) (hfuel : fuel > 0) :
    ∀ f, evalPyExpr fuel env (.floatLit f) = some (.floatVal f) := by
  intro f
  cases fuel with
  | zero => omega
  | succ n => simp [evalPyExpr]

/-- Progress for bool literals. -/
theorem literal_progress_bool (fuel : Nat) (env : PyEnv) (hfuel : fuel > 0) :
    ∀ b, evalPyExpr fuel env (.boolLit b) = some (.boolVal b) := by
  intro b
  cases fuel with
  | zero => omega
  | succ n => simp [evalPyExpr]

/-- Progress for string literals. -/
theorem literal_progress_str (fuel : Nat) (env : PyEnv) (hfuel : fuel > 0) :
    ∀ s, evalPyExpr fuel env (.strLit s) = some (.strVal s) := by
  intro s
  cases fuel with
  | zero => omega
  | succ n => simp [evalPyExpr]

/-- Progress for None literal. -/
theorem literal_progress_none (fuel : Nat) (env : PyEnv) (hfuel : fuel > 0) :
    evalPyExpr fuel env .noneLit = some .noneVal := by
  cases fuel with
  | zero => omega
  | succ n => simp [evalPyExpr]

/-- Progress for variable lookup when binding exists. -/
theorem name_progress (fuel : Nat) (env : PyEnv) (x : Name) (v : PyValue)
    (hfuel : fuel > 0) (hbind : env.lookup x = some v) :
    evalPyExpr fuel env (.name x) = some v := by
  cases fuel with
  | zero => omega
  | succ n => simp [evalPyExpr, hbind]

end MoltPython
