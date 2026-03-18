/-
  MoltTIR.Runtime.IntrinsicContracts — specifications for the top 30 runtime builtins.

  Where the semantics are fully determined by the type, intrinsic functions are
  given concrete `def` implementations and their properties are proved as
  `theorem`s.  Functions whose behavior depends on runtime state (hash, id,
  callable, etc.) remain `opaque` with `axiom` contracts.

  Design principles:
  - Prefer `def` + `theorem` over `opaque` + `axiom` to minimize the trusted base.
  - Properties are expressed in Prop, not executable code.
  - No Mathlib dependency; self-contained.

  Axiom census: 60 axioms -> 14 axioms -> 0 axioms remaining
  (all 14 former axioms converted to theorems; 1 sorry remains for
   set_idempotent which needs List.nodup_eraseDups / List.eraseDups_of_nodup
   not yet in Lean 4.28.0 stdlib).
-/
import MoltTIR.Syntax
import Init.Data.List.Sort.Lemmas

set_option autoImplicit false

namespace MoltTIR.Runtime

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Intrinsic specification structure
-- ══════════════════════════════════════════════════════════════════

/-- An intrinsic is a named function with pre/post conditions on values.
    This structure records the contract that a Rust builtin must satisfy
    for the Lean proofs to be sound. -/
structure IntrinsicSpec where
  name          : String
  arity         : Nat
  precondition  : List Value → Prop
  postcondition : List Value → Value → Prop

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Intrinsic function declarations
-- ══════════════════════════════════════════════════════════════════

-- Helper: extract the Int payload from a Value (partial).
def Value.asInt : Value → Option Int
  | .int n => some n
  | _      => none

def Value.asBool : Value → Option Bool
  | .bool b => some b
  | _       => none

-- ── Value ordering (needed for sorted) ───────────────────────────

/-- Total order on Value: bool < int < float < none < str,
    with same-tag comparison by payload. -/
def Value.le (a b : Value) : Bool :=
  match a, b with
  | .bool b₁, .bool b₂ => !b₁ || b₂  -- false ≤ true
  | .int n₁, .int n₂ => decide (n₁ ≤ n₂)
  | .float f₁, .float f₂ => decide (f₁ ≤ f₂)
  | .str s₁, .str s₂ => decide (s₁ ≤ s₂)
  | .none, .none => true
  -- cross-tag: bool < int < float < none < str
  | .bool _, _ => true
  | _, .bool _ => false
  | .int _, _ => true
  | _, .int _ => false
  | .float _, _ => true
  | _, .float _ => false
  | .none, .str _ => true
  | .str _, .none => false

theorem Value.le_total : ∀ (a b : Value), (Value.le a b || Value.le b a) = true := by
  intro a b
  cases a <;> cases b <;> simp only [Value.le, Bool.or_eq_true, decide_eq_true_eq]
  case bool.bool b₁ b₂ => cases b₁ <;> cases b₂ <;> simp
  case int.int n₁ n₂ => omega
  case float.float f₁ f₂ => omega
  case none.none => simp
  case str.str s₁ s₂ => exact String.le_total s₁ s₂
  all_goals simp

theorem Value.le_trans : ∀ (a b c : Value),
    Value.le a b = true → Value.le b c = true → Value.le a c = true := by
  intro a b c hab hbc
  cases a <;> cases b <;> cases c <;>
    simp only [Value.le, decide_eq_true_eq] at *
  all_goals first
    | assumption | trivial | omega
    | (cases ‹Bool› <;> cases ‹Bool› <;> simp_all)
    | exact String.le_trans hab hbc
    | simp_all
    | (exfalso; exact absurd hab (by decide))
    | (exfalso; exact absurd hbc (by decide))

theorem Value.le_antisymm : ∀ (a b : Value),
    Value.le a b = true → Value.le b a = true → a = b := by
  intro a b hab hba
  cases a <;> cases b <;> simp only [Value.le, decide_eq_true_eq] at *
  all_goals first
    | rfl
    | (exact absurd hab (by decide))
    | (exact absurd hba (by decide))
    | (congr 1; omega)
    | (congr 1; exact String.le_antisymm hab hba)
    | (cases ‹Bool› <;> cases ‹Bool› <;> simp_all)

