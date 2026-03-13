/-
  MoltTIR.Simulation.Diagram — CompCert-style forward simulation diagrams.

  This module defines the core simulation framework for proving that each
  compiler pass preserves observable behavior. The approach follows
  Leroy's CompCert methodology:

  A forward simulation between source and target is a relation
  `match_states` such that every source step has a corresponding
  target step (or step sequence) that preserves the relation.

  Key definitions:
  - `StarStep` — reflexive transitive closure of a step relation
  - `ForwardSimulation` — 1-to-1 forward simulation (lock-step)
  - `ForwardSimulationStar` — 1-to-many forward simulation
  - `SimulationComposition` — composing two simulations
  - `Trace` / `BehavioralEquivalence` — observable trace equivalence

  All definitions are parametric over state and step types so they
  apply uniformly to every pass in the pipeline.
-/
import MoltTIR.Semantics.State
import MoltTIR.Semantics.ExecFunc

set_option autoImplicit false

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Reflexive transitive closure
-- ══════════════════════════════════════════════════════════════════

/-- Reflexive transitive closure of a step relation.
    `StarStep step s1 s2` means s1 can reach s2 in zero or more steps. -/
inductive StarStep {S : Type} (step : S → S → Prop) : S → S → Prop where
  | refl (s : S) : StarStep step s s
  | cons (s1 s2 s3 : S) : step s1 s2 → StarStep step s2 s3 → StarStep step s1 s3

namespace StarStep

variable {S : Type} {step : S → S → Prop}

/-- StarStep is transitive. -/
theorem trans {a b c : S} (h1 : StarStep step a b) (h2 : StarStep step b c) :
    StarStep step a c := by
  induction h1 with
  | refl _ => exact h2
  | cons s1 s2 _ hs _ ih => exact .cons s1 s2 c hs (ih h2)

/-- A single step lifts to StarStep. -/
theorem single {a b : S} (h : step a b) : StarStep step a b :=
  .cons a b b h (.refl b)

/-- Append a single step at the end. -/
theorem snoc {a b c : S} (h1 : StarStep step a b) (h2 : step b c) :
    StarStep step a c :=
  h1.trans (single h2)

end StarStep

-- ══════════════════════════════════════════════════════════════════
-- Section 2: PlusStep (at least one step)
-- ══════════════════════════════════════════════════════════════════

/-- At least one step: the transitive (non-reflexive) closure. -/
inductive PlusStep {S : Type} (step : S → S → Prop) : S → S → Prop where
  | single (s1 s2 : S) : step s1 s2 → PlusStep step s1 s2
  | cons (s1 s2 s3 : S) : step s1 s2 → PlusStep step s2 s3 → PlusStep step s1 s3

namespace PlusStep

variable {S : Type} {step : S → S → Prop}

/-- PlusStep implies StarStep. -/
theorem toStar {a b : S} (h : PlusStep step a b) : StarStep step a b := by
  induction h with
  | single s1 s2 hs => exact .cons s1 s2 s2 hs (.refl s2)
  | cons s1 s2 _ hs _ ih => exact .cons s1 s2 _ hs ih

end PlusStep

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Forward simulation (lock-step, 1-to-1)
-- ══════════════════════════════════════════════════════════════════

/-- A forward simulation diagram between two program representations.
    For every source step from s1 to s2, if match_states s1 t1, then
    there exists t2 such that t1 steps to t2 and match_states s2 t2.

    This is the standard CompCert "diagram" for passes with 1-to-1
    step correspondence (e.g., constant folding, SCCP). -/
structure ForwardSimulation (SourceState TargetState : Type)
    (source_step : SourceState → SourceState → Prop)
    (target_step : TargetState → TargetState → Prop) where
  /-- Relates source states to their target counterparts. -/
  match_states : SourceState → TargetState → Prop
  /-- The simulation property: source steps are matched by target steps. -/
  simulation : ∀ (s1 s2 : SourceState) (t1 : TargetState),
    match_states s1 t1 →
    source_step s1 s2 →
    ∃ t2, target_step t1 t2 ∧ match_states s2 t2

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Forward simulation with multi-step target (1-to-star)
-- ══════════════════════════════════════════════════════════════════

/-- A forward simulation where a single source step may correspond to
    zero or more target steps. Used for passes that eliminate instructions
    (e.g., DCE removes dead code, so some source steps have no target
    counterpart — the target "stutters"). -/
structure ForwardSimulationStar (SourceState TargetState : Type)
    (source_step : SourceState → SourceState → Prop)
    (target_step : TargetState → TargetState → Prop) where
  /-- Relates source states to their target counterparts. -/
  match_states : SourceState → TargetState → Prop
  /-- The simulation property: source steps are matched by zero or more target steps. -/
  simulation : ∀ (s1 s2 : SourceState) (t1 : TargetState),
    match_states s1 t1 →
    source_step s1 s2 →
    ∃ t2, StarStep target_step t1 t2 ∧ match_states s2 t2

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Lifting lock-step to star simulation
-- ══════════════════════════════════════════════════════════════════

/-- Every lock-step simulation is trivially a star simulation. -/
def ForwardSimulation.toStar {S T : Type}
    {ss : S → S → Prop} {ts : T → T → Prop}
    (sim : ForwardSimulation S T ss ts) :
    ForwardSimulationStar S T ss ts where
  match_states := sim.match_states
  simulation := fun s1 s2 t1 hm hs => by
    obtain ⟨t2, ht, hm'⟩ := sim.simulation s1 s2 t1 hm hs
    exact ⟨t2, StarStep.single ht, hm'⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Star simulation preserves multi-step source execution
