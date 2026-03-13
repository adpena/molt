/-
  MoltTIR.Runtime.MemorySafety — Memory safety model for the NaN-boxed runtime.

  Defines the core memory safety abstractions for Molt's NaN-boxed runtime:
  a heap model, heap invariants, and the key safety properties (no use-after-free,
  no dangling pointers, bounds checking, refcount soundness).

  These definitions provide the vocabulary for the correctness proofs in
  MemorySafetyCorrect.lean and the ownership model in OwnershipModel.lean.

  References:
  - runtime/molt-obj-model/src/lib.rs (NaN-boxed object model, RC)
  - runtime/molt-runtime/src/call/bind.rs (callargs alias protection)
  - MoltTIR.Runtime.NanBox (NaN-boxing type predicates)
  - MoltTIR.Runtime.Refcount (callargs refcount protocol)
-/
import MoltTIR.Runtime.NanBox
import MoltTIR.Runtime.Refcount
import MoltTIR.Runtime.WasmNative

set_option autoImplicit false

namespace MoltTIR.Runtime.MemorySafety

open MoltTIR.Runtime

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Address and object metadata
-- ══════════════════════════════════════════════════════════════════

/-- Address space (matches Refcount.Addr). -/
abbrev Addr := Nat

/-- Object metadata stored alongside each heap allocation. -/
structure ObjMeta where
  /-- Reference count for this object. -/
  refcount : Nat
  /-- Size of the object in bytes (header + fields). -/
  size : Nat
  /-- Set of addresses this object's fields point to.
      Models the pointer graph for reachability analysis. -/
  pointers : List Addr
  deriving DecidableEq, Repr

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Heap model
-- ══════════════════════════════════════════════════════════════════

/-- A Heap maps addresses to optional object metadata.
    `none` means the address is not allocated (or has been freed).
    `some meta` means a live object with the given metadata. -/
abbrev Heap := Addr → Option ObjMeta

/-- The empty heap: nothing is allocated. -/
def emptyHeap : Heap := fun _ => none

/-- An address is live (allocated and not freed) on a given heap. -/
def IsLive (h : Heap) (a : Addr) : Prop := (h a).isSome = true

/-- An address is freed (not allocated) on a given heap. -/
def IsFreed (h : Heap) (a : Addr) : Prop := h a = none

/-- Get the metadata of a live object (partial: requires liveness proof). -/
def getMeta (h : Heap) (a : Addr) (hlive : IsLive h a) : ObjMeta :=
  (h a).get (by unfold IsLive at hlive; exact Option.isSome_iff_exists.mp hlive |>.choose_spec ▸ hlive)

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Heap invariant — all pointers in live objects point to
--            live allocations
-- ══════════════════════════════════════════════════════════════════

/-- HeapInvariant: every pointer stored in every live object points to
    a live allocation. This is the fundamental well-formedness condition
    that prevents dangling pointers within the heap graph. -/
def HeapInvariant (h : Heap) : Prop :=
  ∀ (a : Addr), IsLive h a →
    ∀ (hlive : IsLive h a),
      ∀ p ∈ (getMeta h a hlive).pointers, IsLive h p

-- ══════════════════════════════════════════════════════════════════
-- Section 4: NoUseAfterFree — freed addresses are never dereferenced
-- ══════════════════════════════════════════════════════════════════

/-- A dereference event: reading or writing through an address. -/
inductive DerefEvent where
  | read  (addr : Addr)
  | write (addr : Addr)
  deriving DecidableEq, Repr

/-- Extract the address from a dereference event. -/
def DerefEvent.addr : DerefEvent → Addr
  | .read a  => a
  | .write a => a

/-- NoUseAfterFree: no dereference event targets a freed address.
    Parameterized over a trace of dereference events and the heap state
    at each event. -/
def NoUseAfterFree (h : Heap) (events : List DerefEvent) : Prop :=
  ∀ e ∈ events, IsLive h e.addr

-- ══════════════════════════════════════════════════════════════════
-- Section 5: NoDanglingPtr — every pointer value points to a live
--            allocation
-- ══════════════════════════════════════════════════════════════════

/-- NoDanglingPtr: every NaN-boxed pointer value in a set of live values
    points to a live allocation. The address is extracted from the lower
    48 bits of the NaN-boxed pointer (POINTER_MASK). -/
def NoDanglingPtr (h : Heap) (values : List UInt64) : Prop :=
  ∀ v ∈ values, IsPtr v → IsLive h (v &&& WasmNative.POINTER_MASK).toNat