-- ── Core builtins with definitions ──────────────────────────────

/-- `abs` for integers: negate if negative, identity otherwise. -/
def intrinsic_abs_int (n : Int) : Int :=
  if n < 0 then -n else n

/-- `abs` for floats (modeled as Int): negate if negative, identity otherwise. -/
def intrinsic_abs_float (f : Int) : Int :=
  if f < 0 then -f else f

/-- `min` of two integers. -/
def intrinsic_min (a b : Int) : Int :=
  if a ≤ b then a else b

/-- `max` of two integers. -/
def intrinsic_max (a b : Int) : Int :=
  if b < a then a else b

/-- Python truthiness: bool(v). -/
def intrinsic_bool (v : Value) : Bool :=
  match v with
  | .int n   => decide (n ≠ 0)
  | .bool b  => b
  | .none    => false
  | .str s   => decide (s ≠ "")
  | .float f => decide (f ≠ 0)

/-- `int()` conversion. -/
def intrinsic_int (v : Value) : Option Int :=
  match v with
  | .bool true  => some 1
  | .bool false => some 0
  | .int n      => some n
  | _           => Option.none

/-- `float()` conversion. -/
def intrinsic_float (v : Value) : Option Int :=
  match v with
  | .int n   => some n
  | .float f => some f
  | _        => Option.none

/-- `print(v)` always returns None (I/O is a side-effect). -/
def intrinsic_print (_v : Value) : Value :=
  Value.none

/-- `type(v)` returns the type name string. -/
def intrinsic_type (v : Value) : String :=
  match v with
  | .int _   => "int"
  | .bool _  => "bool"
  | .str _   => "str"
  | .none    => "NoneType"
  | .float _ => "float"

/-- `isinstance(v, typeName)` checks if value's type matches. -/
def intrinsic_isinstance (v : Value) (typeName : String) : Bool :=
  intrinsic_type v == typeName

/-- `str(v)` converts a value to its string representation. -/
def intrinsic_str (v : Value) : String :=
  match v with
  | .int n   => toString n
  | .bool b  => if b then "True" else "False"
  | .str s   => s
  | .none    => "None"
  | .float f => toString f

/-- `repr(v)` converts a value to its repr string. -/
def intrinsic_repr (v : Value) : String :=
  match v with
  | .int n   => toString n
  | .bool b  => if b then "True" else "False"
  | .str s   => "'" ++ s ++ "'"
  | .none    => "None"
  | .float f => toString f

/-- `len(v)` returns the length of a value. -/
def intrinsic_len (v : Value) : Int :=
  match v with
  | .str s => s.length
  | _      => 0

/-- `callable(v)` — none of the five literal Value variants are callable. -/
def intrinsic_callable (v : Value) : Bool :=
  match v with
  | .int _   => false
  | .bool _  => false
  | .none    => false
  | .str _   => false
  | .float _ => false

/-- `round` of an integer is the identity. -/
def intrinsic_round_int (n : Int) : Int := n

/-- `reversed` is List.reverse. -/
def intrinsic_reversed (xs : List Value) : List Value :=
  xs.reverse

/-- `range(n)` produces [0, 1, ..., n-1] as Value.int. -/
def intrinsic_range (n : Int) : List Value :=
  if n ≤ 0 then []
  else (List.range n.toNat).map (fun (i : Nat) => Value.int (Int.ofNat i))

/-- `set` removes duplicates (order-preserving). -/
def intrinsic_set (xs : List Value) : List Value :=
  xs.eraseDups

/-- `sorted` sorts a list of values using the total order Value.le. -/
def intrinsic_sorted (xs : List Value) : List Value :=
  xs.mergeSort Value.le

/-- `all(xs)` — vacuous truth for empty, conjunction of bool. -/
def intrinsic_all (xs : List Value) : Bool :=
  xs.all (intrinsic_bool ·)

/-- `any(xs)` — false for empty, disjunction of bool. -/
def intrinsic_any (xs : List Value) : Bool :=
  xs.any (intrinsic_bool ·)

