/-
  MoltTIR.Simulation.Compose — simulation composition and full pipeline proof.

  Proves that simulations compose: if pass1 simulates source→mid and
  pass2 simulates mid→target, then their composition simulates source→target.

  Then instantiates for the full Molt midend pipeline:
    constFold ∘ sccp ∘ dce ∘ cse → BehavioralEquivalence
-/
import MoltTIR.Simulation.Diagram
import MoltTIR.Simulation.PassSimulation
import MoltTIR.Passes.Pipeline

set_option autoImplicit false

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Composing lock-step simulations
-- ══════════════════════════════════════════════════════════════════

/-- Compose two lock-step forward simulations.
    If sim1 : S → M and sim2 : M → T, then compose : S → T.
    The composed match_states is the relational composition:
    ∃ m, sim1.match_states s m ∧ sim2.match_states m t -/
def composeSimulations {S M T : Type}
    {step_s : S → S → Prop}
    {step_m : M → M → Prop}
    {step_t : T → T → Prop}
    (sim1 : ForwardSimulation S M step_s step_m)
    (sim2 : ForwardSimulation M T step_m step_t) :
    ForwardSimulation S T step_s step_t where
  match_states := fun s t => ∃ m, sim1.match_states s m ∧ sim2.match_states m t
  simulation := fun s1 s2 t1 hm hs => by
    obtain ⟨m1, hm1, hm2⟩ := hm
    obtain ⟨m2, hstep_m, hmatch_m⟩ := sim1.simulation s1 s2 m1 hm1 hs
    obtain ⟨t2, hstep_t, hmatch_t⟩ := sim2.simulation m1 m2 t1 hm2 hstep_m
    exact ⟨t2, hstep_t, m2, hmatch_m, hmatch_t⟩

/-- Compose two star simulations. -/
def composeSimulationsStar {S M T : Type}
    {step_s : S → S → Prop}
    {step_m : M → M → Prop}
    {step_t : T → T → Prop}
    (sim1 : ForwardSimulationStar S M step_s step_m)
    (sim2 : ForwardSimulationStar M T step_m step_t) :
    ForwardSimulationStar S T step_s step_t where
  match_states := fun s t => ∃ m, sim1.match_states s m ∧ sim2.match_states m t
  simulation := fun s1 s2 t1 hm hs => by
    obtain ⟨m1, hm1, hm2⟩ := hm
    obtain ⟨m2, hstar_m, hmatch_m⟩ := sim1.simulation s1 s2 m1 hm1 hs
    obtain ⟨t2, hstar_t, hmatch_t⟩ := sim2.star_simulation hm2 hstar_m
    exact ⟨t2, hstar_t, m2, hmatch_m, hmatch_t⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Composing FuncSimulations
-- ══════════════════════════════════════════════════════════════════

/-- Compose two FuncSimulations. If g1 and g2 each preserve execFunc,
    then g2 ∘ g1 preserves execFunc. -/
def composeFuncSimulations
    {g1 g2 : Func → Func}
    (sim1 : FuncSimulation g1)
    (sim2 : FuncSimulation g2) :
    FuncSimulation (g2 ∘ g1) where
  match_env := fun f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => by
    show execFunc (g2 (g1 f)) fuel ρ lbl = execFunc f fuel ρ lbl
    rw [sim2.simulation (g1 f) fuel ρ lbl, sim1.simulation f fuel ρ lbl]
  entry_preserved := fun f => by
    show (g2 (g1 f)).entry = f.entry
    rw [sim2.entry_preserved (g1 f), sim1.entry_preserved f]
  entry_block_some := fun f blk h => by
    obtain ⟨blk1, hblk1, hparams1⟩ := sim1.entry_block_some f blk h
    have hentry1 : (g1 f).entry = f.entry := sim1.entry_preserved f
    have hblk1' : (g1 f).blocks (g1 f).entry = some blk1 := by rw [hentry1]; exact hblk1
    obtain ⟨blk2, hblk2, hparams2⟩ := sim2.entry_block_some (g1 f) blk1 hblk1'
    -- hblk2 : (g2 (g1 f)).blocks (g1 f).entry = some blk2
    -- Need: (g2 (g1 f)).blocks f.entry = some blk2
    have hblk2' : (g2 (g1 f)).blocks f.entry = some blk2 := by rw [← hentry1]; exact hblk2
    exact ⟨blk2, hblk2', hparams2.trans hparams1⟩
  entry_block_none := fun f h => by
    have hentry1 : (g1 f).entry = f.entry := sim1.entry_preserved f
    have hnone1 : (g1 f).blocks f.entry = none := sim1.entry_block_none f h
    have hnone1' : (g1 f).blocks (g1 f).entry = none := hentry1 ▸ hnone1
    have hnone2 : (g2 (g1 f)).blocks (g2 (g1 f)).entry = none := by
      rw [sim2.entry_preserved (g1 f)]
      exact sim2.entry_block_none (g1 f) hnone1'
    have hentry2 : (g2 (g1 f)).entry = f.entry := by
      rw [sim2.entry_preserved (g1 f), sim1.entry_preserved f]
    rw [← hentry2]; exact hnone2

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Composing BehavioralEquivalence
-- ══════════════════════════════════════════════════════════════════