-- ══════════════════════════════════════════════════════════════════

/-- If a star simulation holds, then a multi-step source execution is
    matched by a multi-step target execution. -/
theorem ForwardSimulationStar.star_simulation {S T : Type}
    {ss : S → S → Prop} {ts : T → T → Prop}
    (sim : ForwardSimulationStar S T ss ts)
    {s1 s2 : S} {t1 : T}
    (hm : sim.match_states s1 t1)
    (hs : StarStep ss s1 s2) :
    ∃ t2, StarStep ts t1 t2 ∧ sim.match_states s2 t2 := by
  induction hs generalizing t1 with
  | refl s => exact ⟨t1, .refl t1, hm⟩
  | cons sa sb sc step_ab _star_bc ih =>
    obtain ⟨tb, htb, hmb⟩ := sim.simulation sa sb t1 hm step_ab
    obtain ⟨tc, htc, hmc⟩ := ih hmb
    exact ⟨tc, htb.trans htc, hmc⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Observable traces and behavioral equivalence
-- ══════════════════════════════════════════════════════════════════

/-- An observable event. In Molt TIR, the only observable is the return
    value of a function. This can be extended with I/O events, prints,
    exceptions, etc. as the model grows. -/
inductive Event where
  | retVal (v : Value)
  | stuck
  deriving DecidableEq, Repr

/-- A trace is a (possibly empty) sequence of observable events.
    For terminating programs, this is typically a singleton list. -/
abbrev Trace := List Event

/-- Extract a trace from an Outcome. -/
def outcomeTrace : Outcome → Trace
  | .ret v => [.retVal v]
  | .stuck => [.stuck]

/-- Two programs are behaviorally equivalent if they produce the same
    trace for all fuel values. This captures the intuition that no
    finite observation can distinguish them.

    For Molt TIR functions, this means: for any fuel, if the source
    program produces an outcome, the target produces the same outcome. -/
def BehavioralEquivalence (f1 f2 : Func) : Prop :=
  ∀ (fuel : Nat), runFunc f1 fuel = runFunc f2 fuel

-- ══════════════════════════════════════════════════════════════════
-- Section 8: BehavioralEquivalence is an equivalence relation
-- ══════════════════════════════════════════════════════════════════

theorem BehavioralEquivalence.refl (f : Func) : BehavioralEquivalence f f :=
  fun _ => rfl

theorem BehavioralEquivalence.symm {f1 f2 : Func}
    (h : BehavioralEquivalence f1 f2) : BehavioralEquivalence f2 f1 :=
  fun fuel => (h fuel).symm

theorem BehavioralEquivalence.trans {f1 f2 f3 : Func}
    (h12 : BehavioralEquivalence f1 f2)
    (h23 : BehavioralEquivalence f2 f3) :
    BehavioralEquivalence f1 f3 :=
  fun fuel => (h12 fuel).trans (h23 fuel)

-- ══════════════════════════════════════════════════════════════════
-- Section 9: Fuel-based simulation for Molt TIR functions
-- ══════════════════════════════════════════════════════════════════

/-- The execution state for fuel-based stepping through a Molt TIR function.
    Packages the function, remaining fuel, current environment, and current label. -/
structure ExecState where
  func : Func
  fuel : Nat
  env  : Env
  label : Label

/-- A single block-transition step in execFunc: execute the current block's
    instructions and terminator, consuming one unit of fuel. -/
inductive BlockStep : ExecState → ExecState → Prop where
  | step (f : Func) (n : Nat) (ρ ρ' ρ'' : Env) (lbl target : Label) (blk : Block)
    (hblk : f.blocks lbl = some blk)
    (hinstr : execInstrs ρ blk.instrs = some ρ')
    (hterm : evalTerminator f ρ' blk.term = some (.jump target ρ'')) :
    BlockStep
      { func := f, fuel := n + 1, env := ρ, label := lbl }
      { func := f, fuel := n, env := ρ'', label := target }

/-- Simulation at the Molt TIR function level: a transform `g` on functions
    preserves BlockStep transitions. This is the key interface between the
    generic simulation framework and the concrete Molt TIR semantics. -/
structure FuncSimulation (g : Func → Func) where
  /-- The match_states relation: source and target states correspond when
      the target function is the transform of the source, and environments
      and labels are related. -/
  match_env : Func → Env → Label → Env → Label → Prop
  /-- For every source block transition, the transformed function makes
      a corresponding transition. -/
  simulation : ∀ (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label),
    execFunc (g f) fuel ρ lbl = execFunc f fuel ρ lbl

-- ══════════════════════════════════════════════════════════════════
-- Section 10: From FuncSimulation to BehavioralEquivalence
-- ══════════════════════════════════════════════════════════════════

/-- A FuncSimulation directly implies behavioral equivalence: if the
    transform preserves execFunc for all fuel/env/label, then it
    preserves runFunc for all fuel. -/
theorem FuncSimulation.toBehavioralEquiv {g : Func → Func}
    (sim : FuncSimulation g) (f : Func) :
    BehavioralEquivalence (g f) f := by
  intro fuel
  simp only [runFunc]
  -- The entry block lookup must agree between g f and f
  -- This requires knowing that g preserves the entry label and block structure.
  -- The full proof depends on the specific transform g.
  -- For transforms that preserve blockList structure (constFold, sccp, dce, cse),
  -- the entry label is unchanged and block lookup is preserved.
  sorry
  -- TODO(formal, owner:compiler, milestone:M3, priority:P2, status:partial):
  -- Close this gap by requiring FuncSimulation to carry proof that
  -- g preserves f.entry and the entry block's params.

end MoltTIR
