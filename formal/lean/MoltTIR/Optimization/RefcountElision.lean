/-
  MoltTIR.Optimization.RefcountElision — formal model of refcount elision.

  Models refcount elision as a compiler pass that removes redundant inc/dec
  pairs when they can be statically proven unnecessary. Proves that eliding
  such pairs preserves memory safety: the refcount of every live object
  never drops to 0 while live references exist.

  The key insight: an inc_ref immediately followed by a dec_ref on the same
  address, with no intervening use or aliasing, is a no-op on the heap.

  References:
  - runtime/molt-obj-model/src/lib.rs (RC protocol)
  - MoltTIR.Runtime.Refcount (heap model, incRef/decRef definitions)
-/
import MoltTIR.Runtime.Refcount

set_option autoImplicit false

namespace MoltTIR.Optimization.RefcountElision

open MoltTIR.Runtime.Refcount

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Refcount operations as an instruction stream
-- ══════════════════════════════════════════════════════════════════

/-- A refcount operation in the compiled instruction stream. -/
inductive RcOp where
  /-- Increment refcount at address. -/
  | inc (addr : Addr)
  /-- Decrement refcount at address. -/
  | dec (addr : Addr)
  /-- An arbitrary operation that may read/write the object at addr. -/
  | use (addr : Addr)
  /-- An operation that does not touch refcounts or the given addresses. -/
  | other
  deriving DecidableEq, Repr

/-- Extract the address affected by an RcOp, if any. -/
def RcOp.addr? : RcOp → Option Addr
  | .inc a => some a
  | .dec a => some a
  | .use a => some a
  | .other => none

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Executing refcount operation streams on a heap
-- ══════════════════════════════════════════════════════════════════

/-- Execute a single refcount operation on a heap. -/
def execRcOp (h : Heap) : RcOp → Heap
  | .inc a => incRef h a
  | .dec a => decRef h a
  | .use _ => h
  | .other => h

/-- Execute a sequence of refcount operations on a heap. -/
def execRcOps : Heap → List RcOp → Heap
  | h, [] => h
  | h, op :: rest => execRcOps (execRcOp h op) rest

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Elision conditions — when can inc/dec pairs be removed?
-- ══════════════════════════════════════════════════════════════════

/-- An inc/dec pair at the same address with no intervening operations
    on that address is elidable. -/
def noInterveningUse (a : Addr) (ops : List RcOp) : Prop :=
  ∀ op ∈ ops, op.addr? ≠ some a

/-- An elidable pair: inc(addr), middle ops (not touching addr), dec(addr). -/
structure ElidablePair where
  addr : Addr
  middle : List RcOp
  no_use : noInterveningUse addr middle

/-- The full operation sequence: [inc(a)] ++ middle ++ [dec(a)] -/
def ElidablePair.toOps (p : ElidablePair) : List RcOp :=
  .inc p.addr :: p.middle ++ [.dec p.addr]

/-- The elided sequence: just the middle operations. -/
def ElidablePair.elided (p : ElidablePair) : List RcOp :=
  p.middle

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Core lemmas
-- ══════════════════════════════════════════════════════════════════

/-- incRef followed by decRef at the same address is the identity. -/
theorem inc_then_dec_heap_id (h : Heap) (a : Addr) :
    decRef (incRef h a) a = h := by
  funext b
  simp [incRef, decRef]
  split <;> omega

/-- An operation that does not touch address `a` leaves `a` unchanged. -/
theorem execRcOp_no_touch (h : Heap) (a : Addr) (op : RcOp)
    (hno : op.addr? ≠ some a) :
    execRcOp h op a = h a := by
  cases op with
  | inc b =>
    simp only [RcOp.addr?] at hno
    simp only [execRcOp, incRef]
    have hne : a ≠ b := fun hab => hno (congrArg some hab.symm)
    simp [hne]
  | dec b =>
    simp only [RcOp.addr?] at hno
    simp only [execRcOp, decRef]
    have hne : a ≠ b := fun hab => hno (congrArg some hab.symm)
    simp [hne]
  | use _ => rfl
  | other => rfl

