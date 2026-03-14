/-
  MoltTIR.Simulation.Adequacy — Adequacy theorem: simulation implies observational equivalence.

  The adequacy theorem is the key metatheoretic bridge between the simulation
  framework (Diagram.lean) and the end-user guarantee (observational equivalence).
  It states:

    If a forward simulation exists between a source and target program,
    then no finite observation can distinguish them — the target program's
    observable trace is a superset of (and, for deterministic programs,
    equal to) the source program's observable trace.

  Structure:
  1. Contextual equivalence for Molt TIR functions (fuel-indexed)
  2. Trace inclusion: forward simulation implies trace refinement
  3. Adequacy: simulation implies observational equivalence for deterministic programs
  4. Corollary: FuncSimulation implies contextual equivalence
-/
import MoltTIR.Simulation.Diagram
import MoltTIR.Simulation.Compose

set_option autoImplicit false

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Contextual equivalence
-- ══════════════════════════════════════════════════════════════════

/-- Contextual equivalence for Molt TIR functions: two functions are
    contextually equivalent if they produce the same outcome in every
    execution context (i.e., for all fuel values, all environments,
    and all entry labels).

    This is stronger than BehavioralEquivalence (which only considers
    runFunc with empty env and the function's entry label). Contextual
    equivalence additionally requires agreement under arbitrary env/label
    contexts, making it the gold standard for compiler correctness. -/
def ContextualEquivalence (f1 f2 : Func) : Prop :=
  ∀ (fuel : Nat) (ρ : Env) (lbl : Label),
    execFunc f1 fuel ρ lbl = execFunc f2 fuel ρ lbl

/-- Contextual equivalence is an equivalence relation. -/
theorem ContextualEquivalence.refl (f : Func) : ContextualEquivalence f f :=
  fun _ _ _ => rfl

theorem ContextualEquivalence.symm {f1 f2 : Func}
    (h : ContextualEquivalence f1 f2) : ContextualEquivalence f2 f1 :=
  fun fuel ρ lbl => (h fuel ρ lbl).symm

theorem ContextualEquivalence.trans {f1 f2 f3 : Func}
    (h12 : ContextualEquivalence f1 f2)
    (h23 : ContextualEquivalence f2 f3) :
    ContextualEquivalence f1 f3 :=
  fun fuel ρ lbl => (h12 fuel ρ lbl).trans (h23 fuel ρ lbl)

/-- Contextual equivalence implies behavioral equivalence.
    (The converse does not hold in general.) -/
theorem ContextualEquivalence.toBehavioral {f1 f2 : Func}
    (h : ContextualEquivalence f1 f2) :
    BehavioralEquivalence f1 f2 := by
  intro fuel
  simp only [runFunc]
  -- Both functions must agree on their own entry/blocks structure
  -- for the reduction to work. In general, contextual equivalence
  -- at the execFunc level does not directly imply runFunc equality
  -- unless the functions share entry/block structure.
  -- For compiler transforms g where g f shares entry with f, this holds.
  sorry
  -- TODO(formal, owner:compiler, milestone:M4, priority:P2, status:partial):
  -- Requires showing that contextually equivalent functions with the same
  -- entry label and block params produce the same runFunc outcome.
  -- This is straightforward when f1 and f2 share the entry label.

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Trace inclusion (forward simulation → trace refinement)
-- ══════════════════════════════════════════════════════════════════

/-- Outcome trace: extract the observable trace from an execution outcome. -/
def outcomeTraceOpt : Option Outcome → Trace
  | some (.ret v) => [.retVal v]
  | some .stuck   => [.stuck]
  | none          => []  -- out of fuel: no observation yet

/-- Trace inclusion: every source observation is also a target observation.

    For fuel-bounded execution, this means: if the source program produces
    an outcome with some fuel, the target produces the same outcome with
    the same fuel.

    This is the semantic content of a forward simulation for deterministic
    programs: simulations guarantee that the target can match every source
    step, so the final outcomes must agree. -/
def TraceInclusion (f_src f_tgt : Func) : Prop :=
  ∀ (fuel : Nat) (ρ : Env) (lbl : Label) (o : Outcome),
    execFunc f_src fuel ρ lbl = some o →
    execFunc f_tgt fuel ρ lbl = some o

/-- Trace inclusion implies contextual equivalence for deterministic total programs.

    If both source and target are deterministic (which they are, since execFunc
    is a function) and trace inclusion holds in both directions, then the
    programs are contextually equivalent.

    Note: unidirectional trace inclusion (source ⊆ target) is sufficient for
    compiler correctness — it means the compiled program never exhibits
    behavior that the source didn't have. The reverse direction (target ⊆ source)
    gives completeness (no source behaviors are lost). -/
theorem traceInclusion_both_to_contextual {f1 f2 : Func}
    (h12 : TraceInclusion f1 f2)
    (h21 : TraceInclusion f2 f1) :
    ContextualEquivalence f1 f2 := by
  intro fuel ρ lbl
  match h1 : execFunc f1 fuel ρ lbl, h2 : execFunc f2 fuel ρ lbl with
  | some o1, some o2 =>
    have := h12 fuel ρ lbl o1 h1
    rw [h2] at this
    cases this; rfl
  | some o1, none =>
    -- Source succeeded but target ran out of fuel.
    -- Forward trace inclusion says target also produces o1 — contradiction.
    have := h12 fuel ρ lbl o1 h1
    simp [this] at h2
  | none, some o2 =>
    -- Symmetric case using reverse inclusion.
    have := h21 fuel ρ lbl o2 h2
    simp [this] at h1
  | none, none => rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 3: FuncSimulation implies trace inclusion
-- ══════════════════════════════════════════════════════════════════

/-- A FuncSimulation gives bidirectional trace inclusion (and hence
    contextual equivalence).

    Since FuncSimulation.simulation states
      execFunc (g f) fuel ρ lbl = execFunc f fuel ρ lbl
    for all fuel/ρ/lbl, this directly gives both directions of trace
    inclusion between (g f) and f. -/
theorem funcSimulation_trace_inclusion {g : Func → Func}
    (sim : FuncSimulation g) (f : Func) :
    TraceInclusion (g f) f ∧ TraceInclusion f (g f) := by
  constructor
  · -- Forward: (g f) outcome → f outcome
    intro fuel ρ lbl o h
    rw [sim.simulation f fuel ρ lbl] at h
    exact h
  · -- Backward: f outcome → (g f) outcome
    intro fuel ρ lbl o h
    rw [sim.simulation f fuel ρ lbl]
    exact h

/-- FuncSimulation implies contextual equivalence. -/
theorem funcSimulation_contextual_equiv {g : Func → Func}
    (sim : FuncSimulation g) (f : Func) :
    ContextualEquivalence (g f) f := by
  intro fuel ρ lbl
  exact sim.simulation f fuel ρ lbl

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Adequacy — the main theorem
-- ══════════════════════════════════════════════════════════════════

/-- **Adequacy theorem**: forward simulation implies observational equivalence.

    For a deterministic, fuel-bounded semantics, a forward simulation between
    a source function f and a transformed function (g f) implies that g f and
    f are observationally indistinguishable.

    Concretely: if FuncSimulation g holds, then for any function f, the
    compiled version (g f) produces exactly the same outcomes as f for all
    fuel/env/label contexts.

    This is the key metatheorem that justifies using simulation diagrams as
    the proof methodology for compiler passes. It says:

      "Proving a simulation is sufficient to guarantee that the compiled
       program behaves identically to the source."

    The proof is direct for fuel-bounded deterministic semantics because
    FuncSimulation.simulation already provides the exact equality
    execFunc (g f) fuel ρ lbl = execFunc f fuel ρ lbl. In a more general
    setting (non-deterministic, coinductive), adequacy requires additional
    machinery (e.g., logical relations, biorthogonality). -/
theorem adequacy {g : Func → Func} (sim : FuncSimulation g) (f : Func) :
    ContextualEquivalence (g f) f :=
  funcSimulation_contextual_equiv sim f

/-- Adequacy for behavioral equivalence: simulation implies the programs
    produce the same runFunc result for all fuel values. -/
theorem adequacy_behavioral {g : Func → Func} (sim : FuncSimulation g) (f : Func) :
    BehavioralEquivalence (g f) f :=
  sim.toBehavioralEquiv f

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Composition of contextual equivalences
-- ══════════════════════════════════════════════════════════════════

/-- Contextual equivalences compose: if g1 and g2 each produce contextually
    equivalent functions, then (g2 . g1) does as well. -/
theorem contextual_equiv_compose {g1 g2 : Func → Func}
    (h1 : ∀ f, ContextualEquivalence (g1 f) f)
    (h2 : ∀ f, ContextualEquivalence (g2 f) f)
    (f : Func) :
    ContextualEquivalence (g2 (g1 f)) f := by
  intro fuel ρ lbl
  calc execFunc (g2 (g1 f)) fuel ρ lbl
      = execFunc (g1 f) fuel ρ lbl := h2 (g1 f) fuel ρ lbl
    _ = execFunc f fuel ρ lbl       := h1 f fuel ρ lbl

/-- Pipeline of contextual equivalences: given a list of transforms, each
    preserving contextual equivalence, their fold preserves it. -/
theorem pipeline_contextual_equiv
    (passes : List (Func → Func))
    (hpasses : ∀ g ∈ passes, ∀ f, ContextualEquivalence (g f) f)
    (f : Func) :
    ContextualEquivalence (passes.foldl (fun acc g => g acc) f) f := by
  induction passes generalizing f with
  | nil => exact ContextualEquivalence.refl f
  | cons g rest ih =>
    simp only [List.foldl]
    apply ContextualEquivalence.trans
    · exact ih (fun g' hg' f' => hpasses g' (List.mem_cons_of_mem _ hg') f') (g f)
    · exact hpasses g (List.mem_cons_self _ _) f

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Adequacy for the full midend pipeline
-- ══════════════════════════════════════════════════════════════════

/-- The full midend pipeline (constFold → SCCP → DCE → CSE) preserves
    contextual equivalence, assuming each pass has a FuncSimulation.

    This follows from composing the per-pass adequacy results. -/
theorem fullPipeline_contextual_equiv (f : Func) :
    ContextualEquivalence (cseFunc (dceFunc (sccpFunc (constFoldFunc f)))) f := by
  apply contextual_equiv_compose
    (g1 := fun f => sccpFunc (constFoldFunc f))
    (g2 := fun f => cseFunc (dceFunc f))
  · -- constFold . SCCP contextual equiv
    intro f'
    apply contextual_equiv_compose
      (g1 := constFoldFunc)
      (g2 := sccpFunc)
    · exact fun f'' => funcSimulation_contextual_equiv constFoldSim f''
    · exact fun f'' => funcSimulation_contextual_equiv sccpSim f''
  · -- DCE . CSE contextual equiv
    intro f'
    apply contextual_equiv_compose
      (g1 := dceFunc)
      (g2 := cseFunc)
    · exact fun f'' => funcSimulation_contextual_equiv dceSim f''
    · exact fun f'' => funcSimulation_contextual_equiv cseSim f''

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Summary
-- ══════════════════════════════════════════════════════════════════

/-!
## Adequacy Proof Status

| Component                              | Status   |
|----------------------------------------|----------|
| ContextualEquivalence (defn + equiv)   | proven   |
| TraceInclusion (defn)                  | proven   |
| traceInclusion_both_to_contextual      | proven   |
| funcSimulation_trace_inclusion         | proven   |
| funcSimulation_contextual_equiv        | proven   |
| adequacy (main theorem)               | proven   |
| adequacy_behavioral                    | 1 sorry  |
| ContextualEquivalence.toBehavioral     | 1 sorry  |
| contextual_equiv_compose               | proven   |
| pipeline_contextual_equiv              | proven   |
| fullPipeline_contextual_equiv          | 3 sorry  |

The 2 sorry stubs in adequacy_behavioral and toBehavioral stem from the
same root cause: FuncSimulation constrains execFunc but runFunc has an
additional layer (entry block lookup, params check) that requires the
transform to preserve those structural fields. All concrete transforms
do this, but the generic FuncSimulation type does not encode it.

The 3 sorry stubs in fullPipeline_contextual_equiv correspond to the
same 3 unproven FuncSimulation instances (SCCP, DCE, CSE) as in Compose.lean.
-/

end MoltTIR
