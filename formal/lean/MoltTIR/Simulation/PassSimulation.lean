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
import MoltTIR.Passes.ConstFold
import MoltTIR.Passes.ConstFoldCorrect
import MoltTIR.Passes.DCE
import MoltTIR.Passes.DCECorrect
import MoltTIR.Passes.SCCP
import MoltTIR.Passes.SCCPCorrect
import MoltTIR.Passes.SCCPMultiCorrect
import MoltTIR.Passes.CSE
import MoltTIR.Passes.CSECorrect
import MoltTIR.Semantics.FuncCorrect

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
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨constFoldBlock blk, constFoldFunc_blocks_some f f.entry blk h, constFoldBlock_params blk⟩
  entry_block_none := fun f h => constFoldFunc_blocks_none f f.entry h

/-- Constant folding preserves behavioral equivalence. -/
theorem constFold_behavioralEquiv (f : Func) :
    BehavioralEquivalence (constFoldFunc f) f :=
  constFoldSim.toBehavioralEquiv f

-- ══════════════════════════════════════════════════════════════════
-- Section 2: DCE — block helpers then FuncSimulation
-- ══════════════════════════════════════════════════════════════════

/-- DCE match_states: the target environment agrees with the source
    on all used variables. DCE removes dead instructions, so the target
    environment may lack bindings for dead variables, but agrees on
    all variables that are actually referenced. -/
structure DCEMatchState (used : List Var) where
  src_env : Env
  tgt_env : Env
  agree : EnvAgreeOn used src_env tgt_env

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

/-- DCE simulation at the function level. Since dceFunc transforms each
    block independently (filtering dead instructions), the block-level
    agreement theorem (dce_instrs_agreeOn) lifts to the function level. -/
def dceSim : FuncSimulation dceFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => by
    -- TODO(formal, owner:compiler, milestone:M3, priority:P1, status:partial):
    -- Prove dceFunc preserves execFunc. Requires lifting dce_instrs_agreeOn
    -- through fuel induction + showing terminator agreement from used-var agreement.
    sorry
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨dceBlock blk, dceFunc_blocks_some f f.entry blk h, dceBlock_params blk⟩
  entry_block_none := fun f h => dceFunc_blocks_none f f.entry h

-- ══════════════════════════════════════════════════════════════════
-- Section 3: SCCP — block helpers, instruction correctness, FuncSimulation
-- ══════════════════════════════════════════════════════════════════

/-- SCCP preserves block lookup for found blocks. -/
theorem sccpFunc_blocks_some' (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (sccpFunc f).blocks lbl = some (sccpBlock AbsEnv.top blk).2 :=
  blocks_map_some f (fun b => (sccpBlock AbsEnv.top b).2) lbl blk h

/-- SCCP preserves block lookup failure. -/
theorem sccpFunc_blocks_none' (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (sccpFunc f).blocks lbl = none :=
  blocks_map_none f (fun b => (sccpBlock AbsEnv.top b).2) lbl h

/-- SCCP does not change block parameters. -/
theorem sccpBlock_params (σ : AbsEnv) (b : Block) :
    (sccpBlock σ b).2.params = b.params := rfl

/-- SCCP does not change the terminator. -/
theorem sccpBlock_term (σ : AbsEnv) (b : Block) :
    (sccpBlock σ b).2.term = b.term := rfl

/-- SCCP-transformed instructions preserve execInstrs when the abstract
    environment is sound. Proof by induction on the instruction list,
    using sccpExpr_correct at each step and absEnvSound_set +
    absEvalExpr_concretizes to maintain soundness. -/
theorem sccpInstrs_correct (σ : AbsEnv) (ρ : Env) (instrs : List Instr)
    (hsound : AbsEnvSound σ ρ) :
    execInstrs ρ (sccpInstrs σ instrs).2 = execInstrs ρ instrs := by
  induction instrs generalizing σ ρ with
  | nil => rfl
  | cons i rest ih =>
    simp only [sccpInstrs, execInstrs]
    -- The new RHS matches sccpExpr σ i.rhs by definition
    have heval : evalExpr ρ (match absEvalExpr σ i.rhs with
        | .known v => Expr.val v | _ => i.rhs) = evalExpr ρ i.rhs := by
      cases h : absEvalExpr σ i.rhs with
      | known v =>
        simp only [evalExpr]
        exact (absEvalExpr_sound σ ρ i.rhs hsound v h).symm
      | unknown => rfl
      | overdefined => rfl
    rw [heval]
    match hm : evalExpr ρ i.rhs with
    | none => rfl
    | some v =>
      exact ih (absEnvSound_set σ ρ i.dst v (absEvalExpr σ i.rhs) hsound
        (absEvalExpr_concretizes σ ρ i.rhs v hsound hm))

