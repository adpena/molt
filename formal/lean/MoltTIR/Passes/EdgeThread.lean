/-
  MoltTIR.Passes.EdgeThread — edge threading (jump threading) on TIR.

  Edge threading eliminates redundant branches by specializing control
  flow when a branch condition is known at a predecessor. If block B
  ends with `br cond L_then L_else` and a predecessor A sets cond to
  a known constant, then A's jump to B can be rewritten to jump directly
  to the appropriate successor, bypassing the branch.

  In Molt's midend pipeline, this corresponds to the SCCP + edge
  threading phase: after SCCP computes abstract values, edges where
  the branch condition is a known constant are threaded through.

  Model: given a known-constants map (from SCCP), rewrite branch
  terminators whose condition is known to a direct jump.
-/
import MoltTIR.Passes.SCCP
import MoltTIR.Passes.SCCPMulti

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Edge threading on terminators
-- ══════════════════════════════════════════════════════════════════

/-- Thread a branch terminator when the condition is a known constant.
    If the condition evaluates to a known boolean in the abstract env,
    replace the branch with a direct jump to the appropriate target. -/
def edgeThreadTerminator (σ : AbsEnv) : Terminator → Terminator
  | .br cond thenLbl thenArgs elseLbl elseArgs =>
      match absEvalExpr σ cond with
      | .known (.bool true)  => .jmp thenLbl thenArgs
      | .known (.bool false) => .jmp elseLbl elseArgs
      | _ => .br cond thenLbl thenArgs elseLbl elseArgs
  | t => t

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Block and function-level edge threading
-- ══════════════════════════════════════════════════════════════════

/-- Compute the abstract environment after executing a block's instructions,
    starting from the given input abstract env. -/
def absExecBlockInstrs : AbsEnv → List Instr → AbsEnv
  | σ, [] => σ
  | σ, i :: rest => absExecBlockInstrs (σ.set i.dst (absEvalExpr σ i.rhs)) rest

/-- Apply edge threading to a block given its input abstract environment.
    First compute the abstract state after instructions, then thread
    the terminator. -/
def edgeThreadBlock (σ : AbsEnv) (b : Block) : Block :=
  let σ' := absExecBlockInstrs σ b.instrs
  { b with term := edgeThreadTerminator σ' b.term }

/-- Apply edge threading to a function using multi-block SCCP state.
    Each block uses the abstract input environment computed by SCCP. -/
def edgeThreadFunc (f : Func) (st : SCCPState) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) =>
      let σ := st.blockStates lbl |>.inEnv
      (lbl, edgeThreadBlock σ blk) }

/-- Full edge threading pipeline: run SCCP then thread edges. -/
def edgeThreadPipeline (f : Func) (fuel : Nat) : Func :=
  let st := sccpWorklist f fuel
  edgeThreadFunc f st

end MoltTIR
