/-
  MoltTIR.Simulation.PassSimulation — simulation diagram instances for each midend pass.

  Instantiates the generic simulation framework from Diagram.lean for
  each verified compiler pass:
  - constFoldSim  — constant folding (1-to-1 step correspondence)
  - dceSim        — dead code elimination (source step → 0 or 1 target steps)
  - sccpSim       — SCCP (1-to-1 with abstract env soundness)
  - cseSim        — CSE (n-to-1 expression merging, 1-to-1 block steps)

  Each instantiation defines match_states and proves (or stubs with sorry)
  the simulation property. The proofs leverage the existing per-pass
  correctness theorems (constFoldFunc_correct, etc.) to establish the
  simulation diagrams.
-/
import MoltTIR.Simulation.Diagram
import MoltTIR.Semantics.FuncCorrect
import MoltTIR.Semantics.BlockCorrect
import MoltTIR.Passes.ConstFold
import MoltTIR.Passes.ConstFoldCorrect
import MoltTIR.Passes.DCE
import MoltTIR.Passes.DCECorrect
import MoltTIR.Passes.SCCP
import MoltTIR.Passes.SCCPCorrect
import MoltTIR.Passes.CSE
import MoltTIR.Passes.CSECorrect

set_option autoImplicit false

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Constant Folding — FuncSimulation
-- ══════════════════════════════════════════════════════════════════

/-- Constant folding has a direct FuncSimulation: it preserves execFunc
    exactly (proven in FuncCorrect.lean). The match_states is identity
    on env and label — constant folding doesn't change the control flow
    or the values computed, only the syntactic form of expressions. -/
def constFoldSim : FuncSimulation constFoldFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => constFoldFunc_correct f fuel ρ lbl

/-- Constant folding preserves behavioral equivalence. -/
theorem constFold_behavioralEquiv (f : Func) :
    BehavioralEquivalence (constFoldFunc f) f := by
  intro fuel
  simp only [runFunc]
  have h := constFoldFunc_correct f fuel Env.empty f.entry
  -- Need to handle the entry block lookup
  match hblk : f.blocks f.entry with
  | none =>
    have : (constFoldFunc f).blocks (constFoldFunc f).entry = none := by
      simp only [constFoldFunc]
      exact constFoldFunc_blocks_none f f.entry hblk
    simp [this, hblk]
  | some blk =>
    have hcf : (constFoldFunc f).blocks (constFoldFunc f).entry
        = some (constFoldBlock blk) := by
      simp only [constFoldFunc]
      exact constFoldFunc_blocks_some f f.entry blk hblk
    simp only [hcf, hblk, constFoldBlock_params]
    cases blk.params.isEmpty <;> simp [h]

-- ══════════════════════════════════════════════════════════════════
-- Section 2: DCE — ForwardSimulationStar (source step → 0 or 1 target)
-- ══════════════════════════════════════════════════════════════════

/-- DCE match_states: the target environment agrees with the source
    on all used variables. DCE removes dead instructions, so the target
    environment may lack bindings for dead variables, but agrees on
    all variables that are actually referenced. -/
structure DCEMatchState (used : List Var) where
  src_env : Env
  tgt_env : Env
  agree : EnvAgreeOn used src_env tgt_env

/-- DCE simulation at the function level. Since dceFunc transforms each
    block independently (filtering dead instructions), the block-level
    agreement theorem (dce_instrs_agreeOn) lifts to the function level.

    The simulation is ForwardSimulationStar because dead instructions in
    the source have no corresponding target step — the target "stutters". -/
