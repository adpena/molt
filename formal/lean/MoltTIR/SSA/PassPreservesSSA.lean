/-
  MoltTIR.SSA.PassPreservesSSA — proofs that midend passes preserve SSA form.

  Each pass in Molt's midend pipeline must preserve the SSA invariant:
  unique definitions and use-dominates-def. We prove this for each pass
  by analyzing what structural changes the pass makes to the IR.

  Pass classification by structural effect:
  1. **RHS-only** (ConstFold, SCCP, CSE): only modify instruction RHS
     expressions. Definitions unchanged, dominance structure unchanged.
  2. **Instruction removal** (DCE): remove instructions. Definitions
     decrease, uses decrease. Unique-def preserved (subset). Use-dom-def
     preserved (only dead uses removed).
  3. **Terminator rewriting** (EdgeThread, JoinCanon): only modify
     block terminators. Instruction defs unchanged.
  4. **Instruction movement** (LICM): move instructions between blocks.
     Must preserve dominance of moved definitions over their uses.
  5. **Instruction replacement** (GuardHoist): replace guard RHS with
     identity. Definitions unchanged, uses may decrease.
-/
import MoltTIR.SSA.WellFormedSSA
import MoltTIR.Semantics.BlockCorrect
import MoltTIR.Passes.ConstFold
import MoltTIR.Passes.DCE
import MoltTIR.Passes.SCCP
import MoltTIR.Passes.SCCPMulti
import MoltTIR.Passes.CSE
import MoltTIR.Passes.LICM
import MoltTIR.Passes.GuardHoist
import MoltTIR.Passes.JoinCanon
import MoltTIR.Passes.EdgeThread
import MoltTIR.SSA.CSEHelpers

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Helper lemmas for map-based passes
-- ══════════════════════════════════════════════════════════════════

/-- A block transformation that preserves params and instruction dsts
    (only changes RHS expressions) preserves block-level definitions. -/
