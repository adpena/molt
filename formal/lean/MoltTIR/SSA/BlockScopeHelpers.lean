/-
  MoltTIR.SSA.BlockScopeHelpers — helper lemmas for proving block_scope
  preservation across compiler passes.
-/
import MoltTIR.WellFormed

namespace MoltTIR

-- Lean 4.28 compatibility: List.enum was replaced by List.zipIdx with swapped tuple order.
-- We define a local enum for backward compatibility.
private def List.enum' (l : List α) : List (Nat × α) :=
  l.zipIdx.map fun (a, i) => (i, a)

-- ══════════════════════════════════════════════════════════════════
-- exprVarsIn characterization
-- ══════════════════════════════════════════════════════════════════

/-- exprVarsIn characterized as ∀ over exprVars. -/
theorem exprVarsIn_iff (scope : List Var) (e : Expr) :
    exprVarsIn scope e = true ↔ ∀ v ∈ exprVars e, scope.contains v = true := by
  induction e with
  | val _ => simp [exprVarsIn, exprVars]
  | var x => simp [exprVarsIn, exprVars]
  | bin op a b iha ihb =>
    simp only [exprVarsIn, exprVars, Bool.and_eq_true, List.mem_append]
    constructor
    · intro ⟨ha, hb⟩ v hv
      rcases hv with hva | hvb
      · exact iha.mp ha v hva
      · exact ihb.mp hb v hvb
    · intro h
      exact ⟨iha.mpr (fun v hv => h v (Or.inl hv)),
             ihb.mpr (fun v hv => h v (Or.inr hv))⟩
  | un op a ih => simp only [exprVarsIn, exprVars]; exact ih

