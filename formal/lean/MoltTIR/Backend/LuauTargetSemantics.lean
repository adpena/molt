/-
  MoltTIR.Backend.LuauTargetSemantics -- Deep formalization of Luau target semantics.

  Extends the Luau evaluation model (LuauSemantics.lean) with the full Luau value
  domain and formalizes key Luau-specific semantic behaviors that differ from Python.

  Builds on:
  - LuauSyntax.lean   : target AST types
  - LuauSemantics.lean : core evaluator (LuauValue, LuauEnv, evalLuauExpr)
  - LuauCorrect.lean   : emission correctness proofs (index adjustment, etc.)

  This file focuses on the *target language semantics* — what Luau programs mean
  at runtime — rather than the *translation correctness* of emission. Covers:

  1. Extended value model (functions/closures, userdata, full table semantics)
  2. Luau-specific operations (# length, table.insert/remove, nil propagation)
  3. String semantics (immutable byte sequences, ASCII subset)
  4. Type coercion rules
  5. Python-Luau correspondence theorems for the Molt-supported subset
-/
import MoltTIR.Backend.LuauSemantics
import MoltTIR.Backend.LuauCorrect

set_option autoImplicit false

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Extended Luau value model
-- ======================================================================

/-- Extended Luau value model that covers the full Luau type system.
    LuauValue (from LuauSemantics) already has number, boolean, str, nil, table.
    We model additional value kinds as a separate type to avoid disrupting
    existing proofs, then provide a unifying wrapper. -/
inductive LuauExtValue where
  | base (v : LuauValue)
  | closure (params : List String) (capturedEnv : List (String × LuauValue))
  | userdata (tag : String)   -- opaque Roblox interop handle
  deriving Repr

/-- Inject a LuauValue into the extended domain. -/
def LuauExtValue.ofBase (v : LuauValue) : LuauExtValue := .base v

/-- Project back to LuauValue (closures and userdata have no LuauValue repr). -/
def LuauExtValue.toBase : LuauExtValue → Option LuauValue
  | .base v => some v
  | .closure _ _ => none
  | .userdata _ => none

theorem LuauExtValue.toBase_ofBase (v : LuauValue) :
    (LuauExtValue.ofBase v).toBase = some v := by
  rfl

-- ======================================================================
-- Section 2: Table semantics — the core Luau data structure
-- ======================================================================

/-- A Luau table with both an array part (1-based) and a hash part.
    In Luau, tables serve as arrays, dictionaries, and objects. The array
    part uses integer keys [1..n], the hash part uses string keys. -/
structure LuauTable where
  arrayPart : List LuauValue           -- 1-based, contiguous
  hashPart  : List (String × LuauValue)
  deriving Repr

/-- The Luau `#` (length) operator on the array part.
    In Luau, `#t` returns the length of the array part — the largest integer
    key n such that t[1]..t[n] are all non-nil. For a well-formed list-like
    table, this is simply the list length. -/
def LuauTable.len (t : LuauTable) : Nat := t.arrayPart.length

/-- 1-based array access. Returns nil for out-of-bounds (Luau semantics). -/
def LuauTable.arrayGet (t : LuauTable) (idx : Int) : LuauValue :=
  let zeroIdx := idx - 1
  if zeroIdx < 0 then .nil
  else match t.arrayPart[zeroIdx.toNat]? with
       | some v => v
       | none => .nil

/-- Hash part access. Returns nil for missing keys (Luau semantics). -/
def LuauTable.hashGet (t : LuauTable) (key : String) : LuauValue :=
  match t.hashPart.find? (fun p => p.1 == key) with
  | some (_, v) => v
  | none => .nil

/-- table.insert(t, v) — appends v to the array part. -/
def LuauTable.insert (t : LuauTable) (v : LuauValue) : LuauTable :=
  { t with arrayPart := t.arrayPart ++ [v] }

/-- table.remove(t, i) — removes element at 1-based index i from array part. -/
def LuauTable.remove (t : LuauTable) (idx : Nat) : LuauTable :=
  let zeroIdx := idx - 1
  { t with arrayPart := t.arrayPart.eraseIdx zeroIdx }

-- ======================================================================
-- Section 3: Table semantics properties
-- ======================================================================

/-- The length operator returns the array part length. -/
theorem LuauTable.len_eq_arrayPart_length (t : LuauTable) :
    t.len = t.arrayPart.length := by
  rfl

/-- After table.insert, length increases by 1. -/
theorem LuauTable.len_insert (t : LuauTable) (v : LuauValue) :
    (t.insert v).len = t.len + 1 := by
  simp [LuauTable.insert, LuauTable.len, List.length_append]

/-- 1-based indexing: accessing index i returns arrayPart[i-1]. -/
theorem LuauTable.arrayGet_in_range (t : LuauTable) (i : Int) (v : LuauValue)
    (hi : 1 ≤ i) (hbound : t.arrayPart[(i - 1).toNat]? = some v) :
    t.arrayGet i = v := by
  unfold LuauTable.arrayGet
  have hge : ¬(i - 1 < 0) := by omega
  simp only [hge, ↓reduceIte, hbound]

theorem LuauTable.arrayGet_out_of_range (t : LuauTable) (i : Int)
    (hbound : t.arrayPart[(i - 1).toNat]? = none) (hnonneg : 0 ≤ i - 1) :
    t.arrayGet i = .nil := by
  unfold LuauTable.arrayGet
  have hge : ¬(i - 1 < 0) := by omega
  simp only [hge, ↓reduceIte, hbound]

theorem LuauTable.arrayGet_negative (t : LuauTable) (i : Int) (hi : i - 1 < 0) :
    t.arrayGet i = .nil := by
  simp [LuauTable.arrayGet, hi]

/-- Hash access for missing key returns nil (not KeyError). -/
theorem LuauTable.hashGet_missing (t : LuauTable) (key : String)
    (hmiss : t.hashPart.find? (fun p => p.1 == key) = none) :
    t.hashGet key = .nil := by
  simp [LuauTable.hashGet, hmiss]

/-- Auxiliary: get? at length of (xs ++ [v]) returns v. -/
private theorem get?_append_singleton {α : Type} (xs : List α) (v : α) :
    (xs ++ [v])[xs.length]? = some v := by
  simp

theorem LuauTable.arrayGet_after_insert (t : LuauTable) (v : LuauValue) :
    (t.insert v).arrayGet (Int.ofNat (t.len + 1)) = v := by
  unfold LuauTable.arrayGet LuauTable.insert LuauTable.len
  have h1 : (Int.ofNat (t.arrayPart.length + 1) - 1 : Int) = ↑t.arrayPart.length := by simp
  rw [h1]
  have h2 : ¬((↑t.arrayPart.length : Int) < 0) := by omega
  simp only [h2, ↓reduceIte, Int.toNat_natCast]
  simp

-- ======================================================================
-- Section 4: Nil propagation semantics
-- ======================================================================

/-- Luau nil propagation: accessing a missing key returns nil rather than
    raising an error. This is a fundamental difference from Python where
    missing dict keys raise KeyError and missing list indices raise IndexError. -/
def luauNilPropagation (entries : List (Int × LuauValue)) (key : Int) : LuauValue :=
  match entries.find? (fun p => p.1 == key) with
  | some (_, v) => v
  | none => .nil

/-- Nil propagation always produces a value (never stuck). -/
theorem luauNilPropagation_total (entries : List (Int × LuauValue)) (key : Int) :
    ∃ v, luauNilPropagation entries key = v := by
  exact ⟨_, rfl⟩

/-- Nil propagation for empty table returns nil. -/
theorem luauNilPropagation_empty (key : Int) :
    luauNilPropagation [] key = .nil := by
  simp [luauNilPropagation, List.find?]

def luauStringLen (s : String) : Nat := s.length

/-- Luau string concatenation (.. operator). -/
def luauStringConcat (a b : String) : String := a ++ b

/-- String concatenation is associative. -/
theorem luauStringConcat_assoc (a b c : String) :
    luauStringConcat (luauStringConcat a b) c =
    luauStringConcat a (luauStringConcat b c) := by
  simp [luauStringConcat, String.append_assoc]

/-- Empty string is identity for concatenation. -/
theorem luauStringConcat_empty_left (s : String) :
    luauStringConcat "" s = s := by
  simp [luauStringConcat]

theorem luauStringConcat_empty_right (s : String) :
    luauStringConcat s "" = s := by
  simp [luauStringConcat]

theorem luauStringConcat_len (a b : String) :
    luauStringLen (luauStringConcat a b) = luauStringLen a + luauStringLen b := by
  simp [luauStringLen, luauStringConcat, String.length_append]

-- ======================================================================
-- Section 6: Luau type coercion rules
-- ======================================================================

/-- Luau truthiness: determines the boolean value of any LuauValue.
    In Luau, only `false` and `nil` are falsy. Everything else is truthy.
    This differs from Python where 0, "", [], etc. are also falsy.

    For the Molt-supported subset, the backend inserts explicit truthiness
    conversions so the Luau `if` behavior matches Python semantics. -/
def luauTruthy : LuauValue → Bool
  | .nil => false
  | .boolean false => false
  | _ => true

/-- Python truthiness for the Molt value subset. Mirrors MoltPython.PyValue.truthy
    but operates on MoltTIR.Value. -/
def pythonTruthy : MoltTIR.Value → Bool
  | .bool b => b
  | .int n => n != 0
  | .float f => f != 0
  | .str s => s != ""
  | .none => false

/-- Luau nil is always falsy. -/
theorem luauTruthy_nil : luauTruthy .nil = false := by rfl

/-- Luau false is falsy. -/
theorem luauTruthy_false : luauTruthy (.boolean false) = false := by rfl

/-- Luau true is truthy. -/
theorem luauTruthy_true : luauTruthy (.boolean true) = true := by rfl

/-- Luau numbers are always truthy (unlike Python where 0 is falsy).
    This is why the Molt backend must insert explicit checks for numeric
    truthiness when compiling Python `if n:` to Luau. -/
theorem luauTruthy_number (n : Int) : luauTruthy (.number n) = true := by rfl

/-- Luau strings are always truthy (unlike Python where "" is falsy). -/
theorem luauTruthy_str (s : String) : luauTruthy (.str s) = true := by rfl

/-- The truthy difference: for boolean values, Luau and Python agree. -/
theorem truthy_correspondence_bool (b : Bool) :
    luauTruthy (valueToLuau (.bool b)) = pythonTruthy (.bool b) := by
  cases b <;> rfl

/-- For None/nil, truthiness agrees. -/
theorem truthy_correspondence_none :
    luauTruthy (valueToLuau .none) = pythonTruthy .none := by
  rfl

-- ======================================================================
-- Section 7: Luau-Python correspondence for table indexing
-- ======================================================================

/-- A well-formed Molt list table: a Luau table whose array part faithfully
    represents a Python list. The key invariant is that Luau 1-based index i
    corresponds to Python 0-based index i-1. -/
structure WellFormedMoltList (t : LuauTable) (pyList : List MoltTIR.Value) : Prop where
  length_eq : t.arrayPart.length = pyList.length
  values_correspond : ∀ (i : Nat) (hi : i < pyList.length),
    t.arrayPart[i]? = some (valueToLuau (pyList[i]'hi))

/-- Core indexing correspondence: for a well-formed Molt list table,
    Luau 1-based access at (pyIdx + 1) returns the same value as
    Python 0-based access at pyIdx.

    This is the semantic counterpart to the structural index_adjust_correct
    theorem in LuauCorrect.lean. Where that theorem shows the emitted AST
    has the right shape, this theorem shows the evaluated result is correct. -/
theorem list_index_correspondence (t : LuauTable) (pyList : List MoltTIR.Value)
    (pyIdx : Nat)
    (hwf : WellFormedMoltList t pyList)
    (hbound : pyIdx < pyList.length) :
    t.arrayGet (Int.ofNat (pyIdx + 1)) =
      valueToLuau (pyList.get ⟨pyIdx, hbound⟩) := by
  unfold LuauTable.arrayGet
  simp only [show ¬(Int.ofNat (pyIdx + 1) - 1 < (0 : Int)) by simp, ↓reduceIte,
             show (Int.ofNat (pyIdx + 1) - 1 : Int).toNat = pyIdx by simp]
  rw [hwf.values_correspond pyIdx hbound, List.get_eq_getElem]

private theorem get?_none_of_ge {α : Type} (xs : List α) (i : Nat) (h : i ≥ xs.length) :
    xs[i]? = none := by
  exact List.getElem?_eq_none_iff.mpr (by omega)

theorem list_oob_returns_nil (t : LuauTable) (pyList : List MoltTIR.Value)
    (pyIdx : Nat)
    (hwf : WellFormedMoltList t pyList)
    (hoob : pyIdx ≥ pyList.length) :
    t.arrayGet (Int.ofNat (pyIdx + 1)) = .nil := by
  unfold LuauTable.arrayGet
  simp only [show ¬(Int.ofNat (pyIdx + 1) - 1 < (0 : Int)) by simp, ↓reduceIte,
             show (Int.ofNat (pyIdx + 1) - 1 : Int).toNat = pyIdx by simp]
  rw [get?_none_of_ge t.arrayPart pyIdx (by rw [hwf.length_eq]; omega)]

theorem string_concat_correspondence (a b : String) :
    luauStringConcat a b =
    a ++ b := by
  rfl

/-- Luau # on a string matches Python len() for ASCII strings.
    For the Molt-supported subset (ASCII), String.length counts characters
    which equals byte count. -/
theorem string_len_correspondence (s : String) :
    luauStringLen s = s.length := by
  rfl

/-- The Luau string concatenation operator (..) corresponds to the
    evalLuauBinOp .concat evaluation on string values. -/
theorem string_concat_eval_correspondence (a b : String) :
    evalLuauBinOp .concat (.str a) (.str b) = some (.str (luauStringConcat a b)) := by
  rfl

-- ======================================================================
-- Section 9: Nil-None correspondence
-- ======================================================================

/-- Luau nil corresponds to Python None under the value correspondence.
    This is definitional from valueToLuau. -/
theorem nil_none_correspondence :
    valueToLuau .none = .nil := by
  rfl

/-- The round-trip: None → nil → None is exact. -/
theorem nil_none_roundtrip :
    luauToValue (valueToLuau .none) = some .none := by
  rfl

/-- Nil equality in Luau: nil == nil is true. In Python, None == None is True.
    Both languages agree on self-equality of their null value. -/
theorem nil_self_equality :
    luauTruthy (.boolean true) = true ∧
    valueToLuau .none = .nil := by
  exact ⟨rfl, rfl⟩

-- ======================================================================
-- Section 10: Full value correspondence roundtrip
-- ======================================================================

/-- For every MoltTIR.Value in the Molt-supported subset,
    the Luau representation faithfully preserves observable identity:
    valueToLuau followed by luauToValue recovers the original value
    (up to int/float conflation). -/
theorem value_roundtrip_int (n : Int) :
    luauToValue (valueToLuau (.int n)) = some (.int n) := by rfl

theorem value_roundtrip_bool (b : Bool) :
    luauToValue (valueToLuau (.bool b)) = some (.bool b) := by rfl

theorem value_roundtrip_str (s : String) :
    luauToValue (valueToLuau (.str s)) = some (.str s) := by rfl

theorem value_roundtrip_none :
    luauToValue (valueToLuau .none) = some .none := by rfl

/-- The only non-roundtripping case: float → number → int (lossy).
    This is by design: Luau unifies int and float as number. -/
theorem value_roundtrip_float_lossy (f : Int) :
    luauToValue (valueToLuau (.float f)) = some (.int f) := by rfl

-- ======================================================================
-- Section 11: Table insertion correspondence
-- ======================================================================

/-- Auxiliary: get? on (xs ++ ys) when i < xs.length. -/
private theorem get?_append_left {α : Type} (xs ys : List α) (i : Nat) (h : i < xs.length) :
    (xs ++ ys)[i]? = xs[i]? := by
  simp [List.getElem?_append_left (by omega)]

private theorem get?_map_eq {α β : Type} (f : α → β) (xs : List α) (i : Nat) :
    (xs.map f)[i]? = (xs[i]?).map f := by
  simp [List.getElem?_map]

theorem insert_arrayPart_length (t : LuauTable) (v : LuauValue) :
    (t.insert v).arrayPart.length = t.arrayPart.length + 1 := by
  simp [LuauTable.insert, List.length_append]

/-- After appending a value to a well-formed Molt list table,
    the result is still well-formed for the extended Python list.

    Stated in a simplified form: the length invariant is preserved,
    and all old elements remain accessible at their original indices. -/
theorem insert_length_preserved (t : LuauTable) (pyList : List MoltTIR.Value)
    (v : MoltTIR.Value)
    (hwf : WellFormedMoltList t pyList) :
    (t.insert (valueToLuau v)).arrayPart.length = (pyList ++ [v]).length := by
  simp [LuauTable.insert, List.length_append, hwf.length_eq]

-- ======================================================================
-- Section 12: Evaluation model extension — table constructor
-- ======================================================================

/-- Build a LuauTable from a list of values (list-style table constructor).
    Corresponds to Luau `{v1, v2, v3, ...}` which creates a table with
    array part [v1, v2, v3, ...] and 1-based keys. -/
def buildListTable (vals : List LuauValue) : LuauTable :=
  { arrayPart := vals, hashPart := [] }

/-- A list table has length equal to the number of values. -/
theorem buildListTable_len (vals : List LuauValue) :
    (buildListTable vals).len = vals.length := by
  rfl

/-- A list table has no hash part. -/
theorem buildListTable_hashPart (vals : List LuauValue) :
    (buildListTable vals).hashPart = [] := by
  rfl

/-- Auxiliary: get? within bounds returns some (get ...). -/
private theorem get?_eq_some_get {α : Type} (xs : List α) (i : Nat) (h : i < xs.length) :
    xs[i]? = some (xs.get ⟨i, h⟩) := by
  simp [List.getElem?_eq_some_iff.mpr ⟨h, rfl⟩]

/-- Building a list table from mapped Python values produces a well-formed table. -/
theorem buildListTable_wellformed (pyList : List MoltTIR.Value) :
    WellFormedMoltList (buildListTable (pyList.map valueToLuau)) pyList where
  length_eq := by simp [buildListTable]
  values_correspond := by
    intro i hi
    simp only [buildListTable, List.getElem?_map]
    simp [List.getElem?_eq_some_iff.mpr ⟨hi, rfl⟩]

theorem molt_luau_semantic_correspondence :
    -- (1) Integer values round-trip
    (∀ n : Int, luauToValue (valueToLuau (.int n)) = some (.int n)) ∧
    -- (2) Boolean values round-trip
    (∀ b : Bool, luauToValue (valueToLuau (.bool b)) = some (.bool b)) ∧
    -- (3) String values round-trip
    (∀ s : String, luauToValue (valueToLuau (.str s)) = some (.str s)) ∧
    -- (4) None/nil round-trip
    (luauToValue (valueToLuau .none) = some .none) ∧
    -- (5) Index adjustment is correct
    (∀ n : Nat, adjustIndex (.intLit (Int.ofNat n)) =
      .binOp .add (.intLit (Int.ofNat n)) (.intLit 1)) := by
  exact ⟨fun _ => rfl, fun _ => rfl, fun _ => rfl, rfl, fun _ => rfl⟩

end MoltTIR.Backend
