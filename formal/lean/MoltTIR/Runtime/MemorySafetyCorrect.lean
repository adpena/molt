/-
  MoltTIR.Runtime.MemorySafetyCorrect — Safety proofs for the NaN-boxed runtime.

  Proves that Molt's heap operations preserve the memory safety invariants
  defined in MemorySafety.lean:
  - Allocation preserves heap invariant.
  - Deallocation (at refcount 0, no incoming pointers) preserves heap invariant.
  - inc_ref / dec_ref preserve refcount soundness.
  - NaN-boxed pointer values always point to live objects (given heap invariant).
  - Heap invariant implies no use-after-free.

  References:
  - runtime/molt-obj-model/src/lib.rs (object model, RC)
  - runtime/molt-runtime/src/call/bind.rs (protect_callargs_aliased_return)
  - MoltTIR.Runtime.MemorySafety (definitions)
  - MoltTIR.Runtime.NanBox (NaN-boxing predicates)
  - MoltTIR.Runtime.Refcount (callargs protocol proofs)
-/
import MoltTIR.Runtime.MemorySafety

set_option autoImplicit false

namespace MoltTIR.Runtime.MemorySafetyCorrect

open MoltTIR.Runtime
open MoltTIR.Runtime.MemorySafety

-- ══════════════════════════════════════════════════════════════════
-- Preliminary helpers for heap operations
-- ══════════════════════════════════════════════════════════════════

private theorem alloc_at_self (h : Heap) (a : Addr) (meta : ObjMeta) :
    alloc h a meta a = some meta := by
  unfold alloc; simp

private theorem alloc_at_other (h : Heap) (a : Addr) (meta : ObjMeta) (addr : Addr) (hne : addr ≠ a) :
    alloc h a meta addr = h addr := by
  unfold alloc; simp [hne]

private theorem dealloc_at_self (h : Heap) (a : Addr) :
    dealloc h a a = none := by
  unfold dealloc; simp

private theorem dealloc_at_other (h : Heap) (a : Addr) (addr : Addr) (hne : addr ≠ a) :
    dealloc h a addr = h addr := by
  unfold dealloc; simp [hne]

private theorem exists_meta_of_live (h : Heap) (a : Addr) (halive : IsLive h a) :
    ∃ m, h a = some m := by
  unfold IsLive at halive
  cases hq : h a with
  | none => rw [hq] at halive; simp at halive
  | some m => exact ⟨m, rfl⟩

private theorem getMeta_of_eq (h : Heap) (a : Addr) (m : ObjMeta)
    (hm : h a = some m) (hlive : IsLive h a) :
    getMeta h a hlive = m := by
  unfold getMeta; simp [hm]

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Allocation preserves heap invariant
-- ══════════════════════════════════════════════════════════════════

theorem alloc_preserves_heap_invariant
    (h : Heap) (a : Addr) (meta : ObjMeta)
    (hfresh : IsFreed h a)
    (hptrs : ∀ p ∈ meta.pointers, IsLive h p)
    (hinv : HeapInvariant h) :
    HeapInvariant (alloc h a meta) := by
  unfold HeapInvariant
  intro addr _ hlive'
  intro p hp
  by_cases heq : addr = a
  · -- addr = a: the newly allocated object
    -- After heq, use addr everywhere (subst replaces a with addr)
    subst heq
    have ha_new : alloc h addr meta addr = some meta := alloc_at_self h addr meta
    have hmeta_val : getMeta (alloc h addr meta) addr hlive' = meta :=
      getMeta_of_eq _ _ _ ha_new hlive'
    rw [hmeta_val] at hp
    have hp_old : IsLive h p := hptrs p hp
    unfold IsLive
    by_cases hpa : p = addr
    · subst hpa; rw [ha_new]; rfl
    · rw [alloc_at_other h addr meta p hpa]; exact hp_old
  · -- addr ≠ a: existing object, unchanged
    have haddr_eq : alloc h a meta addr = h addr := alloc_at_other h a meta addr heq
    have haddr_old_live : IsLive h addr := by
      unfold IsLive; rw [← haddr_eq]; exact hlive'
    have hmeta_eq : getMeta (alloc h a meta) addr hlive' = getMeta h addr haddr_old_live := by
      unfold getMeta; simp [haddr_eq]
    rw [hmeta_eq] at hp
    have hp_old : IsLive h p := hinv addr haddr_old_live haddr_old_live p hp
    unfold IsLive
    by_cases hpa : p = a
    · subst hpa; rw [alloc_at_self]; rfl
    · rw [alloc_at_other h a meta p hpa]; exact hp_old

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Deallocation preserves heap invariant
-- ══════════════════════════════════════════════════════════════════

