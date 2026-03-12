/-
  MoltTIR.AbstractInterp.Lattice — lattice-theoretic foundations for abstract
  interpretation, following Cousot & Cousot (1977).

  We formalize bounded lattices with decidable equality, prove the standard
  algebraic laws (commutativity, associativity, idempotency, absorption),
  and establish the Kleene fixed-point theorem for monotone functions on
  finite lattices together with the ascending chain condition.

  These definitions serve as the typeclass foundation that Molt's SCCP,
  constant propagation, and dataflow analyses build upon.
-/

namespace MoltTIR.AbstractInterp

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Bounded Lattice typeclass
-- ══════════════════════════════════════════════════════════════════

/-- A bounded lattice with decidable partial order.

    This is the core algebraic structure for abstract domains in
    abstract interpretation. The partial order models precision:
    `bot` is the most precise (no information) and `top` is the
    least precise (all values). -/
class BoundedLattice (α : Type) where
  /-- Decidable equality on elements. -/
  decEq : DecidableEq α
  /-- Bottom element (least element, most precise). -/
  bot : α
  /-- Top element (greatest element, least precise). -/
  top : α
  /-- Join (least upper bound, ⊔). -/
  join : α → α → α
  /-- Meet (greatest lower bound, ⊓). -/
  meet : α → α → α
  /-- Partial order. -/
  le : α → α → Prop
  /-- Decidable ordering. -/
  le_dec : DecidableRel le
  /-- ⊥ ≤ a for all a. -/
  bot_le : ∀ (a : α), le bot a
  /-- a ≤ ⊤ for all a. -/
  le_top : ∀ (a : α), le a top
  /-- ≤ is reflexive. -/
  le_refl : ∀ (a : α), le a a
  /-- ≤ is transitive. -/
  le_trans : ∀ (a b c : α), le a b → le b c → le a c
  /-- ≤ is antisymmetric. -/
  le_antisymm : ∀ (a b : α), le a b → le b a → a = b
  /-- Join is commutative. -/
  join_comm : ∀ (a b : α), join a b = join b a
  /-- Join is associative. -/
  join_assoc : ∀ (a b c : α), join (join a b) c = join a (join b c)
  /-- Join is idempotent. -/
  join_idem : ∀ (a : α), join a a = a
  /-- a ≤ a ⊔ b. -/
  le_join_left : ∀ (a b : α), le a (join a b)
  /-- b ≤ a ⊔ b. -/
  le_join_right : ∀ (a b : α), le b (join a b)
  /-- a ⊔ b is least upper bound. -/
  join_lub : ∀ (a b c : α), le a c → le b c → le (join a b) c
  /-- Meet is commutative. -/
  meet_comm : ∀ (a b : α), meet a b = meet b a
  /-- Meet is associative. -/
  meet_assoc : ∀ (a b c : α), meet (meet a b) c = meet a (meet b c)
  /-- Meet is idempotent. -/
  meet_idem : ∀ (a : α), meet a a = a
  /-- a ⊓ b ≤ a. -/
  meet_le_left : ∀ (a b : α), le (meet a b) a
  /-- a ⊓ b ≤ b. -/
  meet_le_right : ∀ (a b : α), le (meet a b) b
  /-- a ⊓ b is greatest lower bound. -/
  meet_glb : ∀ (a b c : α), le c a → le c b → le c (meet a b)

namespace BoundedLattice

variable {α : Type} [inst : BoundedLattice α]

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Absorption laws (derived)
-- ══════════════════════════════════════════════════════════════════

/-- Absorption law: a ⊔ (a ⊓ b) = a. -/
theorem join_meet_absorb (a b : α) : BoundedLattice.join a (BoundedLattice.meet a b) = a := by
  apply BoundedLattice.le_antisymm
  · apply BoundedLattice.join_lub
    · exact BoundedLattice.le_refl a
    · exact BoundedLattice.meet_le_left a b
  · exact BoundedLattice.le_join_left a (BoundedLattice.meet a b)

/-- Absorption law: a ⊓ (a ⊔ b) = a. -/
theorem meet_join_absorb (a b : α) : BoundedLattice.meet a (BoundedLattice.join a b) = a := by
  apply BoundedLattice.le_antisymm
  · exact BoundedLattice.meet_le_left a (BoundedLattice.join a b)
  · apply BoundedLattice.meet_glb
    · exact BoundedLattice.le_refl a
    · exact BoundedLattice.le_join_left a b

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Join characterizes the order
-- ══════════════════════════════════════════════════════════════════

/-- a ≤ b ↔ a ⊔ b = b. -/
theorem le_iff_join_eq (a b : α) : BoundedLattice.le a b ↔ BoundedLattice.join a b = b := by
  constructor
  · intro hab
    apply BoundedLattice.le_antisymm
    · exact BoundedLattice.join_lub a b b hab (BoundedLattice.le_refl b)
    · exact BoundedLattice.le_join_right a b
  · intro h
    rw [← h]
    exact BoundedLattice.le_join_left a b