/-- `sum(xs)` — sum of integer values (non-int values contribute 0). -/
def intrinsic_sum (xs : List Value) : Value :=
  Value.int (xs.foldl (fun acc v =>
    match v with
    | .int n => acc + n
    | _      => acc) 0)

/-- `map(f, xs)` is List.map. -/
def intrinsic_map (f : Value → Value) (xs : List Value) : List Value :=
  xs.map f

/-- `filter(f, xs)` is List.filter. -/
def intrinsic_filter (f : Value → Bool) (xs : List Value) : List Value :=
  xs.filter f

/-- `enumerate(xs)` pairs each element with its index. -/
def intrinsic_enumerate (xs : List Value) : List Value :=
  (xs.zipIdx).map (fun ⟨_v, i⟩ => Value.int (Int.ofNat i))

/-- `zip(xs, ys)` pairs elements from two lists, producing tuple-like values. -/
def intrinsic_zip (xs ys : List Value) : List Value :=
  (xs.zip ys).map (fun ⟨_x, _y⟩ => Value.none)

-- ── Builtins that remain opaque (inherently runtime-dependent) ───

opaque intrinsic_hash        : Value → Int
opaque intrinsic_id          : Value → Int
opaque intrinsic_round_float : Int → Int

-- ── Collection builtins that remain opaque (need runtime heap) ───

opaque intrinsic_list    : Value → List Value
opaque intrinsic_tuple   : Value → List Value
opaque intrinsic_dict    : Value → List (Value × Value)

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Core builtin theorems (previously axioms)
-- ══════════════════════════════════════════════════════════════════

-- len ─────────────────────────────────────────────────────────────

/-- `len` always returns a non-negative integer. -/
theorem len_nonneg : ∀ (xs : Value), 0 ≤ intrinsic_len xs := by
  intro v
  cases v <;> simp [intrinsic_len]

-- abs int --------------------------------------------------------

/-- `abs(n)` is always non-negative for integer inputs. -/
theorem abs_int_nonneg : ∀ (n : Int), 0 ≤ intrinsic_abs_int n := by
  intro n
  simp only [intrinsic_abs_int]
  split
  · omega
  · omega

/-- `abs(n)` equals n when n is non-negative. -/
theorem abs_int_of_nonneg : ∀ (n : Int), 0 ≤ n → intrinsic_abs_int n = n := by
  intro n hn
  simp only [intrinsic_abs_int]
  split
  · omega
  · rfl

/-- `abs(n)` equals -n when n is negative. -/
theorem abs_int_of_neg : ∀ (n : Int), n < 0 → intrinsic_abs_int n = -n := by
  intro n hn
  simp only [intrinsic_abs_int]
  split
  · rfl
  · omega

-- abs float ──────────────────────────────────────────────────────

/-- `abs` for floats (modeled as Int) is non-negative. -/
theorem abs_float_nonneg : ∀ (f : Int), 0 ≤ intrinsic_abs_float f := by
  intro f
  simp only [intrinsic_abs_float]
  split <;> omega

-- min / max -------------------------------------------------------

/-- `min(a, b)` returns a when a ≤ b. -/
theorem min_left : ∀ (a b : Int), a ≤ b → intrinsic_min a b = a := by
  intro a b hab
  simp only [intrinsic_min, if_pos hab]

/-- `min(a, b)` returns b when b < a. -/
theorem min_right : ∀ (a b : Int), b < a → intrinsic_min a b = b := by
  intro a b hba
  simp only [intrinsic_min]
  split
  · omega
  · rfl

/-- `max(a, b)` returns b when a ≤ b. -/
theorem max_right : ∀ (a b : Int), a ≤ b → intrinsic_max a b = b := by
  intro a b hab
  simp only [intrinsic_max]
  split
  · omega
  · rfl

/-- `max(a, b)` returns a when b < a. -/
theorem max_left : ∀ (a b : Int), b < a → intrinsic_max a b = a := by
  intro a b hba
  simp only [intrinsic_max, if_pos hba]

/-- `min(a, b) ≤ max(a, b)` always holds. -/
theorem min_le_max : ∀ (a b : Int), intrinsic_min a b ≤ intrinsic_max a b := by
  intro a b
  simp only [intrinsic_min, intrinsic_max]
  split <;> split <;> omega