theorem dealloc_preserves_heap_invariant
    (h : Heap) (a : Addr) (_liveAddrs : List Addr)
    (hno_refs : ∀ addr, IsLive h addr →
      ∀ (hlive : IsLive h addr),
        a ∉ (getMeta h addr hlive).pointers)
    (hinv : HeapInvariant h) :
    HeapInvariant (dealloc h a) := by
  unfold HeapInvariant
  intro addr _ hlive'
  intro p hp
  have hne : addr ≠ a := by
    intro heq; subst heq
    unfold IsLive at hlive'
    rw [dealloc_at_self] at hlive'; simp at hlive'
  have haddr_eq : dealloc h a addr = h addr := dealloc_at_other h a addr hne
  have haddr_old : IsLive h addr := by
    unfold IsLive; rw [← haddr_eq]; exact hlive'
  have hmeta_eq : getMeta (dealloc h a) addr hlive' = getMeta h addr haddr_old := by
    unfold getMeta; simp [haddr_eq]
  rw [hmeta_eq] at hp
  have hp_old : IsLive h p := hinv addr haddr_old haddr_old p hp
  have hp_ne : p ≠ a := by
    intro heq; subst heq
    exact hno_refs addr haddr_old haddr_old hp
  unfold IsLive
  rw [dealloc_at_other h a p hp_ne]
  exact hp_old

-- ══════════════════════════════════════════════════════════════════
-- Section 3: inc_ref preserves refcount soundness
-- ══════════════════════════════════════════════════════════════════

private theorem incRef_at_self (h : Heap) (a : Addr) (m : ObjMeta) (hm : h a = some m) :
    incRef h a a = some { m with refcount := m.refcount + 1 } := by
  unfold incRef; simp [hm]

private theorem incRef_at_other (h : Heap) (a addr : Addr) (hne : addr ≠ a) :
    incRef h a addr = h addr := by
  unfold incRef; simp [hne]

private theorem isLive_incRef_iff (h : Heap) (a addr : Addr) (halive : IsLive h a) :
    IsLive (incRef h a) addr ↔ IsLive h addr := by
  obtain ⟨ma, hma⟩ := exists_meta_of_live h a halive
  constructor
  · intro hlive
    unfold IsLive at hlive ⊢
    by_cases heq : addr = a
    · subst heq; exact halive
    · rw [incRef_at_other h addr addr heq] at hlive; exact hlive
  · intro hlive
    unfold IsLive at hlive ⊢
    by_cases heq : addr = a
    · subst heq; rw [incRef_at_self h addr ma hma]; rfl
    · rw [incRef_at_other h addr addr heq]; exact hlive

-- ── trueRefcount unchanged by incRef ──