def dceSim : FuncSimulation dceFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => by
    -- TODO(formal, owner:compiler, milestone:M3, priority:P1, status:partial):
    -- Prove dceFunc preserves execFunc. Requires:
    -- 1. dceFunc preserves block lookup structure (analogous to blocks_map_some/none)
    -- 2. dceBlock preserves evalTerminator (DCE doesn't touch terminators)
    -- 3. dce_instrs_agreeOn lifts to execFunc level via fuel induction
    -- The block-level correctness is already proven (dce_instrs_agreeOn).
    -- The gap is lifting from environment-agreement to exact execFunc equality,
    -- which requires showing that DCE doesn't change the terminator and that
    -- agreement on used vars implies identical terminator outcomes.
    sorry

/-- DCE preserves block lookup for found blocks. -/
theorem dceFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (dceFunc f).blocks lbl = some (dceBlock blk) :=
  blocks_map_some f dceBlock lbl blk h

/-- DCE preserves block lookup failure. -/
theorem dceFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (dceFunc f).blocks lbl = none :=
  blocks_map_none f dceBlock lbl h

/-- DCE does not change block parameters. -/
theorem dceBlock_params (b : Block) : (dceBlock b).params = b.params := rfl

/-- DCE does not change the terminator. -/
theorem dceBlock_term (b : Block) : (dceBlock b).term = b.term := rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 3: SCCP — FuncSimulation with abstract env soundness
-- ══════════════════════════════════════════════════════════════════

/-- SCCP simulation. The match_states requires the abstract environment
    to soundly approximate the concrete environment. Under this condition,
    SCCP-transformed expressions evaluate identically (sccpExpr_correct). -/
def sccpSim : FuncSimulation sccpFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => by
    -- TODO(formal, owner:compiler, milestone:M3, priority:P1, status:partial):
    -- Prove sccpFunc preserves execFunc. Requires:
    -- 1. sccpFunc preserves block lookup structure
    -- 2. sccpBlock with top abstract env preserves instruction execution
    --    (each sccpExpr replacement is correct by sccpExpr_correct + absEnvTop_sound)
    -- 3. The abstract env must be threaded correctly through instructions
    --    (sccpInstrs updates σ as it processes each instruction)
    -- The expression-level and instruction-level proofs exist (SCCPCorrect.lean).
    -- The gap is showing that sccpBlock with AbsEnv.top is semantics-preserving
    -- at the block level, and then lifting to function level via fuel induction.
    sorry

/-- SCCP preserves block lookup for found blocks. -/
theorem sccpFunc_blocks_some (f : Func) (lbl : Label) (blk : Block) :
    f.blocks lbl = some blk →
    ∃ blk', (sccpFunc f).blocks lbl = some blk' := by
  intro h
  have := blocks_map_some f (fun blk => (sccpBlock AbsEnv.top blk).2) lbl blk h
  exact ⟨_, this⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 4: CSE — FuncSimulation with availability map soundness
-- ══════════════════════════════════════════════════════════════════

/-- CSE simulation. The match_states requires the availability map to be
    sound w.r.t. the current environment. Under SSA freshness, CSE-
    transformed expressions evaluate identically (cseExpr_correct). -/
def cseSim : FuncSimulation cseFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => by
    -- TODO(formal, owner:compiler, milestone:M3, priority:P2, status:partial):
    -- Prove cseFunc preserves execFunc. Requires:
    -- 1. cseFunc preserves block lookup structure
    -- 2. cseBlock preserves block execution under SSA freshness
    -- 3. cseInstr threading maintains AvailMapSound
    -- The expression-level proof exists (cseExpr_correct in CSECorrect.lean).
    -- The gap is threading the availability map through instructions while
    -- maintaining the SSA freshness invariant, then lifting to function level.
    -- This is the most involved proof because CSE's correctness depends on
    -- a global SSA property (no variable is defined twice).
    sorry

/-- CSE preserves block lookup for found blocks. -/
theorem cseFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (cseFunc f).blocks lbl = some (cseBlock blk) :=
  blocks_map_some f cseBlock lbl blk h

/-- CSE preserves block lookup failure. -/
theorem cseFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (cseFunc f).blocks lbl = none :=
  blocks_map_none f cseBlock lbl h

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Summary of simulation status
-- ══════════════════════════════════════════════════════════════════

/-
  Pass simulation status:

  | Pass          | FuncSimulation | execFunc preserved | Behavioral equiv |
  |---------------|:--------------:|:------------------:|:----------------:|
  | ConstFold     |       ✓        |         ✓          |        ✓         |
  | DCE           |    sorry (P1)  |     sorry (P1)     |    sorry (P1)    |
  | SCCP          |    sorry (P1)  |     sorry (P1)     |    sorry (P1)    |
  | CSE           |    sorry (P2)  |     sorry (P2)     |    sorry (P2)    |

  ConstFold is the only pass with a complete end-to-end proof (via
  constFoldFunc_correct). The remaining passes have expression-level or
  instruction-level correctness proven, but the lift to function-level
  execFunc preservation requires additional work as described in each
  sorry's TODO comment.
-/

end MoltTIR