/-- If RHS vars decrease, exprVarsIn is preserved. -/
theorem exprVarsIn_of_subset {scope : List Var} {e e' : Expr}
    (hsub : ∀ v ∈ exprVars e', v ∈ exprVars e)
    (h : exprVarsIn scope e = true) : exprVarsIn scope e' = true :=
  (exprVarsIn_iff scope e').mpr fun v hv => (exprVarsIn_iff scope e).mp h v (hsub v hv)

-- ══════════════════════════════════════════════════════════════════
-- termVarsIn characterization
-- ══════════════════════════════════════════════════════════════════

/-- Extract per-var evidence from termVarsIn. -/
theorem termVarsIn_var (scope : List Var) (t : Terminator)
    (h : termVarsIn scope t = true) (v : Var) (hv : v ∈ termVars t) :
    scope.contains v = true := by
  cases t with
  | ret e => exact (exprVarsIn_iff scope e).mp h v hv
  | jmp _ args =>
    simp only [termVars] at hv
    simp only [termVarsIn, List.all_eq_true] at h
    obtain ⟨e, he, hve⟩ := List.mem_flatMap.mp hv
    exact (exprVarsIn_iff scope e).mp (h e he) v hve
  | br cond _ ta _ ea =>
    simp only [termVars] at hv
    simp only [termVarsIn, Bool.and_eq_true, List.all_eq_true] at h
    rcases List.mem_append.mp hv with hca | hea
    · rcases List.mem_append.mp hca with hc | hta
      · exact (exprVarsIn_iff scope cond).mp h.1.1 v hc
      · obtain ⟨e, he, hve⟩ := List.mem_flatMap.mp hta
        exact (exprVarsIn_iff scope e).mp (h.1.2 e he) v hve
    · obtain ⟨e, he, hve⟩ := List.mem_flatMap.mp hea
      exact (exprVarsIn_iff scope e).mp (h.2 e he) v hve

/-- Build termVarsIn from per-var evidence. -/
theorem termVarsIn_of_forall (scope : List Var) (t : Terminator)
    (h : ∀ v ∈ termVars t, scope.contains v = true) :
    termVarsIn scope t = true := by
  cases t with
  | ret e => exact (exprVarsIn_iff scope e).mpr h
  | jmp _ args =>
    simp only [termVarsIn, List.all_eq_true]
    intro e he; exact (exprVarsIn_iff scope e).mpr fun v hv =>
      h v (by simp only [termVars]; exact List.mem_flatMap.mpr ⟨e, he, hv⟩)
  | br cond _ ta _ ea =>
    simp only [termVarsIn, Bool.and_eq_true, List.all_eq_true]
    refine ⟨⟨?_, ?_⟩, ?_⟩
    · exact (exprVarsIn_iff scope cond).mpr fun v hv =>
        h v (by simp only [termVars]
                exact List.mem_append.mpr (Or.inl (List.mem_append.mpr (Or.inl hv))))
    · intro e he; exact (exprVarsIn_iff scope e).mpr fun v hv =>
        h v (by simp only [termVars]
                exact List.mem_append.mpr (Or.inl (List.mem_append.mpr
                  (Or.inr (List.mem_flatMap.mpr ⟨e, he, hv⟩)))))
    · intro e he; exact (exprVarsIn_iff scope e).mpr fun v hv =>
        h v (by simp only [termVars]
                exact List.mem_append.mpr (Or.inr (List.mem_flatMap.mpr ⟨e, he, hv⟩)))

-- ══════════════════════════════════════════════════════════════════
-- blockWellFormed transfer: term-only changes
-- ══════════════════════════════════════════════════════════════════

/-- blockWellFormed for a block that only changes the terminator.
    Instructions and params are identical, terminator vars are a subset. -/
theorem blockWellFormed_of_term_only (b : Block) (term' : Terminator)
    (hterm_sub : ∀ v ∈ termVars term', v ∈ termVars b.term)
    (hwf : blockWellFormed b = true) :
    blockWellFormed { b with term := term' } = true := by
  unfold blockWellFormed at hwf ⊢
  simp only at hwf ⊢
  rw [Bool.and_eq_true] at hwf ⊢
  exact ⟨hwf.1, termVarsIn_of_forall _ term' fun v hv =>
    termVarsIn_var _ b.term hwf.2 v (hterm_sub v hv)⟩

-- ══════════════════════════════════════════════════════════════════
-- blockWellFormed transfer: RHS-only changes via map
-- ══════════════════════════════════════════════════════════════════

/-- blockWellFormed instrOk part for mapped instructions. -/
private theorem instrOk_of_map (params : List Var) (instrs : List Instr)
    (f : Instr → Instr)
    (hdst : ∀ i, (f i).dst = i.dst)
    (hrhs : ∀ i, ∀ v ∈ exprVars (f i).rhs, v ∈ exprVars i.rhs)
    (h : instrs.enum.all (fun p =>
      exprVarsIn (params ++ (instrs.take p.1).map Instr.dst) p.2.rhs) = true) :
    (instrs.map f).enum.all (fun p =>
      exprVarsIn (params ++ ((instrs.map f).take p.1).map Instr.dst) p.2.rhs) = true := by
  -- The key: ((instrs.map f).take j).map dst = (instrs.take j).map dst
  -- And: for each (j, f instrs[j]) in enum of mapped list,
  -- exprVarsIn (scope_j) (f instrs[j]).rhs follows from
  -- exprVarsIn (scope_j) instrs[j].rhs and hrhs
  rw [List.all_eq_true] at h ⊢
  intro ⟨idx, instr'⟩ hmem
  -- instr' is f applied to the original instruction at idx
  -- Use List.mem_zipIdx to extract info
  have hinfo := List.mem_zipIdx hmem
  obtain ⟨hidx_lt, hinstr'_eq⟩ := hinfo
  rw [List.length_map] at hidx_lt
  -- instr' = (instrs.map f)[idx] = f instrs[idx]
  have h_eq : instr' = f (instrs[idx]) := by
    rw [hinstr'_eq, List.getElem_map]
  rw [h_eq]
  -- scope at idx in mapped list = scope at idx in original list
  have hscope : ((instrs.map f).take idx).map Instr.dst =
      (instrs.take idx).map Instr.dst := by
    simp only [List.map_take, List.map_map]
    congr 1; apply List.map_congr_left; intro a _; exact hdst a
  rw [hscope]
  -- Original instruction at idx passes the check
  -- Need to construct membership in instrs.enum
  have hmem_orig : (idx, instrs[idx]) ∈ instrs.enum := by
    rw [List.mem_zipIdx_iff_getElem?]
    exact instrs.getElem?_eq_getElem hidx_lt
  have horig := h ⟨idx, instrs[idx]⟩ hmem_orig
  exact exprVarsIn_of_subset (hrhs instrs[idx]) horig

/-- blockWellFormed for a block with mapped instructions and changed terminator. -/
theorem blockWellFormed_of_map_instrs (b : Block)
    (f : Instr → Instr)
    (hdst : ∀ i, (f i).dst = i.dst)
    (hrhs : ∀ i, ∀ v ∈ exprVars (f i).rhs, v ∈ exprVars i.rhs)
    (term' : Terminator)
    (hterm_sub : ∀ v ∈ termVars term', v ∈ termVars b.term)
    (hwf : blockWellFormed b = true) :
    blockWellFormed { b with instrs := b.instrs.map f, term := term' } = true := by
  unfold blockWellFormed at hwf ⊢
  simp only at hwf ⊢
  rw [Bool.and_eq_true] at hwf ⊢
  constructor
  · exact instrOk_of_map b.params b.instrs f hdst hrhs hwf.1
  · have hscope_eq : definedVars b.params (b.instrs.map f) = definedVars b.params b.instrs := by
      simp only [definedVars, List.map_map]
      congr 1; apply List.map_congr_left; intro a _; exact hdst a
    rw [hscope_eq]
    exact termVarsIn_of_forall _ term' fun v hv =>
      termVarsIn_var _ b.term hwf.2 v (hterm_sub v hv)

end MoltTIR
