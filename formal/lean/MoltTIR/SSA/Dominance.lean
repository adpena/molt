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

-- ── Path concatenation and suffix extraction ─────────────────────

private theorem CFGPath.append {f : Func} {a b c : Label}
    {p₁ p₂ : List Label}
    (h₁ : CFGPath f a b p₁) (h₂ : CFGPath f b c p₂) :
    ∃ p, CFGPath f a c p ∧ (∀ x, x ∈ p → x ∈ p₁ ∨ x ∈ p₂) := by
  induction h₁ with
  | single _ => exact ⟨p₂, h₂, fun x hx => .inr hx⟩
  | cons l₁ l₂ _ rest hedge htail ih =>
    obtain ⟨p', hp', hsub⟩ := ih h₂
    cases hp' with
    | single =>
      refine ⟨[l₁, _], .cons _ _ _ _ hedge (.single _), fun x hx => ?_⟩
      rcases List.mem_cons.mp hx with rfl | hx'
      · exact .inl (List.Mem.head _)
      · rcases List.mem_cons.mp hx' with rfl | hx''
        · exact .inr (CFGPath.dst_mem h₂)
        · simp at hx''
    | cons _ _ _ _ hedge' htail' =>
      refine ⟨l₁ :: _ :: _ :: _, .cons _ _ _ _ hedge (.cons _ _ _ _ hedge' htail'),
        fun x hx => ?_⟩
      rcases List.mem_cons.mp hx with rfl | hx'
      · exact .inl (List.Mem.head _)
      · rcases hsub x hx' with h | h
        · exact .inl (List.Mem.tail _ h)
        · exact .inr h

private theorem CFGPath.suffix_from {f : Func} {src dst d : Label}
    {path : List Label}
    (hpath : CFGPath f src dst path) (hd : d ∈ path) :
    ∃ sp, CFGPath f d dst sp ∧ (∀ x ∈ sp, x ∈ path) := by
  induction hpath with
  | single l => simp at hd; subst hd; exact ⟨[d], .single d, fun _ hx => hx⟩
  | cons l₁ l₂ dst' rest hedge htail ih =>
    rcases List.mem_cons.mp hd with rfl | hd'
    · exact ⟨d :: l₂ :: rest, .cons d l₂ dst' rest hedge htail, fun _ hx => hx⟩
    · obtain ⟨sp, hsp, hsub⟩ := ih hd'
      exact ⟨sp, hsp, fun x hx => List.Mem.tail _ (hsub x hx)⟩

private theorem CFGPath.suffix_shorter {f : Func} {src dst d : Label}
    {path : List Label}
    (hpath : CFGPath f src dst path) (hd : d ∈ path) (hne : d ≠ src) :
    ∃ sp, CFGPath f d dst sp ∧ sp.length < path.length := by
  induction hpath with
  | single l => simp at hd; subst hd; exact absurd rfl hne
  | cons l₁ l₂ dst' rest hedge htail ih =>
    rcases List.mem_cons.mp hd with rfl | hd'
    · exact absurd rfl hne
    · by_cases hd_eq : d = l₂
      · subst hd_eq; exact ⟨d :: rest, htail, by simp⟩
      · obtain ⟨sp, hsp, hlen⟩ := ih hd' hd_eq
        have : (l₂ :: rest).length < (l₁ :: l₂ :: rest).length := by simp
        exact ⟨sp, hsp, Nat.lt_trans hlen this⟩

private theorem dom_reachable {f : Func} {d l : Label}
    (hreach : Reachable f f.entry l) (hdom : Dom f d l) :
    Reachable f f.entry d := by
  obtain ⟨p, hp⟩ := reachable_to_cfgPath hreach
  have hd_in := hdom hreach p hp
  obtain ⟨pref, hpref, _⟩ := CFGPath.prefix_to_member hp hd_in
  exact cfgPath_implies_reachable hpref

-- ── Dominator chain property ─────────────────────────────────────