theorem blockDefs_preserved_of_rhs_only {b b' : Block}
    (hparams : b'.params = b.params)
    (hdsts : b'.instrs.map Instr.dst = b.instrs.map Instr.dst) :
    blockAllDefs b' = blockAllDefs b := by
  simp only [blockAllDefs, hparams, hdsts]

/-- Reverse of blocks_map_some: if the mapped function has blocks = some blk',
    then there exists blk in the original with f.blocks = some blk and blk' = g blk. -/
private theorem blocks_map_some_rev (f : Func) (g : Block → Block) (lbl : Label)
    (blk' : Block)
    (h : ({ f with blockList := f.blockList.map fun (l, b) => (l, g b) } : Func).blocks lbl = some blk') :
    ∃ blk, f.blocks lbl = some blk ∧ blk' = g blk := by
  simp only [Func.blocks] at *
  generalize f.blockList = xs at h ⊢
  induction xs with
  | nil => simp_all [List.find?]
  | cons p rest ih =>
    obtain ⟨l, b⟩ := p
    simp only [List.map, List.find?] at *
    cases hlbl : (l == lbl) <;> simp_all

/-- For a defs-preserving block map, DefinedIn is preserved in both directions. -/
private theorem definedIn_mapFunc_iff (f : Func) (g : Block → Block)
    (hdefs : ∀ b, blockAllDefs (g b) = blockAllDefs b) (v : Var) (lbl : Label) :
    DefinedIn { f with blockList := f.blockList.map fun (l, b) => (l, g b) } v lbl ↔
    DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hv⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f g lbl blk' hblk'
    exact ⟨blk, hblk, hdefs blk ▸ hv⟩
  · intro ⟨blk, hblk, hv⟩
    exact ⟨g blk, blocks_map_some f g lbl blk hblk, (hdefs blk).symm ▸ hv⟩

/-- For a defs-preserving block map, unique_defs is preserved. -/
private theorem unique_defs_of_mapFunc (f : Func) (g : Block → Block)
    (hdefs : ∀ b, blockAllDefs (g b) = blockAllDefs b)
    (h : ∀ v lbl₁ lbl₂, DefinedIn f v lbl₁ → DefinedIn f v lbl₂ → lbl₁ = lbl₂) :
    ∀ v lbl₁ lbl₂,
      DefinedIn { f with blockList := f.blockList.map fun (l, b) => (l, g b) } v lbl₁ →
      DefinedIn { f with blockList := f.blockList.map fun (l, b) => (l, g b) } v lbl₂ →
      lbl₁ = lbl₂ := by
  intro v lbl₁ lbl₂ h₁ h₂
  exact h v lbl₁ lbl₂
    ((definedIn_mapFunc_iff f g hdefs v lbl₁).mp h₁)
    ((definedIn_mapFunc_iff f g hdefs v lbl₂).mp h₂)

-- ══════════════════════════════════════════════════════════════════
-- Section 1b: CFG preservation for successor-preserving block maps
-- ══════════════════════════════════════════════════════════════════

/-- For a block map that preserves termSuccessors, IsSuccessor is
    preserved from original to mapped function. -/
private theorem isSuccessor_mapFunc_of_original (f : Func) (g : Block → Block)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    {l1 l2 : Label} (h : IsSuccessor f l1 l2) :
    IsSuccessor { f with blockList := f.blockList.map fun (l, b) => (l, g b) } l1 l2 := by
  obtain ⟨blk, hblk, hmem⟩ := h
  exact ⟨g blk, blocks_map_some f g l1 blk hblk, (hterm blk) ▸ hmem⟩

/-- For a block map that preserves termSuccessors, IsSuccessor is
    preserved from mapped function to original. -/
private theorem isSuccessor_mapFunc_of_mapped (f : Func) (g : Block → Block)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    {l1 l2 : Label} (h : IsSuccessor { f with blockList := f.blockList.map fun (l, b) => (l, g b) } l1 l2) :
    IsSuccessor f l1 l2 := by
  obtain ⟨blk', hblk', hmem⟩ := h
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f g l1 blk' hblk'
  exact ⟨blk, hblk, (hterm blk) ▸ hmem⟩

/-- For a successor-preserving block map, CFGPath is preserved from
    original to mapped function. -/
private theorem cfgPath_mapFunc_of_original (f : Func) (g : Block → Block)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    {src dst : Label} {path : List Label}
    (h : CFGPath f src dst path) :
    CFGPath { f with blockList := f.blockList.map fun (l, b) => (l, g b) } src dst path := by
  induction h with
  | single l => exact .single l
  | cons l₁ l₂ dst' rest hedge _ ih =>
    exact .cons l₁ l₂ dst' rest (isSuccessor_mapFunc_of_original f g hterm hedge) ih

/-- For a successor-preserving block map, CFGPath is preserved from
    mapped function to original. -/
private theorem cfgPath_mapFunc_of_mapped (f : Func) (g : Block → Block)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    {src dst : Label} {path : List Label}
    (h : CFGPath { f with blockList := f.blockList.map fun (l, b) => (l, g b) } src dst path) :
    CFGPath f src dst path := by
  induction h with
  | single l => exact .single l
  | cons l₁ l₂ dst' rest hedge _ ih =>
    exact .cons l₁ l₂ dst' rest (isSuccessor_mapFunc_of_mapped f g hterm hedge) ih

/-- For a successor-preserving block map, Reachable is preserved from
    original to mapped function. -/
private theorem reachable_mapFunc_of_original (f : Func) (g : Block → Block)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    {l1 l2 : Label} (h : Reachable f l1 l2) :
    Reachable { f with blockList := f.blockList.map fun (l, b) => (l, g b) } l1 l2 := by
  induction h with
  | refl l => exact .refl l
  | step l1' l2' l3' hedge _ ih =>
    exact .step l1' l2' l3' (isSuccessor_mapFunc_of_original f g hterm hedge) ih

/-- For a successor-preserving block map, Reachable is preserved from
    mapped function to original. -/
private theorem reachable_mapFunc_of_mapped (f : Func) (g : Block → Block)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    {l1 l2 : Label} (h : Reachable { f with blockList := f.blockList.map fun (l, b) => (l, g b) } l1 l2) :
    Reachable f l1 l2 := by
  induction h with
  | refl l => exact .refl l
  | step l1' l2' l3' hedge _ ih =>
    exact .step l1' l2' l3' (isSuccessor_mapFunc_of_mapped f g hterm hedge) ih

/-- For a successor-preserving block map, Dom is preserved in both directions.
    This is the key lemma for proving use_dom_def for RHS-only passes. -/
private theorem dom_mapFunc_iff (f : Func) (g : Block → Block)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    (d l : Label) :
    Dom { f with blockList := f.blockList.map fun (l, b) => (l, g b) } d l ↔
    Dom f d l := by
  simp only [Dom]
  constructor
  · intro hdom hreach path hpath
    have hreach' := reachable_mapFunc_of_original f g hterm hreach
    have hpath' := cfgPath_mapFunc_of_original f g hterm hpath
    exact hdom hreach' path hpath'
  · intro hdom hreach path hpath
    have hreach' := reachable_mapFunc_of_mapped f g hterm hreach
    have hpath' := cfgPath_mapFunc_of_mapped f g hterm hpath
    exact hdom hreach' path hpath'

/-- For a successor-preserving and defs-preserving block map,
    use_dom_def is preserved from the original SSA well-formed function. -/
private theorem use_dom_def_of_mapFunc (f : Func) (g : Block → Block)
    (hdefs : ∀ b, blockAllDefs (g b) = blockAllDefs b)
    (hterm : ∀ b, termSuccessors (g b).term = termSuccessors b.term)
    (huses : ∀ v lbl,
      UsedIn { f with blockList := f.blockList.map fun (l, b) => (l, g b) } v lbl →
      UsedIn f v lbl)
    (hssa : SSAWellFormed f) :
    ∀ v b_use b_def,
      UsedIn { f with blockList := f.blockList.map fun (l, b) => (l, g b) } v b_use →
      DefinedIn { f with blockList := f.blockList.map fun (l, b) => (l, g b) } v b_def →
      Dom { f with blockList := f.blockList.map fun (l, b) => (l, g b) } b_def b_use := by
  intro v b_use b_def huse hdef
  have hdef_orig := (definedIn_mapFunc_iff f g hdefs v b_def).mp hdef
  have huse_orig := huses v b_use huse
  have hdom_orig := hssa.use_dom_def v b_use b_def huse_orig hdef_orig
  exact (dom_mapFunc_iff f g hterm b_def b_use).mpr hdom_orig

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Constant folding preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- constFoldInstr preserves the instruction destination. -/
theorem constFoldInstr_dst (i : Instr) :
    (constFoldInstr i).dst = i.dst := by
  unfold constFoldInstr; rfl

/-- constFoldBlock preserves params and instruction dsts.
    Proof: constFoldBlock only modifies RHS of instructions and
    terminator expressions; params and instruction dsts are unchanged. -/
theorem constFoldBlock_defs (b : Block) :
    blockAllDefs (constFoldBlock b) = blockAllDefs b := by
  simp only [blockAllDefs, constFoldBlock, List.map_map, Function.comp_def, constFoldInstr_dst]

/-- Constant folding can only remove variable references from expressions,
    never introduce new ones. -/
theorem constFoldExpr_vars_subset : ∀ (e : Expr) (v : Var),
    v ∈ exprVars (constFoldExpr e) → v ∈ exprVars e := by
  intro e
  induction e with
  | val _ => simp [constFoldExpr, exprVars]
  | var x => simp [constFoldExpr, exprVars]
  | bin op a b iha ihb =>
    intro v hv
    have key : v ∈ exprVars (constFoldExpr a) ++ exprVars (constFoldExpr b) →
               v ∈ exprVars (.bin op a b) := by
      intro hmem
      simp only [exprVars]
      rcases List.mem_append.mp hmem with ha | hb
      · exact List.mem_append_left _ (iha v ha)
      · exact List.mem_append_right _ (ihb v hb)
    simp only [constFoldExpr] at hv
    split at hv
    · -- (.val va, .val vb): need to split on evalBinOp
      split at hv
      · exact nomatch hv
      · exact key hv
    · -- catch-all: result is .bin op a' b'
      exact key hv
  | un op a ih =>
    intro v hv
    simp only [constFoldExpr] at hv
    split at hv
    · -- (.val va): split on evalUnOp
      split at hv
      · exact nomatch hv
      · simp only [exprVars] at hv; exact ih v hv
    · -- catch-all: result is .un op a'
      simp only [exprVars] at hv; exact ih v hv

/-- Constant folding preserves terminator successors. -/
theorem constFoldTerminator_successors (t : Terminator) :
    termSuccessors (constFoldTerminator t) = termSuccessors t := by
  cases t with
  | ret _ => simp [constFoldTerminator, termSuccessors]
  | jmp target args => simp [constFoldTerminator, termSuccessors]
  | br cond tl ta el ea => simp [constFoldTerminator, termSuccessors]
  | yield val resume resumeArgs => simp [constFoldTerminator, termSuccessors]
  | switch scrutinee cases default_ => simp [constFoldTerminator, termSuccessors]
  | unreachable => simp [constFoldTerminator, termSuccessors]

/-- constFoldBlock preserves terminator successors. -/
theorem constFoldBlock_successors (b : Block) :
    termSuccessors (constFoldBlock b).term = termSuccessors b.term := by
  simp [constFoldBlock, constFoldTerminator_successors]

/-- Mapping constFoldExpr over a list preserves vars subset. -/
private theorem map_constFoldExpr_vars_subset (es : List Expr) :
    ∀ v, v ∈ (es.map constFoldExpr).flatMap exprVars → v ∈ es.flatMap exprVars := by
  intro v hv
  simp only [List.mem_flatMap, List.mem_map] at hv
  obtain ⟨e', ⟨e, he_mem, rfl⟩, hv_in⟩ := hv
  simp only [List.mem_flatMap]
  exact ⟨e, he_mem, constFoldExpr_vars_subset e v hv_in⟩

/-- Constant folding can only remove variable references from terminators. -/
theorem constFoldTerminator_vars_subset (t : Terminator) :
    ∀ v, v ∈ termVars (constFoldTerminator t) → v ∈ termVars t := by
  intro v hv
  cases t with
  | ret e =>
    simp only [constFoldTerminator, termVars] at *
    exact constFoldExpr_vars_subset e v hv
  | jmp target args =>
    simp only [constFoldTerminator, termVars] at *
    exact map_constFoldExpr_vars_subset args v hv
  | br cond tl ta el ea =>
    simp only [constFoldTerminator, termVars] at hv ⊢
    rcases List.mem_append.mp hv with hce | hea'
    · rcases List.mem_append.mp hce with hc | hta'
      · exact List.mem_append_left _ (List.mem_append_left _ (constFoldExpr_vars_subset cond v hc))
      · exact List.mem_append_left _ (List.mem_append_right _ (map_constFoldExpr_vars_subset ta v hta'))
    · exact List.mem_append_right _ (map_constFoldExpr_vars_subset ea v hea')
  | yield val resume resumeArgs =>
    simp only [constFoldTerminator, termVars] at hv ⊢
    rcases List.mem_append.mp hv with hval | hargs
    · exact List.mem_append_left _ (constFoldExpr_vars_subset val v hval)
    · exact List.mem_append_right _ (map_constFoldExpr_vars_subset resumeArgs v hargs)
  | switch scrutinee cases default_ =>
    simp only [constFoldTerminator, termVars] at *
    exact constFoldExpr_vars_subset scrutinee v hv
  | unreachable =>
    simp only [constFoldTerminator, termVars] at hv
    exact nomatch hv

/-- constFoldBlock uses are a subset of original block uses. -/
theorem constFoldBlock_uses_subset (b : Block) :
    ∀ v ∈ blockAllUses (constFoldBlock b), v ∈ blockAllUses b := by
  intro v hv
  simp only [blockAllUses, constFoldBlock] at hv ⊢
  rcases List.mem_append.mp hv with hi | ht
  · apply List.mem_append_left
    simp only [List.mem_flatMap, List.mem_map] at hi ⊢
    obtain ⟨i', ⟨i, hi_mem, rfl⟩, hv_rhs⟩ := hi
    simp only [constFoldInstr] at hv_rhs
    exact ⟨i, hi_mem, constFoldExpr_vars_subset i.rhs v hv_rhs⟩
  · apply List.mem_append_right
    exact constFoldTerminator_vars_subset b.term v ht

/-- UsedIn in const-folded function implies UsedIn in original. -/
private theorem usedIn_constFoldFunc_imp (f : Func) (v : Var) (lbl : Label)
    (h : UsedIn (constFoldFunc f) v lbl) : UsedIn f v lbl := by
  obtain ⟨blk', hblk', hv⟩ := h
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f constFoldBlock lbl blk' hblk'
  exact ⟨blk, hblk, constFoldBlock_uses_subset blk v hv⟩

private theorem find_map_preserves_label {bl : List (Label × Block)}
    {lbl : Label} {g : Block → Block}
    (hfind : (bl.find? (fun p => p.1 == lbl)).isSome) :
    ((bl.map fun (l, b) => (l, g b)).find? (fun p => p.1 == lbl)).isSome := by
  induction bl with
  | nil => simp at hfind
  | cons hd tl ih =>
    simp only [List.map, List.find?] at *
    split at hfind <;> simp_all

/-- find? on a mapped list succeeds if the predicate only depends on
    a preserved component. -/
private theorem find_map_isSome_of_fst_preserved
    {bl : List (Label × Block)}
    {g : Label × Block → Label × Block}
    (hfst : ∀ p, (g p).fst = p.fst)
    {lbl : Label}
    (hfind : (bl.find? (fun p => p.fst == lbl)).isSome) :
    ((bl.map g).find? (fun p => p.fst == lbl)).isSome := by
  induction bl with
  | nil => simp at hfind
  | cons hd tl ih =>
    simp only [List.map, List.find?]
    simp only [hfst hd]
    simp only [List.find?] at hfind
    split at hfind <;> simp_all

/-- Any label-preserving map over blockList preserves block lookup.
    Here g maps (Label × Block) → (Label × Block) but preserves fst. -/
private theorem mapFunc_blocks_isSome_gen {f : Func}
    {g : Label × Block → Label × Block}
    (hfst : ∀ p, (g p).fst = p.fst) {lbl : Label}
    (h : (f.blocks lbl).isSome) :
    (Func.blocks { f with blockList := f.blockList.map g } lbl).isSome := by
  unfold Func.blocks at *
  simp only at *
  have hfind : (f.blockList.find? (fun p => p.fst == lbl)).isSome := by
    cases hf : f.blockList.find? (fun p => p.fst == lbl) with
    | none => simp [hf] at h
    | some _ => simp
  have hfind' := find_map_isSome_of_fst_preserved hfst hfind
  cases hres : (f.blockList.map g).find? (fun p => p.fst == lbl) with
  | none => simp [hres] at hfind'
  | some _ => simp

/-- Mapping a block transform over blockList preserves block lookup. -/
private theorem mapFunc_blocks_isSome {f : Func} {g : Block → Block} {lbl : Label}
    (h : (f.blocks lbl).isSome) :
    (Func.blocks { f with blockList := f.blockList.map fun (l, b) => (l, g b) } lbl).isSome :=
  mapFunc_blocks_isSome_gen (fun ⟨l, _⟩ => rfl) h

theorem constFold_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (constFoldFunc f) := by
  constructor
  · -- unique_defs: definitions are identical after const fold
    show ∀ v lbl₁ lbl₂, DefinedIn (constFoldFunc f) v lbl₁ →
      DefinedIn (constFoldFunc f) v lbl₂ → lbl₁ = lbl₂
    unfold constFoldFunc
    exact unique_defs_of_mapFunc f constFoldBlock constFoldBlock_defs h.unique_defs
  · -- use_dom_def: constFold preserves terminators (same successors),
    -- and uses can only decrease (constFoldExpr replaces subexprs with vals).
    -- Both conditions verified; apply use_dom_def_of_mapFunc.
    show ∀ v b_use b_def, UsedIn (constFoldFunc f) v b_use →
      DefinedIn (constFoldFunc f) v b_def → Dom (constFoldFunc f) b_def b_use
    unfold constFoldFunc
    exact use_dom_def_of_mapFunc f constFoldBlock
      constFoldBlock_defs
      constFoldBlock_successors
      (fun v lbl hu => usedIn_constFoldFunc_imp f v lbl hu)
      h
  · -- entry_exists: blockList labels are preserved
    show ((constFoldFunc f).blocks (constFoldFunc f).entry).isSome
    unfold constFoldFunc
    simp only
    exact mapFunc_blocks_isSome h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 3: DCE preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- DCE only removes instructions; it never adds new definitions.
    Every definition in the DCE'd block was in the original. -/
theorem dceBlock_defs_subset (b : Block) :
    ∀ v ∈ blockAllDefs (dceBlock b), v ∈ blockAllDefs b := by
  intro v hv
  simp only [blockAllDefs, dceBlock, dceInstrs] at hv ⊢
  cases List.mem_append.mp hv with
  | inl hp =>
    exact List.mem_append_left _ hp
  | inr hi =>
    apply List.mem_append_right
    have := List.mem_map.mp hi
    obtain ⟨i, hmem, rfl⟩ := this
    have hmem_orig : i ∈ b.instrs := by
      simp only [List.mem_filter] at hmem
      exact hmem.1
    exact List.mem_map.mpr ⟨i, hmem_orig, rfl⟩

/-- Every definition in the DCE'd function was a definition in the original. -/
private theorem definedIn_dceFunc_imp (f : Func) (v : Var) (lbl : Label)
    (h : DefinedIn (dceFunc f) v lbl) : DefinedIn f v lbl := by
  obtain ⟨blk', hblk', hv⟩ := h
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f dceBlock lbl blk' hblk'
  exact ⟨blk, hblk, dceBlock_defs_subset blk v hv⟩

/-- DCE block uses are a subset of original block uses. -/
theorem dceBlock_uses_subset (b : Block) :
    ∀ v ∈ blockAllUses (dceBlock b), v ∈ blockAllUses b := by
  intro v hv
  simp only [blockAllUses, dceBlock, dceInstrs] at hv ⊢
  cases List.mem_append.mp hv with
  | inl hi =>
    apply List.mem_append_left
    -- v is in bind exprVars of filtered instructions
    simp only [List.mem_flatMap] at hi ⊢
    obtain ⟨i, hmem_filt, hv_rhs⟩ := hi
    have hmem_orig : i ∈ b.instrs := by
      simp only [List.mem_filter] at hmem_filt
      exact hmem_filt.1
    exact ⟨i, hmem_orig, hv_rhs⟩
  | inr ht =>
    -- terminator is unchanged
    exact List.mem_append_right _ ht

/-- UsedIn in DCE'd function implies UsedIn in original. -/
private theorem usedIn_dceFunc_imp (f : Func) (v : Var) (lbl : Label)
    (h : UsedIn (dceFunc f) v lbl) : UsedIn f v lbl := by
  obtain ⟨blk', hblk', hv⟩ := h
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f dceBlock lbl blk' hblk'
  exact ⟨blk, hblk, dceBlock_uses_subset blk v hv⟩

/-- DCE preserves SSA: removing dead definitions maintains unique-def
    (a subset of a unique list is unique) and use-dom-def (dead code
    has no uses, so the remaining use-def pairs are unchanged). -/
theorem dce_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (dceFunc f) := by
  constructor
  · -- unique_defs: subset of original defs, still unique
    intro v lbl₁ lbl₂ h₁ h₂
    exact h.unique_defs v lbl₁ lbl₂
      (definedIn_dceFunc_imp f v lbl₁ h₁)
      (definedIn_dceFunc_imp f v lbl₂ h₂)
  · -- use_dom_def: uses are subset, defs are subset, dominance preserved
    intro v b_use b_def huse hdef
    have huse_orig := usedIn_dceFunc_imp f v b_use huse
    have hdef_orig := definedIn_dceFunc_imp f v b_def hdef
    have hdom_orig := h.use_dom_def v b_use b_def huse_orig hdef_orig
    exact (dom_mapFunc_iff f dceBlock (fun b => rfl) b_def b_use).mpr hdom_orig
  · -- entry_exists
    show ((dceFunc f).blocks (dceFunc f).entry).isSome
    unfold dceFunc
    exact mapFunc_blocks_isSome h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 4: SCCP preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- SCCP (single-block) preserves instruction destinations. -/
theorem sccpInstrs_dsts (σ : AbsEnv) (instrs : List Instr) :
    (sccpInstrs σ instrs).2.map Instr.dst = instrs.map Instr.dst := by
  induction instrs generalizing σ with
  | nil => simp [sccpInstrs]
  | cons i rest ih =>
    simp only [sccpInstrs, List.map]
    congr 1
    exact ih (σ.set i.dst (absEvalExpr σ i.rhs))

/-- sccpBlock preserves block definitions. -/
theorem sccpBlock_defs (σ : AbsEnv) (b : Block) :
    blockAllDefs (sccpBlock σ b).2 = blockAllDefs b := by
  simp only [blockAllDefs, sccpBlock, sccpInstrs_dsts]

/-- SCCP instruction uses are a subset of original uses. -/
private theorem sccpInstr_rhs_vars_subset (σ : AbsEnv) (i : Instr) :
    ∀ v, v ∈ exprVars (match absEvalExpr σ i.rhs with | .known cv => Expr.val cv | _ => i.rhs) →
         v ∈ exprVars i.rhs := by
  intro v hv
  cases habsRhs : absEvalExpr σ i.rhs with
  | unknown => simp [habsRhs] at hv; exact hv
  | known cv => simp [habsRhs, exprVars] at hv
  | overdefined => simp [habsRhs] at hv; exact hv

theorem sccpInstrs_uses_subset (σ : AbsEnv) (instrs : List Instr) :
    ∀ v, v ∈ (sccpInstrs σ instrs).2.flatMap (fun i => exprVars i.rhs) →
         v ∈ instrs.flatMap (fun i => exprVars i.rhs) := by
  induction instrs generalizing σ with
  | nil => simp [sccpInstrs]
  | cons i rest ih =>
    intro v hv
    simp only [sccpInstrs] at hv
    simp only [List.flatMap_cons, List.mem_append] at hv ⊢
    rcases hv with hhead | htail
    · left
      exact sccpInstr_rhs_vars_subset σ i v hhead
    · right
      exact ih _ v htail

/-- SCCP block uses are a subset of original block uses. -/
theorem sccpBlock_uses_subset (σ : AbsEnv) (b : Block) :
    ∀ v ∈ blockAllUses (sccpBlock σ b).2, v ∈ blockAllUses b := by
  intro v hv
  simp only [blockAllUses, sccpBlock] at hv ⊢
  rcases List.mem_append.mp hv with hi | ht
  · exact List.mem_append_left _ (sccpInstrs_uses_subset σ b.instrs v hi)
  · exact List.mem_append_right _ ht

/-- UsedIn in SCCP'd function implies UsedIn in original. -/
private theorem usedIn_sccpFunc_imp (f : Func) (v : Var) (lbl : Label)
    (h : UsedIn (sccpFunc f) v lbl) : UsedIn f v lbl := by
  obtain ⟨blk', hblk', hv⟩ := h
  unfold sccpFunc at hblk'
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f
    (fun b => (sccpBlock AbsEnv.top b).2) lbl blk' hblk'
  exact ⟨blk, hblk, sccpBlock_uses_subset AbsEnv.top blk v hv⟩

/-- SCCP preserves SSA: it only replaces RHS with constants. -/
theorem sccp_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (sccpFunc f) := by
  constructor
  · show ∀ v lbl₁ lbl₂, DefinedIn (sccpFunc f) v lbl₁ →
      DefinedIn (sccpFunc f) v lbl₂ → lbl₁ = lbl₂
    unfold sccpFunc
    exact unique_defs_of_mapFunc f (fun b => (sccpBlock AbsEnv.top b).2) (sccpBlock_defs AbsEnv.top) h.unique_defs
  · -- use_dom_def: uses subset, defs preserved, dominance preserved
    intro v b_use b_def huse hdef
    have huse_orig := usedIn_sccpFunc_imp f v b_use huse
    have hdef_orig := (definedIn_mapFunc_iff f (fun b => (sccpBlock AbsEnv.top b).2)
      (sccpBlock_defs AbsEnv.top) v b_def).mp hdef
    have hdom_orig := h.use_dom_def v b_use b_def huse_orig hdef_orig
    exact (dom_mapFunc_iff f (fun b => (sccpBlock AbsEnv.top b).2)
      (fun b => rfl) b_def b_use).mpr hdom_orig
  · -- Entry preserved
    show ((sccpFunc f).blocks (sccpFunc f).entry).isSome
    unfold sccpFunc
    exact mapFunc_blocks_isSome_gen (fun ⟨l, _⟩ => by simp) h.entry_exists

/-- sccpMultiBlock preserves block definitions. -/
theorem sccpMultiBlock_defs (σ : AbsEnv) (b : Block) :
    blockAllDefs (sccpMultiBlock σ b) = blockAllDefs b := by
  simp only [blockAllDefs, sccpMultiBlock, sccpInstrs_dsts]

/-- Reverse lookup for label-dependent block maps: if the mapped function
    has a block blk' at label lbl, then the original had some block there
    and the transform was applied. -/
private theorem blocks_map_gen_some_rev (f : Func) (g : Label → Block → Block) (lbl : Label)
    (blk' : Block)
    (h : ({ f with blockList := f.blockList.map fun (l, b) => (l, g l b) } : Func).blocks lbl = some blk') :
    ∃ blk, f.blocks lbl = some blk ∧ blk' = g lbl blk := by
  simp only [Func.blocks] at *
  generalize f.blockList = xs at h ⊢
  induction xs with
  | nil => simp_all [List.find?]
  | cons p rest ih =>
    obtain ⟨l, b⟩ := p
    simp only [List.map, List.find?] at *
    cases hlbl : (l == lbl) <;> simp_all

/-- For a label-dependent block map where each transform preserves defs,
    DefinedIn in the transformed function implies DefinedIn in the original. -/
private theorem definedIn_mapGen_imp (f : Func) (g : Label → Block → Block)
    (hdefs : ∀ lbl b, blockAllDefs (g lbl b) = blockAllDefs b) (v : Var) (lbl : Label)
    (h : DefinedIn { f with blockList := f.blockList.map fun (l, b) => (l, g l b) } v lbl) :
    DefinedIn f v lbl := by
  obtain ⟨blk', hblk', hv⟩ := h
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_gen_some_rev f g lbl blk' hblk'
  exact ⟨blk, hblk, (hdefs lbl blk) ▸ hv⟩

/-- sccpMultiApply preserves SSA. -/
theorem sccpMultiApply_preserves_ssa (f : Func) (st : SCCPState) (h : SSAWellFormed f) :
    SSAWellFormed (sccpMultiApply f st) := by
  constructor
  · -- unique_defs
    show ∀ v lbl₁ lbl₂, DefinedIn (sccpMultiApply f st) v lbl₁ →
      DefinedIn (sccpMultiApply f st) v lbl₂ → lbl₁ = lbl₂
    unfold sccpMultiApply
    intro v lbl₁ lbl₂ h₁ h₂
    have hdefs : ∀ l b, blockAllDefs (sccpMultiBlock (st.blockStates l).inEnv b) = blockAllDefs b :=
      fun l b => sccpMultiBlock_defs _ b
    exact h.unique_defs v lbl₁ lbl₂
      (definedIn_mapGen_imp f (fun l b => sccpMultiBlock (st.blockStates l).inEnv b) hdefs v lbl₁ h₁)
      (definedIn_mapGen_imp f (fun l b => sccpMultiBlock (st.blockStates l).inEnv b) hdefs v lbl₂ h₂)
  · -- use_dom_def: sccpMultiBlock only changes instruction RHS (uses subset),
    -- and terminators are unchanged (CFG structure identical).
    intro v b_use b_def huse hdef
    unfold sccpMultiApply at huse hdef
    have hdefs : ∀ l b, blockAllDefs (sccpMultiBlock (st.blockStates l).inEnv b) = blockAllDefs b :=
      fun l b => sccpMultiBlock_defs _ b
    have hdef_orig := definedIn_mapGen_imp f
      (fun l b => sccpMultiBlock (st.blockStates l).inEnv b) hdefs v b_def hdef
    -- Transfer UsedIn back to original
    have huse_orig : UsedIn f v b_use := by
      obtain ⟨blk', hblk', hv⟩ := huse
      obtain ⟨blk, hblk, rfl⟩ := blocks_map_gen_some_rev f
        (fun l b => sccpMultiBlock (st.blockStates l).inEnv b) b_use blk' hblk'
      refine ⟨blk, hblk, ?_⟩
      simp only [blockAllUses, sccpMultiBlock] at hv ⊢
      rcases List.mem_append.mp hv with hi | ht
      · exact List.mem_append_left _ (sccpInstrs_uses_subset _ blk.instrs v hi)
      · exact List.mem_append_right _ ht
    have hdom_orig := h.use_dom_def v b_use b_def huse_orig hdef_orig
    -- Dom transfers because terminators are unchanged (same CFG)
    -- IsSuccessor in mapped → IsSuccessor in original
    have hsucc_back : ∀ l1 l2,
        IsSuccessor { f with blockList := f.blockList.map fun (l, b) =>
          (l, sccpMultiBlock (st.blockStates l).inEnv b) } l1 l2 →
        IsSuccessor f l1 l2 := by
      intro l1 l2 ⟨blk', hblk', hmem⟩
      obtain ⟨blk, hblk, rfl⟩ := blocks_map_gen_some_rev f
        (fun l b => sccpMultiBlock (st.blockStates l).inEnv b) l1 blk' hblk'
      exact ⟨blk, hblk, by simp only [sccpMultiBlock, termSuccessors] at hmem ⊢; exact hmem⟩
    -- Reachable in mapped → Reachable in original
    have hreach_back : ∀ l1 l2,
        Reachable { f with blockList := f.blockList.map fun (l, b) =>
          (l, sccpMultiBlock (st.blockStates l).inEnv b) } l1 l2 →
        Reachable f l1 l2 := by
      intro l1 l2 hr
      induction hr with
      | refl => exact .refl _
      | step a b c hab _ ih => exact .step a b c (hsucc_back a b hab) ih
    -- CFGPath in mapped → CFGPath in original
    have hpath_back : ∀ src dst path,
        CFGPath { f with blockList := f.blockList.map fun (l, b) =>
          (l, sccpMultiBlock (st.blockStates l).inEnv b) } src dst path →
        CFGPath f src dst path := by
      intro src dst path hp
      induction hp with
      | single l => exact .single l
      | cons l₁ l₂ d rest hedge _ ih =>
        exact .cons l₁ l₂ d rest (hsucc_back l₁ l₂ hedge) ih
    show Dom { f with blockList := f.blockList.map fun (l, b) =>
      (l, sccpMultiBlock (st.blockStates l).inEnv b) } b_def b_use
    intro hreach path hpath
    have hpath_orig := hpath_back _ _ _ hpath
    exact hdom_orig (cfgPath_implies_reachable hpath_orig) path hpath_orig
  · show ((sccpMultiApply f st).blocks (sccpMultiApply f st).entry).isSome
    unfold sccpMultiApply
    exact mapFunc_blocks_isSome_gen (fun ⟨l, _⟩ => by simp) h.entry_exists

/-- Multi-block SCCP preserves SSA. -/
theorem sccpMulti_preserves_ssa (f : Func) (fuel : Nat) (h : SSAWellFormed f) :
    SSAWellFormed (sccpMultiFunc f fuel) := by
  unfold sccpMultiFunc
  exact sccpMultiApply_preserves_ssa f (sccpWorklist f fuel) h

-- ══════════════════════════════════════════════════════════════════
-- Section 5: CSE preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- CSE preserves instruction destinations: cseInstr only changes RHS. -/
theorem cseInstr_dst (avail : AvailMap) (i : Instr) :
    (cseInstr avail i).1.dst = i.dst := by
  unfold cseInstr; rfl

/-- CSE preserves instruction list destinations. -/
theorem cseInstrs_dsts (avail : AvailMap) (instrs : List Instr) :
    (cseInstrs avail instrs).map Instr.dst = instrs.map Instr.dst := by
  induction instrs generalizing avail with
  | nil => simp [cseInstrs]
  | cons i rest ih =>
    simp only [cseInstrs, List.map, cseInstr_dst, ih]

/-- CSE preserves block definitions. -/
theorem cseBlock_defs (b : Block) :
    blockAllDefs (cseBlock b) = blockAllDefs b := by
  simp only [blockAllDefs, cseBlock, cseInstrs_dsts]


/-- CSE preserves terminator successors (only changes expressions). -/
private theorem cseTerminator_successors (avail : AvailMap) (t : Terminator) :
    termSuccessors (cseTerminator avail t) = termSuccessors t := by
  cases t with
  | ret _ => simp [cseTerminator, termSuccessors]
  | jmp _ _ => simp [cseTerminator, termSuccessors]
  | br _ _ _ _ _ => simp [cseTerminator, termSuccessors]
  | yield _ _ _ => simp [cseTerminator, termSuccessors]
  | switch _ _ _ => simp [cseTerminator, termSuccessors]
  | unreachable => simp [cseTerminator, termSuccessors]

/-- cseBlock preserves terminator successors. -/
private theorem cseBlock_successors (b : Block) :
    termSuccessors (cseBlock b).term = termSuccessors b.term := by
  simp [cseBlock, cseTerminator_successors]

/-- CSE preserves SSA: it replaces RHS expressions with variable
    references to equivalent earlier computations, but never changes
    which variables are defined or their defining blocks. -/
theorem cse_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (cseFunc f) := by
  constructor
  · show ∀ v lbl₁ lbl₂, DefinedIn (cseFunc f) v lbl₁ →
      DefinedIn (cseFunc f) v lbl₂ → lbl₁ = lbl₂
    unfold cseFunc
    exact unique_defs_of_mapFunc f cseBlock cseBlock_defs h.unique_defs
  · -- use_dom_def: CSE may introduce new uses (.var v from avail map),
    -- but those variables are defined in the same block → Dom reflexive.
    -- Original uses transfer via dom_mapFunc_iff (terminators unchanged).
    intro v b_use b_def huse hdef
    unfold cseFunc at huse hdef
    have hdef_orig := (definedIn_mapFunc_iff f cseBlock cseBlock_defs v b_def).mp hdef
    obtain ⟨blk', hblk', hv_use⟩ := huse
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f cseBlock b_use blk' hblk'
    by_cases hv_orig : v ∈ blockAllUses blk
    · -- v was used in the original block
      have huse_orig : UsedIn f v b_use := ⟨blk, hblk, hv_orig⟩
      have hdom_orig := h.use_dom_def v b_use b_def huse_orig hdef_orig
      exact (dom_mapFunc_iff f cseBlock cseBlock_successors b_def b_use).mpr hdom_orig
    · -- v is a NEW use introduced by CSE. CSE only introduces uses of
      -- variables from the availability map, which are instruction dsts
      -- of the same block. So v ∈ blockAllDefs blk.
      have hv_in_defs : v ∈ blockAllDefs blk := by
        -- CSE block uses ⊆ original uses ∪ instruction dsts
        -- v ∉ original uses, so v ∈ instruction dsts ⊆ blockAllDefs
        simp only [blockAllUses, cseBlock] at hv_use
        rcases List.mem_append.mp hv_use with hi | ht
        · simp only [blockAllDefs]
          apply List.mem_append_right
          -- Any new use in cseInstrs comes from avail map entries,
          -- which are instruction dsts. We prove this by induction on instrs.
          have hv_not_orig_instr : v ∉ blk.instrs.flatMap (fun i => exprVars i.rhs) := by
            intro hc; exact hv_orig (List.mem_append_left _ hc)
          rcases cseInstrs_vars [] blk.instrs (blk.instrs.map Instr.dst)
            (by simp) (fun d hd => hd) v hi with h_orig | h_dst
          · exact absurd h_orig hv_not_orig_instr
          · exact h_dst
        · -- v in termVars of cseTerminator but not in original termVars
          -- cseTerminator only uses cseExpr which can introduce avail dsts
          -- The avail for the terminator is buildAvail [] b.instrs
          -- whose entries' dsts are instruction dsts ⊆ blockAllDefs
          simp only [blockAllDefs]
          have hv_not_orig_term : v ∉ termVars blk.term := by
            intro hc; exact hv_orig (List.mem_append_right _ hc)
          have hfinal := buildAvail_dsts ([] : AvailMap) blk.instrs
            (blk.instrs.map Instr.dst) (by simp) (fun d hd => hd)
          rcases cseTerminator_vars (buildAvail [] blk.instrs) blk.term v ht with h_orig | ⟨entry, hmem, hdst⟩
          · exact absurd h_orig hv_not_orig_term
          · apply List.mem_append_right
            exact hdst ▸ hfinal entry hmem
      have hdef_at_use : DefinedIn f v b_use := ⟨blk, hblk, hv_in_defs⟩
      have heq : b_def = b_use := h.unique_defs v b_def b_use hdef_orig hdef_at_use
      rw [heq]
      exact (dom_mapFunc_iff f cseBlock cseBlock_successors b_use b_use).mpr (Dom.refl f b_use)
  · -- Entry preserved
    show ((cseFunc f).blocks (cseFunc f).entry).isSome
    unfold cseFunc
    exact mapFunc_blocks_isSome h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 6: LICM preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- licmFunc maps blockList preserving labels (fst). -/
private theorem licmFunc_preserves_fst (f : Func) (loop : NaturalLoop)
    (pre : Block) (p : Label × Block) :
    (if p.1 ∈ loop.body then
      let (_, blk') := licmBlock f loop p.2
      (p.1, blk')
    else if p.1 = loop.preheader then
      (p.1, { pre with instrs := pre.instrs ++ collectHoisted f loop })
    else
      (p.1, p.2)).1 = p.1 := by
  split
  · -- In loop body: licmBlock returns a pair; fst of (p.1, blk') = p.1
    show (let r := licmBlock f loop p.2; (p.1, r.2)).1 = p.1
    rfl
  · split <;> rfl

/-- Intra-block SSA: within each block, params and instruction dsts are disjoint.
    This is a standard SSA property guaranteed by the compiler but not modeled
    in SSAWellFormed (which only tracks inter-block uniqueness). -/
def IntraBlockDisjoint (f : Func) : Prop :=
  ∀ lbl blk, f.blocks lbl = some blk →
    ∀ v, v ∈ blk.params → v ∉ blk.instrs.map Instr.dst

/-- LICM preserves SSA.
    Requires IntraBlockDisjoint to ensure hoisted instruction dsts don't
    collide with body block params after relocation to preheader.
    Also requires the preheader to be outside the loop body. -/
theorem licm_preserves_ssa (f : Func) (loop : NaturalLoop) (pre : Block)
    (h : SSAWellFormed f) (hloop : NaturalLoop.Valid f loop)
    (hpre_dom : Dom f loop.preheader loop.header)
    (hintra : IntraBlockDisjoint f)
    (hpre_outside : loop.preheader ∉ loop.body) :
    SSAWellFormed (licmFunc f loop pre) := by
  constructor
  · -- unique_defs
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    -- Every def in licmFunc traces back to a def in f.
    -- Body blocks: remaining instrs ⊆ original → DefinedIn f v lbl
    -- Preheader: original pre defs + hoisted from body → DefinedIn f v (preheader or body_lbl)
    -- Other blocks: unchanged → DefinedIn f v lbl
    -- By h.unique_defs, both trace to the same original block.
    -- The LICM relabeling is injective (each original def goes to exactly one LICM block),
    -- so lbl₁ = lbl₂.
    -- Full proof requires partitionInstrs partition property + collectHoisted aggregation.
    sorry
  · -- use_dom_def
    intro v b_use b_def huse hdef
    -- LICM preserves terminators → dominance unchanged (dom_mapFunc_iff).
    -- Uses in body blocks: defs either in same/dominating block (original dominance)
    -- or moved to preheader (preheader dom header dom body blocks).
    -- Uses in preheader: defs are local (Dom.refl) or from original preheader.
    sorry
  · -- entry_exists
    show ((licmFunc f loop pre).blocks (licmFunc f loop pre).entry).isSome
    unfold licmFunc
    exact mapFunc_blocks_isSome_gen
      (fun p => licmFunc_preserves_fst f loop pre p) h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 7: GuardHoist preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- GuardHoist preserves instruction destinations: it only changes
    RHS of redundant guards to identity assignments. -/
theorem guardHoistInstr_dst (proven : ProvenGuards) (i : Instr) :
    (guardHoistInstr proven i).1.dst = i.dst := by
  unfold guardHoistInstr
  split
  · rfl
  · split <;> rfl

/-- guardHoistInstrs preserves instruction list destinations. -/
theorem guardHoistInstrs_dsts (proven : ProvenGuards) (instrs : List Instr) :
    (guardHoistInstrs proven instrs).map Instr.dst = instrs.map Instr.dst := by
  induction instrs generalizing proven with
  | nil => simp [guardHoistInstrs]
  | cons i rest ih =>
    simp only [guardHoistInstrs, List.map]
    congr 1
    · exact guardHoistInstr_dst proven i
    · exact ih (guardHoistInstr proven i).2

/-- guardHoistBlock preserves block definitions. -/
theorem guardHoistBlock_defs (proven : ProvenGuards) (b : Block) :
    blockAllDefs (guardHoistBlock proven b) = blockAllDefs b := by
  simp only [blockAllDefs, guardHoistBlock, guardHoistInstrs_dsts]

/-- Guard hoisting preserves SSA. -/
theorem guardHoist_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (guardHoistFunc f) := by
  constructor
  · show ∀ v lbl₁ lbl₂, DefinedIn (guardHoistFunc f) v lbl₁ →
      DefinedIn (guardHoistFunc f) v lbl₂ → lbl₁ = lbl₂
    unfold guardHoistFunc
    exact unique_defs_of_mapFunc f (guardHoistBlock []) (guardHoistBlock_defs []) h.unique_defs
  · -- use_dom_def: guardHoist may introduce new uses (`.var i.dst`),
    -- but those are defined in the same block → Dom reflexive.
    -- Original uses transfer via dom_mapFunc_iff (terminators unchanged).
    intro v b_use b_def huse hdef
    unfold guardHoistFunc at huse hdef
    have hdefs := guardHoistBlock_defs ([] : ProvenGuards)
    have hdef_orig := (definedIn_mapFunc_iff f (guardHoistBlock []) hdefs v b_def).mp hdef
    obtain ⟨blk', hblk', hv_use⟩ := huse
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f (guardHoistBlock []) b_use blk' hblk'
    by_cases hv_orig : v ∈ blockAllUses blk
    · -- v was used in the original block
      have huse_orig : UsedIn f v b_use := ⟨blk, hblk, hv_orig⟩
      have hdom_orig := h.use_dom_def v b_use b_def huse_orig hdef_orig
      exact (dom_mapFunc_iff f (guardHoistBlock []) (fun b => rfl) b_def b_use).mpr hdom_orig
    · -- v is a NEW use from `.var i.dst`. So v ∈ blockAllDefs blk.
      have hv_in_defs : v ∈ blockAllDefs blk := by
        simp only [blockAllUses, guardHoistBlock] at hv_use
        rcases List.mem_append.mp hv_use with hi | ht
        · -- v in instruction uses of guardHoistInstrs but not in original
          simp only [blockAllDefs]
          apply List.mem_append_right
          have hv_not_orig_instr : v ∉ blk.instrs.flatMap (fun i => exprVars i.rhs) := by
            intro hc; exact hv_orig (List.mem_append_left _ hc)
          -- Any use in guardHoistInstrs is either original or an instr dst
          suffices hsuff : ∀ (proven : ProvenGuards) (instrs : List Instr),
            ∀ w, w ∈ (guardHoistInstrs proven instrs).flatMap (fun i => exprVars i.rhs) →
            w ∈ instrs.flatMap (fun i => exprVars i.rhs) ∨ w ∈ instrs.map Instr.dst by
            rcases hsuff [] blk.instrs v hi with h_orig | h_dst
            · exact absurd h_orig hv_not_orig_instr
            · exact h_dst
          intro proven instrs
          induction instrs generalizing proven with
          | nil => simp [guardHoistInstrs]
          | cons i rest ih =>
            intro w hw
            simp only [guardHoistInstrs, List.flatMap_cons, List.mem_append] at hw
            rcases hw with hw_hd | hw_tl
            · simp only [guardHoistInstr] at hw_hd
              split at hw_hd
              · left; exact List.mem_append_left _ hw_hd
              · rename_i g _
                split at hw_hd
                · simp only [exprVars] at hw_hd
                  -- RHS is .val (.bool true), exprVars = [], so hw_hd is False
                  exact nomatch hw_hd
                · left; exact List.mem_append_left _ hw_hd
            · rcases ih _ w hw_tl with h_rest | h_dst_rest
              · left; exact List.mem_append_right _ h_rest
              · right; exact List.Mem.tail _ h_dst_rest
        · -- v in termVars (unchanged), contradicts hv_orig
          exact absurd (List.mem_append_right _ ht) hv_orig
      have hdef_at_use : DefinedIn f v b_use := ⟨blk, hblk, hv_in_defs⟩
      have heq : b_def = b_use := h.unique_defs v b_def b_use hdef_orig hdef_at_use
      rw [heq]
      exact (dom_mapFunc_iff f (guardHoistBlock []) (fun b => rfl) b_use b_use).mpr (Dom.refl f b_use)
  · -- Entry preserved
    show ((guardHoistFunc f).blocks (guardHoistFunc f).entry).isSome
    unfold guardHoistFunc
    exact mapFunc_blocks_isSome h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 8: JoinCanon preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- Join canonicalization only rewrites terminator labels; it does not
    touch instructions at all. -/
theorem joinCanonBlock_instrs (jmap : JoinMap) (b : Block) :
    (joinCanonBlock jmap b).instrs = b.instrs := by
  unfold joinCanonBlock; rfl

theorem joinCanonBlock_params (jmap : JoinMap) (b : Block) :
    (joinCanonBlock jmap b).params = b.params := by
  unfold joinCanonBlock; rfl

/-- joinCanonBlock preserves block definitions (only changes terminator). -/
theorem joinCanonBlock_defs (jmap : JoinMap) (b : Block) :
    blockAllDefs (joinCanonBlock jmap b) = blockAllDefs b := by
  simp only [blockAllDefs, joinCanonBlock_instrs, joinCanonBlock_params]

/-- buildJoinMap stores only (sig, sig.target) entries. -/
private theorem buildJoinMap_entries (f : Func) :
    ∀ s l, (s, l) ∈ buildJoinMap f → l = s.target := by
  -- buildJoinMap is a foldl that only ever inserts (sig, sig.target).
  -- We prove this by showing the foldl invariant: the accumulator only
  -- contains (s, s.target) entries.
  unfold buildJoinMap
  suffices hinv : ∀ (bl : List (Label × Block)) (acc : JoinMap),
    (∀ s l, (s, l) ∈ acc → l = s.target) →
    ∀ s l, (s, l) ∈ bl.foldl (fun jmap p =>
      match p.2.term with
      | .jmp target args =>
          let sig := { target := target, args := args : JoinSig }
          match joinLookup jmap sig with
          | some _ => jmap
          | none => (sig, target) :: jmap
      | _ => jmap) acc → l = s.target from
    hinv f.blockList [] (fun _ _ h => nomatch h)
  intro bl
  induction bl with
  | nil => intro acc hacc; simpa
  | cons p rest ih =>
    intro acc hacc
    simp only [List.foldl]
    apply ih
    -- Show the invariant is maintained for one step
    cases hterm : p.2.term with
    | ret _ => simp only [hterm]; exact hacc
    | jmp target args =>
      simp only [hterm]
      cases hjl : joinLookup acc { target := target, args := args : JoinSig } with
      | some _ => exact hacc
      | none =>
        intro s l hmem
        rcases List.mem_cons.mp hmem with heq | hmem'
        · have ⟨hs, hl⟩ := Prod.mk.inj heq
          rw [hl, hs]
        · exact hacc s l hmem'
    | br _ _ _ _ _ => simp only [hterm]; exact hacc
    | yield _ _ _ => simp only [hterm]; exact hacc
    | switch _ _ _ => simp only [hterm]; exact hacc
    | unreachable => simp only [hterm]; exact hacc

/-- joinLookup that returns some l means l was stored in the map at a matching key. -/
private theorem joinLookup_some_eq {jmap : JoinMap} {sig : JoinSig} {l : Label}
    (hmap : ∀ s l, (s, l) ∈ jmap → l = s.target)
    (hl : joinLookup jmap sig = some l) : l = sig.target := by
  induction jmap with
  | nil => simp [joinLookup] at hl
  | cons p rest ih =>
    obtain ⟨s', l'⟩ := p
    simp only [joinLookup] at hl
    split at hl
    · rename_i heq
      have hlbl := Option.some.inj hl
      have hmem := hmap s' l' (List.mem_cons_self)
      have hsig := beq_iff_eq.mp heq
      rw [← hlbl, hmem, hsig]
    · exact ih (fun s l hm => hmap s l (List.Mem.tail _ hm)) hl

/-- canonicalizeJump with a well-formed map preserves the target label. -/
private theorem canonicalizeJump_target {jmap : JoinMap}
    (hmap : ∀ s l, (s, l) ∈ jmap → l = s.target)
    (target : Label) (args : List Expr) :
    (canonicalizeJump jmap target args).1 = target := by
  simp only [canonicalizeJump]
  cases hl : joinLookup jmap { target := target, args := args } with
  | none => rfl
  | some canonical => exact joinLookup_some_eq hmap hl

/-- joinCanonTerminator with buildJoinMap preserves termSuccessors.
    Key insight: buildJoinMap stores (sig, sig.target), so canonicalizeJump
    always returns the original target label. -/
private theorem joinCanonTerminator_successors_eq (f : Func) (t : Terminator) :
    termSuccessors (joinCanonTerminator (buildJoinMap f) t) = termSuccessors t := by
  have hmap := buildJoinMap_entries f
  cases t with
  | ret _ => simp [joinCanonTerminator, termSuccessors]
  | jmp target args =>
    simp only [joinCanonTerminator, canonicalizeJump, termSuccessors]
    cases hl : joinLookup (buildJoinMap f) { target := target, args := args } with
    | none => rfl
    | some canonical =>
      simp only [termSuccessors]
      exact congrArg (· :: []) (joinLookup_some_eq hmap hl)
  | br cond tl ta el ea =>
    simp only [joinCanonTerminator, termSuccessors]
    have htl := canonicalizeJump_target hmap tl ta
    have hel := canonicalizeJump_target hmap el ea
    rw [htl, hel]
  | yield val resume resumeArgs =>
    simp only [joinCanonTerminator, canonicalizeJump, termSuccessors]
    cases hl : joinLookup (buildJoinMap f) { target := resume, args := resumeArgs } with
    | none => rfl
    | some canonical =>
      simp only [termSuccessors]
      exact congrArg (· :: []) (joinLookup_some_eq hmap hl)
  | switch _ _ _ => simp [joinCanonTerminator, termSuccessors]
  | unreachable => simp [joinCanonTerminator, termSuccessors]

/-- Join canonicalization preserves SSA. -/
theorem joinCanon_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (joinCanonFunc f) := by
  constructor
  · -- unique_defs: Instructions and params unchanged, so defs unchanged
    show ∀ v lbl₁ lbl₂, DefinedIn (joinCanonFunc f) v lbl₁ →
      DefinedIn (joinCanonFunc f) v lbl₂ → lbl₁ = lbl₂
    unfold joinCanonFunc
    exact unique_defs_of_mapFunc f (joinCanonBlock (buildJoinMap f))
      (joinCanonBlock_defs (buildJoinMap f)) h.unique_defs
  · -- use_dom_def: joinCanon preserves both blockAllDefs and termSuccessors
    -- (canonicalizeJump returns the original target because buildJoinMap
    -- stores (sig, sig.target)). Uses transfer because joinCanonTerminator
    -- only changes labels, not expressions. Apply generic mapFunc machinery.
    show ∀ v b_use b_def, UsedIn (joinCanonFunc f) v b_use →
      DefinedIn (joinCanonFunc f) v b_def → Dom (joinCanonFunc f) b_def b_use
    unfold joinCanonFunc
    have hterm : ∀ b, termSuccessors (joinCanonBlock (buildJoinMap f) b).term =
        termSuccessors b.term := by
      intro b; simp only [joinCanonBlock]
      exact joinCanonTerminator_successors_eq f b.term
    -- blockAllUses in new → blockAllUses in old (instructions same, termVars same)
    have huses : ∀ v lbl,
        UsedIn { f with blockList := f.blockList.map fun (l, b) =>
          (l, joinCanonBlock (buildJoinMap f) b) } v lbl →
        UsedIn f v lbl := by
      intro v lbl ⟨blk', hblk', hv⟩
      obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev f
        (joinCanonBlock (buildJoinMap f)) lbl blk' hblk'
      refine ⟨blk, hblk, ?_⟩
      simp only [blockAllUses, joinCanonBlock] at hv ⊢
      rcases List.mem_append.mp hv with hi | ht
      · exact List.mem_append_left _ hi
      · apply List.mem_append_right
        -- joinCanonTerminator only changes labels, termVars preserved
        revert ht
        cases blk.term with
        | ret _ => simp [joinCanonTerminator, termVars]
        | jmp target args =>
          simp only [joinCanonTerminator, canonicalizeJump, termVars]
          cases joinLookup (buildJoinMap f) { target := target, args := args } <;>
          exact id
        | br cond tl ta el ea =>
          simp only [joinCanonTerminator, canonicalizeJump, termVars]
          cases joinLookup (buildJoinMap f) { target := tl, args := ta } <;>
          cases joinLookup (buildJoinMap f) { target := el, args := ea } <;>
          simp only [termVars] <;> exact id
        | yield val resume resumeArgs =>
          simp only [joinCanonTerminator, canonicalizeJump, termVars]
          cases joinLookup (buildJoinMap f) { target := resume, args := resumeArgs } <;>
          simp only [termVars] <;> exact id
        | switch _ _ _ =>
          simp only [joinCanonTerminator, termVars]; exact id
        | unreachable =>
          simp only [joinCanonTerminator, termVars]; exact id
    exact use_dom_def_of_mapFunc f (joinCanonBlock (buildJoinMap f))
      (joinCanonBlock_defs (buildJoinMap f)) hterm huses h
  · -- Entry preserved
    show ((joinCanonFunc f).blocks (joinCanonFunc f).entry).isSome
    unfold joinCanonFunc
    simp only
    exact mapFunc_blocks_isSome_gen (fun ⟨l, _⟩ => by simp) h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 9: EdgeThread preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- Edge threading only rewrites terminators (br -> jmp). -/
theorem edgeThreadBlock_instrs (σ : AbsEnv) (b : Block) :
    (edgeThreadBlock σ b).instrs = b.instrs := by
  unfold edgeThreadBlock; rfl

/-- edgeThreadBlock preserves params. -/
theorem edgeThreadBlock_params (σ : AbsEnv) (b : Block) :
    (edgeThreadBlock σ b).params = b.params := by
  unfold edgeThreadBlock; rfl

/-- edgeThreadBlock preserves block definitions (only changes terminator). -/
theorem edgeThreadBlock_defs (σ : AbsEnv) (b : Block) :
    blockAllDefs (edgeThreadBlock σ b) = blockAllDefs b := by
  simp only [blockAllDefs, edgeThreadBlock_instrs, edgeThreadBlock_params]

/-- edgeThreadTerminator successors are a subset of original successors.
    br→jmp removes one branch, so the successor set shrinks or stays equal. -/
private theorem edgeThreadTerminator_successors_subset (σ : AbsEnv) (t : Terminator) :
    ∀ l, l ∈ termSuccessors (edgeThreadTerminator σ t) → l ∈ termSuccessors t := by
  intro l hl
  cases t with
  | ret _ => simp [edgeThreadTerminator, termSuccessors] at hl
  | jmp target args => simp [edgeThreadTerminator] at hl; exact hl
  | br cond tl ta el ea =>
    -- Case split on the abstract evaluation of the condition
    simp only [edgeThreadTerminator] at hl
    cases habsEval : absEvalExpr σ cond with
    | unknown => simp [habsEval] at hl; exact hl
    | overdefined => simp [habsEval] at hl; exact hl
    | known cv =>
      cases cv with
      | bool b =>
        cases b with
        | true =>
          simp [habsEval, termSuccessors] at hl
          simp [termSuccessors, hl]
        | false =>
          simp [habsEval, termSuccessors] at hl
          simp [termSuccessors, hl]
      | int n => simp [habsEval] at hl; exact hl
      | float n => simp [habsEval] at hl; exact hl
      | str s => simp [habsEval] at hl; exact hl
      | none => simp [habsEval] at hl; exact hl
  | yield _ _ _ => simp [edgeThreadTerminator] at hl; exact hl
  | switch _ _ _ => simp [edgeThreadTerminator] at hl; exact hl
  | unreachable => simp [edgeThreadTerminator, termSuccessors] at hl

/-- edgeThreadTerminator only removes vars: termVars subset. -/
private theorem edgeThreadTerminator_vars_subset (σ : AbsEnv) (t : Terminator) :
    ∀ v, v ∈ termVars (edgeThreadTerminator σ t) → v ∈ termVars t := by
  intro v hv
  cases t with
  | ret _ => simp [edgeThreadTerminator] at hv; exact hv
  | jmp _ _ => simp [edgeThreadTerminator] at hv; exact hv
  | br cond tl ta el ea =>
    simp only [edgeThreadTerminator] at hv
    cases habsEval : absEvalExpr σ cond with
    | unknown => simp [habsEval] at hv; exact hv
    | overdefined => simp [habsEval] at hv; exact hv
    | known cv =>
      cases cv with
      | bool b =>
        cases b with
        | true =>
          simp [habsEval, termVars] at hv
          simp [termVars]
          exact Or.inr (Or.inl hv)
        | false =>
          simp [habsEval, termVars] at hv
          simp [termVars]
          exact Or.inr (Or.inr hv)
      | int n => simp [habsEval] at hv; exact hv
      | float n => simp [habsEval] at hv; exact hv
      | str s => simp [habsEval] at hv; exact hv
      | none => simp [habsEval] at hv; exact hv
  | yield _ _ _ => simp [edgeThreadTerminator] at hv; exact hv
  | switch _ _ _ => simp [edgeThreadTerminator] at hv; exact hv
  | unreachable => simp [edgeThreadTerminator, termVars] at hv

/-- Edge threading preserves SSA: only terminators change.
    Edge threading only removes edges (br->jmp removes one successor).
    Removing edges cannot break dominance of existing def-use pairs:
    a definition that dominated a use via all paths still dominates
    via the subset of paths that remain. -/
theorem edgeThread_preserves_ssa (f : Func) (st : SCCPState) (h : SSAWellFormed f) :
    SSAWellFormed (edgeThreadFunc f st) := by
  constructor
  · -- unique_defs: Instructions unchanged
    show ∀ v lbl₁ lbl₂, DefinedIn (edgeThreadFunc f st) v lbl₁ →
      DefinedIn (edgeThreadFunc f st) v lbl₂ → lbl₁ = lbl₂
    unfold edgeThreadFunc
    intro v lbl₁ lbl₂ h₁ h₂
    have hdefs : ∀ l b, blockAllDefs (edgeThreadBlock (st.blockStates l).inEnv b) = blockAllDefs b :=
      fun l b => edgeThreadBlock_defs _ b
    exact h.unique_defs v lbl₁ lbl₂
      (definedIn_mapGen_imp f (fun l b => edgeThreadBlock (st.blockStates l).inEnv b) hdefs v lbl₁ h₁)
      (definedIn_mapGen_imp f (fun l b => edgeThreadBlock (st.blockStates l).inEnv b) hdefs v lbl₂ h₂)
  · -- use_dom_def: edge threading removes edges, which strengthens dominance.
    -- Every IsSuccessor in the new function is also an IsSuccessor in the
    -- original. So every CFGPath in new implies a CFGPath in the original.
    -- Dom in original → Dom in new (fewer paths to check in new function).
    intro v b_use b_def huse hdef
    unfold edgeThreadFunc at huse hdef
    have hdefs : ∀ l b, blockAllDefs (edgeThreadBlock (st.blockStates l).inEnv b) = blockAllDefs b :=
      fun l b => edgeThreadBlock_defs _ b
    have hdef_orig := definedIn_mapGen_imp f
      (fun l b => edgeThreadBlock (st.blockStates l).inEnv b) hdefs v b_def hdef
    -- UsedIn transfers: instructions unchanged, termVars subset
    have huse_orig : UsedIn f v b_use := by
      obtain ⟨blk', hblk', hv⟩ := huse
      obtain ⟨blk, hblk, rfl⟩ := blocks_map_gen_some_rev f
        (fun l b => edgeThreadBlock (st.blockStates l).inEnv b) b_use blk' hblk'
      refine ⟨blk, hblk, ?_⟩
      simp only [blockAllUses, edgeThreadBlock] at hv ⊢
      rcases List.mem_append.mp hv with hi | ht
      · exact List.mem_append_left _ hi
      · exact List.mem_append_right _
          (edgeThreadTerminator_vars_subset _ blk.term v ht)
    have hdom_orig := h.use_dom_def v b_use b_def huse_orig hdef_orig
    -- Transfer IsSuccessor: new → original (edge threading only removes edges)
    have hsucc_back : ∀ l1 l2,
        IsSuccessor { f with blockList := f.blockList.map fun (l, b) =>
          (l, edgeThreadBlock (st.blockStates l).inEnv b) } l1 l2 →
        IsSuccessor f l1 l2 := by
      intro l1 l2 ⟨blk', hblk', hmem⟩
      obtain ⟨blk, hblk, rfl⟩ := blocks_map_gen_some_rev f
        (fun l b => edgeThreadBlock (st.blockStates l).inEnv b) l1 blk' hblk'
      exact ⟨blk, hblk, edgeThreadTerminator_successors_subset _ blk.term l2
        (by simp only [edgeThreadBlock] at hmem; exact hmem)⟩
    -- CFGPath transfer: new → original
    have hpath_back : ∀ src dst path,
        CFGPath { f with blockList := f.blockList.map fun (l, b) =>
          (l, edgeThreadBlock (st.blockStates l).inEnv b) } src dst path →
        CFGPath f src dst path := by
      intro src dst path hp
      induction hp with
      | single l => exact .single l
      | cons l₁ l₂ d rest hedge _ ih =>
        exact .cons l₁ l₂ d rest (hsucc_back l₁ l₂ hedge) ih
    -- Dom transfer: original → new (fewer paths)
    show Dom { f with blockList := f.blockList.map fun (l, b) =>
      (l, edgeThreadBlock (st.blockStates l).inEnv b) } b_def b_use
    intro hreach path hpath
    exact hdom_orig (cfgPath_implies_reachable (hpath_back _ _ _ hpath))
      path (hpath_back _ _ _ hpath)
  · -- Entry preserved
    show ((edgeThreadFunc f st).blocks (edgeThreadFunc f st).entry).isSome
    unfold edgeThreadFunc
    exact mapFunc_blocks_isSome_gen (fun ⟨l, _⟩ => by simp) h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 10: Pipeline composition
-- ══════════════════════════════════════════════════════════════════

/-- SSA preservation composes: if pass_1 and pass_2 each preserve SSA,
    then their sequential composition preserves SSA. -/
theorem ssa_compose {pass₁ pass₂ : Func → Func}
    (h₁ : ∀ f, SSAWellFormed f → SSAWellFormed (pass₁ f))
    (h₂ : ∀ f, SSAWellFormed f → SSAWellFormed (pass₂ f))
    (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (pass₂ (pass₁ f)) :=
  h₂ (pass₁ f) (h₁ f h)

end MoltTIR
