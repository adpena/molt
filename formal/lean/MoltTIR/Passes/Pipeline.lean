/-
  MoltTIR.Passes.Pipeline — composition of verified compiler passes.

  Composes constant folding and SCCP into a pipeline and proves the
  end-to-end correctness theorem: the composed transformation preserves
  expression semantics.

  Corresponds to the midend pass pipeline in Molt's compiler
  (SimpleTIRGenerator._run_ir_midend_passes).
-/
import MoltTIR.Passes.ConstFoldCorrect
import MoltTIR.Passes.SCCPCorrect

namespace MoltTIR

/-- Compose constant folding then SCCP on an expression. -/
def pipelineExpr (σ : AbsEnv) (e : Expr) : Expr :=
  sccpExpr σ (constFoldExpr e)

/-- Pipeline correctness: constant folding followed by SCCP preserves semantics.

    This is the main end-to-end theorem. It chains the individual pass
    correctness results via transitivity:
      evalExpr ρ (sccpExpr σ (constFoldExpr e))
    = evalExpr ρ (constFoldExpr e)         -- by sccpExpr_correct
    = evalExpr ρ e                          -- by constFoldExpr_correct -/
theorem pipelineExpr_correct (σ : AbsEnv) (ρ : Env) (e : Expr)
    (hsound : AbsEnvStrongSound σ ρ) :
    evalExpr ρ (pipelineExpr σ e) = evalExpr ρ e := by
  simp only [pipelineExpr]
  rw [sccpExpr_correct σ ρ (constFoldExpr e) hsound]
  exact constFoldExpr_correct ρ e

/-- Pipeline with top (all-unknown) abstract env is always sound. -/
theorem pipelineExpr_top_correct (ρ : Env) (e : Expr) :
    evalExpr ρ (pipelineExpr AbsEnv.top e) = evalExpr ρ e :=
  pipelineExpr_correct AbsEnv.top ρ e (absEnvTop_strongSound ρ)

/-- Compose constant folding then SCCP on an instruction list. -/
def pipelineInstrs (σ : AbsEnv) (instrs : List Instr) : AbsEnv × List Instr :=
  sccpInstrs σ (instrs.map constFoldInstr)

/-- Compose constant folding then SCCP on a block. -/
def pipelineBlock (σ : AbsEnv) (b : Block) : AbsEnv × Block :=
  sccpBlock σ (constFoldBlock b)

/-- Apply the full pipeline to a function. -/
def pipelineFunc (f : Func) : Func :=
  sccpFunc (constFoldFunc f)

end MoltTIR
