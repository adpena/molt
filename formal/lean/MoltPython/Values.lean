/-
  MoltPython.Values -- Python runtime value model.

  Models the values that Python expressions evaluate to. This is the source-level
  analog of MoltTIR.Value, but richer: it includes lists, tuples, dicts, and
  function/class values needed to model Python source semantics.

  Floats are modeled as Int (same as MoltTIR) for determinism proofs.
-/
import MoltPython.Syntax

set_option autoImplicit false

namespace MoltPython

/-- Python runtime values. -/
inductive PyValue where
  | intVal (n : Int)
  | floatVal (f : Int)            -- Int for determinism (same convention as MoltTIR)
  | boolVal (b : Bool)
  | strVal (s : String)
  | noneVal
  | listVal (elts : List PyValue)
  | tupleVal (elts : List PyValue)
  | dictVal (entries : List (PyValue × PyValue))
  | funcVal (name : Name) (params : List Param) (body : List PyStmt)
  | classVal (name : Name)
  deriving Repr

namespace PyValue

/-- Python truthiness: bool() semantics.
    False values: False, 0, 0.0, "", None, [], (), {} -/
def truthy : PyValue → Bool
  | .boolVal b => b
  | .intVal n => n != 0
  | .floatVal f => f != 0
  | .strVal s => s != ""
  | .noneVal => false
  | .listVal l => !l.isEmpty
  | .tupleVal l => !l.isEmpty
  | .dictVal d => !d.isEmpty
  | .funcVal _ _ _ => true
  | .classVal _ => true

/-- Equality comparison (Python's ==). Returns none for incomparable types. -/
def pyEq : PyValue → PyValue → Option Bool
  | .intVal x, .intVal y => some (x == y)
  | .intVal x, .floatVal y => some (x == y)
  | .floatVal x, .intVal y => some (x == y)
  | .floatVal x, .floatVal y => some (x == y)
  | .boolVal x, .boolVal y => some (x == y)
  | .boolVal x, .intVal y =>
      let xi : Int := if x then 1 else 0
      some (xi == y)
  | .intVal x, .boolVal y =>
      let yi : Int := if y then 1 else 0
      some (x == yi)
  | .strVal x, .strVal y => some (x == y)
  | .noneVal, .noneVal => some true
  | .noneVal, _ => some false
  | _, .noneVal => some false
  | _, _ => none

/-- Less-than comparison (Python's <). Returns none for incomparable types. -/
def pyLt : PyValue → PyValue → Option Bool
  | .intVal x, .intVal y => some (x < y)
  | .intVal x, .floatVal y => some (x < y)
  | .floatVal x, .intVal y => some (x < y)
  | .floatVal x, .floatVal y => some (x < y)
  | .strVal x, .strVal y => some (x < y)
  | .boolVal x, .boolVal y =>
      let xi : Int := if x then 1 else 0
      let yi : Int := if y then 1 else 0
      some (xi < yi)
  | _, _ => none

end PyValue

end MoltPython
