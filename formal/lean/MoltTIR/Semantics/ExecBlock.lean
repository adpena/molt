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

/-- Evaluate a terminator given the post-instruction environment and the function
    (needed to look up target block params). -/
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
          let target := match cases.find? (fun p => p.1 == n) with
            | some (_, lbl) => lbl
            | none => default_
          match f.blocks target with
          | none => none
          | some blk =>
              match bindParams blk.params [] with
              | none => none
              | some ρ' => some (.jump target ρ')
      | _ => none
  | .unreachable => none

end MoltTIR
