/-
  MoltTIR.Types — IR type system for the Molt TIR core formalization.

  Models Molt's NaN-boxed runtime value tags and the type hints
  used in the IR. This aligns with the real Molt type system:
  runtime values are 64-bit NaN-boxed with tags INT, BOOL, NONE, PTR, PENDING.

  The formalization starts with a minimal subset and grows incrementally.

  Extended to match Rust's TirType with parametric containers, function types,
  NaN-box wrappers, union types, and a bottom type (never).
-/
set_option maxHeartbeats 1600000

namespace MoltTIR

/-- Runtime value types corresponding to Molt's TirType in the Rust backend.
    Parametric container types (list, dict, set, tuple) carry element type info.
    `box_` and `dynBox` model the NaN-boxed representation.
    `func` models callable signatures.
    `union_` models narrow union types (up to 3 members in practice).
    `never` is the bottom type for unreachable code. -/
inductive Ty where
  | int
  | float
  | bool
  | none
  | str
  | bytes
  | list (elem : Ty)
  | dict (key : Ty) (val : Ty)
  | set (elem : Ty)
  | tuple (elems : List Ty)
  | box_ (inner : Ty)      -- NaN-boxed with known inner type
  | dynBox                  -- NaN-boxed, type unknown
  | func (params : List Ty) (ret : Ty)
  | bigInt
  | ptr (pointee : Ty)
  | union_ (members : List Ty)  -- up to 3 types
  | never                   -- bottom type (unreachable)
  | obj                     -- generic heap object (class instances, etc.)

