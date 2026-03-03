/-
  MoltTIR.Runtime.Refcount — refcount correctness for callargs alias protection.

  Formalizes the Molt runtime's protect_callargs_aliased_return protocol from
  runtime/molt-runtime/src/call/bind.rs and proves it prevents use-after-free.

  The model:
  - A Heap maps addresses to natural number refcounts.
  - CallArgs owns a list of addresses (pointer values), each inc_ref'd on push.
  - protect increments result's refcount if it aliases any callargs entry.
  - decRefAll decrements every callargs entry once.

  Key results:
  - protect_then_cleanup_net: net refcount change at result is +1 - k (k = alias count).
  - protect_then_cleanup_safe: with protection, result survives cleanup when k ≤ 1.
  - without_protect_unsafe: counterexample showing use-after-free without protection.
-/

namespace MoltTIR.Runtime.Refcount

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Abstract heap model
-- ══════════════════════════════════════════════════════════════════

/-- Address space (simplified to Nat). -/
abbrev Addr := Nat

/-- Heap maps addresses to refcounts. -/
abbrev Heap := Addr → Nat

/-- Increment refcount at address a. -/
def incRef (h : Heap) (a : Addr) : Heap :=
  fun x => if x = a then h x + 1 else h x

/-- Decrement refcount at address a (Nat subtraction: saturates at 0). -/
def decRef (h : Heap) (a : Addr) : Heap :=
  fun x => if x = a then h x - 1 else h x

/-- Decrement all addresses in a list (models callargs_dec_ref_all). -/
def decRefAll : Heap → List Addr → Heap
  | h, [] => h
  | h, a :: rest => decRefAll (decRef h a) rest

/-- Protect: increment result if it appears in the address list
    (models protect_callargs_aliased_return — breaks on first match). -/
def protect (h : Heap) (result : Addr) (addrs : List Addr) : Heap :=
  if result ∈ addrs then incRef h result else h

/-- Count occurrences of an address in a list. -/
def countOcc (a : Addr) : List Addr → Nat
  | [] => 0
  | x :: rest => (if x = a then 1 else 0) + countOcc a rest

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Core lemmas about heap operations
-- ══════════════════════════════════════════════════════════════════

@[simp] theorem incRef_at (h : Heap) (a : Addr) : incRef h a a = h a + 1 := by
  simp [incRef]

@[simp] theorem incRef_ne (h : Heap) (a b : Addr) (hne : b ≠ a) : incRef h a b = h b := by
  simp [incRef, hne]

@[simp] theorem decRef_at (h : Heap) (a : Addr) : decRef h a a = h a - 1 := by
  simp [decRef]

@[simp] theorem decRef_ne (h : Heap) (a b : Addr) (hne : b ≠ a) : decRef h a b = h b := by
  simp [decRef, hne]

/-- decRefAll at an address not in the list is a no-op. -/
theorem decRefAll_absent (h : Heap) (addrs : List Addr) (a : Addr)
    (habs : a ∉ addrs) : decRefAll h addrs a = h a := by
  induction addrs generalizing h with
  | nil => rfl
  | cons x rest ih =>
    simp only [decRefAll]
    have hne : a ≠ x := fun heq => habs (heq ▸ List.mem_cons_self x rest)
    have hrest : a ∉ rest := fun hm => habs (List.mem_cons_of_mem x hm)
    rw [ih (decRef h x) hrest]
    simp [decRef, hne]