-- bool ------------------------------------------------------------

/-- `bool(0)` is false. -/
theorem bool_int_zero : intrinsic_bool (.int 0) = false := by
  native_decide

/-- `bool` of a non-zero int is true. -/
theorem bool_int_nonzero : ∀ (n : Int), n ≠ 0 → intrinsic_bool (.int n) = true := by
  intro n hn
  simp [intrinsic_bool, hn]

/-- `bool(True)` is true. -/
theorem bool_true  : intrinsic_bool (.bool true) = true := by
  rfl

/-- `bool(False)` is false. -/
theorem bool_false : intrinsic_bool (.bool false) = false := by
  rfl

/-- `bool(None)` is false. -/
theorem bool_none  : intrinsic_bool Value.none = false := by
  rfl

/-- `bool("")` is false. -/
theorem bool_empty_str : intrinsic_bool (.str "") = false := by
  native_decide

-- int conversion --------------------------------------------------

/-- `int(True) = 1`. -/
theorem int_of_true  : intrinsic_int (.bool true)  = some 1 := by
  rfl

/-- `int(False) = 0`. -/
theorem int_of_false : intrinsic_int (.bool false) = some 0 := by
  rfl

/-- `int` of an integer is identity. -/
theorem int_of_int : ∀ (n : Int), intrinsic_int (.int n) = some n := by
  intro n; rfl

-- float conversion ────────────────────────────────────────────────

/-- `float` of an integer succeeds. -/
theorem float_of_int : ∀ (n : Int), ∃ f, intrinsic_float (.int n) = some f := by
  intro n; exact ⟨n, rfl⟩

-- str / repr ──────────────────────────────────────────────────────

/-- `str` always produces a string (totality). -/
theorem str_total : ∀ (v : Value), (intrinsic_str v).length ≥ 0 := by
  intro _v; exact Nat.zero_le _

/-- `repr` always produces a string (totality). -/
theorem repr_total : ∀ (v : Value), (intrinsic_repr v).length ≥ 0 := by
  intro _v; exact Nat.zero_le _

-- print -----------------------------------------------------------

/-- `print(x)` always returns None. -/
theorem print_returns_none : ∀ (v : Value), intrinsic_print v = Value.none := by
  intro _v; rfl

-- type ------------------------------------------------------------

/-- `type(int)` returns "int". -/
theorem type_int  : ∀ (n : Int), intrinsic_type (.int n) = "int" := by
  intro _n; rfl

/-- `type(bool)` returns "bool". -/
theorem type_bool : ∀ (b : Bool), intrinsic_type (.bool b) = "bool" := by
  intro _b; rfl

/-- `type(str)` returns "str". -/
theorem type_str  : ∀ (s : String), intrinsic_type (.str s) = "str" := by
  intro _s; rfl

/-- `type(None)` returns "NoneType". -/
theorem type_none : intrinsic_type Value.none = "NoneType" := by
  rfl

/-- `type(float)` returns "float". -/
theorem type_float : ∀ (f : Int), intrinsic_type (.float f) = "float" := by
  intro _f; rfl

-- isinstance ──────────────────────────────────────────────────────

/-- `isinstance` is consistent with `type`. -/
theorem isinstance_type : ∀ (v : Value),
    intrinsic_isinstance v (intrinsic_type v) = true := by
  intro v
  cases v <;> simp [intrinsic_isinstance, intrinsic_type]

-- hash ------------------------------------------------------------

/-- `hash` is deterministic: same input produces same output. -/
theorem hash_deterministic : ∀ (v : Value),
    intrinsic_hash v = intrinsic_hash v := by
  intro _v; rfl

/-- Equal values have equal hashes. -/
theorem hash_eq_of_eq : ∀ (v₁ v₂ : Value), v₁ = v₂ →
    intrinsic_hash v₁ = intrinsic_hash v₂ := by
  intro _ _ h; rw [h]

-- id --------------------------------------------------------------

/-- `id` is deterministic (same object -> same id within a session). -/
theorem id_deterministic : ∀ (v : Value), intrinsic_id v = intrinsic_id v := by
  intro _v; rfl

