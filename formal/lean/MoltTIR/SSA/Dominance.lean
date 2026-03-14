/-
  MoltTIR.SSA.Dominance — dominance relation for SSA verification.

  Formalizes dominance following the classical definition (Lengauer-Tarjan):
  block d dominates block l if every path from the function entry to l
  passes through d. This is the foundation for SSA well-formedness
  (every use must be dominated by its definition) and is used by
  LICM, guard hoisting, and SSA verification.

  Key results:
  - Dominance is reflexive and transitive (a preorder)
  - Strict dominance is irreflexive and transitive (a strict partial order)
  - Entry block dominates all reachable blocks
  - Immediate dominator uniqueness (dominance tree is a tree)

  References:
  - Zhao et al., "Formalizing the LLVM Intermediate Representation for
    Verified Program Transformations" (POPL 2012)
  - Demange et al., "Plan-B: A Buffered Memory Model for Java" (POPL 2013)
  - Lengauer & Tarjan, "A Fast Algorithm for Finding Dominators in a
    Flowgraph" (TOPLAS 1979)
-/
import MoltTIR.CFG

namespace MoltTIR

open Classical

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Path definition (refinement of CFG.Dominates.pathFromTo)
-- ══════════════════════════════════════════════════════════════════

/-- A valid CFG path is a nonempty list of labels where consecutive pairs
    are connected by successor edges. This is a standalone inductive
    definition (separate from the one nested inside `Dominates`) for
    easier reuse. -/
inductive CFGPath (f : Func) : Label → Label → List Label → Prop where
  | single (l : Label) : CFGPath f l l [l]
  | cons (l₁ l₂ dst : Label) (rest : List Label)
      (hedge : IsSuccessor f l₁ l₂)
      (htail : CFGPath f l₂ dst (l₂ :: rest)) :
      CFGPath f l₁ dst (l₁ :: l₂ :: rest)

/-- A CFGPath always starts with its source label. -/
theorem CFGPath.head_eq {f : Func} {src dst : Label} {path : List Label}
    (h : CFGPath f src dst path) : path.head? = some src := by
  cases h with
  | single l => simp
  | cons l₁ l₂ _ _ _ _ => simp

/-- A CFGPath is always nonempty. -/
theorem CFGPath.nonempty {f : Func} {src dst : Label} {path : List Label}
    (h : CFGPath f src dst path) : path ≠ [] := by
  cases h with
  | single _ => simp
  | cons _ _ _ _ _ _ => simp

/-- A CFGPath has length ≥ 1. -/
theorem CFGPath.length_pos {f : Func} {src dst : Label} {path : List Label}
    (h : CFGPath f src dst path) : 0 < path.length := by
  cases h with
  | single _ => simp
  | cons _ _ _ _ _ _ => simp

/-- The source label is always in the path. -/
theorem CFGPath.src_mem {f : Func} {src dst : Label} {path : List Label}
    (h : CFGPath f src dst path) : src ∈ path := by
  cases h with
  | single l => exact List.Mem.head _
  | cons l₁ _ _ _ _ _ => exact List.Mem.head _

/-- The destination label is always in the path. -/
theorem CFGPath.dst_mem {f : Func} {src dst : Label} {path : List Label}
    (h : CFGPath f src dst path) : dst ∈ path := by
  induction h with
  | single l => exact List.Mem.head _
  | cons l₁ l₂ dst' rest _ htail ih => exact List.Mem.tail _ ih

/-- If d is in a CFGPath from src to dst, then there exists a sub-path
    from src to d that is a prefix of the original path.
    (Path splitting lemma — needed for dominance transitivity.) -/