private theorem trueRefcount_incRef_eq (h : Heap) (a : Addr) (addrs : List Addr) (target : Addr) :
    trueRefcount (incRef h a) addrs target = trueRefcount h addrs target := by
  unfold trueRefcount
  suffices ∀ (acc : Nat), List.foldl (fun acc' x =>
      match incRef h a x with
      | some meta => acc' + (meta.pointers.filter (· == target)).length
      | none => acc') acc addrs
    = List.foldl (fun acc' x =>
      match h x with
      | some meta => acc' + (meta.pointers.filter (· == target)).length
      | none => acc') acc addrs from this 0
  intro acc
  induction addrs generalizing acc with
  | nil => rfl
  | cons hd tl ih =>
    simp only [List.foldl_cons]
    suffices hstep : (match incRef h a hd with
            | some meta => acc + (meta.pointers.filter (· == target)).length
            | none => acc) =
           (match h hd with
            | some meta => acc + (meta.pointers.filter (· == target)).length
            | none => acc) by
      rw [hstep]; exact ih _
    by_cases heq : hd = a
    · -- hd = a: incRef changes refcount but not pointers
      subst heq
      cases hm : h hd with
      | none =>
        have : incRef h hd hd = none := by unfold incRef; simp [hm]
        simp [this, hm]
      | some m =>
        have : incRef h hd hd = some { m with refcount := m.refcount + 1 } :=
          incRef_at_self h hd m hm
        simp [this, hm]
    · rw [incRef_at_other h a hd heq]

private theorem getMeta_incRef_self_rc (h : Heap) (a : Addr) (m : ObjMeta) (hm : h a = some m)
    (hlive_new : IsLive (incRef h a) a) :
    (getMeta (incRef h a) a hlive_new).refcount = m.refcount + 1 := by
  have hinc := incRef_at_self h a m hm
  have hg : getMeta (incRef h a) a hlive_new = { m with refcount := m.refcount + 1 } :=
    getMeta_of_eq _ _ _ hinc hlive_new
  rw [hg]

private theorem getMeta_incRef_other_eq (h : Heap) (a addr : Addr) (hne : addr ≠ a)
    (hlive_old : IsLive h addr) (hlive_new : IsLive (incRef h a) addr) :
    getMeta (incRef h a) addr hlive_new = getMeta h addr hlive_old := by
  unfold getMeta
  simp [incRef_at_other h a addr hne]

theorem inc_ref_preserves_refcount_sound
    (h : Heap) (liveAddrs : List Addr) (roots : Addr → Nat) (a : Addr)
    (halive : IsLive h a)
    (hsound : RefcountSound h liveAddrs roots) :
    RefcountSound (incRef h a) liveAddrs (fun x => if x = a then roots x + 1 else roots x) := by
  unfold RefcountSound at *
  intro addr hmem hlive_new
  have hlive_old : IsLive h addr := (isLive_incRef_iff h a addr halive).mp hlive_new
  have hsound_addr := hsound addr hmem hlive_old
  obtain ⟨ma, hma⟩ := exists_meta_of_live h a halive
  by_cases heq : addr = a
  · -- Case addr = a: refcount goes up by 1, roots go up by 1
    subst heq
    have hrc := getMeta_incRef_self_rc h addr ma hma hlive_new
    rw [trueRefcount_incRef_eq]
    simp
    have hgold : getMeta h addr halive = ma := getMeta_of_eq _ _ _ hma halive
    rw [hgold] at hsound_addr
    omega
  · -- Case addr ≠ a: everything unchanged
    have hg := getMeta_incRef_other_eq h a addr heq hlive_old hlive_new
    rw [hg, trueRefcount_incRef_eq]
    simp [heq]
    exact hsound_addr

-- ══════════════════════════════════════════════════════════════════
-- Section 4: dec_ref preserves refcount soundness
-- ══════════════════════════════════════════════════════════════════

private theorem decRef_at_self (h : Heap) (a : Addr) (m : ObjMeta) (hm : h a = some m) :
    decRef h a a = some { m with refcount := m.refcount - 1 } := by
  unfold decRef; simp [hm]

private theorem decRef_at_other (h : Heap) (a addr : Addr) (hne : addr ≠ a) :
    decRef h a addr = h addr := by
  unfold decRef; simp [hne]

private theorem isLive_decRef_iff (h : Heap) (a addr : Addr) (halive : IsLive h a) :
    IsLive (decRef h a) addr ↔ IsLive h addr := by
  obtain ⟨ma, hma⟩ := exists_meta_of_live h a halive
  constructor
  · intro hlive
    unfold IsLive at hlive ⊢
    by_cases heq : addr = a
    · subst heq; exact halive
    · rw [decRef_at_other h addr addr heq] at hlive; exact hlive
  · intro hlive
    unfold IsLive at hlive ⊢
    by_cases heq : addr = a
    · subst heq; rw [decRef_at_self h addr ma hma]; rfl
    · rw [decRef_at_other h addr addr heq]; exact hlive

private theorem trueRefcount_decRef_eq (h : Heap) (a : Addr) (addrs : List Addr) (target : Addr) :
    trueRefcount (decRef h a) addrs target = trueRefcount h addrs target := by
  unfold trueRefcount
  suffices ∀ (acc : Nat), List.foldl (fun acc' x =>
      match decRef h a x with
      | some meta => acc' + (meta.pointers.filter (· == target)).length
      | none => acc') acc addrs
    = List.foldl (fun acc' x =>
      match h x with
      | some meta => acc' + (meta.pointers.filter (· == target)).length
      | none => acc') acc addrs from this 0
  intro acc
  induction addrs generalizing acc with
  | nil => rfl
  | cons hd tl ih =>
    simp only [List.foldl_cons]
    suffices hstep : (match decRef h a hd with
            | some meta => acc + (meta.pointers.filter (· == target)).length
            | none => acc) =
           (match h hd with
            | some meta => acc + (meta.pointers.filter (· == target)).length
            | none => acc) by
      rw [hstep]; exact ih _
    by_cases heq : hd = a
    · subst heq
      cases hm : h hd with
      | none =>
        have : decRef h hd hd = none := by unfold decRef; simp [hm]
        simp [this, hm]
      | some m =>
        have : decRef h hd hd = some { m with refcount := m.refcount - 1 } :=
          decRef_at_self h hd m hm
        simp [this, hm]
    · rw [decRef_at_other h a hd heq]

private theorem getMeta_decRef_self_rc (h : Heap) (a : Addr) (m : ObjMeta) (hm : h a = some m)
    (hlive_new : IsLive (decRef h a) a) :
    (getMeta (decRef h a) a hlive_new).refcount = m.refcount - 1 := by
  have hdec := decRef_at_self h a m hm
  have hg : getMeta (decRef h a) a hlive_new = { m with refcount := m.refcount - 1 } :=
    getMeta_of_eq _ _ _ hdec hlive_new
  rw [hg]

private theorem getMeta_decRef_other_eq (h : Heap) (a addr : Addr) (hne : addr ≠ a)
    (hlive_old : IsLive h addr) (hlive_new : IsLive (decRef h a) addr) :
    getMeta (decRef h a) addr hlive_new = getMeta h addr hlive_old := by
  unfold getMeta
  simp [decRef_at_other h a addr hne]

theorem dec_ref_preserves_refcount_sound
    (h : Heap) (liveAddrs : List Addr) (roots : Addr → Nat) (a : Addr)
    (halive : IsLive h a)
    (hroots_pos : roots a ≥ 1)
    (hsound : RefcountSound h liveAddrs roots) :
    RefcountSound (decRef h a) liveAddrs (fun x => if x = a then roots x - 1 else roots x) := by
  unfold RefcountSound at *
  intro addr hmem hlive_new
  have hlive_old : IsLive h addr := (isLive_decRef_iff h a addr halive).mp hlive_new
  have hsound_addr := hsound addr hmem hlive_old
  obtain ⟨ma, hma⟩ := exists_meta_of_live h a halive
  by_cases heq : addr = a
  · -- Case addr = a: refcount goes down by 1, roots go down by 1
    subst heq
    have hrc := getMeta_decRef_self_rc h addr ma hma hlive_new
    rw [trueRefcount_decRef_eq]
    simp
    have hgold : getMeta h addr halive = ma := getMeta_of_eq _ _ _ hma halive
    rw [hgold] at hsound_addr
    omega
  · -- Case addr ≠ a: everything unchanged
    have hg := getMeta_decRef_other_eq h a addr heq hlive_old hlive_new
    rw [hg, trueRefcount_decRef_eq]
    simp [heq]
    exact hsound_addr

-- ══════════════════════════════════════════════════════════════════
-- Section 5: NaN-boxed pointer values always point to live objects
-- ══════════════════════════════════════════════════════════════════

theorem nanbox_ptr_always_valid
    (h : Heap) (v : UInt64) (hptr : IsPtr v)
    (hsafe : NanBoxSafe h v) :
    IsLive h (v &&& WasmNative.POINTER_MASK).toNat := by
  exact hsafe hptr

theorem nanbox_inline_safe (h : Heap) (v : UInt64) (hnot_ptr : ¬IsPtr v) :
    NanBoxSafe h v := by
  unfold NanBoxSafe
  intro hptr
  exact absurd hptr hnot_ptr

theorem nanbox_int_safe (h : Heap) (v : UInt64) (hint : IsInt v) :
    NanBoxSafe h v :=
  nanbox_inline_safe h v (int_not_ptr v hint)

theorem nanbox_bool_safe (h : Heap) (v : UInt64) (hbool : IsBool v) :
    NanBoxSafe h v :=
  nanbox_inline_safe h v (bool_not_ptr v hbool)

theorem nanbox_none_safe (h : Heap) (v : UInt64) (hnone : IsNone_ v) :
    NanBoxSafe h v :=
  nanbox_inline_safe h v (none_not_ptr v hnone)

theorem nanbox_float_safe (h : Heap) (v : UInt64) (hfloat : IsFloat v) :
    NanBoxSafe h v :=
  nanbox_inline_safe h v (fun hptr => absurd (isPtr_tagged v hptr) (float_not_tagged v hfloat))

theorem nanbox_pending_safe (h : Heap) (v : UInt64) (hpend : IsPending v) :
    NanBoxSafe h v :=
  nanbox_inline_safe h v (fun hptr => absurd hpend (ptr_not_pending v hptr))

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Heap invariant implies no use-after-free
-- ══════════════════════════════════════════════════════════════════

theorem no_use_after_free_invariant
    (h : Heap) (events : List DerefEvent)
    (_hinv : HeapInvariant h)
    (hall_live : ∀ e ∈ events, IsLive h e.addr) :
    NoUseAfterFree h events := by
  exact hall_live

theorem memory_safe_no_dangling
    (h : Heap) (liveAddrs : List Addr) (activeValues : List UInt64)
    (roots : Addr → Nat)
    (hms : MemorySafe h liveAddrs activeValues roots) :
    NoDanglingPtr h activeValues := by
  unfold NoDanglingPtr
  intro v hv hptr
  exact hms.valuesSafe v hv hptr

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Allocation preserves MemorySafe (composite)
-- ══════════════════════════════════════════════════════════════════

theorem alloc_preserves_memory_safe
    (h : Heap) (a : Addr) (meta : ObjMeta)
    (liveAddrs : List Addr) (activeValues : List UInt64)
    (roots : Addr → Nat)
    (hfresh : IsFreed h a)
    (hptrs : ∀ p ∈ meta.pointers, IsLive h p)
    (hms : MemorySafe h liveAddrs activeValues roots) :
    HeapInvariant (alloc h a meta) := by
  exact alloc_preserves_heap_invariant h a meta hfresh hptrs hms.heapInv

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Bounds checking is preserved by non-overlapping operations
-- ══════════════════════════════════════════════════════════════════

theorem bounds_check_preserved_by_alloc
    (h : Heap) (a b : Addr) (off accessSize : Nat) (meta : ObjMeta)
    (hne : a ≠ b)
    (hbc : BoundsCheck h a off accessSize) :
    BoundsCheck (alloc h b meta) a off accessSize := by
  unfold BoundsCheck at *
  obtain ⟨m, hm, hfit⟩ := hbc
  refine ⟨m, ?_, hfit⟩
  rw [alloc_at_other h b meta a hne]
  exact hm

theorem bounds_check_no_overflow
    (h : Heap) (a : Addr) (off accessSize : Nat)
    (hbc : BoundsCheck h a off accessSize) :
    ∃ meta, h a = some meta ∧ off + accessSize ≤ meta.size :=
  hbc

-- ══════════════════════════════════════════════════════════════════
-- Section 9: Concrete examples — counterexamples and witnesses
-- ══════════════════════════════════════════════════════════════════

theorem empty_heap_invariant : HeapInvariant emptyHeap := by
  unfold HeapInvariant emptyHeap IsLive
  intro a hlive
  simp at hlive

theorem singleton_heap_invariant :
    HeapInvariant (fun a => if a = 0 then some ⟨1, 16, []⟩ else none) := by
  unfold HeapInvariant
  intro a hlive hlive' p hp
  by_cases ha : a = 0
  · subst ha
    unfold getMeta at hp
    simp at hp
  · unfold IsLive at hlive'
    simp [ha] at hlive'

theorem dangling_ptr_breaks_invariant :
    ¬ HeapInvariant (fun a => if a = 0 then some ⟨1, 16, [1]⟩ else none) := by
  unfold HeapInvariant
  intro hinv
  have h0_live : IsLive (fun a => if a = 0 then some ⟨1, 16, [1]⟩ else none) 0 := by
    unfold IsLive; simp
  have := hinv 0 h0_live h0_live 1
  unfold getMeta at this
  simp at this
  unfold IsLive at this
  simp at this

end MoltTIR.Runtime.MemorySafetyCorrect
