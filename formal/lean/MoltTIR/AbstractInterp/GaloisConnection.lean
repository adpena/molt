/-
  MoltTIR.AbstractInterp.GaloisConnection — Galois connections between
  concrete and abstract domains, following Cousot & Cousot (1977).

  A Galois connection (α, γ) between a concrete poset C and an abstract
  poset A provides the formal bridge for soundness of abstract interpretation:
  the abstraction α over-approximates concrete sets, and the concretization γ
  gives meaning to abstract elements.

  We prove the standard derived properties:
  - Monotonicity of α and γ
  - Reductive and extensive composites (α ∘ γ ≤ id, id ≤ γ ∘ α)
  - Soundness of abstract operations
-/
import MoltTIR.AbstractInterp.Lattice

namespace MoltTIR.AbstractInterp

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Galois connection structure
-- ══════════════════════════════════════════════════════════════════

/-- A Galois connection between a concrete domain C and an abstract domain A.
    `abstr` is the abstraction function (α), `concr` is the concretization
    function (γ). The Galois connection property states:
      α(c) ≤_A a  ↔  c ≤_C γ(a)
    for all c : C, a : A. -/
structure GaloisConnection (C : Type) (A : Type)
    [BoundedLattice C] [BoundedLattice A] where
  /-- Abstraction function α : C → A. -/
  abstr : C → A
  /-- Concretization function γ : A → C. -/
  concr : A → C
  /-- The Galois connection property. -/
  gc : ∀ (c : C) (a : A), BoundedLattice.le (abstr c) a ↔ BoundedLattice.le c (concr a)

namespace GaloisConnection

variable {C : Type} {A : Type} [BoundedLattice C] [BoundedLattice A]
variable (g : GaloisConnection C A)

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Monotonicity of α and γ
-- ══════════════════════════════════════════════════════════════════

/-- The abstraction function α is monotone. -/
theorem abstr_monotone :
    ∀ (c₁ c₂ : C), BoundedLattice.le c₁ c₂ →
      BoundedLattice.le (g.abstr c₁) (g.abstr c₂) := by
  intro c₁ c₂ hle
  rw [g.gc]
  apply BoundedLattice.le_trans c₁ c₂ (g.concr (g.abstr c₂))
  · exact hle
  · exact (g.gc c₂ (g.abstr c₂)).mp (BoundedLattice.le_refl _)

/-- The concretization function γ is monotone. -/
theorem concr_monotone :
    ∀ (a₁ a₂ : A), BoundedLattice.le a₁ a₂ →
      BoundedLattice.le (g.concr a₁) (g.concr a₂) := by
  intro a₁ a₂ hle
  rw [← g.gc]
  apply BoundedLattice.le_trans (g.abstr (g.concr a₁)) a₁ a₂
  · exact (g.gc (g.concr a₁) a₁).mpr (BoundedLattice.le_refl _)
  · exact hle

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Extensive and reductive composites
-- ══════════════════════════════════════════════════════════════════

/-- γ ∘ α is extensive: c ≤ γ(α(c)) for all c. -/
theorem extensive (c : C) : BoundedLattice.le c (g.concr (g.abstr c)) :=
  (g.gc c (g.abstr c)).mp (BoundedLattice.le_refl _)

/-- α ∘ γ is reductive: α(γ(a)) ≤ a for all a. -/
theorem reductive (a : A) : BoundedLattice.le (g.abstr (g.concr a)) a :=
  (g.gc (g.concr a) a).mpr (BoundedLattice.le_refl _)

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Idempotence of composites
-- ══════════════════════════════════════════════════════════════════

/-- α ∘ γ ∘ α = α (abstraction is idempotent after concretization round-trip). -/
theorem abstr_concr_abstr (c : C) :
    g.abstr (g.concr (g.abstr c)) = g.abstr c := by
  apply BoundedLattice.le_antisymm
  · exact g.reductive (g.abstr c)
  · exact g.abstr_monotone c (g.concr (g.abstr c)) (g.extensive c)

/-- γ ∘ α ∘ γ = γ (concretization is idempotent after abstraction round-trip). -/
theorem concr_abstr_concr (a : A) :
    g.concr (g.abstr (g.concr a)) = g.concr a := by
  apply BoundedLattice.le_antisymm
  · exact g.concr_monotone (g.abstr (g.concr a)) a (g.reductive a)
  · exact g.extensive (g.concr a)

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Soundness of abstract operations
-- ══════════════════════════════════════════════════════════════════

/-- An abstract function f♯ is a sound approximation of a concrete function f
    if: for all a, f(γ(a)) ≤ γ(f♯(a)).
    Equivalently (by GC): α(f(γ(a))) ≤ f♯(a). -/
def SoundAbstraction (f : C → C) (f_sharp : A → A) : Prop :=
  ∀ (a : A), BoundedLattice.le (f (g.concr a)) (g.concr (f_sharp a))

/-- Soundness with explicit monotonicity of the concrete function:
    if f is monotone, f♯ is sound, and c ≤ γ(a), then f(c) ≤ γ(f♯(a)). -/
theorem sound_abstraction_correct_mono (f : C → C) (f_sharp : A → A)
    (hf_mono : BoundedLattice.Monotone f)
    (hsound : SoundAbstraction g f f_sharp)
    (c : C) (a : A) (hc : BoundedLattice.le c (g.concr a)) :
    BoundedLattice.le (f c) (g.concr (f_sharp a)) :=
  BoundedLattice.le_trans _ _ _ (hf_mono c (g.concr a) hc) (hsound a)

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Best abstract transformer
-- ══════════════════════════════════════════════════════════════════

/-- The best (most precise) abstract transformer for f is α ∘ f ∘ γ. -/
def bestAbstraction (f : C → C) : A → A :=
  fun (a : A) => g.abstr (f (g.concr a))

/-- The best abstraction is always a sound abstraction. -/
theorem best_is_sound (f : C → C) :
    SoundAbstraction g f (bestAbstraction g f) := by
  intro a
  exact g.extensive (f (g.concr a))

end GaloisConnection

end MoltTIR.AbstractInterp