/-- If g preserves execFunc for all inputs, then g preserves runFunc. -/
theorem funcSimulation_to_behavioral {g : Func → Func}
    (sim : FuncSimulation g) (f : Func) :
    BehavioralEquivalence (g f) f :=
  sim.toBehavioralEquiv f

/-- Composition of behavioral equivalences. -/
theorem behavioral_equiv_compose {g1 g2 : Func → Func}
    (h1 : ∀ f, BehavioralEquivalence (g1 f) f)
    (h2 : ∀ f, BehavioralEquivalence (g2 f) f)
    (f : Func) :
    BehavioralEquivalence (g2 (g1 f)) f := by
  intro fuel
  have := h2 (g1 f) fuel
  have := h1 f fuel
  simp_all [runFunc, BehavioralEquivalence]

-- ══════════════════════════════════════════════════════════════════
-- Section 3b: InstrTotal preservation through constFold and SCCP
-- ══════════════════════════════════════════════════════════════════

/-- Reverse of blocks_map_some: if a mapped function's block lookup yields
    some blk', then there exists an original block blk with f.blocks lbl = some blk
    and blk' = g blk. -/
private theorem blocks_map_some_rev' (f : Func) (g : Block → Block) (lbl : Label)
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

/-- Constant folding preserves InstrTotal. constFoldExpr preserves evaluation
    (constFoldExpr_correct), so if every instruction evaluates in the original,
    every instruction evaluates in the folded version. -/
theorem constFold_preserves_total (f : Func) (ht : InstrTotal f) :
    InstrTotal (constFoldFunc f) := by
  intro lbl blk' ρ hblk'
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev' f constFoldBlock lbl blk' hblk'
  have h_orig := ht lbl blk ρ hblk
  simp only [constFoldBlock]
  rw [constFoldInstrs_correct ρ blk.instrs]
  exact h_orig

/-- SCCP preserves InstrTotal. sccpExpr either replaces the RHS with a literal
    (which always evaluates) or keeps the original (which evaluates by InstrTotal).
    In both cases, sccpInstrs_correct gives execInstrs equality. -/
theorem sccp_preserves_total (f : Func) (ht : InstrTotal f) :
    InstrTotal (sccpFunc f) := by
  intro lbl blk' ρ hblk'
  obtain ⟨blk, hblk, rfl⟩ :=
    blocks_map_some_rev' f (fun b => (sccpBlock AbsEnv.top b).2) lbl blk' hblk'
  have h_orig := ht lbl blk ρ hblk
  simp only [sccpBlock]
  rw [sccpInstrs_correct AbsEnv.top ρ blk.instrs (absEnvTop_strongSound ρ)]
  exact h_orig

/-- CSE preserves InstrTotal. cseInstrs_correct (under the SSA axiom) gives
    `execInstrs ρ (cseInstrs [] instrs) = execInstrs ρ instrs`, so if the
    original succeeds the CSE version does too. -/
theorem cse_preserves_total (f : Func) (ht : InstrTotal f) :
    InstrTotal (cseFunc f) := by
  intro lbl blk' ρ hblk'
  obtain ⟨blk, hblk, rfl⟩ := blocks_map_some_rev' f cseBlock lbl blk' hblk'
  have h_orig := ht lbl blk ρ hblk
  simp only [cseBlock]
  have hssa := ssa_of_wellformed_tir f lbl blk hblk
  have hempty_fresh : ∀ j ∈ blk.instrs, AvailFreshWrt ([] : AvailMap) j.dst :=
    fun _ _ => availFreshWrt_empty _
  rw [cseInstrs_correct [] ρ blk.instrs (availMapSound_empty ρ) hssa hempty_fresh]
  exact h_orig

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Full pipeline simulation
-- ══════════════════════════════════════════════════════════════════

