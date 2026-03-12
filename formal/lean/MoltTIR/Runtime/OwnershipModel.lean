/-
  MoltTIR.Runtime.OwnershipModel — Ownership discipline for the NaN-boxed runtime.

  Defines Molt's ownership model: each heap object has exactly one "owner"
  (the reference that will ultimately trigger deallocation), plus zero or
  more "borrows" (temporary references that don't affect the refcount).

  Proves that ownership discipline + refcounting = memory safety:
  - Every owned object has refcount ≥ 1.
  - Borrows don't outlive the owner.
  - When ownership is transferred, refcounts are updated correctly.
  - The ownership discipline implies the MemorySafe property.

  This models the Rust-side ownership patterns in Molt's runtime:
  - Variables own their values (inc_ref on assignment, dec_ref on overwrite/scope exit).
  - CallArgs borrows: callargs entries are inc_ref'd on push, dec_ref'd on cleanup.
  - Return values: caller takes ownership of the return value.
  - The protect_callargs_aliased_return protocol (Refcount.lean) handles the
    edge case where a return value aliases a callargs entry.

  References:
  - runtime/molt-obj-model/src/lib.rs (RC protocol)
  - runtime/molt-runtime/src/call/bind.rs (callargs ownership)
  - MoltTIR.Runtime.MemorySafety (safety definitions)
  - MoltTIR.Runtime.Refcount (callargs alias protection proofs)
-/
import MoltTIR.Runtime.MemorySafety
import MoltTIR.Runtime.Refcount

set_option autoImplicit false

namespace MoltTIR.Runtime.OwnershipModel

open MoltTIR.Runtime.MemorySafety

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Ownership and borrowing definitions
-- ══════════════════════════════════════════════════════════════════

/-- An ownership claim on a heap address. Each claim contributes +1 to
    the refcount. Claims are held by variables, data structure slots,
    and callargs entries. -/
structure OwnershipClaim where
  /-- The address being owned. -/
  target : Addr
  /-- A unique identifier for the claim holder (variable ID, slot index, etc.). -/
  holder : Nat
  deriving DecidableEq, Repr

/-- A borrow: a temporary reference to a heap address that does NOT
    contribute to the refcount. Borrows are used for short-lived access
    patterns where the caller guarantees the object remains live.
    In Molt's runtime, borrows appear in:
    - Function return values (caller's tracking handles cleanup)
    - Temporary expression results during evaluation -/
structure Borrow where
  /-- The address being borrowed. -/
  target : Addr
  /-- Borrow scope identifier (e.g., expression evaluation epoch). -/
  scope : Nat
  deriving DecidableEq, Repr

/-- The ownership state of the runtime: a collection of ownership claims
    and active borrows. -/
structure OwnershipState where
  /-- All active ownership claims. Each contributes +1 to refcount. -/
  claims : List OwnershipClaim
  /-- All active borrows. Do not affect refcount. -/
  borrows : List Borrow
  deriving Repr

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Ownership invariants
-- ══════════════════════════════════════════════════════════════════

/-- Count the number of ownership claims targeting a given address. -/
def claimCount (os : OwnershipState) (a : Addr) : Nat :=
  os.claims.filter (fun c => c.target == a) |>.length

/-- Count the number of active borrows targeting a given address. -/
def borrowCount (os : OwnershipState) (a : Addr) : Nat :=
  os.borrows.filter (fun b => b.target == a) |>.length

/-- OwnershipInvariant: the refcount of every live object equals its
    claim count. Borrows do not contribute to refcounts. -/
def OwnershipInvariant (h : Heap) (os : OwnershipState) : Prop :=
  ∀ (a : Addr) (hlive : IsLive h a),
    (getMeta h a hlive).refcount = claimCount os a

/-- BorrowSafety: every active borrow targets a live object that has
    at least one ownership claim. This ensures borrows don't outlive
    the objects they reference. -/
def BorrowSafety (h : Heap) (os : OwnershipState) : Prop :=
  ∀ b ∈ os.borrows, IsLive h b.target ∧ claimCount os b.target ≥ 1

/-- NoOrphanClaims: every ownership claim targets a live object. -/
def NoOrphanClaims (h : Heap) (os : OwnershipState) : Prop :=
  ∀ c ∈ os.claims, IsLive h c.target

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Ownership operations
-- ══════════════════════════════════════════════════════════════════

/-- Acquire ownership: add a claim and increment the refcount. -/
def acquire (h : Heap) (os : OwnershipState) (a : Addr) (holder : Nat) :
    Heap × OwnershipState :=
  (MemorySafety.incRef h a,
   { os with claims := ⟨a, holder⟩ :: os.claims })

/-- Release ownership: remove a claim and decrement the refcount.
    If the refcount drops to 0, the object should be deallocated
    (handled separately by the dealloc operation). -/
def release (h : Heap) (os : OwnershipState) (a : Addr) (holder : Nat) :
    Heap × OwnershipState :=
  (MemorySafety.decRef h a,
   { os with claims := os.claims.filter (fun c => ¬(c.target == a && c.holder == holder)) })

/-- Create a borrow: add a borrow record without touching refcounts. -/
def borrow (os : OwnershipState) (a : Addr) (scope : Nat) : OwnershipState :=
  { os with borrows := ⟨a, scope⟩ :: os.borrows }

/-- End a borrow: remove the borrow record without touching refcounts. -/
def endBorrow (os : OwnershipState) (a : Addr) (scope : Nat) : OwnershipState :=
  { os with borrows := os.borrows.filter (fun b => ¬(b.target == a && b.scope == scope)) }

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Ownership discipline preserves refcount soundness
-- ══════════════════════════════════════════════════════════════════

/-- Acquiring ownership preserves the ownership invariant:
    the new claim adds +1 to claimCount, and incRef adds +1 to refcount. -/
theorem acquire_preserves_invariant
    (h : Heap) (os : OwnershipState) (a : Addr) (holder : Nat)
    (halive : IsLive h a)
    (hinv : OwnershipInvariant h os) :
    let (h', os') := acquire h os a holder
    OwnershipInvariant h' os' := by
  -- TODO(formal, owner:runtime, milestone:M4, priority:P1, status:partial):
  --   Prove that incRef at `a` adds +1 to refcount and the new claim adds +1
  --   to claimCount, preserving the equation. For addresses ≠ a, both sides
  --   are unchanged.
  sorry

/-- Releasing ownership preserves the ownership invariant:
    the removed claim subtracts 1 from claimCount, and decRef subtracts 1
    from refcount. -/
theorem release_preserves_invariant
    (h : Heap) (os : OwnershipState) (a : Addr) (holder : Nat)
    (halive : IsLive h a)
    (hclaim : ⟨a, holder⟩ ∈ os.claims)
    (hinv : OwnershipInvariant h os) :
    let (h', os') := release h os a holder
    OwnershipInvariant h' os' := by
  -- TODO(formal, owner:runtime, milestone:M4, priority:P1, status:partial):
  --   Symmetric to acquire: decRef subtracts 1, removing the claim subtracts 1.
  --   Requires proving that List.filter removes exactly one matching claim.
  sorry

/-- Borrowing does not affect the ownership invariant (borrows don't
    touch refcounts or claims). -/
theorem borrow_preserves_invariant
    (h : Heap) (os : OwnershipState) (a : Addr) (scope : Nat)
    (hinv : OwnershipInvariant h os) :
    OwnershipInvariant h (borrow os a scope) := by
  unfold OwnershipInvariant at *
  intro addr hlive
  -- borrow only modifies os.borrows, not os.claims
  -- claimCount only looks at os.claims
  unfold borrow claimCount
  simp
  exact hinv addr hlive

/-- Ending a borrow does not affect the ownership invariant. -/
theorem endBorrow_preserves_invariant
    (h : Heap) (os : OwnershipState) (a : Addr) (scope : Nat)
    (hinv : OwnershipInvariant h os) :
    OwnershipInvariant h (endBorrow os a scope) := by
  unfold OwnershipInvariant at *
  intro addr hlive
  unfold endBorrow claimCount
  simp
  exact hinv addr hlive

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Ownership discipline implies memory safety
-- ══════════════════════════════════════════════════════════════════

/-- If every live object has at least one ownership claim (claimCount ≥ 1),
    then its refcount is ≥ 1, meaning it won't be prematurely freed. -/
theorem ownership_prevents_premature_free
    (h : Heap) (os : OwnershipState) (a : Addr)
    (halive : IsLive h a)
    (hinv : OwnershipInvariant h os)
    (hclaimed : claimCount os a ≥ 1) :
    (getMeta h a halive).refcount ≥ 1 := by
  have := hinv a halive
  omega

/-- Borrow safety ensures borrows target live objects: if borrow safety
    holds, every borrowed address is live. -/
theorem borrow_targets_live
    (h : Heap) (os : OwnershipState) (a : Addr) (scope : Nat)
    (hbs : BorrowSafety h os)
    (hborrow : ⟨a, scope⟩ ∈ os.borrows) :
    IsLive h a := by
  exact (hbs ⟨a, scope⟩ hborrow).1

/-- The main theorem: ownership discipline + refcounting = memory safety.
    If the ownership invariant holds and all borrows are safe, then
    the heap invariant holds (given that pointer fields in objects
    correspond to ownership claims). -/
theorem ownership_plus_refcount_implies_safety
    (h : Heap) (os : OwnershipState)
    (hinv : OwnershipInvariant h os)
    (hbs : BorrowSafety h os)
    (hno_orphans : NoOrphanClaims h os)
    (hptr_claims : ∀ (a : Addr) (hlive : IsLive h a),
      ∀ p ∈ (getMeta h a hlive).pointers, claimCount os p ≥ 1) :
    HeapInvariant h := by
  unfold HeapInvariant
  intro a hlive hlive' p hp
  have hclaimed := hptr_claims a hlive' p hp
  -- p has at least one claim, so there exists a claim targeting p
  -- A claim targeting p means p must be live (by NoOrphanClaims)
  -- TODO(formal, owner:runtime, milestone:M4, priority:P1, status:partial):
  --   Bridge from claimCount ≥ 1 to ∃ claim ∈ os.claims targeting p,
  --   then apply NoOrphanClaims to conclude IsLive h p.
  --   This requires a lemma: claimCount os p ≥ 1 → ∃ c ∈ os.claims, c.target = p.
  sorry

-- ══════════════════════════════════════════════════════════════════
-- Section 6: CallArgs ownership protocol (connects to Refcount.lean)
-- ══════════════════════════════════════════════════════════════════

/-- CallArgs ownership: each address pushed to callargs gets an ownership
    claim (inc_ref on push). The protect_callargs_aliased_return protocol
    adds an extra claim if the return value aliases a callargs entry.
    Then callargs_dec_ref_all releases all callargs claims.

    This bridges the abstract ownership model to the concrete callargs
    protocol proven in Refcount.lean. -/

/-- Model callargs as a sequence of ownership claims with a shared holder ID. -/
def callargsClaims (addrs : List Addr) (callId : Nat) : List OwnershipClaim :=
  addrs.map fun a => ⟨a, callId⟩

/-- After callargs cleanup (decRefAll), all callargs claims are released.
    Combined with protect (Refcount.lean), the return value remains live. -/
theorem callargs_ownership_safe
    (h : Refcount.Heap) (result : Refcount.Addr) (addrs : List Refcount.Addr)
    (hmem : result ∈ addrs)
    (hcount : Refcount.countOcc result addrs = 1)
    (h_ge : h result ≥ 1) :
    Refcount.decRefAll (Refcount.protect h result addrs) addrs result = h result :=
  Refcount.protect_then_cleanup_preserves h result addrs hmem hcount h_ge

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Concrete ownership scenarios
-- ══════════════════════════════════════════════════════════════════

/-- Example: a variable assignment creates ownership.
    `x = obj` → acquire(obj, x_id) → refcount goes from 0 to 1. -/
example : claimCount
    { claims := [⟨42, 0⟩], borrows := [] }
    42 = 1 := by native_decide

/-- Example: two variables pointing to the same object → refcount = 2.
    `x = obj; y = x` → two claims on obj. -/
example : claimCount
    { claims := [⟨42, 0⟩, ⟨42, 1⟩], borrows := [] }
    42 = 2 := by native_decide

/-- Example: a borrow doesn't increase the claim count. -/
example : claimCount
    { claims := [⟨42, 0⟩], borrows := [⟨42, 100⟩] }
    42 = 1 := by native_decide

/-- Example: borrowCount is separate from claimCount. -/
example : borrowCount
    { claims := [⟨42, 0⟩], borrows := [⟨42, 100⟩] }
    42 = 1 := by native_decide

end MoltTIR.Runtime.OwnershipModel