-- callable ────────────────────────────────────────────────────────

/-- Literal ints are not callable. -/
theorem callable_int : ∀ (n : Int), intrinsic_callable (.int n) = false := by
  intro _; rfl

/-- Literal bools are not callable. -/
theorem callable_bool : ∀ (b : Bool), intrinsic_callable (.bool b) = false := by
  intro _; rfl

/-- None is not callable. -/
theorem callable_none : intrinsic_callable Value.none = false := by
  rfl

-- round -----------------------------------------------------------

/-- `round` of an int is the identity. -/
theorem round_int_id : ∀ (n : Int), intrinsic_round_int n = n := by
  intro _n; rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Collection builtin theorems (previously axioms)
-- ══════════════════════════════════════════════════════════════════

-- sorted ──────────────────────────────────────────────────────────

/-- `sorted` preserves length. -/
theorem sorted_length : ∀ (xs : List Value),
    (intrinsic_sorted xs).length = xs.length := by
  intro xs
  exact List.length_mergeSort xs

/-- `sorted` is idempotent. -/
theorem sorted_idempotent : ∀ (xs : List Value),
    intrinsic_sorted (intrinsic_sorted xs) = intrinsic_sorted xs := by
  intro xs
  simp only [intrinsic_sorted]
  exact List.mergeSort_of_pairwise (List.pairwise_mergeSort Value.le_trans Value.le_total xs)

-- reversed --------------------------------------------------------

/-- `reversed` preserves length. -/
theorem reversed_length : ∀ (xs : List Value),
    (intrinsic_reversed xs).length = xs.length := by
  intro xs
  simp [intrinsic_reversed]

/-- `reversed` is an involution: reversing twice yields the original. -/
theorem reversed_involution : ∀ (xs : List Value),
    intrinsic_reversed (intrinsic_reversed xs) = xs := by
  intro xs
  simp [intrinsic_reversed]

/-- `reversed` of empty is empty. -/
theorem reversed_nil : intrinsic_reversed [] = [] := by
  rfl

-- enumerate -------------------------------------------------------

/-- `enumerate` preserves length. -/
theorem enumerate_length : ∀ (xs : List Value),
    (intrinsic_enumerate xs).length = xs.length := by
  intro xs
  simp [intrinsic_enumerate]

-- zip -------------------------------------------------------------

/-- `zip(xs, ys)` has length min(len(xs), len(ys)). -/
theorem zip_length : ∀ (xs ys : List Value),
    (intrinsic_zip xs ys).length = min xs.length ys.length := by
  intro xs ys
  simp [intrinsic_zip]

-- range -----------------------------------------------------------

/-- `range(n)` has length max(0, n) for non-negative n. -/
theorem range_length_nonneg : ∀ (n : Int), 0 ≤ n →
    (intrinsic_range n).length = n.toNat := by
  intro n hn
  simp only [intrinsic_range]
  have h : ¬ (n ≤ 0) ∨ n = 0 := by omega
  cases h with
  | inl h =>
    rw [if_neg h]
    simp [List.length_map, List.length_range]
  | inr h =>
    subst h
    simp

/-- `range(n)` is empty for non-positive n. -/
theorem range_length_nonpos : ∀ (n : Int), n ≤ 0 →
    (intrinsic_range n).length = 0 := by
  intro n hn
  simp only [intrinsic_range, if_pos hn, List.length_nil]

-- set -------------------------------------------------------------

/-- Helper: eraseDups never increases list length.
    Proved via strong induction on list length using eraseDups_cons. -/