/-- The composed FuncSimulation for constFold then SCCP. -/
def constFold_sccp_sim : FuncSimulation (sccpFunc ∘ constFoldFunc) :=
  composeFuncSimulations constFoldSim sccpSim

/-- Constant folding preserves behavioral equivalence (fully proven). -/
theorem constFold_pipeline_correct (f : Func) :
    BehavioralEquivalence (constFoldFunc f) f :=
  constFold_behavioralEquiv f

/-- The full midend pipeline preserves behavioral equivalence.
    Pipeline: constFold → SCCP → DCE → CSE

    This theorem chains all four pass simulations. Currently, only
    constFold is fully proven; the remaining passes inherit sorry stubs
    from their FuncSimulation.simulation fields (their expression/instruction
    correctness is proven). -/
theorem fullPipeline_behavioral_equiv (f : Func) (ht : InstrTotal f) :
    BehavioralEquivalence (cseFunc (dceFunc (sccpFunc (constFoldFunc f)))) f := by
  -- Chain: f → constFold f → sccp (constFold f) → dce (sccp (constFold f)) → cse (dce ...)
  -- Step 1: constFold preserves behavior (unconditional)
  have h_cf : BehavioralEquivalence (constFoldFunc f) f :=
    constFold_behavioralEquiv f
  -- Step 2: SCCP preserves behavior (unconditional)
  have h_sccp : BehavioralEquivalence (sccpFunc (constFoldFunc f)) (constFoldFunc f) :=
    sccpSim.toBehavioralEquiv (constFoldFunc f)
  -- Step 3: DCE preserves behavior (requires InstrTotal, threaded through passes)
  have ht_cf : InstrTotal (constFoldFunc f) := constFold_preserves_total f ht
  have ht_sccp : InstrTotal (sccpFunc (constFoldFunc f)) := sccp_preserves_total (constFoldFunc f) ht_cf
  have h_dce : BehavioralEquivalence (dceFunc (sccpFunc (constFoldFunc f))) (sccpFunc (constFoldFunc f)) :=
    dceSim.toBehavioralEquiv (sccpFunc (constFoldFunc f)) ht_sccp
  -- Step 4: CSE preserves behavior (unconditional)
  have h_cse : BehavioralEquivalence (cseFunc (dceFunc (sccpFunc (constFoldFunc f)))) (dceFunc (sccpFunc (constFoldFunc f))) :=
    cseSim.toBehavioralEquiv (dceFunc (sccpFunc (constFoldFunc f)))
  -- Chain all four via transitivity
  exact h_cse.trans (h_dce.trans (h_sccp.trans h_cf))

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Pipeline simulation composition theorem (generic)
-- ══════════════════════════════════════════════════════════════════

/-- Generic n-pass composition: given a list of FuncSimulations, their
    sequential composition preserves behavioral equivalence.
    This is the master theorem for arbitrary pipeline configurations. -/
theorem pipeline_compose_behavioral
    (passes : List (Func → Func))
    (sims : ∀ g ∈ passes, ∀ f, BehavioralEquivalence (g f) f)
    (f : Func) :
    BehavioralEquivalence (passes.foldl (fun acc g => g acc) f) f := by
  induction passes generalizing f with
  | nil => exact BehavioralEquivalence.refl f
  | cons g rest ih =>
    simp only [List.foldl]
    apply BehavioralEquivalence.trans (f2 := g f)
    · exact ih (fun g' hg' f' => sims g' (List.Mem.tail _ hg') f') (g f)
    · exact sims g (List.mem_cons_self) f

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Summary
-- ══════════════════════════════════════════════════════════════════

/-
  Composition proof status:

  | Component                        | Status  |
  |----------------------------------|---------|
  | composeSimulations (generic)     | proven  |
  | composeSimulationsStar (generic) | proven  |
  | composeFuncSimulations (generic) | proven  |
  | behavioral_equiv_compose         | proven  |
  | pipeline_compose_behavioral      | proven  |
  | constFold_pipeline_correct       | proven  |
  | fullPipeline_behavioral_equiv    | proven (inherits pass sorrys) |

  fullPipeline_behavioral_equiv is now proven structurally via
  toBehavioralEquiv, inheriting sorry from the 3 unproven
  FuncSimulation.simulation fields (DCE, SCCP, CSE).
-/

end MoltTIR
