/-
  MoltTIR.Passes.GuardHoist — redundant guard elimination on TIR.

  Guards are runtime type/value checks that protect operations (e.g.,
  "check x is int before int_add"). When a guard's condition is provably
  true from a dominating check, the redundant guard can be eliminated.

  In Molt's midend pipeline, this corresponds to
  `_eliminate_redundant_guards_cfg`: it walks the CFG in dominator-tree
  order and removes guards whose conditions are implied by earlier guards
  that dominate them.

  Model: a guard is an instruction whose RHS is a guard expression
  (GuardExpr). If the same guard expression appears in a dominating
  block, the redundant guard is replaced with a no-op (its RHS becomes
  the guarded variable itself, i.e., identity assignment).
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Guard expressions
-- ══════════════════════════════════════════════════════════════════

/-- A guard expression: type-check or value-check on a variable.
    In the real compiler these are `type_guard`, `bound_guard`, etc. -/
structure GuardExpr where
  guardedVar : Var
  guardKind  : Nat   -- abstract guard-kind tag (type guard = 0, bound guard = 1, etc.)
  deriving DecidableEq, Repr

/-- A proven guard: a guard expression that has been verified at a dominating point. -/
abbrev ProvenGuards := List GuardExpr

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Guard elimination on instructions
-- ══════════════════════════════════════════════════════════════════

/-- Check whether an instruction is a guard and, if so, extract its guard expr.
    In the simplified model, a guard instruction has an RHS of the form
    `un (guardOp) (var x)` where guardOp maps to a GuardExpr.
    We model this abstractly: if the instruction is a guard, return Some. -/
def instrGuardExpr : Instr → Option GuardExpr
  | { dst := _, rhs := .un .not (.var x) } =>
      -- Model: `not (var x)` represents a type guard on x
      some { guardedVar := x, guardKind := 0 }
  | _ => none

/-- Check if a guard is already proven (redundant). -/
def isGuardProven (proven : ProvenGuards) (g : GuardExpr) : Bool :=
  proven.any fun p => p == g

/-- Eliminate a redundant guard: replace RHS with identity (var dst).
    A non-redundant guard is kept and added to the proven set. -/
def guardHoistInstr (proven : ProvenGuards) (i : Instr) :
    Instr × ProvenGuards :=
  match instrGuardExpr i with
  | none => (i, proven)
  | some g =>
      if isGuardProven proven g then
        -- Redundant: replace with constant true (guard is known to pass)
        ({ i with rhs := .val (.bool true) }, proven)
      else
        -- First occurrence: keep guard, add to proven set
        (i, g :: proven)

/-- Eliminate redundant guards in an instruction list, threading proven set. -/
def guardHoistInstrs : ProvenGuards → List Instr → List Instr
  | _, [] => []
  | proven, i :: rest =>
      let (i', proven') := guardHoistInstr proven i
      i' :: guardHoistInstrs proven' rest

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Block and function-level guard hoisting
-- ══════════════════════════════════════════════════════════════════

/-- Collect proven guards from a block's instructions. -/
def collectBlockGuards : ProvenGuards → List Instr → ProvenGuards
  | proven, [] => proven
  | proven, i :: rest =>
      match instrGuardExpr i with
      | none => collectBlockGuards proven rest
      | some g => collectBlockGuards (g :: proven) rest

/-- Apply guard hoisting to a single block. -/
def guardHoistBlock (proven : ProvenGuards) (b : Block) : Block :=
  { b with instrs := guardHoistInstrs proven b.instrs }

/-- Apply guard hoisting to a function.
    In the full compiler, this walks the dominator tree so that guards
    proven in a dominating block are propagated to dominated blocks.
    In this simplified model, we process each block independently. -/
def guardHoistFunc (f : Func) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) =>
      (lbl, guardHoistBlock [] blk) }

end MoltTIR
