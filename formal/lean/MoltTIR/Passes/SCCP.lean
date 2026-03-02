/-
  MoltTIR.Passes.SCCP — Sparse Conditional Constant Propagation.

  Abstract interpretation over the three-point lattice.
  When the abstract evaluation of an expression yields a known constant,
  replace it with a literal value.

  Corresponds to the SCCP pass in Molt's midend pipeline.
-/
import MoltTIR.Passes.Lattice
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

/-- Abstract evaluation of a binary operator. -/
def absEvalBinOp (op : BinOp) (a b : AbsVal) : AbsVal :=
  match a, b with
  | .known va, .known vb =>
      match evalBinOp op va vb with
      | some v => .known v
      | none => .overdefined
  | .unknown, _ => .unknown
  | _, .unknown => .unknown
  | _, _ => .overdefined

/-- Abstract evaluation of a unary operator. -/
def absEvalUnOp (op : UnOp) (a : AbsVal) : AbsVal :=
  match a with
  | .known va =>
      match evalUnOp op va with
      | some v => .known v
      | none => .overdefined
  | .unknown => .unknown
  | .overdefined => .overdefined

/-- Abstract evaluation of an expression given an abstract environment. -/
def absEvalExpr (σ : AbsEnv) : Expr → AbsVal
  | .val v => .known v
  | .var x => σ x
  | .bin op a b => absEvalBinOp op (absEvalExpr σ a) (absEvalExpr σ b)
  | .un op a => absEvalUnOp op (absEvalExpr σ a)

/-- Replace an expression with a constant if abstract eval yields known. -/
def sccpExpr (σ : AbsEnv) (e : Expr) : Expr :=
  match absEvalExpr σ e with
  | .known v => .val v
  | _ => e

/-- Run one SCCP pass over an instruction list, updating the abstract env. -/
def sccpInstrs (σ : AbsEnv) : List Instr → AbsEnv × List Instr
  | [] => (σ, [])
  | i :: rest =>
      let absRhs := absEvalExpr σ i.rhs
      let newRhs := match absRhs with
        | .known v => Expr.val v
        | _ => i.rhs
      let σ' := σ.set i.dst absRhs
      let (σ'', rest') := sccpInstrs σ' rest
      (σ'', { i with rhs := newRhs } :: rest')

/-- Apply SCCP to a block. -/
def sccpBlock (σ : AbsEnv) (b : Block) : AbsEnv × Block :=
  let (σ', instrs') := sccpInstrs σ b.instrs
  (σ', { b with instrs := instrs' })

/-- Apply SCCP to a function (single pass, entry block only). -/
def sccpFunc (f : Func) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) =>
      let (_, blk') := sccpBlock AbsEnv.top blk
      (lbl, blk') }

end MoltTIR
