/-
  MoltTIR.Passes.DCE — dead code elimination pass on TIR.

  Removes instructions whose destination variable is not used by any
  subsequent instruction or the block's terminator. This is a conservative,
  instruction-level DCE — it does not remove entire blocks.

  Corresponds to the DCE pass in Molt's midend pipeline
  (SimpleTIRGenerator._run_ir_midend_passes → _dce_pass).
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

/-- Collect all variables used by a suffix of the instruction list and the terminator. -/
def usedVarsSuffix (instrs : List Instr) (term : Terminator) : List Var :=
  instrs.flatMap (fun i => exprVars i.rhs) ++ termVars term

/-- An instruction is live if its destination appears in the used-variables set. -/
def isLive (used : List Var) (i : Instr) : Bool :=
  used.contains i.dst

/-- Remove dead instructions: keep only those whose dst is in `used`. -/
def dceInstrs (used : List Var) (instrs : List Instr) : List Instr :=
  instrs.filter (isLive used)

/-- Compute the set of variables used by subsequent instructions + terminator,
    then filter out dead instructions. -/
def dceBlock (b : Block) : Block :=
  let used := usedVarsSuffix b.instrs b.term
  { b with instrs := dceInstrs used b.instrs }

/-- Apply DCE to all blocks in a function. -/
def dceFunc (f : Func) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) => (lbl, dceBlock blk) }

end MoltTIR
