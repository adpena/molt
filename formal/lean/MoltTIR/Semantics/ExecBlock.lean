/-
  MoltTIR.Semantics.ExecBlock — block-level execution: instructions + terminator.
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

/-- Execute a list of SSA instructions, threading the environment. -/
def execInstrs (ρ : Env) : List Instr → Option Env
  | [] => some ρ
  | i :: rest =>
      match evalExpr ρ i.rhs with
      | none => none
      | some v => execInstrs (ρ.set i.dst v) rest

/-- Evaluate a list of expressions to a list of values. -/
def evalArgs (ρ : Env) : List Expr → Option (List Value)
  | [] => some []
  | e :: es =>
      match evalExpr ρ e with
      | none => none
      | some v =>
          match evalArgs ρ es with
          | none => none
          | some vs => some (v :: vs)

/-- Bind block parameters to argument values, producing a fresh environment. -/
def bindParams : List Var → List Value → Option Env
  | [], [] => some Env.empty
  | p :: ps, v :: vs =>
      match bindParams ps vs with
      | none => none
      | some ρ => some (ρ.set p v)
  | _, _ => none  -- length mismatch

/-- Result of evaluating a terminator: either a return value or a jump target.
    Note: Env is a function type, so we don't derive Repr. -/
inductive TermResult where
  | ret (v : Value)
  | jump (target : Label) (env : Env)

/-- Compute the target label for a switch terminator given the scrutinee value. -/
def switchTarget (cases_ : List (Int × Label)) (default_ : Label) (n : Int) : Label :=
  match cases_.find? (fun p => p.1 == n) with
  | some (_, lbl) => lbl
  | none => default_

def evalTerminator (f : Func) (ρ : Env) : Terminator → Option TermResult
  | .ret e =>
      match evalExpr ρ e with
      | some v => some (.ret v)
      | none => none
  | .jmp target args =>
      match evalArgs ρ args with
      | none => none
      | some vals =>
          match f.blocks target with
          | none => none
          | some blk =>
              match bindParams blk.params vals with
              | none => none
              | some ρ' => some (.jump target ρ')
  | .br cond thenLbl thenArgs elseLbl elseArgs =>
      match evalExpr ρ cond with
      | some (.bool true) =>
          match evalArgs ρ thenArgs with
          | none => none
          | some vals =>
              match f.blocks thenLbl with
              | none => none
              | some blk =>
                  match bindParams blk.params vals with
                  | none => none
                  | some ρ' => some (.jump thenLbl ρ')
      | some (.bool false) =>
          match evalArgs ρ elseArgs with
          | none => none
          | some vals =>
              match f.blocks elseLbl with
              | none => none
              | some blk =>
                  match bindParams blk.params vals with
                  | none => none
                  | some ρ' => some (.jump elseLbl ρ')
      | _ => none
  | .yield _ _ _ => none   -- generators not modeled in formal semantics
  | .switch scrutinee cases default_ =>
      match evalExpr ρ scrutinee with
      | some (.int n) =>
          match f.blocks (switchTarget cases default_ n) with
          | none => none
          | some blk =>
              match bindParams blk.params [] with
              | none => none
              | some ρ' => some (.jump (switchTarget cases default_ n) ρ')
      | _ => none
  | .unreachable => none

end MoltTIR