/-- SCCP preserves evalTerminator even when the function is also
    SCCP-transformed. The terminator expression is unchanged and the
    block params used by jmp/br target lookup are preserved. -/
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

/-- SCCP preserves function execution semantics.
    Proof by induction on fuel, following the constFoldFunc_correct pattern. -/
theorem sccpFunc_correct (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (sccpFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none =>
      simp [sccpFunc_blocks_none' f lbl hblk]
    | some blk =>
      simp only [sccpFunc_blocks_some' f lbl blk hblk]
      rw [sccpInstrs_correct AbsEnv.top ρ blk.instrs (absEnvTop_sound ρ)]
      match execInstrs ρ blk.instrs with
      | none => rfl
      | some ρ' =>
        simp only [sccpBlock_term, sccp_evalTerminator, ih]

/-- SCCP simulation. -/
def sccpSim : FuncSimulation sccpFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => sccpFunc_correct f fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨(sccpBlock AbsEnv.top blk).2, sccpFunc_blocks_some' f f.entry blk h,
     sccpBlock_params AbsEnv.top blk⟩
  entry_block_none := fun f h => sccpFunc_blocks_none' f f.entry h

/-- SCCP preserves block lookup for found blocks (existential form). -/
theorem sccpFunc_blocks_some (f : Func) (lbl : Label) (blk : Block) :
    f.blocks lbl = some blk →
    ∃ blk', (sccpFunc f).blocks lbl = some blk' := by
  intro h
  exact ⟨(sccpBlock AbsEnv.top blk).2, sccpFunc_blocks_some' f lbl blk h⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 4: CSE — block helpers then FuncSimulation
-- ══════════════════════════════════════════════════════════════════

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

/-- CSE does not change block parameters. -/
theorem cseBlock_params (b : Block) : (cseBlock b).params = b.params := rfl

/-- CSE simulation. -/
def cseSim : FuncSimulation cseFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => by
    -- TODO(formal, owner:compiler, milestone:M3, priority:P2, status:partial):
    -- Prove cseFunc preserves execFunc. Requires threading AvailMapSound
    -- through instructions under SSA freshness and lifting via fuel induction.
    sorry
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨cseBlock blk, cseFunc_blocks_some f f.entry blk h, cseBlock_params blk⟩
  entry_block_none := fun f h => cseFunc_blocks_none f f.entry h

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Summary of simulation status
-- ══════════════════════════════════════════════════════════════════

/-
  Pass simulation status:

  | Pass          | FuncSimulation | execFunc preserved | Behavioral equiv | blocks_some/none |
  |---------------|:--------------:|:------------------:|:----------------:|:----------------:|
  | ConstFold     |       ✓        |         ✓          |        ✓         |        ✓         |
  | DCE           |    sorry (P1)  |     sorry (P1)     |    sorry (P1)    |        ✓         |
  | SCCP          |       ✓        |         ✓          |        ✓         |        ✓         |
  | CSE           |    sorry (P2)  |     sorry (P2)     |    sorry (P2)    |        ✓         |

  ConstFold has a complete end-to-end proof chain: FuncSimulation (via
  constFoldFunc_correct from Semantics/FuncCorrect.lean) and BehavioralEquivalence
  (via FuncSimulation.toBehavioralEquiv).

  SCCP now has a complete end-to-end proof chain: FuncSimulation (via
  sccpFunc_correct proved here using sccpInstrs_correct + sccp_evalTerminator)
  and BehavioralEquivalence (via FuncSimulation.toBehavioralEquiv).

  DCE/CSE block lookup lemmas are now proven via blocks_map_some/none
  from BlockCorrect. The remaining sorry gaps are the function-level execFunc
  preservation proofs, which require lifting block-level correctness through
  fuel induction (see TODO comments on each).
-/

end MoltTIR