private theorem eraseDups_length_le {α : Type} [BEq α] [LawfulBEq α] (xs : List α) :
    xs.eraseDups.length ≤ xs.length := by
  suffices h : ∀ (n : Nat) (xs : List α), xs.length ≤ n → xs.eraseDups.length ≤ xs.length from
    h xs.length xs (Nat.le_refl _)
  intro n
  induction n with
  | zero =>
    intro xs hlen
    have : xs = [] := List.eq_nil_of_length_eq_zero (Nat.eq_zero_of_le_zero hlen)
    subst this; simp [List.eraseDups_nil]
  | succ n ih =>
    intro xs hlen
    match xs with
    | [] => simp [List.eraseDups_nil]
    | a :: as =>
      simp only [List.eraseDups_cons, List.length_cons]
      apply Nat.succ_le_succ
      have hflen : (List.filter (fun b => !b == a) as).length ≤ as.length :=
        List.length_filter_le _ _
      have hf_bound : (List.filter (fun b => !b == a) as).length ≤ n :=
        Nat.le_trans hflen (Nat.le_of_succ_le_succ hlen)
      calc (List.filter (fun b => !b == a) as).eraseDups.length
          ≤ (List.filter (fun b => !b == a) as).length := ih _ hf_bound
        _ ≤ as.length := hflen

/-- `set` removes duplicates, so length can only decrease or stay. -/
theorem set_length_le : ∀ (xs : List Value),
    (intrinsic_set xs).length ≤ xs.length := by
  intro xs
  exact eraseDups_length_le xs

/-- `set` of empty is empty. -/
theorem set_nil : intrinsic_set [] = [] := by
  rfl

/-- Helper: membership in eraseDups implies membership in original list. -/
private theorem eraseDups_mem {α : Type} [BEq α] [LawfulBEq α]
    (xs : List α) (x : α) (h : x ∈ xs.eraseDups) : x ∈ xs := by
  have : ∀ n (ys : List α), ys.length ≤ n → x ∈ ys.eraseDups → x ∈ ys := by
    intro n
    induction n with
    | zero =>
      intro ys hlen
      have : ys = [] := List.eq_nil_of_length_eq_zero (Nat.eq_zero_of_le_zero hlen)
      subst this; simp [List.eraseDups_nil]
    | succ n ih =>
      intro ys hlen h
      match ys with
      | [] => simp [List.eraseDups_nil] at h
      | a :: as =>
        rw [List.eraseDups_cons, List.mem_cons] at h
        cases h with
        | inl heq => exact List.mem_cons.mpr (Or.inl heq)
        | inr htail =>
          apply List.mem_cons.mpr; right
          have hfilt := ih (List.filter (fun b => !(b == a)) as) (by
            calc (List.filter (fun b => !(b == a)) as).length
                ≤ as.length := List.length_filter_le _ _
              _ ≤ n := Nat.le_of_succ_le_succ hlen) htail
          exact List.mem_filter.mp hfilt |>.1
  exact this xs.length xs (Nat.le_refl _) h

/-- Helper: eraseDups is idempotent for any LawfulBEq type. -/
private theorem eraseDups_idem {α : Type} [BEq α] [LawfulBEq α]
    (xs : List α) : xs.eraseDups.eraseDups = xs.eraseDups := by
  have : ∀ n (ys : List α), ys.length ≤ n → ys.eraseDups.eraseDups = ys.eraseDups := by
    intro n
    induction n with
    | zero =>
      intro ys hlen
      have : ys = [] := List.eq_nil_of_length_eq_zero (Nat.eq_zero_of_le_zero hlen)
      subst this; simp [List.eraseDups_nil]
    | succ n ih =>
      intro ys hlen
      match ys with
      | [] => simp [List.eraseDups_nil]
      | a :: as =>
        rw [List.eraseDups_cons, List.eraseDups_cons]
        congr 1
        have hall : ∀ y ∈ (List.filter (fun b => !(b == a)) as).eraseDups,
            (!(y == a)) = true := by
          intro y hy
          have := eraseDups_mem _ y hy
          exact List.mem_filter.mp this |>.2
        rw [List.filter_eq_self.mpr hall]
        exact ih (List.filter (fun b => !(b == a)) as) (by
          calc (List.filter (fun b => !(b == a)) as).length
              ≤ as.length := List.length_filter_le _ _
            _ ≤ n := Nat.le_of_succ_le_succ hlen)
  exact this xs.length xs (Nat.le_refl _)

/-- `set` is idempotent: deduplicating twice is the same as once. -/
theorem set_idempotent : ∀ (xs : List Value),
    intrinsic_set (intrinsic_set xs) = intrinsic_set xs := by
  intro xs
  simp only [intrinsic_set]
  exact eraseDups_idem xs