theorem CFGPath.prefix_to_member {f : Func} {src dst d : Label}
    {path : List Label}
    (hpath : CFGPath f src dst path) (hd : d ∈ path) :
    ∃ prefix_path, CFGPath f src d prefix_path ∧
      ∀ x ∈ prefix_path, x ∈ path := by
  induction hpath with
  | single =>
    simp at hd; subst hd
    exact ⟨[d], .single d, fun _ hx => hx⟩
  | cons l₁ l₂ dst' rest hedge htail ih =>
    rcases List.mem_cons.mp hd with rfl | hd'
    · exact ⟨[d], .single d, fun x hx => by
        simp at hx; subst hx; exact List.Mem.head _⟩
    · obtain ⟨prefix_path, hprefix, hsubset⟩ := ih hd'
      cases hprefix with
      | single =>
        refine ⟨[l₁, d], .cons _ _ d [] hedge (.single d), fun x hx => ?_⟩
        rcases List.mem_cons.mp hx with rfl | hx'
        · exact List.Mem.head _
        · exact List.Mem.tail _ (hsubset x hx')
      | cons l₃ l₄ _ rest' hedge' htail' =>
        refine ⟨l₁ :: _ :: _ :: _, .cons _ _ d _ hedge (.cons _ _ d _ hedge' htail'), fun x hx => ?_⟩
        rcases List.mem_cons.mp hx with rfl | hx'
        · exact List.Mem.head _
        · exact List.Mem.tail _ (hsubset x hx')

-- ══════════════════════════════════════════════════════════════════
-- Section 1b: Path infrastructure (reachable→path, prefix_shorter)
-- ══════════════════════════════════════════════════════════════════

/-- Helper: construct a CFGPath from Reachable. -/
theorem reachable_to_cfgPath {f : Func} {src dst : Label}
    (h : Reachable f src dst) : ∃ path, CFGPath f src dst path := by
  induction h with
  | refl _ => exact ⟨[_], .single _⟩
  | step l1 _ _ hs _ ih =>
    obtain ⟨path, hpath⟩ := ih
    cases hpath with
    | single _ =>
      exact ⟨[l1, _], .cons _ _ _ _ hs (.single _)⟩
    | cons _ _ _ _ hedge' htail =>
      exact ⟨l1 :: _ :: _ :: _, .cons _ _ _ _ hs (.cons _ _ _ _ hedge' htail)⟩

/-- If d ∈ path from src to dst and d ≠ dst, there is a strictly shorter
    prefix path from src to d. -/
theorem CFGPath.prefix_shorter {f : Func} {src dst d : Label}
    {path : List Label}
    (hpath : CFGPath f src dst path) : d ∈ path → d ≠ dst →
    ∃ pp : List Label, CFGPath f src d pp ∧ pp.length < path.length := by
  induction hpath with
  | single l =>
    intro hd hne
    simp at hd; subst hd; exact absurd rfl hne
  | cons l₁ l₂ dst' rest hedge htail ih =>
    intro hd hne
    rcases List.mem_cons.mp hd with rfl | hd'
    · -- d = l₁: prefix [d] has length 1 < 2 + |rest|
      exact ⟨[d], .single d, by simp⟩
    · -- d ∈ l₂ :: rest: use IH to get shorter prefix from l₂ to d
      obtain ⟨pp, hpp, hlen⟩ := ih hd' hne
      cases hpp with
      | single =>
        exact ⟨[l₁, d], .cons _ _ d [] hedge (.single d), by simp at hlen ⊢; omega⟩
      | cons a _ _ _ hedge' htail' =>
        exact ⟨l₁ :: _ :: _ :: _, .cons _ _ d _ hedge (.cons _ _ d _ hedge' htail'), by simp at hlen ⊢; omega⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Dominance (path-based, classical)
-- ══════════════════════════════════════════════════════════════════

/-- Block d dominates block l in function f if l is reachable from
    the entry and every path from entry to l passes through d.
    This is equivalent to the definition in CFG.lean but uses the
    standalone CFGPath for easier reasoning. -/
def Dom (f : Func) (d l : Label) : Prop :=
  Reachable f f.entry l →
  ∀ (path : List Label), CFGPath f f.entry l path → d ∈ path

/-- Strict dominance: d strictly dominates l iff d dominates l and d ≠ l. -/
def SDom (f : Func) (d l : Label) : Prop :=
  Dom f d l ∧ d ≠ l

/-- Immediate dominator: d is the immediate dominator of l if d strictly
    dominates l and every other strict dominator of l also strictly
    dominates d. (d is the closest strict dominator.) -/
def ImmDom (f : Func) (d l : Label) : Prop :=
  SDom f d l ∧
  ∀ d', SDom f d' l → d' ≠ d → SDom f d' d

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Dominance is reflexive
-- ══════════════════════════════════════════════════════════════════

/-- Dominance is reflexive: every block dominates itself.
    Proof: l is always the last element of any path from entry to l. -/
theorem Dom.refl (f : Func) (l : Label) : Dom f l l := by
  intro _hreach path hpath
  exact CFGPath.dst_mem hpath

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Dominance is transitive
-- ══════════════════════════════════════════════════════════════════

/-- A CFGPath implies reachability. -/
theorem cfgPath_implies_reachable {f : Func} {src dst : Label} {path : List Label}
    (hpath : CFGPath f src dst path) : Reachable f src dst := by
  induction hpath with
  | single l => exact .refl l
  | cons l₁ l₂ dst' _ hedge _ ih =>
    exact .step l₁ l₂ dst' hedge ih

/-- Dominance is transitive: if d₂ dominates d₁ and d₁ dominates l,
    then d₂ dominates l.

    Proof sketch: Take any path P from entry to l. Since d₁ dom l,
    d₁ ∈ P. The prefix of P from entry to d₁ is a valid path, so
    since d₂ dom d₁, d₂ ∈ prefix ⊆ P. -/
theorem Dom.trans {f : Func} {d₁ d₂ l : Label}
    (h₁ : Dom f d₁ l) (h₂ : Dom f d₂ d₁) : Dom f d₂ l := by
  intro hreach path hpath
  have hd₁_mem := h₁ hreach path hpath
  obtain ⟨prefix_path, hprefix, hsubset⟩ :=
    CFGPath.prefix_to_member hpath hd₁_mem
  have hd₁_reach : Reachable f f.entry d₁ :=
    cfgPath_implies_reachable hprefix
  have hd₂_in_prefix := h₂ hd₁_reach prefix_path hprefix
  exact hsubset d₂ hd₂_in_prefix

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Strict dominance is a strict partial order
-- ══════════════════════════════════════════════════════════════════

/-- Strict dominance is irreflexive by definition. -/
theorem SDom.irrefl (f : Func) (l : Label) : ¬SDom f l l := by
  intro ⟨_, hne⟩
  exact hne rfl

/-- Mutual strict domination is impossible when one node is reachable.

    Proof by well-founded descent on path length: if SDom f a b and
    SDom f b a with a reachable, any path P from entry to a contains b
    (by Dom f b a). Since b ≠ a, the prefix to b is strictly shorter.
    Then a appears in that prefix (by Dom f a b, since b is reachable
    via the prefix). Since a ≠ b, the sub-prefix to a is even shorter.
    This gives a path from entry to a strictly shorter than P, yielding
    infinite descent in ℕ — contradiction. -/
private theorem sdom_not_symmetric {f : Func} {a b : Label}
    (hreach : Reachable f f.entry a)
    (hab : SDom f a b) (hba : SDom f b a) : False := by
  obtain ⟨p₀, hp₀⟩ := reachable_to_cfgPath hreach
  suffices ∀ (n : Nat) (p : List Label),
      CFGPath f f.entry a p → p.length ≤ n → False by
    exact this p₀.length p₀ hp₀ (Nat.le_refl _)
  intro n
  induction n with
  | zero =>
    intro p hp hlen
    have := CFGPath.length_pos hp
    omega
  | succ k ih =>
    intro p hp hlen
    -- b ∈ p (since b dom a and a is reachable)
    have hb_in := hba.1 hreach p hp
    -- Prefix to b is strictly shorter (b ≠ a)
    obtain ⟨pb, hpb, hpb_len⟩ := CFGPath.prefix_shorter hp hb_in hba.2
    -- b is reachable (via the prefix)
    have hb_reach := cfgPath_implies_reachable hpb
    -- a ∈ pb (since a dom b and b is reachable)
    have ha_in := hab.1 hb_reach pb hpb
    -- Sub-prefix to a is strictly shorter than pb
    obtain ⟨pa, hpa, hpa_len⟩ := CFGPath.prefix_shorter hpb ha_in hab.2
    -- pa.length < pb.length < p.length ≤ k + 1
    exact ih pa hpa (by omega)

/-- Strict dominance is transitive (for reachable targets).

    Requires reachability of the target c, since mutual strict domination
    of unreachable nodes is vacuously consistent under the conditional
    Dom definition. In practice, SSA verification only reasons about
    reachable blocks, so this hypothesis is always available.

    Uses Dom.trans for the dominance part and sdom_not_symmetric (the
    well-founded descent argument) to rule out the a = c case. -/
theorem SDom.trans {f : Func} {a b c : Label}
    (hreach : Reachable f f.entry c)
    (h₁ : SDom f a b) (h₂ : SDom f b c) : SDom f a c := by
  refine ⟨Dom.trans h₂.1 h₁.1, ?_⟩
  intro heq
  subst heq
  -- After subst: h₁ : SDom f a b, h₂ : SDom f b a, a (= c) is reachable
  exact sdom_not_symmetric hreach h₁ h₂

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Entry dominates all reachable blocks
-- ══════════════════════════════════════════════════════════════════

/-- The entry block dominates every reachable block.
    Proof: every path from entry to l starts with entry, so entry ∈ path. -/
theorem entry_dom_all (f : Func) (l : Label)
    (_hreach : Reachable f f.entry l) :
    Dom f f.entry l := by
  intro _ path hpath
  exact CFGPath.src_mem hpath

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Dominance tree properties
-- ══════════════════════════════════════════════════════════════════

/-- The immediate dominator of a block (if it exists) is unique.
    This follows from the definition: if d₁ and d₂ are both immediate
    dominators of l, then each strictly dominates the other (from the
    "closest" clause), which contradicts acyclicity of strict domination.

    Requires reachability of l for SDom.trans (needed to derive the
    contradiction from the strict domination cycle d₁ sdom d₂ sdom d₁). -/
theorem immDom_unique {f : Func} {l d₁ d₂ : Label}
    (hreach : Reachable f f.entry l)
    (h₁ : ImmDom f d₁ l) (h₂ : ImmDom f d₂ l) : d₁ = d₂ := by
  by_cases hne : d₁ = d₂
  · exact hne
  · exfalso
    have hne' : d₂ ≠ d₁ := fun heq => hne heq.symm
    have h_d2_sdom_d1 : SDom f d₂ d₁ := h₁.2 d₂ h₂.1 hne'
    have h_d1_sdom_d2 : SDom f d₁ d₂ := h₂.2 d₁ h₁.1 hne
    -- d₁ is reachable (it dominates reachable l, so it's on paths to l)
    have hd1_reach : Reachable f f.entry d₁ := by
      obtain ⟨pathL, hpathL⟩ := reachable_to_cfgPath hreach
      have hd1_in := h₁.1.1 hreach pathL hpathL
      obtain ⟨pref, hpref, _⟩ := CFGPath.prefix_to_member hpathL hd1_in
      exact cfgPath_implies_reachable hpref
    exact absurd (SDom.trans hd1_reach h_d1_sdom_d2 h_d2_sdom_d1) (SDom.irrefl f d₁)

/-- The dominance relation forms a tree rooted at the entry block:
    every non-entry reachable block has an immediate dominator.

    This requires finiteness of the reachable set; we parameterize
    by a finite-reachable-set assumption.

    Proof sketch: The set of strict dominators of l is nonempty (entry
    strictly dominates every non-entry reachable block) and finite
    (each strict dominator is reachable, hence in S). Dominators of a
    reachable node form a total order under domination (the chain
    property, Prosser 1959). The maximum element of this finite chain
    (closest strict dominator to l) is the immediate dominator.

    The chain property (dominators are totally ordered) is the key
    lemma needed here. Its formalization requires path concatenation
    and index-based reasoning on paths, which is factored as a
    separate proof obligation.
    TODO(formal, owner:runtime, milestone:LF3, priority:P2, status:partial):
    close domTree_is_tree by proving the dominator chain property. -/
theorem domTree_is_tree (f : Func) (l : Label)
    (hreach : Reachable f f.entry l) (hne : l ≠ f.entry)
    (hfinite : ∃ (S : List Label), ∀ m, Reachable f f.entry m → m ∈ S) :
    ∃ d, ImmDom f d l := by
  sorry

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Compatibility with CFG.Dominates
-- ══════════════════════════════════════════════════════════════════

/-- CFGPath implies Dominates.pathFromTo. -/
private theorem cfgPath_to_pathFromTo {f : Func} {src dst : Label} {path : List Label}
    (h : CFGPath f src dst path) : Dominates.pathFromTo f src dst path := by
  induction h with
  | single l => exact ⟨rfl, rfl⟩
  | cons l₁ l₂ dst' rest hedge htail ih =>
    exact ⟨rfl, hedge, ih⟩

/-- Dominates.pathFromTo implies CFGPath (for nonempty paths). -/
private theorem pathFromTo_to_cfgPath {f : Func} : {src dst : Label} → {path : List Label} →
    Dominates.pathFromTo f src dst path → CFGPath f src dst path
  | _, _, [], h => absurd h (by simp [Dominates.pathFromTo])
  | _, _, [x], ⟨hsrc, hdst⟩ => by subst hsrc; subst hdst; exact .single _
  | _, _, x :: y :: rest, ⟨hsrc, hedge, hrest⟩ => by
      subst hsrc; exact .cons _ y _ rest hedge (pathFromTo_to_cfgPath hrest)

/-- Our Dom definition is compatible with the original Dominates from CFG.lean.
    (The definitions are morally equivalent; they differ only in the path
    representation used.) -/
theorem Dom_iff_Dominates (f : Func) (d l : Label) :
    Dom f d l ↔ Dominates f d l := by
  constructor
  · -- Dom → Dominates
    intro hdom hreach path hpath
    exact hdom hreach path (pathFromTo_to_cfgPath hpath)
  · -- Dominates → Dom
    intro hdom hreach path hpath
    exact hdom hreach path (cfgPath_to_pathFromTo hpath)

end MoltTIR
