/-
  MoltPython.Env -- Python environment model with scope chain.

  Python has lexical scoping with the LEGB rule (Local, Enclosing, Global, Builtin).
  We model this as a stack of scopes, where each scope is a finite map from
  variable names to values. The topmost scope is the current local scope.
-/
import MoltPython.Values

set_option autoImplicit false

namespace MoltPython

/-- A single scope: maps variable names to values. -/
abbrev Scope := List (Name × PyValue)

namespace Scope

def empty : Scope := []

def lookup (s : Scope) (x : Name) : Option PyValue :=
  match s with
  | [] => none
  | (k, v) :: rest => if k == x then some v else lookup rest x

def set (s : Scope) (x : Name) (v : PyValue) : Scope :=
  (x, v) :: s

theorem lookup_set_eq (s : Scope) (x : Name) (v : PyValue) :
    lookup (set s x v) x = some v := by
  simp [set, lookup]

end Scope

/-- Search through a list of scopes for a variable (innermost first). -/
def lookupScopes : List Scope → Name → Option PyValue
  | [], _ => none
  | s :: rest, x =>
    match s.lookup x with
    | some v => some v
    | none => lookupScopes rest x

/-- Python environment: a stack of scopes (innermost first).
    The last scope is the module/global scope. -/
structure PyEnv where
  scopes : List Scope
  deriving Repr

namespace PyEnv

/-- Empty environment with one global scope. -/
def empty : PyEnv := { scopes := [Scope.empty] }

/-- Look up a variable through the scope chain (LEGB-style, innermost first). -/
def lookup (env : PyEnv) (x : Name) : Option PyValue :=
  lookupScopes env.scopes x

/-- Set a variable in the innermost (current) scope. -/
def set (env : PyEnv) (x : Name) (v : PyValue) : PyEnv :=
  match env.scopes with
  | [] => { scopes := [Scope.set Scope.empty x v] }
  | s :: rest => { scopes := s.set x v :: rest }

/-- Push a new empty scope (entering a function body). -/
def pushScope (env : PyEnv) : PyEnv :=
  { scopes := Scope.empty :: env.scopes }

/-- Pop the innermost scope (leaving a function body). -/
def popScope (env : PyEnv) : PyEnv :=
  match env.scopes with
  | [] => env
  | _ :: rest => { scopes := rest }

/-- Set a variable in the global (outermost) scope.
    Corresponds to Python's `global x` declaration semantics. -/
def setGlobal (env : PyEnv) (x : Name) (v : PyValue) : PyEnv :=
  match env.scopes.reverse with
  | [] => { scopes := [Scope.set Scope.empty x v] }
  | g :: rest => { scopes := (g.set x v :: rest).reverse }

/-- Look up in the global scope only. -/
def lookupGlobal (env : PyEnv) (x : Name) : Option PyValue :=
  match env.scopes.reverse with
  | [] => none
  | g :: _ => g.lookup x

-- Basic properties

theorem lookup_set_eq (env : PyEnv) (x : Name) (v : PyValue)
    (h : env.scopes ≠ []) :
    (env.set x v).lookup x = some v := by
  simp [set, lookup]
  match hs : env.scopes with
  | [] => exact absurd hs h
  | _ :: _ => simp [hs, lookupScopes, Scope.lookup_set_eq]

end PyEnv

end MoltPython