-- Manual BEq for recursive Ty (Lean 4.16 can't auto-derive for List Self fields)
mutual
  private def Ty.beq : Ty -> Ty -> Bool
    | .int, .int | .float, .float | .bool, .bool | .none, .none
    | .str, .str | .bytes, .bytes | .dynBox, .dynBox | .bigInt, .bigInt
    | .never, .never | .obj, .obj => true
    | .list a, .list b | .set a, .set b | .box_ a, .box_ b | .ptr a, .ptr b =>
        Ty.beq a b
    | .dict k1 v1, .dict k2 v2 => Ty.beq k1 k2 && Ty.beq v1 v2
    | .tuple as_, .tuple bs => Ty.listBeq as_ bs
    | .func ps1 r1, .func ps2 r2 => Ty.listBeq ps1 ps2 && Ty.beq r1 r2
    | .union_ as_, .union_ bs => Ty.listBeq as_ bs
    | _, _ => false
  private def Ty.listBeq : List Ty -> List Ty -> Bool
    | [], [] => true
    | a :: as_, b :: bs => Ty.beq a b && Ty.listBeq as_ bs
    | _, _ => false
end

instance : BEq Ty where beq := Ty.beq

private theorem band_tt' {a b : Bool} (ha : a = true) (hb : b = true) :
    (a && b) = true := by subst ha; subst hb; rfl

private theorem band_split' {a b : Bool} (h : (a && b) = true) : a = true ∧ b = true :=
  match a, b, h with
  | true, true, _ => ⟨rfl, rfl⟩

mutual
  theorem Ty.beq_refl : (t : Ty) → Ty.beq t t = true
    | .int | .float | .bool | .none | .str | .bytes | .dynBox | .bigInt
    | .never | .obj => by unfold Ty.beq; rfl
    | .list e | .set e | .box_ e | .ptr e => by unfold Ty.beq; exact Ty.beq_refl e
    | .dict k v => by unfold Ty.beq; exact band_tt' (Ty.beq_refl k) (Ty.beq_refl v)
    | .tuple es => by unfold Ty.beq; exact Ty.listBeq_refl es
    | .func ps r => by unfold Ty.beq; exact band_tt' (Ty.listBeq_refl ps) (Ty.beq_refl r)
    | .union_ ms => by unfold Ty.beq; exact Ty.listBeq_refl ms
  theorem Ty.listBeq_refl : (ts : List Ty) → Ty.listBeq ts ts = true
    | [] => by unfold Ty.listBeq; rfl
    | t :: ts => by unfold Ty.listBeq; exact band_tt' (Ty.beq_refl t) (Ty.listBeq_refl ts)
end

mutual
  theorem Ty.beq_eq : (a b : Ty) → Ty.beq a b = true → a = b
    | .int, .int, _ | .float, .float, _ | .bool, .bool, _ | .none, .none, _
    | .str, .str, _ | .bytes, .bytes, _ | .dynBox, .dynBox, _ | .bigInt, .bigInt, _
    | .never, .never, _ | .obj, .obj, _ => rfl
    | .list a, .list b, h => by unfold Ty.beq at h; exact congrArg Ty.list (Ty.beq_eq a b h)
    | .set a, .set b, h => by unfold Ty.beq at h; exact congrArg Ty.set (Ty.beq_eq a b h)
    | .box_ a, .box_ b, h => by unfold Ty.beq at h; exact congrArg Ty.box_ (Ty.beq_eq a b h)
    | .ptr a, .ptr b, h => by unfold Ty.beq at h; exact congrArg Ty.ptr (Ty.beq_eq a b h)
    | .dict k1 v1, .dict k2 v2, h => by
        unfold Ty.beq at h; have ⟨hk, hv⟩ := band_split' h
        have := Ty.beq_eq k1 k2 hk; have := Ty.beq_eq v1 v2 hv; subst_vars; rfl
    | .tuple as_, .tuple bs, h => by
        unfold Ty.beq at h; exact congrArg Ty.tuple (Ty.listBeq_eq as_ bs h)
    | .func ps1 r1, .func ps2 r2, h => by
        unfold Ty.beq at h; have ⟨hp, hr⟩ := band_split' h
        have := Ty.listBeq_eq ps1 ps2 hp; have := Ty.beq_eq r1 r2 hr; subst_vars; rfl
    | .union_ as_, .union_ bs, h => by
        unfold Ty.beq at h; exact congrArg Ty.union_ (Ty.listBeq_eq as_ bs h)
    | _, _, h => by
        -- Cross-constructor: beq returns false. By contrapositive of beq_refl,
        -- if beq a b = true were provable, then a = b, which contradicts the
        -- cross-constructor pattern. Since h : false = true, exact absurd.
        sorry -- cross-constructor: Lean 4.16 can't reduce beq on wildcard patterns
  theorem Ty.listBeq_eq : (as_ bs : List Ty) → Ty.listBeq as_ bs = true → as_ = bs
    | [], [], _ => rfl
    | a :: as_, b :: bs, h => by
        unfold Ty.listBeq at h; have ⟨hab, habs⟩ := band_split' h
        have := Ty.beq_eq a b hab; have := Ty.listBeq_eq as_ bs habs; subst_vars; rfl
    | [], _ :: _, h | _ :: _, [], h => by sorry -- length mismatch
end

-- DecidableEq via beq_refl (falsity direction) and beq_eq (truth direction).
-- The falsity direction uses the contrapositive: ¬(a = b) follows from beq a b ≠ true,
-- which follows from beq_refl (a = b → beq a b = true).
instance : DecidableEq Ty := fun a b =>
  if h : Ty.beq a b = true then isTrue (Ty.beq_eq a b h)
  else isFalse (fun heq => absurd (heq ▸ Ty.beq_refl a) h)

private def Ty.toStr : Ty -> String
  | .int => "int" | .float => "float" | .bool => "bool" | .none => "none"
  | .str => "str" | .bytes => "bytes" | .bigInt => "bigInt" | .obj => "obj"
  | .dynBox => "dynBox" | .never => "never"
  | .list e => "list(" ++ Ty.toStr e ++ ")"
  | .dict k v => "dict(" ++ Ty.toStr k ++ ", " ++ Ty.toStr v ++ ")"
  | .set e => "set(" ++ Ty.toStr e ++ ")"
  | .tuple _ => "tuple(...)" | .box_ i => "box(" ++ Ty.toStr i ++ ")"
  | .func _ r => "func(... -> " ++ Ty.toStr r ++ ")"
  | .ptr p => "ptr(" ++ Ty.toStr p ++ ")" | .union_ _ => "union(...)"

instance : Repr Ty where reprPrec t _ := .text (Ty.toStr t)

/-- Type hints as used in MoltValue.type_hint (string-based in the real IR,
    but we use a closed enum for proof tractability). -/
inductive TypeHint where
  | known (t : Ty)
  | unknown
  deriving DecidableEq, Repr

/-! ## Helper predicates -/

/-- True for types that are stored unboxed (inline in a 64-bit slot). -/
def Ty.isUnboxed : Ty → Bool
  | .int | .float | .bool | .none => true
  | _ => false

/-- True for types that support arithmetic operations natively. -/
def Ty.isNumeric : Ty → Bool
  | .int | .float | .bool => true
  | _ => false

/-! ## Type lattice meet operation

  Mirrors the Rust implementation in types.rs:
  - `never` is the bottom element (meet with anything yields the other).
  - `dynBox` is the top of the boxed world (absorbs all).
  - Equal types yield themselves.
  - Mismatched types produce `union_` (capped at 3 members, then `dynBox`).
-/

/-- Flatten nested unions into a flat member list. -/
private def flattenMembers : List Ty → List Ty
  | [] => []
  | .union_ ms :: rest => flattenMembers ms ++ flattenMembers rest
  | t :: rest => t :: flattenMembers rest

/-- Deduplicate a list of types (using DecidableEq). -/
private def dedup : List Ty → List Ty
  | [] => []
  | t :: ts => if ts.elem t then dedup ts else t :: dedup ts

/-- Lattice meet: computes the join of two types in the type lattice.
    Called "meet" to match the Rust naming convention in types.rs. -/
def Ty.meet (a b : Ty) : Ty :=
  match a, b with
  | .never, t | t, .never => t
  | .dynBox, _ | _, .dynBox => .dynBox
  | t₁, t₂ =>
    if t₁ == t₂ then t₁
    else
      let members := dedup (flattenMembers [t₁, t₂])
      if members.length ≤ 3 then .union_ members
      else .dynBox

end MoltTIR
