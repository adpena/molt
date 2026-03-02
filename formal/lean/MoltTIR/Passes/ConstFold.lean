/-
  MoltTIR.Passes.ConstFold — constant folding pass on TIR expressions.

  Folds constant sub-expressions at compile time. This is the first
  compiler pass with a machine-checked correctness proof.

  Corresponds to the SCCP pass in Molt's midend pipeline
  (SimpleTIRGenerator._run_ir_midend_passes).
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

/-- Fold constant expressions. Recursively evaluates sub-expressions that
    are fully constant (no variable references). -/
def constFoldExpr : Expr → Expr
  | .val v => .val v
  | .var x => .var x
  | .bin op a b =>
      let a' := constFoldExpr a
      let b' := constFoldExpr b
      match a', b' with
      | .val va, .val vb =>
          match evalBinOp op va vb with
          | some v => .val v
          | none => .bin op a' b'
      | _, _ => .bin op a' b'
  | .un op a =>
      let a' := constFoldExpr a
      match a' with
      | .val va =>
          match evalUnOp op va with
          | some v => .val v
          | none => .un op a'
      | _ => .un op a'

/-- Apply constant folding to an instruction. -/
def constFoldInstr (i : Instr) : Instr :=
  { i with rhs := constFoldExpr i.rhs }

/-- Apply constant folding to a terminator's expressions. -/
def constFoldTerminator : Terminator → Terminator
  | .ret e => .ret (constFoldExpr e)
  | .jmp target args => .jmp target (args.map constFoldExpr)
  | .br cond tl ta el ea =>
      .br (constFoldExpr cond) tl (ta.map constFoldExpr) el (ea.map constFoldExpr)

/-- Apply constant folding to a block. -/
def constFoldBlock (b : Block) : Block :=
  { b with
    instrs := b.instrs.map constFoldInstr
    term := constFoldTerminator b.term }

/-- Apply constant folding to a function (all blocks via blockList). -/
def constFoldFunc (f : Func) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) => (lbl, constFoldBlock blk) }

end MoltTIR
