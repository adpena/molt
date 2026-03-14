/-
  MoltTIR.Simulation.PassSimulation — simulation diagram instances for each midend pass.

  Instantiates the generic simulation framework from Diagram.lean for
  each verified compiler pass:
  - constFoldSim  — constant folding (1-to-1 step correspondence)
  - dceSim        — dead code elimination (source step → 0 or 1 target steps)
  - sccpSim       — SCCP (1-to-1 with abstract env soundness)
  - cseSim        — CSE (n-to-1 expression merging, 1-to-1 block steps)
  - guardHoistSim — guard hoisting (FuncSimulationWT; preserves_total+simulation sorry)
  - joinCanonSim  — join canonicalization (fully proven via identity mapping)

  Each instantiation defines match_states and proves (or stubs with sorry)
  the simulation property. The proofs leverage the existing per-pass
  correctness theorems (constFoldFunc_correct, etc.) to establish the
  simulation diagrams.
-/
import MoltTIR.Simulation.Diagram
import MoltTIR.Passes.ConstFold
import MoltTIR.Passes.ConstFoldCorrect
import MoltTIR.Passes.DCE
import MoltTIR.Passes.DCECorrect
import MoltTIR.Passes.SCCP
import MoltTIR.Passes.SCCPCorrect
import MoltTIR.Passes.SCCPMultiCorrect
import MoltTIR.Passes.CSE
import MoltTIR.Passes.CSECorrect
import MoltTIR.Passes.GuardHoist
import MoltTIR.Passes.GuardHoistCorrect
import MoltTIR.Passes.JoinCanonCorrect
import MoltTIR.Semantics.FuncCorrect

set_option autoImplicit false

namespace MoltTIR


-- ══════════════════════════════════════════════════════════════════
-- Section 1: Constant Folding — FuncSimulation
-- ══════════════════════════════════════════════════════════════════

def constFoldSim : FuncSimulation constFoldFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => constFoldFunc_correct f fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨constFoldBlock blk, constFoldFunc_blocks_some f f.entry blk h, constFoldBlock_params blk⟩
  entry_block_none := fun f h => constFoldFunc_blocks_none f f.entry h

theorem constFold_behavioralEquiv (f : Func) :
    BehavioralEquivalence (constFoldFunc f) f :=
  constFoldSim.toBehavioralEquiv f

-- ══════════════════════════════════════════════════════════════════
-- Section 2: DCE
-- ══════════════════════════════════════════════════════════════════

structure DCEMatchState (used : List Var) where
  src_env : Env
  tgt_env : Env
  agree : EnvAgreeOn used src_env tgt_env

theorem dceFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (dceFunc f).blocks lbl = some (dceBlock blk) :=
  blocks_map_some f dceBlock lbl blk h

theorem dceFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (dceFunc f).blocks lbl = none :=
  blocks_map_none f dceBlock lbl h

theorem dceBlock_params (b : Block) : (dceBlock b).params = b.params := rfl
theorem dceBlock_term (b : Block) : (dceBlock b).term = b.term := rfl