/-- decRefAll at address a reduces the refcount by countOcc a addrs. -/
theorem decRefAll_at (h : Heap) (addrs : List Addr) (a : Addr)
    (h_ge : h a ≥ countOcc a addrs) :
    decRefAll h addrs a = h a - countOcc a addrs := by
  induction addrs generalizing h with
  | nil => simp [decRefAll, countOcc]
  | cons x rest ih =>
    simp only [decRefAll, countOcc]
    by_cases hxa : x = a
    · simp only [hxa, ite_true]
      simp only [countOcc, hxa, ite_true] at h_ge
      have hpre : (decRef h a) a ≥ countOcc a rest := by simp [decRef]; omega
      rw [ih (decRef h a) hpre]; simp [decRef]; omega
    · simp only [hxa, ite_false, Nat.zero_add]
      simp only [countOcc, hxa, ite_false, Nat.zero_add] at h_ge
      have hne : a ≠ x := fun heq => hxa heq.symm
      have hpre : (decRef h x) a ≥ countOcc a rest := by simp [decRef, hne]; omega
      rw [ih (decRef h x) hpre]; simp [decRef, hne]

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Main safety theorems
-- ══════════════════════════════════════════════════════════════════

/-- After protect + decRefAll, the result's refcount changes by exactly +1 - k
    where k is the number of times result appears in addrs.
    This is the core correctness property. -/
theorem protect_then_cleanup_net (h : Heap) (result : Addr) (addrs : List Addr)
    (hmem : result ∈ addrs)
    (h_ge : h result + 1 ≥ countOcc result addrs) :
    decRefAll (protect h result addrs) addrs result
    = h result + 1 - countOcc result addrs := by
  simp only [protect, hmem, ite_true]
  rw [decRefAll_at (incRef h result) addrs result]
  · simp [incRef]
  · simp [incRef]; exact h_ge

/-- When result appears exactly once in addrs (the common case),
    protect + decRefAll preserves the refcount exactly. -/
theorem protect_then_cleanup_preserves (h : Heap) (result : Addr) (addrs : List Addr)
    (hmem : result ∈ addrs)
    (hcount : countOcc result addrs = 1)
    (h_ge : h result ≥ 1) :
    decRefAll (protect h result addrs) addrs result = h result := by
  rw [protect_then_cleanup_net h result addrs hmem (by omega)]
  omega

/-- When result does NOT appear in addrs, protect is a no-op and
    decRefAll doesn't touch result. -/
theorem protect_then_cleanup_absent (h : Heap) (result : Addr) (addrs : List Addr)
    (habs : result ∉ addrs) :
    decRefAll (protect h result addrs) addrs result = h result := by
  simp only [protect, habs, ite_false]
  exact decRefAll_absent h addrs result habs

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Counterexample — without protection, use-after-free
-- ══════════════════════════════════════════════════════════════════

/-- Without protect, if the ONLY reference to result came from callargs
    (refcount = 1, count = 1), decRefAll drops it to 0.
    Refcount 0 = freed object = use-after-free. -/
theorem without_protect_use_after_free :
    let h : Heap := fun _ => 1  -- every object has refcount 1
    let result := 42            -- some address
    let addrs := [42]           -- callargs contains result
    decRefAll h addrs result = 0 := by
  native_decide

/-- With protect, the same scenario preserves the refcount at 1. -/
theorem with_protect_safe :
    let h : Heap := fun _ => 1
    let result := 42
    let addrs := [42]
    decRefAll (protect h result addrs) addrs result = 1 := by
  native_decide

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Non-interference — protect doesn't affect other addresses
-- ══════════════════════════════════════════════════════════════════

/-- protect only modifies the result address. -/
theorem protect_ne (h : Heap) (result : Addr) (addrs : List Addr) (a : Addr)
    (hne : a ≠ result) : protect h result addrs a = h a := by
  simp only [protect]
  split <;> simp [incRef, hne]

/-- After protect + decRefAll, addresses not in addrs and not equal to result
    are completely unaffected. -/
theorem protect_then_cleanup_other (h : Heap) (result : Addr) (addrs : List Addr)
    (a : Addr) (hne : a ≠ result) (habs : a ∉ addrs) :
    decRefAll (protect h result addrs) addrs a = h a := by
  rw [decRefAll_absent _ addrs a habs]
  exact protect_ne h result addrs a hne

end MoltTIR.Runtime.Refcount
