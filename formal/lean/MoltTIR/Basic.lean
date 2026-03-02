/-
  MoltTIR.Basic — shared utilities for the Molt TIR formalization.

  Finite maps, option helpers, and lemmas used across the development.
-/
namespace MoltTIR

/-- A finite map backed by an association list. Proof-friendly but not performant. -/
def FinMap (α β : Type) [DecidableEq α] := List (α × β)

namespace FinMap

variable {α β : Type} [DecidableEq α]

def empty : FinMap α β := []

def lookup (m : FinMap α β) (k : α) : Option β :=
  match m with
  | [] => none
  | (k', v) :: rest => if k == k' then some v else lookup rest k

def insert (m : FinMap α β) (k : α) (v : β) : FinMap α β :=
  (k, v) :: m

theorem lookup_insert_eq (m : FinMap α β) (k : α) (v : β) :
    lookup (insert m k v) k = some v := by
  simp [insert, lookup]

theorem lookup_insert_ne (m : FinMap α β) (k k' : α) (v : β) (h : k ≠ k') :
    lookup (insert m k' v) k = lookup m k := by
  simp [insert, lookup]
  intro heq
  exact absurd heq h

end FinMap

end MoltTIR