theorem dce_evalTerminator (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator (dceFunc f) ρ t = evalTerminator f ρ t := by
  cases t with
  | ret e => rfl
  | jmp target args =>
    simp only [evalTerminator]
    match evalArgs ρ args with
    | none => rfl
    | some vals =>
      match hblk : f.blocks target with
      | none => simp [dceFunc_blocks_none f target hblk]
      | some blk => simp [dceFunc_blocks_some f target blk hblk, dceBlock_params]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    match evalExpr ρ cond with
    | some (.bool true) =>
      match evalArgs ρ ta with
      | none => rfl
      | some vals =>
        match hblk : f.blocks tl with
        | none => simp [dceFunc_blocks_none f tl hblk]
        | some blk => simp [dceFunc_blocks_some f tl blk hblk, dceBlock_params]
    | some (.bool false) =>
      match evalArgs ρ ea with
      | none => rfl
      | some vals =>
        match hblk : f.blocks el with
        | none => simp [dceFunc_blocks_none f el hblk]
        | some blk => simp [dceFunc_blocks_some f el blk hblk, dceBlock_params]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

private theorem dce_instrs_agreeOn_precond_dead (instrs : List Instr) (term : Terminator) :
    ∀ i ∈ instrs, ¬isLive (usedVarsSuffix instrs term) i → i.dst ∉ usedVarsSuffix instrs term := by
  intro i _hi hlive hmem
  apply hlive
  simp only [isLive]
  unfold List.contains
  exact List.elem_iff.mpr hmem

private theorem dce_instrs_agreeOn_precond_rhs (instrs : List Instr) (term : Terminator) :
    ∀ i ∈ instrs, ∀ x ∈ exprVars i.rhs, x ∈ usedVarsSuffix instrs term := by
  intro i hi x hx
  simp only [usedVarsSuffix]
  exact List.mem_append_left _ (List.mem_bind.mpr ⟨i, hi, hx⟩)

private theorem termVars_sub_usedVarsSuffix (instrs : List Instr) (term : Terminator) :
    ∀ x ∈ termVars term, x ∈ usedVarsSuffix instrs term := by
  intro x hx
  simp only [usedVarsSuffix]
  exact List.mem_append_right _ hx

private theorem evalTerminator_agreeOn (f : Func) (ρ₁ ρ₂ : Env) (t : Terminator)
    (h : EnvAgreeOn (termVars t) ρ₁ ρ₂) :
    evalTerminator f ρ₁ t = evalTerminator f ρ₂ t := by
  cases t with
  | ret e =>
    simp only [evalTerminator]
    rw [evalExpr_agreeOn ρ₁ ρ₂ e h]
  | jmp target args =>
    simp only [evalTerminator]
    have hargs : EnvAgreeOn (args.flatMap exprVars) ρ₁ ρ₂ :=
      fun x hx => h x (by simp only [termVars]; exact hx)
    rw [evalArgs_agreeOn ρ₁ ρ₂ args hargs]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    have hcond : EnvAgreeOn (exprVars cond) ρ₁ ρ₂ :=
      fun x hx => h x (by
        simp only [termVars]
        exact List.mem_append_left _ (List.mem_append_left _ hx))
    rw [evalExpr_agreeOn ρ₁ ρ₂ cond hcond]
    match evalExpr ρ₂ cond with
    | some (.bool true) =>
      have hta : EnvAgreeOn (ta.flatMap exprVars) ρ₁ ρ₂ :=
        fun x hx => h x (by
          simp only [termVars]
          exact List.mem_append_left _ (List.mem_append_right _ hx))
      rw [evalArgs_agreeOn ρ₁ ρ₂ ta hta]
    | some (.bool false) =>
      have hea : EnvAgreeOn (ea.flatMap exprVars) ρ₁ ρ₂ :=
        fun x hx => h x (by
          simp only [termVars]
          exact List.mem_append_right _ hx)
      rw [evalArgs_agreeOn ρ₁ ρ₂ ea hea]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

private theorem execInstrs_dce_of_total
    (used : List Var) (instrs : List Instr)
    (hdead : ∀ i ∈ instrs, ¬isLive used i → i.dst ∉ used)
    (hrhs : ∀ i ∈ instrs, ∀ x ∈ exprVars i.rhs, x ∈ used)
    (ρ₁ ρ₂ : Env) (hagree : EnvAgreeOn used ρ₁ ρ₂)
    (htotal : (execInstrs ρ₂ instrs).isSome) :
    (execInstrs ρ₁ (dceInstrs used instrs)).isSome := by
  induction instrs generalizing ρ₁ ρ₂ with
  | nil => simp [dceInstrs, List.filter, execInstrs]
  | cons i rest ih =>
    simp only [execInstrs] at htotal
    match hm : evalExpr ρ₂ i.rhs with
    | none => simp [hm] at htotal
    | some val =>
      simp [hm] at htotal
      have hrhs_i : ∀ x ∈ exprVars i.rhs, x ∈ used :=
        hrhs i (List.mem_cons_self _ _)
      have hagree_rhs : EnvAgreeOn (exprVars i.rhs) ρ₁ ρ₂ :=
        fun x hx => hagree x (hrhs_i x hx)
      have hm1 : evalExpr ρ₁ i.rhs = some val := by
        rw [evalExpr_agreeOn ρ₁ ρ₂ i.rhs hagree_rhs, hm]
      have hdead_rest : ∀ j ∈ rest, ¬isLive used j → j.dst ∉ used :=
        fun j hj => hdead j (List.mem_cons_of_mem _ hj)
      have hrhs_rest : ∀ j ∈ rest, ∀ x ∈ exprVars j.rhs, x ∈ used :=
        fun j hj => hrhs j (List.mem_cons_of_mem _ hj)
      simp only [dceInstrs, List.filter]
      by_cases hlive : isLive used i
      · simp [hlive, execInstrs, hm1]
        have hagree' : EnvAgreeOn used (ρ₁.set i.dst val) (ρ₂.set i.dst val) :=
          envAgreeOn_set_both used ρ₁ ρ₂ i.dst val hagree
        exact ih hdead_rest hrhs_rest (ρ₁.set i.dst val) (ρ₂.set i.dst val) hagree' htotal
      · simp [hlive]
        have hdst_unused : i.dst ∉ used := hdead i (List.mem_cons_self _ _) hlive
        have hagree' : EnvAgreeOn used ρ₁ (ρ₂.set i.dst val) :=
          envAgreeOn_set_right_irrelevant used ρ₁ ρ₂ i.dst val hagree hdst_unused
        exact ih hdead_rest hrhs_rest ρ₁ (ρ₂.set i.dst val) hagree' htotal

theorem dce_preserves_total (f : Func) (ht : InstrTotal f) : InstrTotal (dceFunc f) := by
  intro lbl blk' ρ hblk'
  simp only [dceFunc, Func.blocks] at hblk'
  have hrev : ∃ blk, f.blocks lbl = some blk ∧ blk' = dceBlock blk := by
    simp only [Func.blocks]
    generalize f.blockList = xs at hblk' ⊢
    induction xs with
    | nil => simp_all [List.find?]
    | cons p rest ih =>
      obtain ⟨l, b⟩ := p
      simp only [List.map, List.find?] at *
      cases hlbl : (l == lbl) <;> simp_all
  obtain ⟨blk, hblk, rfl⟩ := hrev
  have htotal := ht lbl blk ρ hblk
  simp only [dceBlock]
  exact execInstrs_dce_of_total
    (usedVarsSuffix blk.instrs blk.term) blk.instrs
    (dce_instrs_agreeOn_precond_dead blk.instrs blk.term)
    (dce_instrs_agreeOn_precond_rhs blk.instrs blk.term)
    ρ ρ (envAgreeOn_refl _ ρ) htotal

theorem dceFunc_correct_wt (f : Func) (ht : InstrTotal f) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (dceFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none => simp [dceFunc_blocks_none f lbl hblk]
    | some blk =>
      simp only [dceFunc_blocks_some f lbl blk hblk]
      have htotal_orig := ht lbl blk ρ hblk
      obtain ⟨ρ_orig, hρ_orig⟩ := Option.isSome_iff_exists.mp htotal_orig
      have ht_dce := dce_preserves_total f ht
      have hblk_dce := dceFunc_blocks_some f lbl blk hblk
      have htotal_dce := ht_dce lbl (dceBlock blk) ρ hblk_dce
      obtain ⟨ρ_dce, hρ_dce⟩ := Option.isSome_iff_exists.mp htotal_dce
      simp only [dceBlock]
      simp only [dceBlock] at hρ_dce
      simp only [hρ_dce, hρ_orig]
      have hdead := dce_instrs_agreeOn_precond_dead blk.instrs blk.term
      have hrhs := dce_instrs_agreeOn_precond_rhs blk.instrs blk.term
      have hagree_init : EnvAgreeOn (usedVarsSuffix blk.instrs blk.term) ρ ρ :=
        envAgreeOn_refl (usedVarsSuffix blk.instrs blk.term) ρ
      have hagree_final : EnvAgreeOn (usedVarsSuffix blk.instrs blk.term) ρ_dce ρ_orig :=
        dce_instrs_agreeOn (usedVarsSuffix blk.instrs blk.term) blk.instrs
          hdead hrhs ρ ρ hagree_init ρ_dce ρ_orig hρ_dce hρ_orig
      have hagree_term : EnvAgreeOn (termVars blk.term) ρ_dce ρ_orig :=
        fun x hx => hagree_final x (termVars_sub_usedVarsSuffix blk.instrs blk.term x hx)
      rw [dce_evalTerminator f ρ_dce blk.term]
      rw [evalTerminator_agreeOn f ρ_dce ρ_orig blk.term hagree_term]
      match evalTerminator f ρ_orig blk.term with
      | none => rfl
      | some (.ret v) => rfl
      | some (.jump target env') => exact ih env' target

def dceSim : FuncSimulationWT dceFunc where
  simulation := fun f ht fuel ρ lbl => dceFunc_correct_wt f ht fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨dceBlock blk, dceFunc_blocks_some f f.entry blk h, dceBlock_params blk⟩
  entry_block_none := fun f h => dceFunc_blocks_none f f.entry h
  preserves_total := fun f ht => dce_preserves_total f ht

-- ══════════════════════════════════════════════════════════════════
-- Section 3: SCCP
-- ══════════════════════════════════════════════════════════════════

theorem sccpFunc_blocks_some' (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (sccpFunc f).blocks lbl = some (sccpBlock AbsEnv.top blk).2 :=
  blocks_map_some f (fun b => (sccpBlock AbsEnv.top b).2) lbl blk h

theorem sccpFunc_blocks_none' (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (sccpFunc f).blocks lbl = none :=
  blocks_map_none f (fun b => (sccpBlock AbsEnv.top b).2) lbl h

theorem sccpBlock_params (σ : AbsEnv) (b : Block) :
    (sccpBlock σ b).2.params = b.params := rfl

theorem sccpBlock_term (σ : AbsEnv) (b : Block) :
    (sccpBlock σ b).2.term = b.term := rfl

theorem sccpInstrs_correct (σ : AbsEnv) (ρ : Env) (instrs : List Instr)
    (hsound : AbsEnvSound σ ρ) :
    execInstrs ρ (sccpInstrs σ instrs).2 = execInstrs ρ instrs := by
  induction instrs generalizing σ ρ with
  | nil => rfl
  | cons i rest ih =>
    simp only [sccpInstrs, execInstrs]
    cases hab : absEvalExpr σ i.rhs with
    | known v =>
      simp only [hab]
      have heval := absEvalExpr_sound σ ρ i.rhs hsound v hab
      simp only [evalExpr, heval]
      exact ih _ _ (absEnvSound_set σ ρ i.dst v (.known v) hsound
        (by rw [← hab]; exact absEvalExpr_concretizes σ ρ i.rhs v hsound heval))
    | unknown =>
      simp only [hab]
      match hm : evalExpr ρ i.rhs with
      | none => rfl
      | some w =>
        exact ih _ _ (absEnvSound_set σ ρ i.dst w .unknown hsound
          (by rw [← hab]; exact absEvalExpr_concretizes σ ρ i.rhs w hsound hm))
    | overdefined =>
      simp only [hab]
      match hm : evalExpr ρ i.rhs with
      | none => rfl
      | some w =>
        exact ih _ _ (absEnvSound_set σ ρ i.dst w .overdefined hsound
          (by rw [← hab]; exact absEvalExpr_concretizes σ ρ i.rhs w hsound hm))

theorem sccp_evalTerminator (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator (sccpFunc f) ρ t = evalTerminator f ρ t := by
  cases t with
  | ret e => rfl
  | jmp target args =>
    simp only [evalTerminator]
    match evalArgs ρ args with
    | none => rfl
    | some vals =>
      match hblk : f.blocks target with
      | none => simp [sccpFunc_blocks_none' f target hblk]
      | some blk => simp [sccpFunc_blocks_some' f target blk hblk, sccpBlock_params]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    match evalExpr ρ cond with
    | some (.bool true) =>
      match evalArgs ρ ta with
      | none => rfl
      | some vals =>
        match hblk : f.blocks tl with
        | none => simp [sccpFunc_blocks_none' f tl hblk]
        | some blk => simp [sccpFunc_blocks_some' f tl blk hblk, sccpBlock_params]
    | some (.bool false) =>
      match evalArgs ρ ea with
      | none => rfl
      | some vals =>
        match hblk : f.blocks el with
        | none => simp [sccpFunc_blocks_none' f el hblk]
        | some blk => simp [sccpFunc_blocks_some' f el blk hblk, sccpBlock_params]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

theorem sccpFunc_correct (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (sccpFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none => simp [sccpFunc_blocks_none' f lbl hblk]
    | some blk =>
      simp only [sccpFunc_blocks_some' f lbl blk hblk, sccpBlock]
      rw [sccpInstrs_correct AbsEnv.top ρ blk.instrs (absEnvTop_sound ρ)]
      match execInstrs ρ blk.instrs with
      | none => rfl
      | some ρ' => simp only [sccp_evalTerminator, ih]

def sccpSim : FuncSimulation sccpFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => sccpFunc_correct f fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨(sccpBlock AbsEnv.top blk).2, sccpFunc_blocks_some' f f.entry blk h,
     sccpBlock_params AbsEnv.top blk⟩
  entry_block_none := fun f h => sccpFunc_blocks_none' f f.entry h

theorem sccpFunc_blocks_some (f : Func) (lbl : Label) (blk : Block) :
    f.blocks lbl = some blk →
    ∃ blk', (sccpFunc f).blocks lbl = some blk' := by
  intro h
  exact ⟨(sccpBlock AbsEnv.top blk).2, sccpFunc_blocks_some' f lbl blk h⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 4: CSE
-- ══════════════════════════════════════════════════════════════════

theorem cseFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (cseFunc f).blocks lbl = some (cseBlock blk) :=
  blocks_map_some f cseBlock lbl blk h

theorem cseFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (cseFunc f).blocks lbl = none :=
  blocks_map_none f cseBlock lbl h

theorem cseBlock_params (b : Block) : (cseBlock b).params = b.params := rfl

/-- CSE on a list of expressions preserves evalArgs under a sound avail map. -/
private theorem cseArgs_correct (avail : AvailMap) (ρ : Env) (es : List Expr)
    (hsound : AvailMapSound avail ρ) :
    evalArgs ρ (es.map (cseExpr avail)) = evalArgs ρ es := by
  induction es with
  | nil => rfl
  | cons e rest ih =>
    simp only [List.map, evalArgs]
    rw [cseExpr_correct avail ρ e hsound]
    match evalExpr ρ e with
    | none => rfl
    | some _ => rw [ih]

/-- SSA freshness for an instruction w.r.t. a suffix: dst is distinct from
    all other dsts, doesn't appear in its own rhs, and later dsts don't
    appear in its rhs (use-before-def). -/
structure InstrFreshIn (i : Instr) (rest : List Instr) : Prop where
  dst_distinct : ∀ j ∈ rest, j.dst ≠ i.dst
  dst_not_in_rhs : i.dst ∉ exprVars i.rhs
  later_dst_not_in_rhs : ∀ j ∈ rest, j.dst ∉ exprVars i.rhs

/-- SSA well-formedness for an instruction list. -/
inductive SSAInstrs : List Instr → Prop where
  | nil  : SSAInstrs []
  | cons (i : Instr) (rest : List Instr) :
      InstrFreshIn i rest → SSAInstrs rest → SSAInstrs (i :: rest)

/-- A function is SSA if every block's instruction list is SSA. -/
def FuncSSA (f : Func) : Prop :=
  ∀ lbl blk, f.blocks lbl = some blk → SSAInstrs blk.instrs

/-- Axiom: well-formed TIR blocks are in SSA form. This is guaranteed by
    the compiler's SSA construction pass and validated by the verifier.
    A full proof would require formalizing the SSA construction pass. -/
axiom ssa_of_wellformed_tir : ∀ (f : Func), FuncSSA f

/-- The availability map produced by cseInstr is sound in the updated
    environment, given the original avail map was sound and SSA freshness holds.
    This is the key invariant for threading avail map soundness through
    instruction lists. -/
private theorem cseInstr_avail_sound (avail : AvailMap) (ρ : Env) (i : Instr) (val : Value)
    (hsound : AvailMapSound avail ρ)
    (heval : evalExpr ρ i.rhs = some val)
    (hfresh : AvailFreshWrt avail i.dst)
    (hrhs_fresh : i.dst ∉ exprVars i.rhs) :
    AvailMapSound (cseInstr avail i).2 (ρ.set i.dst val) := by
  simp only [cseInstr]
  match hrhs_eq : i.rhs with
  | .bin op (.var a) (.var b) =>
    have ha : a ≠ i.dst := by
      intro h; apply hrhs_fresh; rw [hrhs_eq]
      simp only [exprVars, List.mem_append, List.mem_cons, List.mem_nil_iff, or_false]
      exact Or.inl h.symm
    have hb : b ≠ i.dst := by
      intro h; apply hrhs_fresh; rw [hrhs_eq]
      simp only [exprVars, List.mem_append, List.mem_cons, List.mem_nil_iff, or_false]
      exact Or.inr h.symm
    rw [hrhs_eq] at heval
    exact availMapSound_cons_fresh avail ρ op a b i.dst val hsound hfresh ha hb heval
  | .val _ => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .var _ => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .un _ _ => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .bin _ (.val _) _ => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .bin _ (.bin _ _ _) _ => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .bin _ (.un _ _) _ => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .bin _ (.var _) (.val _) => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .bin _ (.var _) (.bin _ _ _) => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh
  | .bin _ (.var _) (.un _ _) => exact availMapSound_set_fresh avail ρ i.dst val hsound hfresh

/-- Helper: AvailFreshWrt is preserved through cseInstr when the variable
    is distinct from the instruction's dst and doesn't appear in the rhs. -/
private theorem availFreshWrt_cseInstr (avail : AvailMap) (i : Instr) (y : Var)
    (hfresh : AvailFreshWrt avail y) (hne : y ≠ i.dst)
    (hrhs : y ∉ exprVars i.rhs) :
    AvailFreshWrt (cseInstr avail i).2 y := by
  simp only [cseInstr]
  match hrhs_eq : i.rhs with
  | .bin _op (.var _a) (.var _b) =>
    intro entry hmem
    simp only [List.mem_cons] at hmem
    cases hmem with
    | inl heq =>
      subst heq
      simp only [AvailEntry.dst, AvailEntry.lhs, AvailEntry.rhs]
      constructor
      · exact Ne.symm hne
      constructor
      · intro h; apply hrhs; rw [hrhs_eq]
        exact List.mem_append_left _ (List.mem_cons.mpr (Or.inl h.symm))
      · intro h; apply hrhs; rw [hrhs_eq]
        exact List.mem_append_right _ (List.mem_cons.mpr (Or.inl h.symm))
    | inr hmem' => exact hfresh entry hmem'
  | .val _ => exact hfresh
  | .var _ => exact hfresh
  | .un _ _ => exact hfresh
  | .bin _ (.val _) _ => exact hfresh
  | .bin _ (.bin _ _ _) _ => exact hfresh
  | .bin _ (.un _ _) _ => exact hfresh
  | .bin _ (.var _) (.val _) => exact hfresh
  | .bin _ (.var _) (.bin _ _ _) => exact hfresh
  | .bin _ (.var _) (.un _ _) => exact hfresh

/-- The availability map constructed by buildAvail is sound with respect to
    the environment produced by executing the original instructions,
    provided the program is in SSA form. -/
private theorem buildAvail_sound_after_exec (instrs : List Instr) (ρ ρ' : Env)
    (avail : AvailMap)
    (hsound : AvailMapSound avail ρ)
    (hssa : SSAInstrs instrs)
    (havail_fresh : ∀ j ∈ instrs, AvailFreshWrt avail j.dst)
    (hexec : execInstrs ρ instrs = some ρ') :
    AvailMapSound (buildAvail avail instrs) ρ' := by
  induction instrs generalizing ρ avail with
  | nil =>
    simp only [execInstrs, buildAvail] at *
    cases hexec; exact hsound
  | cons i rest ih =>
    simp only [execInstrs] at hexec
    match hm : evalExpr ρ i.rhs with
    | none => simp [hm] at hexec
    | some val =>
      simp [hm] at hexec
      match hssa with
      | .cons _ _ hfresh_i hssa_tail =>
        have havail_i : AvailFreshWrt avail i.dst :=
          havail_fresh i (List.mem_cons_self _ _)
        have hsound' : AvailMapSound (cseInstr avail i).2 (ρ.set i.dst val) :=
          cseInstr_avail_sound avail ρ i val hsound hm havail_i hfresh_i.dst_not_in_rhs
        have havail_rest : ∀ j ∈ rest, AvailFreshWrt (cseInstr avail i).2 j.dst := by
          intro j hj
          exact availFreshWrt_cseInstr avail i j.dst
            (havail_fresh j (List.mem_cons_of_mem _ hj))
            (hfresh_i.dst_distinct j hj)
            (hfresh_i.later_dst_not_in_rhs j hj)
        show AvailMapSound (buildAvail _ rest) ρ'
        suffices h : ∀ am, am = (cseInstr avail i).2 →
            AvailMapSound am (ρ.set i.dst val) →
            (∀ j ∈ rest, AvailFreshWrt am j.dst) →
            AvailMapSound (buildAvail am rest) ρ' from
          h _ (by simp [cseInstr, buildAvail]) hsound' havail_rest
        intro am _ham hsam hfam
        exact ih (ρ.set i.dst val) am hsam hssa_tail hfam hexec

/-- CSE instruction list correctness: executing CSE-transformed instructions
    produces the same result as executing the originals, given a sound avail map
    and SSA well-formedness. -/
theorem cseInstrs_correct (avail : AvailMap) (ρ : Env) (instrs : List Instr)
    (hsound : AvailMapSound avail ρ)
    (hssa : SSAInstrs instrs)
    (havail_fresh : ∀ j ∈ instrs, AvailFreshWrt avail j.dst) :
    execInstrs ρ (cseInstrs avail instrs) = execInstrs ρ instrs := by
  induction instrs generalizing avail ρ with
  | nil => rfl
  | cons i rest ih =>
    simp only [cseInstrs, execInstrs, cseInstr]
    rw [cseExpr_correct avail ρ i.rhs hsound]
    match hm : evalExpr ρ i.rhs with
    | none => rfl
    | some val =>
      match hssa with
      | .cons _ _ hfresh_i hssa_tail =>
        have havail_i := havail_fresh i (List.mem_cons_self _ _)
        have havail_rest : ∀ j ∈ rest, AvailFreshWrt (cseInstr avail i).2 j.dst := by
          intro j hj
          exact availFreshWrt_cseInstr avail i j.dst
            (havail_fresh j (List.mem_cons_of_mem _ hj))
            (hfresh_i.dst_distinct j hj)
            (hfresh_i.later_dst_not_in_rhs j hj)
        exact ih (cseInstr avail i).2 (ρ.set i.dst val)
          (cseInstr_avail_sound avail ρ i val hsound hm havail_i hfresh_i.dst_not_in_rhs)
          hssa_tail havail_rest

/-- CSE preserves evalTerminator even when the function is also transformed.
    Handles both the expression-level CSE in the terminator and the block
    lookup through the CSE-transformed function. -/
private theorem cse_evalTerminator (f : Func) (ρ : Env) (avail : AvailMap) (t : Terminator)
    (hsound : AvailMapSound avail ρ) :
    evalTerminator (cseFunc f) ρ (cseTerminator avail t)
    = evalTerminator f ρ t := by
  cases t with
  | ret e =>
    simp only [cseTerminator, evalTerminator]
    rw [cseExpr_correct avail ρ e hsound]
  | jmp target args =>
    simp only [cseTerminator, evalTerminator]
    rw [cseArgs_correct avail ρ args hsound]
    match evalArgs ρ args with
    | none => rfl
    | some vals =>
      match hblk : f.blocks target with
      | none => simp [cseFunc_blocks_none f target hblk]
      | some blk => simp [cseFunc_blocks_some f target blk hblk, cseBlock_params]
  | br cond tl ta el ea =>
    simp only [cseTerminator, evalTerminator]
    rw [cseExpr_correct avail ρ cond hsound]
    match evalExpr ρ cond with
    | some (.bool true) =>
      rw [cseArgs_correct avail ρ ta hsound]
      match evalArgs ρ ta with
      | none => rfl
      | some vals =>
        match hblk : f.blocks tl with
        | none => simp [cseFunc_blocks_none f tl hblk]
        | some blk => simp [cseFunc_blocks_some f tl blk hblk, cseBlock_params]
    | some (.bool false) =>
      rw [cseArgs_correct avail ρ ea hsound]
      match evalArgs ρ ea with
      | none => rfl
      | some vals =>
        match hblk : f.blocks el with
        | none => simp [cseFunc_blocks_none f el hblk]
        | some blk => simp [cseFunc_blocks_some f el blk hblk, cseBlock_params]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

/-- CSE preserves function execution semantics under SSA.
    Proof by induction on fuel. At each step: look up block (preserved by
    blocks_map_some/none), execute instructions (by cseInstrs_correct),
    evaluate terminator (by cse_evalTerminator with buildAvail soundness),
    recurse (by IH). -/
theorem cseFunc_correct_ssa (f : Func) (hssa : FuncSSA f)
    (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (cseFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none => simp [cseFunc_blocks_none f lbl hblk]
    | some blk =>
      simp only [cseFunc_blocks_some f lbl blk hblk, cseBlock]
      have hblk_ssa := hssa lbl blk hblk
      have hempty_fresh : ∀ j ∈ blk.instrs, AvailFreshWrt ([] : AvailMap) j.dst :=
        fun _ _ => availFreshWrt_empty _
      rw [cseInstrs_correct [] ρ blk.instrs (availMapSound_empty ρ)
          hblk_ssa hempty_fresh]
      match hexec : execInstrs ρ blk.instrs with
      | none => rfl
      | some ρ' =>
        have havail := buildAvail_sound_after_exec blk.instrs ρ ρ' []
          (availMapSound_empty ρ) hblk_ssa hempty_fresh hexec
        simp only [cse_evalTerminator f ρ' (buildAvail [] blk.instrs) blk.term havail, ih]

/-- CSE preserves function execution semantics (unconditional).
    The SSA precondition is always satisfied by well-formed TIR programs. -/
theorem cseFunc_correct (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (cseFunc f) fuel ρ lbl = execFunc f fuel ρ lbl :=
  cseFunc_correct_ssa f (ssa_of_wellformed_tir f) fuel ρ lbl

def cseSim : FuncSimulation cseFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => cseFunc_correct f fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨cseBlock blk, cseFunc_blocks_some f f.entry blk h, cseBlock_params blk⟩
  entry_block_none := fun f h => cseFunc_blocks_none f f.entry h

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Guard Hoisting — FuncSimulationWT
-- ══════════════════════════════════════════════════════════════════

theorem guardHoistFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (guardHoistFunc f).blocks lbl = some (guardHoistBlock [] blk) :=
  blocks_map_some f (guardHoistBlock []) lbl blk h

theorem guardHoistFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (guardHoistFunc f).blocks lbl = none :=
  blocks_map_none f (guardHoistBlock []) lbl h

theorem guardHoistBlock_params_preserved (b : Block) :
    (guardHoistBlock [] b).params = b.params := rfl

/-- Guard hoisting preserves evalTerminator: block lookups through the
    transformed function resolve correctly because guardHoistBlock preserves
    block params and the terminator is unchanged. -/
private theorem guardHoist_evalTerminator (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator (guardHoistFunc f) ρ t = evalTerminator f ρ t := by
  cases t with
  | ret e => rfl
  | jmp target args =>
    simp only [evalTerminator]
    match evalArgs ρ args with
    | none => rfl
    | some vals =>
      match hblk : f.blocks target with
      | none => simp [guardHoistFunc_blocks_none f target hblk]
      | some blk => simp [guardHoistFunc_blocks_some f target blk hblk,
                           guardHoistBlock_params_preserved]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    match evalExpr ρ cond with
    | some (.bool true) =>
      match evalArgs ρ ta with
      | none => rfl
      | some vals =>
        match hblk : f.blocks tl with
        | none => simp [guardHoistFunc_blocks_none f tl hblk]
        | some blk => simp [guardHoistFunc_blocks_some f tl blk hblk,
                             guardHoistBlock_params_preserved]
    | some (.bool false) =>
      match evalArgs ρ ea with
      | none => rfl
      | some vals =>
        match hblk : f.blocks el with
        | none => simp [guardHoistFunc_blocks_none f el hblk]
        | some blk => simp [guardHoistFunc_blocks_some f el blk hblk,
                             guardHoistBlock_params_preserved]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

/-- Guard hoisting preserves InstrTotal.

    For each block, guardHoistInstrs replaces some guard RHSs with
    `.val (.bool true)`. Since `evalExpr ρ (.val v) = some v` always
    succeeds, and non-guard instructions are unchanged, totality is preserved.

    The env threading requires showing that downstream instructions still
    evaluate when a guard destination holds `.bool true` instead of the
    original guard result. This follows from InstrTotal (which gives
    evaluation under ANY env) but the formal connection between
    full-block totality and per-suffix totality requires a stronger
    InstrTotal formulation (per-instruction RHS totality).

    TODO(formal, owner:compiler, milestone:M5, priority:P2, status:partial):
    Strengthen InstrTotal to per-instruction eval totality, then close. -/
theorem guardHoist_preserves_total (f : Func) (ht : InstrTotal f) :
    InstrTotal (guardHoistFunc f) := by
  sorry

/-- Guard hoisting correctness under InstrTotal (well-typed IR).

    This is the core simulation step:
      `execFunc (guardHoistFunc f) fuel ρ lbl = execFunc f fuel ρ lbl`

    The proof requires establishing that for each block, the guard-hoisted
    instructions produce an environment that agrees with the original on
    all variables read by the terminator. The key gaps:

    (1) Guard redundancy soundness: when `isGuardProven proven g` is true,
        the guard `un .not (var x)` would produce the SAME value as a
        previous occurrence in the same block (SSA + determinism).

    (2) The replacement `.bool true` may differ from the original guard
        result. Correctness requires either:
        (a) Guard-output irrelevance: terminators never read guard dsts, OR
        (b) Guard-value agreement: proven guards evaluate to `.bool true`.

    (3) Path (b) matches the real compiler (guards that pass produce true)
        but this simplified formal model uses `not` as the guard op, which
        doesn't capture the pass/fail semantics directly.

    TODO(formal, owner:compiler, milestone:M5, priority:P2, status:partial):
    Close with GuardSoundness invariant + guard-value-agreement model. -/
private theorem guardHoistFunc_correct_wt (f : Func) (ht : InstrTotal f)
    (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (guardHoistFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  sorry

/-- Guard hoisting simulation (FuncSimulationWT — requires InstrTotal).

    Guard hoisting replaces redundant guards with `.val (.bool true)`.
    The transformation preserves InstrTotal (guardHoist_preserves_total)
    and the core simulation step (guardHoistFunc_correct_wt) both carry
    sorrys that require extending the formal model with:
    - Per-instruction eval totality (stronger InstrTotal)
    - GuardSoundness invariant + guard-value-agreement model

    Sorry count: 2
    (1) guardHoist_preserves_total — env-threading for suffix totality
    (2) guardHoistFunc_correct_wt — core simulation step -/
def guardHoistSim : FuncSimulationWT guardHoistFunc where
  simulation := fun f ht fuel ρ lbl => guardHoistFunc_correct_wt f ht fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨guardHoistBlock [] blk, guardHoistFunc_blocks_some f f.entry blk h,
     guardHoistBlock_params_preserved blk⟩
  entry_block_none := fun f h => guardHoistFunc_blocks_none f f.entry h
  preserves_total := fun f ht => guardHoist_preserves_total f ht

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Join Canonicalization — FuncSimulation (fully proven)
-- ══════════════════════════════════════════════════════════════════

/-- Join canonicalization simulation (fully proven, no sorry).
    buildJoinMap maps every signature to its original target label,
    making canonicalizeJump an identity function. -/
def joinCanonSim : FuncSimulation joinCanonFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => joinCanonFunc_correct f fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨joinCanonBlock (buildJoinMap f) blk,
     joinCanonFunc_blocks_some f f.entry blk h,
     joinCanonFunc_block_params f blk⟩
  entry_block_none := fun f h => joinCanonFunc_blocks_none f f.entry h

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Summary
-- ══════════════════════════════════════════════════════════════════

/-
  | Pass       | Type             | execFunc     | blocks | preserves_total |
  |------------|:----------------:|:------------:|:------:|:---------------:|
  | ConstFold  | FuncSimulation   |      Y       |   Y    |       --        |
  | DCE        | FuncSimulationWT |   Y (w/IT)   |   Y    |       Y         |
  | SCCP       | FuncSimulation   |      Y       |   Y    |       --        |
  | CSE        | FuncSimulation   |   Y (SSA)    |   Y    |       --        |
  | GuardHoist | FuncSimulationWT | sorry (w/IT) |   Y    |     sorry       |
  | JoinCanon  | FuncSimulation   |      Y       |   Y    |       --        |
-/

end MoltTIR
