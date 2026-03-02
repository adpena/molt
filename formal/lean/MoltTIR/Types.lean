/-
  MoltTIR.Types — IR type system for the Molt TIR core formalization.

  Models Molt's NaN-boxed runtime value tags and the type hints
  used in the IR. This aligns with the real Molt type system:
  runtime values are 64-bit NaN-boxed with tags INT, BOOL, NONE, PTR, PENDING.

  The formalization starts with a minimal subset and grows incrementally.
-/
namespace MoltTIR

/-- Runtime value tags corresponding to Molt's NaN-boxed representation.
    INT and BOOL are inline (no heap allocation); PTR references a heap object. -/
inductive Ty where
  | int
  | bool
  | float
  | str
  | none
  | bytes
  | list
  | dict
  | set
  | tuple
  | obj     -- generic heap object (class instances, etc.)
  deriving DecidableEq, Repr

/-- Type hints as used in MoltValue.type_hint (string-based in the real IR,
    but we use a closed enum for proof tractability). -/
inductive TypeHint where
  | known (t : Ty)
  | unknown
  deriving DecidableEq, Repr

end MoltTIR