-- ══════════════════════════════════════════════════════════════════
-- Section 6: BoundsCheck — array/table accesses within allocated bounds
-- ══════════════════════════════════════════════════════════════════

/-- BoundsCheck: an access at byte offset `off` of size `accessSize` into
    an object at address `a` is within the object's allocated size. -/
def BoundsCheck (h : Heap) (a : Addr) (off : Nat) (accessSize : Nat) : Prop :=
  ∃ meta, h a = some meta ∧ off + accessSize ≤ meta.size

/-- Array bounds check: accessing element `idx` of an array of `elemSize`-byte
    elements stored after a header of `headerSize` bytes. -/
def ArrayBoundsCheck (h : Heap) (a : Addr) (headerSize : Nat)
    (elemSize : Nat) (idx : Nat) : Prop :=
  BoundsCheck h a (headerSize + idx * elemSize) elemSize

-- ══════════════════════════════════════════════════════════════════
-- Section 7: RefcountSound — refcount equals actual reference count
-- ══════════════════════════════════════════════════════════════════

/-- Count how many times address `target` appears as a pointer in live
    objects across the entire heap. Models the "true" reference count. -/
def trueRefcount (h : Heap) (liveAddrs : List Addr) (target : Addr) : Nat :=
  liveAddrs.foldl (fun acc a =>
    match h a with
    | some meta => acc + (meta.pointers.filter (· == target)).length
    | none => acc
  ) 0

/-- RefcountSound: for every live object, its stored refcount equals the
    number of pointers to it from other live objects, plus the number of
    root references (stack/global variables pointing to it). -/
def RefcountSound (h : Heap) (liveAddrs : List Addr) (roots : Addr → Nat) : Prop :=
  ∀ a ∈ liveAddrs, ∀ (hlive : IsLive h a),
    (getMeta h a hlive).refcount = trueRefcount h liveAddrs a + roots a

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Heap operations — alloc, dealloc, incRef, decRef
-- ══════════════════════════════════════════════════════════════════

/-- Allocate a new object at address `a` with given metadata.
    Precondition: `a` is not currently live. -/
def alloc (h : Heap) (a : Addr) (meta : ObjMeta) : Heap :=
  fun x => if x = a then some meta else h x

/-- Free an object at address `a`.
    Precondition: refcount is 0 and no live pointers reference it. -/
def dealloc (h : Heap) (a : Addr) : Heap :=
  fun x => if x = a then none else h x

/-- Increment the refcount of the object at address `a`. -/
def incRef (h : Heap) (a : Addr) : Heap :=
  fun x => if x = a then
    match h a with
    | some meta => some { meta with refcount := meta.refcount + 1 }
    | none => none
  else h x

/-- Decrement the refcount of the object at address `a`. -/
def decRef (h : Heap) (a : Addr) : Heap :=
  fun x => if x = a then
    match h a with
    | some meta => some { meta with refcount := meta.refcount - 1 }
    | none => none
  else h x

-- ══════════════════════════════════════════════════════════════════
-- Section 9: NaN-box pointer validity predicate
-- ══════════════════════════════════════════════════════════════════

/-- A NaN-boxed value is memory-safe with respect to a heap: if it is a
    pointer, it points to a live object; otherwise it is an inline value
    (int, bool, none, float, pending) and requires no heap access. -/
def NanBoxSafe (h : Heap) (v : UInt64) : Prop :=
  IsPtr v → IsLive h (v &&& WasmNative.POINTER_MASK).toNat

/-- All values in a list are NaN-box safe. -/
def AllNanBoxSafe (h : Heap) (vs : List UInt64) : Prop :=
  ∀ v ∈ vs, NanBoxSafe h v

-- ══════════════════════════════════════════════════════════════════
-- Section 10: Composite safety property
-- ══════════════════════════════════════════════════════════════════

/-- The full memory safety property: heap invariant holds, all active
    values are safe, and refcounts are sound. This is the top-level
    property that MemorySafetyCorrect.lean proves is preserved by
    runtime operations. -/
structure MemorySafe (h : Heap) (liveAddrs : List Addr)
    (activeValues : List UInt64) (roots : Addr → Nat) : Prop where
  heapInv : HeapInvariant h
  valuesSafe : AllNanBoxSafe h activeValues
  refcountOk : RefcountSound h liveAddrs roots

end MoltTIR.Runtime.MemorySafety
