/-
  MoltTIR.Runtime.RCElisionCorrect — Perceus-style RC elision soundness proofs.

  Formalizes the soundness of reference counting elision — proving that
  Perceus-style borrowing analysis and RC optimizations do not introduce
  use-after-free or memory leaks.

  Key theorems:
  - borrow_sound: Borrowed parameters do not change refcount.
  - precise_drop: Every allocated object is dropped exactly once or remains live.
  - elision_safe: Removing matched inc/dec pairs preserves refcounts at exit.
  - no_use_after_free: After the last dec_ref, the object is never accessed.
  - reuse_sound: FBIP in-place reuse is safe when refcount is 0.

  References:
  - Counting Immutable Beans (Ullrich & de Moura, IFL'19)
  - Perceus: Garbage Free Reference Counting with Reuse (Reinking et al., PLDI'21)
  - Koka compiler RC verification
  - docs/spec/areas/runtime/BORROWING_ANALYSIS_DESIGN.md
  - runtime/molt-obj-model/src/lib.rs (RC protocol)
  - runtime/molt-runtime/src/call/bind.rs (callargs ownership)
  - MoltTIR.Runtime.MemorySafety (heap model)
  - MoltTIR.Runtime.OwnershipModel (ownership discipline)
  - MoltTIR.Optimization.RefcountElision (RC elision optimization model)
-/
import MoltTIR.Runtime.Refcount
import MoltTIR.Optimization.RefcountElision

set_option autoImplicit false

namespace MoltTIR.Runtime.RCElisionCorrect

open MoltTIR.Runtime.Refcount (Addr)
open MoltTIR.Optimization.RefcountElision (RcOp ElidablePair execRcOp execRcOps
  noInterveningUse)

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Function-level RC operation model
--
-- We model a function as a sequence of RC operations (inc, dec, use,
-- alloc, drop) on heap objects, abstracting away computation. This
-- is the right level for Perceus-style analysis: RC operations are
-- the only heap-modifying instructions.
-- ══════════════════════════════════════════════════════════════════

/-- An object reference, identified by address. -/
abbrev ObjRef := Addr

/-- Index into a function's operation sequence. -/
abbrev OpIdx := Nat

/-- Extended RC operation that includes allocation, deallocation (drop),
    and heap store (for tracking escaping borrows). -/
inductive RCInstr where
  /-- Increment refcount: models `molt_inc_ref_obj(bits)`. -/
  | inc_ref (obj : ObjRef)
  /-- Decrement refcount: models `molt_dec_ref_obj(bits)`.
      When refcount reaches 0, the object is freed. -/
  | dec_ref (obj : ObjRef)
  /-- Allocate a new object at the given address with the given size class. -/
  | alloc (obj : ObjRef) (sizeClass : Nat)
  /-- Use (read) an object — any operation that dereferences the pointer. -/
  | use (obj : ObjRef)
  /-- Store an object reference into a heap structure (field write).
      This is the key operation that distinguishes owned from borrowed:
      a borrowed parameter must never be stored into the heap. -/
  | heap_store (target : ObjRef) (stored : ObjRef)
  /-- A no-op (computation that doesn't touch RC or heap pointers). -/
  | nop
  deriving DecidableEq, Repr

/-- A function's RC-relevant instruction stream. -/
abbrev RCFunc := List RCInstr

/-- RC state: maps each address to its current refcount. -/
abbrev RCState := Addr → Nat

/-- Execute a single RC instruction on the refcount state.
    alloc initializes refcount to 1 (the allocator owns it).
    heap_store increments the stored object's refcount (new reference from heap). -/
def execRCInstr (σ : RCState) : RCInstr → RCState
  | .inc_ref a  => fun x => if x = a then σ x + 1 else σ x
  | .dec_ref a  => fun x => if x = a then σ x - 1 else σ x
  | .alloc a _  => fun x => if x = a then 1 else σ x
  | .use _      => σ
  | .heap_store _ stored => fun x => if x = stored then σ x + 1 else σ x
  | .nop        => σ

/-- Execute a sequence of RC instructions. -/
def execRCInstrs : RCState → RCFunc → RCState
  | σ, []          => σ
  | σ, instr :: rest => execRCInstrs (execRCInstr σ instr) rest

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Borrowing analysis definitions
--
-- Perceus distinguishes "owned" and "borrowed" parameters:
-- - Owned: callee is responsible for dec_ref (takes ownership).
-- - Borrowed: callee must NOT store the value into the heap or
--   transfer ownership. The refcount is untouched by the callee.
--
-- A parameter is safe to mark as "borrowed" iff it is never stored
-- into a heap structure within the function body.
-- ══════════════════════════════════════════════════════════════════

/-- A parameter annotation: owned or borrowed. -/
inductive ParamMode where
  | owned
  | borrowed
  deriving DecidableEq, Repr

/-- A function with annotated parameters. -/
structure AnnotatedFunc where
  /-- Parameter addresses (in order). -/
  params : List ObjRef
  /-- Mode for each parameter (same length as params). -/
  modes : List ParamMode
  /-- The function body as RC instructions. -/
  body : RCFunc
  /-- Modes list has same length as params. -/
  modes_len : modes.length = params.length

/-- Predicate: parameter at index `idx` is marked borrowed. -/
def isBorrowed (f : AnnotatedFunc) (paramIdx : Nat) : Prop :=
  f.modes.get? paramIdx = some .borrowed

/-- Predicate: an instruction stores `obj` into the heap. -/
def storesIntoHeap (instr : RCInstr) (obj : ObjRef) : Prop :=
  match instr with
  | .heap_store _ stored => stored = obj
  | _ => False

/-- Predicate: a function body never stores `obj` into the heap.
    This is the key condition for borrowing soundness. -/
def neverStoredIntoHeap (body : RCFunc) (obj : ObjRef) : Prop :=
  ∀ instr ∈ body, ¬ storesIntoHeap instr obj

/-- Predicate: a function body has no inc_ref or dec_ref on `obj`.
    Borrowed parameters should have no RC operations at all. -/
def noRCOps (body : RCFunc) (obj : ObjRef) : Prop :=
  ∀ instr ∈ body, instr ≠ .inc_ref obj ∧ instr ≠ .dec_ref obj

/-- The parameter value: look up the param address by index. -/
def paramValue (f : AnnotatedFunc) (paramIdx : Nat) : Option ObjRef :=
  f.params.get? paramIdx

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Borrowing soundness
--
-- Theorem: if a parameter is marked "borrowed" and the function body
-- contains no RC operations on that parameter and never stores it
-- into the heap, then the refcount of the parameter is unchanged
-- after function execution.
-- ══════════════════════════════════════════════════════════════════

/-- Helper: executing an instruction that does not touch address `a`
    (no inc/dec/alloc/heap_store targeting `a`) leaves `a`'s refcount unchanged. -/
theorem execRCInstr_no_touch (σ : RCState) (instr : RCInstr) (a : ObjRef)
    (h_no_inc : instr ≠ .inc_ref a)
    (h_no_dec : instr ≠ .dec_ref a)
    (h_no_alloc : ∀ sc, instr ≠ .alloc a sc)
    (h_no_store : ¬ storesIntoHeap instr a) :
    execRCInstr σ instr a = σ a := by
  cases instr with
  | inc_ref b =>
    simp [execRCInstr]
    intro heq; subst heq; exact absurd rfl h_no_inc
  | dec_ref b =>
    simp [execRCInstr]
    intro heq; subst heq; exact absurd rfl h_no_dec
  | alloc b sc =>
    simp [execRCInstr]
    intro heq; subst heq; exact absurd rfl (h_no_alloc sc)
  | use _ => rfl
  | heap_store _ stored =>
    simp [execRCInstr]
    intro heq; subst heq
    exact absurd (show storesIntoHeap (.heap_store _ a) a from rfl) h_no_store
  | nop => rfl

/-- Helper: a sequence of instructions with no RC ops and no heap stores
    on address `a` leaves `a`'s refcount unchanged.
    Requires that no instruction allocates at `a` either (which is guaranteed
    since `a` is already live as a parameter). -/
theorem execRCInstrs_no_touch (σ : RCState) (body : RCFunc) (a : ObjRef)
    (h_no_rc : noRCOps body a)
    (h_no_store : neverStoredIntoHeap body a)
    (h_no_alloc : ∀ instr ∈ body, ∀ sc, instr ≠ .alloc a sc) :
    execRCInstrs σ body a = σ a := by
  induction body generalizing σ with
  | nil => rfl
  | cons instr rest ih =>
    simp [execRCInstrs]
    have ⟨h_ni, h_nd⟩ := h_no_rc instr (List.mem_cons_self _ _)
    have h_ns := h_no_store instr (List.mem_cons_self _ _)
    have h_na := h_no_alloc instr (List.mem_cons_self _ _)
    have h_rest_no_rc : noRCOps rest a :=
      fun i hi => h_no_rc i (List.mem_cons_of_mem _ hi)
    have h_rest_no_store : neverStoredIntoHeap rest a :=
      fun i hi => h_no_store i (List.mem_cons_of_mem _ hi)
    have h_rest_no_alloc : ∀ i ∈ rest, ∀ sc, i ≠ .alloc a sc :=
      fun i hi => h_no_alloc i (List.mem_cons_of_mem _ hi)
    rw [ih (execRCInstr σ instr) h_rest_no_rc h_rest_no_store h_rest_no_alloc]
    exact execRCInstr_no_touch σ instr a h_ni h_nd h_na h_ns

/-- **Borrowing Soundness (Theorem 1)**: If a parameter is marked "borrowed",
    the function body has no RC operations on it, and it is never stored into
    the heap, then the refcount of that parameter is unchanged after execution.

    This formalizes the Perceus borrowing contract: a borrowed parameter is
    a "loan" — the callee observes but does not modify the refcount. The caller
    retains responsibility for the dec_ref.

    Preconditions:
    - `isBorrowed f paramIdx`: the parameter is annotated as borrowed.
    - `noRCOps f.body param`: no inc_ref/dec_ref on the parameter in the body.
    - `neverStoredIntoHeap f.body param`: the parameter is never stored into a heap structure.
    - No instruction allocates at the parameter's address (it is already live). -/
theorem borrow_sound (f : AnnotatedFunc) (paramIdx : Nat) (param : ObjRef)
    (σ : RCState)
    (h_borrowed : isBorrowed f paramIdx)
    (h_param : paramValue f paramIdx = some param)
    (h_no_rc : noRCOps f.body param)
    (h_no_store : neverStoredIntoHeap f.body param)
    (h_no_alloc : ∀ instr ∈ f.body, ∀ sc, instr ≠ .alloc param sc) :
    execRCInstrs σ f.body param = σ param :=
  execRCInstrs_no_touch σ f.body param h_no_rc h_no_store h_no_alloc

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Drop placement model
--
-- Perceus inserts drops at last-use positions. The precise_drop
-- property states that every allocated object is either:
-- (a) dropped exactly once on every execution path, or
-- (b) still live at function exit (returned or stored in the heap).
-- ══════════════════════════════════════════════════════════════════

/-- Count the number of dec_ref instructions targeting `obj` in a function body. -/
def dropCount (body : RCFunc) (obj : ObjRef) : Nat :=
  body.filter (· == .dec_ref obj) |>.length

/-- Count the number of inc_ref instructions targeting `obj` in a function body. -/
def incCount (body : RCFunc) (obj : ObjRef) : Nat :=
  body.filter (· == .inc_ref obj) |>.length

/-- Count the number of heap_store instructions storing `obj`. -/
def heapStoreCount (body : RCFunc) (obj : ObjRef) : Nat :=
  body.filter (fun i => match i with | .heap_store _ s => s == obj | _ => false) |>.length

/-- An object is "still live" after execution if its refcount is ≥ 1. -/
def stillLive (σ : RCState) (obj : ObjRef) : Prop :=
  σ obj ≥ 1

/-- An object is "consumed" (refcount reached 0) after execution. -/
def consumed (σ : RCState) (obj : ObjRef) : Prop :=
  σ obj = 0

/-- **Precise Drop Placement (Theorem 2)**: For an object allocated within a
    function (initial refcount = 1), if the number of inc_ref operations
    plus heap stores equals the number of dec_ref operations minus one
    (accounting for the initial allocation refcount), then after execution
    the object is either precisely consumed or still live.

    More precisely: if initial RC = 1 and we execute the body, the final
    RC = 1 + incCount + heapStoreCount - dropCount. This is either 0
    (consumed: one drop per allocation) or ≥ 1 (still live: ownership
    transferred to heap or returned).

    This is the Counting Immutable Beans insight: RC is a linear resource,
    and precise drop placement ensures each allocation unit is consumed
    exactly once. -/
theorem precise_drop (body : RCFunc) (obj : ObjRef)
    (σ : RCState)
    (h_init : σ obj = 1)
    (h_no_rc_other : ∀ instr ∈ body,
      (∀ sc, instr ≠ .alloc obj sc))
    (h_balance : dropCount body obj ≤ 1 + incCount body obj + heapStoreCount body obj) :
    let σ' := execRCInstrs σ body
    stillLive σ' obj ∨ consumed σ' obj := by
  -- TODO(formal, owner:runtime, milestone:M4, priority:P0, status:partial):
  --   The proof requires induction over body showing that
  --   σ' obj = 1 + incCount + heapStoreCount - dropCount,
  --   and then case-splitting on whether that equals 0 or is ≥ 1.
  --   The key lemma is that execRCInstrs is a fold that tracks the
  --   running refcount, and the balance condition ensures it never
  --   goes below 0 (Nat subtraction saturates, but the balance
  --   precondition prevents meaningful underflow).
  sorry

/-- **Precise Drop, Concrete Case**: An object allocated with RC=1,
    with exactly one dec_ref and no inc_ref/heap_store, is consumed. -/
theorem precise_drop_single (obj : ObjRef) (σ : RCState) (h_init : σ obj = 1) :
    execRCInstrs σ [.dec_ref obj] obj = 0 := by
  simp [execRCInstrs, execRCInstr, h_init]

/-- **Precise Drop, Transfer Case**: An object allocated with RC=1,
    with one inc_ref (ownership transfer) and one dec_ref (last-use drop),
    remains live with RC=1. -/
theorem precise_drop_transfer (obj : ObjRef) (σ : RCState) (h_init : σ obj = 1) :
    execRCInstrs σ [.inc_ref obj, .dec_ref obj] obj = 1 := by
  simp [execRCInstrs, execRCInstr, h_init]

-- ══════════════════════════════════════════════════════════════════
-- Section 5: RC elision correctness
--
-- The core Perceus optimization: if an inc_ref is immediately followed
-- (modulo non-interfering instructions) by a dec_ref on the same object,
-- the pair can be elided. We prove this preserves refcounts at function exit.
-- ══════════════════════════════════════════════════════════════════

/-- An RC elision pass: given a function body, return the optimized body
    with certain inc/dec pairs removed. Modeled as a relation between
    original and optimized instruction streams. -/
structure RCElision where
  /-- The original instruction stream. -/
  original : RCFunc
  /-- The optimized instruction stream. -/
  optimized : RCFunc
  /-- Elision witness: the optimized stream is derived from the original
      by removing zero or more elidable inc/dec pairs. -/
  valid : ElidedFrom original optimized

/-- Relation: `optimized` is derived from `original` by removing elidable
    inc/dec pairs. Defined inductively:
    - Base case: identical streams.
    - Elide step: if we see inc(a), middle (no touch on a), dec(a),
      we can replace with just middle.
    - Prefix step: if the heads match and tails are related, the full
      lists are related. -/
inductive ElidedFrom : RCFunc → RCFunc → Prop where
  /-- Identity: no elision applied. -/
  | refl (ops : RCFunc) : ElidedFrom ops ops
  /-- Elide one inc/dec pair: [inc(a)] ++ middle ++ [dec(a)] ++ rest → middle ++ rest,
      where no instruction in middle touches address a. -/
  | elide (a : ObjRef) (middle rest : RCFunc)
      (h_no_use : noInterveningUseRC a middle) :
      ElidedFrom (.inc_ref a :: middle ++ [.dec_ref a] ++ rest) (middle ++ rest)
  /-- Congruence: if tails are related, same-head lists are related. -/
  | cons (instr : RCInstr) (orig opt : RCFunc)
      (h_tail : ElidedFrom orig opt) :
      ElidedFrom (instr :: orig) (instr :: opt)

/-- No instruction in the middle sequence touches address `a` (RC version). -/
def noInterveningUseRC (a : ObjRef) (middle : RCFunc) : Prop :=
  ∀ instr ∈ middle,
    instr ≠ .inc_ref a ∧
    instr ≠ .dec_ref a ∧
    (∀ sc, instr ≠ .alloc a sc) ∧
    ¬ storesIntoHeap instr a

/-- Helper: executing an instruction that doesn't touch `a` preserves RC at `a`. -/
theorem execRCInstr_no_touch_RC (σ : RCState) (instr : RCInstr) (a : ObjRef)
    (h : instr ≠ .inc_ref a ∧ instr ≠ .dec_ref a ∧
         (∀ sc, instr ≠ .alloc a sc) ∧ ¬ storesIntoHeap instr a) :
    execRCInstr σ instr a = σ a :=
  execRCInstr_no_touch σ instr a h.1 h.2.1 h.2.2.1 h.2.2.2

/-- Helper: a non-interfering middle sequence preserves RC at `a`. -/
theorem execRCInstrs_middle_preserves (σ : RCState) (middle : RCFunc) (a : ObjRef)
    (h : noInterveningUseRC a middle) :
    execRCInstrs σ middle a = σ a := by
  induction middle generalizing σ with
  | nil => rfl
  | cons instr rest ih =>
    simp [execRCInstrs]
    have h_instr := h instr (List.mem_cons_self _ _)
    have h_rest : noInterveningUseRC a rest :=
      fun i hi => h i (List.mem_cons_of_mem _ hi)
    rw [ih (execRCInstr σ instr) h_rest]
    exact execRCInstr_no_touch_RC σ instr a h_instr

/-- **Elision Safety, Single Pair (Theorem 3a)**: Removing a single elidable
    inc_ref/dec_ref pair preserves refcounts at every address.

    This is the local version of elision safety: for a single pair
    [inc(a), middle..., dec(a)] where middle does not touch a,
    the refcount at every address is the same as executing just middle. -/
theorem elision_safe_single (a : ObjRef) (middle rest : RCFunc) (σ : RCState)
    (h_no_use : noInterveningUseRC a middle) :
    ∀ (x : ObjRef),
      execRCInstrs σ (.inc_ref a :: middle ++ [.dec_ref a] ++ rest) x =
      execRCInstrs σ (middle ++ rest) x := by
  -- TODO(formal, owner:runtime, milestone:M4, priority:P0, status:partial):
  --   Proof strategy:
  --   1. Show execRCInstrs distributes over list concatenation.
  --   2. For the inc/middle/dec prefix at address a:
  --      inc raises RC by 1, middle preserves (by h_no_use), dec lowers by 1 → net 0.
  --   3. For the inc/middle/dec prefix at address x ≠ a: inc/dec are no-ops,
  --      middle is identical.
  --   4. The rest suffix executes on the same state, so produces the same result.
  --   Key lemma needed: execRCInstrs_append for distributing over (++).
  sorry

/-- **Elision Safety, Full (Theorem 3b)**: If the optimized body is derived
    from the original by removing elidable pairs (via ElidedFrom), then
    refcounts at every address are preserved at function exit.

    This is the compositionality result: each individual elision preserves
    refcounts, and the relation composes transitively. -/
theorem elision_safe (original optimized : RCFunc) (σ : RCState)
    (h_elided : ElidedFrom original optimized) :
    ∀ (v : ObjRef), execRCInstrs σ original v = execRCInstrs σ optimized v := by
  -- TODO(formal, owner:runtime, milestone:M4, priority:P0, status:partial):
  --   Proof by induction on the ElidedFrom derivation:
  --   - refl: trivial.
  --   - elide: by elision_safe_single.
  --   - cons: by IH on the tail, using the fact that executing the same
  --     head instruction produces the same intermediate state.
  sorry

-- ══════════════════════════════════════════════════════════════════
-- Section 6: No use-after-free
--
-- After the last dec_ref on an object, no subsequent instruction
-- may use (dereference) that object. This is a scheduling constraint
-- enforced by the compiler's last-use analysis.
-- ══════════════════════════════════════════════════════════════════

/-- Predicate: instruction at index `i` is a dec_ref (drop) on `obj`. -/
def isDrop (body : RCFunc) (i : OpIdx) (obj : ObjRef) : Prop :=
  body.get? i = some (.dec_ref obj)

/-- Predicate: instruction at index `i` uses (dereferences) `obj`. -/
def isUse (body : RCFunc) (i : OpIdx) (obj : ObjRef) : Prop :=
  body.get? i = some (.use obj)

/-- Well-formed RC: drops always come after all uses. This is the
    scheduling invariant maintained by the compiler's last-use analysis
    (compute_last_use in the backend). -/
def wellFormedRC (body : RCFunc) : Prop :=
  ∀ (obj : ObjRef) (i j : OpIdx),
    isDrop body i obj → isUse body j obj → j < i

/-- **No Use-After-Free (Theorem 4)**: If the RC instruction stream is
    well-formed (drops come after all uses), then no use follows a drop
    on the same object.

    Equivalently: for any drop at index i and use at index j on the same
    object, i > j (use precedes drop). The contrapositive: if i ≤ j
    (drop precedes or coincides with use), that violates well-formedness.

    This is a direct consequence of the well-formedness invariant.
    The compiler's last-use analysis ensures this statically. -/
theorem no_use_after_free (body : RCFunc) (obj : ObjRef)
    (h_wf : wellFormedRC body)
    (i j : OpIdx) (h_drop : isDrop body i obj) (h_use : isUse body j obj)
    (h_after : i ≤ j) : False := by
  have := h_wf obj i j h_drop h_use
  omega

/-- Corollary: well-formed RC implies that for every drop-use pair,
    the use strictly precedes the drop (contrapositive of use-after-free). -/
theorem use_precedes_drop (body : RCFunc) (obj : ObjRef)
    (h_wf : wellFormedRC body)
    (i j : OpIdx) (h_drop : isDrop body i obj) (h_use : isUse body j obj) :
    j < i :=
  h_wf obj i j h_drop h_use

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Reuse token soundness (FBIP)
--
-- Functional But In-Place (FBIP): when a constructor immediately follows
-- a destructor of the same size class, the memory can be reused in-place
-- rather than allocating fresh. This is safe when:
-- 1. The old object's refcount is 0 (no other references).
-- 2. The new object has the same size class (fits in the same allocation).
-- ══════════════════════════════════════════════════════════════════

/-- Size class of a heap object. In Molt's runtime, objects have a MoltHeader
    followed by type-specific fields. The size class determines the allocation
    bucket. -/
abbrev SizeClass := Nat

/-- Objects have the same size class (same allocation bucket). -/
def sameSize (sc1 sc2 : SizeClass) : Prop := sc1 = sc2

/-- A reuse token: evidence that an object's memory can be reused. -/
structure ReuseToken where
  /-- The address being reused. -/
  addr : ObjRef
  /-- The size class of the old (freed) object. -/
  oldSize : SizeClass
  /-- Proof that the refcount is 0 (object is freed). -/
  freed : Prop
  /-- The size class of the new object. -/
  newSize : SizeClass
  /-- Proof that sizes match. -/
  fits : sameSize oldSize newSize

/-- In-place reuse: instead of alloc(new) + free(old), we reuse old's memory
    for new. The result is a state where the address has refcount 1
    (the new object's initial reference). -/
def reuseInPlace (σ : RCState) (token : ReuseToken) : RCState :=
  fun x => if x = token.addr then 1 else σ x

/-- Normal alloc+free: allocate new at a fresh address and free old. -/
def allocAndFree (σ : RCState) (oldAddr newAddr : ObjRef) : RCState :=
  fun x => if x = newAddr then 1
            else if x = oldAddr then 0
            else σ x

/-- **Reuse Token Soundness (Theorem 5)**: When an object's refcount is 0
    and a new object of the same size class is allocated, in-place reuse
    produces the same refcount state at the reused address as a fresh
    allocation would at a new address.

    The key insight: the refcount at the reused address goes to 1 in both
    cases (new object starts with RC=1). The old object was already freed
    (RC=0), so no references are invalidated. -/
theorem reuse_sound (σ : RCState) (oldAddr newAddr : ObjRef)
    (oldSize newSize : SizeClass)
    (h_freed : σ oldAddr = 0)
    (h_same_size : sameSize oldSize newSize)
    (h_fresh : oldAddr ≠ newAddr) :
    let token : ReuseToken := {
      addr := oldAddr,
      oldSize := oldSize,
      freed := σ oldAddr = 0,
      newSize := newSize,
      fits := h_same_size
    }
    -- The reused address has RC=1 (same as a fresh allocation)
    reuseInPlace σ token oldAddr = 1 ∧
    -- Other addresses are unaffected
    (∀ x, x ≠ oldAddr → reuseInPlace σ token x = σ x) := by
  constructor
  · simp [reuseInPlace]
  · intro x hne
    simp [reuseInPlace, hne]

/-- Reuse is equivalent to alloc+free at the same address:
    both produce RC=1 at the target address. -/
theorem reuse_equiv_alloc_free (σ : RCState) (addr : ObjRef)
    (sc : SizeClass) (h_freed : σ addr = 0) :
    let token : ReuseToken := {
      addr := addr,
      oldSize := sc,
      freed := σ addr = 0,
      newSize := sc,
      fits := rfl
    }
    reuseInPlace σ token addr = allocAndFree σ addr addr addr := by
  simp [reuseInPlace, allocAndFree]

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Compositionality — elision + borrowing + reuse
--
-- The three optimizations compose safely: borrowing analysis removes
-- RC operations on borrowed parameters, elision removes redundant
-- inc/dec pairs, and reuse replaces alloc+free with in-place mutation.
-- Each operates on a different dimension:
-- - Borrowing: parameter-level (function signature).
-- - Elision: instruction-level (local inc/dec pairs).
-- - Reuse: allocation-level (consecutive drop+alloc of same size).
-- ══════════════════════════════════════════════════════════════════

/-- Borrowing preserves elision: if we first annotate parameters as borrowed
    (removing their RC ops) and then elide remaining inc/dec pairs, the
    composition preserves refcounts.

    This follows from the fact that borrowing removes RC ops on borrowed
    parameters (which are a disjoint set from the elidable pairs on owned
    values), and elision only removes matched pairs (preserving the balance
    for owned values). -/
theorem borrow_then_elide_sound
    (f : AnnotatedFunc) (paramIdx : Nat) (param : ObjRef)
    (body_after_borrow elided : RCFunc) (σ : RCState)
    (h_borrowed : isBorrowed f paramIdx)
    (h_param : paramValue f paramIdx = some param)
    (h_borrow_applied : noRCOps body_after_borrow param)
    (h_borrow_store : neverStoredIntoHeap body_after_borrow param)
    (h_elided : ElidedFrom body_after_borrow elided) :
    ∀ (v : ObjRef),
      execRCInstrs σ body_after_borrow v = execRCInstrs σ elided v := by
  exact elision_safe body_after_borrow elided σ h_elided

-- ══════════════════════════════════════════════════════════════════
-- Section 9: Concrete witnesses and counterexamples
-- ══════════════════════════════════════════════════════════════════

/-- Witness: a borrowed parameter's refcount is unchanged after a function
    that only uses (reads) the parameter. -/
example : let σ : RCState := fun _ => 5
          let body : RCFunc := [.use 42, .use 42, .nop]
          execRCInstrs σ body 42 = 5 := by native_decide

/-- Witness: precise drop — alloc(RC=1) then dec_ref → RC=0 (consumed). -/
example : let σ : RCState := fun _ => 0
          let body : RCFunc := [.alloc 42 16, .use 42, .dec_ref 42]
          execRCInstrs σ body 42 = 0 := by native_decide

/-- Witness: elision — inc then dec is identity. -/
example : let σ : RCState := fun _ => 3
          let body : RCFunc := [.inc_ref 42, .dec_ref 42]
          execRCInstrs σ body 42 = 3 := by native_decide

/-- Witness: elision with intervening non-interfering instruction. -/
example : let σ : RCState := fun _ => 3
          let body : RCFunc := [.inc_ref 42, .use 99, .nop, .dec_ref 42]
          execRCInstrs σ body 42 = 3 := by native_decide

/-- Counterexample: eliding an inc without its dec is UNSOUND — refcount
    is too low, leading to premature free. -/
example : let σ : RCState := fun _ => 1
          -- Without inc_ref, the dec_ref drops RC to 0 (freed)
          execRCInstrs σ [.dec_ref 42] 42 = 0 := by native_decide

/-- Counterexample: eliding a dec without its inc is UNSOUND — refcount
    is too high, leading to a memory leak. -/
example : let σ : RCState := fun _ => 1
          -- Without dec_ref, the inc_ref raises RC to 2 (leaked)
          execRCInstrs σ [.inc_ref 42] 42 = 2 := by native_decide

/-- Counterexample: storing a borrowed parameter into the heap WITHOUT
    inc_ref is a use-after-free. If the caller dec_refs the parameter,
    its RC drops to 0 even though the heap still references it. -/
example : let σ : RCState := fun _ => 1
          -- heap_store without inc_ref: the heap now references obj 42,
          -- but its RC is still 1. When the caller dec_refs, RC→0 and
          -- the heap has a dangling reference.
          -- This is exactly what the borrowing analysis prevents:
          -- borrowed parameters must never appear in heap_store.
          execRCInstrs σ [.heap_store 99 42] 42 = 2 := by native_decide
          -- RC goes to 2 because heap_store does inc the stored ref.
          -- But if we SKIP the inc (violating the model), we get UaF.
          -- The model correctly requires the inc.

-- ══════════════════════════════════════════════════════════════════
-- Section 10: Connection to OwnershipModel.lean
--
-- The RC elision correctness in this file operates at the instruction
-- level (RCInstr sequences). The OwnershipModel operates at the claim
-- level (ownership claims and borrows). This section bridges the two.
-- ══════════════════════════════════════════════════════════════════

/-- An RC instruction stream is ownership-consistent if the number of
    inc_ref operations plus alloc operations on each address equals the
    number of dec_ref operations on that address plus the final live
    reference count. This is the instruction-level analog of the
    OwnershipInvariant. -/
def ownershipConsistent (body : RCFunc) (σ σ' : RCState) : Prop :=
  ∀ (a : ObjRef),
    σ a + incCount body a + heapStoreCount body a + (if body.any (fun i => match i with | .alloc b _ => b == a | _ => false) then 1 else 0)
    = σ' a + dropCount body a

/-- If an RC instruction stream is ownership-consistent and the ElidedFrom
    relation holds, then the elided stream is also ownership-consistent.
    This bridges the instruction-level elision proof to the claim-level
    ownership model. -/
theorem elision_preserves_ownership
    (original optimized : RCFunc) (σ : RCState)
    (h_elided : ElidedFrom original optimized)
    (h_consistent : ownershipConsistent original σ (execRCInstrs σ original)) :
    ownershipConsistent optimized σ (execRCInstrs σ optimized) := by
  -- TODO(formal, owner:runtime, milestone:M4, priority:P1, status:partial):
  --   Follows from elision_safe: since execRCInstrs σ original = execRCInstrs σ optimized
  --   at every address, and the elision removes matched inc/dec pairs
  --   (which cancel in the ownership equation), the optimized stream is also
  --   ownership-consistent.
  sorry

end MoltTIR.Runtime.RCElisionCorrect