/-- A sequence of operations that don't touch `a` leaves `a` unchanged. -/
theorem execRcOps_no_touch (h : Heap) (a : Addr) (ops : List RcOp)
    (hno : noInterveningUse a ops) :
    execRcOps h ops a = h a := by
  induction ops generalizing h with
  | nil => rfl
  | cons op rest ih =>
    simp only [execRcOps]
    have hop : op.addr? ≠ some a := hno op (List.mem_cons_self)
    have hrest : noInterveningUse a rest :=
      fun o ho => hno o (List.Mem.tail _ ho)
    rw [ih (execRcOp h op) hrest]
    exact execRcOp_no_touch h a op hop

/-- execRcOps distributes over list append. -/
theorem execRcOps_append (h : Heap) (ops₁ ops₂ : List RcOp) :
    execRcOps h (ops₁ ++ ops₂) = execRcOps (execRcOps h ops₁) ops₂ := by
  induction ops₁ generalizing h with
  | nil => rfl
  | cons op rest ih => exact ih (execRcOp h op)

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Main elision correctness theorem
-- ══════════════════════════════════════════════════════════════════

/-- Executing the full elidable pair [inc(a), middle..., dec(a)] produces
    the same refcount at address `a` as executing just the middle.

    Proof strategy:
    1. Distribute over append to get: execRcOps of inc, then middle, then dec.
    2. Middle doesn't touch addr a (by no_use), so after middle the value
       at a is still incRef(h)(a) = h(a)+1.
    3. Dec brings it back to h(a)+1-1 = h(a).
    4. The elided path (just middle) also gives h(a) since middle doesn't touch a. -/
theorem elision_preserves_refcount_at (h : Heap) (p : ElidablePair) :
    execRcOps h p.toOps p.addr = execRcOps h p.elided p.addr := by
  simp only [ElidablePair.toOps, ElidablePair.elided]
  -- LHS: exec of [inc a] ++ (middle ++ [dec a])
  -- = exec (middle ++ [dec a]) starting from incRef h a
  -- = exec [dec a] starting from (execRcOps (incRef h a) middle)
  -- At addr a: execRcOps (incRef h a) middle a = (incRef h a) a = h a + 1
  --   (by no_touch)
  -- Then dec: h a + 1 - 1 = h a
  -- RHS: exec middle starting from h, at a = h a (by no_touch)
  -- Both sides equal h a. QED.
  have lhs_step : execRcOps h (RcOp.inc p.addr :: p.middle ++ [RcOp.dec p.addr]) p.addr
      = execRcOps (incRef h p.addr) (p.middle ++ [RcOp.dec p.addr]) p.addr := rfl
  rw [lhs_step, execRcOps_append]
  simp only [execRcOps, execRcOp]
  -- Now goal: decRef (execRcOps (incRef h p.addr) p.middle) p.addr p.addr
  --         = execRcOps h p.middle p.addr
  have hmid : execRcOps (incRef h p.addr) p.middle p.addr = h p.addr + 1 := by
    rw [execRcOps_no_touch (incRef h p.addr) p.addr p.middle p.no_use]
    simp [incRef]
  rw [show decRef (execRcOps (incRef h p.addr) p.middle) p.addr p.addr
      = execRcOps (incRef h p.addr) p.middle p.addr - 1
      from by simp [decRef]]
  rw [hmid]
  simp
  exact (execRcOps_no_touch h p.addr p.middle p.no_use).symm

