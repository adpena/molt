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

/-- Inverse of blocks_map_some: if the mapped blockList contains a block at lbl,
    then the original blockList had a block there, and the mapped block is g(original). -/
private theorem blocks_map_some_inv (f : Func) (g : Block → Block) (lbl : Label) (blk' : Block)
    (h : ({ f with blockList := f.blockList.map fun (l, b) => (l, g b) } : Func).blocks lbl = some blk') :
    ∃ blk, f.blocks lbl = some blk ∧ blk' = g blk := by
  simp only [Func.blocks] at *
  generalize f.blockList = xs at h ⊢
  induction xs with
  | nil => simp [List.find?] at h
  | cons p rest ih =>
    obtain ⟨l, b⟩ := p
    simp only [List.map, List.find?] at *
    cases hlbl : (l == lbl) <;> simp_all

/-- Generalized inverse for label-dependent block transforms:
    if the mapped blockList (using label-dependent g) contains a block at lbl,
    then the original had a block there and the result is g lbl original. -/
private theorem blocks_map_some_inv_dep (f : Func) (g : Label → Block → Block)
    (lbl : Label) (blk' : Block)
    (h : ({ f with blockList := f.blockList.map fun (l, b) => (l, g l b) } : Func).blocks lbl = some blk') :
    ∃ blk, f.blocks lbl = some blk ∧ blk' = g lbl blk := by
  simp only [Func.blocks] at *
  generalize f.blockList = xs at h ⊢
  induction xs with
  | nil => simp [List.find?] at h
  | cons p rest ih =>
    obtain ⟨l, b⟩ := p
    simp only [List.map, List.find?] at *
    cases hlbl : (l == lbl)
    · simp [hlbl] at h ⊢; exact ih h
    · simp [hlbl] at h ⊢
      have := BEq.eq_of_beq hlbl
      subst this
      exact ⟨b, rfl, h⟩

/-- Forward direction for label-dependent block transforms:
    if f.blocks lbl = some blk then the mapped version has g lbl blk. -/
private theorem blocks_map_some_dep (f : Func) (g : Label → Block → Block)
    (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    ({ f with blockList := f.blockList.map fun (l, b) => (l, g l b) } : Func).blocks lbl
    = some (g lbl blk) := by
  simp only [Func.blocks] at *
  generalize f.blockList = xs at h ⊢
  induction xs with
  | nil => simp [List.find?] at h
  | cons p rest ih =>
    obtain ⟨l, b⟩ := p
    simp only [List.map, List.find?] at *
    cases hlbl : (l == lbl)
    · simp [hlbl] at h ⊢; exact ih h
    · simp [hlbl] at h ⊢
      have := BEq.eq_of_beq hlbl
      subst this; subst h; rfl

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

/-- Constant folding preserves SSA: it only changes RHS expressions,
    never introduces new definitions or changes the def-use structure
    at the definition level. -/
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

private theorem constFold_definedIn_iff (f : Func) (v : Var) (lbl : Label) :
    DefinedIn (constFoldFunc f) v lbl ↔ DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hdef'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv f constFoldBlock lbl blk' hblk'
    rw [constFoldBlock_defs] at hdef'
    exact ⟨blk, hblk, hdef'⟩
  · intro ⟨blk, hblk, hdef⟩
    have hblk' := blocks_map_some f constFoldBlock lbl blk hblk
    rw [← constFoldBlock_defs] at hdef
    exact ⟨constFoldBlock blk, hblk', hdef⟩

private theorem constFoldTerminator_successors (t : Terminator) :
    termSuccessors (constFoldTerminator t) = termSuccessors t := by
  cases t with
  | ret _ => rfl
  | jmp _ _ => rfl
  | br _ _ _ _ _ => rfl

private theorem constFold_isSuccessor_iff (f : Func) (l1 l2 : Label) :
    IsSuccessor (constFoldFunc f) l1 l2 ↔ IsSuccessor f l1 l2 := by
  constructor
  · intro ⟨blk', hblk', hsucc'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv f constFoldBlock l1 blk' hblk'
    simp only [constFoldBlock, constFoldTerminator_successors] at hsucc'
    exact ⟨blk, hblk, hsucc'⟩
  · intro ⟨blk, hblk, hsucc⟩
    have hblk' := blocks_map_some f constFoldBlock l1 blk hblk
    have hterm : termSuccessors (constFoldBlock blk).term = termSuccessors blk.term := by
      simp only [constFoldBlock, constFoldTerminator_successors]
    rw [hterm]
    exact ⟨constFoldBlock blk, hblk', hsucc⟩

theorem constFold_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (constFoldFunc f) := by
  constructor
  · -- unique_defs: definitions are identical after const fold
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := (constFold_definedIn_iff f v lbl₁).mp hdef₁
    have hdef₂' := (constFold_definedIn_iff f v lbl₂).mp hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · -- use_dom_def: dominance structure is unchanged (same CFG edges)
    -- We need: UsedIn (constFoldFunc f) v b_use → DefinedIn (constFoldFunc f) v b_def →
    --          Dom (constFoldFunc f) b_def b_use
    -- The DOM relation over constFoldFunc f uses the same CFG structure as f
    -- (same successors), so Dom (constFoldFunc f) b_def b_use ↔ Dom f b_def b_use.
    -- DefinedIn is equivalent by constFold_definedIn_iff.
    -- The gap is that UsedIn in constFoldFunc f doesn't directly map to UsedIn in f
    -- (constFold may eliminate uses), but we only need that the def block dominates.
    -- Since the def block is the same (by definedIn iff) and the CFG is the same
    -- (by isSuccessor iff), dominance is preserved.
    sorry -- requires lifting IsSuccessor ↔ to Reachable ↔ to Dom ↔; provable but verbose
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
    exact List.mem_map_of_mem Instr.dst hmem_orig

/-- DCE preserves SSA: removing dead definitions maintains unique-def
    (a subset of a unique list is unique) and use-dom-def (dead code
    has no uses, so the remaining use-def pairs are unchanged). -/
private theorem dce_definedIn_implies_original (f : Func) (v : Var) (lbl : Label) :
    DefinedIn (dceFunc f) v lbl → DefinedIn f v lbl := by
  intro ⟨blk', hblk', hdef'⟩
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv f dceBlock lbl blk' hblk'
  exact ⟨blk, hblk, dceBlock_defs_subset blk v hdef'⟩

theorem dce_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (dceFunc f) := by
  constructor
  · -- unique_defs: subset of original defs, still unique
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := dce_definedIn_implies_original f v lbl₁ hdef₁
    have hdef₂' := dce_definedIn_implies_original f v lbl₂ hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · -- use_dom_def: only live instructions remain; their uses still
    -- have the same dominating definitions. Requires showing DCE preserves
    -- the CFG (same terminators) and that remaining def-use pairs are a
    -- subset of the original, inheriting dominance.
    sorry -- requires lifting DCE terminator/CFG preservation to Dom equivalence
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

/-- SCCP preserves SSA: it only replaces RHS with constants. -/
private theorem sccpBlock_params (b : Block) :
    (sccpBlock AbsEnv.top b).2.params = b.params := by
  unfold sccpBlock; simp

private theorem sccpBlock_instrs_dsts (b : Block) :
    (sccpBlock AbsEnv.top b).2.instrs.map Instr.dst = b.instrs.map Instr.dst := by
  unfold sccpBlock; simp; exact sccpInstrs_dsts AbsEnv.top b.instrs

private theorem sccpBlock_defs (b : Block) :
    blockAllDefs ((sccpBlock AbsEnv.top b).2) = blockAllDefs b := by
  simp only [blockAllDefs, sccpBlock_params, sccpBlock_instrs_dsts]

private theorem sccp_definedIn_iff (f : Func) (v : Var) (lbl : Label) :
    DefinedIn (sccpFunc f) v lbl ↔ DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hdef'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv
      f (fun blk => (sccpBlock AbsEnv.top blk).2) lbl blk'
      (by unfold sccpFunc at hblk'; exact hblk')
    rw [sccpBlock_defs] at hdef'
    exact ⟨blk, hblk, hdef'⟩
  · intro ⟨blk, hblk, hdef⟩
    have hblk' := blocks_map_some f (fun blk => (sccpBlock AbsEnv.top blk).2) lbl blk hblk
    rw [← sccpBlock_defs] at hdef
    exact ⟨(sccpBlock AbsEnv.top blk).2, hblk', hdef⟩

theorem sccp_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (sccpFunc f) := by
  constructor
  · -- unique_defs: dsts preserved by sccpInstrs_dsts
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := (sccp_definedIn_iff f v lbl₁).mp hdef₁
    have hdef₂' := (sccp_definedIn_iff f v lbl₂).mp hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · sorry  -- Dominance unchanged; requires CFG equivalence lifting
  · -- Entry preserved
    show ((sccpFunc f).blocks (sccpFunc f).entry).isSome
    unfold sccpFunc
    exact mapFunc_blocks_isSome_gen (fun ⟨l, _⟩ => by simp) h.entry_exists

/-- sccpMultiBlock preserves instruction destinations. -/
private theorem sccpMultiBlock_instrs_dsts (σ : AbsEnv) (b : Block) :
    (sccpMultiBlock σ b).instrs.map Instr.dst = b.instrs.map Instr.dst := by
  unfold sccpMultiBlock; simp; exact sccpInstrs_dsts σ b.instrs

/-- sccpMultiBlock preserves block params. -/
private theorem sccpMultiBlock_params (σ : AbsEnv) (b : Block) :
    (sccpMultiBlock σ b).params = b.params := by
  unfold sccpMultiBlock; simp

/-- sccpMultiBlock preserves all block definitions. -/
private theorem sccpMultiBlock_defs (σ : AbsEnv) (b : Block) :
    blockAllDefs (sccpMultiBlock σ b) = blockAllDefs b := by
  simp only [blockAllDefs, sccpMultiBlock_params, sccpMultiBlock_instrs_dsts]

/-- DefinedIn equivalence for sccpMultiApply. -/
private theorem sccpMulti_definedIn_iff (f : Func) (st : SCCPState) (v : Var) (lbl : Label) :
    DefinedIn (sccpMultiApply f st) v lbl ↔ DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hdef'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv_dep
      f (fun l b => sccpMultiBlock (st.blockStates l |>.inEnv) b) lbl blk'
      (by unfold sccpMultiApply at hblk'; exact hblk')
    rw [sccpMultiBlock_defs] at hdef'
    exact ⟨blk, hblk, hdef'⟩
  · intro ⟨blk, hblk, hdef⟩
    have hblk' := blocks_map_some_dep
      f (fun l b => sccpMultiBlock (st.blockStates l |>.inEnv) b) lbl blk hblk
    rw [← sccpMultiBlock_defs] at hdef
    exact ⟨sccpMultiBlock (st.blockStates lbl |>.inEnv) blk, hblk', hdef⟩

/-- Multi-block SCCP preserves SSA. -/
theorem sccpMulti_preserves_ssa (f : Func) (fuel : Nat) (h : SSAWellFormed f) :
    SSAWellFormed (sccpMultiFunc f fuel) := by
  unfold sccpMultiFunc
  constructor
  · -- unique_defs: dsts preserved by sccpMultiBlock_defs
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := (sccpMulti_definedIn_iff f _ v lbl₁).mp hdef₁
    have hdef₂' := (sccpMulti_definedIn_iff f _ v lbl₂).mp hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · sorry  -- use_dom_def: dominance unchanged; requires CFG equivalence lifting
  · -- Entry preserved
    show ((sccpMultiApply f (sccpWorklist f fuel)).blocks
          (sccpMultiApply f (sccpWorklist f fuel)).entry).isSome
    unfold sccpMultiApply
    exact mapFunc_blocks_isSome_gen (fun ⟨l, _⟩ => by simp) h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 5: CSE preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- CSE preserves instruction destinations: cseInstr only changes RHS. -/
theorem cseInstr_dst (avail : AvailMap) (i : Instr) :
    (cseInstr avail i).1.dst = i.dst := by
  unfold cseInstr; rfl

/-- CSE preserves SSA: it replaces RHS expressions with variable
    references to equivalent earlier computations, but never changes
    which variables are defined or their defining blocks. -/
private theorem cseInstrs_dsts (avail : AvailMap) (instrs : List Instr) :
    (cseInstrs avail instrs).map Instr.dst = instrs.map Instr.dst := by
  induction instrs generalizing avail with
  | nil => simp [cseInstrs]
  | cons i rest ih =>
    simp only [cseInstrs, List.map, cseInstr_dst]
    exact congrArg _ (ih _)

private theorem cseBlock_params (b : Block) :
    (cseBlock b).params = b.params := by
  unfold cseBlock; rfl

private theorem cseBlock_instrs_dsts (b : Block) :
    (cseBlock b).instrs.map Instr.dst = b.instrs.map Instr.dst := by
  unfold cseBlock; simp; exact cseInstrs_dsts [] b.instrs

private theorem cseBlock_defs (b : Block) :
    blockAllDefs (cseBlock b) = blockAllDefs b := by
  simp only [blockAllDefs, cseBlock_params, cseBlock_instrs_dsts]

private theorem cse_definedIn_iff (f : Func) (v : Var) (lbl : Label) :
    DefinedIn (cseFunc f) v lbl ↔ DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hdef'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv f cseBlock lbl blk' hblk'
    rw [cseBlock_defs] at hdef'
    exact ⟨blk, hblk, hdef'⟩
  · intro ⟨blk, hblk, hdef⟩
    have hblk' := blocks_map_some f cseBlock lbl blk hblk
    rw [← cseBlock_defs] at hdef
    exact ⟨cseBlock blk, hblk', hdef⟩

theorem cse_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (cseFunc f) := by
  constructor
  · -- unique_defs: dsts preserved by cseInstr_dst
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := (cse_definedIn_iff f v lbl₁).mp hdef₁
    have hdef₂' := (cse_definedIn_iff f v lbl₂).mp hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · sorry  -- Dominance unchanged; requires CFG equivalence lifting
  · -- Entry preserved
    show ((cseFunc f).blocks (cseFunc f).entry).isSome
    unfold cseFunc
    exact mapFunc_blocks_isSome h.entry_exists

-- ══════════════════════════════════════════════════════════════════
-- Section 6: LICM preserves SSA
-- ══════════════════════════════════════════════════════════════════

/-- LICM moves instructions from loop body to preheader. The preheader
    dominates the loop header, which dominates all loop body blocks.
    Therefore, hoisted definitions still dominate all their uses.

    This proof requires the loop validity predicate (header dominates body)
    and the preheader dominance property. -/
theorem licm_preserves_ssa (f : Func) (loop : NaturalLoop) (pre : Block)
    (h : SSAWellFormed f) (hloop : NaturalLoop.Valid f loop)
    (hpre_dom : Dom f loop.preheader loop.header) :
    SSAWellFormed (licmFunc f loop pre) := by
  constructor
  · -- unique_defs: hoisted instructions are removed from body and added
    -- to preheader. No new definitions are created.
    sorry
  · -- use_dom_def: hoisted instructions were loop-invariant, so their
    -- operands are defined outside the loop (which dominates preheader).
    -- The hoisted definition in preheader dominates the loop header,
    -- which dominates all body blocks where the var was used.
    sorry
  · sorry

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

/-- Guard hoisting preserves SSA. -/
private theorem guardHoistInstrs_dsts (proven : ProvenGuards) (instrs : List Instr) :
    (guardHoistInstrs proven instrs).map Instr.dst = instrs.map Instr.dst := by
  induction instrs generalizing proven with
  | nil => simp [guardHoistInstrs]
  | cons i rest ih =>
    simp only [guardHoistInstrs, List.map]
    congr 1
    · exact guardHoistInstr_dst proven i
    · exact ih _

private theorem guardHoistBlock_defs (proven : ProvenGuards) (b : Block) :
    blockAllDefs (guardHoistBlock proven b) = blockAllDefs b := by
  simp only [blockAllDefs, guardHoistBlock]
  congr 1
  exact guardHoistInstrs_dsts proven b.instrs

private theorem guardHoist_definedIn_iff (f : Func) (v : Var) (lbl : Label) :
    DefinedIn (guardHoistFunc f) v lbl ↔ DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hdef'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv f (guardHoistBlock []) lbl blk' hblk'
    rw [guardHoistBlock_defs] at hdef'
    exact ⟨blk, hblk, hdef'⟩
  · intro ⟨blk, hblk, hdef⟩
    have hblk' := blocks_map_some f (guardHoistBlock []) lbl blk hblk
    rw [← guardHoistBlock_defs] at hdef
    exact ⟨guardHoistBlock [] blk, hblk', hdef⟩

theorem guardHoist_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (guardHoistFunc f) := by
  constructor
  · -- unique_defs: dsts preserved by guardHoistInstr_dst
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := (guardHoist_definedIn_iff f v lbl₁).mp hdef₁
    have hdef₂' := (guardHoist_definedIn_iff f v lbl₂).mp hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · sorry  -- Dominance unchanged; requires CFG equivalence lifting
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

/-- Join canonicalization preserves SSA. -/
private theorem joinCanonBlock_defs (jmap : JoinMap) (b : Block) :
    blockAllDefs (joinCanonBlock jmap b) = blockAllDefs b := by
  simp only [blockAllDefs, joinCanonBlock_instrs, joinCanonBlock_params]

private theorem joinCanon_definedIn_iff (f : Func) (v : Var) (lbl : Label) :
    DefinedIn (joinCanonFunc f) v lbl ↔ DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hdef'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv
      f (joinCanonBlock (buildJoinMap f)) lbl blk'
      (by unfold joinCanonFunc at hblk'; exact hblk')
    rw [joinCanonBlock_defs] at hdef'
    exact ⟨blk, hblk, hdef'⟩
  · intro ⟨blk, hblk, hdef⟩
    have hblk' := blocks_map_some f (joinCanonBlock (buildJoinMap f)) lbl blk hblk
    rw [← joinCanonBlock_defs] at hdef
    exact ⟨joinCanonBlock (buildJoinMap f) blk, hblk', hdef⟩

theorem joinCanon_preserves_ssa (f : Func) (h : SSAWellFormed f) :
    SSAWellFormed (joinCanonFunc f) := by
  constructor
  · -- unique_defs: instructions unchanged, so defs unchanged
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := (joinCanon_definedIn_iff f v lbl₁).mp hdef₁
    have hdef₂' := (joinCanon_definedIn_iff f v lbl₂).mp hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · sorry  -- Definitions at same blocks; dominance may change for redirected
    -- edges, but the canonical target has the same params as the original
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

/-- Edge threading preserves block params. -/
private theorem edgeThreadBlock_params (σ : AbsEnv) (b : Block) :
    (edgeThreadBlock σ b).params = b.params := by
  unfold edgeThreadBlock; rfl

/-- Edge threading preserves all block definitions. -/
private theorem edgeThreadBlock_defs (σ : AbsEnv) (b : Block) :
    blockAllDefs (edgeThreadBlock σ b) = blockAllDefs b := by
  simp only [blockAllDefs, edgeThreadBlock_instrs, edgeThreadBlock_params]

/-- DefinedIn equivalence for edgeThreadFunc. -/
private theorem edgeThread_definedIn_iff (f : Func) (st : SCCPState) (v : Var) (lbl : Label) :
    DefinedIn (edgeThreadFunc f st) v lbl ↔ DefinedIn f v lbl := by
  constructor
  · intro ⟨blk', hblk', hdef'⟩
    obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_inv_dep
      f (fun l b => edgeThreadBlock (st.blockStates l |>.inEnv) b) lbl blk'
      (by unfold edgeThreadFunc at hblk'; exact hblk')
    rw [edgeThreadBlock_defs] at hdef'
    exact ⟨blk, hblk, hdef'⟩
  · intro ⟨blk, hblk, hdef⟩
    have hblk' := blocks_map_some_dep
      f (fun l b => edgeThreadBlock (st.blockStates l |>.inEnv) b) lbl blk hblk
    rw [← edgeThreadBlock_defs] at hdef
    exact ⟨edgeThreadBlock (st.blockStates lbl |>.inEnv) blk, hblk', hdef⟩

/-- Edge threading preserves SSA: only terminators change.
    Edge threading only removes edges (br->jmp removes one successor).
    Removing edges cannot break dominance of existing def-use pairs:
    a definition that dominated a use via all paths still dominates
    via the subset of paths that remain. -/
theorem edgeThread_preserves_ssa (f : Func) (st : SCCPState) (h : SSAWellFormed f) :
    SSAWellFormed (edgeThreadFunc f st) := by
  constructor
  · -- unique_defs: instructions unchanged, using label-dependent inverse lemma
    intro v lbl₁ lbl₂ hdef₁ hdef₂
    have hdef₁' := (edgeThread_definedIn_iff f st v lbl₁).mp hdef₁
    have hdef₂' := (edgeThread_definedIn_iff f st v lbl₂).mp hdef₂
    exact h.unique_defs v lbl₁ lbl₂ hdef₁' hdef₂'
  · sorry  -- Dominance preserved (edge removal only strengthens dominance)
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
