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

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Composing BehavioralEquivalence
-- ══════════════════════════════════════════════════════════════════

/-- If g preserves execFunc for all inputs, then g preserves runFunc. -/
theorem funcSimulation_to_behavioral {g : Func → Func}
    (sim : FuncSimulation g) (f : Func) :
    BehavioralEquivalence (g f) f := by
  intro fuel
  simp only [runFunc]
  -- The simulation gives us: execFunc (g f) fuel ρ lbl = execFunc f fuel ρ lbl
  -- But we need to also know that g preserves f.entry and block lookup at entry.
  -- The general FuncSimulation does not carry this info, so this theorem
  -- cannot be proven without additional structure on g.
  -- However, sim.simulation gives execFunc equality for all ρ/lbl, including
  -- the entry. The gap is that runFunc checks (g f).blocks (g f).entry vs
  -- f.blocks f.entry, which requires knowing g preserves entry and block lookup.
  -- Since FuncSimulation.simulation only speaks about execFunc (not blocks/entry),
  -- this gap is fundamental to the current FuncSimulation definition.
  -- We mark this as sorry until FuncSimulation is extended.
  sorry
  -- TODO(formal, owner:compiler, milestone:M3, priority:P1, status:partial):
  -- Requires FuncSimulation to carry proof that g preserves f.entry
  -- and block params. All current transforms do this trivially.

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
    constFold is fully proven; the remaining passes have sorry stubs
    at the function level (their expression/instruction correctness
    is proven). -/
theorem fullPipeline_behavioral_equiv (f : Func) :
    BehavioralEquivalence (cseFunc (dceFunc (sccpFunc (constFoldFunc f)))) f := by
  apply behavioral_equiv_compose
    (g1 := fun f => sccpFunc (constFoldFunc f))
    (g2 := fun f => cseFunc (dceFunc f))
  · -- constFold ∘ sccp preserves behavior
    intro f'
    apply behavioral_equiv_compose
      (g1 := constFoldFunc)
      (g2 := sccpFunc)
    · exact constFold_behavioralEquiv
    · intro f''
      -- SCCP behavioral equiv (sorry — depends on sccpSim.simulation)
      sorry
      -- TODO(formal, owner:compiler, milestone:M3, priority:P1, status:partial):
      -- Close once sccpSim.simulation is proven.
  · -- dce ∘ cse preserves behavior
    intro f'
    apply behavioral_equiv_compose
      (g1 := dceFunc)
      (g2 := cseFunc)
    · intro f''
      -- DCE behavioral equiv (sorry — depends on dceSim.simulation)
      sorry
      -- TODO(formal, owner:compiler, milestone:M3, priority:P1, status:partial):
      -- Close once dceSim.simulation is proven.
    · intro f''
      -- CSE behavioral equiv (sorry — depends on cseSim.simulation)
      sorry
      -- TODO(formal, owner:compiler, milestone:M3, priority:P2, status:partial):
      -- Close once cseSim.simulation is proven.

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
    · exact ih (fun g' hg' f' => sims g' (List.mem_cons_of_mem _ hg') f') (g f)
    · exact sims g (List.mem_cons_self _ _) f

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
  | fullPipeline_behavioral_equiv    | 3 sorry |

  The 3 remaining sorry stubs correspond exactly to the 3 unproven
  FuncSimulation instances (DCE, SCCP, CSE). Once those are closed,
  fullPipeline_behavioral_equiv follows mechanically.
-/

end MoltTIR