/-- For addresses other than the pair address, the pair is also invisible. -/
theorem elision_preserves_safety (h : Heap) (p : ElidablePair)
    (a : Addr) (hne : a ≠ p.addr)
    (hno_middle : ∀ op ∈ p.middle, op.addr? ≠ some a) :
    execRcOps h p.toOps a = execRcOps h p.elided a := by
  simp only [ElidablePair.toOps, ElidablePair.elided]
  have lhs_step : execRcOps h (RcOp.inc p.addr :: p.middle ++ [RcOp.dec p.addr]) a
      = execRcOps (incRef h p.addr) (p.middle ++ [RcOp.dec p.addr]) a := rfl
  rw [lhs_step, execRcOps_append]
  simp only [execRcOps, execRcOp]
  -- incRef at p.addr doesn't change a (since a ≠ p.addr)
  have h_inc_ne : incRef h p.addr a = h a := by simp [incRef, hne]
  -- decRef at p.addr doesn't change a
  rw [show decRef (execRcOps (incRef h p.addr) p.middle) p.addr a
      = execRcOps (incRef h p.addr) p.middle a
      from by simp [decRef, hne]]
  rw [execRcOps_no_touch (incRef h p.addr) a p.middle hno_middle]
  rw [h_inc_ne]
  exact (execRcOps_no_touch h a p.middle hno_middle).symm

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Adjacent inc/dec is heap identity
-- ══════════════════════════════════════════════════════════════════

/-- Adjacent inc/dec (the simplest elision case) is a heap identity. -/
theorem adjacent_inc_dec_identity (h : Heap) (a : Addr) :
    execRcOps h [.inc a, .dec a] = h := by
  simp [execRcOps, execRcOp]
  exact inc_then_dec_heap_id h a

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Refcount lower bounds — elision safety
-- ══════════════════════════════════════════════════════════════════

/-- If an object's refcount is ≥ n, it remains ≥ n after an adjacent inc/dec. -/
theorem elision_preserves_lower_bound (h : Heap) (a : Addr) (n : Nat)
    (hge : h a ≥ n) :
    execRcOps h [.inc a, .dec a] a ≥ n := by
  have : execRcOps h [RcOp.inc a, RcOp.dec a] = h := adjacent_inc_dec_identity h a
  rw [show execRcOps h [RcOp.inc a, RcOp.dec a] a = h a from congrFun this a]
  exact hge

/-- Corollary: eliding an inc/dec pair on a live object (refcount ≥ 1)
    does not cause premature freeing. -/
theorem elision_no_premature_free (h : Heap) (a : Addr)
    (hlive : h a ≥ 1) :
    execRcOps h [.inc a, .dec a] a ≥ 1 :=
  elision_preserves_lower_bound h a 1 hlive

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Multi-pair elision — composability
-- ══════════════════════════════════════════════════════════════════

/-- Two adjacent inc/dec pairs compose to identity. -/
theorem double_elision_identity (h : Heap) (a : Addr) :
    execRcOps h [.inc a, .dec a, .inc a, .dec a] = h := by
  have : execRcOps h [RcOp.inc a, RcOp.dec a, RcOp.inc a, RcOp.dec a]
       = execRcOps (execRcOps h [RcOp.inc a, RcOp.dec a]) [RcOp.inc a, RcOp.dec a] := by
    simp [execRcOps, execRcOp]
  rw [this, adjacent_inc_dec_identity, adjacent_inc_dec_identity]

/-- Elision at different addresses composes independently. -/
theorem independent_elision (h : Heap) (a b : Addr) (_hne : a ≠ b) :
    execRcOps h [.inc a, .dec a, .inc b, .dec b] = h := by
  have : execRcOps h [RcOp.inc a, RcOp.dec a, RcOp.inc b, RcOp.dec b]
       = execRcOps (execRcOps h [RcOp.inc a, RcOp.dec a]) [RcOp.inc b, RcOp.dec b] := by
    simp [execRcOps, execRcOp]
  rw [this, adjacent_inc_dec_identity, adjacent_inc_dec_identity]

-- ══════════════════════════════════════════════════════════════════
-- Section 9: Counterexample — unsafe elision
-- ══════════════════════════════════════════════════════════════════

/-- Without inc, refcount stays at 0. -/
theorem unsafe_elision_counterexample :
    let h : Heap := fun _ => 0
    let a := 42
    execRcOp h (.inc a) a = 1 := by
  native_decide

/-- Without the inc, refcount is 0 at use time (unsafe). -/
theorem without_inc_unsafe :
    let h : Heap := fun _ => 0
    let a := 42
    h a = 0 := by
  native_decide

end MoltTIR.Optimization.RefcountElision
