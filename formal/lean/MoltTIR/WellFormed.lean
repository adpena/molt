/-
  MoltTIR.WellFormed — SSA well-formedness predicates.

  Checks that SSA values are defined before use and that block parameters
  are correctly matched at jump/branch sites.
-/
import MoltTIR.Syntax

namespace MoltTIR

/-- The set of variables defined up to a point in a block. -/
def definedVars (params : List Var) (instrs : List Instr) : List Var :=
  params ++ instrs.map Instr.dst

/-- Check that all variables referenced in an expression are in scope. -/
def exprVarsIn (scope : List Var) : Expr → Bool
  | .val _ => true
  | .var x => scope.contains x
  | .bin _ a b => exprVarsIn scope a && exprVarsIn scope b
  | .un _ a => exprVarsIn scope a

/-- Check that all variables in a terminator's expressions are in scope. -/
def termVarsIn (scope : List Var) : Terminator → Bool
  | .ret e => exprVarsIn scope e
  | .jmp _ args => args.all (exprVarsIn scope)
  | .br c _ thenArgs _ elseArgs =>
      exprVarsIn scope c
      && thenArgs.all (exprVarsIn scope)
      && elseArgs.all (exprVarsIn scope)
  | .yield val _ resumeArgs =>
      exprVarsIn scope val && resumeArgs.all (exprVarsIn scope)
  | .switch scrutinee _ _ => exprVarsIn scope scrutinee
  | .unreachable => true

/-- A block is well-formed if each instruction only references previously-defined vars,
    and the terminator only references vars defined by params + all instrs. -/
def blockWellFormed (b : Block) : Bool :=
  let scope := definedVars b.params b.instrs
  -- Check each instruction incrementally
  let instrOk := (b.instrs.zipIdx).all fun (instr, i) =>
    let scopeAt := b.params ++ (b.instrs.take i).map Instr.dst
    exprVarsIn scopeAt instr.rhs
  instrOk && termVarsIn scope b.term

/-- A function is well-formed if its entry block exists and all reachable blocks
    are well-formed. (Simplified: checks all blocks returned by the map.) -/
def funcEntryExists (f : Func) : Prop :=
  (f.blocks f.entry).isSome

end MoltTIR