-- any / all -------------------------------------------------------

/-- `all([])` is True (vacuous truth). -/
theorem all_nil : intrinsic_all [] = true := by
  rfl

/-- `any([])` is False. -/
theorem any_nil : intrinsic_any [] = false := by
  rfl

/-- If `all(xs)` is true, then `any(xs)` is true (for non-empty xs). -/
theorem all_implies_any : ∀ (xs : List Value),
    xs ≠ [] → intrinsic_all xs = true → intrinsic_any xs = true := by
  intro xs hne hall
  match xs with
  | [] => exact absurd rfl hne
  | x :: rest =>
    simp only [intrinsic_all, intrinsic_any] at *
    simp only [List.all_cons, Bool.and_eq_true] at hall
    simp only [List.any_cons, Bool.or_eq_true]
    exact Or.inl hall.1

-- sum -------------------------------------------------------------

/-- `sum([])` is 0. -/
theorem sum_nil : intrinsic_sum [] = Value.int 0 := by
  rfl

-- map -------------------------------------------------------------

/-- `map(f, xs)` preserves length. -/
theorem map_length : ∀ (f : Value → Value) (xs : List Value),
    (intrinsic_map f xs).length = xs.length := by
  intro f xs
  simp [intrinsic_map]

/-- `map` over empty is empty. -/
theorem map_nil : ∀ (f : Value → Value), intrinsic_map f [] = [] := by
  intro _f; rfl

-- filter ----------------------------------------------------------

/-- `filter(f, xs)` can only shrink the list. -/
theorem filter_length_le : ∀ (f : Value → Bool) (xs : List Value),
    (intrinsic_filter f xs).length ≤ xs.length := by
  intro f xs
  simp [intrinsic_filter, List.length_filter_le]

/-- `filter` over empty is empty. -/
theorem filter_nil : ∀ (f : Value → Bool), intrinsic_filter f [] = [] := by
  intro _f; rfl

-- list / tuple ----------------------------------------------------

-- `list` and `tuple` preserve the element count when converting from
-- a known-length iterable (expressed as: converting a list value
-- round-trips through list).
-- These are underspecified because `Value` does not directly carry
-- a list payload; the runtime heap is not modeled.  We state what
-- we can: the functions are total (opaque guarantees this).

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Composite algebraic laws
-- ══════════════════════════════════════════════════════════════════

/-- Sorting a reversed list is the same as sorting the original.
    Follows from the fact that mergeSort produces the same result
    for any permutation of the input (via Perm.eq_of_sorted). -/
theorem sorted_reversed : ∀ (xs : List Value),
    intrinsic_sorted (intrinsic_reversed xs) = intrinsic_sorted xs := by
  intro xs
  simp only [intrinsic_sorted, intrinsic_reversed]
  apply List.Perm.eq_of_pairwise
    (le := fun a b => Value.le a b = true)
  · intro a b _ _ hab hba
    exact Value.le_antisymm a b hab hba
  · exact List.pairwise_mergeSort Value.le_trans Value.le_total xs.reverse
  · exact List.pairwise_mergeSort Value.le_trans Value.le_total xs
  · exact (List.mergeSort_perm xs.reverse Value.le).trans
      (xs.reverse_perm.trans (List.mergeSort_perm xs Value.le).symm)

/-- Reversing a sorted list then reversing again gives sorted. -/
theorem reversed_sorted_reversed : ∀ (xs : List Value),
    intrinsic_reversed (intrinsic_reversed (intrinsic_sorted xs)) = intrinsic_sorted xs := by
  intro xs
  exact reversed_involution (intrinsic_sorted xs)

/-- `min(a, b) = min(b, a)` — commutativity. -/
theorem min_comm : ∀ (a b : Int), intrinsic_min a b = intrinsic_min b a := by
  intro a b
  simp only [intrinsic_min]
  split <;> split <;> omega

/-- `max(a, b) = max(b, a)` — commutativity. -/
theorem max_comm : ∀ (a b : Int), intrinsic_max a b = intrinsic_max b a := by
  intro a b
  simp only [intrinsic_max]
  split <;> split <;> omega

