/-
  MoltTIR.Passes.Lattice — abstract value lattice for SCCP.

  Three-point lattice:     ⊤ (overdefined)
                          / \
                    known(v₁) known(v₂) ...
                          \ /
                     ⊥ (unknown)

  `unknown` means "not yet analyzed" (optimistically constant).
  `known v` means "proven constant with value v."
  `overdefined` means "not constant" (multiple reaching values).

  Corresponds to the SCCP lattice in Molt's midend pipeline.
-/
import MoltTIR.Syntax

namespace MoltTIR

/-- Abstract value in the SCCP lattice. -/
inductive AbsVal where
  | unknown     -- ⊥: not yet analyzed
  | known (v : Value)  -- constant value
  | overdefined -- ⊤: not constant
  deriving DecidableEq, Repr

namespace AbsVal

/-- Lattice join (least upper bound). Uses DecidableEq for cleaner proofs. -/
def join (a b : AbsVal) : AbsVal :=
  match a, b with
  | .unknown, x => x
  | x, .unknown => x
  | .known v1, .known v2 => if v1 = v2 then .known v1 else .overdefined
  | .overdefined, _ => .overdefined
  | _, .overdefined => .overdefined

/-- Ordering: unknown ≤ known v ≤ overdefined. -/
def le (a b : AbsVal) : Prop :=
  join a b = b

instance : LE AbsVal where le := le

/-- ⊥ is the bottom element. -/
theorem unknown_le (a : AbsVal) : .unknown ≤ a := by
  simp [LE.le, le, join]

/-- ⊤ is the top element. -/
theorem le_overdefined (a : AbsVal) : a ≤ .overdefined := by
  simp [LE.le, le, join]
  cases a <;> rfl

/-- Join is commutative. -/
theorem join_comm (a b : AbsVal) : join a b = join b a := by
  cases a <;> cases b <;> simp [join]
  next v1 v2 =>
    by_cases h : v1 = v2
    · subst h; simp
    · have h' : v2 ≠ v1 := fun heq => h heq.symm
      simp [h, h']

/-- Join is idempotent. -/
theorem join_idem (a : AbsVal) : join a a = a := by
  cases a <;> simp [join]

/-- join with overdefined on the left always gives overdefined. -/
@[simp] theorem join_overdefined_left (b : AbsVal) : join .overdefined b = .overdefined := by
  cases b <;> rfl

/-- join with overdefined on the right always gives overdefined. -/
@[simp] theorem join_overdefined_right (a : AbsVal) : join a .overdefined = .overdefined := by
  cases a <;> rfl

/-- join with unknown on the left is identity. -/
@[simp] theorem join_unknown_left (b : AbsVal) : join .unknown b = b := by
  cases b <;> simp [join]

/-- join with unknown on the right is identity. -/
@[simp] theorem join_unknown_right (a : AbsVal) : join a .unknown = a := by
  cases a <;> simp [join]

/-- Join is associative. -/
theorem join_assoc (a b c : AbsVal) : join (join a b) c = join a (join b c) := by
  cases a with
  | unknown => simp
  | overdefined => simp
  | known v1 =>
    cases b with
    | unknown => simp
    | overdefined => simp
    | known v2 =>
      cases c with
      | unknown => simp
      | overdefined => simp
      | known v3 =>
        simp only [join]
        by_cases h12 : v1 = v2 <;> by_cases h23 : v2 = v3 <;> simp_all [join]

/-- ≤ is reflexive. -/
theorem le_refl (a : AbsVal) : a ≤ a := by
  simp [LE.le, le, join_idem]

/-- The concretization relation: an abstract value represents a concrete value. -/
def concretizes : AbsVal → Value → Prop
  | .unknown, _ => True         -- ⊥ represents everything (optimistic)
  | .known v, v' => v = v'      -- exact match
  | .overdefined, _ => True     -- ⊤ represents everything (conservative)

/-- known v concretizes exactly v. -/
theorem known_concretizes (v : Value) : concretizes (.known v) v := rfl

/-- overdefined concretizes everything. -/
theorem overdefined_concretizes (v : Value) : concretizes .overdefined v := trivial

/-- unknown concretizes everything (optimistic assumption). -/
theorem unknown_concretizes (v : Value) : concretizes .unknown v := trivial

/-- Join preserves concretization: if both abstract values represent v,
    so does their join. -/
theorem join_concretizes (a b : AbsVal) (v : Value)
    (ha : concretizes a v) (hb : concretizes b v) :
    concretizes (join a b) v := by
  cases a <;> cases b <;> simp [join, concretizes] at *
  · exact hb
  · exact ha
  · subst ha; subst hb; simp [concretizes]

end AbsVal

/-- Abstract environment mapping variables to abstract values. -/
abbrev AbsEnv := Var → AbsVal

namespace AbsEnv

/-- All-unknown abstract environment. -/
def top : AbsEnv := fun _ => .unknown

/-- Update an abstract environment at a variable. -/
def set (σ : AbsEnv) (x : Var) (a : AbsVal) : AbsEnv :=
  fun y => if y = x then a else σ y

end AbsEnv

end MoltTIR