private theorem dom_chain {f : Func} {a b l : Label}
    (hreach_l : Reachable f f.entry l)
    (ha : Dom f a l) (hb : Dom f b l) :
    Dom f a b ∨ Dom f b a := by
  by_cases hab_dom : Dom f a b
  · exact .inl hab_dom
  · by_cases hba_dom : Dom f b a
    · exact .inr hba_dom
    · exfalso
      have hab : a ≠ b := by intro heq; subst heq; exact hab_dom (Dom.refl f a)
      have hba : b ≠ a := fun h => hab h.symm
      have ⟨pb, hpb, ha_notin_pb⟩ : ∃ pb, CFGPath f f.entry b pb ∧ a ∉ pb :=
        byContradiction fun h_all =>
          hab_dom fun _ path hp =>
            byContradiction fun ha_notin => h_all ⟨path, hp, ha_notin⟩
      have ⟨pa, hpa, hb_notin_pa⟩ : ∃ pa, CFGPath f f.entry a pa ∧ b ∉ pa :=
        byContradiction fun h_all =>
          hba_dom fun _ path hp =>
            byContradiction fun hb_notin => h_all ⟨path, hp, hb_notin⟩
      obtain ⟨pl, hpl⟩ := reachable_to_cfgPath hreach_l
      have hb_in_pl := hb hreach_l pl hpl
      obtain ⟨sp₀, hsp₀, _⟩ := CFGPath.suffix_from hpl hb_in_pl
      suffices ∀ (n : Nat) (sp : List Label),
          CFGPath f b l sp → sp.length ≤ n → a ∉ sp by
        have ha_notin := this sp₀.length sp₀ hsp₀ (Nat.le_refl _)
        obtain ⟨comb, hcomb, hcomb_sub⟩ := CFGPath.append hpb hsp₀
        have ha_in := ha hreach_l comb hcomb
        rcases hcomb_sub a ha_in with h1 | h2
        · exact ha_notin_pb h1
        · exact ha_notin h2
      intro n
      induction n with
      | zero => intro sp hsp hlen _; have := CFGPath.length_pos hsp; omega
      | succ k ih =>
        intro sp hsp hlen ha_in
        obtain ⟨sal, hsal, hsal_len⟩ := CFGPath.suffix_shorter hsp ha_in hab
        obtain ⟨c1, hc1, hc1_sub⟩ := CFGPath.append hpa hsal
        have hb_in := hb hreach_l c1 hc1
        rcases hc1_sub b hb_in with hb_pa | hb_sal
        · exact hb_notin_pa hb_pa
        · obtain ⟨sbl, hsbl, hsbl_len⟩ := CFGPath.suffix_shorter hsal hb_sal hba
          have ha_notin_sbl := ih sbl hsbl (by omega)
          obtain ⟨c2, hc2, hc2_sub⟩ := CFGPath.append hpb hsbl
          have ha_in2 := ha hreach_l c2 hc2
          rcases hc2_sub a ha_in2 with h1 | h2
          · exact ha_notin_pb h1
          · exact ha_notin_sbl h2

-- ── Finding the immediate dominator via finite scan ──────────────