/-- a ≤ b ↔ a ⊓ b = a. -/
theorem le_iff_meet_eq (a b : α) : BoundedLattice.le a b ↔ BoundedLattice.meet a b = a := by
  constructor
  · intro hab
    apply BoundedLattice.le_antisymm
    · exact BoundedLattice.meet_le_left a b
    · exact BoundedLattice.meet_glb a b a (BoundedLattice.le_refl a) hab
  · intro h
    rw [← h]
    exact BoundedLattice.meet_le_right a b

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Monotone functions
-- ══════════════════════════════════════════════════════════════════

/-- A function is monotone w.r.t. the lattice order. -/
def Monotone (f : α → α) : Prop :=
  ∀ (a b : α), BoundedLattice.le a b → BoundedLattice.le (f a) (f b)

/-- The identity function is monotone. -/
theorem monotone_id : Monotone (fun (a : α) => a) := by
  intro a b hab; exact hab

/-- Composition of monotone functions is monotone. -/
theorem monotone_comp {f g : α → α} (hf : Monotone f) (hg : Monotone g) :
    Monotone (fun (a : α) => f (g a)) := by
  intro a b hab
  exact hf (g a) (g b) (hg a b hab)

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Iterated application and Kleene chain
-- ══════════════════════════════════════════════════════════════════

/-- Iterate a function n times starting from ⊥. This is the Kleene chain:
    ⊥, f(⊥), f(f(⊥)), ... -/
def iterBot (f : α → α) : Nat → α
  | 0 => BoundedLattice.bot
  | n + 1 => f (iterBot f n)

/-- The Kleene chain is ascending for monotone functions. -/
theorem iterBot_ascending (f : α → α) (hf : Monotone f) :
    ∀ (n : Nat), BoundedLattice.le (iterBot f n) (iterBot f (n + 1)) := by
  intro n
  induction n with
  | zero =>
    show BoundedLattice.le BoundedLattice.bot (f BoundedLattice.bot)
    exact BoundedLattice.bot_le (f BoundedLattice.bot)
  | succ k ih =>
    show BoundedLattice.le (f (iterBot f k)) (f (iterBot f (k + 1)))
    exact hf _ _ ih

/-- If the chain stabilizes at step n, it stays stable forever. -/
theorem iterBot_stable (f : α → α) (_hf : Monotone f) (n : Nat)
    (hstab : iterBot f n = iterBot f (n + 1)) :
    ∀ (m : Nat), n ≤ m → iterBot f m = iterBot f n := by
  intro m hle
  induction m with
  | zero =>
    have : n = 0 := Nat.le_zero.mp hle
    subst this; rfl
  | succ k ih =>
    cases Nat.eq_or_lt_of_le hle with
    | inl h => rw [h]
    | inr h =>
      have hk : n ≤ k := Nat.lt_succ_iff.mp h
      have heq : iterBot f k = iterBot f n := ih hk
      show f (iterBot f k) = iterBot f n
      rw [heq]
      exact hstab.symm

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Finite lattice and ascending chain condition
-- ══════════════════════════════════════════════════════════════════

/-- A lattice has the ascending chain condition (ACC) if every ascending
    chain stabilizes. We model this via an explicit height bound. -/
class FiniteHeight (α : Type) [BoundedLattice α] where
  /-- Maximum height of any ascending chain. -/
  height : Nat
  /-- Every ascending chain stabilizes within `height` steps:
      for any monotone f, iterBot f stabilizes by step `height`. -/
  chain_bounded : ∀ (f : α → α), Monotone f →
    ∃ (n : Nat), n ≤ height ∧ iterBot f n = iterBot f (n + 1)

/-- Fixed point: f(x) = x. -/
def IsFixedPoint (f : α → α) (x : α) : Prop := f x = x

/-- On a finite-height lattice, every monotone function has a least fixed point
    reached within `height` iterations of the Kleene chain. -/
theorem kleene_lfp (f : α → α) (hf : Monotone f) [fh : FiniteHeight α] :
    ∃ (n : Nat), n ≤ fh.height ∧ IsFixedPoint f (iterBot f n) := by
  obtain ⟨n, hn_le, hn_eq⟩ := fh.chain_bounded f hf
  exact ⟨n, hn_le, hn_eq.symm⟩

/-- The Kleene fixed point is a least pre-fixed point: if f(y) ≤ y,
    then the fixed point ≤ y. We prove this by induction on the chain. -/
theorem iterBot_le_prefp (f : α → α) (hf : Monotone f)
    (y : α) (hy : BoundedLattice.le (f y) y)
    (n : Nat) : BoundedLattice.le (iterBot f n) y := by
  induction n with
  | zero =>
    show BoundedLattice.le BoundedLattice.bot y
    exact BoundedLattice.bot_le y
  | succ k ih =>
    show BoundedLattice.le (f (iterBot f k)) y
    exact BoundedLattice.le_trans _ _ _ (hf _ _ ih) hy

end BoundedLattice

end MoltTIR.AbstractInterp