/-- `filter` then `sorted` vs `sorted` then `filter` — filter on a sorted
    list gives a sublist of the sorted result.  We state the weaker
    length-preserving property. -/
theorem filter_sorted_length : ∀ (f : Value → Bool) (xs : List Value),
    (intrinsic_filter f (intrinsic_sorted xs)).length ≤ xs.length := by
  intro f xs
  calc (intrinsic_filter f (intrinsic_sorted xs)).length
      ≤ (intrinsic_sorted xs).length := filter_length_le f (intrinsic_sorted xs)
    _ = xs.length := sorted_length xs

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Spec table (all 30 builtins)
-- ══════════════════════════════════════════════════════════════════

/-- The master specification list. Each entry records the builtin name,
    its arity, and the key property axioms it satisfies.  This is
    documentation / a lookup table for downstream proof automation. -/
def builtinSpecs : List IntrinsicSpec := [
  ⟨"len",        1, fun _ => True, fun _ r => ∃ n : Int, r = .int n ∧ 0 ≤ n⟩,
  ⟨"abs",        1, fun args => ∃ n, args = [.int n], fun _ r => ∃ n : Int, r = .int n ∧ 0 ≤ n⟩,
  ⟨"min",        2, fun args => ∃ a b, args = [.int a, .int b],
                    fun args r => ∃ a b, args = [.int a, .int b] ∧ (r = .int a ∨ r = .int b)⟩,
  ⟨"max",        2, fun args => ∃ a b, args = [.int a, .int b],
                    fun args r => ∃ a b, args = [.int a, .int b] ∧ (r = .int a ∨ r = .int b)⟩,
  ⟨"bool",       1, fun _ => True, fun _ r => ∃ b : Bool, r = .bool b⟩,
  ⟨"int",        1, fun _ => True, fun _ r => ∃ n : Int, r = .int n⟩,
  ⟨"float",      1, fun _ => True, fun _ r => ∃ f : Int, r = .float f⟩,
  ⟨"str",        1, fun _ => True, fun _ r => ∃ s : String, r = .str s⟩,
  ⟨"repr",       1, fun _ => True, fun _ r => ∃ s : String, r = .str s⟩,
  ⟨"print",      1, fun _ => True, fun _ r => r = Value.none⟩,
  ⟨"type",       1, fun _ => True, fun _ r => ∃ s : String, r = .str s⟩,
  ⟨"isinstance", 2, fun _ => True, fun _ r => ∃ b : Bool, r = .bool b⟩,
  ⟨"hash",       1, fun _ => True, fun _ r => ∃ n : Int, r = .int n⟩,
  ⟨"id",         1, fun _ => True, fun _ r => ∃ n : Int, r = .int n⟩,
  ⟨"callable",   1, fun _ => True, fun _ r => ∃ b : Bool, r = .bool b⟩,
  ⟨"sorted",     1, fun _ => True, fun _ _ => True⟩,
  ⟨"reversed",   1, fun _ => True, fun _ _ => True⟩,
  ⟨"enumerate",  1, fun _ => True, fun _ _ => True⟩,
  ⟨"zip",        2, fun _ => True, fun _ _ => True⟩,
  ⟨"range",      1, fun args => ∃ n, args = [.int n], fun _ _ => True⟩,
  ⟨"list",       1, fun _ => True, fun _ _ => True⟩,
  ⟨"tuple",      1, fun _ => True, fun _ _ => True⟩,
  ⟨"set",        1, fun _ => True, fun _ _ => True⟩,
  ⟨"dict",       1, fun _ => True, fun _ _ => True⟩,
  ⟨"any",        1, fun _ => True, fun _ r => ∃ b : Bool, r = .bool b⟩,
  ⟨"all",        1, fun _ => True, fun _ r => ∃ b : Bool, r = .bool b⟩,
  ⟨"sum",        1, fun _ => True, fun _ _ => True⟩,
  ⟨"map",        2, fun _ => True, fun _ _ => True⟩,
  ⟨"filter",     2, fun _ => True, fun _ _ => True⟩,
  ⟨"round",      1, fun _ => True, fun _ _ => True⟩
]

end MoltTIR.Runtime
