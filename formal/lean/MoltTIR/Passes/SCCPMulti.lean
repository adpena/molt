/-
  MoltTIR.Passes.SCCPMulti — multi-block SCCP with worklist algorithm.

  Extends single-block SCCP to handle control flow across basic blocks
  using a worklist-driven fixed-point iteration. Abstract values flow
  along CFG edges and are joined at merge points.

  Key components:
  - BlockAbsState: per-block abstract input/output environments
  - SCCPState: global worklist state
  - sccpStep: one worklist iteration
  - sccpWorklist: fuel-bounded fixed point
  - sccpMultiApply: transform function using computed abstract state
-/
import MoltTIR.Passes.SCCP
import MoltTIR.CFG

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Per-block abstract state
-- ══════════════════════════════════════════════════════════════════

/-- Per-block abstract state: input environment (from predecessors)
    and output environment (after executing the block's instructions). -/
structure BlockAbsState where
  inEnv  : AbsEnv
  outEnv : AbsEnv

/-- Default block state: all unknown (optimistic). -/
def BlockAbsState.default : BlockAbsState :=
  { inEnv := AbsEnv.top, outEnv := AbsEnv.top }

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Global SCCP state
-- ══════════════════════════════════════════════════════════════════

/-- Map from labels to per-block abstract states. -/
abbrev BlockStateMap := Label → BlockAbsState

/-- Global SCCP worklist state. -/
structure SCCPState where
  blockStates : BlockStateMap
  worklist    : List Label

/-- Default SCCP state: all blocks unknown, worklist contains entry. -/
def SCCPState.init (f : Func) : SCCPState :=
  { blockStates := fun _ => BlockAbsState.default
    worklist := [f.entry] }

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Abstract environment join (pointwise)
-- ══════════════════════════════════════════════════════════════════

/-- Pointwise join of two abstract environments. -/
def absEnvJoin (σ₁ σ₂ : AbsEnv) : AbsEnv :=
  fun x => AbsVal.join (σ₁ x) (σ₂ x)

/-- Join is commutative (pointwise). -/
theorem absEnvJoin_comm (σ₁ σ₂ : AbsEnv) :
    absEnvJoin σ₁ σ₂ = absEnvJoin σ₂ σ₁ := by
  funext x; exact AbsVal.join_comm (σ₁ x) (σ₂ x)

/-- Join with top is identity (since top = all-unknown = ⊥). -/
theorem absEnvJoin_top_left (σ : AbsEnv) :
    absEnvJoin AbsEnv.top σ = σ := by
  funext x; simp [absEnvJoin, AbsEnv.top, AbsVal.join]

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Abstract transfer function
-- ══════════════════════════════════════════════════════════════════

/-- Process one instruction abstractly: evaluate RHS, update abs env. -/
def absExecInstr (σ : AbsEnv) (i : Instr) : AbsEnv :=
  σ.set i.dst (absEvalExpr σ i.rhs)

/-- Process a list of instructions abstractly. -/
def absExecInstrs : AbsEnv → List Instr → AbsEnv
  | σ, [] => σ
  | σ, i :: rest => absExecInstrs (absExecInstr σ i) rest

/-- Abstract transfer function for a block: execute all instructions. -/
def absTransfer (σ : AbsEnv) (b : Block) : AbsEnv :=
  absExecInstrs σ b.instrs

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Worklist step
-- ══════════════════════════════════════════════════════════════════

/-- Update block state map at a label. -/
def BlockStateMap.set (m : BlockStateMap) (lbl : Label) (s : BlockAbsState) : BlockStateMap :=
  fun l => if l = lbl then s else m l

/-- One worklist step: pop a block, compute its transfer, propagate to successors.
    Returns updated state. If worklist is empty, state is unchanged. -/
def sccpStep (f : Func) (st : SCCPState) : SCCPState :=
  match st.worklist with
  | [] => st
  | lbl :: rest =>
    match f.blocks lbl with
    | none =>
      -- Block not found: just remove from worklist
      { st with worklist := rest }
    | some blk =>
      let inEnv := st.blockStates lbl |>.inEnv
      let outEnv := absTransfer inEnv blk
      let newBlockState := { inEnv := inEnv, outEnv := outEnv }
      let blockStates' := st.blockStates.set lbl newBlockState
      -- Propagate to successors: join output with successor's input
      let succs := termSuccessors blk.term
      let (blockStates'', newWork) := succs.foldl (fun (acc : BlockStateMap × List Label) succ =>
        let (bsm, wl) := acc
        let oldIn := bsm succ |>.inEnv
        let newIn := absEnvJoin oldIn outEnv
        -- Only add to worklist if the join changed the input
        -- (simplified: always add for soundness with fuel)
        let bsm' := bsm.set succ { (bsm succ) with inEnv := newIn }
        (bsm', succ :: wl)
      ) (blockStates', [])
      { blockStates := blockStates''
        worklist := newWork ++ rest }

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Fuel-bounded fixed point
-- ══════════════════════════════════════════════════════════════════

/-- Run the worklist algorithm for at most `fuel` steps. -/
def sccpWorklist (f : Func) (fuel : Nat) : SCCPState :=
  match fuel with
  | 0 => SCCPState.init f
  | fuel' + 1 =>
    let st := sccpWorklist f fuel'
    if st.worklist.isEmpty then st
    else sccpStep f st

/-- At fuel 0, the state is the initial state. -/
theorem sccpWorklist_zero (f : Func) :
    sccpWorklist f 0 = SCCPState.init f := rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Apply multi-block SCCP result to transform function
-- ══════════════════════════════════════════════════════════════════

/-- Transform a block using the computed abstract input environment. -/
def sccpMultiBlock (σ : AbsEnv) (b : Block) : Block :=
  let (_, instrs') := sccpInstrs σ b.instrs
  { b with instrs := instrs' }

/-- Apply multi-block SCCP to a function using final worklist state. -/
def sccpMultiApply (f : Func) (st : SCCPState) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) =>
      let σ := st.blockStates lbl |>.inEnv
      (lbl, sccpMultiBlock σ blk) }

/-- Full multi-block SCCP pipeline: compute fixed point then apply. -/
def sccpMultiFunc (f : Func) (fuel : Nat) : Func :=
  sccpMultiApply f (sccpWorklist f fuel)

end MoltTIR