private theorem find_idom (f : Func) (l : Label)
    (hreach : Reachable f f.entry l)
    (hS : List Label)
    (hS_complete : ∀ m, Reachable f f.entry m → m ∈ hS) :
    ∀ (d : Label), SDom f d l →
      (∀ d', SDom f d' l → d' ∈ hS → d' = d ∨ SDom f d' d) →
      ∃ d', ImmDom f d' l := by
  intro d hd hd_best
  refine ⟨d, hd, fun d' hd' hne => ?_⟩
  have hd'_reach := dom_reachable hreach hd'.1
  have hd'_in := hS_complete d' hd'_reach
  rcases hd_best d' hd' hd'_in with heq | hsdom
  · exact absurd heq hne
  · exact hsdom

private theorem scan_candidates (f : Func) (l : Label)
    (hreach : Reachable f f.entry l)
    (hS_full : List Label)
    (hS_complete : ∀ m, Reachable f f.entry m → m ∈ hS_full) :
    ∀ (candidates : List Label) (d : Label),
      SDom f d l →
      (∀ d', SDom f d' l → d' ∈ hS_full → d' ∉ candidates → d' = d ∨ SDom f d' d) →
      ∃ d', ImmDom f d' l := by
  intro candidates
  induction candidates with
  | nil =>
    intro d hd hbest
    exact find_idom f l hreach hS_full hS_complete d hd
      (fun d' hd' hd'_in => hbest d' hd' hd'_in (List.not_mem_nil _))
  | cons c rest ih =>
    intro d hd hbest
    by_cases hc_sdom : SDom f c l
    · rcases dom_chain hreach hd.1 hc_sdom.1 with hd_dom_c | hc_dom_d
      · by_cases hdc : d = c
        · subst hdc
          exact ih d hd (fun d' hd' hd'_in hd'_notin_rest => by
            by_cases hd'd : d' = d
            · exact .inl hd'd
            · exact hbest d' hd' hd'_in (fun hmem =>
                (List.mem_cons.mp hmem).elim (fun h => hd'd h) (fun h => hd'_notin_rest h)))
        · have hd_sdom_c : SDom f d c := ⟨hd_dom_c, hdc⟩
          have hc_reach := dom_reachable hreach hc_sdom.1
          exact ih c hc_sdom (fun d' hd' hd'_in hd'_notin_rest => by
            by_cases hd'c : d' = c
            · exact .inl hd'c
            · have hd'_notin_cands : d' ∉ c :: rest :=
                fun hmem => (List.mem_cons.mp hmem).elim (fun h => hd'c h) (fun h => hd'_notin_rest h)
              rcases hbest d' hd' hd'_in hd'_notin_cands with heq | hsdom
              · exact .inr (heq ▸ hd_sdom_c)
              · exact .inr (SDom.trans hc_reach hsdom hd_sdom_c))
      · by_cases hcd : c = d
        · exact ih d hd (fun d' hd' hd'_in hd'_notin_rest => by
            by_cases hd'c : d' = c
            · subst hd'c; exact .inl hcd
            · exact hbest d' hd' hd'_in (fun hmem =>
                (List.mem_cons.mp hmem).elim (fun h => hd'c h) (fun h => hd'_notin_rest h)))
        · have hc_sdom_d : SDom f c d := ⟨hc_dom_d, hcd⟩
          exact ih d hd (fun d' hd' hd'_in hd'_notin_rest => by
            by_cases hd'c : d' = c
            · exact .inr (hd'c ▸ hc_sdom_d)
            · exact hbest d' hd' hd'_in (fun hmem =>
                (List.mem_cons.mp hmem).elim (fun h => hd'c h) (fun h => hd'_notin_rest h)))
    · exact ih d hd (fun d' hd' hd'_in hd'_notin_rest => by
        by_cases hd'c : d' = c
        · exact absurd (hd'c ▸ hd') hc_sdom
        · exact hbest d' hd' hd'_in (fun hmem =>
            (List.mem_cons.mp hmem).elim (fun h => hd'c h) (fun h => hd'_notin_rest h)))

/-- The dominance relation forms a tree rooted at the entry block:
    every non-entry reachable block has an immediate dominator.

    Proof: the entry strictly dominates l (nonempty set of strict
    dominators). By the chain property, dominators of l are totally
    ordered. We scan the finite list S of reachable nodes, maintaining
    the "deepest" strict dominator seen so far. After processing all
    of S, the current candidate is dominated by all strict dominators
    of l, making it the immediate dominator. -/
theorem domTree_is_tree (f : Func) (l : Label)
    (hreach : Reachable f f.entry l) (hne : l ≠ f.entry)
    (hfinite : ∃ (S : List Label), ∀ m, Reachable f f.entry m → m ∈ S) :
    ∃ d, ImmDom f d l := by
  obtain ⟨S, hS⟩ := hfinite
  have h_entry_sdom : SDom f f.entry l :=
    ⟨entry_dom_all f l hreach, fun h => hne h.symm⟩
  exact scan_candidates f l hreach S hS S f.entry h_entry_sdom
    (fun d' hd' _ hnotin => (hnotin (hS d' (dom_reachable hreach hd'.1))).elim)

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
