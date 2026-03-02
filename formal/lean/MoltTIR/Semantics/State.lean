/-
  MoltTIR.Semantics.State — execution state: environments and outcomes.
-/
import MoltTIR.Syntax

namespace MoltTIR

/-- Environment mapping SSA variables to values. -/
abbrev Env := Var → Option Value

namespace Env

def empty : Env := fun _ => none

def set (ρ : Env) (x : Var) (v : Value) : Env :=
  fun y => if y = x then some v else ρ y

theorem set_eq (ρ : Env) (x : Var) (v : Value) :
    (ρ.set x v) x = some v := by
  simp [set]

theorem set_ne (ρ : Env) (x y : Var) (v : Value) (h : y ≠ x) :
    (ρ.set x v) y = ρ y := by
  simp [set, h]

end Env

/-- Observable outcome of executing a function. -/
inductive Outcome where
  | ret (v : Value)
  | stuck
  deriving DecidableEq, Repr

end MoltTIR
